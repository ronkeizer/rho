# Architecture

A short tour for anyone reading the source. The repo is a single binary
crate. The full set of architectural details is in `CLAUDE.md` at the root
of the repo (kept terser there as in-tree notes); this page is the
external-facing summary.

## Modules

```
src/
  main.rs       # App state, Message enum, update/view/subscription, view helpers
  config.rs     # ~/.rho.yaml + ~/.rho-state.yaml, colors, editor/Quick Look launchers
  domain.rs     # Pane, Entry, sort/filter, Prompt enum — no iced widgets
  fs_ops.rs     # Directory streaming, copy/delete, git probe, file watcher
```

`domain.rs` is pure: no iced types, no I/O. It's the layer where the unit
tests live and where most of the selection / filter / sort logic is.
`fs_ops.rs` returns `Task<Message>` / `Subscription<Message>` so the App
can wire results into `update()`.

## iced feature flag

`Cargo.toml` pins `iced = { version = "0.13", features = ["tokio"] }`. The
`tokio` feature switches iced's executor from `smol` to a tokio runtime —
load-bearing, because the directory-streaming code uses
`tokio::task::spawn_blocking` + `tokio::sync::mpsc`. Removing the feature
reintroduces a `there is no reactor running` panic at runtime.

## Pane state

Each `Pane` holds two parallel views of its listing:

- `entries: Vec<Entry>` — the raw, sorted directory contents.
- `visible_indices: Vec<usize>` — indices into `entries` that pass the
  current filter, in display order.

Selection (`selected`, `anchor`) and `RowClicked` row indices are always in
**visible** space, where row 0 is the synthetic `..` row and rows
`1..=visible_indices.len()` are entries. Use `Pane::entry_at(row_index)` to
resolve a visible row back to an `Entry`.

`recompute_visible(preserve_name)` is the choke point for rebuilding
`visible_indices`. Call it after any mutation of `entries`, `filter`,
`sort_by`, or `sort_dir`. It tries to keep the cursor on the entry it was
previously on, looked up by name.

## Locations: local + remote

`Pane.location` is a `Location` enum, not a `PathBuf`:

- `Location::Local(PathBuf)` — a path on the local filesystem.
- `Location::Remote { backend: BackendId, path: PathBuf }` — a path on a
  registered backend. Two backends exist today, distinguished by
  `BackendId::kind()`:
  - **SSH** — any backend ID; it's an alias from `~/.ssh/config`.
  - **Dropbox** — the reserved ID `BackendId::DROPBOX` (`"dropbox"`), so a
    Dropbox pane serializes as `dropbox:/Photos/x.jpg`. A single account
    is supported, keyed off the credentials in `~/.rho.yaml`.

The two variants share helpers (`location.path()`, `location.parent()`,
`location.join(name)`) for purely-syntactic operations; the helpers
preserve the backend, so navigating up from `alice.dev:/var/log` lands
on `alice.dev:/var`. I/O code that *runs* against the location (listing,
git probe, file ops) must dispatch on the variant.

The dispatch point is `fs_ops::loading_tasks(side, location, generation)`:

- `Local(path)` → `load_dir_task` (the streaming reader) plus a
  `git_info_task`.
- `Remote { backend, path }`, SSH → `load_remote_dir_task`, which spawns
  `ssh <backend> ls -la --time-style=full-iso -- <quoted-path>` in a
  blocking thread, parses the stdout with `parse_ls_la` (in `domain.rs`,
  unit-tested), and emits a single `EntriesChunk` + `EntriesDone`. No
  git info for remote panes — it's not worth the round-trip in the MVP.
- `Remote { backend, path }`, Dropbox → `load_dropbox_dir_task`, which
  pages `files/list_folder` (+ `list_folder/continue`) via `curl`, parses
  each page with `parse_dropbox_list` (in `domain.rs`, unit-tested), and
  emits one `EntriesChunk` + `EntriesDone`. Same fail-soft posture.

**Scope of remote-pane support today**: listing + copy + move + delete.
`copy_task` / `move_task` / `delete_task` in `fs_ops` dispatch on the
source-and-destination `Location` combination:

`fs_ops::transport(&Location)` classifies each side into `Local`,
`Ssh(BackendId)`, or `Dropbox`, and `run_copy` / `run_move` match on the
`(source, destination)` pair:

- `Local → Local` → in-process `copy_recursive` / `move_path` /
  `delete_path` (unchanged from before remote support).
- `Local → Ssh` → one `sftp -b - <alias>` invocation per source
  with a `put -r <src> <dst>` script. `Ssh → Local` uses `get -r`.
- `Ssh → Ssh`, same `BackendId` → `ssh <alias> 'cp -r -- …'` (or
  `mv` for moves) so the operation never round-trips through the local
  machine. Different SSH backends stage through `/tmp` (see below).
- `Local ↔ Dropbox` → `files/upload` (recursive: `create_folder_v2`
  per directory, single-shot `upload` per file) and `files/download`
  (recursive: `get_metadata` to detect folders, then list + recurse).
- `Dropbox → Dropbox` → server-side `files/copy_v2` / `files/move_v2`,
  no local round-trip.
- **Mixed remote backends** (Dropbox ↔ SSH), and different SSH hosts →
  `stage_copy_each`: per source, fetch down to a per-call
  `/tmp/rho-stage-<pid>-<n>/` with its own transport, push up with the
  destination's, then remove the staging dir. Slow but zero-config.

Deletes route the same way: `delete_one` dispatches local → `delete_path`,
Dropbox → `files/delete_v2`, SSH → `ssh <alias> rm -rf`.

**Rename / copy-as vs. into-dir.** `move_task` / `copy_task` interpret
their destination as a *container directory* — each source is placed
*inside* it (the source's own filename is appended). The Move- and
Copy-modal submit handlers pick a second path when the destination doesn't
already exist and exactly one item is selected: `rename_task` /
`copy_as_task` in `fs_ops` move/copy that single source to the **exact**
destination path instead of into it. `rename_dest_ok` (in `main.rs`) gates
both — a local target must not exist and its parent must be a directory;
remote targets are trusted. `run_rename` / `run_copy_as` require source and
destination to share a backend: `Local → Local` uses `move_path` /
`copy_recursive`, same-host SSH uses `mv` / `cp -r -- src dst`,
same-account Dropbox uses `files/move_v2` / `files/copy_v2` to the exact
path; a cross-backend target errors (for copy, copy into the directory
instead). They reuse the `MoveFinished` / `CopyFinished` messages so the
in-flight indicator and error reporting are shared with move/copy.

Mutating ops still gated to local: `compress` / `uncompress` (they
shell out to local `zip`), `EditFile` and `QuickLook` (they hand the
file to a local editor / Preview app), `OpenClaudeCode` and the Git
palette action (both expect a local cwd). The Claude-marker probe also
short-circuits for remote panes. Remote-aware versions of those are
future phases.

When a copy / move / delete / compress / extract finishes, any per-
source failure is surfaced in the bottom status bar (red text) via
`App::last_error`, formatted by the pure `batch_error_message` helper
("Copy failed: …", or "Copy failed (N errors): …" when several
sources failed). The error persists until the next such action starts
(cleared at the top of `PromptSubmit` and the Enter-on-zip extract).
Full per-source detail is still logged to stderr alongside it.

**Entry points**:

- SSH: the "Connect to SSH server" palette modal (`⌘P` → SSH) grew a
  second per-row button, **Open**, that fires `Message::SshOpenInPane(alias)`
  and navigates the active pane to `Location::Remote { backend: alias,
  path: "~" }`. The remote shell expands `~` on the first `ls`, so there's
  no separate `pwd` round-trip to resolve home up-front.
- Dropbox: the **Open Dropbox** palette action (`⌘P` → Dropbox) is always
  listed (`available_palette_actions` no longer gates it), but
  `palette_action_enabled` returns `false` — greying the row and dropping
  its `on_press` — unless `Config::dropbox_auth()` is `Some` (credentials
  present in `~/.rho.yaml`). The flag is captured into
  `Prompt::CommandPalette { dropbox_configured }` at open time. When
  enabled it navigates the active pane to `dropbox:/` — the account root.

**Dropbox transport.** Like SSH, the Dropbox backend shells out — every
call spawns `curl` (no async HTTP client in the dep tree). Access tokens
are minted from the configured refresh token via `oauth2/token` and cached
in-process (`fs_ops::dropbox_access_token`) until ~1 minute before expiry.
JSON is parsed with `serde_json`; the pure parsers (`parse_dropbox_list`,
`parse_dropbox_token`, `dropbox_error_summary`, `dropbox_api_path`) live in
`domain.rs` and are unit-tested. Pane paths map to the API's path
convention via `dropbox_api_path` (the root `"/"` becomes `""`).

Path quoting splits on whether a remote shell is in the loop:

- **Shell-backed calls** (`ls`, `cp`, `mv`, `rm`) use
  `fs_ops::quote_remote_path`, which leaves a leading `~` segment
  unquoted and POSIX-single-quotes only the path tail, so the remote
  shell still expands the tilde.
- **sftp batch calls** (`get` / `put`) use `fs_ops::quote_sftp_path`.
  sftp speaks the SFTP protocol with no shell, so it never expands
  `~` — but its remote working directory already defaults to the login
  user's home. So `~` is rewritten to `.` and `~/rest` to a home-
  relative `rest` before quoting. Passing `~/rest` through literally
  (as the shell calls do) makes sftp resolve it as `<home>/~/rest` and
  fail with "not found" — the reason early remote↔local copies
  silently did nothing.

## Streaming loads with generation tags

Directory loads run via `load_dir_task` (in `fs_ops`), which spawns a
blocking reader that pushes 64-entry batches into a tokio mpsc channel.
The receiver is exposed to iced as `Task::stream(...)`, emitting
`EntriesChunk` messages followed by a final `EntriesDone`.

`Pane::load_generation` is bumped on every `navigate()` and tagged into
every chunk. The chunk / done / git-info handlers all discard messages
whose generation doesn't match the pane's current one — that's how
mid-load navigation doesn't leak stale entries into the new directory.

Sorting is **deferred** to `EntriesDone` rather than re-running on every
chunk; sorting per-chunk costs `O(n² log n)` in total work and was the
original cause of keyboard stalls on large directories.

## Modal system

One `Option<Prompt>` field drives all modals. `Prompt` is an enum:
`Open` (text input), `Copy` (text input), `Delete` (confirm), `NewFiles`
(watcher-triggered), `CommandPalette`, `FileActions` (the Enter-on-file
chooser; see below), and the list-backed action modals. The view stacks
`view_modal(prompt)` on top of the panes via iced's `stack![..]`.

Two non-obvious wiring points:

1. **Message redirect at the top of `update`.** Before the main match, if
   a non-text-input modal is open (Delete, NewFiles, CommandPalette),
   navigation messages like `ActivateSelection` and `SwitchSide` are
   rewritten to focus-aware variants (`PromptSubmit` / `PromptCancel` /
   `SwitchPromptFocus` / `PaletteSelect`). This is how the keyboard-only
   confirmation UX works.

2. **Modal-open suppression.** While **any** modal is open, pane
   navigation messages that didn't get rewritten are dropped at the top of
   `update`. `PromptCancel` is the escape hatch and is context-aware (also
   clears the filter when no modal is open).

## Subscriptions

`keyboard::on_key_press` and `on_key_release` in iced 0.13 take `fn`
pointers, not closures — they can't capture `&self`. The subscription emits
context-free messages and the `update` handler decides what they mean
based on state. That's why, for example, `Backspace` always emits
`GoUpActive` and the handler then checks whether a filter is active.

`on_key_press` only fires for events with `event::Status::Ignored`. When a
`text_input` (Open/Copy modal) has focus and captures Enter/Backspace/
characters, the subscription stays silent — that's why Delete-modal-Enter
handling needs the explicit redirect described above.

## File drag-and-drop (drop into Rho)

A `dnd_listener` (`event::listen_with`) turns two raw iced events into
messages: `Mouse(CursorMoved)` → `CursorMoved(Point)`, which just stores the
latest cursor position on `App`, and `Window(FileDropped(path))` →
`FileDropped(path)`.

The routing problem is that iced 0.13's `FileDropped` carries **no cursor
position** and fires **once per file with no terminal marker**. So:

- **Which pane** the drop targets is derived from the last-tracked
  `cursor_pos` via `domain::drop_target_side(cursor_x, window_width)` — left
  half → left pane, midpoint-and-right → right pane. (This is why the cursor
  has to be tracked continuously.)
- **Batching** a multi-file drop is handled by a debounce: each `FileDropped`
  pushes onto `App::dropped_files`, bumps `drop_seq`, and schedules
  `fs_ops::drop_flush_task(drop_seq)` (a ~150 ms sleep → `FlushDrops(seq)`).
  Only the `FlushDrops` whose `seq` still equals the current `drop_seq`
  drains the buffer and fires one `copy_task` for the whole batch; earlier
  ones in the burst see a stale `seq` and no-op.

The destination pane must be local; a drop onto a remote pane sets
`last_error` instead. Dragging files *out* of Rho isn't supported — iced 0.13
exposes no drag-source API.

## Folder watching (two watchers)

Both live in `fs_ops.rs` on top of the `notify` crate, each a
`Subscription::run_with_id` wrapping a `recommended_watcher` whose events
are coalesced by a quiet-window debounce before reaching `update`:

- **`file_watch_subscription`** watches the configured `watch_folders`
  (`~/.rho.yaml`) for *new* files and emits `NewFilesDetected`, which pops
  the `NewFiles` modal. Its id is the constant `"file-watcher"` and its
  folder set is read once at startup.
- **`pane_watch_subscription`** watches the two panes' *currently open*
  local directories and emits `WatchedDirChanged`, which auto-refreshes any
  pane showing that folder via `App::refresh_local_dir` → `Pane::reload`.
  Because the watched set changes as the user navigates, its
  `run_with_id` id is keyed off the folder list (`("pane-dir-watch",
  folders)`) — so iced tears the watcher down and restarts it on the new
  directories whenever a pane moves. Remote panes aren't watched.

The pane watcher reacts to creates, removes, and renames (structural
changes to the listing; content/metadata edits are ignored as noise, as
are dotfiles and in-progress download temps via
`is_ignored_watch_filename`). Its debounce is a **quiet window** (300 ms,
pushed out on each event so a burst collapses) bounded by a **hard cap**
(1.5 s, so a sustained stream — e.g. 500 files being moved in — still
refreshes periodically instead of starving). A refresh re-reads the
directory, so it never generates events that would feed back into the
watcher.

`Pane::reload` differs from `navigate`: it keeps the filter, sort, and
scroll position and re-homes the cursor onto the same entry by name (via
`pending_focus` with `focus_jump = false`, so the viewport doesn't jump —
contrast the "jump to a freshly-detected file" path used by the new-file
modal, which sets `focus_jump = true`). It still bumps the load generation
and streams through the same tagged-chunk path, so a refresh mid-load
can't leak stale rows.

## Async file ops

`copy_task` and `delete_task` are one-shot: a `spawn_blocking` processes
the whole list and returns a `Vec<(PathBuf, Result<(), String>)>` as a
single message. Errors are logged per-path to stderr (no in-app error UI
yet). On completion, both panes are force-reloaded via
`App::reload_both_panes`, which goes through `navigate()` and therefore
bumps generations and clears filters.

## Git info

`gather_git_info` shells out to `git` (no `git2` native dep). Three
subprocess calls: `branch --show-current`, `status --porcelain`, and
`rev-list --count --left-right HEAD...@{u}`. All three are wrapped in one
`git_info_task` that's part of the loading_tasks batch, so info is
fetched on every navigate and refreshed after copy/delete (because
`reload_both_panes` routes through `navigate()`).

## Claude marker

An orange info bar appears under the file list whenever the pane's
directory contains a `CLAUDE.md` file or a `.claude/` subdirectory —
the "you're in a Claude-aware project" signal. The detection is two
cheap `is_file` / `is_dir` stats; results are cached on `Pane` as
`has_claude_md` / `has_claude_dir` (plus a `has_claude_marker()`
convenience). The App layer calls `refresh_claude_marker(&mut pane)`
from `App::new`, `App::navigate`, and `App::reload_both_panes`, so the
cache is rebuilt on the same lifecycle as `git_info` — external file
additions while a pane is showing won't update it. The probe is skipped
for **remote** panes (stat'ing the local filesystem at the remote path
would be meaningless, and the round-trip cost isn't worth the marker).

## Docker integration

The "Docker containers" palette action opens a modal driven by a
three-state machine — `DockerState::Loading | Loaded(Vec<DockerContainer>)
| Error(String)`. The state lives inside the `Prompt::Docker` variant so
it follows the same `Option<Prompt>` lifecycle as every other modal.

Three subprocess interactions, all in `fs_ops.rs`:

- `docker_ps_task` runs `docker ps --format '{{.ID}}|{{.Names}}|{{.Image}}|{{.Status}}'`
  inside `spawn_blocking` and returns a `Message::DockerListLoaded(Result<…>)`.
  The format string and `splitn(4, '|')` parser are kept in sync — the
  parser lives in `domain.rs` as `parse_docker_ps` so it's reachable from
  unit tests without invoking Docker.
- `docker_kill_task` runs `docker kill <id>` and returns
  `Message::DockerKillFinished(id, Result<…>)`. On completion the update
  handler re-issues `docker_ps_task` so the killed container disappears
  (or sticks around with a fresh status if the kill failed).
- `docker_shell` is **synchronous** — it just spawns a terminal and
  doesn't follow the child's lifetime. Per-OS:
  - macOS: `osascript` with `tell app "Terminal" to do script "..."`. The
    ID is run through `shell_quote` (a tiny `\` / `"` escaper) so a
    well-formed AppleScript string is always produced even if Docker
    ever loosens its ID/name character set.
  - Linux: `x-terminal-emulator -e docker exec -it <id> /bin/sh`.
  - Windows: `cmd /C start cmd /K "docker exec -it <id> /bin/sh"`.
  - `/bin/sh` is used rather than bash because Alpine-based images
    typically don't ship bash.

Error handling: every shell-out maps `ErrorKind::NotFound` (docker missing
from PATH) to a friendly "Docker doesn't appear to be installed" message
that the modal displays in place of the list, and non-zero exits surface
stderr verbatim. Late-arriving `DockerListLoaded` messages are dropped if
the user has already dismissed the modal — same guard pattern as the
per-pane generation tags described above.

The modal has a filter `text_input` at the top — substring match against
container name + image via [`filtered_containers`] in `domain.rs`. The
input is fed through the same `PromptChanged` handler that the Open /
Copy / CommandPalette prompts use; typing into it doesn't trigger any
list navigation because per-row actions stay mouse-only.

[`filtered_containers`]: https://github.com/ronkeizer/rho/blob/main/src/domain.rs

The modal isn't in the `Prompt::Open | Prompt::CommandPalette` arrow-key
redirect arm, so the suppression block at the top of `update` swallows
pane-nav keys while it's open. `PromptSubmit` for `Prompt::Docker` is a
no-op that reinstates the prompt — preventing Enter from closing it
accidentally.

## Processes integration

Picking "Processes" from the command palette opens `Prompt::Processes`,
a near-mirror of the Docker modal. Same three-state machine
(`ProcessesState::Loading | Loaded(Vec<Process>) | Error(String)`), same
modal-on-top stacking, same filter `text_input`. Unlike Docker it carries
a `selected` index: `↑`/`↓` move a blue current-row highlight (reusing the
panes' cursor styling via `cursor_fill`), and `PromptSubmit` (Enter) kills
the highlighted process rather than being a no-op.

Two subprocess interactions, both in `fs_ops.rs`:

- `ps_task` runs `ps -axo pid=,pcpu=,pmem=,comm=` inside
  `spawn_blocking`. The column order is load-bearing: `comm` comes
  **last** because Mac-style command names (`Google Chrome Helper
  (Renderer)`) can contain spaces. The parser in `domain.rs`
  (`parse_ps_output`) splits the first three tokens on whitespace and
  keeps everything after the third boundary as the name. Results are
  sorted by CPU descending before they're handed back to the App.
- `kill_process_task` runs `kill <pid>` (SIGTERM, deliberately polite).
  On completion the App re-issues `ps_task` so the killed process
  disappears.

Both functions are `#[cfg(unix)]`. On Windows the stubs return a
"not supported on this platform" string that the modal surfaces in place
of the list — wiring up `tasklist` and `taskkill` is left for later.
Arrow keys are redirected to `PromptMove` (like the other navigable
modals) so they move the highlight, and `selected` is clamped back into
range by `PromptChanged` (filtering) and `ProcessesListLoaded` (the
post-kill reload) so it never points past the end of the list.

## File actions

Pressing `Enter` on a non-archive file opens `Prompt::FileActions { path,
choices, selected }` instead of opening the OS default app directly. The
choice list is built by `domain::build_file_choices`: `FileChoice::OpenDefault`
is always first, followed by one `FileChoice::Custom` per configured
`file_actions` entry whose glob (`domain::glob_match`, `*`/`?`, case-
insensitive) matched the file name. The same `glob_match` drives the listing
highlight — `view_pane` colors a file's name with `theme.action` when
`file_has_custom_action` is true (suppressed inside the active selection, like
the folder color; both go through the renamed `entry_name_color` helper).

The modal has **no `text_input`**, so unlike Docker/Processes its `Enter`
arrives as `ActivateSelection`. A dedicated redirect arm rewrites it to
`PromptSubmit` (and arrows/Tab to `PromptMove`); a row click emits
`FileChoiceActivate(index)`. Both paths funnel through `App::run_file_choice`:

- `OpenDefault` → `open::that_detached` (errors to the status bar).
- `Custom` → `domain::substitute_command` expands `{file}`/`{stem}`/`{ext}`/
  `{path}`/`{dir}` against the path, then either `run_file_action_in_terminal`
  (when `terminal: true`, reusing the macOS AppleScript / `x-terminal-emulator`
  / `cmd` dispatch) or the background `run_file_action_task`. The latter runs
  the line through `sh -c` / `cmd /C` with the file's folder as the working
  directory and returns `FileActionFinished`, which clears the
  `file_action_in_progress` status indicator and reloads both panes so any
  produced file appears.

`.zip` files keep their existing extract-and-browse flow (with the large-
archive confirm) — they're handled before the chooser, so they don't go
through `FileActions`.

## Quick view

`quick_view` config entries drive a passive, always-on preview instead of a
modal: whenever the cursor sits on a matching file, the *opposite* pane's
bottom half shows `domain::matching_quick_view`'s pick (first-match-wins,
unlike `file_actions`' full list) — either the output of its `command` or, if
`command` is omitted, the file's raw contents.

**Trigger: piggybacking on `ensure_active_visible`, not a new message per
navigation site.** Every message that can move the cursor, change the filter,
or stream in new entries (`RowClicked`, `MoveSelection`, `PageMove`,
`MoveToEdge`, `SwitchSide`, `FilterAppend`, `EntriesChunk`, `navigate`, …)
already calls `App::ensure_active_visible` to keep the scroll position
correct. `App::refresh_quick_view` is called from there — one choke point,
same reasoning as the scroll-follow logic it sits next to — instead of
threading a quick-view refresh into each call site individually.

**Debounce + request-id staleness, mirroring the load-generation pattern.**
`refresh_quick_view` doesn't run the match immediately: it stashes the
matched `domain::QuickViewState` (path, label, command) with a freshly
bumped `App.quick_view_seq` as `request_id`, and returns a
`Task::perform(tokio::time::sleep(150ms), …)` that fires
`Message::QuickViewDebounceFire(id)`. This is the same shape as `Pane::
load_generation` tagging directory-load chunks — holding an arrow key re-
enters `refresh_quick_view` on every row, bumping `quick_view_seq` each time,
so only the debounce fire whose `id` still matches `self.quick_view.
request_id` proceeds to actually spawn `fs_ops::quick_view_task`; every
earlier one is stale and silently dropped. `QuickViewLoaded(id, side, ..)` on
completion re-checks the same `id` (plus `source_side`, in case the active
side changed mid-flight) before applying the result.

**Why `tokio::process` instead of `std::process` + `spawn_blocking`.**
`file_actions` commands (`fs_ops::run_file_action`) run via blocking
`std::process::Command::output()` inside `spawn_blocking` — fine there
because they're explicitly user-triggered once. Quick view fires
automatically on every settled cursor move, unattended, so a command that
hangs (e.g. reads stdin) can't be allowed to block a worker thread
indefinitely. `fs_ops::run_quick_view_command` instead uses
`tokio::process::Command` with `kill_on_drop(true)` and races
`child.wait_with_output()` against a `tokio::time::timeout` (10s); when the
timeout wins, the future carrying `Child` is dropped, which — thanks to
`kill_on_drop` — kills the process instead of leaking it. Output (command
stdout/stderr, or the raw file read for the no-`command` case) is capped at
200KB (`fs_ops::QUICK_VIEW_MAX_BYTES`) with a `(truncated)` marker, and
decoded with `String::from_utf8_lossy` so a binary file can't panic the
preview (it just renders as replacement characters).

**View wiring.** `App::view` computes, per side, whether `self.quick_view`
belongs to *that* side by checking `qv.source_side.other() == side`, passing
`Option<&QuickViewState>` into `view_pane`. When `Some`, `view_pane` splits
its `inner_col` via `column!` into `Length::FillPortion(3)` (the existing
listing) on top and `Length::FillPortion(7)` (`quick_view_panel` — label bar
+ scrollable monospace body) on the bottom, instead of the usual
single-container layout.

**Closing re-measures the pane it was rendered in.** `refresh_quick_view`
routes every "stop showing a preview" case (cursor left the file, cursor hit
`..`, active pane went remote, no `quick_view` entry matches) through
`App::close_quick_view`. The row-virtualization windowing described above
(`first_row`/`last_row` in `view_pane`) uses the *cached* `pane.
viewport_height`, which is only ever updated by a `Message::Scrolled` from
that pane's own `on_scroll`. While the preview is showing, the opposite
pane's listing is squeezed into a `FillPortion(3)` scrollable, and if a
scroll happens during that time, `viewport_height` gets cached at that
squeezed height. When the split then disappears and the container reverts
to full height, the stale small `viewport_height` keeps capping how many
rows `view_pane` builds, so the rest of the (now-larger) pane renders as
blank filler space until something re-measures it. `close_quick_view` clears
`viewport_height` on the affected pane (`qv.source_side.other()`) — so the
very next render falls back to the window-based estimate instead of the
stale value — and re-issues `scrollable::scroll_to` at the pane's current
scroll offset to force iced to measure the real (now full-height) viewport
and report it back via a fresh `Scrolled`. A no-op call (quick_view was
already closed) touches nothing, so idle cursor moves that never opened a
preview don't pay for this on every keypress.

## FTP server

The **FTP server** palette action runs a libunftp server in-process. State
lives on `App.ftp_server: Option<FtpRuntime>` — singleton by construction.
`FtpRuntime` holds the connection details (`FtpServerInfo`) plus a
`tokio::task::AbortHandle` for the listen task; its `Drop` impl aborts the
handle, so dropping the runtime (when the user clicks Stop, when the
Replace flow swaps it for a new one, or when `App` itself is dropped on
quit) cleanly tears the listener down.

`fs_ops::ftp_start_task` builds the server: it pre-binds the chosen port
with `std::net::TcpListener` and drops it immediately so "address already
in use" / permission errors surface synchronously, then constructs a
`libunftp::ServerBuilder` whose storage backend is `fs_ops::RhoFs` — a
thin wrapper around `unftp_sbe_fs::Filesystem` that returns
`ErrorKind::PermissionDenied` from the write methods (`put`/`del`/`mkd`/
`rename`/`rmd`) when `FtpPerms::ReadOnly` is set. Auth is one of two
pre-built `Authenticator`s: libunftp's `AnonymousAuthenticator`, or a
local `StaticAuth` with a constant-time username + password compare for
the generated-credentials path. The server is then `tokio::spawn`ed, and
its `JoinHandle::abort_handle()` is what the App holds onto. Errors from
the listen future after startup are logged to stderr; we don't try to
react to them.

The palette action's three-way dispatch — start fresh / show info / ask
to replace — is encoded in the pure helper `domain::decide_ftp_action`,
which the `update` handler calls when `PaletteAction::FtpServer` fires.
The two new prompt variants (`Prompt::FtpInfo` for "running here, what
now?" and `Prompt::FtpReplace` for "running somewhere else, replace?")
get the same focus-aware Enter / Tab / `←`/`→` redirect as the existing
no-text-input modals (`Delete`, `ConfirmLargeExtract`): Enter on the
focused button emits either `FtpServerStopRequest` /
`FtpServerReplaceConfirmed(path)` or falls back to `PromptCancel`. Stop is
synchronous (just abort the handle via `take()` + Drop), so there's no
round-trip `Message::FtpServerStopped` — the handler clears state in one
pass.

Hot reload is **not** wired up for the `ftp` config block. Restarting the
server with new settings would yank existing connections, so picking up
config changes requires an explicit Stop + invoke. The status bar shows
`FTP server on HOST:PORT → ROOT` whenever a server is running.

### FTP event log

Client activity surfaces in the FtpInfo modal via a process-global
`tokio::sync::broadcast` channel — `fs_ops::FTP_LOG_BUS`. Producers push
`FtpLogEntry { ts, level, message }` onto the bus from four spots:

- `FtpEventListener` implements libunftp's `DataListener` +
  `PresenceListener` for the happy-path events (logins, file ops).
  Registered via `notify_data` + `notify_presence` on the
  `ServerBuilder`.
- `StaticAuth::authenticate` pushes a Warn entry on bad-user / bad-pass
  rejections. (Successful logins are intentionally **not** logged here —
  libunftp already emits `PresenceEvent::LoggedIn` for both
  authenticators, so logging from auth would double-log.)
- `RhoFs::check_writable` pushes a Warn entry whenever the read-only
  gate rejects a write operation, so the user watching the modal sees
  *why* a client just got 550.
- `ftp_start` itself pushes an Info "listening on … → …" entry on
  successful bind, and an Error entry from the `tokio::spawn`ed listen
  closure if the future returns Err.

Consumers are managed by `fs_ops::ftp_log_subscription`, an iced
`Subscription::run_with_id("ftp-log-bus", …)` always included in
`App::subscription`. Its closure calls `FTP_LOG_BUS.subscribe()` once and
forwards every entry as `Message::FtpLogEvent(entry)`. A
`broadcast::error::RecvError::Lagged(n)` (subscriber fell behind a
512-entry burst) surfaces as a single Warn "log bus lagged — dropped N
entries" entry rather than panicking.

`FtpRuntime` (on `App.ftp_server`) holds a `VecDeque<FtpLogEntry>` capped
at 200 entries — `Message::FtpLogEvent` pushes onto the back and trims
from the front when full. Entries arriving with no server running are
dropped on the floor (e.g. in-flight events queued just before
`FtpServerStopRequest`).
