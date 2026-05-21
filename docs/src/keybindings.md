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
| `⌘⇧P` | Command palette — text input over a filterable list of actions (Copy / Delete / Docker containers / Exit). Same controls as `⌘P`. |
| `⌘,` | Open `~/.fm.yaml` in the OS default editor (creates the file if missing) |
| `Esc` | Cancel the current modal, or clear the filter if no modal is open |

Inside a modal, the navigation keys behave differently:

| Modal | Keys |
|---|---|
| Open / Command palette (text input + list) | Type to filter; `↑` / `↓` / `Tab` move the highlight, `PageUp` / `PageDown` jump 5 rows, `Enter` activates the highlight (Open also accepts a typed path), `Esc` cancels. |
| Copy (text input only) | `Enter` submit, `Esc` cancel |
| Delete confirm | `Tab` / `←` / `→` toggle Cancel ↔ Delete focus, `Enter` activates focused button, `Esc` cancel |
| New-files prompt | `Tab` cycles No / Left / Right, `Enter` activates, `Esc` dismisses |
| Docker containers | Mouse-only in v1 — click `Kill` or `Shell` per row; `Esc` dismisses. The list refreshes automatically after a kill. |

### Docker containers modal

Picking **Docker containers** from the command palette runs `docker ps`
and shows each running container as a row with name, image, status, and
two buttons:

- **Kill** — runs `docker kill <id>`. The list re-fetches on completion so
  the killed container disappears (or sticks around with a refreshed
  status if the kill failed).
- **Shell** — opens a new terminal window running `docker exec -it <id>
  /bin/sh`. `/bin/sh` is used rather than `bash` because Alpine-based
  images often don't ship bash. The terminal lives independently of `fm`
  — closing it doesn't affect the container.

Platform notes for the **Shell** button:

- **macOS** — uses `osascript` to open Terminal.app.
- **Linux** — uses `x-terminal-emulator`. If you're on a distro that
  doesn't ship this alternative you'll need to install it or symlink to
  your preferred emulator.
- **Windows** — opens `cmd /K`.

If Docker isn't installed (no `docker` binary on PATH), the modal shows
an explanatory message instead of an empty list.
