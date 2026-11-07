//! Running zypper transactions (install/remove/update/dup/patch) and refreshes.
//!
//! Current zypper refuses to read prompt answers unless it's attached to a
//! real terminal, so we never try to answer its prompts. Instead:
//!
//!  1. run `zypper --non-interactive <cmd> --dry-run` to get the plan,
//!  2. render it and ask for confirmation ourselves,
//!  3. run `zypper --non-interactive --no-refresh <cmd>` to do the work,
//!     turning its `--xmlout` progress stream into a uv-style progress bar.

use std::io::{BufReader, IsTerminal, Read, Write};
use std::time::{Duration, Instant};

use anyhow::Result;
use console::style;
use indicatif::ProgressBar;

use crate::ui::{self, Change};
use crate::xml::{Node, StreamReader};
use crate::zypper::{self, exit};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Transaction,
    Refresh,
}

pub struct Options {
    pub yes: bool,
    pub dry_run: bool,
    pub verbose: bool,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub change: Change,
    pub name: String,
    pub kind: String,
    pub edition: String,
    pub edition_old: Option<String>,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub items: Vec<Item>,
    pub download: i64,
    pub space_diff: i64,
    pub need_reboot: bool,
}

impl Plan {
    fn from_node(n: &Node) -> Plan {
        let mut items = Vec::new();
        for group in &n.children {
            let Some(change) = Change::from_tag(&group.name) else {
                continue;
            };
            for s in group.children_named("solvable") {
                items.push(Item {
                    change,
                    name: s.attr_or("name", "?").to_string(),
                    kind: s
                        .attr("type")
                        .or(s.attr("kind"))
                        .unwrap_or("package")
                        .to_string(),
                    edition: s.attr_or("edition", "").to_string(),
                    edition_old: s.attr("edition-old").map(str::to_string),
                });
            }
        }
        items.sort_by(|a, b| (a.change, &a.name).cmp(&(b.change, &b.name)));
        let num = |k: &str| n.attr(k).and_then(|v| v.parse().ok()).unwrap_or(0);
        Plan {
            items,
            download: num("download-size"),
            space_diff: num("space-usage-diff"),
            need_reboot: n.attr("need-reboot") == Some("1") || n.attr("need-reboot") == Some("true"),
        }
    }

    fn count(&self, c: Change) -> usize {
        self.items.iter().filter(|i| i.change == c).count()
    }

    fn breakdown(&self, future: bool) -> String {
        let order = [
            Change::Install,
            Change::Upgrade,
            Change::Downgrade,
            Change::Reinstall,
            Change::ArchChange,
            Change::Remove,
        ];
        order
            .iter()
            .filter_map(|&c| {
                let n = self.count(c);
                (n > 0).then(|| {
                    if future {
                        format!("{} to {}", style(n).bold(), c.future())
                    } else {
                        format!("{} {}", style(n).bold(), c.past())
                    }
                })
            })
            .collect::<Vec<_>>()
            .join(&ui::dim(" · "))
    }

    fn render(&self) {
        let width = self
            .items
            .iter()
            .map(|i| display_name(i).chars().count())
            .max()
            .unwrap_or(0)
            .min(44);
        for item in &self.items {
            let name = display_name(item);
            let pad = " ".repeat(width.saturating_sub(name.chars().count()));
            let name_s = if item.change == Change::Remove {
                style(&name).red().to_string()
            } else {
                style(&name).bold().to_string()
            };
            let ver = match (&item.edition_old, item.change) {
                (Some(old), Change::Upgrade | Change::Downgrade) => {
                    let (o, n) = ui::version_diff(old, &item.edition);
                    format!("{o} {} {n}", ui::dim("→"))
                }
                _ => ui::dim(&item.edition),
            };
            ui::eprint_line(format!(" {} {name_s}{pad} {ver}", ui::marker(item.change)));
        }

        let mut parts = vec![self.breakdown(true)];
        if self.download > 0 {
            parts.push(format!("{} to download", style(ui::bytes(self.download)).bold()));
        }
        if self.space_diff != 0 {
            let sign = if self.space_diff > 0 { "+" } else { "" };
            parts.push(format!(
                "{} disk",
                style(format!("{sign}{}", ui::bytes(self.space_diff))).bold()
            ));
        }
        ui::eprint_line("");
        ui::eprint_line(format!("  {}", parts.join(&ui::dim(" · "))));
        if self.need_reboot {
            ui::warning("this transaction will require a reboot");
        }
    }
}

fn display_name(i: &Item) -> String {
    if i.kind != "package" && !i.kind.is_empty() {
        format!("{}:{}", i.kind, i.name)
    } else {
        i.name.clone()
    }
}

const VERBS: [&str; 3] = ["Downloading", "Installing", "Removing"];

#[derive(Default, Clone, Copy)]
struct Stat {
    count: usize,
    time: Duration,
}

/// The single progress bar used while zypper commits. zypper interleaves
/// downloads and installs ("Retrieving 1/8", "Installing 1/8", ...), so one
/// bar switches its label and counter rather than starting new bars.
struct Progress {
    bar: ProgressBar,
    verb: usize,
    since: Instant,
}

/// What one zypper run produced.
#[derive(Default)]
struct Outcome {
    code: i32,
    plan: Option<Plan>,
    nothing_to_do: bool,
    errors: usize,
    problems: Vec<String>,
    stderr: String,
    /// Other output (rpm scriptlets etc.), shown on failure or with -v.
    log: Vec<String>,
}

struct Session<'a> {
    mode: Mode,
    opts: &'a Options,
    committing: bool,
    spinner: Option<ProgressBar>,
    progress: Option<Progress>,
    stats: [Stat; 3],
    preloaded: usize,
    preload_started: Option<Instant>,
    repos_refreshed: usize,
    out: Outcome,
}

impl<'a> Session<'a> {
    fn new(mode: Mode, opts: &'a Options) -> Self {
        Session {
            mode,
            opts,
            committing: false,
            spinner: None,
            progress: None,
            stats: [Stat::default(); 3],
            preloaded: 0,
            preload_started: None,
            repos_refreshed: 0,
            out: Outcome::default(),
        }
    }

    fn println(&self, s: impl AsRef<str>) {
        let active = self.progress.as_ref().map(|p| &p.bar).or(self.spinner.as_ref());
        match active {
            Some(pb) if !pb.is_hidden() => pb.println(s.as_ref()),
            _ => ui::eprint_line(s),
        }
    }

    fn set_status(&mut self, msg: &str) {
        let msg = msg.lines().next().unwrap_or("").trim().to_string();
        if self.progress.is_some() {
            return; // the bar's message shows the current package
        }
        match &self.spinner {
            Some(sp) => sp.set_message(msg),
            None => self.spinner = Some(ui::spinner(msg)),
        }
    }

    fn clear(&mut self) {
        if let Some(sp) = self.spinner.take() {
            sp.finish_and_clear();
        }
        if let Some(p) = self.progress.take() {
            self.stats[p.verb].time += p.since.elapsed();
            p.bar.finish_and_clear();
        }
    }

    /// Run one zypper process to completion, rendering as we go.
    fn exec(&mut self, sudo: bool, global: &[&str], args: &[String]) -> Result<()> {
        let mut child = zypper::spawn_xml(sudo, global, args)?;
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let err_thread = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            s
        });
        let log = std::env::var_os("PZYP_LOG").and_then(|p| {
            std::fs::OpenOptions::new().create(true).append(true).open(p).ok()
        });
        let mut reader = StreamReader::new(BufReader::new(Tee { inner: stdout, log }));
        while let Ok(Some(node)) = reader.next_node() {
            self.handle(&node);
        }
        let status = child.wait()?;
        self.out.stderr = err_thread.join().unwrap_or_default();
        self.out.code = status.code().unwrap_or(exit::ON_SIGNAL);
        Ok(())
    }

    fn handle(&mut self, node: &Node) {
        match node.name.as_str() {
            "message" => self.on_message(node),
            "install-summary" => self.out.plan = Some(Plan::from_node(node)),
            "prompt" => self.on_prompt(node),
            "progress" => self.on_progress(node),
            "download" => self.on_download(node),
            _ => {
                // e.g. <fileconflict-summary>: surface any nested errors.
                let mut msgs = Vec::new();
                node.descendants("message", &mut msgs);
                for m in msgs {
                    if m.attr("type") == Some("error") {
                        self.on_message(m);
                    }
                }
            }
        }
    }

    fn on_message(&mut self, n: &Node) {
        let text = n.text.trim();
        if text.is_empty() {
            return;
        }
        match n.attr("type") {
            Some("error") => {
                self.out.errors += 1;
                self.println(format!("{} {}", style("error:").red().bold(), text));
            }
            Some("warning") => {
                self.println(format!("{} {}", style("warning:").yellow().bold(), text));
            }
            _ => self.on_info(text),
        }
    }

    fn on_info(&mut self, text: &str) {
        if self.opts.verbose {
            self.println(ui::dim(text));
        }
        if self.observe_label(text) {
            return;
        }
        // Parallel downloads: "Preloading: foo-1.0.x86_64.rpm [done]"
        // (or "[Error: ..., trying next mirror.]", which zypper recovers from).
        if let Some(rest) = text.strip_prefix("Preloading: ") {
            if rest.ends_with("[done]") {
                self.preloaded += 1;
                let file = rest.trim_end_matches("[done]").trim();
                self.set_status(&format!("Downloaded {file}"));
            }
            return;
        }
        if text.starts_with("Nothing to do") {
            self.out.nothing_to_do = true;
            return;
        }
        if let Some(name) = text.strip_suffix(" is already installed.") {
            let name = name.trim_matches(|c| c == '\'' || c == '"');
            self.println(format!(
                " {} {} {}",
                style("✓").green().bold(),
                style(name).bold(),
                ui::dim("is already installed")
            ));
            return;
        }
        if text.starts_with("No update candidate") || text.contains("is not installed") {
            self.println(format!(" {} {}", ui::dim("·"), text));
            return;
        }
        if self.mode == Mode::Refresh
            && let Some(repo) = quoted(text)
        {
            if text.ends_with("is up to date.") {
                self.repos_refreshed += 1;
                self.println(format!(" {} {repo} {}", style("✓").green().bold(), ui::dim("up to date")));
                return;
            }
            if text.starts_with("Building repository") {
                self.repos_refreshed += 1;
                self.println(format!(" {} {repo} {}", style("↓").cyan().bold(), style("updated").green()));
            }
        }
        if self.committing {
            self.out.log.push(text.to_string());
        } else {
            self.set_status(text);
        }
    }

    /// zypper auto-answers prompts in --non-interactive mode, but solver
    /// problems arrive as a prompt description; keep them for the report.
    fn on_prompt(&mut self, n: &Node) {
        if let Some(desc) = n.child_text("description")
            && desc.contains("Problem")
        {
            self.out.problems.push(desc.trim_end().to_string());
        }
    }

    fn on_progress(&mut self, n: &Node) {
        let name = n.attr_or("name", "");
        if name.is_empty() || self.observe_label(name) {
            return;
        }
        if n.attr("done").is_some() && !name.starts_with("Preloading") {
            return;
        }
        // "Preloading Packages: [ (3.1 MiB / 4.5 MiB) (2.5 KiB/s)]"
        if let Some(rest) = name.strip_prefix("Preloading Packages") {
            self.preload_started.get_or_insert_with(Instant::now);
            if n.attr("done").is_some() {
                return;
            }
            let detail = rest
                .trim_start_matches(':')
                .trim()
                .trim_matches(|c| c == '[' || c == ']')
                .replace(") (", " · ")
                .replace(['(', ')'], "");
            let detail = detail.trim();
            let msg = if detail.is_empty() {
                "Downloading packages…".to_string()
            } else {
                format!("Downloading packages {detail}")
            };
            self.set_status(&msg);
            return;
        }
        let pct = n.attr("value").map(|v| format!(" {v}%")).unwrap_or_default();
        self.set_status(&format!("{}{pct}", name.trim_end_matches(':')));
    }

    fn on_download(&mut self, n: &Node) {
        if n.attr("done").is_some() {
            return;
        }
        let rate = n
            .attr("rate")
            .and_then(|r| r.parse::<i64>().ok())
            .filter(|r| *r > 0)
            .map(|r| format!(" · {}/s", ui::bytes(r)))
            .unwrap_or_default();
        let pct = n
            .attr("percent")
            .filter(|p| !p.starts_with('-'))
            .map(|p| format!(" {p}%"))
            .unwrap_or_default();
        if let Some(p) = &self.progress {
            if VERBS[p.verb] == "Downloading" {
                let base = p.bar.message();
                let base = base.split(" (").next().unwrap_or("").to_string();
                p.bar.set_message(format!("{base} ({}{rate})", pct.trim()));
            }
            return;
        }
        let url = n.attr_or("url", "");
        let file = url.rsplit('/').find(|s| !s.is_empty()).unwrap_or(url);
        self.set_status(&format!("Downloading {file}{pct}{rate}"));
    }

    /// Recognise "Retrieving: foo-1.0-1.x86_64 (repo) (3/12), 1.6 MiB" and
    /// "(3/12) Installing: foo-1.0-1.x86_64" and drive the bar from them.
    fn observe_label(&mut self, label: &str) -> bool {
        let Some((i, total)) = parse_counter(label) else {
            return false;
        };
        let verb = if label.starts_with("Retrieving") || label.contains("Downloading") {
            0
        } else if label.contains("Installing") {
            1
        } else if label.contains("Removing") {
            2
        } else {
            return false;
        };
        let thing = label
            .split_once(": ")
            .map(|(_, rest)| rest)
            .unwrap_or(label)
            .split(" (")
            .next()
            .unwrap_or("")
            .split(", ")
            .next()
            .unwrap_or("")
            .trim()
            .to_string();

        if let Some(t) = self.preload_started.take() {
            self.println(ui::dim(&format!(
                "Downloaded {} in {}",
                ui::plural(self.preloaded, "package"),
                ui::elapsed(t.elapsed())
            )));
            // Packages are now cached; "Retrieving" just reads them back.
            self.stats[0].count = usize::MAX;
        }
        if verb == 0 && self.stats[0].count == usize::MAX {
            return true;
        }
        if let Some(sp) = self.spinner.take() {
            sp.finish_and_clear();
        }
        let p = self.progress.get_or_insert_with(|| Progress {
            bar: ui::bar(total as u64, VERBS[verb]),
            verb,
            since: Instant::now(),
        });
        if p.verb != verb {
            self.stats[p.verb].time += p.since.elapsed();
            p.verb = verb;
            p.since = Instant::now();
            p.bar.set_prefix(VERBS[verb]);
        }
        p.bar.set_length(total as u64);
        p.bar.set_position(i.saturating_sub(1) as u64);
        p.bar.set_message(thing);
        let st = &mut self.stats[verb];
        if st.count != usize::MAX {
            st.count = st.count.max(i);
        }
        true
    }

    fn print_stats(&self) {
        for (verb, past) in [(0, "Downloaded"), (1, "Installed"), (2, "Removed")] {
            let st = self.stats[verb];
            if st.count > 0 && st.count != usize::MAX {
                ui::status(format!(
                    "{past} {} in {}",
                    ui::plural(st.count, "package"),
                    ui::elapsed(st.time)
                ));
            }
        }
    }

    /// Print everything we know about a failed run.
    fn report_failure(&self, args: &[String]) {
        for p in &self.out.problems {
            ui::error("dependency problem");
            for line in p.lines() {
                let t = line.trim_start();
                let styled = if t.starts_with("Problem") {
                    style(line).bold().to_string()
                } else if t.starts_with("Solution ") {
                    style(line).cyan().to_string()
                } else {
                    ui::dim(line)
                };
                ui::eprint_line(format!("  {styled}"));
            }
        }
        if self.out.errors == 0 && self.out.problems.is_empty() {
            let e = self.out.stderr.trim();
            if e.is_empty() {
                ui::error(format!("zypper exited with status {}", self.out.code));
            } else {
                ui::error(e);
            }
        }
        if !self.out.log.is_empty() && !self.opts.verbose {
            ui::eprint_line(ui::dim("zypper output:"));
            for l in &self.out.log {
                for line in l.lines() {
                    ui::eprint_line(format!("  {}", ui::dim(line)));
                }
            }
        }
        let first_pkg = args.iter().skip(1).find(|s| !s.starts_with('-'));
        let interactive = format!("sudo zypper {}", args.join(" "));
        match self.out.code {
            exit::INF_CAP_NOT_FOUND => {
                if let Some(q) = first_pkg {
                    ui::hint(format!("search for it with {}", ui::cmd(&format!("pzyp search {q}"))));
                }
            }
            exit::INF_RPM_SCRIPT_FAILED => {
                ui::warning("the transaction finished, but some package scripts failed")
            }
            _ if !self.out.problems.is_empty() => ui::hint(format!(
                "run {} to pick a solution interactively",
                ui::cmd(&interactive)
            )),
            _ => {
                let all = self.out.log.join("\n") + &self.out.stderr;
                if all.contains("license") || self.out.errors > 0 && all.contains("--auto-agree-with-licenses") {
                    ui::hint(format!("pass {} to accept package licenses", ui::cmd("--agree-licenses")));
                }
            }
        }
    }
}

fn post_hints(code: i32) {
    match code {
        exit::INF_REBOOT_NEEDED => ui::hint("a reboot is required to complete this update"),
        exit::INF_RESTART_NEEDED => ui::hint(format!(
            "the package manager updated itself; run {} again to continue",
            ui::cmd("pzyp")
        )),
        exit::INF_REPOS_SKIPPED => ui::warning("some repositories were skipped due to errors"),
        _ => {}
    }
}

/// Copies zypper's raw XML to `$PZYP_LOG` (if set) while we parse it.
struct Tee<R> {
    inner: R,
    log: Option<std::fs::File>,
}

impl<R: Read> Read for Tee<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if let Some(f) = self.log.as_mut() {
            let _ = f.write_all(&buf[..n]);
        }
        Ok(n)
    }
}

/// Find "(i/n)" in a label.
fn parse_counter(s: &str) -> Option<(usize, usize)> {
    let mut start = 0;
    while let Some(off) = s[start..].find('(') {
        let open = start + off;
        if let Some(close_off) = s[open..].find(')') {
            let inner = &s[open + 1..open + close_off];
            if let Some((a, b)) = inner.split_once('/')
                && let (Ok(a), Ok(b)) = (a.trim().parse(), b.trim().parse())
            {
                return Some((a, b));
            }
        }
        start = open + 1;
    }
    None
}

/// Extract the text between the first pair of single quotes.
fn quoted(s: &str) -> Option<&str> {
    let a = s.find('\'')?;
    let b = s[a + 1..].find('\'')?;
    Some(&s[a + 1..a + 1 + b])
}

pub fn ask_yes_no(question: &str, default: bool) -> Result<bool> {
    let tty = std::io::stdin().is_terminal();
    let mut term = ui::stderr();
    let hint = if default { "[Y/n]" } else { "[y/N]" };
    write!(term, "{} {} {} ", style("?").cyan().bold(), style(question).bold(), ui::dim(hint))?;
    term.flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if !tty {
        // Piped input: EOF means "no", never silently proceed.
        let _ = term.write_line(line.trim());
        if line.is_empty() {
            return Ok(false);
        }
    }
    let a = line.trim().to_lowercase();
    Ok(if a.is_empty() { default } else { a == "y" || a == "yes" })
}

/// Run a zypper command (`subcommand args...`) as root, rendering everything.
pub fn run(mode: Mode, args: Vec<String>, opts: Options) -> Result<i32> {
    let sudo = zypper::ensure_sudo()?;
    let started = Instant::now();
    let mut s = Session::new(mode, &opts);

    if mode == Mode::Refresh {
        s.set_status("Refreshing repositories…");
        s.exec(sudo, &["--non-interactive"], &args)?;
        s.clear();
        if !exit::is_success(s.out.code) {
            s.report_failure(&args);
            ui::hint(format!(
                "to import new repository keys, run {}",
                ui::cmd("sudo zypper refresh")
            ));
            return Ok(s.out.code);
        }
        let n = s.repos_refreshed;
        ui::status(format!(
            "Refreshed {n} {} in {}",
            if n == 1 { "repository" } else { "repositories" },
            ui::elapsed(started.elapsed())
        ));
        post_hints(s.out.code);
        return Ok(s.out.code);
    }

    // 1. Resolve.
    let mut resolve = args.clone();
    resolve.insert(1, "--dry-run".into());
    s.set_status("Resolving dependencies…");
    s.exec(sudo, &["--non-interactive"], &resolve)?;
    s.clear();
    if !exit::is_success(s.out.code) {
        s.report_failure(&args);
        return Ok(s.out.code);
    }
    let plan = match s.out.plan.take() {
        Some(p) if !p.items.is_empty() => p,
        _ => {
            ui::status(format!("Nothing to do (checked in {})", ui::elapsed(started.elapsed())));
            return Ok(0);
        }
    };
    ui::status(format!(
        "Resolved {} in {}",
        ui::plural(plan.items.len(), "package"),
        ui::elapsed(started.elapsed())
    ));
    plan.render();

    // 2. Confirm.
    if opts.dry_run {
        ui::hint("dry run, no changes were made");
        return Ok(0);
    }
    if !opts.yes && !ask_yes_no("Proceed?", true)? {
        ui::status("Aborted, no changes were made");
        return Ok(1);
    }

    // 3. Commit.
    let commit_started = Instant::now();
    let mut s = Session::new(mode, &opts);
    s.committing = true;
    s.set_status("Preparing transaction…");
    s.exec(sudo, &["--non-interactive", "--no-refresh"], &args)?;
    s.clear();
    s.print_stats();
    if !exit::is_success(s.out.code) {
        s.report_failure(&args);
        return Ok(s.out.code);
    }
    if opts.verbose {
        for l in &s.out.log {
            ui::eprint_line(ui::dim(l));
        }
    }
    ui::eprint_line(format!(
        "{} {} {}",
        style("✓").green().bold(),
        plan.breakdown(false),
        ui::dim(&format!("in {}", ui::elapsed(commit_started.elapsed())))
    ));
    post_hints(s.out.code);
    Ok(s.out.code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters() {
        assert_eq!(parse_counter("(3/12) Installing: foo-1.0"), Some((3, 12)));
        assert_eq!(
            parse_counter("Retrieving: foo-1.0.x86_64 (Main Repository (OSS)) (3/12), 1.6 MiB"),
            Some((3, 12))
        );
        assert_eq!(parse_counter("Checking for file conflicts"), None);
    }

    #[test]
    fn quotes() {
        assert_eq!(quoted("Repository 'Main (OSS)' is up to date."), Some("Main (OSS)"));
    }

    #[test]
    fn plan_from_xml() {
        let xml = br#"<stream><install-summary download-size="2048" space-usage-diff="4096" packages-to-change="3" need-reboot="0">
<to-upgrade><solvable type="package" name="audit" edition="4.0.2-3.6" edition-old="4.0.2-3.5" arch="x86_64"/></to-upgrade>
<to-install><solvable type="package" name="ripgrep" edition="15.2.0-1.3" arch="x86_64"/></to-install>
<to-remove><solvable type="package" name="old" edition="1.0" arch="noarch"/></to-remove>
</install-summary></stream>"#;
        let nodes = crate::xml::parse_all(xml);
        let plan = Plan::from_node(&nodes[0]);
        assert_eq!(plan.items.len(), 3);
        assert_eq!(plan.items[0].change, Change::Upgrade);
        assert_eq!(plan.items[2].change, Change::Remove);
        assert_eq!(plan.download, 2048);
    }
}
