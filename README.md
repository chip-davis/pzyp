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

## Choosing a style

Three ways to pick a style, from most to least specific:

```sh
pzyp --style pacman add ripgrep   # just this command
export PZYP_STYLE=dnf             # this shell session
pzyp config set style apt         # permanently (saved to the config file)
```

When more than one is set, `--style` wins over `PZYP_STYLE`, which wins over
the config file. With none of them, pzyp uses `uv`.

## Configuration

Settings live in `~/.config/pzyp/config.toml` (or
`$XDG_CONFIG_HOME/pzyp/config.toml`). Under `sudo pzyp`, the invoking user's
file is used, not root's. Manage it with `pzyp config`:

```sh
pzyp config                    # show every setting, its value and where it came from
pzyp config get style          # print one setting (including --style / PZYP_STYLE overrides)
pzyp config set style pacman   # change a setting
pzyp config unset style        # back to the default
pzyp config path               # print the config file's path
```

Or edit the file by hand:

```toml
style = "pacman"   # uv (default), pacman, dnf, apt, brew or npm
color = "auto"     # auto (default), always or never
candy = true       # pacman style: chomping progress bars (default true)
```

| Setting | Values | Default | What it does |
|---------|--------|---------|--------------|
| `style` | `uv`, `pacman`, `dnf`, `apt`, `brew`, `npm` | `uv` | Which package manager pzyp looks like |
| `color` | `auto`, `always`, `never` | `auto` | `auto` colors output only when writing to a terminal |
| `candy` | `true`, `false` | `true` | Pacman style only: chomping `[--------C o  o  o ]` bars like pacman's `ILoveCandy`; `false` gives plain `[#####-----]` bars |

If the file is malformed, pzyp prints a warning and uses the defaults instead
of failing.

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
