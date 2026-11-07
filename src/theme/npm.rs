//! An npm-flavoured look: the classic `[####......] ⠋ reify:pkg: timing …`
//! gauge, npx's `Need to install the following packages: … Ok to proceed? (y)`,
//! `npm ERR!` / `npm WARN` / `npm notice`, `npm outdated`-style tables and
//! `added 3 packages, removed 1 package in 2s`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Result;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

use super::*;
use crate::ui::{self, Change};

pub struct Npm;

static REFRESH_UPDATED: AtomicUsize = AtomicUsize::new(0);

/// npm reports whole seconds, or milliseconds under a second.
fn npm_time(d: Duration) -> String {
    if d.as_millis() < 1000 {
        format!("{}ms", d.as_millis())
    } else {
        format!("{}s", d.as_secs_f64().round() as u64)
    }
}

fn count(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Prefix every line, as npm does for multi-line errors.
fn prefixed(prefix: &str, text: &str) -> String {
    text.lines().map(|l| format!("{prefix} {l}")).collect::<Vec<_>>().join("\n")
}

/// Pad cells to column widths. Widths are measured without ANSI codes.
fn table(header: &[&str], rows: &[Vec<String>]) {
    let mut w: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            w[i] = w[i].max(console::measure_text_width(c));
        }
    }
    let line = |cells: Vec<String>| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let pad = w[i].saturating_sub(console::measure_text_width(c));
                format!("{c}{}", " ".repeat(pad))
            })
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    println!("{}", line(header.iter().map(|h| style(h).underlined().to_string()).collect()));
    for r in rows {
        println!("{}", line(r.clone()));
    }
}

fn gauge() -> ProgressBar {
    let pb = ProgressBar::new(100);
    pb.set_style(
        ProgressStyle::with_template("[{bar:18}] {spinner} {wide_msg}")
            .unwrap()
            .progress_chars("#..")
            .tick_chars("-\\|/ "),
    );
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

/// "reify:ripgrep: timing reifyNode:…" with npm's coloured log level.
fn gauge_msg(stage: &str, pkg: &str, level: &str, detail: &str) -> String {
    let lvl = match level {
        "http" => style(level).green().to_string(),
        "timing" => style(level).magenta().to_string(),
        "sill" => style(level).dim().to_string(),
        _ => level.to_string(),
    };
    let pkg = if pkg.is_empty() { String::new() } else { format!(":{pkg}") };
    format!("{stage}{pkg}: {lvl} {detail}")
}

impl Theme for Npm {
    fn message(&self, kind: MsgKind, text: &str) -> String {
        let text = match text.strip_prefix("No provider of '").and_then(|r| r.strip_suffix("' found.")) {
            Some(pkg) => format!("code E404\n404 Not Found - '{pkg}' is not in this registry."),
            None => text.to_string(),
        };
        match kind {
            MsgKind::Error => prefixed(&format!("npm {}", style("ERR!").red()), &text),
            MsgKind::Warning => prefixed(&format!("npm {}", style("WARN").black().on_yellow()), &text),
        }
    }

    fn hint(&self, text: &str) -> String {
        prefixed(&format!("npm {}", style("notice").blue()), text)
    }

    fn spinner(&self, _msg: &str) -> ProgressBar {
        let pb = ProgressBar::new_spinner();
        pb.set_style(ProgressStyle::with_template("{spinner}").unwrap().tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "));
        pb.enable_steady_tick(Duration::from_millis(80));
        pb
    }

    fn confirm(&self, question: &str) -> Result<bool> {
        ask(&format!("{question} (y) "), true)
    }

    fn search_wants_details(&self) -> bool {
        true
    }

    fn search(&self, rows: &[SearchRow], query: &str, _details: bool, _took: Duration) {
        let q = query.to_lowercase();
        let desc_w = ui::term_width().saturating_sub(70).clamp(20, 60);
        let rows: Vec<Vec<String>> = rows
            .iter()
            .map(|r| {
                // npm search bolds the matched part of the name.
                let name = r.label();
                let shown = match name.to_lowercase().find(&q) {
                    Some(i) if !q.is_empty() => format!(
                        "{}{}{}",
                        &name[..i],
                        style(&name[i..i + q.len()]).bold(),
                        &name[i + q.len()..]
                    ),
                    _ => name.clone(),
                };
                vec![
                    shown,
                    format!("| {}", ui::truncate(&r.summary, desc_w)),
                    format!("| {}", r.version),
                    format!("| {}", r.repo),
                ]
            })
            .collect();
        table(&["NAME", "| DESCRIPTION", "| VERSION", "| REPOSITORY"], &rows);
    }

    fn search_empty(&self, query: &str, _took: Duration) {
        ui::eprint_line(format!("No matches found for \"{query}\""));
    }

    fn info(&self, fields: &InfoFields) {
        let get = |k: &str| fields.iter().find(|(f, _)| f == k).map(|(_, v)| v.as_str()).unwrap_or("");
        println!();
        let head = style(format!("{}@{}", get("Name"), get("Version"))).green().bold().underlined();
        let mut meta = vec![head.to_string()];
        if !get("Arch").is_empty() {
            meta.push(get("Arch").to_string());
        }
        if !get("Repository").is_empty() {
            meta.push(format!("repo: {}", style(get("Repository")).cyan()));
        }
        println!("{}", meta.join(" | "));
        println!("{}", get("Summary"));
        if !get("Upstream URL").is_empty() {
            println!("{}", style(get("Upstream URL")).cyan());
        }
        println!();
        println!("dist");
        if !get("Installed Size").is_empty() {
            println!(".unpackedSize: {}", style(get("Installed Size")).yellow());
        }
        if !get("Source package").is_empty() {
            println!(".source: {}", style(get("Source package")).cyan());
        }
        println!();
        if !get("Vendor").is_empty() {
            println!("maintainers:");
            println!("- {}", get("Vendor"));
            println!();
        }
        println!("status: {}", get("Status"));
        println!();
    }

    fn repos(&self, rows: &[RepoRow]) {
        println!("; \"zypper\" config from /etc/zypp/repos.d");
        for r in rows {
            println!();
            println!("{}", ui::dim(&format!("; {}", r.name)));
            let line = format!("{}:registry = \"{}\"", r.alias, r.url);
            if r.enabled {
                println!("{line}");
            } else {
                println!("{}", ui::dim(&format!("; {line} ; disabled")));
            }
        }
    }

    fn updates(&self, rows: &[UpdateRow], patches: bool, _took: Duration, _cmd: &str) {
        if patches {
            let rows: Vec<Vec<String>> = rows
                .iter()
                .map(|u| vec![style(&u.name).red().to_string(), u.category.clone(), u.summary.clone()])
                .collect();
            table(&["Patch", "Category", "Summary"], &rows);
            return;
        }
        let rows: Vec<Vec<String>> = rows
            .iter()
            .map(|u| {
                vec![
                    style(&u.name).red().to_string(),
                    u.old.clone(),
                    style(&u.new).green().to_string(),
                    style(&u.new).magenta().to_string(),
                    u.repo.clone(),
                ]
            })
            .collect();
        table(&["Package", "Current", "Wanted", "Latest", "Location"], &rows);
    }

    fn up_to_date(&self, _took: Duration) {}

    fn view(&self, kind: ViewKind, plan: Option<&Plan>) -> Box<dyn View + '_> {
        let stage = match kind {
            ViewKind::Resolve => "idealTree",
            ViewKind::Commit => "reify",
            ViewKind::Refresh => "update",
        };
        let pb = gauge();
        pb.set_message(gauge_msg(stage, "", "sill", "loadActual"));
        Box::new(NpmView {
            kind,
            stage,
            items: plan.map(|p| p.items.clone()).unwrap_or_default(),
            pb,
        })
    }

    fn already_installed(&self, _name: &str) -> String {
        String::new()
    }

    fn note(&self, text: &str) -> String {
        self.message(MsgKind::Warning, text)
    }

    fn plan(&self, plan: &Plan, _took: Duration) {
        let spec = |i: &Item| format!("{}@{}", i.display_name(), i.edition);
        let groups: [(&str, Vec<String>); 4] = [
            (
                "Need to install the following packages:",
                plan.items.iter().filter(|i| i.change == Change::Install).map(spec).collect(),
            ),
            (
                "Need to update the following packages:",
                plan.items
                    .iter()
                    .filter(|i| matches!(i.change, Change::Upgrade | Change::Downgrade))
                    .map(|i| format!("{}@{} -> {}", i.name, i.edition_old.as_deref().unwrap_or("?"), i.edition))
                    .collect(),
            ),
            (
                "Need to reinstall the following packages:",
                plan.items
                    .iter()
                    .filter(|i| matches!(i.change, Change::Reinstall | Change::ArchChange))
                    .map(spec)
                    .collect(),
            ),
            (
                "Need to remove the following packages:",
                plan.items.iter().filter(|i| i.change == Change::Remove).map(spec).collect(),
            ),
        ];
        for (title, list) in groups {
            if list.is_empty() {
                continue;
            }
            ui::eprint_line(title);
            for l in list {
                ui::eprint_line(format!("  {l}"));
            }
        }
        if plan.need_reboot {
            ui::warning("a reboot will be required to complete this transaction");
        }
    }

    fn confirm_question(&self, _plan: &Plan) -> String {
        "Ok to proceed?".into()
    }

    fn nothing_to_do(&self, took: Duration) {
        ui::eprint_line("");
        ui::eprint_line(format!("up to date in {}", npm_time(took)));
    }

    fn dry_run_done(&self) {
        ui::hint("dry run: no changes were made");
    }

    fn aborted(&self) {
        ui::error("canceled");
    }

    fn done(&self, plan: &Plan, took: Duration) {
        let added = plan.count(Change::Install);
        let removed = plan.count(Change::Remove);
        let changed = plan.items.len() - added - removed;
        let mut parts = Vec::new();
        if added > 0 {
            parts.push(format!("added {}", count(added, "package")));
        }
        if removed > 0 {
            parts.push(format!("removed {}", count(removed, "package")));
        }
        if changed > 0 {
            parts.push(format!("changed {}", count(changed, "package")));
        }
        let joined = match parts.len() {
            0 => "up to date".to_string(),
            1 => parts[0].clone(),
            n => format!("{}, and {}", parts[..n - 1].join(", "), parts[n - 1]),
        };
        ui::eprint_line("");
        ui::eprint_line(format!("{joined} in {}", npm_time(took)));
    }

    fn repo_refreshed(&self, _repo: &str, updated: bool) -> String {
        if updated {
            REFRESH_UPDATED.fetch_add(1, Ordering::Relaxed);
        }
        String::new()
    }

    fn refresh_done(&self, count_all: usize, took: Duration) {
        let updated = REFRESH_UPDATED.load(Ordering::Relaxed);
        let repos = |n| if n == 1 { "repository".to_string() } else { format!("{n} repositories") };
        ui::eprint_line("");
        if updated > 0 {
            ui::eprint_line(format!(
                "changed {}, and audited {} in {}",
                if updated == 1 { "1 repository".into() } else { repos(updated) },
                if count_all == 1 { "1 repository".into() } else { repos(count_all) },
                npm_time(took)
            ));
        } else {
            ui::eprint_line(format!(
                "up to date, audited {} in {}",
                if count_all == 1 { "1 repository".into() } else { repos(count_all) },
                npm_time(took)
            ));
        }
    }
}

// ---------------------------------------------------------------- live view

/// npm shows a single gauge line throughout, then clears it.
struct NpmView {
    kind: ViewKind,
    stage: &'static str,
    items: Vec<Item>,
    pb: ProgressBar,
}

impl NpmView {
    fn name_for(&self, s: &str) -> String {
        let plan = Plan { items: self.items.clone(), ..Default::default() };
        plan.lookup(s).map(|i| i.name.clone()).unwrap_or_else(|| s.to_string())
    }
}

impl View for NpmView {
    fn status(&mut self, text: &str) {
        let t = text.trim().trim_end_matches("...").to_lowercase();
        self.pb.set_message(gauge_msg(self.stage, "", "sill", &t));
    }

    fn step(&mut self, name: &str, pct: Option<u64>, done: bool) {
        if done {
            return;
        }
        let n = name.trim_end_matches(':').to_lowercase();
        self.pb.set_message(gauge_msg(self.stage, "", "timing", &n));
        if self.kind != ViewKind::Commit
            && let Some(p) = pct
        {
            self.pb.set_position(p.min(100));
        }
    }

    fn item(&mut self, verb: Verb, i: usize, total: usize, nevra: &str, _pct: Option<u64>, _done: bool) {
        let name = self.name_for(nevra);
        let (level, detail) = match verb {
            Verb::Download => ("http", format!("fetch GET 200 {nevra}.rpm")),
            Verb::Install => ("timing", format!("reifyNode:{nevra}")),
            Verb::Remove => ("timing", format!("removeNode:{nevra}")),
        };
        // Downloads and installs are numbered separately; only the install
        // sequence moves the gauge, so it never jumps backwards.
        if verb != Verb::Download {
            self.pb.set_length(total.max(1) as u64);
            self.pb.set_position(i.saturating_sub(1) as u64);
        }
        self.pb.set_message(gauge_msg(self.stage, &name, level, &detail));
    }

    fn download(&mut self, file: &str, pct: Option<u64>, _rate: Option<i64>) {
        let p = pct.map(|p| format!(" {p}%")).unwrap_or_default();
        self.pb.set_message(gauge_msg(self.stage, "", "http", &format!("fetch GET 200 {file}{p}")));
    }

    fn preload(&mut self, detail: &str, pct: Option<u64>, _done: bool) {
        if let Some(p) = pct {
            self.pb.set_length(100);
            self.pb.set_position(p.min(100));
        }
        self.pb.set_message(gauge_msg(self.stage, "", "http", &format!("fetch {detail}")));
    }

    fn preloaded(&mut self, file: &str) {
        let name = self.name_for(file.trim_end_matches(".rpm"));
        self.pb.set_message(gauge_msg(self.stage, &name, "http", &format!("fetch GET 200 {file}")));
    }

    fn println(&self, line: &str) {
        println_above(Some(&self.pb), line);
    }

    fn finish(&mut self) {
        self.pb.finish_and_clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(npm_time(Duration::from_millis(523)), "523ms");
        assert_eq!(npm_time(Duration::from_millis(2400)), "2s");
        console::set_colors_enabled(false);
        assert_eq!(
            Npm.message(MsgKind::Error, "No provider of 'x' found."),
            "npm ERR! code E404\nnpm ERR! 404 Not Found - 'x' is not in this registry."
        );
        assert_eq!(Npm.hint("dry run"), "npm notice dry run");
    }
}
