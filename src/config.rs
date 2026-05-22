//! User settings (`~/.rho.yaml`), session state (`~/.rho-state.yaml`), color
//! parsing, and the small process-launching helpers (`$EDITOR` / Quick Look)
//! used by the keybindings.

use std::path::{Path, PathBuf};

use iced::{Color, Size};
use serde::Deserialize;

use crate::domain::Side;

pub const SETTINGS_FILENAME: &str = ".rho.yaml";
pub const STATE_FILENAME: &str = ".rho-state.yaml";

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
        }
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
    pub left: PathBuf,
    pub right: PathBuf,
    #[serde(default = "default_active_side")]
    pub active: Side,
    /// Recently-navigated directories, most-recent first. Used by the
    /// "Go to folder" modal as filterable suggestions. Maintained via
    /// [`crate::domain::add_recent`].
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
     # terminal_app: \"iTerm\"\n"
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
            left: PathBuf::from("/tmp/left"),
            right: PathBuf::from("/tmp/right"),
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
        assert_eq!(parsed.left, PathBuf::from("/a"));
        assert_eq!(parsed.right, PathBuf::from("/b"));
        assert_eq!(parsed.active, Side::Left);
        assert!(parsed.recent.is_empty());
    }

    #[test]
    fn saved_state_yaml_uses_lowercase_side() {
        let state = SavedState {
            left: PathBuf::from("/x"),
            right: PathBuf::from("/y"),
            active: Side::Right,
            recent: Vec::new(),
        };
        let yaml = serde_yaml::to_string(&state).unwrap();
        // serde(rename_all = "lowercase") should produce "right" not "Right".
        assert!(yaml.contains("active: right"));
    }
}
