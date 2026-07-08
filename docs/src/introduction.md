# Introduction

Rho is a cross-platform dual-pane file manager in the style of
[fman](https://fman.io/) and [Total Commander](https://www.ghisler.com/).
It is written in Rust on top of the [iced](https://iced.rs/) GUI toolkit.

Two panes sit side by side, each showing one directory. The active pane
reads at full contrast; the inactive one is dimmed so "where am I?"
stays unambiguous. Nearly every action — open, copy, delete, sort,
filter — runs against the active pane. The keyboard is the primary
input device.

## Features

**Browsing & navigation**

- Dual-pane layout with `Tab` to swap focus.
- Type-to-filter the active pane — regex, with a substring fallback
  while you're still typing partial syntax.
- Sortable columns (Name / Size / Modified); directories always
  cluster before files.
- Range selection with `Shift +` arrows / page keys / click.
- Streaming directory loads — large folders render the first chunks
  while the rest stream in, so the UI stays responsive.

**Git awareness (per pane)**

- Per-file `●` marker when an entry is dirty in `git status`.
- Info bar showing branch, uncommitted count, and ahead/behind vs.
  upstream.
- `Git: branch` palette action — pick a branch and check it out
  without leaving the app.

**Claude Code awareness**

- Orange info bar when the pane's directory contains `CLAUDE.md` or
  a `.claude/` subdirectory.
- `Open Claude Code in this folder` palette action — spawns a new
  terminal in the active pane's directory with `claude` running.

**File operations**

- Copy (`F5`), move (`F6`), and delete (`Delete`) with a confirmation
  modal for delete. Move uses `fs::rename` and falls back to copy +
  delete when the source and destination are on different filesystems.
  Move and Copy also rename: with one item selected, type a new
  not-yet-existing destination name and it's applied as the exact target
  path (Move renames it, Copy duplicates it under the new name).
- Compress marked files / folders into a `.zip` (palette → Compress);
  extract `.zip` / `.tar.gz` archives (palette → Uncompress). Both
  default the destination to the other pane.
- `Enter` on a file opens a chooser: "Open with default application"
  plus any user-defined [`file_actions`](./configuration.md) whose glob
  matches (e.g. `*.md` → run pandoc). Matching files are highlighted in
  the listing. Actions run in the background (panes refresh on completion)
  or in a terminal window.
- "Go to folder" prompt (`⌘P`) with a recent-locations cache —
  filter-as-you-type and arrow-key picker; typing a fresh path also
  just works.
- New-files watcher (configurable folders, default `~/Downloads`)
  pops a modal offering to switch one of the panes when something
  lands.

**Command palette (`⌘⇧P`)**

A filterable list of one-shot actions:

- Copy / Delete (same as the keyboard shortcuts).
- Docker containers — modal with each running container, per-row
  Kill and Shell buttons, sortable columns, filter.
- Processes — `ps -axo …` output, per-row Kill (SIGTERM), sortable
  by Name / PID / CPU / MEM, filter.
- Launch Application (macOS) — lists `.app` bundles under
  `/Applications`, `/Applications/Utilities`, and `~/Applications`;
  Enter or click to `open` the bundle.
- Git: branch (visible only inside a git repo).
- Connect to SSH server — reads `~/.ssh/config`, picks a Host, opens
  a new terminal running `ssh <alias>`.
- Open Claude Code in this folder.
- Open Terminal in this folder — opens a shell already `cd`'d into the
  active pane's directory (uses the `terminal_app` setting).
- Open folder in editor — opens the active pane's directory in an
  external editor (uses the `folder_editor` setting, defaults to the VS
  Code CLI).
- Keyboard shortcuts — same content as the
  [Keybindings](./keybindings.md) chapter, in-app.
- Exit.

**Configuration & polish**

- Hot-reloading YAML config at `~/.rho.yaml` — fonts, colors, row
  geometry, watched folders, terminal-app preference.
- Session restore via `~/.rho-state.yaml` — both pane folders, the
  active side, and the most-recent-locations cache.
- macOS: pick which terminal app the SSH and Docker-shell actions
  use (iTerm by default if installed, otherwise Terminal.app).
- Quick Look preview (`Space`) and edit-in-`$EDITOR` (`F4`) on macOS.
- `F10` to quit.

The rest of this book covers what you need to use it and what you
need to hack on it:

- [Build & run](./build-and-run.md) — getting a binary on disk.
- [Keybindings](./keybindings.md) — every key the app responds to.
- [Configuration](./configuration.md) — `~/.rho.yaml` reference.
- [Session state](./session-state.md) — `~/.rho-state.yaml` (the
  pane-restore file).
- [Architecture](./architecture.md) — module layout and the
  load-bearing invariants you should know before changing the code.
