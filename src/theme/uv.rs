//! The default look, modelled on uv: dim status lines, `+`/`-` markers,
//! one spinner or progress bar at a time, timing everywhere.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::Result;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

use super::*;
use crate::ui::{self, Change};

pub struct Uv;

fn spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.dim} {wide_msg:.dim}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

fn bar(len: u64, verb: &str) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::with_template(
            "{prefix:>11.bold.cyan} {bar:30.green/dim} {pos:>4}/{len:<4} {wide_msg:.dim}",
        )
        .unwrap()
        .progress_chars("━━╌"),
    );
    pb.set_prefix(verb.to_string());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

fn breakdown(plan: &Plan, future: bool) -> String {
    [
        Change::Install,
        Change::Upgrade,
        Change::Downgrade,
        Change::Reinstall,
        Change::ArchChange,
        Change::Remove,
    ]
    .iter()
    .filter_map(|&c| {
        let n = plan.count(c);
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

impl Theme for Uv {
    fn message(&self, kind: MsgKind, text: &str) -> String {
        match kind {
            MsgKind::Error => format!("{} {text}", style("error:").red().bold()),
            MsgKind::Warning => format!("{} {text}", style("warning:").yellow().bold()),
        }
    }

    fn hint(&self, text: &str) -> String {
        format!("{} {text}", style("hint:").cyan().bold())
    }

    fn spinner(&self, msg: &str) -> ProgressBar {
        spinner(msg)
    }

    fn confirm(&self, question: &str) -> Result<bool> {
        ask(
            &format!("{} {} {} ", style("?").cyan().bold(), style(question).bold(), ui::dim("[Y/n]")),
            true,
        )
    }

    fn search_wants_details(&self) -> bool {
        false
    }

    fn search(&self, rows: &[SearchRow], query: &str, details: bool, took: Duration) {
        let width = ui::term_width();
        let mark = |r: &SearchRow| {
            if r.installed {
                style("✓").green().bold().to_string()
            } else {
                " ".into()
            }
        };
        let name_style = |r: &SearchRow, s: String| {
            if r.installed {
                style(s).green().bold().to_string()
            } else {
                style(s).bold().to_string()
            }
        };
        let nw = rows.iter().map(|r| r.label().chars().count()).max().unwrap_or(0).min(40);
        if details {
            let vw = rows.iter().map(|r| r.version.len()).max().unwrap_or(0).min(30);
            for r in rows {
                let name = format!("{:nw$}", ui::truncate(&r.label(), nw));
                println!(
                    " {} {}  {}  {}  {}",
                    mark(r),
                    name_style(r, name),
                    style(format!("{:vw$}", r.version)).cyan(),
                    ui::dim(&format!("{:7}", r.arch)),
                    ui::dim(&r.repo),
                );
            }
            ui::status(format!(
                "Found {} matching '{query}' in {}",
                ui::plural(rows.len(), "result"),
                ui::elapsed(took)
            ));
            return;
        }
        let mut seen = HashSet::new();
        let rows: Vec<_> = rows.iter().filter(|r| seen.insert(r.label())).collect();
        for r in &rows {
            let plain = ui::truncate(&r.label(), nw);
            let pad = " ".repeat(nw.saturating_sub(plain.chars().count()));
            let summary = ui::truncate(&r.summary, width.saturating_sub(nw + 6).max(10));
            println!(" {} {}{pad}  {}", mark(r), name_style(r, plain), ui::dim(&summary));
        }
        let n_inst = rows.iter().filter(|r| r.installed).count();
        let extra = if n_inst > 0 { format!(" ({n_inst} installed)") } else { String::new() };
        ui::status(format!(
            "Found {}{extra} matching '{query}' in {}",
            ui::plural(rows.len(), "package"),
            ui::elapsed(took)
        ));
    }

    fn search_empty(&self, query: &str, took: Duration) {
        ui::status(format!("No packages found matching '{query}' in {}", ui::elapsed(took)));
    }

    fn info(&self, fields: &InfoFields) {
        let get = |k: &str| fields.iter().find(|(f, _)| f == k).map(|(_, v)| v.as_str());
        let status = get("Status").unwrap_or("");
        let status_s = if status.starts_with("up-to-date") {
            style("● installed").green().to_string()
        } else if status.starts_with("out-of-date") {
            style(format!("● {status}")).yellow().to_string()
        } else if status.is_empty() {
            String::new()
        } else {
            ui::dim(&format!("○ {status}"))
        };
        println!(
            "{} {}  {status_s}",
            style(get("Name").unwrap_or("?")).bold(),
            style(get("Version").unwrap_or("")).cyan()
        );
        if let Some(s) = get("Summary") {
            println!("{s}");
        }
        println!();
        let skip = ["Name", "Version", "Summary", "Description", "Status", "Installed"];
        let rows: Vec<_> = fields
            .iter()
            .filter(|(k, v)| !skip.contains(&k.as_str()) && !v.is_empty())
            .collect();
        let kw = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        for (k, v) in rows {
            let val = if v.starts_with("http://") || v.starts_with("https://") {
                style(v).cyan().underlined().to_string()
            } else {
                v.to_string()
            };
            let mut lines = val.lines();
            println!("  {}  {}", ui::dim(&format!("{k:kw$}")), lines.next().unwrap_or(""));
            for l in lines {
                println!("  {}  {l}", " ".repeat(kw));
            }
        }
        if let Some(d) = get("Description").filter(|d| !d.trim().is_empty()) {
            println!();
            for l in d.lines() {
                println!("  {}", ui::dim(l));
            }
        }
    }

    fn repos(&self, rows: &[RepoRow]) {
        let aw = rows.iter().map(|r| r.alias.len()).max().unwrap_or(0).min(36);
        let width = ui::term_width();
        for r in rows {
            let dot = if r.enabled { style("●").green().to_string() } else { ui::dim("○") };
            let alias = format!("{:aw$}", ui::truncate(&r.alias, aw));
            let alias = if r.enabled { style(alias).bold().to_string() } else { ui::dim(&alias) };
            let prio = if r.priority != "99" {
                format!(" {}", style(format!("p{}", r.priority)).magenta())
            } else {
                String::new()
            };
            let refresh = if r.autorefresh { "" } else { " no-refresh" };
            println!(" {dot} {alias}  {}{prio}{}", r.name, ui::dim(refresh));
            if !r.url.is_empty() {
                let url = ui::truncate(&r.url, width.saturating_sub(aw + 6).max(20));
                println!("   {:aw$}  {}", "", ui::dim(&url));
            }
        }
        let enabled = rows.iter().filter(|r| r.enabled).count();
        let disabled = rows.len() - enabled;
        let extra = if disabled > 0 { format!(", {disabled} disabled") } else { String::new() };
        ui::status(format!(
            "{enabled} {} enabled{extra}",
            if enabled == 1 { "repository" } else { "repositories" }
        ));
    }

    fn updates(&self, rows: &[UpdateRow], patches: bool, took: Duration, cmd: &str) {
        let nw = rows.iter().map(|u| u.name.len()).max().unwrap_or(0).min(40);
        if patches {
            for u in rows {
                let cat = format!("{:11}", u.category);
                let cat_s = match u.category.as_str() {
                    "security" => style(cat).red().bold().to_string(),
                    "recommended" => style(cat).yellow().to_string(),
                    _ => ui::dim(&cat),
                };
                println!(
                    " {} {cat_s} {}  {}",
                    ui::marker(Change::Upgrade),
                    style(format!("{:nw$}", u.name)).bold(),
                    ui::dim(&u.summary)
                );
            }
        } else {
            let ow = rows.iter().map(|u| u.old.len()).max().unwrap_or(0);
            for u in rows {
                let (o, n) = ui::version_diff(&u.old, &u.new);
                let pad = " ".repeat(ow.saturating_sub(u.old.len()));
                println!(
                    " {} {}  {o}{pad} {} {n}",
                    ui::marker(Change::Upgrade),
                    style(format!("{:nw$}", ui::truncate(&u.name, nw))).bold(),
                    ui::dim("→"),
                );
            }
        }
        let n = rows.len();
        let what = if patches {
            if n == 1 { "1 patch".to_string() } else { format!("{n} patches") }
        } else {
            ui::plural(n, "update")
        };
        ui::status(format!("{what} available (checked in {})", ui::elapsed(took)));
        ui::hint(format!("run {} to install them", ui::cmd(cmd)));
    }

    fn up_to_date(&self, took: Duration) {
        ui::eprint_line(format!(
            "{} Everything is up to date {}",
            style("✓").green().bold(),
            ui::dim(&format!("(checked in {})", ui::elapsed(took)))
        ));
    }

    fn view(&self, kind: ViewKind, _plan: Option<&Plan>) -> Box<dyn View + '_> {
        let initial = match kind {
            ViewKind::Resolve => "Resolving dependencies…",
            ViewKind::Commit => "Preparing transaction…",
            ViewKind::Refresh => "Refreshing repositories…",
        };
        Box::new(UvView {
            spinner: Some(spinner(initial)),
            bar: None,
            stats: [Stat::default(); 3],
            preloaded: 0,
            last: [0; 3],
            preload_started: None,
            cached: false,
        })
    }

    fn already_installed(&self, name: &str) -> String {
        format!(
            " {} {} {}",
            style("✓").green().bold(),
            style(name).bold(),
            ui::dim("is already installed")
        )
    }

    fn note(&self, text: &str) -> String {
        format!(" {} {text}", ui::dim("·"))
    }

    fn plan(&self, plan: &Plan, took: Duration) {
        ui::status(format!(
            "Resolved {} in {}",
            ui::plural(plan.items.len(), "package"),
            ui::elapsed(took)
        ));
        let width = plan
            .items
            .iter()
            .map(|i| i.display_name().chars().count())
            .max()
            .unwrap_or(0)
            .min(44);
        for item in &plan.items {
            let name = item.display_name();
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
        let mut parts = vec![breakdown(plan, true)];
        if plan.download > 0 {
            parts.push(format!("{} to download", style(ui::bytes(plan.download)).bold()));
        }
        if plan.space_diff != 0 {
            let sign = if plan.space_diff > 0 { "+" } else { "" };
            parts.push(format!(
                "{} disk",
                style(format!("{sign}{}", ui::bytes(plan.space_diff))).bold()
            ));
        }
        ui::eprint_line("");
        ui::eprint_line(format!("  {}", parts.join(&ui::dim(" · "))));
        if plan.need_reboot {
            ui::warning("this transaction will require a reboot");
        }
    }

    fn confirm_question(&self, _plan: &Plan) -> String {
        "Proceed?".into()
    }

    fn nothing_to_do(&self, took: Duration) {
        ui::status(format!("Nothing to do (checked in {})", ui::elapsed(took)));
    }

    fn dry_run_done(&self) {
        ui::hint("dry run, no changes were made");
    }

    fn aborted(&self) {
        ui::status("Aborted, no changes were made");
    }

    fn done(&self, plan: &Plan, took: Duration) {
        ui::eprint_line(format!(
            "{} {} {}",
            style("✓").green().bold(),
            breakdown(plan, false),
            ui::dim(&format!("in {}", ui::elapsed(took)))
        ));
    }

    fn repo_refreshed(&self, repo: &str, updated: bool) -> String {
        if updated {
            format!(" {} {repo} {}", style("↓").cyan().bold(), style("updated").green())
        } else {
            format!(" {} {repo} {}", style("✓").green().bold(), ui::dim("up to date"))
        }
    }

    fn refresh_done(&self, count: usize, took: Duration) {
        ui::status(format!(
            "Refreshed {count} {} in {}",
            if count == 1 { "repository" } else { "repositories" },
            ui::elapsed(took)
        ));
    }
}

// ---------------------------------------------------------------- live view

const VERBS: [&str; 3] = ["Downloading", "Installing", "Removing"];

#[derive(Default, Clone, Copy)]
struct Stat {
    count: usize,
    time: Duration,
}

struct Bar {
    pb: ProgressBar,
    verb: usize,
    since: Instant,
}

/// zypper interleaves downloads and installs ("Retrieving 1/8",
/// "Installing 1/8", ...), so one bar switches label rather than restarting.
struct UvView {
    spinner: Option<ProgressBar>,
    bar: Option<Bar>,
    stats: [Stat; 3],
    preloaded: usize,
    last: [usize; 3],
    preload_started: Option<Instant>,
    /// Packages were preloaded; later "Retrieving" lines only read the cache.
    cached: bool,
}

impl UvView {
    fn set_status(&mut self, msg: &str) {
        if self.bar.is_some() {
            return;
        }
        let msg = msg.lines().next().unwrap_or("").trim().to_string();
        match &self.spinner {
            Some(sp) => sp.set_message(msg),
            None => self.spinner = Some(spinner(&msg)),
        }
    }

    fn end_preload(&mut self) {
        if let Some(t) = self.preload_started.take() {
            let line = ui::dim(&format!(
                "Downloaded {} in {}",
                ui::plural(self.preloaded, "package"),
                ui::elapsed(t.elapsed())
            ));
            self.println(&line);
            self.cached = true;
        }
    }
}

impl View for UvView {
    fn status(&mut self, text: &str) {
        self.set_status(text);
    }

    fn step(&mut self, name: &str, pct: Option<u64>, done: bool) {
        if done {
            return;
        }
        let pct = pct.map(|v| format!(" {v}%")).unwrap_or_default();
        self.set_status(&format!("{}{pct}", name.trim_end_matches(':')));
    }

    fn item(&mut self, verb: Verb, i: usize, total: usize, nevra: &str, _pct: Option<u64>, _done: bool) {
        self.end_preload();
        let v = verb as usize;
        if verb == Verb::Download && self.cached {
            return;
        }
        if let Some(sp) = self.spinner.take() {
            sp.finish_and_clear();
        }
        let b = self.bar.get_or_insert_with(|| Bar {
            pb: bar(total as u64, VERBS[v]),
            verb: v,
            since: Instant::now(),
        });
        if b.verb != v {
            self.stats[b.verb].time += b.since.elapsed();
            b.verb = v;
            b.since = Instant::now();
            b.pb.set_prefix(VERBS[v]);
        }
        b.pb.set_length(total as u64);
        b.pb.set_position(i.saturating_sub(1) as u64);
        b.pb.set_message(nevra.to_string());
        // zypper numbers installs and removals in one sequence, so count
        // distinct items rather than trusting the counter's value.
        if self.last[v] != i {
            self.last[v] = i;
            self.stats[v].count += 1;
        }
    }

    fn download(&mut self, file: &str, pct: Option<u64>, rate: Option<i64>) {
        let rate = rate.map(|r| format!(" · {}/s", ui::bytes(r))).unwrap_or_default();
        let pct = pct.map(|p| format!("{p}%")).unwrap_or_default();
        if let Some(b) = &self.bar {
            if b.verb == 0 {
                let base = b.pb.message();
                let base = base.split(" (").next().unwrap_or("").to_string();
                b.pb.set_message(format!("{base} ({pct}{rate})"));
            }
            return;
        }
        self.set_status(&format!("Downloading {file} {pct}{rate}"));
    }

    fn preload(&mut self, detail: &str, _pct: Option<u64>, done: bool) {
        self.preload_started.get_or_insert_with(Instant::now);
        if done {
            return;
        }
        if detail.is_empty() {
            self.set_status("Downloading packages…");
        } else {
            self.set_status(&format!("Downloading packages {detail}"));
        }
    }

    fn preloaded(&mut self, file: &str) {
        self.preload_started.get_or_insert_with(Instant::now);
        self.preloaded += 1;
        self.set_status(&format!("Downloaded {file}"));
    }

    fn println(&self, line: &str) {
        let pb = self.bar.as_ref().map(|b| &b.pb).or(self.spinner.as_ref());
        println_above(pb, line);
    }

    fn finish(&mut self) {
        self.end_preload();
        if let Some(sp) = self.spinner.take() {
            sp.finish_and_clear();
        }
        if let Some(b) = self.bar.take() {
            self.stats[b.verb].time += b.since.elapsed();
            b.pb.finish_and_clear();
        }
        for (v, past) in [(0, "Downloaded"), (1, "Installed"), (2, "Removed")] {
            let st = self.stats[v];
            if st.count > 0 && !(v == 0 && self.cached) {
                ui::status(format!(
                    "{past} {} in {}",
                    ui::plural(st.count, "package"),
                    ui::elapsed(st.time)
                ));
            }
        }
    }
}
