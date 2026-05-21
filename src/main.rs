use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local};
use iced::alignment::Horizontal;
use iced::keyboard::{self, key::Named, Key, Modifiers};
use iced::widget::{
    button, column, container, mouse_area, row, scrollable, stack, text, text_input, Space,
};
use iced::{
    time, window, Border, Color, Element, Font, Length, Padding, Shadow, Size, Subscription, Task,
    Theme, Vector,
};
use serde::Deserialize;

const PROMPT_ID: &str = "prompt";
const SETTINGS_FILENAME: &str = ".fm.yaml";
const STATE_FILENAME: &str = ".fm-state.yaml";

// ---------------------------------------------------------------------------
// Configuration (~/.fm.yaml)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct Config {
    row_height_px: f32,
    row_font_size: u16,
    header_font_size: u16,
    size_column_px: f32,
    modified_column_px: f32,
    window_width: f32,
    window_rows: u32,
    mono_glyph_px: f32,
    /// Optional `#rrggbb` overrides; `None` means "derive from theme".
    stripe_color: Option<String>,
    cursor_color: Option<String>,
    mark_color: Option<String>,
    folder_color: Option<String>,
    /// Folders to watch for new files. Each accepts a `~/` prefix. Non-recursive.
    /// Read once at startup — editing this requires restarting the app.
    watch_folders: Vec<String>,
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
        }
    }
}

impl Config {
    /// Read the file if present; never writes. Missing file → defaults.
    /// Parse errors are logged and fall back to defaults so a bad save doesn't
    /// brick the running app during hot reload.
    fn load() -> Self {
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

    fn row_padding_y(&self) -> u16 {
        let text_h = self.row_font_size as f32 * 1.3;
        let pad = ((self.row_height_px - text_h) / 2.0).max(0.0);
        pad.round() as u16
    }

    fn window_size(&self) -> Size {
        let chrome = 90.0;
        let height = chrome + self.window_rows as f32 * self.row_height_px;
        Size::new(self.window_width, height)
    }
}

fn settings_path() -> PathBuf {
    home_dir().join(SETTINGS_FILENAME)
}

// ---------------------------------------------------------------------------
// Per-pane folder persistence (~/.fm-state.yaml)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SavedState {
    left: PathBuf,
    right: PathBuf,
    #[serde(default = "default_active_side")]
    active: Side,
}

fn default_active_side() -> Side {
    Side::Left
}

fn state_path() -> PathBuf {
    home_dir().join(STATE_FILENAME)
}

/// Best-effort read. Missing file, parse errors, or vanished paths all just
/// fall back to the home directory — we never want a stale state file to
/// block startup.
fn load_saved_state() -> Option<SavedState> {
    let contents = std::fs::read_to_string(state_path()).ok()?;
    serde_yaml::from_str(&contents).ok()
}

fn save_state_to_disk(state: &SavedState) {
    if let Ok(yaml) = serde_yaml::to_string(state) {
        let _ = std::fs::write(state_path(), yaml);
    }
}

/// Create the settings file with a starter template if it doesn't exist yet.
/// Called from `main()` and before opening the file in the editor (Cmd+,).
fn ensure_settings_file() {
    let path = settings_path();
    if !path.exists() {
        let _ = std::fs::write(&path, default_template_yaml());
    }
}

fn default_template_yaml() -> &'static str {
    "# fm configuration file — edits are picked up live (no restart needed).\n\
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
     \x20\x20- \"~/Downloads\"\n"
}

#[derive(Debug, Clone, Copy)]
struct RowColors {
    cursor: Option<Color>,
    mark: Option<Color>,
    stripe: Option<Color>,
    folder: Option<Color>,
}

fn row_colors_from(config: &Config) -> RowColors {
    RowColors {
        cursor: config.cursor_color.as_deref().and_then(parse_hex_color),
        mark: config.mark_color.as_deref().and_then(parse_hex_color),
        stripe: config.stripe_color.as_deref().and_then(parse_hex_color),
        folder: config.folder_color.as_deref().and_then(parse_hex_color),
    }
}

fn parse_hex_color(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()? as f32 / 255.0;
    let g = u8::from_str_radix(&s[2..4], 16).ok()? as f32 / 255.0;
    let b = u8::from_str_radix(&s[4..6], 16).ok()? as f32 / 255.0;
    Some(Color { r, g, b, a: 1.0 })
}

fn blend(a: Color, b: Color, t: f32) -> Color {
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
fn dim(c: Color) -> Color {
    Color {
        r: c.r * 0.55,
        g: c.g * 0.55,
        b: c.b * 0.55,
        a: c.a,
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> iced::Result {
    ensure_settings_file();
    let config = Config::load();
    let window_size = config.window_size();
    iced::application("fm", App::update, App::view)
        .subscription(App::subscription)
        .theme(|_| Theme::Dark)
        .window_size(window_size)
        .run_with(move || App::new(config))
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum Side {
    Left,
    Right,
}

impl Side {
    fn other(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortBy {
    Name,
    Size,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortDir {
    Asc,
    Desc,
}

impl SortDir {
    fn toggled(self) -> Self {
        match self {
            SortDir::Asc => SortDir::Desc,
            SortDir::Desc => SortDir::Asc,
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    is_dir: bool,
    size: Option<u64>,
    modified: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RowVisual {
    None,
    Cursor,
    Marked,
}

/// Modal flavors. Each carries the data the modal needs.
#[derive(Debug, Clone)]
enum Prompt {
    Open {
        input: String,
    },
    Copy {
        input: String,
    },
    Delete {
        paths: Vec<PathBuf>,
        focus: DeleteFocus,
    },
    /// Filesystem watcher noticed new files in a watched folder; ask the user
    /// whether to open the folder in one of the panes.
    NewFiles {
        folder: PathBuf,
        files: Vec<String>,
        focus: NewFilesFocus,
    },
    CommandPalette {
        selected: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteFocus {
    Cancel,
    Confirm,
}

impl DeleteFocus {
    fn toggle(self) -> Self {
        match self {
            DeleteFocus::Cancel => DeleteFocus::Confirm,
            DeleteFocus::Confirm => DeleteFocus::Cancel,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewFilesFocus {
    No,
    Left,
    Right,
}

impl NewFilesFocus {
    fn next(self) -> Self {
        match self {
            NewFilesFocus::No => NewFilesFocus::Left,
            NewFilesFocus::Left => NewFilesFocus::Right,
            NewFilesFocus::Right => NewFilesFocus::No,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteAction {
    Copy,
    Delete,
    Exit,
}

impl PaletteAction {
    const ALL: &'static [PaletteAction] = &[
        PaletteAction::Copy,
        PaletteAction::Delete,
        PaletteAction::Exit,
    ];

    fn label(self) -> &'static str {
        match self {
            PaletteAction::Copy => "Copy",
            PaletteAction::Delete => "Delete",
            PaletteAction::Exit => "Exit",
        }
    }
}

#[derive(Debug, Clone)]
struct GitInfo {
    branch: String,
    uncommitted: usize,
    ahead: usize,
    behind: usize,
    /// Names within the current pane directory that have uncommitted changes.
    /// For files, the name matches directly; for directories, at least one
    /// descendant has changes (we collapse subpaths to their first segment).
    modified_names: HashSet<String>,
}

#[derive(Debug)]
struct Pane {
    path: PathBuf,
    entries: Vec<Entry>,
    /// Indices into `entries` that pass the current filter, in display order.
    /// Row 0 is the ".." entry; rows 1..=visible_indices.len() map to
    /// `entries[visible_indices[i - 1]]`.
    visible_indices: Vec<usize>,
    /// Current type-to-filter query. Empty = all entries visible.
    filter: String,
    selected: usize,
    /// Anchor for shift-extended range selection.
    anchor: usize,
    sort_by: SortBy,
    sort_dir: SortDir,
    scroll_y: f32,
    viewport_height: Option<f32>,
    loading: bool,
    /// Bumped on every navigate. Streaming load tasks tag each chunk with the
    /// generation that started them, so chunks that finish late after the user
    /// has already moved on get dropped.
    load_generation: u64,
    /// Result of the async git probe, if this directory is inside a git repo.
    git_info: Option<GitInfo>,
    /// Name of an entry the cursor should land on as soon as it appears in
    /// the streaming load. Used to focus on a newly-detected file after the
    /// "new files" modal opens its folder. Cleared on `navigate()`, or when
    /// the entry has been located.
    pending_focus: Option<String>,
}

impl Pane {
    fn empty(path: PathBuf) -> Self {
        Self {
            path,
            entries: Vec::new(),
            visible_indices: Vec::new(),
            filter: String::new(),
            selected: 0,
            anchor: 0,
            sort_by: SortBy::Name,
            sort_dir: SortDir::Asc,
            scroll_y: 0.0,
            viewport_height: None,
            loading: true,
            load_generation: 0,
            git_info: None,
            pending_focus: None,
        }
    }

    /// Reset the pane to "about to load `path`". Returns the new generation
    /// that the matching load_dir_task should tag chunks with.
    fn navigate(&mut self, path: PathBuf) -> u64 {
        self.path = path;
        self.entries.clear();
        self.visible_indices.clear();
        self.filter.clear();
        self.selected = 0;
        self.anchor = 0;
        self.loading = true;
        self.load_generation += 1;
        self.git_info = None;
        // A fresh navigation overrides any in-flight focus request. The caller
        // re-sets pending_focus *after* navigate() if it wants one.
        self.pending_focus = None;
        self.load_generation
    }

    fn append_chunk(&mut self, mut chunk: Vec<Entry>) {
        if chunk.is_empty() {
            return;
        }
        // `pending_focus` (set by the caller, e.g. NewFilesPickSide) wins over
        // the natural "preserve previous cursor" behavior — we want the cursor
        // to *land on* the new file as soon as it shows up in any chunk.
        let preserve = self.pending_focus.clone().or_else(|| self.cursor_name());
        self.entries.append(&mut chunk);
        // Sorting is deferred to EntriesDone so we don't re-sort the growing
        // list on every chunk (which would be O(n² log n) total work on the
        // main thread and stall keyboard events like F5).
        self.recompute_visible(preserve.as_deref());

        // Clear the focus request once we've actually positioned the cursor
        // on the requested entry. If the chunk didn't include it yet, leave
        // pending_focus set so the next chunk can retry.
        if let Some(target) = &self.pending_focus {
            if self.cursor_name().as_deref() == Some(target.as_str()) {
                self.pending_focus = None;
            }
        }
    }

    /// Rebuild `visible_indices` from `entries` and the current filter, then
    /// either restore the cursor onto the entry that was selected before
    /// (if it's still visible) or clamp it into range.
    fn recompute_visible(&mut self, preserve_name: Option<&str>) {
        self.visible_indices = if self.filter.is_empty() {
            (0..self.entries.len()).collect()
        } else {
            match regex::RegexBuilder::new(&self.filter)
                .case_insensitive(true)
                .build()
            {
                Ok(re) => (0..self.entries.len())
                    .filter(|&i| re.is_match(&self.entries[i].name))
                    .collect(),
                Err(_) => {
                    // Partial input that isn't a valid regex yet — fall back
                    // to a case-insensitive substring match so the user sees
                    // sensible results while typing things like "(".
                    let needle = self.filter.to_lowercase();
                    (0..self.entries.len())
                        .filter(|&i| self.entries[i].name.to_lowercase().contains(&needle))
                        .collect()
                }
            }
        };

        let new_selected = match preserve_name {
            Some(name) => self
                .visible_indices
                .iter()
                .position(|&i| self.entries.get(i).map(|e| e.name.as_str()) == Some(name))
                .map(|p| p + 1)
                .unwrap_or(0),
            None => {
                let max = self.visible_indices.len();
                self.selected.min(max)
            }
        };
        self.selected = new_selected;
        self.anchor = new_selected;
    }

    fn cursor_name(&self) -> Option<String> {
        if self.selected == 0 {
            return None;
        }
        self.visible_indices
            .get(self.selected - 1)
            .and_then(|&i| self.entries.get(i))
            .map(|e| e.name.clone())
    }

    fn entry_at(&self, row_index: usize) -> Option<&Entry> {
        if row_index == 0 {
            return None;
        }
        self.visible_indices
            .get(row_index - 1)
            .and_then(|&i| self.entries.get(i))
    }

    fn move_to(&mut self, target: i32, extend: bool) {
        let max = self.visible_indices.len() as i32;
        let next = target.clamp(0, max) as usize;
        if !extend {
            self.anchor = next;
        }
        self.selected = next;
    }

    fn move_by(&mut self, delta: i32, extend: bool) {
        let target = self.selected as i32 + delta;
        self.move_to(target, extend);
    }

    fn mark_range(&self) -> (usize, usize) {
        if self.anchor < self.selected {
            (self.anchor, self.selected)
        } else {
            (self.selected, self.anchor)
        }
    }

    fn is_marked(&self, row_index: usize) -> bool {
        let (lo, hi) = self.mark_range();
        row_index >= lo && row_index <= hi
    }

    fn marked_paths(&self) -> Vec<PathBuf> {
        let (lo, hi) = self.mark_range();
        (lo..=hi)
            .filter_map(|i| self.entry_at(i))
            .map(|e| self.path.join(&e.name))
            .collect()
    }

    fn toggle_sort(&mut self, by: SortBy) {
        let prior = self.cursor_name();
        if self.sort_by == by {
            self.sort_dir = self.sort_dir.toggled();
        } else {
            self.sort_by = by;
            self.sort_dir = SortDir::Asc;
        }
        sort_entries(&mut self.entries, self.sort_by, self.sort_dir);
        self.recompute_visible(prior.as_deref());
    }
}

fn sort_entries(entries: &mut [Entry], by: SortBy, dir: SortDir) {
    use std::cmp::Reverse;
    // Dirs always sort before files; within each group sort by the chosen
    // column. sort_by_cached_key computes the key once per element (O(n)
    // allocations for the Name case) instead of on every pairwise comparison
    // (O(n log n) allocations with the old sort_by approach).
    match (by, dir) {
        (SortBy::Name, SortDir::Asc) => {
            entries.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase()));
        }
        (SortBy::Name, SortDir::Desc) => {
            entries.sort_by_cached_key(|e| (!e.is_dir, Reverse(e.name.to_lowercase())));
        }
        (SortBy::Size, SortDir::Asc) => {
            entries.sort_by_key(|e| (!e.is_dir, e.size.unwrap_or(0)));
        }
        (SortBy::Size, SortDir::Desc) => {
            entries.sort_by_key(|e| (!e.is_dir, Reverse(e.size.unwrap_or(0))));
        }
        (SortBy::Modified, SortDir::Asc) => {
            entries.sort_by_key(|e| (!e.is_dir, e.modified));
        }
        (SortBy::Modified, SortDir::Desc) => {
            entries.sort_by_key(|e| (!e.is_dir, Reverse(e.modified)));
        }
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Message {
    ShiftChanged(bool),
    Activate(Side),
    RowClicked(Side, usize),
    GoUpActive,
    MoveSelection(i32, bool),
    PageMove(i32, bool),
    SwitchSide,
    ActivateSelection,
    ToggleSort(Side, SortBy),
    Scrolled(Side, f32, f32),
    OpenPrompt,
    OpenCopyPrompt,
    OpenDeletePrompt,
    SwitchPromptFocus,
    OpenSettingsFile,
    FilterAppend(String),
    CheckSettings,
    PromptChanged(String),
    PromptSubmit,
    PromptCancel,
    EntriesChunk(Side, u64, Vec<Entry>),
    EntriesDone(Side, u64),
    GitInfoLoaded(Side, u64, Option<GitInfo>),
    OpenFile,
    CopyFinished(Vec<(PathBuf, Result<(), String>)>),
    DeleteFinished(Vec<(PathBuf, Result<(), String>)>),
    Resized(Size),
    /// The watcher subscription noticed new files in `folder`.
    NewFilesDetected(PathBuf, Vec<String>),
    /// User picked a pane in the NewFiles modal.
    NewFilesPickSide(Side),
    /// Open the command-palette modal (Cmd+Shift+P).
    OpenCommandPalette,
    /// Move the palette cursor by `delta` rows (wraps).
    PaletteMove(i32),
    /// User selected an action from the command palette.
    PaletteSelect(PaletteAction),
    NoOp,
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

struct App {
    config: Config,
    left: Pane,
    right: Pane,
    active: Side,
    prompt: Option<Prompt>,
    shift_held: bool,
    window_size: Size,
    settings_mtime: Option<SystemTime>,
    /// (count, dest) while a copy task is in flight.
    copy_in_progress: Option<(usize, PathBuf)>,
    /// File count while a delete task is in flight.
    delete_in_progress: Option<usize>,
    /// New-file detections that arrived while another modal was open. Drained
    /// FIFO when the current modal closes.
    pending_new_files: std::collections::VecDeque<(PathBuf, Vec<String>)>,
}

impl App {
    fn new(config: Config) -> (Self, Task<Message>) {
        let home = home_dir();
        let window_size = config.window_size();
        let settings_mtime = std::fs::metadata(settings_path())
            .and_then(|m| m.modified())
            .ok();

        // Restore last-used folders + active side. Any saved path that no
        // longer exists falls back to the home directory.
        let saved = load_saved_state();
        let left_path = saved
            .as_ref()
            .map(|s| s.left.clone())
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| home.clone());
        let right_path = saved
            .as_ref()
            .map(|s| s.right.clone())
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| home.clone());
        let active = saved.as_ref().map(|s| s.active).unwrap_or(Side::Left);

        let app = Self {
            config,
            left: Pane::empty(left_path.clone()),
            right: Pane::empty(right_path.clone()),
            active,
            prompt: None,
            shift_held: false,
            window_size,
            settings_mtime,
            copy_in_progress: None,
            delete_in_progress: None,
            pending_new_files: std::collections::VecDeque::new(),
        };
        // Pane::empty starts at load_generation 0; the initial loads tag
        // their chunks the same way so they're accepted.
        let init = Task::batch([
            loading_tasks(Side::Left, left_path, 0),
            loading_tasks(Side::Right, right_path, 0),
        ]);
        (app, init)
    }

    fn save_state(&self) {
        save_state_to_disk(&SavedState {
            left: self.left.path.clone(),
            right: self.right.path.clone(),
            active: self.active,
        });
    }

    fn pane(&self, side: Side) -> &Pane {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    fn pane_mut(&mut self, side: Side) -> &mut Pane {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        }
    }

    fn navigate(&mut self, side: Side, path: PathBuf) -> Task<Message> {
        let generation = self.pane_mut(side).navigate(path.clone());
        self.save_state();
        Task::batch([
            loading_tasks(side, path, generation),
            self.ensure_active_visible(),
        ])
    }

    fn page_size(&self) -> i32 {
        let vh = self
            .pane(self.active)
            .viewport_height
            .unwrap_or_else(|| viewport_height_estimate(self.window_size.height));
        let rows = (vh / self.config.row_height_px).floor() as i32 - 1;
        rows.max(1)
    }

    fn ensure_active_visible(&mut self) -> Task<Message> {
        let side = self.active;
        let fallback_vh = viewport_height_estimate(self.window_size.height);
        let stride = self.config.row_height_px;
        let pane = self.pane_mut(side);
        let vh = pane.viewport_height.unwrap_or(fallback_vh);
        let row_top = pane.selected as f32 * stride;
        let row_bottom = row_top + stride;
        let view_top = pane.scroll_y;
        let view_bottom = view_top + vh;

        let target = if row_top < view_top {
            Some(row_top)
        } else if row_bottom > view_bottom {
            Some((row_bottom - vh).max(0.0))
        } else {
            None
        };

        if let Some(y) = target {
            pane.scroll_y = y;
            scrollable::scroll_to(scroll_id(side), scrollable::AbsoluteOffset { x: 0.0, y })
        } else {
            Task::none()
        }
    }

    /// Force-scroll `side`'s pane so its selected row sits roughly a third of
    /// the way down the viewport. Used right after a `pending_focus` request
    /// is resolved — `ensure_active_visible` is too conservative there because
    /// `pane.scroll_y` may be stale from before the navigation.
    fn scroll_to_focused(&mut self, side: Side) -> Task<Message> {
        let fallback_vh = viewport_height_estimate(self.window_size.height);
        let stride = self.config.row_height_px;
        let pane = self.pane_mut(side);
        let vh = pane.viewport_height.unwrap_or(fallback_vh);
        let row_top = pane.selected as f32 * stride;
        // Leave some context above the file so it's not glued to the top edge.
        let y = (row_top - vh / 3.0).max(0.0);
        pane.scroll_y = y;
        scrollable::scroll_to(scroll_id(side), scrollable::AbsoluteOffset { x: 0.0, y })
    }

    fn reload_both_panes(&mut self) -> Task<Message> {
        let left_path = self.left.path.clone();
        let right_path = self.right.path.clone();
        let left_gen = self.left.navigate(left_path.clone());
        let right_gen = self.right.navigate(right_path.clone());
        Task::batch([
            loading_tasks(Side::Left, left_path, left_gen),
            loading_tasks(Side::Right, right_path, right_gen),
        ])
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        // Modals without a text_input let navigation keys fall through to
        // top-level messages. Rewrite them to focus-aware actions so the user
        // can drive the dialog with the keyboard alone: Enter activates the
        // focused button, Tab/←/→ flip focus.
        let msg = match (&self.prompt, msg) {
            (Some(Prompt::Delete { focus, .. }), m) => {
                let focus = *focus;
                match m {
                    Message::ActivateSelection => match focus {
                        DeleteFocus::Confirm => Message::PromptSubmit,
                        DeleteFocus::Cancel => Message::PromptCancel,
                    },
                    Message::SwitchSide => Message::SwitchPromptFocus,
                    other => other,
                }
            }
            (Some(Prompt::NewFiles { focus, .. }), m) => {
                let focus = *focus;
                match m {
                    Message::ActivateSelection => match focus {
                        NewFilesFocus::No => Message::PromptCancel,
                        NewFilesFocus::Left => Message::NewFilesPickSide(Side::Left),
                        NewFilesFocus::Right => Message::NewFilesPickSide(Side::Right),
                    },
                    Message::SwitchSide => Message::SwitchPromptFocus,
                    other => other,
                }
            }
            (Some(Prompt::CommandPalette { selected }), m) => {
                let selected = *selected;
                match m {
                    // Enter activates the currently highlighted action.
                    Message::ActivateSelection => {
                        Message::PaletteSelect(PaletteAction::ALL[selected])
                    }
                    // Arrow keys + Tab cycle the selection (handled as PaletteMove).
                    Message::MoveSelection(delta, _) => Message::PaletteMove(delta),
                    Message::SwitchSide => Message::PaletteMove(1),
                    other => other,
                }
            }
            (_, m) => m,
        };

        // While *any* modal is open, suppress pane-nav keys that text_input
        // didn't capture (Tab, F-keys, arrows-in-no-input).
        if self.prompt.is_some() {
            match &msg {
                Message::MoveSelection(..)
                | Message::PageMove(..)
                | Message::ActivateSelection
                | Message::SwitchSide
                | Message::GoUpActive
                | Message::OpenFile
                | Message::OpenCopyPrompt
                | Message::OpenDeletePrompt
                | Message::OpenCommandPalette
                | Message::FilterAppend(..) => return Task::none(),
                _ => {}
            }
        }

        match msg {
            Message::ShiftChanged(state) => {
                self.shift_held = state;
            }
            Message::Activate(side) => {
                if self.active != side {
                    self.active = side;
                    self.save_state();
                }
            }
            Message::RowClicked(side, row_index) => {
                let active_changed = self.active != side;
                self.active = side;
                let extend = self.shift_held;
                self.pane_mut(side).move_to(row_index as i32, extend);
                if active_changed {
                    self.save_state();
                }
                return self.ensure_active_visible();
            }
            Message::GoUpActive => {
                let side = self.active;
                // If a filter is active, Backspace edits the filter instead
                // of leaving the current directory.
                {
                    let pane = self.pane_mut(side);
                    if !pane.filter.is_empty() {
                        let prior = pane.cursor_name();
                        pane.filter.pop();
                        pane.recompute_visible(prior.as_deref());
                        return Task::none();
                    }
                }
                if let Some(parent) = self.pane(side).path.parent().map(|p| p.to_path_buf()) {
                    return self.navigate(side, parent);
                }
            }
            Message::MoveSelection(delta, extend) => {
                let side = self.active;
                self.pane_mut(side).move_by(delta, extend);
                return self.ensure_active_visible();
            }
            Message::PageMove(dir, extend) => {
                let page = self.page_size();
                let side = self.active;
                self.pane_mut(side).move_by(dir * page, extend);
                return self.ensure_active_visible();
            }
            Message::SwitchSide => {
                self.active = self.active.other();
                self.save_state();
                return self.ensure_active_visible();
            }
            Message::ActivateSelection => {
                let side = self.active;
                let target = if self.pane(side).selected == 0 {
                    self.pane(side).path.parent().map(|p| p.to_path_buf())
                } else {
                    let pane = self.pane(side);
                    pane.entry_at(pane.selected).and_then(|e| {
                        if e.is_dir {
                            Some(pane.path.join(&e.name))
                        } else {
                            None
                        }
                    })
                };
                if let Some(target) = target {
                    return self.navigate(side, target);
                }
            }
            Message::ToggleSort(side, by) => {
                self.active = side;
                self.pane_mut(side).toggle_sort(by);
                return self.ensure_active_visible();
            }
            Message::Scrolled(side, scroll_y, vh) => {
                let pane = self.pane_mut(side);
                pane.scroll_y = scroll_y;
                pane.viewport_height = Some(vh);
            }
            Message::OpenPrompt => {
                let current = self.pane(self.active).path.display().to_string();
                self.prompt = Some(Prompt::Open { input: current });
                return text_input::focus(text_input::Id::new(PROMPT_ID));
            }
            Message::OpenCopyPrompt => {
                let other = self.active.other();
                let dest = self.pane(other).path.display().to_string();
                self.prompt = Some(Prompt::Copy { input: dest });
                return text_input::focus(text_input::Id::new(PROMPT_ID));
            }
            Message::OpenDeletePrompt => {
                let paths = self.pane(self.active).marked_paths();
                if !paths.is_empty() {
                    // Default focus on Cancel: a stray Enter shouldn't delete.
                    self.prompt = Some(Prompt::Delete {
                        paths,
                        focus: DeleteFocus::Cancel,
                    });
                }
            }
            Message::SwitchPromptFocus => match self.prompt.as_mut() {
                Some(Prompt::Delete { focus, .. }) => *focus = focus.toggle(),
                Some(Prompt::NewFiles { focus, .. }) => *focus = focus.next(),
                _ => {}
            },
            Message::OpenSettingsFile => {
                ensure_settings_file();
                let path = settings_path();
                if let Err(e) = open::that_detached(&path) {
                    eprintln!("failed to open {}: {}", path.display(), e);
                }
            }
            Message::CheckSettings => {
                let current = std::fs::metadata(settings_path())
                    .and_then(|m| m.modified())
                    .ok();
                if current != self.settings_mtime {
                    self.settings_mtime = current;
                    self.config = Config::load();
                }
            }
            Message::PromptChanged(value) => {
                if let Some(prompt) = self.prompt.as_mut() {
                    match prompt {
                        Prompt::Open { input } | Prompt::Copy { input } => {
                            *input = value;
                        }
                        Prompt::Delete { .. }
                        | Prompt::NewFiles { .. }
                        | Prompt::CommandPalette { .. } => {}
                    }
                }
            }
            Message::PromptSubmit => {
                let task = if let Some(prompt) = self.prompt.take() {
                    match prompt {
                        Prompt::Open { input } => {
                            let path = PathBuf::from(expand_tilde(input.trim()));
                            if path.is_dir() {
                                Some(self.navigate(self.active, path))
                            } else {
                                None
                            }
                        }
                        Prompt::Copy { input } => {
                            let dest = PathBuf::from(expand_tilde(input.trim()));
                            if dest.is_dir() {
                                let srcs = self.pane(self.active).marked_paths();
                                if !srcs.is_empty() {
                                    self.copy_in_progress = Some((srcs.len(), dest.clone()));
                                    Some(copy_task(srcs, dest))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                        Prompt::Delete { paths, .. } => {
                            if !paths.is_empty() {
                                self.delete_in_progress = Some(paths.len());
                                Some(delete_task(paths))
                            } else {
                                None
                            }
                        }
                        // NewFiles uses NewFilesPickSide / PromptCancel, never
                        // PromptSubmit. If we somehow get here, just drop it.
                        Prompt::NewFiles { .. } => None,
                        // CommandPalette has its own action messages; PromptSubmit
                        // is a no-op here.
                        Prompt::CommandPalette { .. } => None,
                    }
                } else {
                    None
                };
                self.show_next_pending();
                if let Some(t) = task {
                    return t;
                }
            }
            Message::PromptCancel => {
                if self.prompt.is_some() {
                    self.prompt = None;
                    self.show_next_pending();
                } else {
                    // No modal — Esc clears the active pane's filter.
                    let side = self.active;
                    let pane = self.pane_mut(side);
                    if !pane.filter.is_empty() {
                        let prior = pane.cursor_name();
                        pane.filter.clear();
                        pane.recompute_visible(prior.as_deref());
                    }
                }
            }
            Message::FilterAppend(s) => {
                let side = self.active;
                let pane = self.pane_mut(side);
                let prior = pane.cursor_name();
                pane.filter.push_str(&s);
                pane.recompute_visible(prior.as_deref());
            }
            Message::EntriesChunk(side, generation, chunk) => {
                // Read pending-focus state *before* the append, so we can
                // detect when this chunk is the one that resolves the focus
                // request and force a scroll in that case.
                let had_pending = self.pane(side).pending_focus.is_some();
                let pane = self.pane_mut(side);
                if pane.load_generation == generation {
                    pane.append_chunk(chunk);
                    if had_pending && self.pane(side).pending_focus.is_none() {
                        return self.scroll_to_focused(side);
                    }
                    return self.ensure_active_visible();
                }
            }
            Message::EntriesDone(side, generation) => {
                let pane = self.pane_mut(side);
                if pane.load_generation == generation {
                    // All chunks have arrived — sort once now that the list is
                    // complete. This is O(n log n) total instead of the
                    // O(n² log n) that resulted from sorting in append_chunk.
                    let preserve = pane.cursor_name();
                    sort_entries(&mut pane.entries, pane.sort_by, pane.sort_dir);
                    pane.recompute_visible(preserve.as_deref());
                    pane.loading = false;
                }
            }
            Message::GitInfoLoaded(side, generation, info) => {
                let pane = self.pane_mut(side);
                if pane.load_generation == generation {
                    pane.git_info = info;
                }
            }
            Message::OpenFile => {
                let pane = self.pane(self.active);
                if let Some(entry) = pane.entry_at(pane.selected) {
                    let path = pane.path.join(&entry.name);
                    if let Err(e) = open::that_detached(&path) {
                        eprintln!("failed to open {}: {}", path.display(), e);
                    }
                }
            }
            Message::CopyFinished(results) => {
                self.copy_in_progress = None;
                for (src, res) in &results {
                    if let Err(e) = res {
                        eprintln!("copy {} failed: {}", src.display(), e);
                    }
                }
                return self.reload_both_panes();
            }
            Message::DeleteFinished(results) => {
                self.delete_in_progress = None;
                for (src, res) in &results {
                    if let Err(e) = res {
                        eprintln!("delete {} failed: {}", src.display(), e);
                    }
                }
                return self.reload_both_panes();
            }
            Message::Resized(size) => {
                self.window_size = size;
            }
            Message::NewFilesDetected(folder, files) => {
                if files.is_empty() {
                    // Shouldn't happen — the watcher only emits non-empty
                    // batches — but guard so a stray empty event doesn't pop
                    // an empty modal.
                } else if self.prompt.is_none() {
                    self.prompt = Some(Prompt::NewFiles {
                        folder,
                        files,
                        focus: NewFilesFocus::No,
                    });
                } else {
                    self.pending_new_files.push_back((folder, files));
                }
            }
            Message::NewFilesPickSide(side) => {
                if let Some(Prompt::NewFiles { folder, files, .. }) = self.prompt.take() {
                    let task = self.navigate(side, folder);
                    // The watcher accumulates names in arrival order within a
                    // burst, so `.last()` is the most-recently-added file.
                    // navigate() just cleared pending_focus; set it after.
                    if let Some(latest) = files.last().cloned() {
                        self.pane_mut(side).pending_focus = Some(latest);
                    }
                    self.active = side;
                    self.save_state();
                    self.show_next_pending();
                    return task;
                }
            }
            Message::OpenCommandPalette => {
                self.prompt = Some(Prompt::CommandPalette { selected: 0 });
            }
            Message::PaletteMove(delta) => {
                if let Some(Prompt::CommandPalette { selected }) = self.prompt.as_mut() {
                    let n = PaletteAction::ALL.len() as i32;
                    *selected = ((*selected as i32 + delta).rem_euclid(n)) as usize;
                }
            }
            Message::PaletteSelect(action) => {
                self.prompt = None;
                match action {
                    PaletteAction::Copy => {
                        let other = self.active.other();
                        let dest = self.pane(other).path.display().to_string();
                        self.prompt = Some(Prompt::Copy { input: dest });
                        return text_input::focus(text_input::Id::new(PROMPT_ID));
                    }
                    PaletteAction::Delete => {
                        let paths = self.pane(self.active).marked_paths();
                        if !paths.is_empty() {
                            self.prompt = Some(Prompt::Delete {
                                paths,
                                focus: DeleteFocus::Cancel,
                            });
                        }
                    }
                    PaletteAction::Exit => {
                        return window::get_oldest().and_then(window::close);
                    }
                }
            }
            Message::NoOp => {}
        }
        Task::none()
    }

    /// If the currently-displayed modal has just closed and we have queued
    /// new-file detections waiting, surface the next one.
    fn show_next_pending(&mut self) {
        if self.prompt.is_some() {
            return;
        }
        if let Some((folder, files)) = self.pending_new_files.pop_front() {
            self.prompt = Some(Prompt::NewFiles {
                folder,
                files,
                focus: NewFilesFocus::No,
            });
        }
    }

    /// Returns a transient status string when something async is happening.
    /// Priority is copy > delete > load (copy/delete usually start *after*
    /// the panes have loaded, so this ordering surfaces the user-initiated
    /// action over background loading noise).
    fn status_text(&self) -> Option<String> {
        if let Some((count, dest)) = &self.copy_in_progress {
            let noun = if *count == 1 { "file" } else { "files" };
            return Some(format!("Copying {} {} to {}…", count, noun, dest.display()));
        }
        if let Some(count) = self.delete_in_progress {
            let noun = if count == 1 { "file" } else { "files" };
            return Some(format!("Deleting {} {}…", count, noun));
        }
        if self.left.loading || self.right.loading {
            let loading_side = if self.left.loading {
                Side::Left
            } else {
                Side::Right
            };
            let p = self.pane(loading_side);
            return Some(format!("Loading {}…", p.path.display()));
        }
        None
    }

    fn view(&self) -> Element<'_, Message> {
        let colors = row_colors_from(&self.config);
        let name_max = name_max_chars(self.window_size.width, &self.config);
        let panes = row![
            view_pane(
                Side::Left,
                &self.left,
                self.active == Side::Left,
                name_max,
                &self.config,
                colors,
                self.window_size.height,
            ),
            view_pane(
                Side::Right,
                &self.right,
                self.active == Side::Right,
                name_max,
                &self.config,
                colors,
                self.window_size.height,
            ),
        ]
        .spacing(8)
        .height(Length::Fill);

        let marks = {
            let p = self.pane(self.active);
            let n = p.marked_paths().len();
            if n > 1 {
                format!("{} marked", n)
            } else {
                String::new()
            }
        };
        let hints = text(format!(
            "Active: {}{}{}   ·   ⌘P go to folder  ·  ⌘, settings  ·  F4 open  ·  F5 copy  ·  Delete  ·  ↑↓/PgUp/PgDn (+Shift extend)  ·  Tab switch  ·  Backspace up",
            match self.active {
                Side::Left => "left",
                Side::Right => "right",
            },
            if marks.is_empty() { "" } else { "   ·   " },
            marks,
        ))
        .size(11);

        let status_bar: Element<'_, Message> = match self.status_text() {
            Some(msg) => container(text(msg).size(11))
                .padding(Padding::from([4, 8]))
                .width(Length::Fill)
                .style(|theme: &Theme| {
                    let palette = theme.extended_palette();
                    container::Style {
                        background: Some(palette.primary.weak.color.into()),
                        text_color: Some(palette.primary.weak.text),
                        ..Default::default()
                    }
                })
                .into(),
            None => container(text("Ready").size(11))
                .padding(Padding::from([4, 8]))
                .width(Length::Fill)
                .style(|theme: &Theme| {
                    let palette = theme.extended_palette();
                    container::Style {
                        background: Some(palette.background.weak.color.into()),
                        text_color: Some(palette.background.weak.text),
                        ..Default::default()
                    }
                })
                .into(),
        };

        let base: Element<'_, Message> = container(column![panes, hints, status_bar].spacing(6))
            .padding(8)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();

        if let Some(prompt) = &self.prompt {
            stack![base, view_modal(prompt)].into()
        } else {
            base
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let key_press = keyboard::on_key_press(|key, mods: Modifiers| {
            if let Key::Named(Named::Shift) = key {
                return Some(Message::ShiftChanged(true));
            }
            match key {
                Key::Character(ref c)
                    if mods.command() && mods.shift() && c.eq_ignore_ascii_case("p") =>
                {
                    Some(Message::OpenCommandPalette)
                }
                Key::Character(ref c) if mods.command() && c.eq_ignore_ascii_case("p") => {
                    Some(Message::OpenPrompt)
                }
                Key::Character(ref c) if mods.command() && c.as_str() == "," => {
                    Some(Message::OpenSettingsFile)
                }
                Key::Named(Named::Escape) => Some(Message::PromptCancel),
                Key::Named(Named::ArrowUp) => Some(Message::MoveSelection(-1, mods.shift())),
                Key::Named(Named::ArrowDown) => Some(Message::MoveSelection(1, mods.shift())),
                Key::Named(Named::PageUp) => Some(Message::PageMove(-1, mods.shift())),
                Key::Named(Named::PageDown) => Some(Message::PageMove(1, mods.shift())),
                // ←/→ are only meaningful inside the Delete modal; in main
                // view the SwitchPromptFocus handler is a no-op.
                Key::Named(Named::ArrowLeft) | Key::Named(Named::ArrowRight) => {
                    Some(Message::SwitchPromptFocus)
                }
                Key::Named(Named::Enter) => Some(Message::ActivateSelection),
                Key::Named(Named::Tab) => Some(Message::SwitchSide),
                Key::Named(Named::Backspace) => Some(Message::GoUpActive),
                Key::Named(Named::Delete) => Some(Message::OpenDeletePrompt),
                Key::Named(Named::F4) => Some(Message::OpenFile),
                Key::Named(Named::F5) => Some(Message::OpenCopyPrompt),
                // Plain character keys (no Ctrl/Cmd/Alt) feed the type-to-filter.
                // The earlier Cmd+P / Cmd+, guards have already matched before
                // we get here, so unmodified characters fall through to this.
                Key::Character(c) if !mods.command() && !mods.control() && !mods.alt() => {
                    Some(Message::FilterAppend(c.to_string()))
                }
                _ => None,
            }
        });

        let key_release = keyboard::on_key_release(|key, _mods| match key {
            Key::Named(Named::Shift) => Some(Message::ShiftChanged(false)),
            _ => None,
        });

        let resizes = window::resize_events().map(|(_, size)| Message::Resized(size));

        let settings_poll = time::every(Duration::from_secs(1)).map(|_| Message::CheckSettings);

        let mut subs = vec![key_press, key_release, resizes, settings_poll];

        // Watch folders are read once at startup (same lifecycle as window
        // size). Filter out paths that don't exist so notify::watch() doesn't
        // log a misleading error.
        let watched: Vec<PathBuf> = self
            .config
            .watch_folders
            .iter()
            .map(|s| PathBuf::from(expand_tilde(s)))
            .filter(|p| p.is_dir())
            .collect();
        if !watched.is_empty() {
            subs.push(file_watch_subscription(watched));
        }

        Subscription::batch(subs)
    }
}

// ---------------------------------------------------------------------------
// Async tasks
// ---------------------------------------------------------------------------

/// Both side-loads (directory entries + git info) for a single pane.
fn loading_tasks(side: Side, path: PathBuf, generation: u64) -> Task<Message> {
    Task::batch([
        load_dir_task(side, path.clone(), generation),
        git_info_task(side, path, generation),
    ])
}

/// Read `path` in a blocking thread and stream batches of `Entry` back to the
/// app as `EntriesChunk` messages, followed by a final `EntriesDone`. The
/// `generation` tag lets the receiver discard chunks from a load that's been
/// superseded by a later navigation.
fn load_dir_task(side: Side, path: PathBuf, generation: u64) -> Task<Message> {
    use iced::futures::stream::{self, StreamExt};

    const CHUNK_SIZE: usize = 64;

    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<Entry>>(8);
    let path_for_io = path.clone();

    tokio::task::spawn_blocking(move || {
        let iter = match std::fs::read_dir(&path_for_io) {
            Ok(it) => it,
            Err(_) => return,
        };
        let mut batch: Vec<Entry> = Vec::with_capacity(CHUNK_SIZE);
        for entry in iter.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let metadata = entry.metadata().ok();
            let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = metadata
                .as_ref()
                .and_then(|m| if m.is_file() { Some(m.len()) } else { None });
            let modified = metadata.as_ref().and_then(|m| m.modified().ok());
            batch.push(Entry {
                name,
                is_dir,
                size,
                modified,
            });
            if batch.len() >= CHUNK_SIZE {
                let chunk = std::mem::replace(&mut batch, Vec::with_capacity(CHUNK_SIZE));
                // If the receiver was dropped (e.g. a newer load superseded
                // this one), bail out — no point reading the rest.
                if tx.blocking_send(chunk).is_err() {
                    return;
                }
            }
        }
        if !batch.is_empty() {
            let _ = tx.blocking_send(batch);
        }
        // Sender dropped here; receiver gets None and the stream terminates.
    });

    let chunks = stream::unfold(rx, move |mut rx| async move {
        rx.recv()
            .await
            .map(|chunk| (Message::EntriesChunk(side, generation, chunk), rx))
    });
    let done = stream::once(async move { Message::EntriesDone(side, generation) });

    Task::stream(chunks.chain(done))
}

/// Subscription that watches `folders` (non-recursively) for newly-created or
/// renamed-in files and emits a `NewFilesDetected` message per folder, with a
/// short quiet-window so a burst (e.g. unpacking an archive) is coalesced into
/// a single modal.
fn file_watch_subscription(folders: Vec<PathBuf>) -> Subscription<Message> {
    use iced::futures::SinkExt;
    use iced::stream;
    use notify::{
        event::ModifyKind, recommended_watcher, Event, EventKind, RecursiveMode, Watcher,
    };
    use std::collections::HashMap;
    use std::time::Instant;

    Subscription::run_with_id(
        "file-watcher",
        stream::channel(64, move |mut output| async move {
            let (raw_tx, mut raw_rx) = tokio::sync::mpsc::channel::<Event>(256);

            // notify's callback runs on its own thread (not a tokio worker),
            // so blocking_send is the right way to hand events back to us.
            let mut watcher = match recommended_watcher(move |res| {
                if let Ok(event) = res {
                    let _ = raw_tx.blocking_send(event);
                }
            }) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("file watcher: init failed: {}", e);
                    return;
                }
            };

            for folder in &folders {
                if let Err(e) = watcher.watch(folder, RecursiveMode::NonRecursive) {
                    eprintln!("file watcher: skipping {}: {}", folder.display(), e);
                }
            }

            // Per-folder accumulator. `deadline` is the time at which we
            // flush. Any incoming event pushes the deadline out by `quiet`
            // so a burst of fast-arriving events fires one modal at the end.
            let mut pending: HashMap<PathBuf, Vec<String>> = HashMap::new();
            let mut deadline: Option<Instant> = None;
            let quiet = Duration::from_millis(500);
            let idle_timeout = Duration::from_secs(3600);

            loop {
                let wait = deadline
                    .map(|d| d.saturating_duration_since(Instant::now()))
                    .unwrap_or(idle_timeout);

                tokio::select! {
                    maybe_evt = raw_rx.recv() => {
                        let Some(event) = maybe_evt else { break };
                        let relevant = matches!(
                            event.kind,
                            EventKind::Create(_)
                                | EventKind::Modify(ModifyKind::Name(_))
                        );
                        if !relevant {
                            continue;
                        }
                        for path in event.paths {
                            let Some(name) = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                            else {
                                continue;
                            };
                            if is_ignored_watch_filename(&name) {
                                continue;
                            }
                            let Some(parent) = path.parent().map(PathBuf::from)
                            else {
                                continue;
                            };
                            // Drop events for paths outside our exact watch
                            // set (some backends report adjacent paths).
                            if !folders.iter().any(|f| f == &parent) {
                                continue;
                            }
                            // Skip directories and stale events whose target
                            // is already gone.
                            if !path.is_file() {
                                continue;
                            }
                            let bucket = pending.entry(parent).or_default();
                            if !bucket.iter().any(|n| n == &name) {
                                bucket.push(name);
                            }
                        }
                        if !pending.is_empty() {
                            deadline = Some(Instant::now() + quiet);
                        }
                    }
                    _ = tokio::time::sleep(wait), if deadline.is_some() => {
                        for (folder, files) in pending.drain() {
                            let _ = output
                                .send(Message::NewFilesDetected(folder, files))
                                .await;
                        }
                        deadline = None;
                    }
                }
            }

            // Keep the watcher alive until the stream is dropped.
            drop(watcher);
        }),
    )
}

/// Filenames the watcher should treat as noise: in-progress downloads (Chrome,
/// Firefox, browsers' generic temps) and hidden files (`.DS_Store` etc).
fn is_ignored_watch_filename(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    let ext = name
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "crdownload" | "part" | "download" | "tmp")
}

fn copy_task(srcs: Vec<PathBuf>, dest_dir: PathBuf) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                srcs.into_iter()
                    .map(|src| {
                        let name = match src.file_name() {
                            Some(n) => n.to_owned(),
                            None => {
                                return (src.clone(), Err("source has no file name".to_string()))
                            }
                        };
                        let target = dest_dir.join(name);
                        let res = copy_recursive(&src, &target).map_err(|e| e.to_string());
                        (src, res)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default()
        },
        Message::CopyFinished,
    )
}

fn delete_task(paths: Vec<PathBuf>) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                paths
                    .into_iter()
                    .map(|path| {
                        let res = delete_path(&path).map_err(|e| e.to_string());
                        (path, res)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default()
        },
        Message::DeleteFinished,
    )
}

fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = std::fs::metadata(src)?;
    if meta.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let entry_path = entry.path();
            let entry_name = entry.file_name();
            copy_recursive(&entry_path, &dst.join(entry_name))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

fn delete_path(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Probe the directory for git status. Returns None when `path` isn't inside
/// a git repository (or when `git` is missing from PATH).
fn git_info_task(side: Side, path: PathBuf, generation: u64) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || gather_git_info(&path))
                .await
                .unwrap_or(None)
        },
        move |info| Message::GitInfoLoaded(side, generation, info),
    )
}

fn gather_git_info(path: &Path) -> Option<GitInfo> {
    // First call doubles as the "are we in a repo?" probe — `git branch
    // --show-current` returns a non-zero status outside repos and returns an
    // empty stdout when HEAD is detached.
    let branch_out = run_git(path, &["branch", "--show-current"])?;
    let branch = if branch_out.trim().is_empty() {
        "(detached)".to_string()
    } else {
        branch_out.trim().to_string()
    };

    // `--no-renames` keeps each line in the simple `XY path` shape so we don't
    // have to deal with the `orig -> new` rename syntax when extracting names.
    let status = run_git(path, &["status", "--porcelain", "--no-renames"]).unwrap_or_default();
    let mut uncommitted = 0;
    let mut modified_names: HashSet<String> = HashSet::new();
    for line in status.lines() {
        if line.is_empty() {
            continue;
        }
        uncommitted += 1;
        // Porcelain v1: two status chars, one space, then the path.
        if line.len() < 4 {
            continue;
        }
        let raw = &line[3..];
        // Git quotes paths containing unusual chars; the inner string is good
        // enough for our prefix-segment match.
        let unquoted = raw
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(raw);
        // Entries outside the current pane (deeper-repo paths surface as
        // `../foo`) don't get a marker in this directory.
        if unquoted.starts_with("../") || unquoted == ".." {
            continue;
        }
        if let Some(first_seg) = unquoted.split('/').next() {
            if !first_seg.is_empty() {
                modified_names.insert(first_seg.to_string());
            }
        }
    }

    // Ahead/behind requires an upstream — fall back to (0, 0) if it isn't set.
    let (ahead, behind) = run_git(
        path,
        &["rev-list", "--count", "--left-right", "HEAD...@{u}"],
    )
    .and_then(|s| {
        let mut parts = s.split_whitespace();
        let a: usize = parts.next()?.parse().ok()?;
        let b: usize = parts.next()?.parse().ok()?;
        Some((a, b))
    })
    .unwrap_or((0, 0));

    Some(GitInfo {
        branch,
        uncommitted,
        ahead,
        behind,
        modified_names,
    })
}

fn run_git(path: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// View helpers
// ---------------------------------------------------------------------------

fn view_pane<'a>(
    side: Side,
    pane: &'a Pane,
    active: bool,
    name_max_chars: usize,
    config: &Config,
    colors: RowColors,
    window_height: f32,
) -> Element<'a, Message> {
    let path_header = text(pane.path.display().to_string())
        .font(Font::MONOSPACE)
        .size(config.row_font_size);

    // Match the body row's leading git-marker gutter so the "Name" header
    // sits over the actual names rather than the marker column.
    let name_header = row![
        Space::with_width(Length::Fixed(config.row_font_size as f32)),
        header_cell(
            side,
            "Name",
            SortBy::Name,
            pane,
            Length::Fill,
            Horizontal::Left,
            config
        ),
    ]
    .spacing(0)
    .width(Length::Fill);

    let column_header = container(
        row![
            name_header,
            header_cell(
                side,
                "Size",
                SortBy::Size,
                pane,
                Length::Fixed(config.size_column_px),
                Horizontal::Right,
                config,
            ),
            header_cell(
                side,
                "Modified",
                SortBy::Modified,
                pane,
                Length::Fixed(config.modified_column_px),
                Horizontal::Left,
                config,
            ),
        ]
        .spacing(8),
    )
    .padding(Padding::from([0, 8]))
    .style(|theme: &Theme| container::Style {
        background: Some(theme.extended_palette().background.weak.color.into()),
        ..Default::default()
    });

    let pad_y = config.row_padding_y();
    let stride = config.row_height_px;
    let fallback_vh = viewport_height_estimate(window_height);
    let vh = pane.viewport_height.unwrap_or(fallback_vh);

    // Total list rows: the synthetic ".." row plus all visible entries.
    let total_rows = 1 + pane.visible_indices.len();

    // Virtual rendering: only build widgets for rows inside (or near) the
    // current viewport. `Space` spacers above and below fill the remaining
    // height so the scrollable's total content size — and therefore its
    // scrollbar position — stays accurate regardless of how many entries
    // exist. This keeps view() O(visible rows) instead of O(total entries),
    // which is the main reason large directories stall key-press handling.
    let first_row = (pane.scroll_y / stride).floor() as usize;
    // +2 overdraw: ensures partial rows at the top and bottom edge of the
    // viewport are never blank while the user scrolls.
    let last_row = ((pane.scroll_y + vh) / stride).ceil() as usize + 2;
    let first_row = first_row.min(total_rows);
    let last_row = last_row.min(total_rows);

    let top_space = first_row as f32 * stride;
    let bottom_space = total_rows.saturating_sub(last_row) as f32 * stride;

    // ".." row — treated as a directory for color purposes, never dimmed.
    let up_visual = row_visual(pane, 0);
    let up_name_color = folder_name_color(true, up_visual, active, colors);
    let up = build_row(
        0,
        "..".to_string(),
        String::new(),
        String::new(),
        Message::RowClicked(side, 0),
        up_visual,
        active,
        config,
        colors,
        pad_y,
        up_name_color,
        false,
        false,
    );

    let mut list = column![].spacing(0).push(Space::with_height(top_space));
    if first_row == 0 {
        list = list.push(up);
    }
    for row_idx in first_row.max(1)..last_row {
        let visible_pos = row_idx - 1;
        let &entry_idx = match pane.visible_indices.get(visible_pos) {
            Some(i) => i,
            None => break,
        };
        let entry = match pane.entries.get(entry_idx) {
            Some(e) => e,
            None => continue,
        };
        let raw_name = if entry.is_dir {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
        };
        let name = truncate_with_ellipsis(&raw_name, name_max_chars);
        let size_str = entry.size.map(format_size).unwrap_or_default();
        let mod_str = entry.modified.map(format_modified).unwrap_or_default();
        let visual = row_visual(pane, row_idx);
        let name_color = folder_name_color(entry.is_dir, visual, active, colors);
        let in_selection = active && matches!(visual, RowVisual::Cursor | RowVisual::Marked);
        let dim_row = entry.name.starts_with('.') && !in_selection;
        let git_modified = pane
            .git_info
            .as_ref()
            .map(|i| i.modified_names.contains(&entry.name))
            .unwrap_or(false);
        list = list.push(build_row(
            row_idx,
            name,
            size_str,
            mod_str,
            Message::RowClicked(side, row_idx),
            visual,
            active,
            config,
            colors,
            pad_y,
            name_color,
            dim_row,
            git_modified,
        ));
    }
    list = list.push(Space::with_height(bottom_space));

    // Show a placeholder only when we have nothing to display yet. Once the
    // first streamed chunk arrives we switch to the scrollable so the user
    // sees entries appear progressively even if more are still loading.
    let body: Element<'a, Message> = if pane.loading && pane.entries.is_empty() {
        container(text("Loading…").size(config.row_font_size))
            .padding(16)
            .width(Length::Fill)
            .into()
    } else {
        scrollable(list)
            .id(scroll_id(side))
            .height(Length::Fill)
            .on_scroll(move |viewport| {
                Message::Scrolled(side, viewport.absolute_offset().y, viewport.bounds().height)
            })
            .into()
    };

    let mut inner_col = column![path_header, column_header, body].spacing(4);
    if !pane.filter.is_empty() {
        inner_col = inner_col.push(filter_bar(pane, config));
    }
    if let Some(info) = &pane.git_info {
        inner_col = inner_col.push(git_info_bar(info, config));
    }
    let inner = container(inner_col).padding(6);

    let bordered = container(inner)
        .style(move |theme: &Theme| {
            let palette = theme.extended_palette();
            let border_color = if active {
                palette.primary.strong.color
            } else {
                palette.background.strong.color
            };
            container::Style {
                border: Border {
                    color: border_color,
                    width: if active { 2.0 } else { 1.0 },
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        })
        .width(Length::FillPortion(1))
        .height(Length::Fill);

    mouse_area(bordered)
        .on_press(Message::Activate(side))
        .into()
}

fn git_info_bar<'a>(info: &GitInfo, config: &Config) -> Element<'a, Message> {
    let mut parts: Vec<String> = vec![format!("branch: {}", info.branch)];
    if info.uncommitted > 0 {
        let noun = if info.uncommitted == 1 {
            "change"
        } else {
            "changes"
        };
        parts.push(format!("{} uncommitted {}", info.uncommitted, noun));
    }
    if info.ahead > 0 || info.behind > 0 {
        parts.push(format!("↑{} ↓{} vs upstream", info.ahead, info.behind));
    } else {
        // Best-effort hint when there's no divergence — but also no upstream
        // configured, you'll see exactly "↑0 ↓0" suppressed (this line stays
        // empty). Skipping it keeps the bar quieter for "clean" repos.
    }
    let label = parts.join("   ·   ");

    container(
        text(label)
            .font(Font::MONOSPACE)
            .size(config.header_font_size),
    )
    .padding(Padding::from([2, 8]))
    .width(Length::Fill)
    .style(|theme: &Theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(palette.success.weak.color.into()),
            text_color: Some(palette.success.weak.text),
            ..Default::default()
        }
    })
    .into()
}

fn filter_bar<'a>(pane: &Pane, config: &Config) -> Element<'a, Message> {
    let label = format!(
        "filter: /{}/   ({} of {})",
        pane.filter,
        pane.visible_indices.len(),
        pane.entries.len()
    );
    container(
        text(label)
            .font(Font::MONOSPACE)
            .size(config.header_font_size),
    )
    .padding(Padding::from([4, 8]))
    .width(Length::Fill)
    .style(|theme: &Theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(palette.primary.weak.color.into()),
            text_color: Some(palette.primary.weak.text),
            ..Default::default()
        }
    })
    .into()
}

/// Folder color applies only when the row is *not* part of the active pane's
/// selection — otherwise the cursor/mark text color would clash with it.
fn folder_name_color(
    is_dir: bool,
    visual: RowVisual,
    pane_active: bool,
    colors: RowColors,
) -> Option<Color> {
    let in_selection = pane_active && matches!(visual, RowVisual::Cursor | RowVisual::Marked);
    if !in_selection && is_dir {
        colors.folder
    } else {
        None
    }
}

fn row_visual(pane: &Pane, row_index: usize) -> RowVisual {
    if pane.selected == row_index {
        RowVisual::Cursor
    } else if pane.is_marked(row_index) {
        RowVisual::Marked
    } else {
        RowVisual::None
    }
}

fn header_cell<'a>(
    side: Side,
    label: &'a str,
    column: SortBy,
    pane: &Pane,
    width: Length,
    align: Horizontal,
    config: &Config,
) -> Element<'a, Message> {
    let arrow = if pane.sort_by == column {
        match pane.sort_dir {
            SortDir::Asc => " ↑",
            SortDir::Desc => " ↓",
        }
    } else {
        ""
    };
    let label_full = format!("{}{}", label, arrow);
    button(
        text(label_full)
            .size(config.header_font_size)
            .width(Length::Fill)
            .align_x(align),
    )
    .width(width)
    .padding(Padding::from([2, 4]))
    .style(button::text)
    .on_press(Message::ToggleSort(side, column))
    .into()
}

fn build_row<'a>(
    row_index: usize,
    name: String,
    size: String,
    modified: String,
    msg: Message,
    visual: RowVisual,
    pane_active: bool,
    config: &Config,
    colors: RowColors,
    pad_y: u16,
    name_color: Option<Color>,
    dim_row: bool,
    git_modified: bool,
) -> Element<'a, Message> {
    let font_size = config.row_font_size;
    // Fixed-width gutter so every row's name column aligns regardless of
    // whether a git marker is present.
    let git_marker_width = Length::Fixed(font_size as f32);
    let git_marker = text(if git_modified { "●" } else { "" })
        .font(Font::MONOSPACE)
        .size(font_size)
        .width(git_marker_width)
        .style(|_theme: &Theme| iced::widget::text::Style {
            // GitHub-ish "modified" amber, picked to stand out on both light
            // and dark themes without further tuning.
            color: Some(Color::from_rgb8(0xd2, 0x99, 0x22)),
        });
    let name_widget = text(name)
        .font(Font::MONOSPACE)
        .size(font_size)
        .width(Length::Fill)
        .style(move |theme: &Theme| {
            // Resolve the final name color:
            //   - explicit override (e.g. folder_color) wins over theme default
            //   - dim_row pulls whichever color we'd use toward the background
            let resolved = match (name_color, dim_row) {
                (Some(c), true) => Some(dim(c)),
                (Some(c), false) => Some(c),
                (None, true) => Some(dim(theme.extended_palette().background.base.text)),
                (None, false) => None,
            };
            iced::widget::text::Style { color: resolved }
        });

    // Keep marker and name visually adjacent — the outer row's 8px spacing
    // would otherwise push the name away from the marker.
    let name_cell = row![git_marker, name_widget].spacing(4).width(Length::Fill);

    let content = row![
        name_cell,
        text(size)
            .font(Font::MONOSPACE)
            .size(font_size)
            .width(Length::Fixed(config.size_column_px))
            .align_x(Horizontal::Right),
        text(modified)
            .font(Font::MONOSPACE)
            .size(font_size)
            .width(Length::Fixed(config.modified_column_px)),
    ]
    .spacing(8);

    button(content)
        .on_press(msg)
        .width(Length::Fill)
        .padding(Padding::from([pad_y, 8]))
        .style(move |theme: &Theme, status: button::Status| {
            compute_row_style(
                theme,
                status,
                visual,
                pane_active,
                row_index,
                colors,
                dim_row,
            )
        })
        .into()
}

fn compute_row_style(
    theme: &Theme,
    _status: button::Status,
    visual: RowVisual,
    pane_active: bool,
    row_index: usize,
    colors: RowColors,
    dim_row: bool,
) -> button::Style {
    let palette = theme.extended_palette();
    // dim_row affects the row's default text color (i.e. the Size and Modified
    // columns). The name widget handles its own dimming above so it can
    // combine with the folder-color override.
    let text_default = if dim_row {
        dim(palette.background.base.text)
    } else {
        palette.background.base.text
    };

    // Note: hover state intentionally ignored — rows shouldn't light up under
    // the cursor; the only emphasis is the active pane's selection.
    let (background, text_color) = match (visual, pane_active) {
        (RowVisual::Cursor, true) => {
            let pair = palette.primary.strong;
            let bg = colors.cursor.unwrap_or(pair.color);
            let fg = if colors.cursor.is_some() {
                Color::WHITE
            } else {
                pair.text
            };
            (Some(bg.into()), fg)
        }
        (RowVisual::Marked, true) => {
            let pair = palette.primary.weak;
            let bg = colors.mark.unwrap_or(pair.color);
            let fg = if colors.mark.is_some() {
                Color::WHITE
            } else {
                pair.text
            };
            (Some(bg.into()), fg)
        }
        _ => {
            if row_index % 2 == 1 {
                let stripe = colors.stripe.unwrap_or_else(|| {
                    blend(
                        palette.background.base.color,
                        palette.background.weak.color,
                        0.15,
                    )
                });
                (Some(stripe.into()), text_default)
            } else {
                (None, text_default)
            }
        }
    };

    button::Style {
        background,
        text_color,
        border: Border::default(),
        shadow: Shadow::default(),
    }
}

fn view_modal(prompt: &Prompt) -> Element<'_, Message> {
    let dialog_inner = container(match prompt {
        Prompt::Open { input } | Prompt::Copy { input } => {
            let (title, placeholder, hint) = match prompt {
                Prompt::Open { .. } => (
                    "Go to folder",
                    "/path/to/folder",
                    "Enter to open  ·  Esc or click outside to cancel",
                ),
                Prompt::Copy { .. } => (
                    "Copy selected to",
                    "/path/to/destination",
                    "Enter to copy  ·  Esc or click outside to cancel",
                ),
                Prompt::Delete { .. } | Prompt::NewFiles { .. } | Prompt::CommandPalette { .. } => {
                    unreachable!()
                }
            };
            column![
                text(title).size(15),
                text_input(placeholder, input)
                    .id(text_input::Id::new(PROMPT_ID))
                    .on_input(Message::PromptChanged)
                    .on_submit(Message::PromptSubmit)
                    .padding(8),
                text(hint).size(11),
            ]
            .spacing(10)
        }
        Prompt::CommandPalette { selected } => {
            let actions_col = PaletteAction::ALL.iter().enumerate().fold(
                column![].spacing(4),
                |col, (i, action)| {
                    let style: fn(&Theme, button::Status) -> button::Style = if i == *selected {
                        button::primary
                    } else {
                        button::secondary
                    };
                    col.push(
                        button(text(action.label()))
                            .on_press(Message::PaletteSelect(*action))
                            .padding(Padding::from([8, 20]))
                            .width(Length::Fill)
                            .style(style),
                    )
                },
            );
            column![
                text("Command Palette").size(15),
                actions_col,
                text("↑/↓ or Tab navigate  ·  Enter activate  ·  Esc dismiss").size(11),
            ]
            .spacing(10)
        }
        Prompt::Delete { paths, focus } => {
            let question = if paths.len() == 1 {
                let name = paths[0]
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| paths[0].display().to_string());
                format!("Do you really want to delete \"{}\"?", name)
            } else {
                format!("Do you really want to delete {} files?", paths.len())
            };

            // Focused button gets a prominent style; unfocused stays subdued.
            // Same fn-pointer type, so the conditional resolves to a single
            // `fn(&Theme, button::Status) -> button::Style` for `.style()`.
            let cancel_style: fn(&Theme, button::Status) -> button::Style =
                if *focus == DeleteFocus::Cancel {
                    button::primary
                } else {
                    button::secondary
                };
            let confirm_style: fn(&Theme, button::Status) -> button::Style =
                if *focus == DeleteFocus::Confirm {
                    button::danger
                } else {
                    button::secondary
                };

            let actions = row![
                button(text("Cancel"))
                    .on_press(Message::PromptCancel)
                    .padding(Padding::from([6, 16]))
                    .style(cancel_style),
                button(text("Delete"))
                    .on_press(Message::PromptSubmit)
                    .padding(Padding::from([6, 16]))
                    .style(confirm_style),
            ]
            .spacing(10);
            column![
                text("Confirm delete").size(15),
                text(question),
                actions,
                text("Tab or ←/→ switch  ·  Enter activate  ·  Esc cancel").size(11),
            ]
            .spacing(10)
        }
        Prompt::NewFiles {
            folder,
            files,
            focus,
        } => {
            let focus = *focus;
            let primary: fn(&Theme, button::Status) -> button::Style = button::primary;
            let secondary: fn(&Theme, button::Status) -> button::Style = button::secondary;

            let header = if files.len() == 1 {
                format!("New file detected in {}:", folder.display())
            } else {
                format!(
                    "{} new files detected in {}:",
                    files.len(),
                    folder.display()
                )
            };

            // Cap the listed file count so the modal doesn't grow unbounded
            // when a big extraction lands.
            const MAX_LISTED: usize = 8;
            let mut listing = String::new();
            for name in files.iter().take(MAX_LISTED) {
                listing.push_str("  • ");
                listing.push_str(name);
                listing.push('\n');
            }
            if files.len() > MAX_LISTED {
                listing.push_str(&format!("  …and {} more\n", files.len() - MAX_LISTED));
            }
            let listing = listing.trim_end().to_string();

            let actions = row![
                button(text("No"))
                    .on_press(Message::PromptCancel)
                    .padding(Padding::from([6, 16]))
                    .style(if focus == NewFilesFocus::No {
                        primary
                    } else {
                        secondary
                    }),
                button(text("Left pane"))
                    .on_press(Message::NewFilesPickSide(Side::Left))
                    .padding(Padding::from([6, 16]))
                    .style(if focus == NewFilesFocus::Left {
                        primary
                    } else {
                        secondary
                    }),
                button(text("Right pane"))
                    .on_press(Message::NewFilesPickSide(Side::Right))
                    .padding(Padding::from([6, 16]))
                    .style(if focus == NewFilesFocus::Right {
                        primary
                    } else {
                        secondary
                    }),
            ]
            .spacing(10);

            column![
                text("New files detected").size(15),
                text(header),
                text(listing).font(Font::MONOSPACE).size(12),
                text("Would you like to switch a pane to this folder?"),
                actions,
                text("Tab cycles focus  ·  Enter activates  ·  Esc dismisses").size(11),
            ]
            .spacing(10)
        }
    })
    .padding(16)
    .width(Length::Fixed(440.0))
    .style(modal_style);

    let dialog = mouse_area(dialog_inner).on_press(Message::NoOp);

    let backdrop = mouse_area(
        container(text(""))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(backdrop_style),
    )
    .on_press(Message::PromptCancel);

    let centered = container(dialog)
        .center_x(Length::Fill)
        .center_y(Length::Fill);

    stack![backdrop, centered].into()
}

fn modal_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.base.color.into()),
        border: Border {
            color: palette.background.strong.color,
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: Vector::new(0.0, 6.0),
            blur_radius: 24.0,
        },
        ..Default::default()
    }
}

fn backdrop_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.45).into()),
        ..Default::default()
    }
}

fn scroll_id(side: Side) -> scrollable::Id {
    match side {
        Side::Left => scrollable::Id::new("scroll-left"),
        Side::Right => scrollable::Id::new("scroll-right"),
    }
}

fn name_max_chars(window_width: f32, config: &Config) -> usize {
    let outer_padding = 8.0 * 2.0;
    let gap_between_panes = 8.0;
    let pane_width = (window_width - outer_padding - gap_between_panes) / 2.0;

    let pane_border_padding = 6.0 * 2.0 + 2.0;
    let row_horizontal_padding = 8.0 * 2.0;
    let row_gaps = 8.0 * 2.0;
    let scrollbar = 16.0;

    let name_px = pane_width
        - pane_border_padding
        - row_horizontal_padding
        - row_gaps
        - config.size_column_px
        - config.modified_column_px
        - scrollbar;

    ((name_px / config.mono_glyph_px).max(8.0)) as usize
}

fn viewport_height_estimate(window_height: f32) -> f32 {
    (window_height - 110.0).max(100.0)
}

fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    let len = s.chars().count();
    if len <= max_chars {
        s.to_string()
    } else if max_chars == 0 {
        String::new()
    } else {
        let mut out: String = s.chars().take(max_chars - 1).collect();
        out.push('…');
        out
    }
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes < KB {
        format!("{} B", bytes)
    } else if bytes < MB {
        format!("{} KB", bytes / KB)
    } else if bytes < GB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    }
}

fn format_modified(t: SystemTime) -> String {
    let dt: DateTime<Local> = t.into();
    dt.format("%-m/%-d/%y %-I:%M %p").to_string()
}

fn home_dir() -> PathBuf {
    #[allow(deprecated)]
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn expand_tilde(p: &str) -> String {
    if let Some(rest) = p.strip_prefix('~') {
        return format!("{}{}", home_dir().display(), rest);
    }
    p.to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    // --- parse_hex_color ---

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

    // --- blend ---

    #[test]
    fn blend_at_zero_returns_a() {
        let a = Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let b = Color {
            r: 0.0,
            g: 1.0,
            b: 0.0,
            a: 1.0,
        };
        let result = blend(a, b, 0.0);
        assert!(approx_eq(result.r, 1.0));
        assert!(approx_eq(result.g, 0.0));
    }

    #[test]
    fn blend_at_one_returns_b() {
        let a = Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let b = Color {
            r: 0.0,
            g: 1.0,
            b: 0.0,
            a: 1.0,
        };
        let result = blend(a, b, 1.0);
        assert!(approx_eq(result.r, 0.0));
        assert!(approx_eq(result.g, 1.0));
    }

    #[test]
    fn blend_at_midpoint() {
        let a = Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let b = Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        let result = blend(a, b, 0.5);
        assert!(approx_eq(result.r, 0.5));
        assert!(approx_eq(result.g, 0.5));
        assert!(approx_eq(result.b, 0.5));
    }

    // --- dim ---

    #[test]
    fn dim_scales_each_channel() {
        let c = Color {
            r: 1.0,
            g: 0.8,
            b: 0.4,
            a: 1.0,
        };
        let d = dim(c);
        assert!(approx_eq(d.r, 0.55));
        assert!(approx_eq(d.g, 0.8 * 0.55));
        assert!(approx_eq(d.b, 0.4 * 0.55));
        // Alpha is unchanged.
        assert!(approx_eq(d.a, 1.0));
    }

    // --- format_size ---

    #[test]
    fn format_size_zero() {
        assert_eq!(format_size(0), "0 B");
    }

    #[test]
    fn format_size_under_kb() {
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn format_size_exact_kb() {
        assert_eq!(format_size(1024), "1 KB");
    }

    #[test]
    fn format_size_kb_range() {
        assert_eq!(format_size(2048), "2 KB");
    }

    #[test]
    fn format_size_exact_mb() {
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn format_size_mb_fractional() {
        // 1.5 MB → "1.5 MB"
        assert_eq!(format_size(1024 * 1024 + 512 * 1024), "1.5 MB");
    }

    #[test]
    fn format_size_exact_gb() {
        assert_eq!(format_size(1024u64.pow(3)), "1.0 GB");
    }

    // --- truncate_with_ellipsis ---

    #[test]
    fn truncate_shorter_than_max_returns_input() {
        assert_eq!(truncate_with_ellipsis("hi", 10), "hi");
    }

    #[test]
    fn truncate_exactly_at_max_returns_input() {
        assert_eq!(truncate_with_ellipsis("hello", 5), "hello");
    }

    #[test]
    fn truncate_over_max_adds_ellipsis() {
        // max_chars=5 → 4 chars + '…' = 5 chars total.
        assert_eq!(truncate_with_ellipsis("hello world", 5), "hell…");
    }

    #[test]
    fn truncate_max_zero_returns_empty() {
        assert_eq!(truncate_with_ellipsis("anything", 0), "");
    }

    #[test]
    fn truncate_unicode_safe() {
        // The 'é' is a single char but multiple bytes; truncation should work
        // on chars, not bytes.
        let s = "café-society";
        let out = truncate_with_ellipsis(s, 5);
        assert_eq!(out.chars().count(), 5);
    }

    // --- expand_tilde ---

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

    // --- enums ---

    #[test]
    fn sort_dir_toggle_round_trips() {
        assert_eq!(SortDir::Asc.toggled(), SortDir::Desc);
        assert_eq!(SortDir::Desc.toggled(), SortDir::Asc);
        assert_eq!(SortDir::Asc.toggled().toggled(), SortDir::Asc);
    }

    #[test]
    fn side_other_flips() {
        assert_eq!(Side::Left.other(), Side::Right);
        assert_eq!(Side::Right.other(), Side::Left);
        assert_eq!(Side::Left.other().other(), Side::Left);
    }

    #[test]
    fn delete_focus_toggle() {
        assert_eq!(DeleteFocus::Cancel.toggle(), DeleteFocus::Confirm);
        assert_eq!(DeleteFocus::Confirm.toggle(), DeleteFocus::Cancel);
    }

    #[test]
    fn default_active_side_is_left() {
        assert_eq!(default_active_side(), Side::Left);
    }

    // --- Config ---

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

    // --- SavedState ---

    #[test]
    fn saved_state_roundtrips_through_yaml() {
        let state = SavedState {
            left: PathBuf::from("/tmp/left"),
            right: PathBuf::from("/tmp/right"),
            active: Side::Right,
        };
        let yaml = serde_yaml::to_string(&state).unwrap();
        let parsed: SavedState = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.left, state.left);
        assert_eq!(parsed.right, state.right);
        assert_eq!(parsed.active, state.active);
    }

    #[test]
    fn saved_state_active_field_defaults_when_missing() {
        // Older state files (or hand-edited ones) may omit `active`. The
        // serde default should fill in Side::Left rather than failing.
        let yaml = "left: /a\nright: /b\n";
        let parsed: SavedState = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.left, PathBuf::from("/a"));
        assert_eq!(parsed.right, PathBuf::from("/b"));
        assert_eq!(parsed.active, Side::Left);
    }

    #[test]
    fn saved_state_yaml_uses_lowercase_side() {
        let state = SavedState {
            left: PathBuf::from("/x"),
            right: PathBuf::from("/y"),
            active: Side::Right,
        };
        let yaml = serde_yaml::to_string(&state).unwrap();
        // serde(rename_all = "lowercase") should produce "right" not "Right".
        assert!(yaml.contains("active: right"));
    }

    // --- sort_entries ---

    fn mk_entry(name: &str, is_dir: bool, size: Option<u64>) -> Entry {
        Entry {
            name: name.to_string(),
            is_dir,
            size,
            modified: None,
        }
    }

    #[test]
    fn sort_clusters_directories_first() {
        let mut entries = vec![
            mk_entry("zfile", false, Some(10)),
            mk_entry("adir", true, None),
            mk_entry("mfile", false, Some(20)),
            mk_entry("bdir", true, None),
        ];
        sort_entries(&mut entries, SortBy::Name, SortDir::Asc);
        assert!(entries[0].is_dir);
        assert!(entries[1].is_dir);
        assert!(!entries[2].is_dir);
        assert!(!entries[3].is_dir);
    }

    #[test]
    fn sort_by_name_is_case_insensitive() {
        let mut entries = vec![
            mk_entry("Banana", false, None),
            mk_entry("apple", false, None),
            mk_entry("cherry", false, None),
        ];
        sort_entries(&mut entries, SortBy::Name, SortDir::Asc);
        assert_eq!(entries[0].name, "apple");
        assert_eq!(entries[1].name, "Banana");
        assert_eq!(entries[2].name, "cherry");
    }

    #[test]
    fn sort_by_name_desc_reverses() {
        let mut entries = vec![
            mk_entry("a", false, None),
            mk_entry("b", false, None),
            mk_entry("c", false, None),
        ];
        sort_entries(&mut entries, SortBy::Name, SortDir::Desc);
        assert_eq!(entries[0].name, "c");
        assert_eq!(entries[2].name, "a");
    }

    #[test]
    fn sort_by_size_ascending() {
        let mut entries = vec![
            mk_entry("c", false, Some(300)),
            mk_entry("a", false, Some(100)),
            mk_entry("b", false, Some(200)),
        ];
        sort_entries(&mut entries, SortBy::Size, SortDir::Asc);
        assert_eq!(entries[0].size, Some(100));
        assert_eq!(entries[1].size, Some(200));
        assert_eq!(entries[2].size, Some(300));
    }

    #[test]
    fn sort_by_size_treats_none_as_zero() {
        let mut entries = vec![
            mk_entry("a", false, Some(50)),
            mk_entry("b", false, None),
            mk_entry("c", false, Some(10)),
        ];
        sort_entries(&mut entries, SortBy::Size, SortDir::Asc);
        // None (treated as 0) sorts first.
        assert_eq!(entries[0].size, None);
        assert_eq!(entries[1].size, Some(10));
        assert_eq!(entries[2].size, Some(50));
    }

    #[test]
    fn sort_by_modified_handles_some_and_none() {
        use std::time::{Duration, UNIX_EPOCH};
        let t1 = UNIX_EPOCH + Duration::from_secs(100);
        let t2 = UNIX_EPOCH + Duration::from_secs(200);
        let mut entries = vec![
            Entry {
                name: "a".into(),
                is_dir: false,
                size: None,
                modified: Some(t2),
            },
            Entry {
                name: "b".into(),
                is_dir: false,
                size: None,
                modified: None,
            },
            Entry {
                name: "c".into(),
                is_dir: false,
                size: None,
                modified: Some(t1),
            },
        ];
        sort_entries(&mut entries, SortBy::Modified, SortDir::Asc);
        // Option<SystemTime>: None < Some(_), so the no-mtime entry sorts first.
        assert_eq!(entries[0].name, "b");
        assert_eq!(entries[1].name, "c");
        assert_eq!(entries[2].name, "a");
    }

    #[test]
    fn sort_keeps_dirs_first_even_when_sorting_by_size() {
        let mut entries = vec![
            mk_entry("file_big", false, Some(1000)),
            mk_entry("dir", true, None),
            mk_entry("file_small", false, Some(10)),
        ];
        sort_entries(&mut entries, SortBy::Size, SortDir::Asc);
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].size, Some(10));
        assert_eq!(entries[2].size, Some(1000));
    }

    // --- Pane init + navigate ---

    /// Helper to construct a Pane preloaded with entries, bypassing the
    /// streaming load path so tests don't depend on the filesystem.
    fn pane_with_entries(path: &str, entries: Vec<Entry>) -> Pane {
        let mut pane = Pane::empty(PathBuf::from(path));
        pane.append_chunk(entries);
        // Simulate EntriesDone: sort once, then rebuild visible indices.
        // append_chunk itself no longer sorts (deferred to EntriesDone so
        // the main thread isn't doing O(n² log n) work during streaming).
        let preserve = pane.cursor_name();
        sort_entries(&mut pane.entries, pane.sort_by, pane.sort_dir);
        pane.recompute_visible(preserve.as_deref());
        pane.loading = false;
        pane
    }

    #[test]
    fn pane_empty_starts_loading_with_no_entries() {
        let p = Pane::empty(PathBuf::from("/tmp"));
        assert_eq!(p.path, PathBuf::from("/tmp"));
        assert!(p.entries.is_empty());
        assert!(p.visible_indices.is_empty());
        assert!(p.filter.is_empty());
        assert_eq!(p.selected, 0);
        assert_eq!(p.anchor, 0);
        assert_eq!(p.sort_by, SortBy::Name);
        assert_eq!(p.sort_dir, SortDir::Asc);
        assert!(p.loading);
        assert_eq!(p.load_generation, 0);
        assert!(p.git_info.is_none());
    }

    #[test]
    fn navigate_bumps_generation_and_resets_state() {
        let mut p = pane_with_entries(
            "/old",
            vec![mk_entry("a", false, Some(1)), mk_entry("b", false, Some(2))],
        );
        p.selected = 2;
        p.anchor = 1;
        p.filter = "x".to_string();
        p.git_info = Some(GitInfo {
            branch: "main".into(),
            uncommitted: 1,
            ahead: 0,
            behind: 0,
            modified_names: HashSet::new(),
        });
        let gen_before = p.load_generation;

        let gen_after = p.navigate(PathBuf::from("/new"));

        assert_eq!(gen_after, gen_before + 1);
        assert_eq!(p.load_generation, gen_after);
        assert_eq!(p.path, PathBuf::from("/new"));
        assert!(p.entries.is_empty());
        assert!(p.visible_indices.is_empty());
        assert!(p.filter.is_empty());
        assert_eq!(p.selected, 0);
        assert_eq!(p.anchor, 0);
        assert!(p.loading);
        assert!(p.git_info.is_none());
    }

    #[test]
    fn navigate_generation_strictly_increases() {
        let mut p = Pane::empty(PathBuf::from("/"));
        let g0 = p.load_generation;
        let g1 = p.navigate(PathBuf::from("/a"));
        let g2 = p.navigate(PathBuf::from("/b"));
        let g3 = p.navigate(PathBuf::from("/c"));
        assert!(g1 > g0);
        assert!(g2 > g1);
        assert!(g3 > g2);
    }

    // --- Pane selection mechanics ---

    fn three_entry_pane() -> Pane {
        pane_with_entries(
            "/p",
            vec![
                mk_entry("a", false, Some(1)),
                mk_entry("b", false, Some(2)),
                mk_entry("c", false, Some(3)),
            ],
        )
    }

    #[test]
    fn move_to_clamps_at_top() {
        let mut p = three_entry_pane();
        p.move_to(-5, false);
        assert_eq!(p.selected, 0);
        assert_eq!(p.anchor, 0);
    }

    #[test]
    fn move_to_clamps_at_bottom() {
        // 3 entries → max row index is 3 (".." is row 0, entries 1..=3).
        let mut p = three_entry_pane();
        p.move_to(99, false);
        assert_eq!(p.selected, 3);
        assert_eq!(p.anchor, 3);
    }

    #[test]
    fn move_to_collapses_anchor_when_not_extending() {
        let mut p = three_entry_pane();
        p.selected = 1;
        p.anchor = 3;
        p.move_to(2, false);
        assert_eq!(p.selected, 2);
        assert_eq!(p.anchor, 2);
    }

    #[test]
    fn move_to_preserves_anchor_when_extending() {
        let mut p = three_entry_pane();
        p.selected = 1;
        p.anchor = 1;
        p.move_to(3, true);
        assert_eq!(p.selected, 3);
        // Anchor stays at the start of the range.
        assert_eq!(p.anchor, 1);
    }

    #[test]
    fn move_by_uses_relative_delta() {
        let mut p = three_entry_pane();
        p.selected = 1;
        p.anchor = 1;
        p.move_by(2, false);
        assert_eq!(p.selected, 3);
    }

    #[test]
    fn move_by_clamps_negative_below_zero() {
        let mut p = three_entry_pane();
        p.selected = 1;
        p.move_by(-100, false);
        assert_eq!(p.selected, 0);
    }

    // --- mark_range / is_marked ---

    #[test]
    fn mark_range_returns_low_high_regardless_of_order() {
        let mut p = three_entry_pane();
        p.anchor = 1;
        p.selected = 3;
        assert_eq!(p.mark_range(), (1, 3));

        p.anchor = 3;
        p.selected = 1;
        assert_eq!(p.mark_range(), (1, 3));
    }

    #[test]
    fn mark_range_collapses_to_point_when_equal() {
        let mut p = three_entry_pane();
        p.anchor = 2;
        p.selected = 2;
        assert_eq!(p.mark_range(), (2, 2));
    }

    #[test]
    fn is_marked_includes_both_endpoints() {
        let mut p = three_entry_pane();
        p.anchor = 1;
        p.selected = 3;
        assert!(p.is_marked(1));
        assert!(p.is_marked(2));
        assert!(p.is_marked(3));
        assert!(!p.is_marked(0));
    }

    #[test]
    fn is_marked_excludes_out_of_range_rows() {
        let mut p = three_entry_pane();
        p.anchor = 1;
        p.selected = 2;
        assert!(!p.is_marked(3));
    }

    // --- recompute_visible (filter) ---

    fn alpha_pane() -> Pane {
        pane_with_entries(
            "/p",
            vec![
                mk_entry("alpha", false, Some(1)),
                mk_entry("beta", false, Some(2)),
                mk_entry("gamma", false, Some(3)),
                mk_entry("alphabet", false, Some(4)),
            ],
        )
    }

    #[test]
    fn empty_filter_shows_all_entries() {
        let p = alpha_pane();
        assert!(p.filter.is_empty());
        assert_eq!(p.visible_indices.len(), p.entries.len());
    }

    #[test]
    fn substring_filter_matches() {
        let mut p = alpha_pane();
        p.filter = "alpha".to_string();
        p.recompute_visible(None);
        assert_eq!(p.visible_indices.len(), 2);
        let names: Vec<_> = p
            .visible_indices
            .iter()
            .map(|&i| p.entries[i].name.as_str())
            .collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"alphabet"));
    }

    #[test]
    fn filter_is_case_insensitive() {
        let mut p = alpha_pane();
        p.filter = "ALPHA".to_string();
        p.recompute_visible(None);
        assert_eq!(p.visible_indices.len(), 2);
    }

    #[test]
    fn invalid_regex_falls_back_to_substring() {
        let mut p = alpha_pane();
        // Unbalanced paren — invalid regex syntax.
        p.filter = "(alpha".to_string();
        p.recompute_visible(None);
        // Substring search for "(alpha" finds nothing in our entries.
        assert!(p.visible_indices.is_empty());

        // But "(a" likewise invalid regex, substring matches…well only literal
        // "(a" which is absent. Better: test fallback with a string that
        // *would* match as substring but is invalid regex: "alpha(" → bare
        // "(" makes the regex error, fallback looks for the literal substring
        // "alpha(" which is also absent. So instead use a clear case:
        p.filter = "[".to_string(); // invalid regex; substring "[" not in names.
        p.recompute_visible(None);
        assert!(p.visible_indices.is_empty());
    }

    #[test]
    fn cursor_preserved_by_name_when_still_visible() {
        let mut p = alpha_pane();
        // Cursor on "beta" (visible row 2, since alpha is sorted before beta).
        // The pane_with_entries helper calls append_chunk which sorts. After
        // sort: alpha, alphabet, beta, gamma → "beta" at visible row 3.
        p.selected = 3; // "beta"
        p.anchor = 3;
        assert_eq!(p.cursor_name().as_deref(), Some("beta"));

        // Apply a filter that still matches "beta".
        p.filter = "b".to_string();
        let prior = p.cursor_name();
        p.recompute_visible(prior.as_deref());

        // Cursor should still be on "beta".
        assert_eq!(p.cursor_name().as_deref(), Some("beta"));
    }

    #[test]
    fn cursor_resets_when_previously_focused_entry_filtered_out() {
        let mut p = alpha_pane();
        p.selected = 3; // "beta" after sorting
        p.anchor = 3;
        let prior = p.cursor_name();
        assert_eq!(prior.as_deref(), Some("beta"));

        // Filter excludes "beta".
        p.filter = "alpha".to_string();
        p.recompute_visible(prior.as_deref());

        // Cursor falls back to the ".." row.
        assert_eq!(p.selected, 0);
        assert_eq!(p.anchor, 0);
    }

    #[test]
    fn clearing_filter_restores_all_entries() {
        let mut p = alpha_pane();
        p.filter = "alpha".to_string();
        p.recompute_visible(None);
        assert_eq!(p.visible_indices.len(), 2);

        p.filter.clear();
        p.recompute_visible(None);
        assert_eq!(p.visible_indices.len(), 4);
    }

    // --- toggle_sort ---

    #[test]
    fn toggle_sort_same_column_flips_direction() {
        let mut p = alpha_pane();
        assert_eq!(p.sort_dir, SortDir::Asc);
        p.toggle_sort(SortBy::Name);
        assert_eq!(p.sort_by, SortBy::Name);
        assert_eq!(p.sort_dir, SortDir::Desc);
        p.toggle_sort(SortBy::Name);
        assert_eq!(p.sort_dir, SortDir::Asc);
    }

    #[test]
    fn toggle_sort_different_column_resets_to_asc() {
        let mut p = alpha_pane();
        p.sort_dir = SortDir::Desc; // pretend we were in descending order
        p.sort_by = SortBy::Name;
        p.toggle_sort(SortBy::Size);
        assert_eq!(p.sort_by, SortBy::Size);
        assert_eq!(p.sort_dir, SortDir::Asc);
    }

    #[test]
    fn toggle_sort_preserves_cursor_by_name() {
        let mut p = alpha_pane();
        // Default sort by Name Asc: alpha, alphabet, beta, gamma → "gamma"
        // is at visible row 4. Cursor there.
        p.selected = 4;
        p.anchor = 4;
        assert_eq!(p.cursor_name().as_deref(), Some("gamma"));

        // Sort by Size Asc: 1=alpha, 2=beta, 3=gamma, 4=alphabet.
        // "gamma" is now at row 3 (visible row index = entry index + 1).
        p.toggle_sort(SortBy::Size);
        assert_eq!(p.cursor_name().as_deref(), Some("gamma"));
    }

    // --- append_chunk ---

    #[test]
    fn append_chunk_accumulates_across_calls() {
        let mut p = Pane::empty(PathBuf::from("/p"));
        p.append_chunk(vec![mk_entry("b", false, Some(1))]);
        p.append_chunk(vec![mk_entry("a", false, Some(2))]);
        assert_eq!(p.entries.len(), 2);
        // Entries are stored in arrival order during streaming; sorting is
        // deferred to the EntriesDone handler to avoid O(n² log n) work on
        // the main thread. So here "b" still precedes "a".
        assert_eq!(p.entries[0].name, "b");
        assert_eq!(p.entries[1].name, "a");
    }

    #[test]
    fn append_chunk_rebuilds_visible_indices() {
        let mut p = Pane::empty(PathBuf::from("/p"));
        p.append_chunk(vec![
            mk_entry("a", false, Some(1)),
            mk_entry("b", false, Some(2)),
        ]);
        assert_eq!(p.visible_indices.len(), 2);
    }

    #[test]
    fn append_chunk_respects_existing_filter() {
        let mut p = Pane::empty(PathBuf::from("/p"));
        p.filter = "alp".to_string();
        p.recompute_visible(None);
        p.append_chunk(vec![
            mk_entry("alpha", false, None),
            mk_entry("zebra", false, None),
        ]);
        // "alp" matches "alpha", not "zebra".
        assert_eq!(p.visible_indices.len(), 1);
        assert_eq!(p.entries[p.visible_indices[0]].name, "alpha");
    }

    #[test]
    fn append_empty_chunk_is_noop() {
        let mut p = Pane::empty(PathBuf::from("/p"));
        p.append_chunk(vec![mk_entry("a", false, None)]);
        let before_len = p.entries.len();
        p.append_chunk(Vec::new());
        assert_eq!(p.entries.len(), before_len);
    }

    // --- cursor_name / entry_at ---

    #[test]
    fn cursor_name_returns_none_for_dotdot_row() {
        let mut p = alpha_pane();
        p.selected = 0; // ".." row
        assert!(p.cursor_name().is_none());
    }

    #[test]
    fn cursor_name_returns_entry_name_for_real_row() {
        let mut p = alpha_pane();
        p.selected = 1; // first entry after sort
        assert_eq!(p.cursor_name().as_deref(), Some("alpha"));
    }

    #[test]
    fn entry_at_zero_is_none() {
        let p = alpha_pane();
        assert!(p.entry_at(0).is_none());
    }

    #[test]
    fn entry_at_returns_visible_entry() {
        let p = alpha_pane();
        let e = p.entry_at(1).unwrap();
        assert_eq!(e.name, "alpha");
    }

    #[test]
    fn entry_at_out_of_range_returns_none() {
        let p = alpha_pane();
        assert!(p.entry_at(99).is_none());
    }

    // --- marked_paths ---

    #[test]
    fn marked_paths_for_single_row_returns_one_path() {
        let mut p = alpha_pane();
        p.selected = 1; // "alpha"
        p.anchor = 1;
        let paths = p.marked_paths();
        assert_eq!(paths, vec![PathBuf::from("/p").join("alpha")]);
    }

    #[test]
    fn marked_paths_for_range_returns_all_entries_in_range() {
        let mut p = alpha_pane();
        p.anchor = 1; // alpha
        p.selected = 3; // beta (after sort: alpha, alphabet, beta, gamma)
        let paths = p.marked_paths();
        assert_eq!(paths.len(), 3);
        assert!(paths.contains(&PathBuf::from("/p").join("alpha")));
        assert!(paths.contains(&PathBuf::from("/p").join("alphabet")));
        assert!(paths.contains(&PathBuf::from("/p").join("beta")));
    }

    #[test]
    fn marked_paths_skips_dotdot_row() {
        let mut p = alpha_pane();
        // Range includes the ".." row at index 0.
        p.anchor = 0;
        p.selected = 2;
        let paths = p.marked_paths();
        // 2 entries returned (alpha + alphabet), not 3 — the ".." row is dropped.
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn marked_paths_with_only_dotdot_selected_is_empty() {
        let mut p = alpha_pane();
        p.anchor = 0;
        p.selected = 0;
        assert!(p.marked_paths().is_empty());
    }

    // --- copy_recursive / delete_path (filesystem) ---

    #[test]
    fn copy_recursive_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&src, b"hello world").unwrap();

        copy_recursive(&src, &dst).unwrap();

        assert!(dst.exists());
        assert_eq!(std::fs::read(&dst).unwrap(), b"hello world");
        // Source is untouched.
        assert!(src.exists());
    }

    #[test]
    fn copy_recursive_directory_tree() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir(&src).unwrap();
        std::fs::create_dir(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), b"a").unwrap();
        std::fs::write(src.join("sub/b.txt"), b"b").unwrap();

        copy_recursive(&src, &dst).unwrap();

        assert!(dst.join("a.txt").exists());
        assert!(dst.join("sub/b.txt").exists());
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"a");
        assert_eq!(std::fs::read(dst.join("sub/b.txt")).unwrap(), b"b");
    }

    #[test]
    fn copy_recursive_missing_source_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        let dst = dir.path().join("dst");
        assert!(copy_recursive(&missing, &dst).is_err());
    }

    #[test]
    fn delete_path_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("to_delete.txt");
        std::fs::write(&file, b"bye").unwrap();
        assert!(file.exists());

        delete_path(&file).unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn delete_path_removes_directory_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("doomed");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("a.txt"), b"a").unwrap();
        std::fs::create_dir(target.join("sub")).unwrap();
        std::fs::write(target.join("sub/b.txt"), b"b").unwrap();

        delete_path(&target).unwrap();
        assert!(!target.exists());
    }

    #[test]
    fn delete_path_missing_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-here");
        assert!(delete_path(&missing).is_err());
    }

    // --- layout helpers ---

    #[test]
    fn name_max_chars_at_normal_width_is_reasonable() {
        let cfg = Config::default();
        let n = name_max_chars(1100.0, &cfg);
        // Sanity bounds: at 1100px window with the default columns the name
        // column should fit a few dozen glyphs.
        assert!(n >= 20, "expected at least 20 chars, got {}", n);
        assert!(n <= 100, "expected at most 100 chars, got {}", n);
    }

    #[test]
    fn name_max_chars_at_tiny_width_clamps_at_minimum() {
        let cfg = Config::default();
        // Even with a very narrow window, the helper clamps at 8 so we never
        // truncate filenames down to one or two glyphs.
        let n = name_max_chars(100.0, &cfg);
        assert!(n >= 8);
    }

    #[test]
    fn viewport_height_estimate_subtracts_chrome() {
        // At 700 window height, available rows ≈ 700 - 110 = 590.
        let h = viewport_height_estimate(700.0);
        assert!(approx_eq(h, 590.0));
    }

    #[test]
    fn viewport_height_estimate_clamps_at_minimum() {
        // At absurdly small heights it must still return at least 100 so the
        // scroll math doesn't divide by zero or produce negatives.
        let h = viewport_height_estimate(0.0);
        assert!(h >= 100.0);
    }
}
