# pzyp

A prettier front-end for `zypper`. Supports several different 'themes' to change the look to match your favorite package manager.

![pzyp demo](assets/demo.gif)

## Requirements

- openSUSE (Tumbleweed, Slowroll or Leap) — or anything else with `zypper`.
- `sudo` for transactions when not running as root.

## Install

One-line installer (prebuilt binary for x86_64 / aarch64 Linux):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/chip-davis/pzyp/releases/latest/download/pzyp-installer.sh | sh
```

With [cargo-binstall](https://github.com/cargo-bins/cargo-binstall) (downloads a prebuilt binary):

```sh
cargo binstall pzyp
```

Or build from source with Cargo:

```sh
cargo install pzyp
```

Prebuilt binaries are also on the [releases page](https://github.com/chip-davis/pzyp/releases).

Or from a checkout of this repository:

```sh
cargo install --path .
```

## Commands

| pzyp                        | zypper equivalent |
|-----------------------------|-------------------|
| `pzyp install` / `add` / `in` | `zypper install`  |
| `pzyp remove` / `rm`        | `zypper remove`   |
| `pzyp update` / `up`        | `zypper update`   |
| `pzyp dup` / `upgrade`      | `zypper dist-upgrade` |
| `pzyp patch`                | `zypper patch`    |
| `pzyp search` / `se`        | `zypper search`   |
| `pzyp info`                 | `zypper info`     |
| `pzyp updates` / `lu`       | `zypper list-updates` (`-p` for patches) |
| `pzyp repos` / `lr`         | `zypper repos`    |
| `pzyp refresh` / `ref`      | `zypper refresh`  |

Transactions accept `-y/--yes`, `-D/--dry-run`, `-v/--verbose` and `--agree-licenses`.
Anything else (`pzyp addlock foo`, `pzyp ps`, …) is passed straight to zypper.

## Styles

pzyp can look like **uv** (the default), **pacman**, **dnf**, **apt**,
**brew** (Homebrew) or **npm**:

| Style    | Plan & prompt                                              | Progress                                        |
|----------|------------------------------------------------------------|-------------------------------------------------|
| `uv`     | `+ ripgrep 15.2.0` lines, `? Proceed? [Y/n]`               | one spinner/bar, `Installed 3 packages in 1.2s` |
| `pacman` | `Packages (3) …`, `:: Proceed with installation? [Y/n]`    | `[----C o o]` lines (ILoveCandy)                |
| `dnf`    | `Package / Arch / Version / Repository` table, `[y/N]`     | `[1/3] … 100% \| rate \| 00m01s`, `Complete!`   |
| `apt`    | `The following NEW packages will be installed:`            | `Get:1 …`, `Unpacking …`, pinned `Progress:` bar |
| `brew`   | `==> Installing ripgrep`, `==> Upgrading 2 outdated…`      | `✔︎ Bottle …`, `==> Pouring …`, `🍺`             |
| `npm`    | `Need to install the following packages: … Ok to proceed?` | `[####....] ⠋ reify:ripgrep: …`, `added 3 packages in 2s` |

Note that the `dnf` style keeps dnf's default answer of **no** at the prompt.

The pacman look:

```
$ pzyp --style pacman add ripgrep
resolving dependencies...
looking for conflicting packages...

Packages (2) ripgrep-15.2.0-1.3  ripgrep-bash-completion-15.2.0-1.3

Total Download Size:      1.70 MiB
Total Installed Size:     5.40 MiB

:: Proceed with installation? [Y/n]
:: Retrieving packages...
 ripgrep-15.2.0-1.3-x86_64                    [------------------------------] 100%
 ripgrep-bash-completion-15.2.0-1.3-noarch    [------------------------------] 100%
 Total (2/2)                                  [------------------------------] 100%
:: Processing package changes...
(1/2) installing ripgrep                      [------------------------------] 100%
(2/2) installing ripgrep-bash-completion      [------------------------------] 100%
```

Pick one per command with `--style pacman`, per shell with `PZYP_STYLE=pacman`,
or permanently with:

```
pzyp config set style pacman     # writes ~/.config/pzyp/config.toml
pzyp config                      # show current settings
pzyp config unset style          # back to the default
```

Like pacman's `ILoveCandy`, pacman-style bars chomp: `[--------C o  o  o ]`.
Prefer plain `[#####-----]` bars? `pzyp config set candy false`.

The config file also takes `color = "auto" | "always" | "never"`.
Precedence: `--style` > `PZYP_STYLE` > config file > default.

## How it works

pzyp runs zypper with `--xmlout --non-interactive` and renders its event
stream itself. zypper refuses to read prompt answers from anything but a real
terminal, so a transaction runs in two passes:

1. `zypper <cmd> --dry-run` resolves dependencies; pzyp shows the plan and asks
   for confirmation.
2. `zypper --no-refresh <cmd>` does the work; download/install progress is
   turned into a single progress bar.

If the solver needs a decision, pzyp shows the problem and suggests the
equivalent `sudo zypper ...` command to resolve it interactively. Package
licenses can be accepted with `--agree-licenses`. When not run as root, pzyp
uses `sudo`, asking for the password up front.

Set `PZYP_LOG=/path/file.xml` to save zypper's raw XML for debugging.

## License

Copyright (c) chip

This project is licensed under the MIT license ([LICENSE](./LICENSE) or <http://opensource.org/licenses/MIT>)
