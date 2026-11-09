mod config;
mod query;
mod session;
mod theme;
mod ui;
mod xml;
mod zypper;

use std::process::{Command, ExitCode};

use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Args, Parser, Subcommand};

use config::Config;
use session::{Mode, Options};
use theme::StyleName;

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default())
    .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
    .valid(AnsiColor::Green.on_default())
    .invalid(AnsiColor::Yellow.on_default());

/// A prettier zypper.
///
/// Any command pzyp doesn't know is passed straight through to zypper,
/// so `pzyp addlock foo` works just like `zypper addlock foo`.
#[derive(Parser)]
#[command(name = "pzyp", version, styles = STYLES, allow_external_subcommands = true)]
struct Cli {
    /// Output style (overrides $PZYP_STYLE and the config file)
    #[arg(long, global = true, value_name = "STYLE")]
    style: Option<StyleName>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print a setting's value
    Get { key: String },
    /// Change a setting (e.g. `pzyp config set style pacman`)
    Set { key: String, value: String },
    /// Reset a setting to its default
    Unset { key: String },
    /// Print the config file's path
    Path,
}

#[derive(Args, Clone)]
struct TxArgs {
    /// Don't ask for confirmation
    #[arg(short, long)]
    yes: bool,
    /// Resolve and simulate the transaction without changing anything
    #[arg(short = 'D', long)]
    dry_run: bool,
    /// Show every message zypper prints
    #[arg(short, long)]
    verbose: bool,
    /// Accept package licenses without asking
    #[arg(long)]
    agree_licenses: bool,
}

impl TxArgs {
    fn opts(&self) -> Options {
        Options {
            yes: self.yes,
            dry_run: self.dry_run,
            verbose: self.verbose,
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Install packages
    #[command(visible_aliases = ["add", "in"])]
    Install {
        /// Packages, capabilities, or .rpm files to install
        #[arg(required = true)]
        packages: Vec<String>,
        /// Don't install recommended packages
        #[arg(long)]
        no_recommends: bool,
        #[command(flatten)]
        tx: TxArgs,
    },
    /// Remove packages
    #[command(visible_aliases = ["rm", "uninstall"])]
    Remove {
        #[arg(required = true)]
        packages: Vec<String>,
        /// Also remove dependencies that are no longer needed
        #[arg(short = 'u', long)]
        clean_deps: bool,
        #[command(flatten)]
        tx: TxArgs,
    },
    /// Update installed packages (all, or the ones given)
    #[command(visible_alias = "up")]
    Update {
        packages: Vec<String>,
        #[command(flatten)]
        tx: TxArgs,
    },
    /// Upgrade the whole distribution (the right way to update Tumbleweed)
    #[command(name = "dup", visible_aliases = ["dist-upgrade", "upgrade"])]
    Dup {
        /// Don't install recommended packages
        #[arg(long)]
        no_recommends: bool,
        #[command(flatten)]
        tx: TxArgs,
    },
    /// Install needed patches
    Patch {
        #[command(flatten)]
        tx: TxArgs,
    },
    /// Search for packages
    #[command(visible_aliases = ["se", "s"])]
    Search {
        #[arg(required = true)]
        terms: Vec<String>,
        /// Only show installed packages
        #[arg(short, long)]
        installed: bool,
        /// Only show packages that aren't installed
        #[arg(short = 'n', long, conflicts_with = "installed")]
        not_installed: bool,
        /// Show every version, architecture and repository
        #[arg(short = 's', long)]
        details: bool,
        /// Search descriptions too
        #[arg(short, long)]
        description: bool,
        /// Match the exact package name
        #[arg(short = 'x', long)]
        exact: bool,
    },
    /// Show package details
    #[command(visible_alias = "if")]
    Info {
        #[arg(required = true)]
        packages: Vec<String>,
    },
    /// List available updates
    #[command(visible_aliases = ["lu", "outdated"])]
    Updates {
        /// List patches instead of package updates
        #[arg(short, long)]
        patches: bool,
    },
    /// List repositories
    #[command(visible_aliases = ["lr", "repositories"])]
    Repos,
    /// Refresh repository metadata
    #[command(visible_alias = "ref")]
    Refresh {
        /// Force a full refresh
        #[arg(short, long)]
        force: bool,
        /// Only refresh these repositories
        repos: Vec<String>,
    },
    /// Show or change pzyp settings (style, color, candy)
    Config {
        #[command(subcommand)]
        action: Option<ConfigCmd>,
    },
    #[command(external_subcommand)]
    External(Vec<String>),
}

fn config_cmd(
    action: Option<ConfigCmd>,
    cfg: Config,
    flag: Option<StyleName>,
) -> anyhow::Result<i32> {
    let path = config::path();
    let path_s = path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(unknown)".into());
    match action {
        None => {
            println!("{}", ui::dim(&format!("# {path_s}")));
            let style = cfg.resolve_style(flag);
            let source = if flag.is_some() {
                "--style"
            } else if std::env::var("PZYP_STYLE").is_ok() {
                "PZYP_STYLE"
            } else if cfg.style.is_some() {
                "config"
            } else {
                "default"
            };
            println!(
                "style = \"{}\"  {}",
                style.as_str(),
                ui::dim(&format!(
                    "# {source}; {}",
                    theme::style_names().replace(", ", " | ")
                ))
            );
            let color = cfg.color.as_deref().unwrap_or("auto");
            let source = if cfg.color.is_some() {
                "config"
            } else {
                "default"
            };
            println!(
                "color = \"{color}\"  {}",
                ui::dim(&format!("# {source}; auto | always | never"))
            );
            let source = if cfg.candy.is_some() {
                "config"
            } else {
                "default"
            };
            println!(
                "candy = {}  {}",
                cfg.candy(),
                ui::dim(&format!("# {source}; pacman-style chomping bars"))
            );
            Ok(0)
        }
        Some(ConfigCmd::Path) => {
            println!("{path_s}");
            Ok(0)
        }
        Some(ConfigCmd::Get { key }) => {
            // Print the value in effect, including flag/env overrides.
            let v = cfg.get(&key)?;
            match key.as_str() {
                "style" => println!("{}", cfg.resolve_style(flag).as_str()),
                "candy" => println!("{}", cfg.candy()),
                _ => println!("{}", v.unwrap_or("auto")),
            }
            Ok(0)
        }
        Some(ConfigCmd::Set { key, value }) => {
            let mut cfg = cfg;
            cfg.set(&key, Some(&value))?;
            let p = cfg.save()?;
            ui::eprint_line(format!(
                "{} {}",
                console::style("✓").green().bold(),
                ui::dim(&format!(
                    "set {key} = \"{}\" in {}",
                    value.to_lowercase(),
                    p.display()
                ))
            ));
            Ok(0)
        }
        Some(ConfigCmd::Unset { key }) => {
            let mut cfg = cfg;
            cfg.set(&key, None)?;
            let p = cfg.save()?;
            ui::eprint_line(ui::dim(&format!("reset {key} in {}", p.display())));
            Ok(0)
        }
    }
}

fn tx(sub: &str, flags: &[&str], pkgs: &[String], t: &TxArgs) -> anyhow::Result<i32> {
    let mut args = vec![sub.to_string()];
    args.extend(flags.iter().map(|s| s.to_string()));
    if t.agree_licenses && sub != "remove" {
        args.push("--auto-agree-with-licenses".into());
    }
    args.extend(pkgs.iter().cloned());
    session::run(Mode::Transaction, args, t.opts())
}

/// Behave like a normal Unix tool when piped into `head` etc. Only for
/// read-only commands: transactions write to zypper's stdin and must not die
/// from SIGPIPE.
fn default_sigpipe() {
    // SAFETY: resetting a signal disposition to its default is always sound.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

fn run(cli: Cli, cfg: Config) -> anyhow::Result<i32> {
    if matches!(
        cli.command,
        Cmd::Search { .. } | Cmd::Info { .. } | Cmd::Updates { .. } | Cmd::Repos
    ) {
        default_sigpipe();
    }
    match cli.command {
        Cmd::Install {
            packages,
            no_recommends,
            tx: t,
        } => {
            let flags: &[&str] = if no_recommends {
                &["--no-recommends"]
            } else {
                &[]
            };
            tx("install", flags, &packages, &t)
        }
        Cmd::Remove {
            packages,
            clean_deps,
            tx: t,
        } => {
            let flags: &[&str] = if clean_deps { &["--clean-deps"] } else { &[] };
            tx("remove", flags, &packages, &t)
        }
        Cmd::Update { packages, tx: t } => {
            if packages.is_empty() && query::is_rolling() && !t.yes {
                ui::hint(format!(
                    "on Tumbleweed, {} is the recommended way to update",
                    ui::cmd("pzyp dup")
                ));
            }
            tx("update", &[], &packages, &t)
        }
        Cmd::Dup {
            no_recommends,
            tx: t,
        } => {
            let flags: &[&str] = if no_recommends {
                &["--no-recommends"]
            } else {
                &[]
            };
            tx("dist-upgrade", flags, &[], &t)
        }
        Cmd::Patch { tx: t } => tx("patch", &[], &[], &t),
        Cmd::Search {
            terms,
            installed,
            not_installed,
            details,
            description,
            exact,
        } => query::search(
            &terms,
            query::SearchOpts {
                installed,
                not_installed,
                details,
                description,
                exact,
            },
        ),
        Cmd::Info { packages } => query::info(&packages),
        Cmd::Updates { patches } => query::updates(patches),
        Cmd::Repos => query::repos(),
        Cmd::Refresh { force, repos } => {
            let mut args = vec!["refresh".to_string()];
            if force {
                args.push("--force".into());
            }
            args.extend(repos);
            session::run(
                Mode::Refresh,
                args,
                Options {
                    yes: false,
                    dry_run: false,
                    verbose: false,
                },
            )
        }
        Cmd::Config { action } => config_cmd(action, cfg, cli.style),
        Cmd::External(args) => {
            let st = Command::new("zypper").args(&args).status()?;
            Ok(st.code().unwrap_or(1))
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cfg = Config::load();
    cfg.apply_color();
    theme::init(cfg.resolve_style(cli.style), cfg.candy());
    match run(cli, cfg) {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(e) => {
            ui::error(format!("{e:#}"));
            ExitCode::from(2)
        }
    }
}
