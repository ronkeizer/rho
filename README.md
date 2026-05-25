# Rho

Cross-platform dual-pane file manager in the style of
[fman](https://fman.io/) and
[Total Commander](https://www.ghisler.com/), written in Rust on
[iced](https://iced.rs/).

Two panes side by side, each showing one directory. The active pane reads
at full contrast; the inactive one is dimmed. The keyboard is the primary
input device — nearly every action has a key.

## Highlights

- **Dual-pane browsing** — `Tab` to swap focus, type-to-filter (regex with
  a substring fallback), sortable Name / Size / Modified columns,
  streaming loads for large directories.
- **File operations** — Copy (`F5`), Move (`F6`), Delete (`Delete`),
  Compress / Uncompress, and Enter-on-`.zip` to unpack into `/tmp` and
  browse the result.
- **Git awareness** — per-file dirty marker, info bar with branch /
  uncommitted count / ahead-behind, and a `Git: branch` palette action
  for in-app checkout.
- **Claude Code awareness** — orange info bar when `CLAUDE.md` or
  `.claude/` is present, plus an `Open Claude Code in this folder`
  palette action that spawns a terminal running `claude`.
- **Command palette (`⌘⇧P`)** — Docker containers (per-row Kill + Shell),
  Processes (per-row Kill, sortable by CPU/MEM), Launch Application
  (macOS), Connect to SSH server (reads `~/.ssh/config`), Keyboard
  shortcuts, Exit.
- **Hot-reloading YAML config** (`~/.rho.yaml`) and session restore
  (`~/.rho-state.yaml`).

See the
[mdBook](https://ronkeizer.github.io/rho/) for the full feature list,
keybindings, and architecture notes.

## Build & run

```sh
cargo run              # launch the app
cargo check            # fast type-check during edits
cargo build --release  # optimized binary at target/release/rho
cargo test             # run the unit tests
```

On Linux you'll also need iced's graphics deps. Debian/Ubuntu:

```sh
sudo apt-get install libxkbcommon-dev libwayland-dev
```

macOS and Windows have no extra prerequisites beyond a stable Rust
toolchain.

On first run, Rho creates `~/.rho.yaml` with a starter template and
opens with both panes pointed at `$HOME`.

### macOS app bundle

`cargo run` shows the generic binary icon in the Dock. To get a proper
`Rho.app` with its own Dock icon:

```sh
./scripts/make-macos-app.sh   # builds the release binary + target/macos/Rho.app
open target/macos/Rho.app
```

The icon artwork is generated from scratch (no design tool needed) by
`scripts/gen-icon.py`, which writes `assets/icon.png`; that same PNG is
embedded as the window icon on every platform. Re-run it after editing the
script to regenerate the art.

## Docs

- [Build & run](https://ronkeizer.github.io/rho/build-and-run.html)
- [Keybindings](https://ronkeizer.github.io/rho/keybindings.html)
- [Configuration](https://ronkeizer.github.io/rho/configuration.html)
- [Session state](https://ronkeizer.github.io/rho/session-state.html)
- [Architecture](https://ronkeizer.github.io/rho/architecture.html)

## Contributing

Every behavior change ships with **tests** (in-module unit tests under
`src/`) and **docs** (`docs/src/`). CI enforces both — see
[CLAUDE.md](./CLAUDE.md) for the conventions.
