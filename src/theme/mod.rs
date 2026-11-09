//! Output themes. Everything pzyp prints goes through a [`Theme`], so the
//! look can be swapped (`--style`, `PZYP_STYLE`, or the config file).
//!
//! The rest of the program only gathers data (search rows, the transaction
//! plan, progress events) and hands it to the active theme.

mod apt;
mod brew;
mod dnf;
mod npm;
mod pacman;
mod uv;

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Result;
use clap::ValueEnum;
use indicatif::ProgressBar;

use crate::ui::Change;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum StyleName {
    /// Compact, colourful, uv-inspired output (default)
    #[default]
    Uv,
    /// Arch Linux pacman-style output
    Pacman,
    /// Fedora dnf-style output
    Dnf,
    /// Debian/Ubuntu apt-style output
    Apt,
    /// Homebrew-style output
    Brew,
    /// npm-style output
    Npm,
}

impl StyleName {
    pub fn parse(s: &str) -> Option<StyleName> {
        StyleName::from_str(s.trim(), true).ok()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            StyleName::Uv => "uv",
            StyleName::Pacman => "pacman",
            StyleName::Dnf => "dnf",
            StyleName::Apt => "apt",
            StyleName::Brew => "brew",
            StyleName::Npm => "npm",
        }
    }
}

static THEME: OnceLock<Box<dyn Theme>> = OnceLock::new();

pub fn init(style: StyleName, candy: bool) {
    pacman::set_candy(candy);
    let t: Box<dyn Theme> = match style {
        StyleName::Uv => Box::new(uv::Uv),
        StyleName::Pacman => Box::new(pacman::Pacman),
        StyleName::Dnf => Box::new(dnf::Dnf),
        StyleName::Apt => Box::new(apt::Apt),
        StyleName::Brew => Box::new(brew::Brew),
        StyleName::Npm => Box::new(npm::Npm),
    };
    let _ = THEME.set(t);
}

pub fn theme() -> &'static dyn Theme {
    THEME.get_or_init(|| Box::new(uv::Uv)).as_ref()
}

// ---------------------------------------------------------------- data

#[derive(Debug, Clone)]
pub struct Item {
    pub change: Change,
    pub name: String,
    pub kind: String,
    pub edition: String,
    pub edition_old: Option<String>,
    pub arch: String,
    pub repo: String,
}

impl Item {
    pub fn display_name(&self) -> String {
        if self.kind != "package" && !self.kind.is_empty() {
            format!("{}:{}", self.kind, self.name)
        } else {
            self.name.clone()
        }
    }
}

#[derive(Debug, Default)]
pub struct Plan {
    pub items: Vec<Item>,
    pub download: i64,
    pub space_diff: i64,
    pub installed_size: i64,
    pub removed_size: i64,
    pub need_reboot: bool,
    /// Names the user asked for (to tell them apart from dependencies).
    pub requested: Vec<String>,
}

impl Plan {
    pub fn count(&self, c: Change) -> usize {
        self.items.iter().filter(|i| i.change == c).count()
    }

    pub fn is_requested(&self, item: &Item) -> bool {
        self.requested.iter().any(|r| r == &item.name)
    }

    pub fn only(&self, c: Change) -> bool {
        !self.items.is_empty() && self.items.iter().all(|i| i.change == c)
    }

    /// Find the plan item a zypper label like "foo-1.0-1.x86_64" refers to.
    pub fn lookup(&self, nevra: &str) -> Option<&Item> {
        self.items
            .iter()
            .filter(|i| nevra.starts_with(&format!("{}-{}", i.name, i.edition)))
            .max_by_key(|i| i.name.len())
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchRow {
    pub name: String,
    pub kind: String,
    pub installed: bool,
    /// Installed version, when it differs from `version`.
    pub installed_version: Option<String>,
    pub summary: String,
    pub version: String,
    pub arch: String,
    pub repo: String,
}

impl SearchRow {
    pub fn label(&self) -> String {
        if self.kind == "package" || self.kind.is_empty() {
            self.name.clone()
        } else {
            format!("{}:{}", self.kind, self.name)
        }
    }
}

#[derive(Debug, Clone)]
pub struct RepoRow {
    pub alias: String,
    pub name: String,
    pub enabled: bool,
    pub autorefresh: bool,
    pub priority: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct UpdateRow {
    pub name: String,
    pub arch: String,
    pub repo: String,
    pub old: String,
    pub new: String,
    pub category: String,
    pub summary: String,
}

pub type InfoFields = Vec<(String, String)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Download,
    Install,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    /// The `--dry-run` resolve pass.
    Resolve,
    /// The real transaction.
    Commit,
    /// `pzyp refresh`.
    Refresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    Error,
    Warning,
}

// ---------------------------------------------------------------- traits

/// Live rendering of a running zypper process.
pub trait View {
    /// Free-form status chatter ("Loading repository data...").
    fn status(&mut self, text: &str);
    /// An uncounted step with optional percentage ("Checking for file conflicts").
    fn step(&mut self, name: &str, pct: Option<u64>, done: bool);
    /// A counted item: "(3/12) Installing: foo-1.0-1.x86_64".
    fn item(
        &mut self,
        verb: Verb,
        i: usize,
        total: usize,
        nevra: &str,
        pct: Option<u64>,
        done: bool,
    );
    /// Byte-level progress of a single download.
    fn download(&mut self, file: &str, pct: Option<u64>, rate: Option<i64>);
    /// Parallel package preloading ("3.1 MiB / 4.5 MiB · 2.5 KiB/s").
    fn preload(&mut self, detail: &str, pct: Option<u64>, done: bool);
    /// One preloaded file finished.
    fn preloaded(&mut self, file: &str);
    /// Print a line above any live progress.
    fn println(&self, line: &str);
    /// Clear live output and print any closing statistics.
    fn finish(&mut self);
}

pub trait Theme: Sync + Send {
    // -- general
    fn message(&self, kind: MsgKind, text: &str) -> String;
    fn hint(&self, text: &str) -> String;
    fn spinner(&self, msg: &str) -> ProgressBar;
    fn confirm(&self, question: &str) -> Result<bool>;

    // -- queries
    /// Whether search results should include version and repository.
    fn search_wants_details(&self) -> bool;
    fn search(&self, rows: &[SearchRow], query: &str, details: bool, took: Duration);
    fn search_empty(&self, query: &str, took: Duration);
    fn info(&self, fields: &InfoFields);
    fn repos(&self, rows: &[RepoRow]);
    fn updates(&self, rows: &[UpdateRow], patches: bool, took: Duration, cmd: &str);
    fn up_to_date(&self, took: Duration);

    // -- transactions
    fn view(&self, kind: ViewKind, plan: Option<&Plan>) -> Box<dyn View + '_>;
    fn resolve_start(&self) {}
    // Line-returning methods may return "" to print nothing.
    fn already_installed(&self, name: &str) -> String;
    fn note(&self, text: &str) -> String;
    fn plan(&self, plan: &Plan, took: Duration);
    fn confirm_question(&self, plan: &Plan) -> String;
    fn nothing_to_do(&self, took: Duration);
    fn dry_run_done(&self);
    fn aborted(&self);
    fn done(&self, plan: &Plan, took: Duration);
    fn repo_refreshed(&self, repo: &str, updated: bool) -> String;
    fn refresh_start(&self) {}
    fn refresh_done(&self, count: usize, took: Duration);
}

// ---------------------------------------------------------------- shared helpers

/// Ask a yes/no question on stderr. Piped input with EOF means "no".
pub(crate) fn ask(prompt: &str, default: bool) -> Result<bool> {
    use std::io::{IsTerminal, Write};
    let tty = std::io::stdin().is_terminal();
    let mut term = crate::ui::stderr();
    write!(term, "{prompt}")?;
    term.flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if !tty {
        let _ = term.write_line(line.trim());
        if line.is_empty() {
            return Ok(false);
        }
    }
    let a = line.trim().to_lowercase();
    Ok(if a.is_empty() {
        default
    } else {
        a == "y" || a == "yes"
    })
}

/// The names a style list should show, e.g. "uv, pacman, dnf, apt, brew, npm".
pub fn style_names() -> String {
    StyleName::value_variants()
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Split "1.2.3-4.5" into ("1.2.3", "4.5").
pub(crate) fn split_edition(ed: &str) -> (&str, &str) {
    ed.rsplit_once('-').unwrap_or((ed, ""))
}

/// Print through a progress bar if it's visible, else straight to stderr.
pub(crate) fn println_above(pb: Option<&ProgressBar>, line: &str) {
    match pb {
        // suspend() clears the bar, prints, and redraws it below. (println()
        // relies on cursor movement that misbehaves at the bottom of the screen.)
        Some(pb) if !pb.is_hidden() => pb.suspend(|| crate::ui::eprint_line(line)),
        _ => crate::ui::eprint_line(line),
    }
}
