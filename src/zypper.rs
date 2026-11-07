//! Spawning zypper.

use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::xml::{self, Node};

/// Well-known zypper exit codes (see `man zypper`, EXIT CODES).
pub mod exit {
    pub const OK: i32 = 0;
    pub const INF_UPDATE_NEEDED: i32 = 100;
    pub const INF_SEC_UPDATE_NEEDED: i32 = 101;
    pub const INF_REBOOT_NEEDED: i32 = 102;
    pub const INF_RESTART_NEEDED: i32 = 103;
    pub const INF_CAP_NOT_FOUND: i32 = 104;
    pub const ON_SIGNAL: i32 = 105;
    pub const INF_REPOS_SKIPPED: i32 = 106;
    pub const INF_RPM_SCRIPT_FAILED: i32 = 107;

    /// Codes that mean "it worked, but here's some information".
    pub fn is_success(code: i32) -> bool {
        matches!(
            code,
            OK | INF_UPDATE_NEEDED
                | INF_SEC_UPDATE_NEEDED
                | INF_REBOOT_NEEDED
                | INF_RESTART_NEEDED
                | INF_REPOS_SKIPPED
        )
    }
}

pub fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// Make sure we can run zypper as root. If we're not root, validate sudo
/// credentials *now*, before any spinner is drawn, so the password prompt is clean.
pub fn ensure_sudo() -> Result<bool> {
    if is_root() {
        return Ok(false);
    }
    let cached = Command::new("sudo")
        .args(["-n", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !cached {
        let st = Command::new("sudo")
            .args(["-v", "-p", "[pzyp] password for %u: "])
            .status()
            .context("failed to run sudo")?;
        if !st.success() {
            bail!("root privileges are required for this command");
        }
    }
    Ok(true)
}

pub fn command(sudo: bool) -> Command {
    let mut c = if sudo {
        let mut c = Command::new("sudo");
        c.arg("zypper");
        c
    } else {
        Command::new("zypper")
    };
    // We parse some of zypper's messages, so keep them in English.
    // (openSUSE's sudoers keeps LC_ALL in env_keep.)
    c.env("LC_ALL", "C.UTF-8");
    c
}

pub struct Output {
    pub code: i32,
    pub nodes: Vec<Node>,
    pub stderr: String,
}

impl Output {
    pub fn messages(&self, kind: &str) -> impl Iterator<Item = &str> {
        self.nodes
            .iter()
            .filter(move |n| n.name == "message" && n.attr("type") == Some(kind))
            .map(|n| n.text.trim())
    }

    pub fn find(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.name == name)
    }
}

/// Run a non-interactive zypper query with XML output and collect everything.
pub fn query(args: &[&str]) -> Result<Output> {
    let out = command(false)
        .args(["--xmlout", "--non-interactive", "--no-refresh"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("failed to run zypper (is it installed?)")?;
    Ok(Output {
        code: out.status.code().unwrap_or(exit::ON_SIGNAL),
        nodes: xml::parse_all(&out.stdout),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Spawn zypper with XML output. stdin is closed: zypper must be run with
/// --non-interactive, since it refuses to read answers from a non-terminal.
pub fn spawn_xml(sudo: bool, global: &[&str], args: &[String]) -> Result<Child> {
    let mut c = command(sudo);
    c.arg("--xmlout").args(global).args(args);
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c.spawn().context("failed to run zypper")
}

/// Report errors from a finished query in pzyp style; returns the exit code.
pub fn report_failure(out: &Output) -> i32 {
    let mut any = false;
    for m in out.messages("error") {
        crate::ui::error(m);
        any = true;
    }
    if !any {
        let e = out.stderr.trim();
        if !e.is_empty() {
            crate::ui::error(e);
        } else {
            crate::ui::error(format!("zypper exited with status {}", out.code));
        }
    }
    out.code
}
