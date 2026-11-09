//! Styling helpers, modelled on uv's output conventions:
//! dim status lines, `+`/`-` change markers, `error:`/`warning:`/`hint:` prefixes.

use std::time::Duration;

use console::{Style, Term, style};

use crate::theme::{MsgKind, theme};

pub fn stderr() -> Term {
    Term::stderr()
}

pub fn eprint_line(s: impl AsRef<str>) {
    let _ = stderr().write_line(s.as_ref());
}

/// A dim status line, e.g. "Resolved 12 packages in 140ms".
pub fn status(s: impl AsRef<str>) {
    eprint_line(style(s.as_ref()).dim().to_string());
}

pub fn error(s: impl AsRef<str>) {
    eprint_line(theme().message(MsgKind::Error, s.as_ref()));
}

pub fn warning(s: impl AsRef<str>) {
    eprint_line(theme().message(MsgKind::Warning, s.as_ref()));
}

pub fn hint(s: impl AsRef<str>) {
    eprint_line(theme().hint(s.as_ref()));
}

/// Highlight a command the user could run, the way uv does: `pzyp search foo`.
pub fn cmd(s: &str) -> String {
    style(format!("`{s}`")).green().to_string()
}

pub fn dim(s: &str) -> String {
    style(s).dim().to_string()
}

pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// uv-style elapsed formatting: 8ms, 1.42s, 2m 03s.
pub fn elapsed(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.2}s", d.as_secs_f64())
    } else {
        let s = d.as_secs();
        format!("{}m {:02}s", s / 60, s % 60)
    }
}

pub fn bytes(n: i64) -> String {
    let neg = n < 0;
    let mut v = n.unsigned_abs() as f64;
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    let s = if i == 0 {
        format!("{v:.0} {}", units[i])
    } else {
        format!("{v:.1} {}", units[i])
    };
    if neg { format!("-{s}") } else { s }
}

/// Split two version strings and highlight the part that changed in `new`.
pub fn version_diff(old: &str, new: &str) -> (String, String) {
    let common = old
        .char_indices()
        .zip(new.chars())
        .take_while(|((_, a), b)| a == b)
        .last()
        .map(|((i, c), _)| i + c.len_utf8())
        .unwrap_or(0);
    // Back up to a segment boundary so we highlight "1.2.10" -> "1.2.11" as ".11"-ish
    // rather than splitting in the middle of a number.
    let mut cut = common;
    while cut > 0 {
        let prev = new.as_bytes()[cut - 1];
        if prev == b'.' || prev == b'-' || prev == b'+' || prev == b'~' || prev == b'_' {
            break;
        }
        cut -= 1;
    }
    let old_s = style(old).dim().to_string();
    let new_s = format!(
        "{}{}",
        style(&new[..cut]).dim(),
        style(&new[cut..]).green().bold()
    );
    (old_s, new_s)
}

pub fn marker(kind: Change) -> String {
    let s = kind.symbol();
    let st: Style = match kind {
        Change::Install => Style::new().green().bold(),
        Change::Remove => Style::new().red().bold(),
        Change::Upgrade => Style::new().cyan().bold(),
        Change::Downgrade => Style::new().yellow().bold(),
        Change::Reinstall | Change::ArchChange => Style::new().magenta().bold(),
    };
    st.apply_to(s).to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    Upgrade,
    Install,
    Reinstall,
    ArchChange,
    Downgrade,
    Remove,
}

impl Change {
    pub fn symbol(self) -> &'static str {
        match self {
            Change::Install => "+",
            Change::Remove => "-",
            Change::Upgrade => "↑",
            Change::Downgrade => "↓",
            Change::Reinstall => "↻",
            Change::ArchChange => "~",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Change> {
        Some(match tag {
            "to-install" => Change::Install,
            "to-remove" => Change::Remove,
            "to-upgrade" | "to-upgrade-change-arch" => Change::Upgrade,
            "to-downgrade" | "to-downgrade-change-arch" => Change::Downgrade,
            "to-reinstall" => Change::Reinstall,
            "to-change-arch" => Change::ArchChange,
            _ => return None,
        })
    }

    pub fn past(self) -> &'static str {
        match self {
            Change::Install => "installed",
            Change::Remove => "removed",
            Change::Upgrade => "upgraded",
            Change::Downgrade => "downgraded",
            Change::Reinstall => "reinstalled",
            Change::ArchChange => "changed arch of",
        }
    }

    pub fn future(self) -> &'static str {
        match self {
            Change::Install => "install",
            Change::Remove => "remove",
            Change::Upgrade => "upgrade",
            Change::Downgrade => "downgrade",
            Change::Reinstall => "reinstall",
            Change::ArchChange => "change arch",
        }
    }
}

pub fn term_width() -> usize {
    Term::stdout()
        .size_checked()
        .map(|(_, w)| w as usize)
        .unwrap_or(100)
}

/// Pad (or truncate) to exactly `w` display columns.
pub fn pad_or_truncate(s: &str, w: usize) -> String {
    let t = truncate(s, w);
    let pad = w.saturating_sub(console::measure_text_width(&t));
    format!("{t}{}", " ".repeat(pad))
}

/// Truncate to a display width, adding an ellipsis.
pub fn truncate(s: &str, max: usize) -> String {
    if console::measure_text_width(s) <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".into();
    }
    console::truncate_str(s, max - 1, "").to_string() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(elapsed(Duration::from_millis(12)), "12ms");
        assert_eq!(elapsed(Duration::from_millis(1420)), "1.42s");
        assert_eq!(elapsed(Duration::from_secs(123)), "2m 03s");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(-5 * 1024 * 1024), "-5.0 MiB");
        assert_eq!(plural(1, "package"), "1 package");
        assert_eq!(plural(3, "package"), "3 packages");
    }

    #[test]
    fn diff_splits_on_segment() {
        console::set_colors_enabled(false);
        let (_, n) = version_diff("4.0.2-3.5", "4.0.2-3.6");
        assert_eq!(n, "4.0.2-3.6");
    }
}
