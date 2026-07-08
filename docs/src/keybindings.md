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
| `⌘↑` / `⌘↓` | Jump to the top / bottom of the list |
| `PageUp` / `PageDown` | Move by one page (computed from current viewport) |
| `Shift +` arrow / page key | Extend the selection from the anchor |
| `Tab` | Switch the active pane (left ↔ right) |
| `←` / `→` | Switch the active pane (left ↔ right) — same as `Tab`. Inside a modal these flip the modal's own focus instead (see the modal table below). |
| `Enter` | If cursor is on `..`, go up. On a directory, descend. On a `.zip`, extract to `/tmp` and browse (large archives prompt first — see below). On any other file, open the **file-action chooser** (see the modal table below) — always "Open with default application", plus any matching [`file_actions`](./configuration.md). Files with a matching action are highlighted in the listing. |
| `Backspace` | Go to the parent directory (or, if a filter is active, delete one character from it) |

## Marking & range selection

A "mark" is a multi-row selection: anchor + cursor define a contiguous
range. The current cursor row is always part of the range, so a single-row
selection just means "this row".

| Key | Action |
|---|---|
| `Shift + ↑/↓` | Extend the range one row |
| `⌘⇧↑ / ⌘⇧↓` | Extend the range to the top / bottom of the list |
| `Shift + PageUp/PageDown` | Extend the range by a page |
| Click | Drop the anchor on the clicked row (single-row selection) |
| `Shift + Click` | Extend the range from the existing anchor |

## File actions

| Key | Action |
|---|---|
| `F4` | Edit the cursor row's file in `$VISUAL` / `$EDITOR` (falls back to `open -t` on macOS) |
| `Space` | Quick Look preview the cursor row's file (macOS only; no-op elsewhere) |
| `F5` | Open the copy modal (destination defaults to the other pane's directory; with a single item selected, type a new not-yet-existing name to copy it under that name) |
| `F6` | Open the move modal (destination defaults to the other pane's directory; with a single item selected, type a new not-yet-existing name to rename it) |
| `F7` | Open the new-folder modal — type a name to create a directory in the active pane (local panes only) |
| `F10` | Quit the app immediately. Works even with a modal open — no confirmation. |
| `F8` / `Delete` | Open the delete-confirm modal for the current mark |

Copy and delete operate on the **mark**, not just the cursor row. With no
range selected, that's just the row under the cursor.

## Drag & drop

Drag one or more files from another application (Finder, a browser, etc.) and
drop them onto Rho to copy them in. The target pane is the one under the
cursor at the moment of the drop — drop on the left half of the window to copy
into the left pane, the right half for the right pane. Remote panes can't be a
drop target yet (the status bar says so). Dragging files *out* of Rho into
another application isn't supported.

## Filtering

Plain characters (no `⌘` / `Ctrl` / `Alt`) feed a type-to-filter regex on the
active pane. The pane re-renders with only matching rows; the cursor stays
on the same entry if it's still visible, otherwise it jumps to the first
match rather than the `..` row.

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
| `⌘⇧P` | Command palette — text input over a filterable list of actions (Copy / Move / Delete / Compress / Uncompress / Docker containers / Processes / Launch Application (macOS) / Git: branch (when in a repo) / Connect to SSH server / Open Dropbox (greyed out until configured) / Open Claude Code in this folder / Open Terminal in this folder / Open folder in editor / FTP server / Open folder in Finder (macOS) / Keyboard shortcuts / Exit). Same controls as `⌘P`. |
| `⌘,` | Open `~/.rho.yaml` in the OS default editor (creates the file if missing) |
| `Esc` | Cancel the current modal, or clear the filter if no modal is open |

Inside a modal, the navigation keys behave differently:

| Modal | Keys |
|---|---|
| Open / Command palette (text input + list) | Type to filter; `↑` / `↓` / `Tab` move the highlight, `PageUp` / `PageDown` jump 5 rows, `Enter` activates the highlight (Open also accepts a typed path), `Esc` cancels. |
| Copy / Move (text input only) | `Enter` submit, `Esc` cancel. Move uses `fs::rename` and falls back to copy+delete when the source and destination are on different filesystems. A relative destination (e.g. `test/` for a subfolder) resolves against the active pane's current directory, not just an absolute path or the other pane's location. If the destination is an existing directory, sources move/copy *into* it; if exactly one item is selected and the destination doesn't exist yet (e.g. a bare `renamed` or `../renamed`), it's treated as the exact new path — this is how a folder gets renamed (Move) or copied under a new name (Copy). Copy-to-a-new-name only works within the same backend; across backends, copy into the destination directory instead. |
| New folder (text input only) | `Enter` creates the named directory in the active pane (nested names like `a/b` are created in full), `Esc` cancels. Refuses to clobber an existing path; remote panes aren't supported yet. |
| Open file (chooser) | Shown when `Enter` is pressed on a non-archive file. `↑` / `↓` move the highlight, `Enter` or a click runs it, `Esc` cancels. The first row is always "Open with default application"; the rest are [`file_actions`](./configuration.md) whose pattern matched. `Tab` expands the highlighted custom action into an editable command pre-filled with its placeholders already substituted — edit it, then `Enter` runs the edited text verbatim (not the original template); `Tab` again collapses back without running anything. Background actions show "Running…" in the status bar and refresh the panes when done; `terminal: true` actions open a terminal window. |
| Compress / Uncompress (text input only) | `Enter` submit, `Esc` cancel. Compress runs `zip -r` with the active pane as the working directory, so paths inside the archive are relative. Uncompress recognises `.zip` (→ `unzip -d`) and `.tar.gz` / `.tgz` (→ `tar -xzf -C`). |
| Delete confirm | `Tab` / `←` / `→` toggle Cancel ↔ Delete focus, `Enter` activates focused button, `Esc` cancel |
| Large-archive extract confirm | Shown when `Enter` is pressed on a `.zip` over 100 MiB. `Tab` / `←` / `→` toggle Cancel ↔ Extract focus, `Enter` activates focused button, `Esc` cancel. Default focus is Cancel. |
| FTP info | Shown right after starting the FTP server, or when the action is invoked while a server is already running on the active pane's folder. Lists address, root, permissions, (for `auth: generated`) the username + password, and a streaming log of incoming client activity — logins, `GET` / `PUT` / `DEL` / `MKD` / `RMD` / `RENAME`, failed auth attempts, and read-only rejections. `Tab` / `←` / `→` toggle Stop ↔ Close focus, `Enter` activates focused button, `Esc` closes the modal but leaves the server running. Default focus is Close. |
| FTP replace confirm | Shown when the FTP-server action fires while a server is already running on a **different** folder. Lists both roots. `Tab` / `←` / `→` toggle Cancel ↔ Stop-and-restart focus, `Enter` activates focused button, `Esc` cancel. Default focus is Cancel (tearing down a live server drops connections). |
| New-files prompt | `Tab` cycles No / Left / Right, `Enter` activates, `Esc` dismisses |
| Docker containers | Type to filter (substring against name + image). Click a column header (Name / Image / Status) to sort — clicking the active column flips direction. Click `Kill` or `Shell` per row; `Esc` dismisses. The list refreshes automatically after a kill. |
| Processes | Type to filter (substring against name). `↑`/`↓` move the highlighted row; `Enter` kills it (SIGTERM). Click a column header (Name / PID / CPU / MEM) to sort — clicking the active column flips direction. Defaults to CPU descending. Click `Kill` per row (also sends SIGTERM); `Esc` dismisses. The list refreshes automatically after a kill. |
| Launch Application (macOS) | Type to filter (substring against name). `↑`/`↓`/`Tab` move the highlight; `Enter` or clicking `Launch` opens the app via `open`. `Esc` dismisses. |
| Git: branch | Type to filter (substring against branch name). `↑`/`↓`/`Tab` move the highlight; `Enter` or clicking `Checkout` runs `git checkout`. On success the modal closes and both panes reload; on failure the error is shown in the modal. `Esc` dismisses. |
| Keyboard shortcuts | Read-only modal — scrollable list of all bindings grouped by section. `Esc` dismisses. Same content as this page, available in-app via `⌘⇧P → "Keyboard shortcuts"`. |
| Connect to SSH server | Type to filter (substring against alias or hostname). `↑`/`↓`/`Tab` move the highlight; `Enter` or clicking `Connect` opens a new terminal running `ssh <alias>`. `Open` lists the host's home directory in the active pane and lets you Copy / Move / Delete to and from it (via `sftp` and `ssh rm -rf`). `Esc` dismisses. |

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

### Enter on a `.zip`

Pressing `Enter` on a `.zip` row extracts it into a fresh
`/tmp/rho-<sanitized-stem>-<epoch-ms>/` directory (on Windows, the OS
temp dir is used instead of `/tmp`) and then navigates the active pane
into that directory so you can browse the unpacked contents as if it
were any other folder. The active pane's [recent locations](./session-state.md)
get updated the same as any other navigate.

If the archive is larger than **100 MiB**, a confirmation modal appears
first with Cancel and Extract buttons — default focus is Cancel so a
stray `Enter` won't kick off a long unpack. Tab / ←/→ flip focus,
`Enter` activates the focused button, `Esc` cancels.

Only `.zip` triggers this — other archive types (`.tar.gz`, `.tar.bz2`,
…) still open via the OS default. To unpack those, mark the archives and
use the **Uncompress** action from the command palette.

The extracted folder is left behind in `/tmp` and not cleaned up by Rho;
the OS handles temp-directory garbage collection.

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

`↑` / `↓` move a blue highlight over the rows (the same cursor styling
the panes use), and `Enter` kills the highlighted process — the
keyboard equivalent of clicking its **Kill** button.

- **Kill** — sends `SIGTERM` via `kill <pid>`. Re-fetches the list on
  completion. No confirmation is shown: be deliberate about which row
  you target (by click or `Enter`), especially with system processes.
  Users who want SIGKILL should run `kill -9` from a real terminal.

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

- **Open** — lists the remote host's home directory in the active
  pane. The pane's path header switches to `<alias>:<path>` and
  navigation (`Enter`, `Backspace`, arrow keys) works as normal, with
  each step running a fresh `ssh <alias> ls -la --time-style=full-iso`
  against the remote. The remote host needs GNU `ls` (any Linux
  distro) — BSD-style `ls` doesn't accept `--time-style=full-iso`.

  **What works against remote panes today**: directory listing,
  `F5`/`F6` (Copy/Move) in either direction between local and remote,
  cross-pane copy/move on the same remote host (short-circuits to
  `ssh <alias> cp -r` / `mv`), copy/move *between* two different remote
  hosts (slow: stages through local `/tmp`), and `Delete` on remote
  selections (via `ssh <alias> rm -rf`). What doesn't (yet): Compress /
  Uncompress, `F4` Edit, Space Quick Look, "Open Claude Code in this
  folder", "Open Terminal in this folder", "Open folder in editor",
  "Open folder in Finder", and the Git branch palette action — all of
  those still silently no-op when the active pane is remote, because
  they hand a file or cwd to a local subprocess.

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

### Open Dropbox

Picking **Open Dropbox** from the command palette points the active pane
at the Dropbox account root (`dropbox:/`). The entry is **always listed**
so the feature is discoverable, but it's only activatable when Dropbox
credentials are present in `~/.rho.yaml` (see
[Configuration → Dropbox](./configuration.md#dropbox)). Without them the
row is greyed out and non-clickable, with a hint to set credentials in
`~/.rho.yaml`.

Navigation (`Enter`, `Backspace`, arrow keys) works as normal, with each
step paging `files/list_folder` over the Dropbox API. The pane header
shows `dropbox:<path>`.

**What works against Dropbox panes today**: directory listing, `F5`/`F6`
(Copy/Move) in either direction between local and Dropbox, copy/move
within Dropbox (server-side `copy_v2` / `move_v2`), copy/move between
Dropbox and an SSH host (stages through local `/tmp`), and `Delete` on
Dropbox selections (`delete_v2`). The same local-only actions that no-op
for SSH panes (Compress / Uncompress, `F4` Edit, Space Quick Look, Open
Claude Code, Open folder in Finder, Git: branch) also no-op for Dropbox
panes.

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

### Open Terminal in this folder

Picking **Open Terminal in this folder** opens a new terminal window
with an interactive shell already `cd`'d into the active pane's
directory. Like Open Claude Code it's fire-and-forget (no modal), and it
honors the same `terminal_app` setting. The only difference is it stops
at the `cd` — no command is run, so you're left at a prompt. Per OS:

- **macOS** — `osascript` with the resolved `terminal_app`. Terminal.app
  types `cd '<path>'` into the shell; iTerm runs
  `sh -c "cd '<path>' && exec ${SHELL:-/bin/sh} -l"` so the window stays
  open (its `command` is the session's main process, so a bare `cd`
  would exit immediately).
- **Linux** — `x-terminal-emulator` spawned with `cwd` set to the pane's
  path; the default interactive shell inherits it.
- **Windows** — `cmd` spawned with `cwd` set similarly.

The action no-ops for Dropbox (non-local) panes, since there's no local
directory to open.

### Open folder in editor

Picking **Open folder in editor** opens the active pane's directory in an
external editor by spawning `<editor> <folder>`. Also fire-and-forget,
and like the terminal actions it no-ops for Dropbox (non-local) panes.

The editor binary comes from the `folder_editor`
[setting](./configuration.md), which defaults to `/usr/local/bin/code`
— the VS Code CLI installed via *Shell Command: Install 'code' command in
PATH*, which opens the folder as a workspace. Point it at any editor that
accepts a directory argument (e.g. `/opt/homebrew/bin/code`, a `subl`
path, or a wrapper script). If the binary doesn't exist the action fails
silently (the error is logged to stderr).

### FTP server

Picking **FTP server** from the command palette starts an in-process FTP
server rooted at the active pane's folder and pops the **FTP info** modal
with the connection details — address, root, permissions, and a username
+ password (for `auth: generated`, the default). The password is a fresh
12-character random string on every start unless you pin one in
`~/.rho.yaml`; the username defaults to `rho` and can be pinned the same
way. Defaults are LAN-accessible (`0.0.0.0:2121`) and read-write; see
[Configuration → FTP server](./configuration.md#ftp-server) for the knobs
(including how to switch to read-only).

Only one server runs at a time. Re-invoking the action with the server
already up on the *same* folder just re-shows the info modal. Invoking on
a *different* folder pops the **FTP replace confirm** modal — pick
*Cancel* to leave the running server alone, or *Stop and restart here* to
tear it down (dropping any live FTP connections) and start a fresh one
rooted at the new folder. Default focus is *Cancel*.

Dismissing the FTP info modal (Close button, Esc, click on the backdrop)
leaves the server running; the status bar shows `FTP server on
HOST:PORT → ROOT` until you explicitly stop it. Stopping happens from
the FTP info modal's *Stop server* button — quitting the app also tears
the server down via the runtime's `Drop` impl.

### Open folder in Finder

Picking **Open folder in Finder** spawns `open <folder>`, revealing the
active pane's directory in Finder. Fire-and-forget (no modal), macOS-only
— the action isn't listed in the palette on other platforms. Like the
terminal/editor actions it no-ops for non-local (SSH/Dropbox) panes.

The modal also shows a **streaming log** of client activity below the
connection details: a row per event, newest at the top. Coverage:

- `username: logged in` / `logged out` — connections accepted / closed
  (libunftp `PresenceEvent`).
- `username: GET path (size)` / `PUT` / `DEL` / `MKD` / `RMD` /
  `RENAME from → to` — successful file operations (libunftp
  `DataEvent`).
- `auth failed: bad password for "name"` / `unknown user "name"` —
  rejected authentication attempts (amber).
- `denied write attempt (server is read-only)` — STOR / DELE / MKD /
  RMD / RENAME blocked because `permissions: read-only` is set (amber).
- `listen exited: …` — the listen task crashed mid-flight (red).

The log buffer keeps the most recent 200 entries; older ones drop off
the bottom. Closing the modal pauses rendering but doesn't pause
collection — re-opening shows everything that arrived while it was
closed. Stopping the server clears the buffer.

The server is local-only: starting from a remote (SSH / Dropbox) pane
surfaces an error rather than trying to serve files over libunftp's local
filesystem backend. Read-write mode (the default) forwards `STOR` /
`DELE` / `MKD` / `RMD` / `RNFR` through to the underlying filesystem;
read-only rejects all of those with 550.

Can't connect from another device on the LAN? See
[Troubleshooting LAN connections](./configuration.md#troubleshooting-lan-connections)
— almost always a firewall blocking either the control port or the
passive data ports (defaults: `2121` and `50000-50050`).
