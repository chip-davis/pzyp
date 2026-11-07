//! Read-only commands: search, info, repos, updates. These don't need root.

use std::collections::HashSet;
use std::time::Instant;

use anyhow::Result;
use console::style;

use crate::ui;
use crate::zypper::{self, exit};

pub struct SearchOpts {
    pub installed: bool,
    pub not_installed: bool,
    pub details: bool,
    pub description: bool,
    pub exact: bool,
}

pub fn search(terms: &[String], o: SearchOpts) -> Result<i32> {
    let t = Instant::now();
    let sp = ui::spinner("Searching…");
    let mut args: Vec<&str> = vec!["search"];
    if o.installed {
        args.push("--installed-only");
    }
    if o.not_installed {
        args.push("--not-installed-only");
    }
    if o.details {
        args.push("--details");
    }
    if o.description {
        args.push("--search-descriptions");
    }
    if o.exact {
        args.push("--match-exact");
    }
    args.extend(terms.iter().map(String::as_str));
    let out = zypper::query(&args)?;
    sp.finish_and_clear();

    let query = terms.join(" ");
    if out.code == exit::INF_CAP_NOT_FOUND {
        ui::status(format!("No packages found matching '{query}' in {}", ui::elapsed(t.elapsed())));
        return Ok(1);
    }
    if !exit::is_success(out.code) {
        return Ok(zypper::report_failure(&out));
    }

    let mut sols = Vec::new();
    if let Some(r) = out.find("search-result") {
        r.descendants("solvable", &mut sols);
    }

    let width = ui::term_width();
    if o.details {
        // name, version, arch, repo
        let rows: Vec<_> = sols
            .iter()
            .filter(|s| s.attr("kind") != Some("srcpackage"))
            .collect();
        let nw = rows.iter().map(|s| s.attr_or("name", "").len()).max().unwrap_or(0).min(40);
        let vw = rows.iter().map(|s| s.attr_or("edition", "").len()).max().unwrap_or(0).min(30);
        for s in &rows {
            let installed = s.attr("status") == Some("installed");
            let mark = if installed { style("✓").green().bold().to_string() } else { " ".into() };
            let name = format!("{:nw$}", ui::truncate(s.attr_or("name", ""), nw));
            let name = if installed { style(name).green().bold().to_string() } else { style(name).bold().to_string() };
            println!(
                " {mark} {name}  {}  {}  {}",
                style(format!("{:vw$}", s.attr_or("edition", ""))).cyan(),
                ui::dim(&format!("{:7}", s.attr_or("arch", ""))),
                ui::dim(s.attr_or("repository", "")),
            );
        }
        ui::status(format!(
            "Found {} matching '{query}' in {}",
            ui::plural(rows.len(), "result"),
            ui::elapsed(t.elapsed())
        ));
        return Ok(0);
    }

    let mut seen = HashSet::new();
    let rows: Vec<_> = sols
        .into_iter()
        .filter(|s| s.attr("kind") != Some("srcpackage"))
        .filter(|s| seen.insert((s.attr_or("name", "").to_string(), s.attr_or("kind", "").to_string())))
        .collect();
    let label = |s: &crate::xml::Node| {
        let kind = s.attr_or("kind", "package");
        if kind == "package" {
            s.attr_or("name", "").to_string()
        } else {
            format!("{kind}:{}", s.attr_or("name", ""))
        }
    };
    let nw = rows.iter().map(|s| label(s).chars().count()).max().unwrap_or(0).min(40);
    for s in &rows {
        let installed = s.attr("status") == Some("installed");
        let mark = if installed { style("✓").green().bold().to_string() } else { " ".into() };
        let name_plain = ui::truncate(&label(s), nw);
        let pad = " ".repeat(nw.saturating_sub(name_plain.chars().count()));
        let name = if installed {
            style(&name_plain).green().bold().to_string()
        } else {
            style(&name_plain).bold().to_string()
        };
        let room = width.saturating_sub(nw + 6);
        let summary = ui::truncate(s.attr_or("summary", ""), room.max(10));
        println!(" {mark} {name}{pad}  {}", ui::dim(&summary));
    }
    let n_inst = rows.iter().filter(|s| s.attr("status") == Some("installed")).count();
    let extra = if n_inst > 0 { format!(" ({n_inst} installed)") } else { String::new() };
    ui::status(format!(
        "Found {}{extra} matching '{query}' in {}",
        ui::plural(rows.len(), "package"),
        ui::elapsed(t.elapsed())
    ));
    Ok(0)
}

pub fn info(pkgs: &[String]) -> Result<i32> {
    let sp = ui::spinner("Loading package information…");
    let mut args = vec!["info"];
    args.extend(pkgs.iter().map(String::as_str));
    let out = zypper::query(&args)?;
    sp.finish_and_clear();

    let mut shown = 0;
    let mut first = true;
    for n in &out.nodes {
        if n.name != "message" {
            continue;
        }
        let text = n.text.as_str();
        match n.attr("type") {
            Some("warning") => {
                ui::warning(text.trim());
                continue;
            }
            Some("error") => {
                ui::error(text.trim());
                continue;
            }
            _ => {}
        }
        let Some(fields) = parse_info(text) else { continue };
        if !first {
            println!();
        }
        first = false;
        shown += 1;
        render_info(&fields);
    }
    if shown == 0 && out.messages("error").next().is_none() && out.messages("warning").next().is_none() {
        ui::error(format!("no package found matching '{}'", pkgs.join(" ")));
        ui::hint(format!("try {}", ui::cmd(&format!("pzyp search {}", pkgs.join(" ")))));
        return Ok(exit::INF_CAP_NOT_FOUND);
    }
    Ok(if shown > 0 { 0 } else { out.code.max(1) })
}

/// Parse zypper's "Key : value" info block. Returns None for unrelated messages.
fn parse_info(text: &str) -> Option<Vec<(String, String)>> {
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some((_, v)) = fields.last_mut() {
                if !v.is_empty() {
                    v.push('\n');
                }
                v.push_str(line.trim());
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(" : ").or_else(|| line.split_once(" :")) {
            let k = k.trim();
            if !k.is_empty() && k.len() <= 24 {
                fields.push((k.to_string(), v.trim().to_string()));
                continue;
            }
        }
        if let Some((_, v)) = fields.last_mut() {
            if !v.is_empty() {
                v.push('\n');
            }
            v.push_str(line.trim());
        }
    }
    fields.iter().any(|(k, _)| k == "Name").then_some(fields)
}

fn render_info(fields: &[(String, String)]) {
    let get = |k: &str| fields.iter().find(|(f, _)| f == k).map(|(_, v)| v.as_str());
    let name = get("Name").unwrap_or("?");
    let version = get("Version").unwrap_or("");
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
    println!("{} {}  {status_s}", style(name).bold(), style(version).cyan());
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

pub fn repos() -> Result<i32> {
    let out = zypper::query(&["repos"])?;
    if !exit::is_success(out.code) {
        return Ok(zypper::report_failure(&out));
    }
    let mut repos = Vec::new();
    if let Some(list) = out.find("repo-list") {
        repos = list.children_named("repo").collect();
    }
    let aw = repos.iter().map(|r| r.attr_or("alias", "").len()).max().unwrap_or(0).min(36);
    let width = ui::term_width();
    let mut enabled = 0;
    for r in &repos {
        let on = r.attr("enabled") == Some("1");
        enabled += on as usize;
        let dot = if on { style("●").green().to_string() } else { ui::dim("○") };
        let alias = ui::truncate(r.attr_or("alias", ""), aw);
        let alias = format!("{alias:aw$}");
        let alias = if on { style(alias).bold().to_string() } else { ui::dim(&alias) };
        let prio = r.attr_or("priority", "99");
        let prio_s = if prio != "99" {
            format!(" {}", style(format!("p{prio}")).magenta())
        } else {
            String::new()
        };
        let refresh = if r.attr("autorefresh") == Some("1") { "" } else { " no-refresh" };
        println!(
            " {dot} {alias}  {}{prio_s}{}",
            r.attr_or("name", ""),
            ui::dim(refresh)
        );
        if let Some(url) = r.child_text("url") {
            let url = ui::truncate(url.trim(), width.saturating_sub(aw + 6).max(20));
            println!("   {:aw$}  {}", "", ui::dim(&url));
        }
    }
    let disabled = repos.len() - enabled;
    let extra = if disabled > 0 { format!(", {disabled} disabled") } else { String::new() };
    ui::status(format!(
        "{} {} enabled{extra}",
        enabled,
        if enabled == 1 { "repository" } else { "repositories" }
    ));
    Ok(0)
}

pub fn updates(patches: bool) -> Result<i32> {
    let t = Instant::now();
    let sp = ui::spinner("Checking for updates…");
    let args: &[&str] = if patches { &["list-patches"] } else { &["list-updates"] };
    let out = zypper::query(args)?;
    sp.finish_and_clear();
    if !exit::is_success(out.code) {
        return Ok(zypper::report_failure(&out));
    }
    let mut ups = Vec::new();
    if let Some(st) = out.find("update-status") {
        st.descendants("update", &mut ups);
    }
    if ups.is_empty() {
        ui::eprint_line(format!(
            "{} {} {}",
            style("✓").green().bold(),
            "Everything is up to date",
            ui::dim(&format!("(checked in {})", ui::elapsed(t.elapsed())))
        ));
        return Ok(0);
    }

    let nw = ups.iter().map(|u| u.attr_or("name", "").len()).max().unwrap_or(0).min(40);
    if patches {
        for u in &ups {
            let cat = u.attr_or("category", "");
            let cat_s = match cat {
                "security" => style(format!("{cat:11}")).red().bold().to_string(),
                "recommended" => style(format!("{cat:11}")).yellow().to_string(),
                _ => ui::dim(&format!("{cat:11}")),
            };
            println!(
                " {} {cat_s} {}  {}",
                ui::marker(ui::Change::Upgrade),
                style(format!("{:nw$}", u.attr_or("name", ""))).bold(),
                ui::dim(u.child_text("summary").unwrap_or("").trim())
            );
        }
    } else {
        let ow = ups.iter().map(|u| u.attr_or("edition-old", "").len()).max().unwrap_or(0);
        for u in &ups {
            let old = u.attr_or("edition-old", "");
            let (o, n) = ui::version_diff(old, u.attr_or("edition", ""));
            let pad = " ".repeat(ow.saturating_sub(old.len()));
            println!(
                " {} {}  {o}{pad} {} {n}",
                ui::marker(ui::Change::Upgrade),
                style(format!("{:nw$}", ui::truncate(u.attr_or("name", ""), nw))).bold(),
                ui::dim("→"),
            );
        }
    }
    let what = if patches { "patch" } else { "update" };
    let n = ups.len();
    ui::status(format!(
        "{} available (checked in {})",
        if patches && n != 1 { format!("{n} patches") } else { ui::plural(n, what) },
        ui::elapsed(t.elapsed())
    ));
    let cmd = if patches {
        "pzyp patch"
    } else if is_rolling() {
        "pzyp dup"
    } else {
        "pzyp up"
    };
    ui::hint(format!("run {} to install them", ui::cmd(cmd)));
    Ok(0)
}

/// Tumbleweed / Slowroll should be upgraded with `dup`, not `up`.
pub fn is_rolling() -> bool {
    std::fs::read_to_string("/etc/os-release")
        .map(|s| s.contains("tumbleweed") || s.contains("slowroll"))
        .unwrap_or(false)
}
