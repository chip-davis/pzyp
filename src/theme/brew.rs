//! A Homebrew-flavoured look: blue `==>` headings, column-packed search
//! results with `✔`, `✔︎ Bottle foo (1.0)` for parallel downloads, the
//! full-width `#####… 100.0%` curl bar, `==> Pouring …` and `🍺`.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

use super::*;
use crate::ui::{self, Change};

pub struct Brew;

/// Repositories updated during `pzyp refresh`, for the closing summary.
static UPDATED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn arrow(text: &str) -> String {
    format!("{} {}", style("==>").blue().bold(), style(text).bold())
}

fn tick() -> String {
    style("✔︎").green().to_string()
}

/// Homebrew prints sizes without a space, in decimal units: "6.3MB".
fn brew_size(bytes: i64) -> String {
    let b = bytes.unsigned_abs() as f64;
    if b < 1e3 {
        format!("{b}B")
    } else if b < 1e6 {
        format!("{:.1}KB", b / 1e3)
    } else if b < 1e9 {
        format!("{:.1}MB", b / 1e6)
    } else {
        format!("{:.1}GB", b / 1e9)
    }
}

/// Lay names out in columns, top-to-bottom, like `brew search`.
fn columns(names: &[String]) -> Vec<String> {
    let tw = ui::term_width().max(20);
    let w = names
        .iter()
        .map(|n| console::measure_text_width(n))
        .max()
        .unwrap_or(0)
        + 2;
    let cols = (tw / w.max(1)).max(1);
    let rows = names.len().div_ceil(cols);
    (0..rows)
        .map(|r| {
            let mut line = String::new();
            for c in 0..cols {
                if let Some(n) = names.get(c * rows + r) {
                    let pad = w.saturating_sub(console::measure_text_width(n));
                    line.push_str(n);
                    if c + 1 < cols && names.get((c + 1) * rows + r).is_some() {
                        line.push_str(&" ".repeat(pad));
                    }
                }
            }
            line
        })
        .collect()
}

/// The curl-style progress bar brew shows for each download.
fn curl_bar() -> ProgressBar {
    let w = ui::term_width().clamp(30, 120).saturating_sub(8);
    let pb = ProgressBar::new(1000);
    pb.set_style(
        ProgressStyle::with_template(&format!("{{bar:{w}}} {{pct}}"))
            .unwrap()
            .with_key(
                "pct",
                |s: &indicatif::ProgressState, w: &mut dyn std::fmt::Write| {
                    let _ = write!(w, "{:>5.1}%", s.fraction() * 100.0);
                },
            )
            .progress_chars("## "),
    );
    pb
}

fn curl_done() -> String {
    let w = ui::term_width().clamp(30, 120).saturating_sub(8);
    format!("{} 100.0%", "#".repeat(w))
}

impl Theme for Brew {
    fn message(&self, kind: MsgKind, text: &str) -> String {
        let text = match text
            .strip_prefix("No provider of '")
            .and_then(|r| r.strip_suffix("' found."))
        {
            Some(pkg) => format!("No available formula with the name \"{pkg}\"."),
            None => text.to_string(),
        };
        match kind {
            MsgKind::Error => format!("{}: {text}", style("Error").red().underlined()),
            MsgKind::Warning => format!("{}: {text}", style("Warning").yellow().underlined()),
        }
    }

    fn hint(&self, text: &str) -> String {
        text.to_string()
    }

    fn spinner(&self, _msg: &str) -> ProgressBar {
        ProgressBar::hidden()
    }

    fn confirm(&self, question: &str) -> Result<bool> {
        ask(&format!("{} [Y/n] ", arrow(question)), true)
    }

    fn search_wants_details(&self) -> bool {
        false
    }

    fn search(&self, rows: &[SearchRow], _query: &str, _details: bool, _took: Duration) {
        let mut seen = HashSet::new();
        let mut packages = Vec::new();
        let mut patterns = Vec::new();
        for r in rows {
            if !seen.insert(r.label()) {
                continue;
            }
            let name = if r.installed {
                format!("{} {}", r.name, tick())
            } else {
                r.name.clone()
            };
            if r.kind == "package" || r.kind.is_empty() {
                packages.push(name);
            } else {
                patterns.push(name);
            }
        }
        for (title, list) in [("Formulae", packages), ("Patterns", patterns)] {
            if list.is_empty() {
                continue;
            }
            println!("{}", arrow(title));
            for line in columns(&list) {
                println!("{line}");
            }
        }
    }

    fn search_empty(&self, query: &str, _took: Duration) {
        ui::error(format!("No formulae or casks found for \"{query}\"."));
    }

    fn info(&self, fields: &InfoFields) {
        let get = |k: &str| {
            fields
                .iter()
                .find(|(f, _)| f == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        };
        let arch = get("Arch");
        let arch = if arch.is_empty() {
            String::new()
        } else {
            format!(" ({arch})")
        };
        println!(
            "{}",
            arrow(&format!("{}: stable {}{arch}", get("Name"), get("Version")))
        );
        println!("{}", get("Summary"));
        if !get("Upstream URL").is_empty() {
            println!("{}", style(get("Upstream URL")).underlined());
        }
        let status = get("Status");
        if status.starts_with("up-to-date") {
            println!("Installed");
            println!("{} *", get("Installed Size"));
        } else if status.starts_with("out-of-date") {
            println!("Installed (outdated: {status})");
        } else {
            println!("Not installed");
        }
        if !get("Repository").is_empty() {
            println!("From: {}", get("Repository"));
        }
        if !get("Vendor").is_empty() {
            println!("Vendor: {}", get("Vendor"));
        }
    }

    fn repos(&self, rows: &[RepoRow]) {
        // `brew tap` lists only the taps in use.
        for r in rows.iter().filter(|r| r.enabled) {
            println!("{}", r.alias);
        }
    }

    fn updates(&self, rows: &[UpdateRow], patches: bool, _took: Duration, _cmd: &str) {
        for u in rows {
            if patches {
                println!("{} ({})", u.name, u.category);
            } else {
                println!("{} ({}) < {}", u.name, u.old, u.new);
            }
        }
    }

    fn up_to_date(&self, _took: Duration) {}

    fn view(&self, kind: ViewKind, plan: Option<&Plan>) -> Box<dyn View + '_> {
        let items = plan.map(|p| p.items.clone()).unwrap_or_default();
        Box::new(BrewView {
            kind,
            items,
            spin: None,
            bar: None,
            current: None,
            cached: false,
            hooks: false,
            seen: HashSet::new(),
        })
    }

    fn already_installed(&self, name: &str) -> String {
        self.message(
            MsgKind::Warning,
            &format!("{name} is already installed and up-to-date."),
        )
    }

    fn note(&self, text: &str) -> String {
        self.message(MsgKind::Warning, text)
    }

    fn plan(&self, plan: &Plan, _took: Duration) {
        let list = |pred: &dyn Fn(&Item) -> bool| -> Vec<&Item> {
            plan.items.iter().filter(|i| pred(i)).collect()
        };
        let names = |v: &[&Item]| {
            v.iter()
                .map(|i| i.display_name())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let has_req = !plan.requested.is_empty();
        let deps = list(&|i| i.change == Change::Install && has_req && !plan.is_requested(i));
        let installs = list(&|i| i.change == Change::Install && (!has_req || plan.is_requested(i)));
        if !deps.is_empty() {
            let of = installs
                .first()
                .map(|i| format!(" for {}", i.name))
                .unwrap_or_default();
            ui::eprint_line(arrow(&format!(
                "Installing dependencies{of}: {}",
                names(&deps)
            )));
        }
        if !installs.is_empty() {
            ui::eprint_line(arrow(&format!("Installing {}", names(&installs))));
        }
        let ups = list(&|i| i.change == Change::Upgrade);
        if !ups.is_empty() {
            ui::eprint_line(arrow(&format!(
                "Upgrading {} outdated {}:",
                ups.len(),
                if ups.len() == 1 {
                    "package"
                } else {
                    "packages"
                }
            )));
            for i in ups {
                ui::eprint_line(format!(
                    "{} {} -> {}",
                    i.name,
                    i.edition_old.as_deref().unwrap_or("?"),
                    i.edition
                ));
            }
        }
        for (verb, change) in [
            ("Downgrading", Change::Downgrade),
            ("Reinstalling", Change::Reinstall),
            ("Changing architecture of", Change::ArchChange),
            ("Uninstalling", Change::Remove),
        ] {
            let v = list(&|i| i.change == change);
            if !v.is_empty() {
                ui::eprint_line(arrow(&format!("{verb} {}", names(&v))));
            }
        }
        let mut sizes = Vec::new();
        if plan.download > 0 {
            sizes.push(format!("{} to download", brew_size(plan.download)));
        }
        if plan.space_diff != 0 {
            let sign = if plan.space_diff > 0 { "+" } else { "-" };
            sizes.push(format!("{sign}{} on disk", brew_size(plan.space_diff)));
        }
        if !sizes.is_empty() {
            ui::eprint_line(ui::dim(&sizes.join(", ")));
        }
        if plan.need_reboot {
            ui::warning("a reboot will be required to complete this transaction.");
        }
    }

    fn confirm_question(&self, _plan: &Plan) -> String {
        "Proceed?".into()
    }

    fn nothing_to_do(&self, _took: Duration) {}

    fn dry_run_done(&self) {
        ui::eprint_line(arrow("Dry run: nothing was changed"));
    }

    fn aborted(&self) {}

    fn done(&self, plan: &Plan, took: Duration) {
        let mut parts = Vec::new();
        for (c, word) in [
            (Change::Install, "installed"),
            (Change::Upgrade, "upgraded"),
            (Change::Downgrade, "downgraded"),
            (Change::Reinstall, "reinstalled"),
            (Change::Remove, "uninstalled"),
        ] {
            let n = plan.count(c);
            if n > 0 {
                parts.push(format!("{n} {word}"));
            }
        }
        ui::eprint_line(arrow("Summary"));
        let secs = took.as_secs_f64().round() as u64;
        ui::eprint_line(format!(
            "🍺  {}, done in {secs} {}",
            parts.join(", "),
            if secs == 1 { "second" } else { "seconds" }
        ));
    }

    fn repo_refreshed(&self, repo: &str, updated: bool) -> String {
        if updated && let Ok(mut v) = UPDATED.lock() {
            v.push(repo.to_string());
        }
        String::new()
    }

    fn refresh_start(&self) {
        ui::eprint_line(arrow("Updating repositories..."));
    }

    fn refresh_done(&self, _count: usize, _took: Duration) {
        let v = UPDATED.lock().map(|v| v.clone()).unwrap_or_default();
        if v.is_empty() {
            ui::eprint_line("Already up-to-date.");
        } else {
            ui::eprint_line(format!(
                "Updated {} {} ({}).",
                v.len(),
                if v.len() == 1 {
                    "repository"
                } else {
                    "repositories"
                },
                v.join(", ")
            ));
        }
    }
}

// ---------------------------------------------------------------- live view

struct BrewView {
    kind: ViewKind,
    items: Vec<Item>,
    /// Spinner shown while bottles download in parallel.
    spin: Option<ProgressBar>,
    /// curl bar for a single download: (key, bar).
    bar: Option<(String, ProgressBar)>,
    /// Package being poured: (verb, counter, item, name) — gets its 🍺 when done.
    current: Option<(Verb, usize, Option<Item>, String)>,
    cached: bool,
    hooks: bool,
    seen: HashSet<String>,
}

impl BrewView {
    fn item_for(&self, s: &str) -> Option<Item> {
        let plan = Plan {
            items: self.items.clone(),
            ..Default::default()
        };
        plan.lookup(s).cloned()
    }

    fn end_bar(&mut self) {
        if let Some((_, pb)) = self.bar.take() {
            pb.finish_and_clear();
            ui::eprint_line(curl_done());
        }
    }

    fn end_spin(&mut self) {
        if let Some(sp) = self.spin.take() {
            sp.finish_and_clear();
            self.cached = true;
        }
    }

    fn pour_done(&mut self) {
        if let Some((verb, _, item, name)) = self.current.take() {
            if verb == Verb::Remove {
                return;
            }
            let ver = item.map(|i| i.edition).unwrap_or_default();
            self.println(&format!("🍺  {name} {ver}"));
        }
    }
}

impl View for BrewView {
    fn status(&mut self, _text: &str) {}

    fn step(&mut self, name: &str, _pct: Option<u64>, _done: bool) {
        if self.kind != ViewKind::Commit {
            return;
        }
        if name.starts_with("Running post-transaction") || name.starts_with("Running %") {
            if !self.hooks {
                self.hooks = true;
                self.pour_done();
                self.println(&arrow("Running post-install scripts"));
            }
            let n = name.trim().to_string();
            if n.starts_with("Running %") && self.seen.insert(n.clone()) {
                self.println(&ui::dim(&n));
            }
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
        let item = self.item_for(nevra);
        let name = item
            .as_ref()
            .map(|i| i.name.clone())
            .unwrap_or_else(|| nevra.to_string());
        if verb == Verb::Download {
            self.end_spin();
            if self.cached {
                return;
            }
            let key = format!("dl {i}");
            if self.bar.as_ref().map(|(k, _)| k) != Some(&key) {
                self.end_bar();
                self.println(&arrow(&format!("Fetching {name}")));
                self.println(&arrow(&format!("Downloading {nevra}")));
                self.bar = Some((key, curl_bar()));
            }
            return;
        }
        self.end_spin();
        self.end_bar();
        if self.current.as_ref().map(|(v, n, _, _)| (*v, *n)) != Some((verb, i)) {
            self.pour_done();
            match (verb, &item) {
                (Verb::Remove, _) => self.println(&format!("Uninstalling {name}... ")),
                (_, Some(it)) => {
                    if let (Change::Upgrade, Some(old)) = (it.change, &it.edition_old) {
                        self.println(&arrow(&format!("Upgrading {name}")));
                        self.println(&format!("  {old} -> {}", it.edition));
                    }
                    self.println(&arrow(&format!(
                        "Pouring {}--{}.{}.bottle.tar.gz",
                        it.name, it.edition, it.arch
                    )));
                }
                (_, None) => self.println(&arrow(&format!("Pouring {nevra}"))),
            }
            self.current = Some((verb, i, item, name));
        }
        if done {
            self.pour_done();
        }
    }

    fn download(&mut self, _file: &str, pct: Option<u64>, _rate: Option<i64>) {
        if let (Some((_, pb)), Some(p)) = (&self.bar, pct) {
            pb.set_position(p.min(100) * 10);
        }
    }

    fn preload(&mut self, detail: &str, _pct: Option<u64>, done: bool) {
        if self.kind != ViewKind::Commit {
            return;
        }
        if done {
            self.end_spin();
            return;
        }
        let sp = self.spin.get_or_insert_with(|| {
            let pb = ProgressBar::new_spinner();
            pb.set_style(
                ProgressStyle::with_template("{spinner:.blue} {msg}")
                    .unwrap()
                    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
            );
            pb.enable_steady_tick(Duration::from_millis(80));
            pb
        });
        sp.set_message(format!("Downloading bottles {detail}"));
    }

    fn preloaded(&mut self, file: &str) {
        if self.kind != ViewKind::Commit {
            return;
        }
        let base = file.trim_end_matches(".rpm");
        let line = match self.item_for(base) {
            Some(i) => format!("{} Bottle {} ({})", tick(), i.name, i.edition),
            None => format!("{} Bottle {base}", tick()),
        };
        self.println(&line);
    }

    fn println(&self, line: &str) {
        let pb = self.bar.as_ref().map(|(_, pb)| pb).or(self.spin.as_ref());
        println_above(pb, line);
    }

    fn finish(&mut self) {
        self.end_spin();
        self.end_bar();
        self.pour_done();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(brew_size(6_300_000), "6.3MB");
        let cols = columns(&["a".into(), "b".into(), "c".into()]);
        assert!(!cols.is_empty());
        console::set_colors_enabled(false);
        assert_eq!(
            Brew.message(MsgKind::Error, "No provider of 'x' found."),
            "Error: No available formula with the name \"x\"."
        );
    }
}
