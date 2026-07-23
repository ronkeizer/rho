//! User settings (`~/.rho.yaml`), session state (`~/.rho-state.yaml`), color
//! parsing, and the small process-launching helpers (`$EDITOR` / Quick Look)
//! used by the keybindings.

use std::path::{Path, PathBuf};

use iced::{Color, Size};
use serde::Deserialize;

use crate::domain::{Location, Side};

pub const SETTINGS_FILENAME: &str = ".rho.yaml";
pub const STATE_FILENAME: &str = ".rho-state.yaml";

/// Default editor binary for the "Open folder in editor" action — the VS Code
/// CLI as installed by "Shell Command: Install 'code' command in PATH".
pub const DEFAULT_FOLDER_EDITOR: &str = "/usr/local/bin/code";

/// Default name color for files that have a matching `file_actions` entry — a
/// soft violet, distinct from the blue `folder_color` and the amber git marker.
pub const DEFAULT_ACTION_COLOR: &str = "#b48ead";

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Initial window geometry. Only read once at startup — hot reload
    /// doesn't resize a running window.
    #[serde(default)]
    pub window: WindowConfig,
    /// Row/column/font geometry for the pane lists. Hot-reloaded.
    #[serde(default)]
    pub layout: LayoutConfig,
    /// `#rrggbb` color overrides. Any field left unset derives from the
    /// active iced theme.
    #[serde(default)]
    pub theme: ThemeConfig,
    /// User-defined "open with…" actions, matched by filename glob. When Enter
    /// is pressed on a file the [`Prompt::FileActions`](crate::domain::Prompt)
    /// modal lists the default-open plus every action whose `pattern` matches.
    #[serde(default)]
    pub file_actions: Vec<crate::domain::FileAction>,
    /// User-defined "quick view" previews, matched by filename glob. When the
    /// cursor sits on a matching file, the first match's `label` and command
    /// output (or, if `command` is omitted, the raw file contents) are shown
    /// live in the opposite pane's bottom half.
    #[serde(default)]
    pub quick_view: Vec<crate::domain::QuickViewAction>,
    /// Folders to watch for new files. Each accepts a `~/` prefix. Non-recursive.
    /// Read once at startup — editing this requires restarting the app.
    pub watch_folders: Vec<String>,
    /// Which terminal app to use for "Connect to SSH server" and the Docker
    /// `Shell` action. Currently honored on macOS; the value is the literal
    /// `tell application "<name>"` target. `None` resolves at launch time
    /// to `"iTerm"` when `/Applications/iTerm.app` exists, otherwise
    /// `"Terminal"`. On Linux / Windows this field is currently ignored.
    pub terminal_app: Option<String>,
    /// Editor binary launched by the "Open folder in editor" palette action,
    /// invoked as `<folder_editor> <folder-path>`. Defaults to
    /// [`DEFAULT_FOLDER_EDITOR`] (the VS Code CLI). Resolve via
    /// [`Config::folder_editor`] so an empty / whitespace value falls back to
    /// the default rather than trying to spawn `""`.
    #[serde(default)]
    pub folder_editor: Option<String>,
    /// Dropbox app credentials for the `dropbox:` remote backend. Absent
    /// (the default) hides the "Open Dropbox" palette action.
    #[serde(default)]
    pub dropbox: DropboxConfig,
    /// Settings for the in-app FTP server (Command Palette → "FTP server").
    /// Defaults: LAN-accessible on `0.0.0.0:2121`, generated credentials,
    /// read-only. The server is singleton — restarting it picks up changes
    /// here.
    #[serde(default)]
    pub ftp: FtpConfig,
    /// Interval (seconds) between background CPU/memory samples feeding the
    /// System monitor modal. The sampler runs from app start so the graph is
    /// pre-filled on open. Hot-reloaded via the settings subscription.
    #[serde(default = "default_stats_interval_secs")]
    pub stats_interval_secs: u64,
}

/// Default sampling cadence for the System monitor. 2s balances a responsive
/// graph against the cost of an always-on `sysinfo` refresh.
fn default_stats_interval_secs() -> u64 {
    2
}

impl Default for Config {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            layout: LayoutConfig::default(),
            theme: ThemeConfig::default(),
            file_actions: Vec::new(),
            quick_view: Vec::new(),
            watch_folders: vec!["~/Downloads".to_string()],
            terminal_app: None,
            folder_editor: Some(DEFAULT_FOLDER_EDITOR.to_string()),
            dropbox: DropboxConfig::default(),
            ftp: FtpConfig::default(),
            stats_interval_secs: default_stats_interval_secs(),
        }
    }
}

/// Initial window dimensions. `width` is a literal pixel width; `height` is
/// derived from `rows` × `layout.row_height_px` (see [`Config::window_size`])
/// so the window opens showing exactly that many rows.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub width: f32,
    pub rows: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 1100.0,
            rows: 35,
        }
    }
}

/// Row/column/font geometry for the pane lists. See
/// [`crate::main`]'s `ROW_STRIDE coupling` note — `row_height_px` is the
/// single source of truth for vertical row geometry and auto-scroll math.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LayoutConfig {
    pub row_height_px: f32,
    pub row_font_size: u16,
    pub header_font_size: u16,
    pub size_column_px: f32,
    pub modified_column_px: f32,
    pub mono_glyph_px: f32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            row_height_px: 19.0,
            row_font_size: 13,
            header_font_size: 11,
            size_column_px: 80.0,
            modified_column_px: 140.0,
            mono_glyph_px: 7.5,
        }
    }
}

/// `#rrggbb` color overrides for pane rows; `None` on any field means
/// "derive from the active iced theme".
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub stripe: Option<String>,
    pub cursor: Option<String>,
    pub mark: Option<String>,
    pub folder: Option<String>,
    /// Name color for files that have at least one matching `file_actions`
    /// entry — a cue that Enter offers more than the default open. `None`
    /// falls back to [`DEFAULT_ACTION_COLOR`].
    pub action: Option<String>,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            stripe: None,
            cursor: None,
            mark: None,
            folder: Some("#6db4ff".to_string()),
            action: Some(DEFAULT_ACTION_COLOR.to_string()),
        }
    }
}

/// Dropbox app credentials for the `dropbox:` remote backend. Combined by
/// [`Config::dropbox_auth`] into a [`DropboxAuth`] once both `app_key` and
/// `refresh_token` are present.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DropboxConfig {
    /// App key (client id) from the Dropbox App Console.
    pub app_key: Option<String>,
    /// App secret. Required for "full" (non-PKCE) apps; PKCE apps can leave
    /// this unset. Only ever sent to Dropbox's `oauth2/token` endpoint when
    /// exchanging the refresh token.
    pub app_secret: Option<String>,
    /// Long-lived OAuth2 refresh token (obtained once via the Dropbox
    /// authorize flow with `token_access_type=offline`). Exchanged for a
    /// short-lived access token on demand — see `fs_ops::dropbox_access_token`.
    pub refresh_token: Option<String>,
}

/// Settings for the in-app FTP server. Surfaced under the `ftp:` key in
/// `~/.rho.yaml`; an absent section uses the defaults below. Changes are not
/// hot-reloaded into a running server — restart via "Stop server" + invoking
/// the palette action again.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FtpConfig {
    /// TCP port the control channel listens on. Defaults to 2121 (the
    /// conventional unprivileged FTP port — 21 needs root).
    pub port: u16,
    /// Bind address. `"0.0.0.0"` exposes the server on every interface (i.e.
    /// reachable from other devices on the LAN). Use `"127.0.0.1"` to restrict
    /// to the loopback interface for local-only testing.
    pub bind: String,
    /// How clients authenticate. `Generated` (default) shows a username +
    /// password in the FTP info modal — either the values pinned in
    /// [`Self::username`] / [`Self::password`], or, when those are unset,
    /// the fallback username `"rho"` plus a random 12-char password
    /// generated at startup. `Anonymous` lets any client log in and ignores
    /// the credential fields.
    pub auth: FtpAuthMode,
    /// Whether clients can mutate the served directory. `ReadWrite`
    /// (default) allows uploads / deletes / renames / directory creation.
    /// `ReadOnly` rejects all of those with 550 — pick it when you're
    /// sharing files with someone you don't fully trust.
    pub permissions: FtpPerms,
    /// Username for `auth: generated`. Unset → defaults to `"rho"`. Ignored
    /// for `auth: anonymous`. Setting this pins the username across
    /// restarts (handy for saving server bookmarks in an FTP client).
    #[serde(default)]
    pub username: Option<String>,
    /// Password for `auth: generated`. Unset → a fresh random 12-char
    /// password is generated on every server start. Ignored for
    /// `auth: anonymous`. Setting this trades the rotating password for a
    /// fixed one — note that `~/.rho.yaml` is plain-text, so anyone with
    /// read access to your home directory can see it.
    #[serde(default)]
    pub password: Option<String>,
    /// Low end of the passive-mode data-channel port range. FTP uses two
    /// ports per session — the control channel (`port` above) and a
    /// passive data port chosen from this range. Narrow ranges are
    /// dramatically easier to whitelist on a firewall; libunftp's
    /// own default of `49152..=65535` is large enough that nothing short
    /// of a wide-open NAT policy reliably allows transfers through. Default
    /// is `50000`.
    #[serde(default = "default_passive_port_min")]
    pub passive_port_min: u16,
    /// High end of the passive-mode port range (inclusive). Default is
    /// `50050` — 51 ports, plenty for a personal share.
    #[serde(default = "default_passive_port_max")]
    pub passive_port_max: u16,
    /// What hostname/IP the server advertises in PASV / EPSV responses for
    /// the data-channel connect-back. Unset → libunftp's default
    /// `FromConnection`, which echoes whichever interface IP the client's
    /// control connection arrived on. Set to a literal IPv4 address (e.g.
    /// `"192.168.1.42"`) or a DNS name (e.g. `"laptop.local"`) when the
    /// auto-pick is wrong — typically only relevant on multi-homed boxes
    /// or behind NAT.
    #[serde(default)]
    pub passive_host: Option<String>,
}

fn default_passive_port_min() -> u16 {
    50_000
}

fn default_passive_port_max() -> u16 {
    50_050
}

/// Authentication mode for the in-app FTP server. Serialized as `generated`
/// or `anonymous`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FtpAuthMode {
    /// Random username + 12-char password generated at server start.
    Generated,
    /// No authentication — any username/password is accepted.
    Anonymous,
}

/// Filesystem permissions for the in-app FTP server. Serialized as
/// `read-only` or `read-write`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FtpPerms {
    /// Read-only — STOR/DELE/MKD/RMD/RNFR return 550 Permission denied.
    ReadOnly,
    /// Full access — uploads, deletes, renames, and directory creation are
    /// permitted.
    ReadWrite,
}

impl Default for FtpConfig {
    fn default() -> Self {
        Self {
            port: 2121,
            bind: "0.0.0.0".to_string(),
            auth: FtpAuthMode::Generated,
            permissions: FtpPerms::ReadWrite,
            username: None,
            password: None,
            passive_port_min: default_passive_port_min(),
            passive_port_max: default_passive_port_max(),
            passive_host: None,
        }
    }
}

impl FtpConfig {
    /// Resolved username for `auth: generated`. Returns the configured
    /// value when non-empty, otherwise the built-in default (`"rho"`).
    /// Empty / whitespace-only configured values are treated as unset so
    /// a typo doesn't lock the user out of their own server.
    pub fn resolved_username(&self) -> String {
        self.username
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("rho")
            .to_string()
    }

    /// Resolved password for `auth: generated` — `Some(value)` when the
    /// user has pinned one in config, `None` when the caller should
    /// generate a fresh random one. Whitespace-only configured values are
    /// treated as unset (same reasoning as [`Self::resolved_username`]).
    pub fn resolved_password(&self) -> Option<String> {
        self.password
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }

    /// Normalize the passive-mode port range to a strictly-non-empty
    /// `min..=max` tuple. Falls back to the defaults when the configured
    /// pair is inverted, both zero, or otherwise unusable — a typo
    /// shouldn't silently disable data transfers.
    pub fn resolved_passive_ports(&self) -> (u16, u16) {
        let (lo, hi) = (self.passive_port_min, self.passive_port_max);
        if lo == 0 || hi == 0 || lo > hi {
            (default_passive_port_min(), default_passive_port_max())
        } else {
            (lo, hi)
        }
    }
}

/// Credentials for the `dropbox:` remote backend, pulled from
/// [`Config`]. Present only when both an app key and a refresh token are
/// configured; `app_secret` stays optional (PKCE apps omit it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropboxAuth {
    pub app_key: String,
    pub app_secret: Option<String>,
    pub refresh_token: String,
}

impl Config {
    /// The Dropbox credentials, if both the app key and refresh token are
    /// set. Used to gate the "Open Dropbox" palette action and to mint
    /// access tokens in `fs_ops`.
    pub fn dropbox_auth(&self) -> Option<DropboxAuth> {
        let app_key = self.dropbox.app_key.clone()?;
        let refresh_token = self.dropbox.refresh_token.clone()?;
        if app_key.trim().is_empty() || refresh_token.trim().is_empty() {
            return None;
        }
        Some(DropboxAuth {
            app_key,
            app_secret: self
                .dropbox
                .app_secret
                .clone()
                .filter(|s| !s.trim().is_empty()),
            refresh_token,
        })
    }
}

impl Config {
    /// Read the file if present; never writes. Missing file → defaults.
    /// Parse errors are logged and fall back to defaults so a bad save doesn't
    /// brick the running app during hot reload.
    pub fn load() -> Self {
        let path = settings_path();
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_yaml::from_str::<Self>(&contents) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "warning: failed to parse {}: {} — keeping previous settings",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    /// The editor binary for "Open folder in editor". Falls back to
    /// [`DEFAULT_FOLDER_EDITOR`] when unset or blank.
    pub fn folder_editor(&self) -> &str {
        self.folder_editor
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_FOLDER_EDITOR)
    }

    pub fn row_padding_y(&self) -> u16 {
        let text_h = self.layout.row_font_size as f32 * 1.3;
        let pad = ((self.layout.row_height_px - text_h) / 2.0).max(0.0);
        pad.round() as u16
    }

    pub fn window_size(&self) -> Size {
        let chrome = 90.0;
        let height = chrome + self.window.rows as f32 * self.layout.row_height_px;
        Size::new(self.window.width, height)
    }
}

pub fn settings_path() -> PathBuf {
    home_dir().join(SETTINGS_FILENAME)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SavedState {
    /// Where the left pane was pointing. Serialized as a string —
    /// local paths round-trip unchanged (`left: /Users/ron`); remote
    /// locations use the `<backend>:<path>` form (e.g.
    /// `left: alice.dev:/var/log`).
    pub left: Location,
    pub right: Location,
    #[serde(default = "default_active_side")]
    pub active: Side,
    /// Recently-navigated **local** directories, most-recent first.
    /// Used by the "Go to folder" modal as filterable suggestions.
    /// Maintained via [`crate::domain::add_recent`]. Remote panes
    /// don't feed this list — the SSH-server picker is the entry point
    /// for those.
    #[serde(default)]
    pub recent: Vec<PathBuf>,
}

pub fn default_active_side() -> Side {
    Side::Left
}

pub fn state_path() -> PathBuf {
    home_dir().join(STATE_FILENAME)
}

/// Best-effort read. Missing file, parse errors, or vanished paths all just
/// fall back to the home directory — we never want a stale state file to
/// block startup.
pub fn load_saved_state() -> Option<SavedState> {
    let contents = std::fs::read_to_string(state_path()).ok()?;
    serde_yaml::from_str(&contents).ok()
}

pub fn save_state_to_disk(state: &SavedState) {
    if let Ok(yaml) = serde_yaml::to_string(state) {
        let _ = std::fs::write(state_path(), yaml);
    }
}

/// Create the settings file with a starter template if it doesn't exist yet.
/// Called from `main()` and before opening the file in the editor (Cmd+,).
pub fn ensure_settings_file() {
    let path = settings_path();
    if !path.exists() {
        let _ = std::fs::write(&path, default_template_yaml());
    }
}

/// Open `path` in a code editor. Honors `$VISUAL` then `$EDITOR`; otherwise
/// uses the platform's default text-editor opener (`open -t` on macOS).
pub fn open_in_editor(path: &Path) -> std::io::Result<()> {
    let env_editor = std::env::var_os("VISUAL")
        .or_else(|| std::env::var_os("EDITOR"))
        .filter(|s| !s.is_empty());
    if let Some(editor) = env_editor {
        let editor = editor.to_string_lossy().into_owned();
        let mut parts = editor.split_whitespace();
        if let Some(prog) = parts.next() {
            let args: Vec<&str> = parts.collect();
            return std::process::Command::new(prog)
                .args(args)
                .arg(path)
                .spawn()
                .map(|_| ());
        }
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-t")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(target_os = "macos"))]
    {
        open::that_detached(path).map(|_| ())
    }
}

/// Spawn a Quick Look preview window for `path` (macOS only; no-op elsewhere).
pub fn quick_look(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("qlmanage")
            .arg("-p")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Ok(())
    }
}

fn default_template_yaml() -> &'static str {
    "# Rho configuration file — edits are picked up live (no restart needed).\n\
     # Initial window size. Only read at startup — hot reload won't resize an\n\
     # already-open window.\n\
     window:\n\
     \x20\x20width: 1100.0\n\
     \x20\x20rows: 35\n\
     # Row/column/font geometry for the pane lists.\n\
     layout:\n\
     \x20\x20row_height_px: 19.0\n\
     \x20\x20row_font_size: 13\n\
     \x20\x20header_font_size: 11\n\
     \x20\x20size_column_px: 80.0\n\
     \x20\x20modified_column_px: 140.0\n\
     \x20\x20mono_glyph_px: 7.5\n\
     # Color overrides (#rrggbb). Comment out a field to derive it from theme.\n\
     theme:\n\
     \x20\x20folder: \"#6db4ff\"\n\
     \x20\x20# stripe: \"#1c1d1f\"\n\
     \x20\x20# cursor: \"#3a80c8\"\n\
     \x20\x20# mark: \"#2a4a6a\"\n\
     \x20\x20# Name color for files with a matching file_actions entry (below).\n\
     \x20\x20# action: \"#b48ead\"\n\
     # Folders to watch for new files. App pops up a prompt offering to switch\n\
     # a pane to that folder. Non-recursive. Restart to apply changes.\n\
     watch_folders:\n\
     \x20\x20- \"~/Downloads\"\n\
     # macOS only. Which terminal app the SSH / Docker shell actions use.\n\
     # Defaults to \"iTerm\" if /Applications/iTerm.app exists, else \"Terminal\".\n\
     # terminal_app: \"iTerm\"\n\
     # Editor launched by \"Open folder in editor\" (run as `<editor> <folder>`).\n\
     # Defaults to the VS Code CLI; point it at any editor that opens a folder.\n\
     # folder_editor: \"/usr/local/bin/code\"\n\
     # Custom \"open with…\" actions. Pressing Enter on a file shows a chooser:\n\
     # \"Open with default application\" plus every action whose pattern (a\n\
     # *.glob, case-insensitive) matches the file name. command placeholders:\n\
     #   {file} name (report.md)  {stem} name w/o ext (report)  {ext} ext (md)\n\
     #   {path} absolute path     {dir} parent dir\n\
     # Commands run with the file's folder as the working directory. Each\n\
     # placeholder is shell-quoted automatically, so DON'T add your own quotes\n\
     # around one (names with spaces / special chars are handled). terminal:\n\
     # true opens a terminal window (for interactive / long-running commands);\n\
     # the default (false) runs in the background and refreshes the pane.\n\
     # file_actions:\n\
     #   - pattern: \"*.md\"\n\
     #     label: \"Convert to PDF (pandoc)\"\n\
     #     command: \"pandoc -o {stem}.pdf {file}\"\n\
     #   - pattern: \"*.tar.gz\"\n\
     #     label: \"Inspect in shell\"\n\
     #     command: \"tar tzvf {file} | less\"\n\
     #     terminal: true\n\
     # Quick view: while the cursor sits on a matching file, the first entry\n\
     # whose pattern matches runs (or, with no command, just reads the file)\n\
     # and shows the result live in the opposite pane's bottom half. Same\n\
     # placeholders as file_actions. Commands time out after 5s and output is\n\
     # capped at 200KB; no terminal option (output is always captured, never\n\
     # interactive).\n\
     # quick_view:\n\
     #   - pattern: \"*.log\"\n\
     #     label: \"Tail\"\n\
     #     command: \"tail -n 200 {file}\"\n\
     #   - pattern: \"*\"\n\
     #     label: \"Preview\"\n\
     # Dropbox backend. Lets panes browse / copy / move / delete under dropbox:/\n\
     # and enables the \"Open Dropbox\" command. Full setup (create app, grant\n\
     # files.* scopes, mint a refresh token via the authorize flow) is in the\n\
     # docs: https://ronkeizer.github.io/rho/configuration.html#dropbox\n\
     # Gotchas: the refresh_token is NOT the App Console's \"Generate access\n\
     # token\" button (that's a short-lived sl.* access token); after changing\n\
     # scopes you must re-authorize; after editing these, restart rho.\n\
     # app_secret is only needed for non-PKCE apps.\n\
     # dropbox:\n\
     #   app_key: \"xxxxxxxxxxxxxxx\"\n\
     #   app_secret: \"xxxxxxxxxxxxxxx\"\n\
     #   refresh_token: \"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"\n\
     # In-app FTP server (Command Palette → \"FTP server\"). Defaults shown;\n\
     # any omitted field falls back to the default. Changes apply on next\n\
     # server start — stop the running server first via the FTP info modal.\n\
     # Leave username / password unset to use the built-in default user\n\
     # (\"rho\") plus a fresh random password on every start. Setting them\n\
     # pins the credentials; rho.yaml is plain text, so treat the file as\n\
     # readable to anyone with access to your home directory.\n\
     # ftp:\n\
     #   port: 2121\n\
     #   bind: \"0.0.0.0\"            # \"127.0.0.1\" for loopback-only\n\
     #   auth: generated             # generated | anonymous\n\
     #   permissions: read-write     # read-write | read-only\n\
     #   username: \"ron\"\n\
     #   password: \"hunter2\"\n\
     #   # Passive-mode data-channel ports. Narrow + predictable so you\n\
     #   # can whitelist them on a router / firewall once. Open these in\n\
     #   # your firewall along with the control port (2121 above) — most\n\
     #   # \"control connects but transfer hangs\" symptoms come from a\n\
     #   # filtered passive range. macOS users: also tell the Application\n\
     #   # Firewall to allow rho (System Settings → Network → Firewall).\n\
     #   passive_port_min: 50000\n\
     #   passive_port_max: 50050\n\
     #   # Hostname / IPv4 the server advertises in PASV responses. Unset\n\
     #   # means \"echo whichever interface the client connected to\" —\n\
     #   # set this when you're behind NAT or on a multi-homed box.\n\
     #   passive_host: \"192.168.1.42\"\n\
     # System monitor (Command Palette → \"System monitor\"). Seconds between\n\
     # background CPU/memory samples. The sampler runs from app start so the\n\
     # graph is already populated when you open the modal. Lower = smoother\n\
     # graph but more frequent polling. Hot-reloaded.\n\
     # stats_interval_secs: 2\n"
}

#[derive(Debug, Clone, Copy)]
pub struct RowColors {
    pub cursor: Option<Color>,
    pub mark: Option<Color>,
    pub stripe: Option<Color>,
    pub folder: Option<Color>,
    /// Name color for files with a matching `file_actions` entry. Falls back
    /// to [`DEFAULT_ACTION_COLOR`] when unset / unparseable.
    pub action: Option<Color>,
}

pub fn row_colors_from(config: &Config) -> RowColors {
    RowColors {
        cursor: config.theme.cursor.as_deref().and_then(parse_hex_color),
        mark: config.theme.mark.as_deref().and_then(parse_hex_color),
        stripe: config.theme.stripe.as_deref().and_then(parse_hex_color),
        folder: config.theme.folder.as_deref().and_then(parse_hex_color),
        action: config
            .theme
            .action
            .as_deref()
            .and_then(parse_hex_color)
            .or_else(|| parse_hex_color(DEFAULT_ACTION_COLOR)),
    }
}

pub fn parse_hex_color(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()? as f32 / 255.0;
    let g = u8::from_str_radix(&s[2..4], 16).ok()? as f32 / 255.0;
    let b = u8::from_str_radix(&s[4..6], 16).ok()? as f32 / 255.0;
    Some(Color { r, g, b, a: 1.0 })
}

pub fn blend(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r * (1.0 - t) + b.r * t,
        g: a.g * (1.0 - t) + b.g * t,
        b: a.b * (1.0 - t) + b.b * t,
        a: 1.0,
    }
}

/// Pull a color closer to the background so hidden entries (`.foo`) read as
/// muted compared to a "regular" file. Used both for the name widget (which
/// may have a folder-color override) and for the row's default text color.
pub fn dim(c: Color) -> Color {
    Color {
        r: c.r * 0.55,
        g: c.g * 0.55,
        b: c.b * 0.55,
        a: c.a,
    }
}

pub fn home_dir() -> PathBuf {
    #[allow(deprecated)]
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

pub fn expand_tilde(p: &str) -> String {
    if let Some(rest) = p.strip_prefix('~') {
        return format!("{}{}", home_dir().display(), rest);
    }
    p.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn parse_hex_color_with_hash() {
        let c = parse_hex_color("#ff8800").unwrap();
        assert!(approx_eq(c.r, 1.0));
        assert!(approx_eq(c.g, 0x88 as f32 / 255.0));
        assert!(approx_eq(c.b, 0.0));
        assert!(approx_eq(c.a, 1.0));
    }

    #[test]
    fn parse_hex_color_without_hash() {
        let c = parse_hex_color("00ff00").unwrap();
        assert!(approx_eq(c.r, 0.0));
        assert!(approx_eq(c.g, 1.0));
        assert!(approx_eq(c.b, 0.0));
    }

    #[test]
    fn parse_hex_color_rejects_short() {
        assert!(parse_hex_color("#fff").is_none());
    }

    #[test]
    fn parse_hex_color_rejects_long() {
        assert!(parse_hex_color("#ff88001a").is_none());
    }

    #[test]
    fn parse_hex_color_rejects_non_hex() {
        assert!(parse_hex_color("#gghhii").is_none());
    }

    #[test]
    fn parse_hex_color_rejects_empty() {
        assert!(parse_hex_color("").is_none());
    }

    #[test]
    fn default_stats_interval_is_two_secs() {
        assert_eq!(Config::default().stats_interval_secs, 2);
    }

    #[test]
    fn missing_stats_interval_falls_back_to_default() {
        // A config with no `stats_interval_secs:` key should deserialize to
        // the 2s default rather than 0 (which would busy-poll).
        let cfg: Config = serde_yaml::from_str("watch_folders: []\n").unwrap();
        assert_eq!(cfg.stats_interval_secs, 2);
    }

    #[test]
    fn parse_hex_color_strips_whitespace() {
        let c = parse_hex_color("  #112233  ").unwrap();
        assert!(approx_eq(c.r, 0x11 as f32 / 255.0));
        assert!(approx_eq(c.g, 0x22 as f32 / 255.0));
        assert!(approx_eq(c.b, 0x33 as f32 / 255.0));
    }

    #[test]
    fn blend_at_zero_returns_a() {
        let a = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
        let b = Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 };
        let result = blend(a, b, 0.0);
        assert!(approx_eq(result.r, 1.0));
        assert!(approx_eq(result.g, 0.0));
    }

    #[test]
    fn blend_at_one_returns_b() {
        let a = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
        let b = Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 };
        let result = blend(a, b, 1.0);
        assert!(approx_eq(result.r, 0.0));
        assert!(approx_eq(result.g, 1.0));
    }

    #[test]
    fn blend_at_midpoint() {
        let a = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
        let b = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
        let result = blend(a, b, 0.5);
        assert!(approx_eq(result.r, 0.5));
        assert!(approx_eq(result.g, 0.5));
        assert!(approx_eq(result.b, 0.5));
    }

    #[test]
    fn dim_scales_each_channel() {
        let c = Color { r: 1.0, g: 0.8, b: 0.4, a: 1.0 };
        let d = dim(c);
        assert!(approx_eq(d.r, 0.55));
        assert!(approx_eq(d.g, 0.8 * 0.55));
        assert!(approx_eq(d.b, 0.4 * 0.55));
        // Alpha is unchanged.
        assert!(approx_eq(d.a, 1.0));
    }

    #[test]
    fn expand_tilde_without_tilde_passes_through() {
        assert_eq!(expand_tilde("/var/log"), "/var/log");
    }

    #[test]
    fn expand_tilde_with_tilde_slash_substitutes_home() {
        let expanded = expand_tilde("~/Documents");
        let home = home_dir();
        assert_eq!(expanded, format!("{}/Documents", home.display()));
    }

    #[test]
    fn expand_tilde_only() {
        // Just "~" expands to home (no trailing content).
        let expanded = expand_tilde("~");
        assert_eq!(expanded, home_dir().display().to_string());
    }

    #[test]
    fn default_active_side_is_left() {
        assert_eq!(default_active_side(), Side::Left);
    }

    #[test]
    fn config_default_values_are_sensible() {
        let c = Config::default();
        assert!(c.layout.row_height_px > 0.0);
        assert!(c.layout.row_font_size > 0);
        assert!(c.layout.size_column_px > 0.0);
        assert!(c.layout.modified_column_px > 0.0);
        assert!(c.window.width > 0.0);
        assert!(c.window.rows > 0);
        assert!(c.layout.mono_glyph_px > 0.0);
        // Default ships with folder color set so users see colored dirs out of
        // the box.
        assert!(c.theme.folder.is_some());
    }

    #[test]
    fn folder_editor_defaults_to_vscode_cli() {
        // Out of the box the action targets the VS Code CLI.
        assert_eq!(Config::default().folder_editor(), DEFAULT_FOLDER_EDITOR);
    }

    #[test]
    fn folder_editor_uses_configured_value_and_trims() {
        let c = Config {
            folder_editor: Some("  /opt/nvim/bin/nvim  ".to_string()),
            ..Config::default()
        };
        assert_eq!(c.folder_editor(), "/opt/nvim/bin/nvim");
    }

    #[test]
    fn folder_editor_falls_back_when_blank_or_unset() {
        let blank = Config {
            folder_editor: Some("   ".to_string()),
            ..Config::default()
        };
        assert_eq!(blank.folder_editor(), DEFAULT_FOLDER_EDITOR);
        let unset = Config {
            folder_editor: None,
            ..Config::default()
        };
        assert_eq!(unset.folder_editor(), DEFAULT_FOLDER_EDITOR);
    }

    #[test]
    fn action_color_defaults_and_parses_into_row_colors() {
        // Out of the box RowColors.action resolves to the default violet.
        let colors = row_colors_from(&Config::default());
        assert_eq!(colors.action, parse_hex_color(DEFAULT_ACTION_COLOR));
        // A configured color wins; an unparseable one falls back to default.
        let custom = Config {
            theme: ThemeConfig {
                action: Some("#102030".to_string()),
                ..ThemeConfig::default()
            },
            ..Config::default()
        };
        assert_eq!(row_colors_from(&custom).action, parse_hex_color("#102030"));
        let bad = Config {
            theme: ThemeConfig {
                action: Some("not-a-color".to_string()),
                ..ThemeConfig::default()
            },
            ..Config::default()
        };
        assert_eq!(
            row_colors_from(&bad).action,
            parse_hex_color(DEFAULT_ACTION_COLOR)
        );
    }

    #[test]
    fn file_actions_deserialize_from_yaml() {
        let yaml = r#"
file_actions:
  - pattern: "*.md"
    label: "Convert to PDF"
    command: "pandoc -o {stem}.pdf {file}"
  - pattern: "*.tar.gz"
    label: "Inspect"
    command: "tar tzvf {file} | less"
    terminal: true
"#;
        let c: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.file_actions.len(), 2);
        // `terminal` defaults to false when omitted.
        assert!(!c.file_actions[0].terminal);
        assert_eq!(c.file_actions[0].pattern, "*.md");
        assert!(c.file_actions[1].terminal);
    }

    #[test]
    fn file_actions_default_to_empty() {
        assert!(Config::default().file_actions.is_empty());
    }

    #[test]
    fn quick_view_deserialize_from_yaml() {
        let yaml = r#"
quick_view:
  - pattern: "*.log"
    label: "Tail"
    command: "tail -n 200 {file}"
  - pattern: "*"
    label: "Preview"
"#;
        let c: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.quick_view.len(), 2);
        assert_eq!(c.quick_view[0].command.as_deref(), Some("tail -n 200 {file}"));
        // `command` is optional — omitted means "show raw file contents".
        assert_eq!(c.quick_view[1].command, None);
    }

    #[test]
    fn quick_view_defaults_to_empty() {
        assert!(Config::default().quick_view.is_empty());
    }

    #[test]
    fn row_padding_y_is_non_negative_and_centered() {
        let c = Config {
            layout: LayoutConfig {
                row_height_px: 20.0,
                row_font_size: 13,
                ..LayoutConfig::default()
            },
            ..Config::default()
        };
        // text_h = 13 * 1.3 = 16.9; padding = (20 - 16.9)/2 = 1.55 → round 2.
        assert_eq!(c.row_padding_y(), 2);
    }

    #[test]
    fn row_padding_y_floors_at_zero_when_height_too_small() {
        let c = Config {
            layout: LayoutConfig {
                row_height_px: 5.0,
                row_font_size: 13,
                ..LayoutConfig::default()
            },
            ..Config::default()
        };
        // text_h = 16.9 > row_height; pad would be negative → clamped to 0.
        assert_eq!(c.row_padding_y(), 0);
    }

    #[test]
    fn window_size_depends_on_rows_and_height() {
        let c = Config {
            layout: LayoutConfig {
                row_height_px: 20.0,
                ..LayoutConfig::default()
            },
            window: WindowConfig {
                rows: 30,
                width: 1000.0,
            },
            ..Config::default()
        };
        let size = c.window_size();
        assert!(approx_eq(size.width, 1000.0));
        // chrome (90) + 30 * 20 = 690.
        assert!(approx_eq(size.height, 690.0));
    }

    #[test]
    fn config_yaml_partial_uses_defaults_for_missing_fields() {
        let yaml = "layout:\n  row_font_size: 17\n";
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.layout.row_font_size, 17);
        // Other fields fall back to defaults.
        assert_eq!(
            cfg.layout.row_height_px,
            Config::default().layout.row_height_px
        );
        assert_eq!(cfg.window.rows, Config::default().window.rows);
    }

    #[test]
    fn config_yaml_empty_is_all_defaults() {
        let cfg: Config = serde_yaml::from_str("").unwrap_or_default();
        // Both fields the same as Default::default().
        assert_eq!(
            cfg.layout.row_height_px,
            Config::default().layout.row_height_px
        );
        assert_eq!(
            cfg.layout.row_font_size,
            Config::default().layout.row_font_size
        );
    }

    #[test]
    fn row_colors_parses_hex_overrides() {
        let cfg = Config {
            theme: ThemeConfig {
                stripe: Some("#112233".to_string()),
                cursor: Some("#445566".to_string()),
                mark: Some("#778899".to_string()),
                folder: Some("#aabbcc".to_string()),
                ..ThemeConfig::default()
            },
            ..Config::default()
        };
        let colors = row_colors_from(&cfg);
        assert!(colors.stripe.is_some());
        assert!(colors.cursor.is_some());
        assert!(colors.mark.is_some());
        assert!(colors.folder.is_some());
    }

    #[test]
    fn row_colors_unset_or_invalid_resolve_to_none() {
        let cfg = Config {
            theme: ThemeConfig {
                stripe: None,
                cursor: Some("not-a-color".to_string()),
                mark: Some("#xyzxyz".to_string()),
                folder: None,
                ..ThemeConfig::default()
            },
            ..Config::default()
        };
        let colors = row_colors_from(&cfg);
        assert!(colors.stripe.is_none());
        assert!(colors.cursor.is_none());
        assert!(colors.mark.is_none());
        assert!(colors.folder.is_none());
    }

    #[test]
    fn saved_state_roundtrips_through_yaml() {
        let state = SavedState {
            left: Location::Local(PathBuf::from("/tmp/left")),
            right: Location::Local(PathBuf::from("/tmp/right")),
            active: Side::Right,
            recent: vec![PathBuf::from("/etc"), PathBuf::from("/usr/local")],
        };
        let yaml = serde_yaml::to_string(&state).unwrap();
        let parsed: SavedState = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.left, state.left);
        assert_eq!(parsed.right, state.right);
        assert_eq!(parsed.active, state.active);
        assert_eq!(parsed.recent, state.recent);
    }

    #[test]
    fn saved_state_active_field_defaults_when_missing() {
        // Older state files (or hand-edited ones) may omit `active` and
        // `recent`. The serde defaults should fill in Side::Left and an
        // empty list rather than failing.
        let yaml = "left: /a\nright: /b\n";
        let parsed: SavedState = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.left, Location::Local(PathBuf::from("/a")));
        assert_eq!(parsed.right, Location::Local(PathBuf::from("/b")));
        assert_eq!(parsed.active, Side::Left);
        assert!(parsed.recent.is_empty());
    }

    #[test]
    fn saved_state_yaml_uses_lowercase_side() {
        let state = SavedState {
            left: Location::Local(PathBuf::from("/x")),
            right: Location::Local(PathBuf::from("/y")),
            active: Side::Right,
            recent: Vec::new(),
        };
        let yaml = serde_yaml::to_string(&state).unwrap();
        // serde(rename_all = "lowercase") should produce "right" not "Right".
        assert!(yaml.contains("active: right"));
    }

    #[test]
    fn saved_state_reads_pre_phase1_yaml_unchanged() {
        // State files written by older versions of rho stored `left`/`right`
        // as plain path strings. Those round-trip through `Location::Local`
        // identically (the serialization format is the same for local
        // paths), so existing state files keep loading after the type
        // change without migration.
        let yaml = "left: /Users/ron\nright: /tmp\nactive: left\nrecent: []\n";
        let parsed: SavedState = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.left, Location::Local(PathBuf::from("/Users/ron")));
        assert_eq!(parsed.right, Location::Local(PathBuf::from("/tmp")));
    }

    #[test]
    fn ftp_config_defaults_are_lan_readwrite_generated() {
        let c = FtpConfig::default();
        assert_eq!(c.port, 2121);
        assert_eq!(c.bind, "0.0.0.0");
        assert_eq!(c.auth, FtpAuthMode::Generated);
        assert_eq!(c.permissions, FtpPerms::ReadWrite);
        // Narrow predictable passive range — easy to whitelist on a LAN
        // firewall. The defaults should be 51 ports.
        assert_eq!(c.passive_port_min, 50_000);
        assert_eq!(c.passive_port_max, 50_050);
        assert!(c.passive_host.is_none());
    }

    #[test]
    fn ftp_config_passive_range_resolves_normally() {
        let c = FtpConfig::default();
        assert_eq!(c.resolved_passive_ports(), (50_000, 50_050));
    }

    #[test]
    fn ftp_config_passive_range_falls_back_when_inverted() {
        let c = FtpConfig {
            passive_port_min: 60_000,
            passive_port_max: 50_000,
            ..FtpConfig::default()
        };
        assert_eq!(c.resolved_passive_ports(), (50_000, 50_050));
    }

    #[test]
    fn ftp_config_passive_range_falls_back_when_zero() {
        let c = FtpConfig {
            passive_port_min: 0,
            passive_port_max: 0,
            ..FtpConfig::default()
        };
        assert_eq!(c.resolved_passive_ports(), (50_000, 50_050));
    }

    #[test]
    fn ftp_config_passive_range_honors_explicit_pair() {
        let c = FtpConfig {
            passive_port_min: 30_000,
            passive_port_max: 30_010,
            ..FtpConfig::default()
        };
        assert_eq!(c.resolved_passive_ports(), (30_000, 30_010));
    }

    #[test]
    fn ftp_config_absent_section_uses_defaults() {
        let cfg: Config = serde_yaml::from_str("row_font_size: 14\n").unwrap();
        let d = FtpConfig::default();
        assert_eq!(cfg.ftp.port, d.port);
        assert_eq!(cfg.ftp.bind, d.bind);
        assert_eq!(cfg.ftp.auth, d.auth);
        assert_eq!(cfg.ftp.permissions, d.permissions);
    }

    #[test]
    fn ftp_config_partial_section_fills_in_defaults_per_field() {
        // Only `port` provided — the other three fall back to defaults.
        let yaml = "ftp:\n  port: 9999\n";
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.ftp.port, 9999);
        assert_eq!(cfg.ftp.bind, FtpConfig::default().bind);
        assert_eq!(cfg.ftp.auth, FtpAuthMode::Generated);
        assert_eq!(cfg.ftp.permissions, FtpPerms::ReadWrite);
    }

    #[test]
    fn ftp_config_parses_all_fields_and_enum_renames() {
        let yaml = "\
ftp:
  port: 2100
  bind: \"127.0.0.1\"
  auth: anonymous
  permissions: read-write
";
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.ftp.port, 2100);
        assert_eq!(cfg.ftp.bind, "127.0.0.1");
        assert_eq!(cfg.ftp.auth, FtpAuthMode::Anonymous);
        assert_eq!(cfg.ftp.permissions, FtpPerms::ReadWrite);
    }

    #[test]
    fn ftp_config_username_pinned_via_yaml() {
        let yaml = "ftp:\n  username: ron\n";
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.ftp.resolved_username(), "ron");
        // No password pinned — caller generates one.
        assert!(cfg.ftp.resolved_password().is_none());
    }

    #[test]
    fn ftp_config_password_pinned_via_yaml() {
        let yaml = "ftp:\n  password: hunter2\n";
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        // Username falls back to the built-in default.
        assert_eq!(cfg.ftp.resolved_username(), "rho");
        assert_eq!(cfg.ftp.resolved_password().as_deref(), Some("hunter2"));
    }

    #[test]
    fn ftp_config_blank_credentials_fall_back_to_defaults() {
        // Whitespace-only values are treated as unset — a stray space
        // shouldn't silently lock the user out of their own server.
        let yaml = "ftp:\n  username: \"   \"\n  password: \"\"\n";
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.ftp.resolved_username(), "rho");
        assert!(cfg.ftp.resolved_password().is_none());
    }

    #[test]
    fn ftp_config_resolution_preserves_pinned_pair() {
        let yaml = "ftp:\n  username: ron\n  password: hunter2\n";
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.ftp.resolved_username(), "ron");
        assert_eq!(cfg.ftp.resolved_password().as_deref(), Some("hunter2"));
    }

    #[test]
    fn ftp_config_unknown_enum_value_fails_parse() {
        // A typo on the perms enum should be a hard error rather than a
        // silent fallback — a misconfigured server is something to notice.
        let yaml = "ftp:\n  permissions: read-only-ish\n";
        assert!(serde_yaml::from_str::<Config>(yaml).is_err());
    }

    #[test]
    fn saved_state_round_trips_remote_location() {
        let state = SavedState {
            left: Location::Local(PathBuf::from("/tmp")),
            right: Location::Remote {
                backend: crate::domain::BackendId::new("alice.dev"),
                path: PathBuf::from("/var/log"),
            },
            active: Side::Left,
            recent: Vec::new(),
        };
        let yaml = serde_yaml::to_string(&state).unwrap();
        let parsed: SavedState = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.right, state.right);
    }
}
