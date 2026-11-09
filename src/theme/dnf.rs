//! A dnf5-flavoured look: the `Package / Arch / Version / Repository`
//! transaction table, `Transaction Summary`, `Is this ok [y/N]:`,
//! `[1/3] name-0:ver.arch  100% | 2.3 MiB/s | 00m01s` progress rows and
//! `Complete!`.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::Result;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

use super::*;
use crate::ui::{self, Change};

pub struct Dnf;

/// dnf5 writes packages as name-epoch:version-release.arch.
fn nevra(i: &Item) -> String {
    if i.arch.is_empty() {
        format!("{}-0:{}", i.name, i.edition)
    } else {
        format!("{}-0:{}.{}", i.name, i.edition, i.arch)
    }
}

fn dnf_time(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}m{:02}s", s / 60, s % 60)
}

fn repo_of(i: &Item) -> String {
    if i.change == Change::Remove || i.repo.is_empty() || i.repo.contains("System") {
        "@System".into()
    } else {
        i.repo.clone()
    }
}

fn loaded_lines() {
    ui::eprint_line("Updating and loading repositories:");
    ui::eprint_line("Repositories loaded.");
}

/// Width of the name column in a progress row.
fn prefix_w() -> usize {
    ui::term_width().clamp(60, 140).saturating_sub(34)
}

fn live(prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(100);
    pb.set_style(
        ProgressStyle::with_template(&format!(
            "{{prefix:{}!}} {{percent:>3}}% [{{bar:12}}] | {{msg:>12}} | {{dnftime}}",
            prefix_w().saturating_sub(15)
        ))
        .unwrap()
        .with_key(
            "dnftime",
            |s: &indicatif::ProgressState, w: &mut dyn std::fmt::Write| {
                let _ = w.write_str(&dnf_time(s.elapsed()));
            },
        )
        .progress_chars("== "),
    );
    pb.set_prefix(prefix.to_string());
    pb
}

fn done_row(prefix: &str, rate: &str, took: Duration) -> String {
    format!(
        "{} 100% | {:>12} | {}",
        ui::pad_or_truncate(prefix, prefix_w()),
        rate,
        dnf_time(took)
    )
}

impl Theme for Dnf {
    fn message(&self, kind: MsgKind, text: &str) -> String {
        if let Some(pkg) = text
            .strip_prefix("No provider of '")
            .and_then(|r| r.strip_suffix("' found."))
        {
            return format!("No match for argument: {pkg}");
        }
        match kind {
            MsgKind::Error => format!("{} {text}", style("Error:").red().bold()),
            MsgKind::Warning => format!("{} {text}", style("Warning:").yellow().bold()),
        }
    }

    fn hint(&self, text: &str) -> String {
        ui::dim(text)
    }

    fn spinner(&self, _msg: &str) -> ProgressBar {
        ProgressBar::hidden()
    }

    fn confirm(&self, question: &str) -> Result<bool> {
        // dnf defaults to "no".
        ask(&format!("{question} [y/N]: "), false)
    }

    fn search_wants_details(&self) -> bool {
        true
    }

    fn search(&self, rows: &[SearchRow], query: &str, _details: bool, _took: Duration) {
        loaded_lines();
        let q = query.to_lowercase();
        let label = |r: &SearchRow| {
            if r.arch.is_empty() {
                r.label()
            } else {
                format!("{}.{}", r.label(), r.arch)
            }
        };
        let w = rows
            .iter()
            .map(|r| label(r).chars().count())
            .max()
            .unwrap_or(0)
            .min(48);
        let groups = [
            (
                "name (exact)",
                rows.iter()
                    .filter(|r| r.name.to_lowercase() == q)
                    .collect::<Vec<_>>(),
            ),
            (
                "name",
                rows.iter()
                    .filter(|r| r.name.to_lowercase() != q && r.name.to_lowercase().contains(&q))
                    .collect(),
            ),
            (
                "summary",
                rows.iter()
                    .filter(|r| !r.name.to_lowercase().contains(&q))
                    .collect(),
            ),
        ];
        for (field, group) in groups {
            if group.is_empty() {
                continue;
            }
            println!("Matched fields: {field}");
            for r in group {
                let l = label(r);
                let pad = " ".repeat(w.saturating_sub(l.chars().count()));
                // Highlight the matched part of the name, like dnf5.
                let shown = match l.to_lowercase().find(&q) {
                    Some(i) if !q.is_empty() => format!(
                        "{}{}{}",
                        &l[..i],
                        style(&l[i..i + q.len()]).green().bold(),
                        &l[i + q.len()..]
                    ),
                    _ => l.clone(),
                };
                println!(" {shown}{pad}  {}", r.summary);
            }
        }
    }

    fn search_empty(&self, query: &str, _took: Duration) {
        loaded_lines();
        ui::eprint_line(format!("No matches found for \"{query}\"."));
    }

    fn info(&self, fields: &InfoFields) {
        let get = |k: &str| {
            fields
                .iter()
                .find(|(f, _)| f == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        };
        let installed =
            get("Status").starts_with("up-to-date") || get("Status").starts_with("out-of-date");
        println!(
            "{}",
            if installed {
                "Installed packages"
            } else {
                "Available packages"
            }
        );
        let (ver, rel) = split_edition(get("Version"));
        let row = |k: &str, v: &str| {
            let mut lines = v.lines();
            println!("{k:<16}: {}", lines.next().unwrap_or(""));
            for l in lines {
                println!("{:<16}: {l}", "");
            }
        };
        row("Name", get("Name"));
        row("Epoch", "0");
        row("Version", ver);
        row("Release", rel);
        row("Architecture", get("Arch"));
        row("Installed size", get("Installed Size"));
        let src = get("Source package");
        row(
            "Source",
            &if src.is_empty() {
                String::new()
            } else {
                format!("{src}.rpm")
            },
        );
        row("Repository", get("Repository"));
        row("Summary", get("Summary"));
        row("URL", get("Upstream URL"));
        row("Description", get("Description"));
        row("Vendor", get("Vendor"));
    }

    fn repos(&self, rows: &[RepoRow]) {
        let iw = rows.iter().map(|r| r.alias.len()).max().unwrap_or(7).max(7);
        let nw = rows.iter().map(|r| r.name.len()).max().unwrap_or(9).max(9);
        println!("{:<iw$}  {:<nw$}  status", "repo id", "repo name");
        for r in rows {
            let status = if r.enabled {
                style("enabled").green().to_string()
            } else {
                style("disabled").red().to_string()
            };
            println!("{:<iw$}  {:<nw$}  {status}", r.alias, r.name);
        }
    }

    fn updates(&self, rows: &[UpdateRow], patches: bool, _took: Duration, _cmd: &str) {
        loaded_lines();
        if patches {
            let nw = rows.iter().map(|u| u.name.len()).max().unwrap_or(4).max(4);
            println!("{:<nw$}  {:<11}  Summary", "Name", "Type");
            for u in rows {
                println!("{:<nw$}  {:<11}  {}", u.name, u.category, u.summary);
            }
            return;
        }
        let label = |u: &UpdateRow| format!("{}.{}", u.name, u.arch);
        let nw = rows.iter().map(|u| label(u).len()).max().unwrap_or(0);
        let vw = rows.iter().map(|u| u.new.len()).max().unwrap_or(0);
        for u in rows {
            println!("{:<nw$}  {:<vw$}  {}", label(u), u.new, u.repo);
        }
    }

    fn up_to_date(&self, _took: Duration) {
        loaded_lines();
    }

    fn view(&self, kind: ViewKind, plan: Option<&Plan>) -> Box<dyn View + '_> {
        let items = plan.map(|p| p.items.clone()).unwrap_or_default();
        let downloads = items
            .iter()
            .filter(|i| i.change != Change::Remove && (i.kind == "package" || i.kind.is_empty()))
            .count();
        Box::new(DnfView {
            kind,
            tx_total: items.len() + 1,
            items,
            downloads,
            download_size: plan.map(|p| p.download).unwrap_or(0),
            line: None,
            total: None,
            started: Instant::now(),
            dl_started: None,
            dl_count: 0,
            dl_done: false,
            rate: String::new(),
            tx_started: false,
            tx_index: 0,
            seen: HashSet::new(),
            preloaded_any: false,
        })
    }

    fn resolve_start(&self) {
        loaded_lines();
    }

    fn already_installed(&self, name: &str) -> String {
        format!("Package \"{name}\" is already installed.")
    }

    fn note(&self, text: &str) -> String {
        text.to_string()
    }

    fn plan(&self, plan: &Plan, _took: Duration) {
        let nw = plan
            .items
            .iter()
            .map(|i| i.display_name().len() + 2)
            .chain(
                plan.items
                    .iter()
                    .filter_map(|i| i.edition_old.as_ref().map(|_| i.name.len() + 13)),
            )
            .max()
            .unwrap_or(7)
            .clamp(7, 48);
        let aw = plan
            .items
            .iter()
            .map(|i| i.arch.len())
            .max()
            .unwrap_or(4)
            .max(4);
        let vw = plan
            .items
            .iter()
            .map(|i| {
                i.edition
                    .len()
                    .max(i.edition_old.as_ref().map_or(0, |o| o.len()))
            })
            .max()
            .unwrap_or(7)
            .max(7);
        ui::eprint_line(
            style(format!(
                "{:<nw$} {:<aw$} {:<vw$} Repository",
                "Package", "Arch", "Version"
            ))
            .bold()
            .to_string(),
        );
        let has_requested = !plan.requested.is_empty();
        type Pick<'a> = Box<dyn Fn(&Item) -> bool + 'a>;
        let sections: [(&str, Pick); 7] = [
            (
                "Installing:",
                Box::new(|i: &Item| {
                    i.change == Change::Install && (!has_requested || plan.is_requested(i))
                }),
            ),
            (
                "Installing dependencies:",
                Box::new(|i: &Item| {
                    i.change == Change::Install && has_requested && !plan.is_requested(i)
                }),
            ),
            (
                "Upgrading:",
                Box::new(|i: &Item| i.change == Change::Upgrade),
            ),
            (
                "Downgrading:",
                Box::new(|i: &Item| i.change == Change::Downgrade),
            ),
            (
                "Reinstalling:",
                Box::new(|i: &Item| i.change == Change::Reinstall),
            ),
            (
                "Changing architecture:",
                Box::new(|i: &Item| i.change == Change::ArchChange),
            ),
            ("Removing:", Box::new(|i: &Item| i.change == Change::Remove)),
        ];
        for (title, pick) in &sections {
            let items: Vec<&Item> = plan.items.iter().filter(|i| pick(i)).collect();
            if items.is_empty() {
                continue;
            }
            ui::eprint_line(*title);
            for i in items {
                let name = format!(" {}", i.display_name());
                let name = format!("{name:<nw$}");
                let name = match i.change {
                    Change::Remove => style(name).red().to_string(),
                    Change::Downgrade => style(name).magenta().to_string(),
                    _ => style(name).green().to_string(),
                };
                ui::eprint_line(format!(
                    "{name} {:<aw$} {:<vw$} {}",
                    i.arch,
                    i.edition,
                    repo_of(i)
                ));
                if let Some(old) = &i.edition_old {
                    let r = format!("   replacing {}", i.name);
                    ui::eprint_line(format!("{r:<nw$} {:<aw$} {old:<vw$} @System", i.arch));
                }
            }
        }
        ui::eprint_line("");
        ui::eprint_line("Transaction Summary:");
        let replacing = plan
            .items
            .iter()
            .filter(|i| i.edition_old.is_some())
            .count();
        let summary = [
            ("Installing:", plan.count(Change::Install)),
            ("Upgrading:", plan.count(Change::Upgrade)),
            ("Replacing:", replacing),
            ("Downgrading:", plan.count(Change::Downgrade)),
            ("Reinstalling:", plan.count(Change::Reinstall)),
            ("Removing:", plan.count(Change::Remove)),
        ];
        for (label, n) in summary {
            if n > 0 {
                ui::eprint_line(format!(" {label:<17}{}", ui::plural(n, "package")));
            }
        }
        ui::eprint_line("");
        if plan.download > 0 {
            let d = ui::bytes(plan.download);
            ui::eprint_line(format!(
                "Total size of inbound packages is {d}. Need to download {d}."
            ));
        }
        let diff = plan.space_diff;
        let detail = format!(
            "(install {}, remove {})",
            ui::bytes(plan.installed_size),
            ui::bytes(plan.removed_size)
        );
        if diff >= 0 {
            ui::eprint_line(format!(
                "After this operation, {} extra will be used {detail}.",
                ui::bytes(diff)
            ));
        } else {
            ui::eprint_line(format!(
                "After this operation, {} will be freed {detail}.",
                ui::bytes(-diff)
            ));
        }
        if plan.need_reboot {
            ui::warning("this transaction will require a reboot");
        }
    }

    fn confirm_question(&self, _plan: &Plan) -> String {
        "Is this ok".into()
    }

    fn nothing_to_do(&self, _took: Duration) {
        ui::eprint_line("Nothing to do.");
    }

    fn dry_run_done(&self) {
        ui::eprint_line("Test mode enabled: no changes were made.");
    }

    fn aborted(&self) {
        ui::eprint_line("Operation aborted by the user.");
    }

    fn done(&self, _plan: &Plan, _took: Duration) {
        ui::eprint_line("Complete!");
    }

    fn repo_refreshed(&self, repo: &str, updated: bool) -> String {
        if updated {
            done_row(&format!(" {repo}"), "", Duration::ZERO)
        } else {
            String::new()
        }
    }

    fn refresh_start(&self) {
        ui::eprint_line("Updating and loading repositories:");
    }

    fn refresh_done(&self, _count: usize, _took: Duration) {
        ui::eprint_line("Repositories loaded.");
        ui::eprint_line("Metadata cache created.");
    }
}

// ---------------------------------------------------------------- live view

struct DnfView {
    kind: ViewKind,
    items: Vec<Item>,
    downloads: usize,
    download_size: i64,
    /// The live row, keyed so repeated progress events update the same row.
    line: Option<(String, ProgressBar, Instant)>,
    total: Option<ProgressBar>,
    started: Instant,
    dl_started: Option<Instant>,
    dl_count: usize,
    dl_done: bool,
    rate: String,
    tx_started: bool,
    tx_index: usize,
    tx_total: usize,
    seen: HashSet<String>,
    preloaded_any: bool,
}

impl DnfView {
    fn item_for(&self, nevra_or_file: &str) -> Option<Item> {
        let plan = Plan {
            items: self.items.clone(),
            ..Default::default()
        };
        plan.lookup(nevra_or_file).cloned()
    }

    fn end_line(&mut self) {
        if let Some((key, pb, since)) = self.line.take() {
            let rate = if key.starts_with("dl ") {
                self.rate.as_str()
            } else {
                ""
            };
            let row = done_row(&pb.prefix(), rate, since.elapsed());
            pb.finish_and_clear();
            ui::eprint_line(row);
        }
    }

    fn row(&mut self, key: &str, prefix: &str) -> ProgressBar {
        if self.line.as_ref().map(|(k, _, _)| k.as_str()) != Some(key) {
            self.end_line();
            self.line = Some((key.to_string(), live(prefix), Instant::now()));
        }
        self.line.as_ref().unwrap().1.clone()
    }

    /// Close the download phase with dnf's rule and Total row.
    fn end_downloads(&mut self) {
        if self.dl_done {
            return;
        }
        self.dl_done = true;
        if let Some(pb) = self.total.take() {
            pb.finish_and_clear();
        }
        if self.dl_count == 0 {
            return;
        }
        self.end_line();
        let took = self.dl_started.map(|t| t.elapsed()).unwrap_or_default();
        let n = self.dl_count.max(self.downloads);
        ui::eprint_line("-".repeat(ui::term_width().clamp(60, 140).saturating_sub(1)));
        let size = if self.download_size > 0 {
            format!(" ({})", ui::bytes(self.download_size))
        } else {
            String::new()
        };
        ui::eprint_line(done_row(
            &format!("[{n}/{n}] Total{size}"),
            &self.rate,
            took,
        ));
    }

    fn start_tx(&mut self) {
        if !self.tx_started {
            if self.preloaded_any || self.dl_count > 0 {
                self.end_downloads();
            } else {
                self.dl_done = true; // interleaved downloads: no separate Total row
            }
            self.end_line();
            self.tx_started = true;
            ui::eprint_line("Running transaction");
        }
    }

    fn next_tx(&mut self) -> usize {
        self.tx_index += 1;
        self.tx_total = self.tx_total.max(self.tx_index);
        self.tx_index
    }
}

impl View for DnfView {
    fn status(&mut self, _text: &str) {}

    fn step(&mut self, name: &str, pct: Option<u64>, done: bool) {
        if self.kind != ViewKind::Commit {
            return;
        }
        let n = name.split(':').next().unwrap_or(name).trim().to_string();
        if n.is_empty() || n.starts_with("Running post-transaction") {
            return;
        }
        // rpm scriptlets: "Running %transfiletriggerin(man-2.13.1-8.1.x86_64)"
        if let Some(rest) = n.strip_prefix("Running %") {
            if self.seen.insert(n.clone()) {
                self.end_line();
                let line = match rest.split_once('(') {
                    Some((kind, pkg)) => format!(
                        ">>> Running %{kind} scriptlet: {}",
                        pkg.trim_end_matches(')')
                    ),
                    None => format!(">>> Running %{rest} scriptlet"),
                };
                self.println(&line);
            }
            return;
        }
        if self.seen.contains(&n) && self.line.as_ref().map(|(k, _, _)| k) != Some(&n) {
            return;
        }
        self.start_tx();
        let pb = if self.line.as_ref().map(|(k, _, _)| k) == Some(&n) {
            self.line.as_ref().unwrap().1.clone()
        } else {
            let k = self.next_tx();
            let t = self.tx_total;
            self.row(&n, &format!("[{k}/{t}] {n}"))
        };
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
        nevra_s: &str,
        pct: Option<u64>,
        done: bool,
    ) {
        if self.kind != ViewKind::Commit {
            return;
        }
        let item = self.item_for(nevra_s);
        let shown = item
            .as_ref()
            .map(nevra)
            .unwrap_or_else(|| nevra_s.to_string());
        if verb == Verb::Download {
            if self.preloaded_any {
                return; // preloaded already; zypper is just reading the cache
            }
            self.dl_started.get_or_insert_with(Instant::now);
            let key = format!("dl {i}");
            if self.line.as_ref().map(|(k, _, _)| k) != Some(&key) {
                self.dl_count += 1;
            }
            self.row(&key, &format!("[{i}/{total}] {shown}"));
            return;
        }
        self.start_tx();
        let key = format!("{verb:?} {i}");
        let pb = if self.line.as_ref().map(|(k, _, _)| k) == Some(&key) {
            self.line.as_ref().unwrap().1.clone()
        } else {
            let word = match (verb, item.as_ref().map(|i| i.change)) {
                (Verb::Remove, _) => "Removing",
                (_, Some(Change::Upgrade)) => "Upgrading",
                (_, Some(Change::Downgrade)) => "Downgrading",
                (_, Some(Change::Reinstall)) => "Reinstalling",
                _ => "Installing",
            };
            let k = self.next_tx();
            let t = self.tx_total;
            self.row(&key, &format!("[{k}/{t}] {word} {shown}"))
        };
        if let Some(p) = pct {
            pb.set_position(p.min(100));
        }
        if done {
            self.end_line();
        }
    }

    fn download(&mut self, _file: &str, pct: Option<u64>, rate: Option<i64>) {
        if let Some(r) = rate {
            self.rate = format!("{}/s", ui::bytes(r));
        }
        if let Some((k, pb, _)) = &self.line
            && k.starts_with("dl ")
        {
            if let Some(p) = pct {
                pb.set_position(p.min(100));
            }
            pb.set_message(self.rate.clone());
        }
    }

    fn preload(&mut self, detail: &str, pct: Option<u64>, done: bool) {
        if self.kind != ViewKind::Commit || self.dl_done {
            return;
        }
        self.preloaded_any = true;
        self.dl_started.get_or_insert_with(Instant::now);
        if let Some(rate) = detail.rsplit(" · ").next().filter(|r| r.ends_with("/s")) {
            self.rate = rate.to_string();
        }
        if done {
            self.end_downloads();
            return;
        }
        let n = self.downloads.max(self.dl_count);
        let pb = self
            .total
            .get_or_insert_with(|| live(&format!("[{n}/{n}] Total")))
            .clone();
        if let Some(p) = pct {
            pb.set_position(p.min(100));
        }
        pb.set_message(self.rate.clone());
    }

    fn preloaded(&mut self, file: &str) {
        if self.kind != ViewKind::Commit {
            return;
        }
        self.dl_started.get_or_insert_with(Instant::now);
        self.preloaded_any = true;
        self.dl_count += 1;
        let base = file.trim_end_matches(".rpm");
        let shown = self
            .item_for(base)
            .map(|i| nevra(&i))
            .unwrap_or_else(|| base.to_string());
        let n = self.downloads.max(self.dl_count);
        let row = done_row(
            &format!("[{}/{n}] {shown}", self.dl_count),
            "",
            self.started.elapsed(),
        );
        self.println(&row);
    }

    fn println(&self, line: &str) {
        let pb = self
            .line
            .as_ref()
            .map(|(_, pb, _)| pb)
            .or(self.total.as_ref());
        println_above(pb, line);
    }

    fn finish(&mut self) {
        self.end_line();
        if self.kind == ViewKind::Commit {
            self.end_downloads();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(dnf_time(Duration::from_secs(61)), "01m01s");
        let i = Item {
            change: Change::Install,
            name: "ripgrep".into(),
            kind: "package".into(),
            edition: "15.2.0-1.3".into(),
            edition_old: None,
            arch: "x86_64".into(),
            repo: "oss".into(),
        };
        assert_eq!(nevra(&i), "ripgrep-0:15.2.0-1.3.x86_64");
        console::set_colors_enabled(false);
        assert_eq!(
            Dnf.message(MsgKind::Error, "No provider of 'x' found."),
            "No match for argument: x"
        );
    }
}
