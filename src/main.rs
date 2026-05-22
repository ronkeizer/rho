use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use iced::keyboard::{self, key::Named, Key, Modifiers};
use iced::widget::{column, container, row, scrollable, stack, text, text_input};
use iced::{time, window, Color, Element, Length, Padding, Size, Subscription, Task, Theme};

mod config;
mod domain;
mod fs_ops;
mod view_modal;
mod view_pane;

use config::{
    dim, ensure_settings_file, expand_tilde, home_dir, load_saved_state, open_in_editor,
    quick_look, row_colors_from, save_state_to_disk, settings_path, Config, SavedState,
};
use domain::{
    add_recent, available_palette_actions, default_zip_filename, filtered_actions, filtered_apps,
    filtered_branches, filtered_recents, filtered_servers, sort_apps,
    sort_containers, sort_entries, sort_processes, Application, AppsState, DeleteFocus,
    DockerContainer, DockerSortBy, DockerState, Entry, GitBranch, GitBranchesState, GitInfo,
    Location, NewFilesFocus, PaletteAction, Pane, Process, ProcessSortBy, ProcessesState, Prompt,
    Side, SortBy, SortDir, SshServer, SshServersState,
};
use fs_ops::{
    apps_task, compress_task, copy_task, delete_task, docker_kill_task, docker_ps_task,
    docker_shell, extract_to_temp_task, file_watch_subscription, git_branches_task,
    git_checkout_task, kill_process_task, launch_app, loading_tasks, move_task, open_claude_code,
    ps_task, ssh_connect, ssh_servers_task, uncompress_task,
};
use view_modal::view_modal;
use view_pane::{name_max_chars, scroll_id, view_pane, viewport_height_estimate};

pub(crate) const PROMPT_ID: &str = "prompt";
const RECENT_CAP: usize = 50;
/// Enter on a `.zip` over this size pops a confirmation modal before we
/// shell out to `unzip`. 100 MiB.
const LARGE_ARCHIVE_THRESHOLD: u64 = 100 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> iced::Result {
    ensure_settings_file();
    let config = Config::load();
    let window_size = config.window_size();
    iced::application("Rho", App::update, App::view)
        .subscription(App::subscription)
        .theme(|_| Theme::Dark)
        .window_size(window_size)
        .run_with(move || App::new(config))
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) enum Message {
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
    OpenMovePrompt,
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
    MoveFinished(Vec<(PathBuf, Result<(), String>)>),
    DeleteFinished(Vec<(PathBuf, Result<(), String>)>),
    /// `zip` task completed — `Ok(path)` is the new archive's path.
    CompressFinished(Result<PathBuf, String>),
    /// Per-archive extraction results.
    UncompressFinished(Vec<(PathBuf, Result<(), String>)>),
    /// Enter-on-zip extraction completed. On success, navigate the active
    /// pane into `dest` so the user can browse the unpacked contents.
    ExtractedToTemp(PathBuf, Result<(), String>),
    Resized(Size),
    /// The watcher subscription noticed new files in `folder`.
    NewFilesDetected(PathBuf, Vec<String>),
    /// User picked a pane in the NewFiles modal.
    NewFilesPickSide(Side),
    /// Open the command-palette modal (Cmd+Shift+P).
    OpenCommandPalette,
    /// Move the highlight in the current filterable modal (Open / CommandPalette)
    /// by `delta` rows (wraps).
    PromptMove(i32),
    /// User clicked a recent location in the Open modal — open it directly.
    OpenRecent(PathBuf),
    /// User clicked an action in the command palette — activate it directly.
    PaletteSelect(PaletteAction),
    /// `docker ps` completed (or failed) for the Docker modal.
    DockerListLoaded(Result<Vec<DockerContainer>, String>),
    /// User clicked Kill on a container row — start the kill task.
    DockerKill(String),
    /// `docker kill <id>` completed.
    DockerKillFinished(String, Result<(), String>),
    /// User clicked Shell on a container row — spawn a terminal.
    DockerShell(String),
    /// `ps -axo …` completed (or failed) for the Processes modal.
    ProcessesListLoaded(Result<Vec<Process>, String>),
    /// User clicked Kill on a process row — send SIGTERM.
    ProcessKill(u32),
    /// `kill <pid>` completed.
    ProcessKillFinished(u32, Result<(), String>),
    /// User clicked a Docker column header — toggle / switch the sort.
    DockerToggleSort(DockerSortBy),
    /// User clicked a Processes column header — toggle / switch the sort.
    ProcessToggleSort(ProcessSortBy),
    /// `scan_applications` finished (or failed) for the Apps modal.
    AppsListLoaded(Result<Vec<Application>, String>),
    /// User clicked Launch on an app row — `open` the bundle.
    LaunchApp(PathBuf),
    /// `git for-each-ref` completed (or failed) for the GitBranches modal.
    GitBranchesLoaded(Result<Vec<GitBranch>, String>),
    /// User clicked Checkout on a branch row — start the checkout task.
    GitCheckout(String),
    /// `git checkout <branch>` completed.
    GitCheckoutFinished(String, Result<(), String>),
    /// `read_ssh_config` completed (or failed) for the SshServers modal.
    SshServersLoaded(Result<Vec<SshServer>, String>),
    /// User clicked Connect on an SSH server row — spawn a terminal.
    SshConnect(String),
    /// Global "quit the app" — currently bound to F10.
    ExitApp,
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
    /// (count, dest) while a move task is in flight.
    move_in_progress: Option<(usize, PathBuf)>,
    /// File count while a delete task is in flight.
    delete_in_progress: Option<usize>,
    /// (count, output_zip) while a compress task is in flight.
    compress_in_progress: Option<(usize, PathBuf)>,
    /// (count, dest_dir) while an uncompress task is in flight.
    uncompress_in_progress: Option<(usize, PathBuf)>,
    /// Source archive path while an Enter-on-zip extraction is running.
    extract_in_progress: Option<PathBuf>,
    /// New-file detections that arrived while another modal was open. Drained
    /// FIFO when the current modal closes.
    pending_new_files: std::collections::VecDeque<(PathBuf, Vec<String>)>,
    /// Most-recently-visited directories, front-first. Persisted to
    /// `~/.rho-state.yaml`. Updated on every successful navigate.
    recent_locations: Vec<PathBuf>,
}

impl App {
    fn new(config: Config) -> (Self, Task<Message>) {
        let home = home_dir();
        let window_size = config.window_size();
        let settings_mtime = std::fs::metadata(settings_path())
            .and_then(|m| m.modified())
            .ok();

        // Restore last-used folders + active side. Any saved path that no
        // longer exists falls back to the home directory. Phase 1 only
        // accepts Local saved locations — a saved Remote (if a future
        // version wrote one and we somehow downgraded) silently falls
        // back to home until Phase 2 wires up the SSH backend.
        let saved = load_saved_state();
        let saved_local = |loc: &Location| match loc {
            Location::Local(p) if p.is_dir() => Some(p.clone()),
            _ => None,
        };
        let left_path = saved
            .as_ref()
            .and_then(|s| saved_local(&s.left))
            .unwrap_or_else(|| home.clone());
        let right_path = saved
            .as_ref()
            .and_then(|s| saved_local(&s.right))
            .unwrap_or_else(|| home.clone());
        let active = saved.as_ref().map(|s| s.active).unwrap_or(Side::Left);
        let recent_locations = saved
            .as_ref()
            .map(|s| s.recent.clone())
            .unwrap_or_default();

        let app = Self {
            config,
            left: Pane::empty(Location::Local(left_path.clone())),
            right: Pane::empty(Location::Local(right_path.clone())),
            active,
            prompt: None,
            shift_held: false,
            window_size,
            settings_mtime,
            copy_in_progress: None,
            move_in_progress: None,
            delete_in_progress: None,
            compress_in_progress: None,
            uncompress_in_progress: None,
            extract_in_progress: None,
            pending_new_files: std::collections::VecDeque::new(),
            recent_locations,
        };
        // Stat the initial pane paths for the CLAUDE.md / .claude marker so
        // the info bar shows on startup, not just after the first navigate.
        let mut app = app;
        refresh_claude_marker(&mut app.left);
        refresh_claude_marker(&mut app.right);
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
            left: self.left.location.clone(),
            right: self.right.location.clone(),
            active: self.active,
            recent: self.recent_locations.clone(),
        });
    }

    fn pane(&self, side: Side) -> &Pane {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    /// True iff the active pane is inside a git repository (i.e. `git_info`
    /// was populated by the per-navigate probe). Used by the command palette
    /// to gate the Git: branch action.
    fn in_git_repo(&self) -> bool {
        self.pane(self.active).git_info.is_some()
    }

    fn pane_mut(&mut self, side: Side) -> &mut Pane {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        }
    }

    fn navigate(&mut self, side: Side, path: PathBuf) -> Task<Message> {
        // Phase 1: every navigate target is a local path. When Phase 2
        // adds an "Open remote folder" entry point, it'll call
        // `Pane::navigate(Location::Remote { … })` directly (or this
        // helper will grow a `Location` overload).
        let generation = self
            .pane_mut(side)
            .navigate(Location::Local(path.clone()));
        refresh_claude_marker(self.pane_mut(side));
        add_recent(&mut self.recent_locations, path.clone(), RECENT_CAP);
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
        let left_loc = self.left.location.clone();
        let right_loc = self.right.location.clone();
        let left_path = left_loc.path().to_path_buf();
        let right_path = right_loc.path().to_path_buf();
        let left_gen = self.left.navigate(left_loc);
        let right_gen = self.right.navigate(right_loc);
        refresh_claude_marker(&mut self.left);
        refresh_claude_marker(&mut self.right);
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
            // Same Cancel/Confirm keyboard wiring as Delete — Enter routes to
            // the focused button, Tab/←/→ flip focus.
            (Some(Prompt::ConfirmLargeExtract { focus, .. }), m) => {
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
            // Open, CommandPalette, and Apps all have a text_input on top
            // and a filterable list below. text_input consumes Enter / Left
            // / Right / Backspace / printable chars, so ↑/↓/PgUp/PgDn/Tab
            // fall through here and we redirect them to PromptMove for list
            // navigation. Enter is delivered via text_input's on_submit →
            // PromptSubmit and doesn't reach this redirect.
            (
                Some(Prompt::Open { .. })
                | Some(Prompt::CommandPalette { .. })
                | Some(Prompt::Apps { .. })
                | Some(Prompt::GitBranches { .. })
                | Some(Prompt::SshServers { .. }),
                m,
            ) => match m {
                Message::MoveSelection(delta, _) => Message::PromptMove(delta),
                Message::PageMove(dir, _) => Message::PromptMove(dir * 5),
                Message::SwitchSide => Message::PromptMove(1),
                other => other,
            },
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
                | Message::OpenMovePrompt
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
                let filter_shrank = {
                    let pane = self.pane_mut(side);
                    if !pane.filter.is_empty() {
                        let prior = pane.cursor_name();
                        pane.filter.pop();
                        pane.recompute_visible(prior.as_deref());
                        true
                    } else {
                        false
                    }
                };
                if filter_shrank {
                    // visible row count changed — re-anchor scroll so the
                    // newly-matching rows are visible. See FilterAppend.
                    return self.ensure_active_visible();
                }
                if let Some(parent) = self.pane(side).path().parent().map(|p| p.to_path_buf()) {
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
                    if let Some(parent) = self.pane(side).path().parent().map(|p| p.to_path_buf()) {
                        return self.navigate(side, parent);
                    }
                    return Task::none();
                }
                let pane = self.pane(side);
                if let Some(entry) = pane.entry_at(pane.selected) {
                    let path = pane.path().join(&entry.name);
                    if entry.is_dir {
                        return self.navigate(side, path);
                    }
                    // Enter on a .zip extracts to /tmp and browses the result.
                    // Large archives (>100 MiB) pop a confirmation first so
                    // the user doesn't accidentally kick off a long unpack.
                    if is_zip(&path) {
                        let size = entry.size.unwrap_or(0);
                        if size > LARGE_ARCHIVE_THRESHOLD {
                            self.prompt = Some(Prompt::ConfirmLargeExtract {
                                archive_path: path,
                                size_bytes: size,
                                focus: DeleteFocus::Cancel,
                            });
                            return Task::none();
                        }
                        self.extract_in_progress = Some(path.clone());
                        return extract_to_temp_task(path);
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
                self.prompt = Some(Prompt::Open {
                    input: String::new(),
                    recents: self.recent_locations.clone(),
                    selected: 0,
                });
                return text_input::focus(text_input::Id::new(PROMPT_ID));
            }
            Message::OpenCopyPrompt => {
                let other = self.active.other();
                let dest = self.pane(other).path().display().to_string();
                self.prompt = Some(Prompt::Copy { input: dest });
                return text_input::focus(text_input::Id::new(PROMPT_ID));
            }
            Message::OpenMovePrompt => {
                let other = self.active.other();
                let dest = self.pane(other).path().display().to_string();
                self.prompt = Some(Prompt::Move { input: dest });
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
                Some(Prompt::Delete { focus, .. })
                | Some(Prompt::ConfirmLargeExtract { focus, .. }) => *focus = focus.toggle(),
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
                        Prompt::Open {
                            input,
                            recents,
                            selected,
                        } => {
                            *input = value;
                            // The filtered view just shrank or grew — clamp
                            // the highlight so it points at a real row.
                            let n = filtered_recents(recents, input).len();
                            *selected = if n == 0 { 0 } else { (*selected).min(n - 1) };
                        }
                        Prompt::Copy { input }
                        | Prompt::Move { input }
                        | Prompt::Compress { input }
                        | Prompt::Uncompress { input } => {
                            *input = value;
                        }
                        Prompt::CommandPalette {
                            input,
                            selected,
                            actions,
                        } => {
                            *input = value;
                            let n = filtered_actions(actions, input).len();
                            *selected = if n == 0 { 0 } else { (*selected).min(n - 1) };
                        }
                        Prompt::Docker { input, .. } | Prompt::Processes { input, .. } => {
                            *input = value;
                        }
                        Prompt::Apps {
                            input,
                            state,
                            selected,
                        } => {
                            *input = value;
                            if let AppsState::Loaded(list) = state {
                                let n = filtered_apps(list, input).len();
                                *selected = if n == 0 { 0 } else { (*selected).min(n - 1) };
                            }
                        }
                        Prompt::GitBranches {
                            input,
                            state,
                            selected,
                            ..
                        } => {
                            *input = value;
                            if let GitBranchesState::Loaded(list) = state {
                                let n = filtered_branches(list, input).len();
                                *selected = if n == 0 { 0 } else { (*selected).min(n - 1) };
                            }
                        }
                        Prompt::SshServers {
                            input,
                            state,
                            selected,
                        } => {
                            *input = value;
                            if let SshServersState::Loaded(list) = state {
                                let n = filtered_servers(list, input).len();
                                *selected = if n == 0 { 0 } else { (*selected).min(n - 1) };
                            }
                        }
                        Prompt::Delete { .. }
                        | Prompt::ConfirmLargeExtract { .. }
                        | Prompt::NewFiles { .. }
                        | Prompt::KeyboardShortcuts => {}
                    }
                }
            }
            Message::PromptSubmit => {
                let task = if let Some(prompt) = self.prompt.take() {
                    match prompt {
                        Prompt::Open {
                            input,
                            recents,
                            selected,
                        } => {
                            // Priority: typed input first (if it resolves to
                            // a real directory), otherwise the highlighted
                            // recent. This lets the user type a fresh path
                            // even when a substring matches an existing one.
                            let typed = PathBuf::from(expand_tilde(input.trim()));
                            let target = if typed.is_dir() {
                                Some(typed)
                            } else {
                                let filtered = filtered_recents(&recents, &input);
                                filtered.get(selected).map(|&p| p.clone())
                            };
                            target
                                .filter(|p| p.is_dir())
                                .map(|p| self.navigate(self.active, p))
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
                        Prompt::Move { input } => {
                            let dest = PathBuf::from(expand_tilde(input.trim()));
                            if dest.is_dir() {
                                let srcs = self.pane(self.active).marked_paths();
                                if !srcs.is_empty() {
                                    self.move_in_progress = Some((srcs.len(), dest.clone()));
                                    Some(move_task(srcs, dest))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                        Prompt::Compress { input } => {
                            let output = PathBuf::from(expand_tilde(input.trim()));
                            let srcs = self.pane(self.active).marked_paths();
                            let working_dir = self.pane(self.active).path().to_path_buf();
                            if !srcs.is_empty() {
                                self.compress_in_progress = Some((srcs.len(), output.clone()));
                                Some(compress_task(srcs, output, working_dir))
                            } else {
                                None
                            }
                        }
                        Prompt::Uncompress { input } => {
                            let dest = PathBuf::from(expand_tilde(input.trim()));
                            if dest.is_dir() {
                                let archives = self.pane(self.active).marked_paths();
                                if !archives.is_empty() {
                                    self.uncompress_in_progress =
                                        Some((archives.len(), dest.clone()));
                                    Some(uncompress_task(archives, dest))
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
                        Prompt::ConfirmLargeExtract { archive_path, .. } => {
                            self.extract_in_progress = Some(archive_path.clone());
                            Some(extract_to_temp_task(archive_path))
                        }
                        // NewFiles uses NewFilesPickSide / PromptCancel, never
                        // PromptSubmit. If we somehow get here, just drop it.
                        Prompt::NewFiles { .. } => None,
                        Prompt::CommandPalette {
                            input,
                            selected,
                            actions,
                        } => {
                            let filtered = filtered_actions(&actions, &input);
                            if let Some(action) = filtered.get(selected).copied() {
                                return self.execute_palette_action(action);
                            }
                            None
                        }
                        // Docker / Processes have a filter input but actions
                        // are mouse-only — Enter is a no-op. Re-instate the
                        // prompt so the modal doesn't close.
                        Prompt::Docker {
                            state,
                            input,
                            sort_by,
                            sort_dir,
                        } => {
                            self.prompt = Some(Prompt::Docker {
                                state,
                                input,
                                sort_by,
                                sort_dir,
                            });
                            None
                        }
                        Prompt::Processes {
                            state,
                            input,
                            sort_by,
                            sort_dir,
                        } => {
                            self.prompt = Some(Prompt::Processes {
                                state,
                                input,
                                sort_by,
                                sort_dir,
                            });
                            None
                        }
                        // Apps modal: Enter launches the highlighted row.
                        // If no row is selectable (empty / loading / error),
                        // fall through to closing the modal silently.
                        Prompt::Apps {
                            state,
                            input,
                            selected,
                        } => {
                            let target = if let AppsState::Loaded(list) = &state {
                                let filtered = filtered_apps(list, &input);
                                filtered.get(selected).map(|&a| a.path.clone())
                            } else {
                                None
                            };
                            if let Some(path) = target {
                                if let Err(e) = launch_app(&path) {
                                    eprintln!("launch {} failed: {}", path.display(), e);
                                }
                                None
                            } else {
                                // Nothing to launch — keep the modal open.
                                self.prompt = Some(Prompt::Apps {
                                    state,
                                    input,
                                    selected,
                                });
                                None
                            }
                        }
                        // GitBranches modal: Enter checks out the highlighted
                        // branch. Empty/loading/error keeps the modal open.
                        Prompt::GitBranches {
                            state,
                            input,
                            selected,
                            repo_path,
                        } => {
                            let branch_name = if let GitBranchesState::Loaded(list) = &state {
                                let filtered = filtered_branches(list, &input);
                                filtered.get(selected).map(|&b| b.name.clone())
                            } else {
                                None
                            };
                            if let Some(branch) = branch_name {
                                Some(git_checkout_task(repo_path.clone(), branch))
                            } else {
                                self.prompt = Some(Prompt::GitBranches {
                                    state,
                                    input,
                                    selected,
                                    repo_path,
                                });
                                None
                            }
                        }
                        // Informational modal — Enter is a no-op; put the
                        // prompt back so the modal doesn't close.
                        Prompt::KeyboardShortcuts => {
                            self.prompt = Some(Prompt::KeyboardShortcuts);
                            None
                        }
                        // SshServers: Enter connects to the highlighted row.
                        // Empty/loading/error keeps the modal open.
                        Prompt::SshServers {
                            state,
                            input,
                            selected,
                        } => {
                            let alias = if let SshServersState::Loaded(list) = &state {
                                let filtered = filtered_servers(list, &input);
                                filtered.get(selected).map(|&s| s.alias.clone())
                            } else {
                                None
                            };
                            if let Some(alias) = alias {
                                if let Err(e) =
                                    ssh_connect(&alias, self.config.terminal_app.as_deref())
                                {
                                    eprintln!("ssh connect {} failed: {}", alias, e);
                                }
                                None
                            } else {
                                self.prompt = Some(Prompt::SshServers {
                                    state,
                                    input,
                                    selected,
                                });
                                None
                            }
                        }
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
                    let filter_was_active = {
                        let pane = self.pane_mut(side);
                        if !pane.filter.is_empty() {
                            let prior = pane.cursor_name();
                            pane.filter.clear();
                            pane.recompute_visible(prior.as_deref());
                            true
                        } else {
                            false
                        }
                    };
                    if filter_was_active {
                        // Visible row count changed (filter dropped → all
                        // entries shown again). Re-anchor scroll to the
                        // cursor — without this, the unfiltered list may
                        // still appear scrolled to the wrong position.
                        return self.ensure_active_visible();
                    }
                }
            }
            Message::FilterAppend(s) => {
                let side = self.active;
                let pane = self.pane_mut(side);
                let prior = pane.cursor_name();
                pane.filter.push_str(&s);
                pane.recompute_visible(prior.as_deref());
                // After the visible-row count changes, the existing scroll
                // position may be past the end of the new (filtered) list.
                // ensure_active_visible re-anchors scroll_y around the
                // (possibly-reset) cursor so the matching rows are on-screen.
                return self.ensure_active_visible();
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
                    let path = pane.path().join(&entry.name);
                    if let Err(e) = open_in_editor(&path) {
                        eprintln!("failed to edit {}: {}", path.display(), e);
                    }
                }
            }
            Message::QuickLook => {
                let pane = self.pane(self.active);
                if let Some(entry) = pane.entry_at(pane.selected) {
                    let path = pane.path().join(&entry.name);
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
            Message::MoveFinished(results) => {
                self.move_in_progress = None;
                for (src, res) in &results {
                    if let Err(e) = res {
                        eprintln!("move {} failed: {}", src.display(), e);
                    }
                }
                return self.reload_both_panes();
            }
            Message::CompressFinished(result) => {
                self.compress_in_progress = None;
                if let Err(e) = &result {
                    eprintln!("compress failed: {}", e);
                }
                return self.reload_both_panes();
            }
            Message::UncompressFinished(results) => {
                self.uncompress_in_progress = None;
                for (archive, res) in &results {
                    if let Err(e) = res {
                        eprintln!("uncompress {} failed: {}", archive.display(), e);
                    }
                }
                return self.reload_both_panes();
            }
            Message::ExtractedToTemp(dest, result) => {
                let archive = self.extract_in_progress.take();
                match result {
                    Ok(()) => {
                        let side = self.active;
                        return self.navigate(side, dest);
                    }
                    Err(e) => {
                        let name = archive
                            .as_deref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| dest.display().to_string());
                        eprintln!("extract {} failed: {}", name, e);
                    }
                }
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
                let actions = available_palette_actions(self.in_git_repo());
                self.prompt = Some(Prompt::CommandPalette {
                    input: String::new(),
                    selected: 0,
                    actions,
                });
                return text_input::focus(text_input::Id::new(PROMPT_ID));
            }
            Message::PromptMove(delta) => match self.prompt.as_mut() {
                Some(Prompt::Open {
                    input,
                    recents,
                    selected,
                }) => {
                    let n = filtered_recents(recents, input).len() as i32;
                    if n > 0 {
                        *selected = (*selected as i32 + delta).rem_euclid(n) as usize;
                    }
                }
                Some(Prompt::CommandPalette {
                    input,
                    selected,
                    actions,
                }) => {
                    let n = filtered_actions(actions, input).len() as i32;
                    if n > 0 {
                        *selected = (*selected as i32 + delta).rem_euclid(n) as usize;
                    }
                }
                Some(Prompt::Apps {
                    state,
                    input,
                    selected,
                }) => {
                    if let AppsState::Loaded(list) = state {
                        let n = filtered_apps(list, input).len() as i32;
                        if n > 0 {
                            *selected = (*selected as i32 + delta).rem_euclid(n) as usize;
                        }
                    }
                }
                Some(Prompt::GitBranches {
                    state,
                    input,
                    selected,
                    ..
                }) => {
                    if let GitBranchesState::Loaded(list) = state {
                        let n = filtered_branches(list, input).len() as i32;
                        if n > 0 {
                            *selected = (*selected as i32 + delta).rem_euclid(n) as usize;
                        }
                    }
                }
                Some(Prompt::SshServers {
                    state,
                    input,
                    selected,
                }) => {
                    if let SshServersState::Loaded(list) = state {
                        let n = filtered_servers(list, input).len() as i32;
                        if n > 0 {
                            *selected = (*selected as i32 + delta).rem_euclid(n) as usize;
                        }
                    }
                }
                _ => {}
            },
            Message::OpenRecent(path) => {
                self.prompt = None;
                self.show_next_pending();
                if path.is_dir() {
                    return self.navigate(self.active, path);
                }
            }
            Message::PaletteSelect(action) => {
                return self.execute_palette_action(action);
            }
            Message::DockerListLoaded(result) => {
                // Only fold the result back in if the Docker modal is still
                // up; the user may have dismissed it before docker ps
                // finished.
                if let Some(Prompt::Docker {
                    state,
                    sort_by,
                    sort_dir,
                    ..
                }) = self.prompt.as_mut()
                {
                    *state = match result {
                        Ok(mut list) => {
                            sort_containers(&mut list, *sort_by, *sort_dir);
                            DockerState::Loaded(list)
                        }
                        Err(msg) => DockerState::Error(msg),
                    };
                }
            }
            Message::DockerKill(id) => {
                return docker_kill_task(id);
            }
            Message::DockerKillFinished(id, result) => {
                if let Err(e) = &result {
                    eprintln!("docker kill {} failed: {}", id, e);
                }
                // Re-fetch the list so the killed container disappears (or
                // sticks around with a refreshed status if the kill failed).
                if let Some(Prompt::Docker { .. }) = &self.prompt {
                    return docker_ps_task();
                }
            }
            Message::DockerShell(id) => {
                if let Err(e) = docker_shell(&id, self.config.terminal_app.as_deref()) {
                    eprintln!("docker shell {} failed: {}", id, e);
                }
            }
            Message::ProcessesListLoaded(result) => {
                if let Some(Prompt::Processes {
                    state,
                    sort_by,
                    sort_dir,
                    ..
                }) = self.prompt.as_mut()
                {
                    *state = match result {
                        Ok(mut list) => {
                            sort_processes(&mut list, *sort_by, *sort_dir);
                            ProcessesState::Loaded(list)
                        }
                        Err(msg) => ProcessesState::Error(msg),
                    };
                }
            }
            Message::ProcessKill(pid) => {
                return kill_process_task(pid);
            }
            Message::ProcessKillFinished(pid, result) => {
                if let Err(e) = &result {
                    eprintln!("kill {} failed: {}", pid, e);
                }
                if let Some(Prompt::Processes { .. }) = &self.prompt {
                    return ps_task();
                }
            }
            Message::DockerToggleSort(column) => {
                if let Some(Prompt::Docker {
                    state,
                    sort_by,
                    sort_dir,
                    ..
                }) = self.prompt.as_mut()
                {
                    if *sort_by == column {
                        *sort_dir = sort_dir.toggled();
                    } else {
                        *sort_by = column;
                        *sort_dir = column.initial_dir();
                    }
                    if let DockerState::Loaded(list) = state {
                        sort_containers(list, *sort_by, *sort_dir);
                    }
                }
            }
            Message::AppsListLoaded(result) => {
                if let Some(Prompt::Apps { state, .. }) = self.prompt.as_mut() {
                    *state = match result {
                        Ok(mut list) => {
                            sort_apps(&mut list);
                            AppsState::Loaded(list)
                        }
                        Err(msg) => AppsState::Error(msg),
                    };
                }
            }
            Message::LaunchApp(path) => {
                self.prompt = None;
                self.show_next_pending();
                if let Err(e) = launch_app(&path) {
                    eprintln!("launch {} failed: {}", path.display(), e);
                }
            }
            Message::GitBranchesLoaded(result) => {
                if let Some(Prompt::GitBranches { state, .. }) = self.prompt.as_mut() {
                    // git for-each-ref already returns committerdate-sorted
                    // rows, so we hand them through unchanged.
                    *state = match result {
                        Ok(list) => GitBranchesState::Loaded(list),
                        Err(msg) => GitBranchesState::Error(msg),
                    };
                }
            }
            Message::GitCheckout(branch) => {
                if let Some(Prompt::GitBranches { repo_path, .. }) = &self.prompt {
                    let path = repo_path.clone();
                    return git_checkout_task(path, branch);
                }
            }
            Message::SshServersLoaded(result) => {
                if let Some(Prompt::SshServers { state, .. }) = self.prompt.as_mut() {
                    *state = match result {
                        Ok(list) => SshServersState::Loaded(list),
                        Err(msg) => SshServersState::Error(msg),
                    };
                }
            }
            Message::SshConnect(alias) => {
                // Close the modal first — the terminal launches in a separate
                // window, so once it's spawned the user is interacting with
                // ssh, not with us.
                self.prompt = None;
                self.show_next_pending();
                if let Err(e) = ssh_connect(&alias, self.config.terminal_app.as_deref()) {
                    eprintln!("ssh connect {} failed: {}", alias, e);
                }
            }
            Message::GitCheckoutFinished(branch, result) => {
                match result {
                    Ok(()) => {
                        // Switch to the new branch — close the modal and refresh
                        // both panes so file listings + git_info reflect HEAD.
                        self.prompt = None;
                        self.show_next_pending();
                        return self.reload_both_panes();
                    }
                    Err(e) => {
                        eprintln!("git checkout {} failed: {}", branch, e);
                        // Surface the error in the modal so the user knows it
                        // didn't take effect (dirty tree, missing branch, …).
                        if let Some(Prompt::GitBranches { state, .. }) = self.prompt.as_mut() {
                            *state = GitBranchesState::Error(e);
                        }
                    }
                }
            }
            Message::ExitApp => {
                return window::get_oldest().and_then(window::close);
            }
            Message::ProcessToggleSort(column) => {
                if let Some(Prompt::Processes {
                    state,
                    sort_by,
                    sort_dir,
                    ..
                }) = self.prompt.as_mut()
                {
                    if *sort_by == column {
                        *sort_dir = sort_dir.toggled();
                    } else {
                        *sort_by = column;
                        *sort_dir = column.initial_dir();
                    }
                    if let ProcessesState::Loaded(list) = state {
                        sort_processes(list, *sort_by, *sort_dir);
                    }
                }
            }
            Message::NoOp => {}
        }
        Task::none()
    }

    /// Dispatch a command-palette action. Shared between direct clicks
    /// (`PaletteSelect`) and Enter-on-highlighted (`PromptSubmit`).
    fn execute_palette_action(&mut self, action: PaletteAction) -> Task<Message> {
        self.prompt = None;
        match action {
            PaletteAction::Copy => {
                let other = self.active.other();
                let dest = self.pane(other).path().display().to_string();
                self.prompt = Some(Prompt::Copy { input: dest });
                text_input::focus(text_input::Id::new(PROMPT_ID))
            }
            PaletteAction::Move => {
                let other = self.active.other();
                let dest = self.pane(other).path().display().to_string();
                self.prompt = Some(Prompt::Move { input: dest });
                text_input::focus(text_input::Id::new(PROMPT_ID))
            }
            PaletteAction::Delete => {
                let paths = self.pane(self.active).marked_paths();
                if !paths.is_empty() {
                    self.prompt = Some(Prompt::Delete {
                        paths,
                        focus: DeleteFocus::Cancel,
                    });
                }
                Task::none()
            }
            PaletteAction::Compress => {
                let marked = self.pane(self.active).marked_paths();
                if marked.is_empty() {
                    return Task::none();
                }
                let other = self.active.other();
                let dest_dir = self.pane(other).path().to_path_buf();
                let default_path = dest_dir.join(default_zip_filename(&marked));
                self.prompt = Some(Prompt::Compress {
                    input: default_path.display().to_string(),
                });
                text_input::focus(text_input::Id::new(PROMPT_ID))
            }
            PaletteAction::Uncompress => {
                let marked = self.pane(self.active).marked_paths();
                if marked.is_empty() {
                    return Task::none();
                }
                let other = self.active.other();
                let dest = self.pane(other).path().display().to_string();
                self.prompt = Some(Prompt::Uncompress { input: dest });
                text_input::focus(text_input::Id::new(PROMPT_ID))
            }
            PaletteAction::DockerContainers => {
                self.prompt = Some(Prompt::Docker {
                    state: DockerState::Loading,
                    input: String::new(),
                    sort_by: DockerSortBy::Name,
                    sort_dir: SortDir::Asc,
                });
                Task::batch([
                    docker_ps_task(),
                    text_input::focus(text_input::Id::new(PROMPT_ID)),
                ])
            }
            PaletteAction::Processes => {
                self.prompt = Some(Prompt::Processes {
                    state: ProcessesState::Loading,
                    input: String::new(),
                    sort_by: ProcessSortBy::Cpu,
                    sort_dir: SortDir::Desc,
                });
                Task::batch([
                    ps_task(),
                    text_input::focus(text_input::Id::new(PROMPT_ID)),
                ])
            }
            PaletteAction::LaunchApplication => {
                self.prompt = Some(Prompt::Apps {
                    state: AppsState::Loading,
                    input: String::new(),
                    selected: 0,
                });
                Task::batch([
                    apps_task(),
                    text_input::focus(text_input::Id::new(PROMPT_ID)),
                ])
            }
            PaletteAction::GitBranch => {
                let repo_path = self.pane(self.active).path().to_path_buf();
                self.prompt = Some(Prompt::GitBranches {
                    state: GitBranchesState::Loading,
                    input: String::new(),
                    selected: 0,
                    repo_path: repo_path.clone(),
                });
                Task::batch([
                    git_branches_task(repo_path),
                    text_input::focus(text_input::Id::new(PROMPT_ID)),
                ])
            }
            PaletteAction::KeyboardShortcuts => {
                self.prompt = Some(Prompt::KeyboardShortcuts);
                Task::none()
            }
            PaletteAction::SshConnect => {
                self.prompt = Some(Prompt::SshServers {
                    state: SshServersState::Loading,
                    input: String::new(),
                    selected: 0,
                });
                Task::batch([
                    ssh_servers_task(),
                    text_input::focus(text_input::Id::new(PROMPT_ID)),
                ])
            }
            PaletteAction::OpenClaudeCode => {
                // Fire-and-forget: spawn a terminal with `claude` running in
                // the active pane's directory. No modal — the action just
                // launches and closes the palette.
                let path = self.pane(self.active).path().to_path_buf();
                if let Err(e) = open_claude_code(&path, self.config.terminal_app.as_deref()) {
                    eprintln!("open Claude Code in {} failed: {}", path.display(), e);
                }
                Task::none()
            }
            PaletteAction::Exit => window::get_oldest().and_then(window::close),
        }
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
        if let Some((count, dest)) = &self.move_in_progress {
            let noun = if *count == 1 { "file" } else { "files" };
            return Some(format!("Moving {} {} to {}…", count, noun, dest.display()));
        }
        if let Some((count, output)) = &self.compress_in_progress {
            let noun = if *count == 1 { "file" } else { "files" };
            return Some(format!(
                "Compressing {} {} → {}…",
                count,
                noun,
                output.display()
            ));
        }
        if let Some((count, dest)) = &self.uncompress_in_progress {
            let noun = if *count == 1 { "archive" } else { "archives" };
            return Some(format!(
                "Extracting {} {} to {}…",
                count,
                noun,
                dest.display()
            ));
        }
        if let Some(archive) = &self.extract_in_progress {
            let name = archive
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| archive.display().to_string());
            return Some(format!("Extracting {} to /tmp…", name));
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
            return Some(format!("Loading {}…", p.path().display()));
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

        // Black background + dim text — the quietest band we can give a row
        // that still needs to be readable. Same look for "Ready" and for any
        // in-progress task message.
        let status_msg = self.status_text().unwrap_or_else(|| "Ready".to_string());
        let status_bar: Element<'_, Message> = container(text(status_msg).size(11))
            .padding(Padding::from([4, 8]))
            .width(Length::Fill)
            .style(|theme: &Theme| container::Style {
                background: Some(Color::BLACK.into()),
                text_color: Some(dim(theme.extended_palette().background.base.text)),
                ..Default::default()
            })
            .into();

        let base: Element<'_, Message> = container(column![panes, status_bar].spacing(6))
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
                // Esc is handled by the dedicated `esc_listener`
                // subscription below — text_input captures Esc to clear its
                // own focus, so routing it through `on_key_press` (which
                // only fires on Ignored events) makes the user hit Esc
                // twice. `event::listen_with` sees the press regardless
                // of widget capture.
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
                Key::Named(Named::F6) => Some(Message::OpenMovePrompt),
                Key::Named(Named::F10) => Some(Message::ExitApp),
                // Plain character keys (no Ctrl/Cmd/Alt) feed the type-to-filter.
                // The earlier Cmd+P / Cmd+, guards have already matched before
                // we get here, so unmodified characters fall through to this.
                Key::Character(c) if !mods.command() && !mods.control() && !mods.alt() => {
                    Some(Message::FilterAppend(c.to_string()))
                }
                _ => None,
            }
        });

        // Shift state — tracked via the raw `ModifiersChanged` event so it
        // fires regardless of widget capture. The earlier `on_key_press` /
        // `on_key_release` based approach only fired on `Status::Ignored`,
        // which meant a released-after-click shift could be swallowed by
        // the focused button and leave `shift_held` stuck `true` — causing
        // the next plain click to extend the selection.
        let mods_listener =
            iced::event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::ModifiersChanged(mods)) => {
                    Some(Message::ShiftChanged(mods.shift()))
                }
                _ => None,
            });

        // Esc — captured at the raw-event level (status is intentionally
        // ignored) so it fires even when a `text_input` has focus and
        // would otherwise swallow the first press to clear its own focus.
        // `PromptCancel` is idempotent: closes the current modal if any,
        // else clears the active pane's filter, else no-op.
        let esc_listener =
            iced::event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: Key::Named(Named::Escape),
                    ..
                }) => Some(Message::PromptCancel),
                _ => None,
            });

        let resizes = window::resize_events().map(|(_, size)| Message::Resized(size));

        let settings_poll = time::every(Duration::from_secs(1)).map(|_| Message::CheckSettings);

        let mut subs = vec![key_press, mods_listener, esc_listener, resizes, settings_poll];

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

/// Stat `pane.path()` for the Claude context markers (`CLAUDE.md` /
/// `.claude/`) and update the pane's cached booleans. Called on every
/// navigate / reload — same lifecycle as `git_info`. Phase 1 always
/// sees a local pane, so the inner-path stat is correct; Phase 2 will
/// gate this behind `pane.location.is_local()` once remote panes exist.
fn refresh_claude_marker(pane: &mut Pane) {
    pane.has_claude_md = pane.path().join("CLAUDE.md").is_file();
    pane.has_claude_dir = pane.path().join(".claude").is_dir();
}

/// True if the path's extension is `zip` (case-insensitive). Used to gate
/// Enter-on-archive extraction.
fn is_zip(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::is_zip;
    use std::path::Path;

    #[test]
    fn is_zip_matches_lowercase() {
        assert!(is_zip(Path::new("archive.zip")));
    }

    #[test]
    fn is_zip_matches_uppercase() {
        assert!(is_zip(Path::new("ARCHIVE.ZIP")));
    }

    #[test]
    fn is_zip_rejects_tar_gz() {
        assert!(!is_zip(Path::new("archive.tar.gz")));
    }

    #[test]
    fn is_zip_rejects_no_extension() {
        assert!(!is_zip(Path::new("README")));
    }
}
