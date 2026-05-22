# Keybindings

The keyboard is the primary input device. Mouse clicks work for activating
panes, selecting rows, sorting columns, and dismissing modals, but every
action also has a key.

`⌘` is the Command modifier on macOS; on Linux and Windows that's the same
key the OS treats as the "logo" / Super key. The app reads it through iced's
`Modifiers::command()`.

## Navigation

| Key | Action |
|---|---|
| `↑` / `↓` | Move the cursor one row |
| `PageUp` / `PageDown` | Move by one page (computed from current viewport) |
| `Shift +` arrow / page key | Extend the selection from the anchor |
| `Tab` | Switch the active pane (left ↔ right) |
| `Enter` | If cursor is on `..`, go up. On a directory, descend. On a file, open via the OS default app. |
| `Backspace` | Go to the parent directory (or, if a filter is active, delete one character from it) |

## Marking & range selection

A "mark" is a multi-row selection: anchor + cursor define a contiguous
range. The current cursor row is always part of the range, so a single-row
selection just means "this row".

| Key | Action |
|---|---|
| `Shift + ↑/↓` | Extend the range one row |
| `Shift + PageUp/PageDown` | Extend the range by a page |
| Click | Drop the anchor on the clicked row (single-row selection) |
| `Shift + Click` | Extend the range from the existing anchor |

## File actions

| Key | Action |
|---|---|
| `F4` | Edit the cursor row's file in `$VISUAL` / `$EDITOR` (falls back to `open -t` on macOS) |
| `Space` | Quick Look preview the cursor row's file (macOS only; no-op elsewhere) |
| `F5` | Open the copy modal (destination defaults to the other pane's directory) |
| `F6` | Open the move modal (destination defaults to the other pane's directory) |
| `F10` | Quit the app immediately. Works even with a modal open — no confirmation. |
| `Delete` | Open the delete-confirm modal for the current mark |

Copy and delete operate on the **mark**, not just the cursor row. With no
range selected, that's just the row under the cursor.

## Filtering

Plain characters (no `⌘` / `Ctrl` / `Alt`) feed a type-to-filter regex on the
active pane. The pane re-renders with only matching rows; the cursor stays
on the same entry if it's still visible.

| Key | Action |
|---|---|
| any printable char | Append to the filter |
| `Backspace` (with filter active) | Remove the last character |
| `Esc` (with filter active, no modal) | Clear the filter |

Partial regex input that isn't yet a valid pattern (e.g. you typed `(`)
falls back to a case-insensitive substring search so you don't see an empty
list while typing.

## Sorting

| Key | Action |
|---|---|
| Click a column header (`Name` / `Size` / `Modified`) | Sort by that column; click again to reverse |

Directories always cluster before files regardless of sort column.

## Modals & global commands

| Key | Action |
|---|---|
| `⌘P` | "Go to folder" prompt — blank text input over a filterable list of [recent locations](./session-state.md). Type to filter, ↑/↓ to pick a recent, Enter to open. Typing a fresh path and pressing Enter opens it even if it isn't in recents (typed path wins when it's a real directory). |
| `⌘⇧P` | Command palette — text input over a filterable list of actions (Copy / Move / Delete / Compress / Uncompress / Docker containers / Processes / Launch Application (macOS) / Git: branch (when in a repo) / Connect to SSH server / Open Claude Code in this folder / Keyboard shortcuts / Exit). Same controls as `⌘P`. |
| `⌘,` | Open `~/.rho.yaml` in the OS default editor (creates the file if missing) |
| `Esc` | Cancel the current modal, or clear the filter if no modal is open |

Inside a modal, the navigation keys behave differently:

| Modal | Keys |
|---|---|
| Open / Command palette (text input + list) | Type to filter; `↑` / `↓` / `Tab` move the highlight, `PageUp` / `PageDown` jump 5 rows, `Enter` activates the highlight (Open also accepts a typed path), `Esc` cancels. |
| Copy / Move (text input only) | `Enter` submit, `Esc` cancel. Move uses `fs::rename` and falls back to copy+delete when the source and destination are on different filesystems. |
| Compress / Uncompress (text input only) | `Enter` submit, `Esc` cancel. Compress runs `zip -r` with the active pane as the working directory, so paths inside the archive are relative. Uncompress recognises `.zip` (→ `unzip -d`) and `.tar.gz` / `.tgz` (→ `tar -xzf -C`). |
| Delete confirm | `Tab` / `←` / `→` toggle Cancel ↔ Delete focus, `Enter` activates focused button, `Esc` cancel |
| New-files prompt | `Tab` cycles No / Left / Right, `Enter` activates, `Esc` dismisses |
| Docker containers | Type to filter (substring against name + image). Click a column header (Name / Image / Status) to sort — clicking the active column flips direction. Click `Kill` or `Shell` per row; `Esc` dismisses. The list refreshes automatically after a kill. |
| Processes | Type to filter (substring against name). Click a column header (Name / PID / CPU / MEM) to sort — clicking the active column flips direction. Defaults to CPU descending. Click `Kill` per row (sends SIGTERM); `Esc` dismisses. The list refreshes automatically after a kill. |
| Launch Application (macOS) | Type to filter (substring against name). `↑`/`↓`/`Tab` move the highlight; `Enter` or clicking `Launch` opens the app via `open`. `Esc` dismisses. |
| Git: branch | Type to filter (substring against branch name). `↑`/`↓`/`Tab` move the highlight; `Enter` or clicking `Checkout` runs `git checkout`. On success the modal closes and both panes reload; on failure the error is shown in the modal. `Esc` dismisses. |
| Keyboard shortcuts | Read-only modal — scrollable list of all bindings grouped by section. `Esc` dismisses. Same content as this page, available in-app via `⌘⇧P → "Keyboard shortcuts"`. |
| Connect to SSH server | Type to filter (substring against alias or hostname). `↑`/`↓`/`Tab` move the highlight; `Enter` or clicking `Connect` opens a new terminal running `ssh <alias>`. `Esc` dismisses. |

### Compress / Uncompress modals

**Compress** bundles the marked files and folders into a single `.zip`.
The destination text input is pre-filled with `<other-pane>/<first-mark-stem>.zip`
— for `report.tar.gz` you get `report.zip`, for the folder `MyApp` you
get `MyApp.zip`. The full `.tar.gz` / `.tar.bz2` compound extension is
stripped, not just the trailing `.gz` / `.bz2`. `zip -r` is invoked with
the active pane as its working directory so paths *inside* the archive
are relative (you get `report.pdf`, not `/Users/me/project/report.pdf`).

**Uncompress** extracts every marked archive into the destination
directory (defaults to the other pane). Per archive: `.zip` is dispatched
to `unzip -d`, `.tar.gz` / `.tgz` to `tar -xzf -C`. Unknown extensions
fail per-archive without blocking the others.

Both actions require their respective CLIs (`zip`, `unzip`, `tar`) to be
on `PATH`. macOS ships all three; most Linux distros do too. On Windows
`tar` is built-in but `zip` / `unzip` need to be installed separately
(or the actions will error out with "not found in PATH").

### Docker containers modal

Picking **Docker containers** from the command palette runs `docker ps`
and shows each running container as a row with name, image, status, and
two buttons. A filter input at the top narrows the list as you type —
substring match against name + image, case-insensitive. The Name /
Image / Status column headers are clickable: the first click sorts by
that column ascending; clicking the active column again flips direction.
Default sort is Name ascending.

- **Kill** — runs `docker kill <id>`. The list re-fetches on completion so
  the killed container disappears (or sticks around with a refreshed
  status if the kill failed).
- **Shell** — opens a new terminal window running `docker exec -it <id>
  /bin/sh`. `/bin/sh` is used rather than `bash` because Alpine-based
  images often don't ship bash. The terminal lives independently of Rho
  — closing it doesn't affect the container.

Platform notes for the **Shell** button:

- **macOS** — uses `osascript` to open Terminal.app.
- **Linux** — uses `x-terminal-emulator`. If you're on a distro that
  doesn't ship this alternative you'll need to install it or symlink to
  your preferred emulator.
- **Windows** — opens `cmd /K`.

If Docker isn't installed (no `docker` binary on PATH), the modal shows
an explanatory message instead of an empty list.

### Processes modal

Picking **Processes** from the command palette runs
`ps -axo pid=,pcpu=,pmem=,comm=` and shows each process as a row with
name, PID, CPU%, and MEM%. A filter input at the top narrows by process
name (substring, case-insensitive). Default sort is CPU descending so the
heaviest processes are at the top; click any column header to switch —
numeric columns (PID / CPU / MEM) start descending on first click, Name
starts ascending. Clicking the active column flips direction.

- **Kill** — sends `SIGTERM` via `kill <pid>`. Re-fetches the list on
  completion. No confirmation is shown: be deliberate about which row
  you click, especially with system processes. Users who want SIGKILL
  should run `kill -9` from a real terminal.

The CPU% / MEM% values are snapshots from the moment of the `ps` call —
they don't auto-refresh. Dismiss and reopen the modal to refresh.

The modal is Unix-only in v1 (macOS + Linux). On other platforms it
displays an explanatory message instead of a list — wiring up `tasklist`
+ `taskkill` on Windows is left for a future change.

### Launch Application modal (macOS only)

Picking **Launch Application** from the command palette scans
`/Applications`, `/Applications/Utilities`, and `~/Applications` for
`.app` bundles and lists them sorted by name. A filter input narrows by
substring (case-insensitive). `↑`/`↓`/`Tab` (and `PageUp`/`PageDown` for
5-row jumps) move the highlight; `Enter` or clicking the per-row
`Launch` button calls `open <bundle>` and dismisses the modal.

The action is hidden from the command palette on non-macOS platforms;
launching macOS `.app` bundles isn't meaningful elsewhere.

App icons are not rendered in v1 — supporting them needs `.icns` parsing
(adds a dependency) plus per-app I/O on modal open. Likely added later.

### Git: branch modal

Picking **Git: branch** from the command palette runs
`git for-each-ref --sort=-committerdate refs/heads/ ...` against the
active pane's directory and lists local branches with the date of their
last commit. Most-recently-committed branches appear first (best proxy
for "last used"). A filter input narrows by substring of the branch name
(case-insensitive).

The action is hidden from the command palette unless the active pane is
inside a git repository — `git_info` being populated is the signal,
which is the same probe that drives the per-pane git bar.

- **Checkout** — runs `git checkout <branch>` in the pane's directory.
  On success the modal closes and both panes reload (so file listings
  and the git info bar reflect the new HEAD). On failure (dirty working
  tree, branch already removed, etc.) the modal stays open with the
  error message in place of the list — re-open the modal to try
  another branch after fixing the issue.

The branch list is captured at modal open time and isn't refreshed
during the session. Dismiss and reopen to refresh.

### Connect to SSH server modal

Picking **Connect to SSH server** from the command palette reads
`~/.ssh/config` and lists every `Host` entry — alias, `user@hostname`
(or just hostname when no User is set), and IdentityFile. Sorted by
alias ascending. Filter input narrows by substring of either the
alias or the hostname (case-insensitive).

Wildcard-only blocks (`Host *`) are skipped — they're defaults that
apply to every host, not specific servers. `Include` directives are
not followed in v1: only the top-level `~/.ssh/config` is parsed.
When a `Host` line lists multiple patterns (e.g. `Host alpha *.foo`),
the first non-wildcard pattern becomes the entry's alias.

- **Connect** — opens a new terminal window with `ssh <alias>` as its
  main process. Per OS:
  - macOS: `osascript` + Terminal.app, with the command prefixed by
    `exec` so the shell that `do script` spawns immediately replaces
    itself with `ssh`. You'll briefly see the shell prompt — that
    flicker is unavoidable with `do script` — but no shell process
    lingers behind the ssh session.
  - Linux: `x-terminal-emulator -e ssh <alias>` — terminal exec's
    directly into ssh, no shell wrapper.
  - Windows: `cmd /K ssh <alias>`.

  All real configuration (HostName, User, Port, ProxyJump, etc.) is
  resolved by `ssh` itself from `~/.ssh/config`, so options the modal
  doesn't display still take effect.

If `~/.ssh/config` is missing or contains no specific Host entries,
the modal shows an explanatory message instead of an empty list.

### Open Claude Code in this folder

Picking **Open Claude Code in this folder** spawns a new terminal in
the active pane's directory and runs `claude`. There's no modal — the
action fires and the palette closes. Per OS:

- **macOS** — `osascript` honors the `terminal_app` setting (defaults
  to iTerm if installed, else Terminal). Terminal.app types
  `cd '<path>' && exec claude` into the shell; iTerm wraps that in
  `sh -c "..."` so `&&` is interpreted, since iTerm runs the
  `command` value as the session's argv rather than through a shell.
- **Linux** — `x-terminal-emulator -e claude`, spawned with `cwd`
  set to the pane's path, so the terminal (and `claude` inside it)
  inherits the working directory. No shell wrapper.
- **Windows** — `cmd /K claude`, spawned with `cwd` set similarly.

Requires the [Claude Code CLI](https://docs.claude.com/en/docs/claude-code/overview)
to be installed and on `PATH`. If `claude` isn't found you'll see the
usual "command not found" inside the new terminal window.
