use std::path::PathBuf;
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

mod config;
mod domain;
mod fs_ops;

use config::{
    blend, dim, ensure_settings_file, expand_tilde, home_dir, load_saved_state, open_in_editor,
    quick_look, row_colors_from, save_state_to_disk, settings_path, Config, RowColors, SavedState,
};
use domain::{
    sort_entries, DeleteFocus, Entry, GitInfo, NewFilesFocus, PaletteAction, Pane, Prompt,
    RowVisual, Side, SortBy, SortDir,
};
use fs_ops::{copy_task, delete_task, file_watch_subscription, loading_tasks};

const PROMPT_ID: &str = "prompt";

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
    EditFile,
    QuickLook,
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
                | Message::EditFile
                | Message::QuickLook
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
                if self.pane(side).selected == 0 {
                    if let Some(parent) = self.pane(side).path.parent().map(|p| p.to_path_buf()) {
                        return self.navigate(side, parent);
                    }
                    return Task::none();
                }
                let pane = self.pane(side);
                if let Some(entry) = pane.entry_at(pane.selected) {
                    let path = pane.path.join(&entry.name);
                    if entry.is_dir {
                        return self.navigate(side, path);
                    }
                    if let Err(e) = open::that_detached(&path) {
                        eprintln!("failed to open {}: {}", path.display(), e);
                    }
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
            Message::EditFile => {
                let pane = self.pane(self.active);
                if let Some(entry) = pane.entry_at(pane.selected) {
                    if entry.is_dir {
                        return Task::none();
                    }
                    let path = pane.path.join(&entry.name);
                    if let Err(e) = open_in_editor(&path) {
                        eprintln!("failed to edit {}: {}", path.display(), e);
                    }
                }
            }
            Message::QuickLook => {
                let pane = self.pane(self.active);
                if let Some(entry) = pane.entry_at(pane.selected) {
                    let path = pane.path.join(&entry.name);
                    if let Err(e) = quick_look(&path) {
                        eprintln!("failed to preview {}: {}", path.display(), e);
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
                Key::Named(Named::F4) => Some(Message::EditFile),
                Key::Named(Named::Space) => Some(Message::QuickLook),
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
        let name = if entry.is_dir {
            // Reserve one slot for the trailing '/' so dir names never exceed
            // name_max_chars after the suffix is appended.
            let budget = name_max_chars.saturating_sub(1).max(1);
            format!("{}/", truncate_with_ellipsis(&entry.name, budget))
        } else {
            truncate_with_ellipsis(&entry.name, name_max_chars)
        };
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
        .wrapping(iced::widget::text::Wrapping::None)
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
            .align_x(Horizontal::Right)
            .wrapping(iced::widget::text::Wrapping::None),
        text(modified)
            .font(Font::MONOSPACE)
            .size(font_size)
            .width(Length::Fixed(config.modified_column_px))
            .wrapping(iced::widget::text::Wrapping::None),
    ]
    .spacing(8);

    button(content)
        .on_press(msg)
        .width(Length::Fill)
        .height(Length::Fixed(config.row_height_px))
        .padding(Padding::from([pad_y, 8]))
        .clip(true)
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
    // The name column also contains a git-modified marker (width = font_size)
    // plus 4px spacing before the name text itself.
    let git_marker_with_gap = config.row_font_size as f32 + 4.0;
    // A couple of glyphs of breathing room: cosmic-text's measured glyph width
    // is rarely exactly mono_glyph_px, and we'd rather ellipsize one char too
    // early than have the name overflow into a second visual line.
    let safety_px = config.mono_glyph_px * 2.0;

    let name_px = pane_width
        - pane_border_padding
        - row_horizontal_padding
        - row_gaps
        - git_marker_with_gap
        - safety_px
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

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

