# Configuration

Rho reads its visual settings from `~/.rho.yaml`. The file is created with
a starter template the first time you launch the app, and is re-read live
on save (a 1 Hz mtime check picks up changes without a restart). If the
file is missing or fails to parse, defaults are used and a warning is
logged to stderr.

Hot-reload covers everything **except** window size, which iced sets once
at startup — change `window_width` / `window_rows` and you need to restart
to see it.

`⌘,` from inside the app opens this file in your OS default editor
(creating it from the template if it doesn't exist).

## Fields

| Key | Type | Default | Notes |
|---|---|---|---|
| `row_height_px` | float | `19.0` | Single source of truth for vertical row stride. Auto-scroll math reads this directly. |
| `row_font_size` | int | `13` | Used for entry rows. |
| `header_font_size` | int | `11` | Used for column headers, the git info bar, the filter bar, and the bottom status bar. |
| `size_column_px` | float | `80.0` | Width of the Size column. |
| `modified_column_px` | float | `140.0` | Width of the Modified column. |
| `window_width` | float | `1100.0` | Initial window width. Restart required to apply. |
| `window_rows` | int | `35` | Initial window height, expressed in row counts. Restart required to apply. |
| `mono_glyph_px` | float | `7.5` | Approximate width of a monospace glyph. Used to estimate how many characters fit in the Name column before ellipsizing. |
| `stripe_color` | string `#rrggbb` | _(theme-derived)_ | Zebra stripe color for odd rows. Omit to fall back to a derived theme blend. |
| `cursor_color` | string `#rrggbb` | _(theme-derived)_ | Selection cursor color. Omit to use the iced theme's primary-strong color. |
| `mark_color` | string `#rrggbb` | _(theme-derived)_ | Background for marked rows (range selection). Omit to use the theme's primary-weak color. |
| `folder_color` | string `#rrggbb` | `#6db4ff` | Name color for directory entries. |
| `watch_folders` | list of strings | `["~/Downloads"]` | Folders to watch for new files. Read once at startup; restart to apply changes. |
| `terminal_app` | string | _(auto)_ | macOS only. Which terminal app to launch for the SSH `Connect` and Docker `Shell` actions. Common values: `"iTerm"`, `"Terminal"`. Omit (or leave `None`) to auto-pick: `iTerm` when `/Applications/iTerm.app` exists, otherwise `Terminal`. Ignored on Linux / Windows. |

## Colors

Color fields are `#rrggbb` (hash optional). Invalid values are silently
ignored and the theme-derived default is used instead. Parsing is
case-insensitive and whitespace-tolerant.

## Terminal app (macOS)

When you set `terminal_app` (or rely on the auto-pick), the SSH and
Docker-shell actions emit AppleScript tailored to that app:

- **iTerm / iTerm2** → `create window with default profile command "..."`.
  The command becomes the session's main process — no shell wrapper, no
  prompt visible before the command runs.
- **Terminal** (or any other name) → `do script "exec ..."`. `do script`
  always wraps in a shell, so you'll briefly see the shell prompt, but
  `exec` immediately replaces it with your command.

That's also why the auto-pick prefers iTerm if it's installed — the
iTerm path has a noticeably smoother launch.

## Watch folders

Each entry in `watch_folders` is watched non-recursively (so files appearing
in subdirectories are ignored). When new files land in a watched folder, the
app shows a modal asking whether to switch one of the panes to that folder.

- `~/` is expanded against `$HOME`.
- Paths that don't exist at startup are skipped with a warning to stderr.
- In-progress download files (`.crdownload`, `.part`, `.download`, `.tmp`)
  and hidden files (anything starting with `.`) are filtered out so the
  modal doesn't fire on browser temps.
- A 500ms quiet window coalesces bursts (e.g. extracting an archive) into a
  single modal.

## Example

```yaml
# Rho configuration file — edits are picked up live (no restart needed).
row_height_px: 19.0
row_font_size: 13
header_font_size: 11
size_column_px: 80.0
modified_column_px: 140.0
window_width: 1100.0
window_rows: 35
mono_glyph_px: 7.5
# Optional color overrides (#rrggbb). Comment out to derive from theme.
folder_color: "#6db4ff"
# stripe_color: "#1c1d1f"
# cursor_color: "#3a80c8"
# mark_color: "#2a4a6a"
# Folders to watch for new files.
watch_folders:
  - "~/Downloads"
```
