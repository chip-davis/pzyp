//! User configuration: `$XDG_CONFIG_HOME/pzyp/config.toml`
//! (usually `~/.config/pzyp/config.toml`).
//!
//! ```toml
//! style = "pacman"   # uv (default), pacman, dnf, apt, brew or npm
//! color = "auto"     # "auto" (default), "always" or "never"
//! candy = true       # pacman style: chomping progress bars (ILoveCandy)
//! ```
//!
//! Precedence: `--style` flag > `PZYP_STYLE` env var > config file > default.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::theme::StyleName;

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candy: Option<bool>,
}

/// Home directory of the real user, even under `sudo pzyp`.
fn home() -> Option<PathBuf> {
    if crate::zypper::is_root()
        && let Ok(user) = std::env::var("SUDO_USER")
        && let Ok(name) = std::ffi::CString::new(user)
    {
        // SAFETY: getpwnam returns a pointer to static storage or null; we copy
        // the string out immediately.
        unsafe {
            let pw = libc::getpwnam(name.as_ptr());
            if !pw.is_null() && !(*pw).pw_dir.is_null() {
                let dir = std::ffi::CStr::from_ptr((*pw).pw_dir);
                return Some(PathBuf::from(dir.to_string_lossy().into_owned()));
            }
        }
    }
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(x) if !crate::zypper::is_root() => PathBuf::from(x),
        _ => home()?.join(".config"),
    };
    Some(base.join("pzyp").join("config.toml"))
}

impl Config {
    /// Load the config file. A broken file is reported but never fatal.
    pub fn load() -> Config {
        let Some(p) = path() else {
            return Config::default();
        };
        let Ok(text) = std::fs::read_to_string(&p) else {
            return Config::default();
        };
        match toml::from_str(&text) {
            Ok(c) => c,
            Err(e) => {
                crate::ui::warning(format!("ignoring {}: {}", p.display(), e.message()));
                Config::default()
            }
        }
    }

    pub fn save(&self) -> Result<PathBuf> {
        let p = path().context("can't determine the config directory (is $HOME set?)")?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        let body = toml::to_string(self)?;
        let text = format!("# pzyp configuration\n{body}");
        std::fs::write(&p, text).with_context(|| format!("failed to write {}", p.display()))?;
        Ok(p)
    }

    pub fn get(&self, key: &str) -> Result<Option<&str>> {
        Ok(match key {
            "style" => self.style.as_deref(),
            "color" => self.color.as_deref(),
            "candy" => self.candy.map(|b| if b { "true" } else { "false" }),
            _ => bail!("unknown setting '{key}'"),
        })
    }

    pub fn set(&mut self, key: &str, value: Option<&str>) -> Result<()> {
        let value = value.map(|v| v.trim().to_lowercase());
        match key {
            "style" => {
                if let Some(v) = &value
                    && StyleName::parse(v).is_none()
                {
                    bail!(
                        "invalid style '{v}' (expected: {})",
                        crate::theme::style_names()
                    );
                }
                self.style = value;
            }
            "color" => {
                if let Some(v) = &value
                    && !matches!(v.as_str(), "auto" | "always" | "never")
                {
                    bail!("invalid color '{v}' (expected: auto, always, never)");
                }
                self.color = value;
            }
            "candy" => {
                self.candy = match value.as_deref() {
                    None => None,
                    Some("true" | "yes" | "on" | "1") => Some(true),
                    Some("false" | "no" | "off" | "0") => Some(false),
                    Some(v) => bail!("invalid candy '{v}' (expected: true, false)"),
                };
            }
            _ => bail!("unknown setting '{key}' (known: style, color, candy)"),
        }
        Ok(())
    }

    /// The style to use, given an optional CLI override.
    pub fn resolve_style(&self, flag: Option<StyleName>) -> StyleName {
        if let Some(s) = flag {
            return s;
        }
        if let Ok(env) = std::env::var("PZYP_STYLE") {
            match StyleName::parse(&env) {
                Some(s) => return s,
                None => crate::ui::warning(format!(
                    "ignoring PZYP_STYLE={env} (expected: {})",
                    crate::theme::style_names()
                )),
            }
        }
        match self.style.as_deref().map(StyleName::parse) {
            Some(Some(s)) => s,
            Some(None) => {
                crate::ui::warning(format!(
                    "ignoring style = \"{}\" in config (expected: {})",
                    self.style.as_deref().unwrap_or(""),
                    crate::theme::style_names()
                ));
                StyleName::default()
            }
            None => StyleName::default(),
        }
    }

    /// pacman style draws chomping bars unless `candy = false`.
    pub fn candy(&self) -> bool {
        self.candy.unwrap_or(true)
    }

    pub fn apply_color(&self) {
        match self.color.as_deref() {
            Some("always") => {
                console::set_colors_enabled(true);
                console::set_colors_enabled_stderr(true);
            }
            Some("never") => {
                console::set_colors_enabled(false);
                console::set_colors_enabled_stderr(false);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_validate() {
        let c: Config = toml::from_str("style = \"pacman\"\ncolor = \"never\"\n").unwrap();
        assert_eq!(c.style.as_deref(), Some("pacman"));
        assert_eq!(c.resolve_style(Some(StyleName::Uv)), StyleName::Uv);
        let mut c = Config::default();
        assert!(c.set("style", Some("fancy")).is_err());
        assert!(c.set("bogus", Some("x")).is_err());
        c.set("style", Some("Pacman")).unwrap();
        assert_eq!(c.style.as_deref(), Some("pacman"));
        assert!(toml::to_string(&c).unwrap().contains("style = \"pacman\""));
    }
}
