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
(watcher-triggered), `CommandPalette`. The view stacks `view_modal(prompt)`
on top of the panes via iced's `stack![..]`.

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
additions while a pane is showing won't update it.

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
modal-on-top stacking, same filter `text_input`, same one-shot
`PromptSubmit` no-op.

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
of the list — wiring up `tasklist` and `taskkill` is left for later. The
modal-open suppression handles arrow keys the same way as the Docker
modal, and `PromptSubmit` for `Prompt::Processes` is the same no-op.
