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

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub row_height_px: f32,
    pub row_font_size: u16,
    pub header_font_size: u16,
    pub size_column_px: f32,
    pub modified_column_px: f32,
    pub window_width: f32,
    pub window_rows: u32,
    pub mono_glyph_px: f32,
    /// Optional `#rrggbb` overrides; `None` means "derive from theme".
    pub stripe_color: Option<String>,
    pub cursor_color: Option<String>,
    pub mark_color: Option<String>,
    pub folder_color: Option<String>,
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
    /// Dropbox app key (client id) from the Dropbox App Console. Combined
    /// with `dropbox_refresh_token` to mint short-lived access tokens for
    /// the `dropbox:` remote backend. `None` (the default) hides the
    /// "Open Dropbox" palette action.
    #[serde(default)]
    pub dropbox_app_key: Option<String>,
    /// Dropbox app secret. Required for "full" (non-PKCE) apps; PKCE apps
    /// can leave this unset. Only ever sent to Dropbox's `oauth2/token`
    /// endpoint when exchanging the refresh token.
    #[serde(default)]
    pub dropbox_app_secret: Option<String>,
    /// Long-lived OAuth2 refresh token (obtained once via the Dropbox
    /// authorize flow with `token_access_type=offline`). Exchanged for a
    /// short-lived access token on demand — see `fs_ops::dropbox_access_token`.
    #[serde(default)]
    pub dropbox_refresh_token: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            row_height_px: 19.0,
            row_font_size: 13,
            header_font_size: 11,
            size_column_px: 80.0,
            modified_column_px: 140.0,
            window_width: 1100.0,
            window_rows: 35,
            mono_glyph_px: 7.5,
            stripe_color: None,
            cursor_color: None,
            mark_color: None,
            folder_color: Some("#6db4ff".to_string()),
            watch_folders: vec!["~/Downloads".to_string()],
            terminal_app: None,
            folder_editor: Some(DEFAULT_FOLDER_EDITOR.to_string()),
            dropbox_app_key: None,
            dropbox_app_secret: None,
            dropbox_refresh_token: None,
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
        let app_key = self.dropbox_app_key.clone()?;
        let refresh_token = self.dropbox_refresh_token.clone()?;
        if app_key.trim().is_empty() || refresh_token.trim().is_empty() {
            return None;
        }
        Some(DropboxAuth {
            app_key,
            app_secret: self
                .dropbox_app_secret
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
        let text_h = self.row_font_size as f32 * 1.3;
        let pad = ((self.row_height_px - text_h) / 2.0).max(0.0);
        pad.round() as u16
    }

    pub fn window_size(&self) -> Size {
        let chrome = 90.0;
        let height = chrome + self.window_rows as f32 * self.row_height_px;
        Size::new(self.window_width, height)
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
     row_height_px: 19.0\n\
     row_font_size: 13\n\
     header_font_size: 11\n\
     size_column_px: 80.0\n\
     modified_column_px: 140.0\n\
     window_width: 1100.0\n\
     window_rows: 35\n\
     mono_glyph_px: 7.5\n\
     # Optional color overrides (#rrggbb). Comment out to derive from theme.\n\
     folder_color: \"#6db4ff\"\n\
     # stripe_color: \"#1c1d1f\"\n\
     # cursor_color: \"#3a80c8\"\n\
     # mark_color: \"#2a4a6a\"\n\
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
     # Dropbox backend. Lets panes browse / copy / move / delete under dropbox:/\n\
     # and enables the \"Open Dropbox\" command. Full setup (create app, grant\n\
     # files.* scopes, mint a refresh token via the authorize flow) is in the\n\
     # docs: https://ronkeizer.github.io/rho/configuration.html#dropbox\n\
     # Gotchas: the refresh_token is NOT the App Console's \"Generate access\n\
     # token\" button (that's a short-lived sl.* access token); after changing\n\
     # scopes you must re-authorize; after editing these, restart rho.\n\
     # app_secret is only needed for non-PKCE apps.\n\
     # dropbox_app_key: \"xxxxxxxxxxxxxxx\"\n\
     # dropbox_app_secret: \"xxxxxxxxxxxxxxx\"\n\
     # dropbox_refresh_token: \"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"\n"
}

#[derive(Debug, Clone, Copy)]
pub struct RowColors {
    pub cursor: Option<Color>,
    pub mark: Option<Color>,
    pub stripe: Option<Color>,
    pub folder: Option<Color>,
}

pub fn row_colors_from(config: &Config) -> RowColors {
    RowColors {
        cursor: config.cursor_color.as_deref().and_then(parse_hex_color),
        mark: config.mark_color.as_deref().and_then(parse_hex_color),
        stripe: config.stripe_color.as_deref().and_then(parse_hex_color),
        folder: config.folder_color.as_deref().and_then(parse_hex_color),
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
        assert!(c.row_height_px > 0.0);
        assert!(c.row_font_size > 0);
        assert!(c.size_column_px > 0.0);
        assert!(c.modified_column_px > 0.0);
        assert!(c.window_width > 0.0);
        assert!(c.window_rows > 0);
        assert!(c.mono_glyph_px > 0.0);
        // Default ships with folder color set so users see colored dirs out of
        // the box.
        assert!(c.folder_color.is_some());
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
    fn row_padding_y_is_non_negative_and_centered() {
        let c = Config {
            row_height_px: 20.0,
            row_font_size: 13,
            ..Config::default()
        };
        // text_h = 13 * 1.3 = 16.9; padding = (20 - 16.9)/2 = 1.55 → round 2.
        assert_eq!(c.row_padding_y(), 2);
    }

    #[test]
    fn row_padding_y_floors_at_zero_when_height_too_small() {
        let c = Config {
            row_height_px: 5.0,
            row_font_size: 13,
            ..Config::default()
        };
        // text_h = 16.9 > row_height; pad would be negative → clamped to 0.
        assert_eq!(c.row_padding_y(), 0);
    }

    #[test]
    fn window_size_depends_on_rows_and_height() {
        let c = Config {
            row_height_px: 20.0,
            window_rows: 30,
            window_width: 1000.0,
            ..Config::default()
        };
        let size = c.window_size();
        assert!(approx_eq(size.width, 1000.0));
        // chrome (90) + 30 * 20 = 690.
        assert!(approx_eq(size.height, 690.0));
    }

    #[test]
    fn config_yaml_partial_uses_defaults_for_missing_fields() {
        let yaml = "row_font_size: 17\n";
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.row_font_size, 17);
        // Other fields fall back to defaults.
        assert_eq!(cfg.row_height_px, Config::default().row_height_px);
        assert_eq!(cfg.window_rows, Config::default().window_rows);
    }

    #[test]
    fn config_yaml_empty_is_all_defaults() {
        let cfg: Config = serde_yaml::from_str("").unwrap_or_default();
        // Both fields the same as Default::default().
        assert_eq!(cfg.row_height_px, Config::default().row_height_px);
        assert_eq!(cfg.row_font_size, Config::default().row_font_size);
    }

    #[test]
    fn row_colors_parses_hex_overrides() {
        let cfg = Config {
            stripe_color: Some("#112233".to_string()),
            cursor_color: Some("#445566".to_string()),
            mark_color: Some("#778899".to_string()),
            folder_color: Some("#aabbcc".to_string()),
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
            stripe_color: None,
            cursor_color: Some("not-a-color".to_string()),
            mark_color: Some("#xyzxyz".to_string()),
            folder_color: None,
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
