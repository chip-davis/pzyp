# pzyp

A prettier front-end for `zypper`, styled after [uv](https://github.com/astral-sh/uv).

```
$ pzyp add ripgrep
Resolved 2 packages in 1.03s
 + ripgrep                  15.2.0-1.3
 + ripgrep-bash-completion  15.2.0-1.3

  2 to install · 1.7 MiB to download · +5.4 MiB disk
? Proceed? [Y/n]
Downloaded 2 packages in 0.82s
Installed 2 packages in 0.41s
✓ 2 installed in 1.31s
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

## Install

```
cargo install --path .
```
