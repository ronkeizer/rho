# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`fm` is a cross-platform dual-pane file manager (fman/Total Commander style) written in Rust on top of `iced 0.13`. A single binary crate split into four modules:

- `src/config.rs` — `~/.fm.yaml` settings (with hot reload), `~/.fm-state.yaml` session restore, color parsing, editor/Quick Look launchers, `home_dir`/`expand_tilde`.
- `src/domain.rs` — pure types with no iced widget code: `Side`, `SortBy`/`SortDir`, `Entry`, `Pane` (selection/filter/sort), `Prompt` + focus enums, `PaletteAction`, `GitInfo`, `sort_entries`. Unit-tested in isolation.
- `src/fs_ops.rs` — directory streaming (`load_dir_task`), copy/delete (`copy_task`, `delete_task`, `copy_recursive`, `delete_path`), git probe (`git_info_task`, `gather_git_info`), file-watch subscription. Everything that returns `Task<Message>` / `Subscription<Message>` lives here.
- `src/main.rs` — `App` state, the central `Message` enum, the `update`/`view`/`subscription` glue, and the view helpers (`view_pane`, `view_modal`, `build_row`, `compute_row_style`, layout math, formatters). Sections inside still use `// -----` headers.

## Commands

- `cargo run` — launch the app
- `cargo check` — preferred during edits; full builds take ~15s on cold cache because of iced's dep tree, type-check is fast
- `cargo build --release` — release binary
- `cargo test` — 94 unit tests across `config`, `domain`, `fs_ops`, plus view-helper tests in `main`

No lint config or CI yet.

## iced feature flag (load-bearing)

`Cargo.toml` pins `iced = { version = "0.13", features = ["tokio"] }`. The `tokio` feature switches iced's executor from `smol` to a tokio runtime, which is required because directory streaming uses `tokio::task::spawn_blocking` + `tokio::sync::mpsc`. Removing the feature reintroduces a `there is no reactor running` panic at runtime.

## Architecture

### State shape

- `App` owns two `Pane`s, an `active: Side`, an `Option<Prompt>` (current modal, if any), and book-keeping for shift state, copy/delete in-flight indicators, settings file mtime, and window size.
- `Pane` has two parallel views of the listing:
  - `entries: Vec<Entry>` — the raw, sorted directory contents.
  - `visible_indices: Vec<usize>` — indices into `entries` that pass the current filter, in display order. **Selection (`selected`, `anchor`) and `RowClicked` row indices are always in *visible* space**, where row 0 is the synthetic `..` row and rows `1..=visible_indices.len()` are entries. Use `Pane::entry_at(row_index)` to resolve a visible row back to an `Entry`.
- `recompute_visible(preserve_name: Option<&str>)` is the choke point for rebuilding `visible_indices`. Call it after any mutation of `entries`, `filter`, `sort_by`, or `sort_dir`. It tries to keep the cursor on the entry it was previously on, looked up by name.

### Async dir loads stream with generation tags

`load_dir_task(side, path, generation)` spawns a `tokio::task::spawn_blocking` reader that pushes 64-entry batches into a `tokio::sync::mpsc` channel. The receiver is wrapped as a stream via `stream::unfold` and handed to `Task::stream(...)`, emitting `EntriesChunk` messages followed by a final `EntriesDone`.

`Pane::load_generation` is bumped on every `navigate()` and tagged into every chunk. The `EntriesChunk` / `EntriesDone` / `GitInfoLoaded` handlers all discard messages whose generation doesn't match the pane's current one — this is how mid-load navigation doesn't leave stale entries leaking into the new directory.

Whenever you add a new per-pane async source, tag it with the same generation and use `loading_tasks(side, path, generation)` (the helper that batches `load_dir_task` + `git_info_task`).

### Modal system

One `Option<Prompt>` field drives all modals. `Prompt` is an enum with variants for `Open { input }`, `Copy { input }`, `Delete { paths, focus }`. The view stacks `view_modal(prompt)` on top of the panes via `stack![base, modal]`.

Two non-obvious wiring points:

1. **Top-of-`update` message redirect.** Before the main match, if the active prompt is `Delete`, `ActivateSelection` (Enter) is rewritten to either `PromptSubmit` or `PromptCancel` based on `DeleteFocus`, and `SwitchSide` (Tab) is rewritten to `SwitchPromptFocus`. This is how the keyboard-only Cancel/Confirm UX works for the no-text-input Delete modal.
2. **Modal-open suppression guard.** Right after the redirect there's a list of nav messages that short-circuit while a prompt is open (arrows, F-keys, Backspace-as-GoUp, etc.). Anything else falls through. `PromptCancel` is *not* in the list — it's the escape hatch and is itself context-aware (clears filter when no modal is open).

When adding a new modal kind: extend the `Prompt` enum, add the open-message handler, add a branch to `view_modal`, and decide whether new nav keys need to be added to the suppression list.

### Subscription is fn-pointer only

`keyboard::on_key_press` and `on_key_release` in iced 0.13 take `fn` pointers, not closures — they can't capture `&self`. The subscription emits context-free messages and the `update` handler decides what they mean based on state. That's why, for example, `ArrowLeft`/`ArrowRight` always emit `SwitchPromptFocus` and `Backspace` always emits `GoUpActive`; the handlers then check whether a Delete modal is open / whether a filter is active and dispatch accordingly.

`on_key_press` only fires for events with `event::Status::Ignored`. When `text_input` (in the Open/Copy modal) has focus and captures Enter/Backspace/characters, the subscription stays silent — that's why Delete-modal-Enter handling needs the explicit redirect.

### Settings (`~/.fm.yaml`) and state (`~/.fm-state.yaml`)

Two separate YAML files in `$HOME`:

- **`.fm.yaml`** (user-editable settings): colors, font sizes, row geometry, initial window dimensions. Created with a starter template on first run via `ensure_settings_file()`. Cmd+, opens it in the system default editor.
- **`.fm-state.yaml`** (session restore): `left`, `right`, `active`. Written eagerly on every path/active change. Read in `App::new`, with non-existent paths silently falling back to `$HOME`.

Settings hot-reload: a 1 Hz `iced::time::every` subscription emits `CheckSettings`, which compares the file's mtime to the last-seen value and calls `Config::load()` if it changed. Hot reload doesn't touch window size (iced sets that once at startup) — only render-time settings (font, colors, row height, column widths).

### ROW_STRIDE coupling

`config.row_height_px` is the single source of truth for vertical row geometry, and the auto-scroll math (`App::ensure_active_visible`, `App::page_size`) reads it directly. The actual rendered row height is composed from `config.row_padding_y()` (derived from `row_height_px - row_font_size * 1.3`) plus the text height — if you change row padding, button styling, or font-line-height defaults, also re-check the `row_height_px` configuration so scroll positions don't drift one row at a time on PageUp/Down.

### View color resolution

Two layers cooperate on per-row colors:

- `compute_row_style` returns a `button::Style` that sets the row's *default* text color (used by the Size and Modified cells). It also handles selection (Cursor/Marked) and zebra stripes. Hover is intentionally inert.
- The Name cell has its *own* `text::Style` closure that can override the row default with the folder color, and applies the `dim()` transform for hidden entries.

`folder_name_color` and the dim flag are both suppressed when the row is part of the active pane's selection so the highlight stays unambiguous. `RowColors` is `Copy` and pre-parsed from the config once per `view()` so the per-row style closures don't re-parse hex strings on each render.

### Async copy / delete

`copy_task` and `delete_task` are one-shot: `Task::perform` wraps a `spawn_blocking` that processes the whole list and returns a `Vec<(PathBuf, Result<(), String>)>` as a single message. Errors are logged per-path to stderr (no in-app error UI yet). On completion, both panes are force-reloaded via `App::reload_both_panes` which goes through `navigate()` and therefore bumps generations + clears filters.

The `copy_in_progress` / `delete_in_progress` fields on `App` exist solely so `status_text()` can show a "Copying / Deleting…" indicator in the bottom status bar; clear them in the respective `*Finished` handlers.

### Git info

`gather_git_info` shells out to `git` (no `git2` native dep). Three subprocess calls: `branch --show-current`, `status --porcelain`, and `rev-list --count --left-right HEAD...@{u}`. All three are wrapped in one `git_info_task` that's part of `loading_tasks`, so the info is fetched on every navigate and refreshed after copy/delete (because `reload_both_panes` routes through `navigate()`).

No external-change detection — the bar refreshes only on user-initiated reloads.
