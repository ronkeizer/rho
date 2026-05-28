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
highlight — `view_pane` colors a file's name with `action_color` when
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
