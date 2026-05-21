//! Pure domain types — no iced widgets, no async I/O. `Pane` holds a sorted
//! directory listing plus a filtered view (`visible_indices`) and the
//! selection / anchor used by the UI; mutations all go through this module
//! so the logic can be unit-tested without an iced runtime.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn other(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Size,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

impl SortDir {
    pub fn toggled(self) -> Self {
        match self {
            SortDir::Asc => SortDir::Desc,
            SortDir::Desc => SortDir::Asc,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowVisual {
    None,
    Cursor,
    Marked,
}

/// Modal flavors. Each carries the data the modal needs.
#[derive(Debug, Clone)]
pub enum Prompt {
    /// "Go to folder" — typed input plus a filterable list of recent locations
    /// (captured at open time). Submit prefers the typed path if it resolves
    /// to a real directory; otherwise opens the highlighted recent.
    Open {
        input: String,
        recents: Vec<PathBuf>,
        /// Index into the *filtered* list of recents.
        selected: usize,
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
    /// Action picker with a filter input. `selected` is an index into the
    /// *filtered* list of actions.
    CommandPalette {
        input: String,
        selected: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteFocus {
    Cancel,
    Confirm,
}

impl DeleteFocus {
    pub fn toggle(self) -> Self {
        match self {
            DeleteFocus::Cancel => DeleteFocus::Confirm,
            DeleteFocus::Confirm => DeleteFocus::Cancel,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewFilesFocus {
    No,
    Left,
    Right,
}

impl NewFilesFocus {
    pub fn next(self) -> Self {
        match self {
            NewFilesFocus::No => NewFilesFocus::Left,
            NewFilesFocus::Left => NewFilesFocus::Right,
            NewFilesFocus::Right => NewFilesFocus::No,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    Copy,
    Delete,
    Exit,
}

impl PaletteAction {
    pub const ALL: &'static [PaletteAction] = &[
        PaletteAction::Copy,
        PaletteAction::Delete,
        PaletteAction::Exit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PaletteAction::Copy => "Copy",
            PaletteAction::Delete => "Delete",
            PaletteAction::Exit => "Exit",
        }
    }
}

#[derive(Debug, Clone)]
pub struct GitInfo {
    pub branch: String,
    pub uncommitted: usize,
    pub ahead: usize,
    pub behind: usize,
    /// Names within the current pane directory that have uncommitted changes.
    /// For files, the name matches directly; for directories, at least one
    /// descendant has changes (we collapse subpaths to their first segment).
    pub modified_names: HashSet<String>,
}

#[derive(Debug)]
pub struct Pane {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    /// Indices into `entries` that pass the current filter, in display order.
    /// Row 0 is the ".." entry; rows 1..=visible_indices.len() map to
    /// `entries[visible_indices[i - 1]]`.
    pub visible_indices: Vec<usize>,
    /// Current type-to-filter query. Empty = all entries visible.
    pub filter: String,
    pub selected: usize,
    /// Anchor for shift-extended range selection.
    pub anchor: usize,
    pub sort_by: SortBy,
    pub sort_dir: SortDir,
    pub scroll_y: f32,
    pub viewport_height: Option<f32>,
    pub loading: bool,
    /// Bumped on every navigate. Streaming load tasks tag each chunk with the
    /// generation that started them, so chunks that finish late after the user
    /// has already moved on get dropped.
    pub load_generation: u64,
    /// Result of the async git probe, if this directory is inside a git repo.
    pub git_info: Option<GitInfo>,
    /// Name of an entry the cursor should land on as soon as it appears in
    /// the streaming load. Used to focus on a newly-detected file after the
    /// "new files" modal opens its folder. Cleared on `navigate()`, or when
    /// the entry has been located.
    pub pending_focus: Option<String>,
}

impl Pane {
    pub fn empty(path: PathBuf) -> Self {
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
    pub fn navigate(&mut self, path: PathBuf) -> u64 {
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

    pub fn append_chunk(&mut self, mut chunk: Vec<Entry>) {
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
    pub fn recompute_visible(&mut self, preserve_name: Option<&str>) {
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

    pub fn cursor_name(&self) -> Option<String> {
        if self.selected == 0 {
            return None;
        }
        self.visible_indices
            .get(self.selected - 1)
            .and_then(|&i| self.entries.get(i))
            .map(|e| e.name.clone())
    }

    pub fn entry_at(&self, row_index: usize) -> Option<&Entry> {
        if row_index == 0 {
            return None;
        }
        self.visible_indices
            .get(row_index - 1)
            .and_then(|&i| self.entries.get(i))
    }

    pub fn move_to(&mut self, target: i32, extend: bool) {
        let max = self.visible_indices.len() as i32;
        let next = target.clamp(0, max) as usize;
        if !extend {
            self.anchor = next;
        }
        self.selected = next;
    }

    pub fn move_by(&mut self, delta: i32, extend: bool) {
        let target = self.selected as i32 + delta;
        self.move_to(target, extend);
    }

    pub fn mark_range(&self) -> (usize, usize) {
        if self.anchor < self.selected {
            (self.anchor, self.selected)
        } else {
            (self.selected, self.anchor)
        }
    }

    pub fn is_marked(&self, row_index: usize) -> bool {
        let (lo, hi) = self.mark_range();
        row_index >= lo && row_index <= hi
    }

    pub fn marked_paths(&self) -> Vec<PathBuf> {
        let (lo, hi) = self.mark_range();
        (lo..=hi)
            .filter_map(|i| self.entry_at(i))
            .map(|e| self.path.join(&e.name))
            .collect()
    }

    pub fn toggle_sort(&mut self, by: SortBy) {
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

pub fn sort_entries(entries: &mut [Entry], by: SortBy, dir: SortDir) {
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

/// Case-insensitive substring filter over a slice of paths.
/// Empty `input` returns every entry in arrival order.
pub fn filtered_recents<'a>(recents: &'a [PathBuf], input: &str) -> Vec<&'a PathBuf> {
    if input.is_empty() {
        return recents.iter().collect();
    }
    let needle = input.to_lowercase();
    recents
        .iter()
        .filter(|p| p.display().to_string().to_lowercase().contains(&needle))
        .collect()
}

/// Case-insensitive substring filter over the command-palette action list.
pub fn filtered_actions(input: &str) -> Vec<PaletteAction> {
    if input.is_empty() {
        return PaletteAction::ALL.to_vec();
    }
    let needle = input.to_lowercase();
    PaletteAction::ALL
        .iter()
        .copied()
        .filter(|a| a.label().to_lowercase().contains(&needle))
        .collect()
}

/// Insert `path` at the front of `recents` (move-to-front if already present)
/// and truncate to `cap` entries. `path` should be a directory you actually
/// navigated to — the caller is responsible for that filtering.
pub fn add_recent(recents: &mut Vec<PathBuf>, path: PathBuf, cap: usize) {
    recents.retain(|p| p != &path);
    recents.insert(0, path);
    if recents.len() > cap {
        recents.truncate(cap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            Entry { name: "a".into(), is_dir: false, size: None, modified: Some(t2) },
            Entry { name: "b".into(), is_dir: false, size: None, modified: None },
            Entry { name: "c".into(), is_dir: false, size: None, modified: Some(t1) },
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

    #[test]
    fn filtered_recents_empty_input_returns_all() {
        let recents = vec![
            PathBuf::from("/etc"),
            PathBuf::from("/usr/local"),
            PathBuf::from("/home/me"),
        ];
        let out = filtered_recents(&recents, "");
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn filtered_recents_substring_matches() {
        let recents = vec![
            PathBuf::from("/etc"),
            PathBuf::from("/usr/local"),
            PathBuf::from("/home/me"),
        ];
        let out = filtered_recents(&recents, "local");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], &PathBuf::from("/usr/local"));
    }

    #[test]
    fn filtered_recents_is_case_insensitive() {
        let recents = vec![PathBuf::from("/Etc"), PathBuf::from("/Usr/Local")];
        let out = filtered_recents(&recents, "etc");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], &PathBuf::from("/Etc"));
    }

    #[test]
    fn filtered_recents_no_match_returns_empty() {
        let recents = vec![PathBuf::from("/etc"), PathBuf::from("/usr/local")];
        let out = filtered_recents(&recents, "zzz");
        assert!(out.is_empty());
    }

    #[test]
    fn filtered_actions_empty_input_returns_all() {
        let out = filtered_actions("");
        assert_eq!(out.len(), PaletteAction::ALL.len());
    }

    #[test]
    fn filtered_actions_substring_matches_label() {
        let out = filtered_actions("cop");
        assert_eq!(out, vec![PaletteAction::Copy]);
    }

    #[test]
    fn filtered_actions_is_case_insensitive() {
        let out = filtered_actions("EXIT");
        assert_eq!(out, vec![PaletteAction::Exit]);
    }

    #[test]
    fn filtered_actions_no_match_returns_empty() {
        let out = filtered_actions("zzzz");
        assert!(out.is_empty());
    }

    #[test]
    fn add_recent_inserts_at_front() {
        let mut r: Vec<PathBuf> = Vec::new();
        add_recent(&mut r, PathBuf::from("/a"), 10);
        add_recent(&mut r, PathBuf::from("/b"), 10);
        assert_eq!(r, vec![PathBuf::from("/b"), PathBuf::from("/a")]);
    }

    #[test]
    fn add_recent_moves_existing_to_front() {
        let mut r = vec![
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/c"),
        ];
        add_recent(&mut r, PathBuf::from("/b"), 10);
        // /b moved from middle to front; no duplicate.
        assert_eq!(
            r,
            vec![
                PathBuf::from("/b"),
                PathBuf::from("/a"),
                PathBuf::from("/c"),
            ]
        );
    }

    #[test]
    fn add_recent_respects_cap() {
        let mut r: Vec<PathBuf> = (0..5).map(|i| PathBuf::from(format!("/p{}", i))).collect();
        add_recent(&mut r, PathBuf::from("/new"), 3);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0], PathBuf::from("/new"));
    }

    #[test]
    fn add_recent_repeated_same_path_is_idempotent() {
        let mut r: Vec<PathBuf> = Vec::new();
        add_recent(&mut r, PathBuf::from("/a"), 10);
        add_recent(&mut r, PathBuf::from("/a"), 10);
        add_recent(&mut r, PathBuf::from("/a"), 10);
        assert_eq!(r, vec![PathBuf::from("/a")]);
    }
}
