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
| `dropbox_app_key` | string | _(unset)_ | Dropbox app key (client ID) from the [App Console](https://www.dropbox.com/developers/apps). Required to enable the Dropbox backend. |
| `dropbox_app_secret` | string | _(unset)_ | Dropbox app secret. Required only for "full" (non-PKCE) apps; PKCE apps can omit it. |
| `dropbox_refresh_token` | string | _(unset)_ | Long-lived OAuth2 refresh token, exchanged on demand for short-lived access tokens. Set this together with `dropbox_app_key` to unlock the **Open Dropbox** command. |

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

## Dropbox

The Dropbox backend lets a pane browse, copy, move, and delete under
`dropbox:/`. The **Open Dropbox** command palette entry is always listed,
but stays greyed out and non-activatable until credentials are present —
with none set, picking it does nothing.

Rho never sees your Dropbox password. Instead you create a Dropbox "app",
grant it a few scopes, and hand rho a long-lived **refresh token** that it
exchanges on demand for short-lived access tokens (cached in-process, so
you authorize once and it survives restarts). Access tokens never touch
disk. All API calls shell out to `curl`, which must be on `PATH` (it ships
with macOS and most Linux distros).

### One-time setup

**1. Create the app.** Go to <https://www.dropbox.com/developers/apps> →
**Create app** → "Scoped access" → "Full Dropbox" (or "App folder" to
sandbox it). Note the **App key** and, unless you made a PKCE app, the
**App secret** from the Settings tab.

**2. Grant scopes.** On the app's **Permissions** tab, enable:

- `files.metadata.read`
- `files.content.read`
- `files.content.write`

…and click **Submit**. Do this *before* the next step — see the gotcha
below.

**3. Authorize (get an auth code).** Open this in a browser, substituting
your app key:

```
https://www.dropbox.com/oauth2/authorize?client_id=YOUR_APP_KEY&token_access_type=offline&response_type=code
```

`token_access_type=offline` is what makes Dropbox hand back a *refresh*
token rather than only a short-lived access token. Approve, then copy the
one-time `code` it shows (it expires fast — do step 4 right away).

**4. Exchange the code for a refresh token.**

```sh
curl -s https://api.dropbox.com/oauth2/token \
    -d code=PASTE_CODE_HERE \
    -d grant_type=authorization_code \
    -d client_id=YOUR_APP_KEY \
    -d client_secret=YOUR_APP_SECRET
```

(PKCE apps have no secret — drop `client_secret` and add
`-d code_verifier=…` from the pair you generated for step 3.)

**5. Verify the scopes, then save.** In the JSON response, the `scope`
field **must** list the files scopes, not just `account_info.read`:

```json
"scope": "account_info.read files.content.read files.content.write files.metadata.read"
```

If it only says `account_info.read`, you authorized before submitting the
Permissions tab — redo steps 2–4. Otherwise copy the `refresh_token` value
into `~/.rho.yaml` alongside the key and secret, and restart rho.

### Gotchas

- **A refresh token is not an access token.** The App Console's "Generate
  access token" button gives a short-lived `sl.…` access token, *not* a
  refresh token. Pasting that yields `invalid_grant: refresh token is
  malformed`. Refresh tokens come only from the step-3/4 flow and have no
  `sl.` prefix.
- **Changing scopes requires re-authorizing.** A refresh token is frozen
  with whatever scopes existed when it was minted. Enabling more scopes on
  the Permissions tab does **not** upgrade an existing token — you'll keep
  getting `missing_scope` until you redo steps 3–4 and paste the new
  token.
- **Editing `~/.rho.yaml` doesn't drop the cached access token.** Rho
  caches the minted access token in-process for its ~4h lifetime. After
  swapping the refresh token, **restart rho** so it mints a fresh one.

> **Note**: single-shot upload is used for files, so files over Dropbox's
> 150 MB single-request limit aren't supported yet.

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
# Dropbox backend (optional). See the Dropbox section above.
# dropbox_app_key: "xxxxxxxxxxxxxxx"
# dropbox_app_secret: "xxxxxxxxxxxxxxx"
# dropbox_refresh_token: "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
```
