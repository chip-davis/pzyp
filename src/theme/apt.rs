//! An apt-flavoured look: `Reading package lists... Done`, the
//! `The following NEW packages will be installed:` blocks, `Get:1 ...`
//! download lines, `Unpacking` / `Setting up` / `Processing triggers`
//! lines, and the green `Progress: [ 40%] [####......]` bar pinned to the
//! bottom of the screen.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

use super::*;
use crate::ui::{self, Change};

pub struct Apt;

/// Counter for `Hit:n` / `Get:n` lines during refresh.
static REFRESH_N: AtomicUsize = AtomicUsize::new(0);

fn commas(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// apt prints decimal units with thousands separators: "1,520 kB", "15.7 MB".
fn apt_size(bytes: i64) -> String {
    let b = bytes.unsigned_abs() as f64;
    if b < 1000.0 {
        format!("{} B", b as u64)
    } else if b < 10_000_000.0 {
        format!("{} kB", commas((b / 1000.0).round() as u64))
    } else if b < 10_000_000_000.0 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.1} GB", b / 1e9)
    }
}

/// Indented, wrapped package list.
fn wrapped(names: &[String]) -> String {
    let tw = ui::term_width().max(40);
    let mut out = String::from(" ");
    let mut col = 1;
    for n in names {
        if col + n.len() + 1 > tw && col > 1 {
            out.push_str("\n ");
            col = 1;
        }
        out.push(' ');
        out.push_str(n);
        col += n.len() + 1;
    }
    out
}

fn tree_lines() {
    ui::eprint_line("Building dependency tree... Done");
    ui::eprint_line("Reading state information... Done");
}

/// "Progress: [ 40%] [#########...........]" with apt's green label.
fn progress_line(frac: f64) -> String {
    let tw = ui::term_width().max(40);
    let pct = (frac * 100.0).round() as u64;
    let label = format!("Progress: [{pct:>3}%]");
    let bar_w = tw.saturating_sub(label.len() + 4);
    let filled = ((frac * bar_w as f64) as usize).min(bar_w);
    format!(
        "{} [{}{}] ",
        style(label).black().on_green(),
        "#".repeat(filled),
        ".".repeat(bar_w - filled)
    )
}

impl Theme for Apt {
    fn message(&self, kind: MsgKind, text: &str) -> String {
        let text = match text
            .strip_prefix("No provider of '")
            .and_then(|r| r.strip_suffix("' found."))
        {
            Some(pkg) => format!("Unable to locate package {pkg}"),
            None => text.to_string(),
        };
        match kind {
            MsgKind::Error => format!("{} {text}", style("E:").red().bold()),
            MsgKind::Warning => format!("{} {text}", style("W:").yellow().bold()),
        }
    }

    fn hint(&self, text: &str) -> String {
        format!("N: {text}")
    }

    fn spinner(&self, _msg: &str) -> ProgressBar {
        ProgressBar::hidden()
    }

    fn confirm(&self, question: &str) -> Result<bool> {
        ask(&format!("{question} [Y/n] "), true)
    }

    fn search_wants_details(&self) -> bool {
        true
    }

    fn search(&self, rows: &[SearchRow], _query: &str, _details: bool, _took: Duration) {
        ui::eprint_line("Sorting... Done");
        ui::eprint_line("Full Text Search... Done");
        for r in rows {
            let state = match (&r.installed_version, r.installed) {
                (Some(v), _) => format!(" [installed,upgradable from: {v}]"),
                (None, true) => " [installed]".into(),
                _ => String::new(),
            };
            println!(
                "{}/{} {} {}{state}",
                style(r.label()).green().bold(),
                r.repo,
                r.version,
                r.arch
            );
            println!("  {}", r.summary);
            println!();
        }
    }

    fn search_empty(&self, _query: &str, _took: Duration) {
        ui::eprint_line("Sorting... Done");
        ui::eprint_line("Full Text Search... Done");
    }

    fn info(&self, fields: &InfoFields) {
        let get = |k: &str| {
            fields
                .iter()
                .find(|(f, _)| f == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        };
        let row = |k: &str, v: &str| {
            if !v.is_empty() {
                println!("{}: {v}", style(k).bold());
            }
        };
        row("Package", get("Name"));
        row("Version", get("Version"));
        row("Architecture", get("Arch"));
        row("Maintainer", get("Vendor"));
        row("Installed-Size", get("Installed Size"));
        let status = get("Status");
        if status.starts_with("up-to-date") || status.starts_with("out-of-date") {
            row("APT-Manual-Installed", "yes");
        }
        row("Homepage", get("Upstream URL"));
        row("Source", get("Source package"));
        row("APT-Sources", get("Repository"));
        println!("{}: {}", style("Description").bold(), get("Summary"));
        for l in get("Description").lines() {
            // Debian control-file style: blank lines become " ."
            println!(" {}", if l.trim().is_empty() { "." } else { l });
        }
    }

    fn repos(&self, rows: &[RepoRow]) {
        for (i, r) in rows.iter().enumerate() {
            if i > 0 {
                println!();
            }
            println!("{}", ui::dim(&format!("# {}", r.name)));
            let line = format!("deb {} {} main", r.url, r.alias);
            if r.enabled {
                println!("{line}");
            } else {
                println!("{}", ui::dim(&format!("# {line}")));
            }
        }
    }

    fn updates(&self, rows: &[UpdateRow], patches: bool, _took: Duration, _cmd: &str) {
        ui::eprint_line("Listing... Done");
        for u in rows {
            if patches {
                println!(
                    "{}/{} {}",
                    style(&u.name).green().bold(),
                    u.category,
                    u.summary
                );
            } else {
                println!(
                    "{}/{} {} {} [upgradable from: {}]",
                    style(&u.name).green().bold(),
                    u.repo,
                    u.new,
                    u.arch,
                    u.old
                );
            }
        }
    }

    fn up_to_date(&self, _took: Duration) {
        ui::eprint_line("Listing... Done");
        ui::eprint_line("All packages are up to date.");
    }

    fn view(&self, kind: ViewKind, plan: Option<&Plan>) -> Box<dyn View + '_> {
        Box::new(AptView {
            kind,
            items: plan.map(|p| p.items.clone()).unwrap_or_default(),
            download_size: plan.map(|p| p.download).unwrap_or(0),
            bar: None,
            gets: 0,
            dl_started: None,
            fetched: false,
            current: None,
            done_items: 0,
            total_items: plan.map(|p| p.items.len()).unwrap_or(0),
            seen: HashSet::new(),
            preloaded: false,
        })
    }

    fn resolve_start(&self) {
        ui::eprint_line("Reading package lists... Done");
    }

    fn already_installed(&self, name: &str) -> String {
        format!("{name} is already the newest version.")
    }

    fn note(&self, text: &str) -> String {
        text.to_string()
    }

    fn plan(&self, plan: &Plan, _took: Duration) {
        tree_lines();
        let names = |pred: &dyn Fn(&Item) -> bool| -> Vec<String> {
            plan.items
                .iter()
                .filter(|i| pred(i))
                .map(|i| i.display_name())
                .collect()
        };
        let has_req = !plan.requested.is_empty();
        let sections: [(&str, Vec<String>); 6] = [
            (
                "The following additional packages will be installed:",
                names(&|i| has_req && i.change == Change::Install && !plan.is_requested(i)),
            ),
            (
                "The following packages will be REMOVED:",
                names(&|i| i.change == Change::Remove),
            ),
            (
                "The following NEW packages will be installed:",
                names(&|i| i.change == Change::Install),
            ),
            (
                "The following packages will be upgraded:",
                names(&|i| i.change == Change::Upgrade),
            ),
            (
                "The following packages will be DOWNGRADED:",
                names(&|i| i.change == Change::Downgrade),
            ),
            (
                "The following packages will be reinstalled:",
                names(&|i| matches!(i.change, Change::Reinstall | Change::ArchChange)),
            ),
        ];
        for (title, list) in sections {
            if !list.is_empty() {
                ui::eprint_line(title);
                ui::eprint_line(wrapped(&list));
            }
        }
        let mut parts = vec![
            format!("{} upgraded", plan.count(Change::Upgrade)),
            format!("{} newly installed", plan.count(Change::Install)),
        ];
        let re = plan.count(Change::Reinstall) + plan.count(Change::ArchChange);
        if re > 0 {
            parts.push(format!("{re} reinstalled"));
        }
        if plan.count(Change::Downgrade) > 0 {
            parts.push(format!("{} downgraded", plan.count(Change::Downgrade)));
        }
        parts.push(format!("{} to remove", plan.count(Change::Remove)));
        ui::eprint_line(format!("{}.", parts.join(", ")));
        if plan.download > 0 {
            ui::eprint_line(format!(
                "Need to get {} of archives.",
                apt_size(plan.download)
            ));
        }
        if plan.space_diff >= 0 {
            ui::eprint_line(format!(
                "After this operation, {} of additional disk space will be used.",
                apt_size(plan.space_diff)
            ));
        } else {
            ui::eprint_line(format!(
                "After this operation, {} disk space will be freed.",
                apt_size(-plan.space_diff)
            ));
        }
        if plan.need_reboot {
            ui::warning("A reboot will be required to complete this transaction.");
        }
    }

    fn confirm_question(&self, _plan: &Plan) -> String {
        "Do you want to continue?".into()
    }

    fn nothing_to_do(&self, _took: Duration) {
        tree_lines();
        ui::eprint_line("0 upgraded, 0 newly installed, 0 to remove.");
    }

    fn dry_run_done(&self) {
        ui::eprint_line("NOTE: This is only a simulation! No changes were made.");
    }

    fn aborted(&self) {
        ui::eprint_line("Abort.");
    }

    fn done(&self, _plan: &Plan, _took: Duration) {}

    fn repo_refreshed(&self, repo: &str, updated: bool) -> String {
        let n = REFRESH_N.fetch_add(1, Ordering::Relaxed) + 1;
        if updated {
            format!("Get:{n} {repo} InRelease")
        } else {
            format!("Hit:{n} {repo} InRelease")
        }
    }

    fn refresh_done(&self, _count: usize, _took: Duration) {
        ui::eprint_line("Reading package lists... Done");
    }
}

// ---------------------------------------------------------------- live view

struct AptView {
    kind: ViewKind,
    items: Vec<Item>,
    download_size: i64,
    /// The pinned bottom "Progress:" bar.
    bar: Option<ProgressBar>,
    gets: usize,
    dl_started: Option<Instant>,
    fetched: bool,
    /// The package currently being unpacked: (verb, counter, item).
    current: Option<(Verb, usize, Option<Item>, String)>,
    done_items: usize,
    total_items: usize,
    seen: HashSet<String>,
    preloaded: bool,
}

impl AptView {
    fn item_for(&self, s: &str) -> Option<Item> {
        let plan = Plan {
            items: self.items.clone(),
            ..Default::default()
        };
        plan.lookup(s).cloned()
    }

    fn get_line(&mut self, what: &str) {
        self.dl_started.get_or_insert_with(Instant::now);
        self.gets += 1;
        let line = match self.item_for(what) {
            Some(i) => {
                let parts: Vec<&str> = [i.repo.as_str(), &i.name, &i.arch, &i.edition]
                    .into_iter()
                    .filter(|p| !p.is_empty())
                    .collect();
                format!("Get:{} {}", self.gets, parts.join(" "))
            }
            None => format!("Get:{} {what}", self.gets),
        };
        self.println(&line);
    }

    fn fetched_line(&mut self) {
        if self.fetched || self.gets == 0 {
            return;
        }
        self.fetched = true;
        let secs = self
            .dl_started
            .map(|t| t.elapsed())
            .unwrap_or_default()
            .as_secs_f64();
        let rate = if secs > 0.0 {
            (self.download_size as f64 / secs) as i64
        } else {
            0
        };
        self.println(&format!(
            "Fetched {} in {}s ({}/s)",
            apt_size(self.download_size),
            secs.round() as u64,
            apt_size(rate)
        ));
    }

    /// Finish the current package: "Setting up foo (1.0) ...".
    fn settle(&mut self) {
        if let Some((verb, _, item, shown)) = self.current.take() {
            self.done_items += 1;
            if verb != Verb::Remove {
                let ver = item.as_ref().map(|i| i.edition.clone()).unwrap_or_default();
                self.println(&format!("Setting up {shown} ({ver}) ..."));
            }
            self.update_bar();
        }
    }

    fn update_bar(&mut self) {
        let total = self.total_items.max(self.done_items).max(1);
        let frac = self.done_items as f64 / total as f64;
        let bar = self.bar.get_or_insert_with(|| {
            let pb = ProgressBar::new(100);
            pb.set_style(ProgressStyle::with_template("{msg}").unwrap());
            pb
        });
        bar.set_message(progress_line(frac));
    }
}

impl View for AptView {
    fn status(&mut self, _text: &str) {}

    fn step(&mut self, name: &str, _pct: Option<u64>, _done: bool) {
        if self.kind != ViewKind::Commit {
            return;
        }
        // rpm triggers: "Running %transfiletriggerin(man-2.13.1-8.1.x86_64)"
        let n = name.split(':').next().unwrap_or(name).trim().to_string();
        if let Some(rest) = n.strip_prefix("Running %")
            && let Some((_, pkg)) = rest.split_once('(')
            && self.seen.insert(n.clone())
        {
            self.settle();
            let pkg = pkg.trim_end_matches(')');
            let name = self
                .item_for(pkg)
                .map(|i| i.name)
                .unwrap_or_else(|| pkg.to_string());
            self.println(&format!("Processing triggers for {name} ..."));
        }
    }

    fn item(
        &mut self,
        verb: Verb,
        i: usize,
        _total: usize,
        nevra: &str,
        _pct: Option<u64>,
        done: bool,
    ) {
        if self.kind != ViewKind::Commit {
            return;
        }
        if verb == Verb::Download {
            if !self.preloaded && self.seen.insert(format!("dl {i}")) {
                self.get_line(nevra);
            }
            return;
        }
        // apt fetches everything before unpacking; when zypper interleaves
        // downloads and installs, the "Fetched" line waits for the end.
        if self.preloaded {
            self.fetched_line();
        }
        if self.current.as_ref().map(|(v, n, _, _)| (*v, *n)) != Some((verb, i)) {
            self.settle();
            let item = self.item_for(nevra);
            let shown = item
                .as_ref()
                .map(|i| i.name.clone())
                .unwrap_or_else(|| nevra.to_string());
            let lines = match (verb, &item) {
                (Verb::Remove, Some(it)) => vec![format!("Removing {shown} ({}) ...", it.edition)],
                (Verb::Remove, None) => vec![format!("Removing {shown} ...")],
                (_, Some(it)) => {
                    let file = format!(".../{}_{}_{}.rpm", it.name, it.edition, it.arch);
                    let mut v = Vec::new();
                    if it.change == Change::Install {
                        v.push(format!("Selecting previously unselected package {shown}."));
                    }
                    v.push(format!("Preparing to unpack {file} ..."));
                    match &it.edition_old {
                        Some(old) => v.push(format!(
                            "Unpacking {shown} ({}) over ({old}) ...",
                            it.edition
                        )),
                        None => v.push(format!("Unpacking {shown} ({}) ...", it.edition)),
                    }
                    v
                }
                (_, None) => vec![format!("Unpacking {shown} ...")],
            };
            for l in lines {
                self.println(&l);
            }
            self.current = Some((verb, i, item, shown));
            self.update_bar();
        }
        if done {
            self.settle();
        }
    }

    fn download(&mut self, _file: &str, _pct: Option<u64>, _rate: Option<i64>) {}

    fn preload(&mut self, _detail: &str, _pct: Option<u64>, done: bool) {
        if self.kind != ViewKind::Commit {
            return;
        }
        self.preloaded = true;
        self.dl_started.get_or_insert_with(Instant::now);
        if done {
            self.fetched_line();
        }
    }

    fn preloaded(&mut self, file: &str) {
        if self.kind == ViewKind::Commit {
            self.preloaded = true;
            self.get_line(file.trim_end_matches(".rpm"));
        }
    }

    fn println(&self, line: &str) {
        println_above(self.bar.as_ref(), line);
    }

    fn finish(&mut self) {
        self.settle();
        self.fetched_line();
        if let Some(b) = self.bar.take() {
            b.finish_and_clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(apt_size(1_520_000), "1,520 kB");
        assert_eq!(apt_size(15_700_000), "15.7 MB");
        assert_eq!(apt_size(523), "523 B");
        assert_eq!(commas(1234567), "1,234,567");
        console::set_colors_enabled(false);
        assert_eq!(
            Apt.message(MsgKind::Error, "No provider of 'x' found."),
            "E: Unable to locate package x"
        );
    }
}
