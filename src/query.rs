//! Read-only commands: search, info, repos, updates. These don't need root.
//! This module only gathers data; the active theme does all the printing.

use std::collections::HashMap;
use std::time::Instant;

use anyhow::Result;

use crate::theme::{InfoFields, RepoRow, SearchRow, UpdateRow, theme};
use crate::ui;
use crate::xml::Node;
use crate::zypper::{self, Output, exit};

pub struct SearchOpts {
    pub installed: bool,
    pub not_installed: bool,
    pub details: bool,
    pub description: bool,
    pub exact: bool,
}

fn solvables(out: &Output) -> Vec<&Node> {
    let mut sols = Vec::new();
    if let Some(r) = out.find("search-result") {
        r.descendants("solvable", &mut sols);
    }
    sols.retain(|s| s.attr("kind") != Some("srcpackage"));
    sols
}

fn row(s: &Node) -> SearchRow {
    SearchRow {
        name: s.attr_or("name", "").to_string(),
        kind: s.attr_or("kind", "package").to_string(),
        installed: s.attr("status") == Some("installed"),
        installed_version: None,
        summary: s.attr_or("summary", "").to_string(),
        version: s.attr_or("edition", "").to_string(),
        arch: s.attr_or("arch", "").to_string(),
        repo: s.attr_or("repository", "").to_string(),
    }
}

/// Short repo label for pacman-style "repo/name": prefer the alias, minus
/// the noisy "download.opensuse.org-" / "repo-" prefixes.
fn short_alias(alias: &str) -> String {
    alias
        .trim_start_matches("download.opensuse.org-")
        .trim_start_matches("repo-")
        .to_string()
}

pub fn search(terms: &[String], o: SearchOpts) -> Result<i32> {
    let t = Instant::now();
    let th = theme();
    let sp = th.spinner("Searching…");
    let mut base: Vec<&str> = vec!["search"];
    if o.installed {
        base.push("--installed-only");
    }
    if o.not_installed {
        base.push("--not-installed-only");
    }
    if o.description {
        base.push("--search-descriptions");
    }
    if o.exact {
        base.push("--match-exact");
    }
    let mut detailed = base.clone();
    detailed.push("--details");
    base.extend(terms.iter().map(String::as_str));
    detailed.extend(terms.iter().map(String::as_str));

    let merge = th.search_wants_details();
    let want_details = o.details || merge;
    // Summaries only come with the plain search, versions/repos only with
    // --details; when the theme wants both, run them (and `repos`) in parallel.
    let (plain, details, repos) = std::thread::scope(|s| {
        let p = (!want_details || merge).then(|| s.spawn(|| zypper::query(&base)));
        let d = want_details.then(|| s.spawn(|| zypper::query(&detailed)));
        let r = merge.then(|| s.spawn(|| zypper::query(&["repos"])));
        let join = |h: Option<std::thread::ScopedJoinHandle<'_, Result<Output>>>| {
            h.map(|h| h.join().expect("zypper query thread panicked"))
        };
        (join(p), join(d), join(r))
    });
    sp.finish_and_clear();
    let plain = plain.transpose()?;
    let details = details.transpose()?;
    let repos = repos.transpose()?;

    let query = terms.join(" ");
    let primary = details.as_ref().or(plain.as_ref()).expect("at least one search ran");
    if primary.code == exit::INF_CAP_NOT_FOUND {
        th.search_empty(&query, t.elapsed());
        return Ok(1);
    }
    if !exit::is_success(primary.code) {
        return Ok(zypper::report_failure(primary));
    }

    let rows: Vec<SearchRow> = if merge {
        let summaries: HashMap<(String, String), String> = plain
            .as_ref()
            .map(|p| {
                solvables(p)
                    .into_iter()
                    .map(|s| {
                        let r = row(s);
                        ((r.name, r.kind), r.summary)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let aliases: HashMap<String, String> = repos
            .as_ref()
            .and_then(|r| r.find("repo-list"))
            .map(|l| {
                l.children_named("repo")
                    .map(|r| {
                        (r.attr_or("name", "").to_string(), short_alias(r.attr_or("alias", "")))
                    })
                    .collect()
            })
            .unwrap_or_default();
        merge_rows(details.as_ref().map(solvables).unwrap_or_default(), &summaries, &aliases, o.details)
    } else {
        solvables(primary).into_iter().map(row).collect()
    };
    th.search(&rows, &query, o.details, t.elapsed());
    Ok(0)
}

/// Collapse `--details` rows (one per version and repo) into one row per
/// package, pacman-style: newest repo version, with installed state.
fn merge_rows(
    sols: Vec<&Node>,
    summaries: &HashMap<(String, String), String>,
    aliases: &HashMap<String, String>,
    keep_all: bool,
) -> Vec<SearchRow> {
    let mut order: Vec<(String, String)> = Vec::new();
    let mut groups: HashMap<(String, String), Vec<SearchRow>> = HashMap::new();
    for s in sols {
        let r = row(s);
        let key = (r.name.clone(), r.kind.clone());
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(r);
    }
    let mut out = Vec::new();
    for key in order {
        let rows = groups.remove(&key).unwrap_or_default();
        let summary = summaries.get(&key).cloned().unwrap_or_default();
        let installed = rows.iter().find(|r| r.installed).map(|r| r.version.clone());
        let label = |repo: &str| match aliases.get(repo) {
            Some(a) => a.clone(),
            None if repo.starts_with('@') || repo.contains("System Packages") => "local".into(),
            None => repo.to_string(),
        };
        let finish = |mut r: SearchRow| {
            r.repo = label(&r.repo);
            r.summary = summary.clone();
            match &installed {
                Some(v) if *v == r.version => r.installed = true,
                Some(v) => {
                    r.installed = true;
                    r.installed_version = Some(v.clone());
                }
                None => {}
            }
            r
        };
        if keep_all {
            let mut seen = std::collections::HashSet::new();
            for r in rows {
                if seen.insert((r.version.clone(), r.repo.clone())) {
                    out.push(finish(r));
                }
            }
        } else {
            // zypper lists the best candidate first; skip "@System" rows
            // when a repository provides the package.
            let best = rows
                .iter()
                .find(|r| !r.repo.contains("System Packages") && !r.repo.starts_with('@'))
                .or(rows.first())
                .cloned();
            if let Some(r) = best {
                out.push(finish(r));
            }
        }
    }
    out
}

pub fn info(pkgs: &[String]) -> Result<i32> {
    let th = theme();
    let sp = th.spinner("Loading package information…");
    let mut args = vec!["info"];
    args.extend(pkgs.iter().map(String::as_str));
    let out = zypper::query(&args)?;
    sp.finish_and_clear();

    let mut shown = 0;
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
        if shown > 0 {
            println!();
        }
        shown += 1;
        th.info(&fields);
    }
    if shown == 0 && out.messages("error").next().is_none() && out.messages("warning").next().is_none() {
        ui::error(format!("no package found matching '{}'", pkgs.join(" ")));
        ui::hint(format!("try {}", ui::cmd(&format!("pzyp search {}", pkgs.join(" ")))));
        return Ok(exit::INF_CAP_NOT_FOUND);
    }
    Ok(if shown > 0 { 0 } else { out.code.max(1) })
}

/// Parse zypper's "Key : value" info block. Returns None for unrelated messages.
fn parse_info(text: &str) -> Option<InfoFields> {
    let mut fields: InfoFields = Vec::new();
    for line in text.lines() {
        let continuation = line.starts_with(' ') || line.starts_with('\t');
        if !continuation
            && let Some((k, v)) = line.split_once(" : ").or_else(|| line.split_once(" :"))
        {
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

pub fn repos() -> Result<i32> {
    let out = zypper::query(&["repos"])?;
    if !exit::is_success(out.code) {
        return Ok(zypper::report_failure(&out));
    }
    let rows: Vec<RepoRow> = out
        .find("repo-list")
        .map(|list| {
            list.children_named("repo")
                .map(|r| RepoRow {
                    alias: r.attr_or("alias", "").to_string(),
                    name: r.attr_or("name", "").to_string(),
                    enabled: r.attr("enabled") == Some("1"),
                    autorefresh: r.attr("autorefresh") == Some("1"),
                    priority: r.attr_or("priority", "99").to_string(),
                    url: r.child_text("url").unwrap_or("").trim().to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    theme().repos(&rows);
    Ok(0)
}

pub fn updates(patches: bool) -> Result<i32> {
    let t = Instant::now();
    let th = theme();
    let sp = th.spinner("Checking for updates…");
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
        th.up_to_date(t.elapsed());
        return Ok(0);
    }
    let rows: Vec<UpdateRow> = ups
        .iter()
        .map(|u| UpdateRow {
            name: u.attr_or("name", "").to_string(),
            arch: u.attr_or("arch", "").to_string(),
            repo: short_alias(u.child("source").map(|s| s.attr_or("alias", "")).unwrap_or("")),
            old: u.attr_or("edition-old", "").to_string(),
            new: u.attr_or("edition", "").to_string(),
            category: u.attr_or("category", "").to_string(),
            summary: u.child_text("summary").unwrap_or("").trim().to_string(),
        })
        .collect();
    let cmd = if patches {
        "pzyp patch"
    } else if is_rolling() {
        "pzyp dup"
    } else {
        "pzyp up"
    };
    th.updates(&rows, patches, t.elapsed(), cmd);
    Ok(0)
}

/// Tumbleweed / Slowroll should be upgraded with `dup`, not `up`.
pub fn is_rolling() -> bool {
    std::fs::read_to_string("/etc/os-release")
        .map(|s| s.contains("tumbleweed") || s.contains("slowroll"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_pacman_rows() {
        let xml = br#"<stream><search-result version="0.0"><solvable-list>
<solvable status="installed" name="bash" kind="package" edition="5.2-1" arch="x86_64" repository="(System Packages)"/>
<solvable status="other-version" name="bash" kind="package" edition="5.3-1" arch="x86_64" repository="Main Repository (OSS)"/>
<solvable status="not-installed" name="zsh" kind="package" edition="5.9-1" arch="x86_64" repository="Main Repository (OSS)"/>
</solvable-list></search-result></stream>"#;
        let nodes = crate::xml::parse_all(xml);
        let out = Output { code: 0, nodes, stderr: String::new() };
        let mut aliases = HashMap::new();
        aliases.insert("Main Repository (OSS)".to_string(), "oss".to_string());
        let rows = merge_rows(solvables(&out), &HashMap::new(), &aliases, false);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].repo, "oss");
        assert_eq!(rows[0].version, "5.3-1");
        assert_eq!(rows[0].installed_version.as_deref(), Some("5.2-1"));
        assert!(!rows[1].installed);
        assert_eq!(short_alias("download.opensuse.org-non-oss"), "non-oss");
    }
}
