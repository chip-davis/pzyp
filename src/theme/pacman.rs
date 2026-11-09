//! A pacman-flavoured look: `::` headings, `repo/name version [installed]`
//! search results, a `Packages (N)` transaction summary, and one
//! progress line per step that stays on screen. With `candy` (pacman's
//! `ILoveCandy`) the bars chomp: `[------C o  o  o ]`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

use super::*;
use crate::ui::{self, Change};

pub struct Pacman;

/// pacman.conf's `ILoveCandy`. Set once at startup from the config.
static CANDY: AtomicBool = AtomicBool::new(true);

pub fn set_candy(on: bool) {
    CANDY.store(on, Ordering::Relaxed);
}

/// Draw the inside of a bar, the way pacman's `fill_progress` does.
/// Plain: `#####-----`. Candy: eaten `-`, a yellow `C`/`c` mouth that opens
/// and closes as it moves, and `o` pellets every third column ahead of it.
fn bar_str(frac: f64, width: usize) -> String {
    let hash = ((frac.clamp(0.0, 1.0) * width as f64) as usize).min(width);
    if !CANDY.load(Ordering::Relaxed) {
        return format!("{}{}", "#".repeat(hash), "-".repeat(width - hash));
    }
    let mut out = String::with_capacity(width * 2);
    for i in (1..=width).rev() {
        if i > width - hash {
            out.push('-');
        } else if i == width - hash {
            let mouth = if hash.is_multiple_of(2) { "C" } else { "c" };
            out.push_str(&style(mouth).yellow().bold().to_string());
        } else if i % 3 == 0 {
            out.push('o');
        } else {
            out.push(' ');
        }
    }
    out
}

fn heading(text: &str) -> String {
    format!("{} {}", style("::").blue().bold(), style(text).bold())
}

/// pacman prints sizes with two decimals: "1.50 MiB".
fn size(n: i64) -> String {
    let neg = n < 0;
    let mut v = n.unsigned_abs() as f64 / 1024.0;
    let units = ["KiB", "MiB", "GiB", "TiB"];
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{}{v:.2} {}", if neg { "-" } else { "" }, units[i])
}

/// "foo-1.0-1.x86_64.rpm" -> "foo-1.0-1-x86_64", the way pacman names files.
fn pkg_file(file: &str) -> String {
    let f = file.trim_end_matches(".rpm");
    match f.rsplit_once('.') {
        Some((base, arch)) if !arch.contains('-') => format!("{base}-{arch}"),
        _ => f.to_string(),
    }
}

/// Layout for a progress line: `prefix ... msg [####----] 100%`.
#[derive(Clone, Copy)]
struct Layout {
    prefix_w: usize,
    msg_w: usize,
    bar_w: usize,
}

fn layout() -> Layout {
    let tw = ui::term_width().clamp(60, 140);
    let bar_w = (tw * 3 / 10).clamp(10, 40);
    let msg_w = 22;
    Layout {
        prefix_w: tw.saturating_sub(bar_w + msg_w + 10),
        msg_w,
        bar_w,
    }
}

fn line_bar(prefix: &str) -> ProgressBar {
    let l = layout();
    let pb = ProgressBar::new(100);
    pb.set_style(
        ProgressStyle::with_template(&format!(
            "{{prefix:{}!}} {{msg:>{}!}} [{{pbar}}] {{percent:>3}}%",
            l.prefix_w, l.msg_w
        ))
        .unwrap()
        .with_key(
            "pbar",
            move |s: &indicatif::ProgressState, w: &mut dyn std::fmt::Write| {
                let _ = w.write_str(&bar_str(s.fraction() as f64, l.bar_w));
            },
        ),
    );
    pb.set_prefix(prefix.to_string());
    pb
}

/// The same line, fully completed, as plain text (for non-TTY output and
/// for files that finish while another bar is live).
fn static_line(prefix: &str, msg: &str) -> String {
    let l = layout();
    format!(
        "{} {:>mw$} [{}] 100%",
        ui::pad_or_truncate(prefix, l.prefix_w),
        ui::truncate(msg, l.msg_w),
        bar_str(1.0, l.bar_w),
        mw = l.msg_w
    )
}

/// Replace a live bar with its finished line. We print the line ourselves
/// rather than letting indicatif leave the bar drawn: indicatif doesn't end
/// it with a newline, so the next line would run straight on from it.
fn complete(pb: &ProgressBar) {
    let line = static_line(&pb.prefix(), &pb.message());
    pb.finish_and_clear();
    ui::eprint_line(line);
}

fn wrap_list(label: &str, words: &[String]) -> String {
    let tw = ui::term_width().max(40);
    let indent = console::measure_text_width(label) + 1;
    let mut out = style(label).bold().to_string();
    out.push(' ');
    let mut col = indent;
    for (i, w) in words.iter().enumerate() {
        let wl = console::measure_text_width(w);
        if i > 0 && col + 2 + wl > tw {
            out.push('\n');
            out.push_str(&" ".repeat(indent));
            col = indent;
        } else if i > 0 {
            out.push_str("  ");
            col += 2;
        }
        out.push_str(w);
        col += wl;
    }
    out
}

/// zypper info keys -> pacman -Si keys (None = not shown).
fn rename(k: &str) -> Option<&str> {
    match k {
        "Summary" => Some("Description"),
        "Arch" => Some("Architecture"),
        "Upstream URL" => Some("URL"),
        // pacman has no long description or separate "Installed" flag.
        "Description" | "Installed" => None,
        other => Some(other),
    }
}

/// "Checking for file conflicts: (1 skipped)" -> "checking for file conflicts"
fn step_name(name: &str) -> String {
    let n = name.split(':').next().unwrap_or(name).trim();
    let mut c = n.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

impl Theme for Pacman {
    fn message(&self, kind: MsgKind, text: &str) -> String {
        // zypper: "No provider of 'foo' found." -> pacman: "target not found: foo"
        let text = match text
            .strip_prefix("No provider of '")
            .and_then(|r| r.strip_suffix("' found."))
        {
            Some(t) => format!("target not found: {t}"),
            None => text.to_string(),
        };
        match kind {
            MsgKind::Error => format!("{} {text}", style("error:").red().bold()),
            MsgKind::Warning => format!("{} {text}", style("warning:").yellow().bold()),
        }
    }

    fn hint(&self, text: &str) -> String {
        heading(text)
    }

    fn spinner(&self, _msg: &str) -> ProgressBar {
        ProgressBar::hidden()
    }

    fn confirm(&self, question: &str) -> Result<bool> {
        ask(&format!("{} [Y/n] ", heading(question)), true)
    }

    fn search_wants_details(&self) -> bool {
        true
    }

    fn search(&self, rows: &[SearchRow], _query: &str, _details: bool, _took: Duration) {
        for r in rows {
            let kind = if r.kind != "package" && !r.kind.is_empty() {
                format!(" {}", style(format!("({})", r.kind)).blue().bold())
            } else {
                String::new()
            };
            let inst = match (&r.installed_version, r.installed) {
                (Some(v), _) => format!(" {}", style(format!("[installed: {v}]")).cyan().bold()),
                (None, true) => format!(" {}", style("[installed]").cyan().bold()),
                _ => String::new(),
            };
            println!(
                "{}{}{} {}{kind}{inst}",
                style(&r.repo).magenta().bold(),
                style("/").magenta().bold(),
                style(&r.name).bold(),
                style(&r.version).green().bold(),
            );
            if !r.summary.is_empty() {
                println!("    {}", r.summary);
            }
        }
    }

    fn search_empty(&self, _query: &str, _took: Duration) {}

    fn info(&self, fields: &InfoFields) {
        let first = ["Repository", "Name", "Version", "Summary"];
        let mut ordered: Vec<(&str, &str)> = Vec::new();
        for f in first {
            if let Some((k, v)) = fields.iter().find(|(k, _)| k == f) {
                ordered.push((k, v));
            }
        }
        for (k, v) in fields {
            if !first.contains(&k.as_str()) {
                ordered.push((k, v));
            }
        }
        for (k, v) in ordered {
            let Some(key) = rename(k) else { continue };
            let mut lines = v.lines();
            println!(
                "{} : {}",
                style(format!("{key:<15}")).bold(),
                lines.next().unwrap_or("None")
            );
            for l in lines {
                println!("{:18}{l}", "");
            }
        }
    }

    fn repos(&self, rows: &[RepoRow]) {
        for (i, r) in rows.iter().enumerate() {
            if i > 0 {
                println!();
            }
            let section = format!("[{}]", r.alias);
            if r.enabled {
                println!("{}", style(section).bold());
            } else {
                println!("{}", ui::dim(&format!("#{section}")));
            }
            let mut comment = r.name.clone();
            if r.priority != "99" {
                comment.push_str(&format!(", priority {}", r.priority));
            }
            if !r.autorefresh {
                comment.push_str(", no auto-refresh");
            }
            println!("{}", ui::dim(&format!("# {comment}")));
            let server = format!("Server = {}", r.url);
            if r.enabled {
                println!("{server}");
            } else {
                println!("{}", ui::dim(&format!("#{server}")));
            }
        }
    }

    fn updates(&self, rows: &[UpdateRow], patches: bool, _took: Duration, _cmd: &str) {
        for u in rows {
            if patches {
                println!(
                    "{}{}{} {}",
                    style(&u.category).magenta().bold(),
                    style("/").magenta().bold(),
                    style(&u.name).bold(),
                    u.summary
                );
            } else {
                println!(
                    "{} {} -> {}",
                    style(&u.name).bold(),
                    style(&u.old).red().bold(),
                    style(&u.new).green().bold()
                );
            }
        }
    }

    fn up_to_date(&self, _took: Duration) {}

    fn view(&self, kind: ViewKind, plan: Option<&Plan>) -> Box<dyn View + '_> {
        let downloads = plan
            .map(|p| {
                p.items
                    .iter()
                    .filter(|i| {
                        i.change != Change::Remove && (i.kind == "package" || i.kind.is_empty())
                    })
                    .count()
            })
            .unwrap_or(0);
        // Can't borrow the plan for the view's lifetime here, so copy what we need.
        let items = plan.map(|p| p.items.clone()).unwrap_or_default();
        Box::new(PacView {
            kind,
            items,
            downloads,
            line: None,
            total: None,
            preloaded: 0,
            cached: false,
            retrieving: false,
            processing: false,
            hooks: false,
            hook_count: 0,
            seen: Default::default(),
        })
    }

    fn resolve_start(&self) {
        ui::eprint_line("resolving dependencies...");
    }

    fn already_installed(&self, name: &str) -> String {
        self.message(
            MsgKind::Warning,
            &format!("{name} is up to date -- skipping"),
        )
    }

    fn note(&self, text: &str) -> String {
        self.message(MsgKind::Warning, text)
    }

    fn plan(&self, plan: &Plan, _took: Duration) {
        let removing = plan.only(Change::Remove);
        if !removing {
            ui::eprint_line("looking for conflicting packages...");
        }
        ui::eprint_line("");
        let words: Vec<String> = plan
            .items
            .iter()
            .map(|i| {
                let w = format!("{}-{}", i.display_name(), i.edition);
                if i.change == Change::Remove && !removing {
                    format!("{w} {}", style("[removal]").red())
                } else {
                    w
                }
            })
            .collect();
        ui::eprint_line(wrap_list(
            &format!("Packages ({})", plan.items.len()),
            &words,
        ));
        ui::eprint_line("");
        let row = |label: &str, v: i64| {
            ui::eprint_line(format!(
                "{} {:>12}",
                style(format!("{label:<21}")).bold(),
                size(v)
            ));
        };
        if plan.download > 0 {
            row("Total Download Size:", plan.download);
        }
        if removing {
            row(
                "Total Removed Size:",
                plan.removed_size.max(-plan.space_diff),
            );
        } else if plan.only(Change::Install) {
            row(
                "Total Installed Size:",
                plan.installed_size.max(plan.space_diff),
            );
        } else {
            row("Net Upgrade Size:", plan.space_diff);
        }
        ui::eprint_line("");
        if plan.need_reboot {
            ui::warning("this transaction will require a reboot");
        }
    }

    fn confirm_question(&self, plan: &Plan) -> String {
        if plan.only(Change::Remove) {
            "Do you want to remove these packages?".into()
        } else {
            "Proceed with installation?".into()
        }
    }

    fn nothing_to_do(&self, _took: Duration) {
        ui::eprint_line(" there is nothing to do");
    }

    fn dry_run_done(&self) {
        ui::eprint_line(heading("dry run: no changes were made"));
    }

    fn aborted(&self) {}

    fn done(&self, _plan: &Plan, _took: Duration) {}

    fn repo_refreshed(&self, repo: &str, updated: bool) -> String {
        if updated {
            static_line(&format!(" {repo}"), "")
        } else {
            format!(" {repo} is up to date")
        }
    }

    fn refresh_start(&self) {
        ui::eprint_line(heading("Synchronizing package databases..."));
    }

    fn refresh_done(&self, _count: usize, _took: Duration) {}
}

// ---------------------------------------------------------------- live view

struct PacView {
    kind: ViewKind,
    items: Vec<Item>,
    downloads: usize,
    /// The current per-step line.
    line: Option<(String, ProgressBar)>,
    /// The "Total (n/N)" bar shown while packages are preloaded.
    total: Option<ProgressBar>,
    preloaded: usize,
    cached: bool,
    retrieving: bool,
    processing: bool,
    /// Post-transaction hooks have started.
    hooks: bool,
    hook_count: usize,
    seen: std::collections::HashSet<String>,
}

impl PacView {
    fn end_line(&mut self) {
        if let Some((_, pb)) = self.line.take() {
            complete(&pb);
        }
    }

    fn end_total(&mut self) {
        if let Some(pb) = self.total.take() {
            complete(&pb);
            self.cached = true;
        }
    }

    /// Start (or continue) the line identified by `key`.
    fn line(&mut self, key: &str, prefix: &str) -> &ProgressBar {
        if self.line.as_ref().map(|(k, _)| k.as_str()) != Some(key) {
            self.end_line();
            self.line = Some((key.to_string(), line_bar(prefix)));
        }
        &self.line.as_ref().unwrap().1
    }

    fn announce_retrieving(&mut self) {
        if !self.retrieving {
            self.retrieving = true;
            self.println(&heading("Retrieving packages..."));
        }
    }

    fn display(&self, nevra: &str) -> (String, Option<Change>) {
        let plan = Plan {
            items: self.items.clone(),
            ..Default::default()
        };
        match plan.lookup(nevra) {
            Some(i) => (i.display_name(), Some(i.change)),
            None => (nevra.to_string(), None),
        }
    }
}

impl View for PacView {
    fn status(&mut self, _text: &str) {}

    fn step(&mut self, name: &str, pct: Option<u64>, done: bool) {
        if self.kind != ViewKind::Commit {
            return;
        }
        let n = step_name(name);
        if n.is_empty() {
            return;
        }
        // After the packages: pacman-style hooks, as plain lines, once each.
        if n.starts_with("running post-transaction") {
            if !self.hooks {
                self.end_line();
                self.hooks = true;
                self.println(&heading("Running post-transaction hooks..."));
            }
            return;
        }
        if self.hooks {
            if self.seen.insert(n.clone()) {
                self.hook_count += 1;
                self.println(&format!("({}) {n}...", self.hook_count));
            }
            return;
        }
        // Before the packages: "(n/n) checking for file conflicts [###] 100%".
        if self.seen.contains(&n) && self.line.as_ref().map(|(k, _)| k) != Some(&n) {
            return;
        }
        let total = self.items.len().max(1);
        let prefix = format!("({total}/{total}) {n}");
        let pb = self.line(&n, &prefix).clone();
        if let Some(p) = pct {
            pb.set_position(p.min(100));
        }
        if done {
            self.seen.insert(n);
            self.end_line();
        }
    }

    fn item(
        &mut self,
        verb: Verb,
        i: usize,
        total: usize,
        nevra: &str,
        pct: Option<u64>,
        done: bool,
    ) {
        if self.kind != ViewKind::Commit {
            return;
        }
        if verb == Verb::Download {
            self.end_total();
            if self.cached {
                return;
            }
            self.announce_retrieving();
            let prefix = format!(" {nevra}");
            self.line(&format!("dl {i}"), &prefix);
            return;
        }
        self.end_total();
        if !self.processing {
            self.end_line();
            self.processing = true;
            self.println(&heading("Processing package changes..."));
        }
        let (name, change) = self.display(nevra);
        let word = match (verb, change) {
            (Verb::Remove, _) => "removing",
            (_, Some(Change::Upgrade)) => "upgrading",
            (_, Some(Change::Downgrade)) => "downgrading",
            (_, Some(Change::Reinstall)) => "reinstalling",
            _ => "installing",
        };
        let w = total.to_string().len();
        let prefix = format!("({i:>w$}/{total}) {word} {name}");
        let pb = self.line(&format!("{verb:?} {i}"), &prefix).clone();
        if let Some(p) = pct {
            pb.set_position(p.min(100));
        }
        if done {
            self.end_line();
        }
    }

    fn download(&mut self, _file: &str, pct: Option<u64>, rate: Option<i64>) {
        if let Some((k, pb)) = &self.line
            && k.starts_with("dl ")
        {
            if let Some(p) = pct {
                pb.set_position(p.min(100));
            }
            if let Some(r) = rate {
                pb.set_message(format!("{}/s", size(r)));
            }
        }
    }

    fn preload(&mut self, detail: &str, pct: Option<u64>, done: bool) {
        if self.kind != ViewKind::Commit {
            return;
        }
        self.announce_retrieving();
        let (n, total) = (self.preloaded, self.downloads.max(self.preloaded));
        let pb = self
            .total
            .get_or_insert_with(|| line_bar(&format!(" Total ({n}/{total})")))
            .clone();
        pb.set_prefix(format!(" Total ({n}/{total})"));
        // "3.1 MiB / 4.5 MiB · 2.5 KiB/s" -> "2.5 KiB/s"
        if let Some(rate) = detail.rsplit(" · ").next().filter(|r| r.ends_with("/s")) {
            pb.set_message(rate.to_string());
        }
        if let Some(p) = pct {
            pb.set_position(p.min(100));
        }
        if done {
            self.end_total();
        }
    }

    fn preloaded(&mut self, file: &str) {
        if self.kind != ViewKind::Commit {
            return;
        }
        self.announce_retrieving();
        self.preloaded += 1;
        let line = static_line(&format!(" {}", pkg_file(file)), "");
        self.println(&line);
        if let Some(pb) = &self.total {
            pb.set_prefix(format!(
                " Total ({}/{})",
                self.preloaded,
                self.downloads.max(self.preloaded)
            ));
        }
    }

    fn println(&self, line: &str) {
        let pb = self.line.as_ref().map(|(_, pb)| pb).or(self.total.as_ref());
        println_above(pb, line);
    }

    fn finish(&mut self) {
        self.end_line();
        self.end_total();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(size(1572864), "1.50 MiB");
        assert_eq!(size(512), "0.50 KiB");
        assert_eq!(
            pkg_file("ripgrep-15.2.0-1.3.x86_64.rpm"),
            "ripgrep-15.2.0-1.3-x86_64"
        );
        set_candy(false);
        assert_eq!(bar_str(0.5, 10), "#####-----");
        set_candy(true);
        assert_eq!(console::strip_ansi_codes(&bar_str(0.5, 10)), "-----c o  ");
        assert_eq!(console::strip_ansi_codes(&bar_str(0.6, 10)), "------Co  ");
        assert_eq!(bar_str(1.0, 6), "------");
        assert_eq!(
            step_name("Checking for file conflicts: (1 skipped)"),
            "checking for file conflicts"
        );
        console::set_colors_enabled(false);
        assert_eq!(
            Pacman.message(MsgKind::Error, "No provider of 'nosuch' found."),
            "error: target not found: nosuch"
        );
    }
}
