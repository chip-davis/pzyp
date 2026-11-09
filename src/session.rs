//! Running zypper transactions (install/remove/update/dup/patch) and refreshes.
//!
//! Current zypper refuses to read prompt answers unless it's attached to a
//! real terminal, so we never try to answer its prompts. Instead:
//!
//!  1. run `zypper --non-interactive <cmd> --dry-run` to get the plan,
//!  2. render it and ask for confirmation ourselves,
//!  3. run `zypper --non-interactive --no-refresh <cmd>` to do the work.
//!
//! This module parses zypper's `--xmlout` stream into semantic events and
//! hands them to the active theme's [`View`]; it prints nothing itself.

use std::io::{BufReader, Read, Write};
use std::time::Instant;

use anyhow::Result;
use console::style;

use crate::theme::{Item, MsgKind, Plan, Verb, View, ViewKind, theme};
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

pub fn plan_from_node(n: &Node) -> Plan {
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
                arch: s.attr_or("arch", "").to_string(),
                repo: s.attr_or("repository", "").to_string(),
            });
        }
    }
    items.sort_by(|a, b| (a.change, &a.name).cmp(&(b.change, &b.name)));
    let num = |k: &str| n.attr(k).and_then(|v| v.parse().ok()).unwrap_or(0);
    Plan {
        items,
        download: num("download-size"),
        space_diff: num("space-usage-diff"),
        installed_size: num("space-usage-installed"),
        removed_size: num("space-usage-removed"),
        need_reboot: matches!(n.attr("need-reboot"), Some("1") | Some("true")),
        requested: Vec::new(),
    }
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
    kind: ViewKind,
    opts: &'a Options,
    view: Box<dyn View + 'static>,
    repos_refreshed: usize,
    out: Outcome,
}

impl<'a> Session<'a> {
    fn new(kind: ViewKind, opts: &'a Options, plan: Option<&Plan>) -> Self {
        Session {
            kind,
            opts,
            view: theme().view(kind, plan),
            repos_refreshed: 0,
            out: Outcome::default(),
        }
    }

    /// Run one zypper process to completion, feeding the view as we go.
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
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        });
        let mut reader = StreamReader::new(BufReader::new(Tee { inner: stdout, log }));
        while let Ok(Some(node)) = reader.next_node() {
            self.handle(&node);
        }
        let status = child.wait()?;
        self.view.finish();
        self.out.stderr = err_thread.join().unwrap_or_default();
        self.out.code = status.code().unwrap_or(exit::ON_SIGNAL);
        Ok(())
    }

    /// Print a theme-produced line; themes return "" for "nothing".
    fn print(&self, line: String) {
        if !line.is_empty() {
            self.view.println(&line);
        }
    }

    fn handle(&mut self, node: &Node) {
        match node.name.as_str() {
            "message" => self.on_message(node),
            "install-summary" => self.out.plan = Some(plan_from_node(node)),
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
                self.view.println(&theme().message(MsgKind::Error, text));
            }
            Some("warning") => self.view.println(&theme().message(MsgKind::Warning, text)),
            _ => self.on_info(text),
        }
    }

    fn on_info(&mut self, text: &str) {
        if self.opts.verbose {
            self.view.println(&ui::dim(text));
        }
        if self.observe_label(text, None, false) {
            return;
        }
        // Parallel downloads: "Preloading: foo-1.0.x86_64.rpm [done]"
        // (or "[Error: ..., trying next mirror.]", which zypper recovers from).
        if let Some(rest) = text.strip_prefix("Preloading: ") {
            if let Some(file) = rest.strip_suffix("[done]") {
                self.view.preloaded(file.trim());
            }
            return;
        }
        if text.starts_with("Nothing to do") {
            self.out.nothing_to_do = true;
            return;
        }
        if let Some(name) = text.strip_suffix(" is already installed.") {
            let name = name.trim_matches(|c| c == '\'' || c == '"');
            self.print(theme().already_installed(name));
            return;
        }
        if text.starts_with("No update candidate") || text.contains("is not installed") {
            self.print(theme().note(text));
            return;
        }
        if self.kind == ViewKind::Refresh
            && let Some(repo) = quoted(text)
        {
            let updated = text.starts_with("Building repository");
            if updated || text.ends_with("is up to date.") {
                self.repos_refreshed += 1;
                self.print(theme().repo_refreshed(repo, updated));
                return;
            }
        }
        if self.kind == ViewKind::Commit {
            self.out.log.push(text.to_string());
        } else {
            self.view.status(text);
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
        if name.is_empty() {
            return;
        }
        let pct = n.attr("value").and_then(|v| v.parse::<u64>().ok());
        let done = n.attr("done").is_some();
        if self.observe_label(name, pct, done) {
            return;
        }
        // "Preloading Packages: [ (3.1 MiB / 4.5 MiB) (2.5 KiB/s)]"
        if name.starts_with("Preload finished") {
            self.view.preload("", None, true);
            return;
        }
        if let Some(rest) = name.strip_prefix("Preloading Packages") {
            let detail = rest
                .trim_start_matches(':')
                .trim()
                .trim_matches(|c| c == '[' || c == ']')
                .replace(") (", " · ")
                .replace(['(', ')'], "");
            self.view.preload(detail.trim(), pct, done);
            return;
        }
        self.view.step(name, pct, done);
    }

    fn on_download(&mut self, n: &Node) {
        if n.attr("done").is_some() {
            return;
        }
        let rate = n
            .attr("rate")
            .and_then(|r| r.parse::<i64>().ok())
            .filter(|r| *r > 0);
        let pct = n.attr("percent").and_then(|p| p.parse::<u64>().ok());
        let url = n.attr_or("url", "");
        let file = url.rsplit('/').find(|s| !s.is_empty()).unwrap_or(url);
        self.view.download(file, pct, rate);
    }

    /// Recognise "Retrieving: foo-1.0-1.x86_64 (repo) (3/12), 1.6 MiB" and
    /// "(3/12) Installing: foo-1.0-1.x86_64".
    fn observe_label(&mut self, label: &str, pct: Option<u64>, done: bool) -> bool {
        let Some((i, total)) = parse_counter(label) else {
            return false;
        };
        let verb = if label.starts_with("Retrieving") || label.contains("Downloading") {
            Verb::Download
        } else if label.contains("Installing") {
            Verb::Install
        } else if label.contains("Removing") {
            Verb::Remove
        } else {
            return false;
        };
        let nevra = label
            .split_once(": ")
            .map(|(_, rest)| rest)
            .unwrap_or(label)
            .split(" (")
            .next()
            .unwrap_or("")
            .split(", ")
            .next()
            .unwrap_or("")
            .trim();
        self.view.item(verb, i, total, nevra, pct, done);
        true
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
        match self.out.code {
            exit::INF_CAP_NOT_FOUND => {
                if let Some(q) = first_pkg {
                    ui::hint(format!(
                        "search for it with {}",
                        ui::cmd(&format!("pzyp search {q}"))
                    ));
                }
            }
            exit::INF_RPM_SCRIPT_FAILED => {
                ui::warning("the transaction finished, but some package scripts failed")
            }
            _ if !self.out.problems.is_empty() => ui::hint(format!(
                "run {} to pick a solution interactively",
                ui::cmd(&format!("sudo zypper {}", args.join(" ")))
            )),
            _ => {
                let all = self.out.log.join("\n") + &self.out.stderr;
                if all.contains("--auto-agree-with-licenses") {
                    ui::hint(format!(
                        "pass {} to accept package licenses",
                        ui::cmd("--agree-licenses")
                    ));
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

/// Run a zypper command (`subcommand args...`) as root, rendering everything.
pub fn run(mode: Mode, args: Vec<String>, opts: Options) -> Result<i32> {
    let sudo = zypper::ensure_sudo()?;
    let th = theme();
    let started = Instant::now();

    if mode == Mode::Refresh {
        th.refresh_start();
        let mut s = Session::new(ViewKind::Refresh, &opts, None);
        s.exec(sudo, &["--non-interactive"], &args)?;
        if !exit::is_success(s.out.code) {
            s.report_failure(&args);
            ui::hint(format!(
                "to import new repository keys, run {}",
                ui::cmd("sudo zypper refresh")
            ));
            return Ok(s.out.code);
        }
        th.refresh_done(s.repos_refreshed, started.elapsed());
        post_hints(s.out.code);
        return Ok(s.out.code);
    }

    // 1. Resolve.
    th.resolve_start();
    let mut resolve = args.clone();
    resolve.insert(1, "--dry-run".into());
    let mut s = Session::new(ViewKind::Resolve, &opts, None);
    s.exec(sudo, &["--non-interactive"], &resolve)?;
    if !exit::is_success(s.out.code) {
        s.report_failure(&args);
        return Ok(s.out.code);
    }
    let plan = match s.out.plan.take() {
        Some(mut p) if !p.items.is_empty() => {
            p.requested = args
                .iter()
                .skip(1)
                .filter(|a| !a.starts_with('-'))
                .cloned()
                .collect();
            p
        }
        _ => {
            th.nothing_to_do(started.elapsed());
            return Ok(0);
        }
    };
    th.plan(&plan, started.elapsed());

    // 2. Confirm.
    if opts.dry_run {
        th.dry_run_done();
        return Ok(0);
    }
    if !opts.yes && !th.confirm(&th.confirm_question(&plan))? {
        th.aborted();
        return Ok(1);
    }

    // 3. Commit.
    let commit_started = Instant::now();
    let mut s = Session::new(ViewKind::Commit, &opts, Some(&plan));
    s.exec(sudo, &["--non-interactive", "--no-refresh"], &args)?;
    if !exit::is_success(s.out.code) {
        s.report_failure(&args);
        return Ok(s.out.code);
    }
    if opts.verbose {
        for l in &s.out.log {
            ui::eprint_line(ui::dim(l));
        }
    }
    th.done(&plan, commit_started.elapsed());
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
        assert_eq!(
            quoted("Repository 'Main (OSS)' is up to date."),
            Some("Main (OSS)")
        );
    }

    #[test]
    fn plan_from_xml() {
        let xml = br#"<stream><install-summary download-size="2048" space-usage-diff="4096" packages-to-change="3" need-reboot="0">
<to-upgrade><solvable type="package" name="audit" edition="4.0.2-3.6" edition-old="4.0.2-3.5" arch="x86_64"/></to-upgrade>
<to-install><solvable type="package" name="ripgrep" edition="15.2.0-1.3" arch="x86_64"/></to-install>
<to-remove><solvable type="package" name="old" edition="1.0" arch="noarch"/></to-remove>
</install-summary></stream>"#;
        let nodes = crate::xml::parse_all(xml);
        let plan = plan_from_node(&nodes[0]);
        assert_eq!(plan.items.len(), 3);
        assert_eq!(plan.items[0].change, Change::Upgrade);
        assert_eq!(plan.items[2].change, Change::Remove);
        assert_eq!(plan.download, 2048);
        assert_eq!(
            plan.lookup("ripgrep-15.2.0-1.3.x86_64")
                .map(|i| i.name.as_str()),
            Some("ripgrep")
        );
    }
}
