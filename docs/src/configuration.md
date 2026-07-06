# Configuration

Rho reads its visual settings from `~/.rho.yaml`. The file is created with
a starter template the first time you launch the app, and is re-read live
on save (a 1 Hz mtime check picks up changes without a restart). If the
file is missing or fails to parse, defaults are used and a warning is
logged to stderr.

Hot-reload covers everything **except** the `window` section, which iced
sets once at startup — change `window.width` / `window.rows` and you need
to restart to see it.

`⌘,` from inside the app opens this file in your OS default editor
(creating it from the template if it doesn't exist).

Related settings are grouped into sections (`window`, `layout`, `theme`,
`dropbox`, `ftp`); everything else stays top-level. Every section is
optional — an absent section, or an absent field within one, falls back to
its default.

## Fields

| Key | Type | Default | Notes |
|---|---|---|---|
| `window.width` | float | `1100.0` | Initial window width. Restart required to apply. |
| `window.rows` | int | `35` | Initial window height, expressed in row counts. Restart required to apply. |
| `layout.row_height_px` | float | `19.0` | Single source of truth for vertical row stride. Auto-scroll math reads this directly. |
| `layout.row_font_size` | int | `13` | Used for entry rows. |
| `layout.header_font_size` | int | `11` | Used for column headers, the git info bar, the filter bar, and the bottom status bar. |
| `layout.size_column_px` | float | `80.0` | Width of the Size column. |
| `layout.modified_column_px` | float | `140.0` | Width of the Modified column. |
| `layout.mono_glyph_px` | float | `7.5` | Approximate width of a monospace glyph. Used to estimate how many characters fit in the Name column before ellipsizing. |
| `theme.stripe` | string `#rrggbb` | _(theme-derived)_ | Zebra stripe color for odd rows (in the panes and the Processes list). Omit to fall back to a derived theme blend. |
| `theme.cursor` | string `#rrggbb` | _(theme-derived)_ | Selection cursor color. Omit to use the iced theme's primary-strong color. |
| `theme.mark` | string `#rrggbb` | _(theme-derived)_ | Background for marked rows (range selection). Omit to use the theme's primary-weak color. |
| `theme.folder` | string `#rrggbb` | `#6db4ff` | Name color for directory entries. |
| `theme.action` | string `#rrggbb` | `#b48ead` | Name color for files that have a matching `file_actions` entry — a cue that `Enter` offers more than the default open. |
| `file_actions` | list of actions | _(empty)_ | Custom "open with…" entries shown in the file-action chooser. See [File actions](#file-actions) below. |
| `quick_view` | list of actions | _(empty)_ | Live cursor-driven previews shown in the opposite pane. See [Quick view](#quick-view) below. |
| `watch_folders` | list of strings | `["~/Downloads"]` | Folders to watch for new files. Read once at startup; restart to apply changes. |
| `terminal_app` | string | _(auto)_ | macOS only. Which terminal app to launch for the SSH `Connect`, Docker `Shell`, `Open Claude Code in this folder`, and `Open Terminal in this folder` actions. Common values: `"iTerm"`, `"Terminal"`. Omit (or leave `None`) to auto-pick: `iTerm` when `/Applications/iTerm.app` exists, otherwise `Terminal`. Ignored on Linux / Windows. |
| `folder_editor` | string | `/usr/local/bin/code` | Editor binary for the **Open folder in editor** action, invoked as `<folder_editor> <folder>`. Defaults to the VS Code CLI. Point it at any editor that opens a directory argument (Sublime's `subl`, a `code` under `/opt/homebrew/bin`, an editor wrapper script, etc.). A blank value falls back to the default. |
| `dropbox.app_key` | string | _(unset)_ | Dropbox app key (client ID) from the [App Console](https://www.dropbox.com/developers/apps). Required to enable the Dropbox backend. |
| `dropbox.app_secret` | string | _(unset)_ | Dropbox app secret. Required only for "full" (non-PKCE) apps; PKCE apps can omit it. |
| `dropbox.refresh_token` | string | _(unset)_ | Long-lived OAuth2 refresh token, exchanged on demand for short-lived access tokens. Set this together with `dropbox.app_key` to unlock the **Open Dropbox** command. |
| `ftp` | section | _(defaults)_ | Settings for the in-app FTP server (Command Palette → **FTP server**). See [FTP server](#ftp-server) below. An absent section applies the defaults; a partial section fills in missing fields per field. |

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

## File actions

Pressing `Enter` on a file (other than a `.zip`, which still extracts) opens a
small chooser. The first row is always **Open with default application**; below
it are any `file_actions` entries whose `pattern` matched the file name. Files
that have at least one matching action are shown in `theme.action` in the
listing, so you can tell at a glance that `Enter` offers more than a plain open.

Each entry has these fields:

| Field | Required | Meaning |
|---|---|---|
| `pattern` | yes | Filename glob, matched case-insensitively. `*` matches any run (incl. empty), `?` matches one character; everything else is literal. The whole name must match, so `*.md` matches `notes.md` but not `notes.mdx`. |
| `label` | yes | Text shown for the choice in the chooser. |
| `command` | yes | Shell line to run, with placeholders substituted (see below). |
| `terminal` | no (default `false`) | `false` runs the command in the background and refreshes the panes when it finishes (errors go to the status bar). `true` opens a terminal window running it — use it for interactive or long-running commands. |

Commands run with the file's **own folder as the working directory**, so bare
relative names usually suffice. Placeholders in `command`:

| Placeholder | Expands to | Example (`/docs/report.md`) |
|---|---|---|
| `{file}` | file name with extension | `report.md` |
| `{stem}` | file name without its final extension | `report` |
| `{ext}` | final extension, no dot | `md` |
| `{path}` | the absolute path | `/docs/report.md` |
| `{dir}` | the parent directory | `/docs` |

Each placeholder is **shell-quoted automatically**, so a file name containing
spaces, quotes, or shell metacharacters (`$(…)`, `;`, `` ` ``) is passed as one
literal argument and can't inject commands — **don't add your own quotes**
around a placeholder (write `pandoc -o {stem}.pdf {file}`, not `"{file}"`). The
template text around placeholders is left as-is, so pipes, `&&`, and redirects
still work. `terminal: false` commands discard stdout (there's no terminal to
show it) and report a non-zero exit in the status bar; a command that streams a
lot of output or never exits (`tail -f`) should use `terminal: true`. On macOS
the `terminal: true` variant honors the `terminal_app` setting; only local
panes are supported (the action no-ops on a remote pane).

```yaml
file_actions:
  - pattern: "*.md"
    label: "Convert to PDF (pandoc)"
    command: "pandoc -o {stem}.pdf {file}"
  - pattern: "*.tar.gz"
    label: "Inspect in shell"
    command: "tar tzvf {file} | less"
    terminal: true
```

## Quick view

While the cursor sits on a file (arrow keys or a click, in either pane),
`quick_view` lets rho show a live preview of it in the **opposite** pane's
bottom half, in a fixed-width font. Unlike file actions there's no chooser —
the first entry whose `pattern` matches wins, so put more specific patterns
before a catch-all `pattern: "*"`. Moving the cursor off the file (including
onto a directory or `..`) clears the preview; only local panes are supported.

| Field | Required | Meaning |
|---|---|---|
| `pattern` | yes | Filename glob, same rules as `file_actions.pattern`. |
| `label` | yes | Text shown above the preview body. |
| `command` | no | Shell line to run, with the same placeholders as `file_actions.command` (see above), run in the file's own folder. **Omit it** to show the file's raw contents instead of running anything. |

There's no `terminal` option — output is always captured, never interactive.
A few limits keep an automatic, unattended preview from ever hanging or
ballooning memory: the cursor must settle for ~150ms before anything runs (so
scrolling with an arrow key held down doesn't spawn a process per row),
`command` is killed and reported as an error if it runs longer than 5
seconds, and output (command or raw file) is capped at 200KB with a
`(truncated)` marker. Non-UTF-8 bytes are lossily decoded — binary files
won't crash the preview, but won't render as anything useful either.

```yaml
quick_view:
  - pattern: "*.log"
    label: "Tail"
    command: "tail -n 200 {file}"
  - pattern: "*"
    label: "Preview"
```

## FTP server

Settings for the in-app FTP server (Command Palette → **FTP server**).
Defaults are LAN-accessible and read-write — out of the box, picking the
action shares the active pane's folder on `0.0.0.0:2121` with a generated
username + password and accepts uploads, deletes, and renames. Tune via
the `ftp:` block:

```yaml
ftp:
  port: 2121
  bind: "0.0.0.0"          # "127.0.0.1" for loopback only
  auth: generated          # generated | anonymous
  permissions: read-write  # read-write | read-only
  # username: ron          # optional — pins username (generated mode only)
  # password: hunter2      # optional — pins password (generated mode only)
  passive_port_min: 50000
  passive_port_max: 50050
  # passive_host: "192.168.1.42"   # optional — IP/DNS server announces in PASV
```

| Field | Default | Notes |
|---|---|---|
| `port` | `2121` | TCP port for the FTP control channel. 2121 is the conventional unprivileged port; binding 21 requires root. |
| `bind` | `0.0.0.0` | Bind address. `0.0.0.0` exposes the server on every interface (LAN-accessible); `127.0.0.1` restricts to loopback for local-only testing. |
| `auth` | `generated` | `generated` mints a random 12-character password every time the server starts (and a fixed `rho` username, unless overridden — see below) and shows them in the FTP info modal. `anonymous` lets any client log in with any credentials, and the `username` / `password` fields are ignored. |
| `permissions` | `read-write` | `read-write` forwards `STOR` / `DELE` / `MKD` / `RMD` / `RNFR` through to the underlying filesystem. `read-only` rejects all of those with `550 Permission denied` — pick it when you're sharing files with someone you don't fully trust. |
| `username` | _(unset)_ | For `auth: generated`. When set, the server uses this name verbatim instead of the built-in `"rho"` default. Whitespace-only values are treated as unset. |
| `password` | _(unset)_ | For `auth: generated`. When set, the server uses this password verbatim instead of generating a fresh 12-char random one on every start. Pin this if you want the credentials to survive a restart (e.g. so a saved bookmark in your FTP client keeps working). Whitespace-only values are treated as unset. |
| `passive_port_min` | `50000` | Low end (inclusive) of the passive-mode data-channel port range. FTP uses two ports per session — `port` above for control, and one chosen from this range per file transfer. A narrow range is dramatically easier to whitelist on a router or host firewall. |
| `passive_port_max` | `50050` | High end (inclusive). 51 ports is plenty for a personal share. An inverted or zero pair falls back to the defaults — a typo shouldn't silently disable transfers. |
| `passive_host` | _(unset)_ | What the server advertises in `PASV` responses as the IP for the data-channel connect-back. Unset → libunftp echoes whichever interface IP the client's control connection arrived on, which is usually right. Set to a literal IPv4 (e.g. `"192.168.1.42"`) or a DNS name (e.g. `"laptop.local"`) when the auto-pick is wrong — typically only relevant on multi-homed boxes or behind NAT. |

Note that `~/.rho.yaml` is **plain text** and lives in your home
directory. Anyone with read access to that file can see a pinned
password — don't pin one if that's a concern, or `chmod 600 ~/.rho.yaml`
to restrict it to your user.

Changes are **not hot-reloaded** into a running server — that would yank
existing connections. To pick up new settings: open the FTP info modal,
click **Stop server**, then invoke **FTP server** again. The status bar
shows `FTP server on HOST:PORT → ROOT` while one is running.

### Troubleshooting LAN connections

If the FTP server works from `127.0.0.1` but not from another device on
your WiFi, the cause is almost always a firewall. Walk through this list:

**1. Did the control channel even connect?** Try `lftp -p 2121 -u rho,PASSWORD HOST` from the other machine and watch the modal log:
- If you see `rho: logged in`, the control channel is open — skip to step 3.
- If you see `auth failed: bad password for "rho"`, double-check
  the password against the modal (passwords are random per start by
  default — see [`password`](#ftp-server)).
- If the connection times out with no log entry, it's a firewall on the
  control port — go to step 2.

**2. macOS Application Firewall.** If you're running rho via `cargo run`,
macOS asks Terminal (not rho) for incoming-connection permission, which
is the wrong granularity. Either:
- Run a release build (`cargo build --release && ./target/release/rho`)
  so the firewall prompt names *rho* specifically and clicking *Allow*
  whitelists the binary, or
- Open *System Settings → Network → Firewall → Options…* and add the
  rho binary explicitly with *Allow incoming connections*.

The Application Firewall is per-binary, so once rho is allowed all of
its ports (control + passive) are reachable.

**3. Control channel works, transfers hang.** Almost always a blocked
passive port range. FTP uses two TCP connections per session — the
control port (`2121` by default) and a separate data port chosen from
`passive_port_min..=passive_port_max` (`50000-50050` by default). The
modal logs the actual range on startup.
- On a home router with NAT, the data ports usually only need to be
  open on the host firewall, not forwarded. Add the range to whatever
  blocks them.
- On a Linux box with `ufw`: `sudo ufw allow 50000:50050/tcp` plus
  `sudo ufw allow 2121/tcp`.
- On macOS with the built-in Application Firewall, step 2 is sufficient.

**4. PASV reports the wrong IP.** Rare, but happens behind NAT or on
multi-homed machines. The modal log shows what the server is advertising
(`PASV will advertise host: …` or `PASV will advertise whichever
interface the client connected to`). When the auto-pick (`FromConnection`)
is wrong, set [`passive_host`](#ftp-server) to a literal IP or DNS name.

**5. Some clients only speak EPSV / EPRT.** Modern clients (`lftp`,
Cyberduck, FileZilla) use `EPSV` by default, which avoids the PASV-host
issue entirely. If you're using something exotic (e.g. an old IoT
device), force passive mode explicitly in the client's options.

## Example

```yaml
# Rho configuration file — edits are picked up live (no restart needed).
window:
  width: 1100.0
  rows: 35
layout:
  row_height_px: 19.0
  row_font_size: 13
  header_font_size: 11
  size_column_px: 80.0
  modified_column_px: 140.0
  mono_glyph_px: 7.5
# Optional color overrides (#rrggbb). Comment out a field to derive it from theme.
theme:
  folder: "#6db4ff"
  # stripe: "#1c1d1f"
  # cursor: "#3a80c8"
  # mark: "#2a4a6a"
  # Name color for files that have a matching file_actions entry.
  # action: "#b48ead"
# Custom "open with…" actions — see the File actions section above.
# file_actions:
#   - pattern: "*.md"
#     label: "Convert to PDF (pandoc)"
#     command: "pandoc -o {stem}.pdf {file}"
# Live cursor-driven previews in the opposite pane — see the Quick view
# section above.
# quick_view:
#   - pattern: "*.log"
#     label: "Tail"
#     command: "tail -n 200 {file}"
#   - pattern: "*"
#     label: "Preview"
# Folders to watch for new files.
watch_folders:
  - "~/Downloads"
# Dropbox backend (optional). See the Dropbox section above.
# dropbox:
#   app_key: "xxxxxxxxxxxxxxx"
#   app_secret: "xxxxxxxxxxxxxxx"
#   refresh_token: "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
# In-app FTP server. Omit to use the defaults shown.
# ftp:
#   port: 2121
#   bind: "0.0.0.0"
#   auth: generated
#   permissions: read-write
#   username: "ron"        # optional — pins username across restarts
#   password: "hunter2"    # optional — pins password across restarts
#   passive_port_min: 50000
#   passive_port_max: 50050
#   # passive_host: "192.168.1.42"   # optional — override PASV-advertised host
```
