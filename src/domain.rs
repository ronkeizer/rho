//! Pure domain types — no iced widgets, no async I/O. `Pane` holds a sorted
//! directory listing plus a filtered view (`visible_indices`) and the
//! selection / anchor used by the UI; mutations all go through this module
//! so the logic can be unit-tested without an iced runtime.

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::convert::Infallible;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::SystemTime;

/// Stable identifier for a registered backend instance — e.g.
/// `"alice.dev"` for an SSH host alias, or `"work"` for a Dropbox
/// account. Used in [`Location::Remote`] to dispatch operations to the
/// right backend at runtime.
///
/// Today's only backend is SSH (the identifier is a `Host` alias from
/// `~/.ssh/config`); the type stays open so other transports — Docker
/// volumes, cloud storage — can plug in without churning call sites.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendId(String);

impl BackendId {
    /// Reserved backend id for the single Dropbox account. Locations under
    /// it serialize as `dropbox:/path`; the remote dispatchers in `fs_ops`
    /// route this id to the Dropbox HTTP API instead of ssh/sftp.
    pub const DROPBOX: &'static str = "dropbox";

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Which transport this backend speaks. Today the only non-SSH backend
    /// is Dropbox, keyed off the reserved [`BackendId::DROPBOX`] id.
    pub fn kind(&self) -> BackendKind {
        if self.0 == Self::DROPBOX {
            BackendKind::Dropbox
        } else {
            BackendKind::Ssh
        }
    }

    pub fn is_dropbox(&self) -> bool {
        self.kind() == BackendKind::Dropbox
    }
}

/// Which transport a [`BackendId`] resolves to. Drives the `fs_ops`
/// dispatch between the ssh/sftp subprocess path and the Dropbox HTTP API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Ssh,
    Dropbox,
}

impl fmt::Display for BackendId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a pane's listing lives. `Local` is a path on the local
/// filesystem; `Remote` is a path on a registered backend (SSH host,
/// Dropbox account, …) identified by its [`BackendId`].
///
/// Serialized to / from a single string in `~/.rho-state.yaml`:
/// - `Local(PathBuf)` ↔ the path as-is (`"/Users/ron"`, `"C:\\foo"`).
/// - `Remote { backend, path }` ↔ `"<backend>:<path>"` where `<path>`
///   starts with `/` or `~` so it can't be confused with a local
///   filename containing a colon (`file:notes.txt` stays Local).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Local(PathBuf),
    Remote { backend: BackendId, path: PathBuf },
}

impl Location {
    // `local` / `remote` are ergonomic constructors; call sites
    // currently use the variant tuple form directly, but these stay so
    // future code (and tests) can avoid the `BackendId::new("…")`
    // boilerplate at the call site.
    #[allow(dead_code)]
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::Local(path.into())
    }

    #[allow(dead_code)]
    pub fn remote(backend: BackendId, path: impl Into<PathBuf>) -> Self {
        Self::Remote {
            backend,
            path: path.into(),
        }
    }

    /// Gate for "this op only works on local panes" — used to short-
    /// circuit file operations (copy/move/delete/edit/…) when the
    /// active or other pane is remote.
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    /// Inner path, regardless of variant. Safe to use for display,
    /// `join`, `parent`, and other purely-syntactic path operations.
    /// I/O *must* dispatch on the variant — see `is_local()`.
    pub fn path(&self) -> &Path {
        match self {
            Location::Local(p) => p,
            Location::Remote { path, .. } => path,
        }
    }

    /// Parent location, preserving the backend. Returns `None` at the
    /// filesystem root (`/` locally, `<backend>:/` remotely).
    pub fn parent(&self) -> Option<Location> {
        match self {
            Location::Local(p) => p.parent().map(|p| Location::Local(p.to_path_buf())),
            Location::Remote { backend, path } => path.parent().map(|p| Location::Remote {
                backend: backend.clone(),
                path: p.to_path_buf(),
            }),
        }
    }

    /// Join an entry name onto the location, preserving the backend.
    pub fn join(&self, name: impl AsRef<Path>) -> Location {
        match self {
            Location::Local(p) => Location::Local(p.join(name)),
            Location::Remote { backend, path } => Location::Remote {
                backend: backend.clone(),
                path: path.join(name),
            },
        }
    }
}

impl FromStr for Location {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // No colon → unambiguous local path.
        let colon = match s.find(':') {
            Some(i) => i,
            None => return Ok(Location::Local(PathBuf::from(s))),
        };
        let prefix = &s[..colon];
        let rest = &s[colon + 1..];

        // Windows drive letter (`C:\foo` or `C:/foo`) — single ASCII
        // alpha followed by `:` and a path separator.
        if prefix.len() == 1
            && prefix.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && rest.starts_with(['\\', '/'])
        {
            return Ok(Location::Local(PathBuf::from(s)));
        }

        // <backend-id>:<remote-path>. The remote path must start with
        // `/` or `~` — without that anchor we treat the colon as part
        // of a local filename (e.g. `file:notes.txt`).
        let is_backend_id = !prefix.is_empty()
            && prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if is_backend_id && (rest.starts_with('/') || rest.starts_with('~')) {
            return Ok(Location::Remote {
                backend: BackendId::new(prefix),
                path: PathBuf::from(rest),
            });
        }

        Ok(Location::Local(PathBuf::from(s)))
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Location::Local(p) => write!(f, "{}", p.display()),
            Location::Remote { backend, path } => {
                write!(f, "{}:{}", backend.as_str(), path.display())
            }
        }
    }
}

impl serde::Serialize for Location {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Location {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        Ok(Location::from_str(&s).expect("Location::from_str is infallible"))
    }
}

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

/// Which pane an OS file-drop lands in, given the cursor's window-space x and
/// the window width. Left half → `Left`; the right half and the exact midpoint
/// → `Right`. iced's `FileDropped` event carries no position, so the drop is
/// routed by the last-tracked cursor position instead.
pub fn drop_target_side(cursor_x: f32, window_width: f32) -> Side {
    if cursor_x < window_width / 2.0 {
        Side::Left
    } else {
        Side::Right
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

#[derive(Debug, Clone, PartialEq)]
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
/// A user-defined "open with…" action for files matching `pattern`, read from
/// the `file_actions` list in `~/.rho.yaml`. When Enter is pressed on a file
/// whose name matches `pattern` (a `*`/`?` glob, see [`glob_match`]), `label`
/// appears as a choice in the [`Prompt::FileActions`] modal and `command` is
/// run on activation. `command` is a shell line with placeholders substituted
/// by [`substitute_command`]; `terminal` decides whether it runs in the
/// background (`false`, default) or in a terminal window (`true`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct FileAction {
    pub pattern: String,
    pub label: String,
    pub command: String,
    #[serde(default)]
    pub terminal: bool,
}

/// One row in the [`Prompt::FileActions`] modal. `OpenDefault` is the built-in
/// "open in the OS default app" choice (always first); `Custom` wraps a
/// matched [`FileAction`] (label + raw command template + terminal flag).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChoice {
    OpenDefault,
    Custom {
        label: String,
        command: String,
        terminal: bool,
    },
}

impl FileChoice {
    /// Display label for the choice's primary line.
    pub fn label(&self) -> &str {
        match self {
            FileChoice::OpenDefault => "Open with default application",
            FileChoice::Custom { label, .. } => label,
        }
    }
}

/// Case-insensitive glob match of `name` against `pattern`, supporting `*`
/// (any run, including empty) and `?` (exactly one char). Everything else is
/// a literal. The whole name must match (the pattern is implicitly anchored at
/// both ends), so `*.md` matches `notes.md` but `*.md` does not match `a.mdx`.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    // Classic two-pointer wildcard match with backtracking on the last `*`.
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            // Mismatch: let the last `*` swallow one more char and retry.
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    // Trailing `*`s in the pattern match the empty remainder.
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// The configured file actions whose `pattern` matches `name`, in config order.
pub fn matching_file_actions<'a>(actions: &'a [FileAction], name: &str) -> Vec<&'a FileAction> {
    actions
        .iter()
        .filter(|a| glob_match(&a.pattern, name))
        .collect()
}

/// Whether any configured action matches `name` — drives the pane row
/// highlight (a colored name) so the user knows Enter offers more than the
/// default open.
pub fn file_has_custom_action(actions: &[FileAction], name: &str) -> bool {
    actions.iter().any(|a| glob_match(&a.pattern, name))
}

/// Build the choice list for the [`Prompt::FileActions`] modal: the built-in
/// default-open first, then every matching custom action in config order.
pub fn build_file_choices(actions: &[FileAction], name: &str) -> Vec<FileChoice> {
    let mut choices = vec![FileChoice::OpenDefault];
    for a in matching_file_actions(actions, name) {
        choices.push(FileChoice::Custom {
            label: a.label.clone(),
            command: a.command.clone(),
            terminal: a.terminal,
        });
    }
    choices
}

/// A `quick_view` entry from `~/.rho.yaml`: when the cursor sits on a file
/// matching `pattern`, `label` and the output of `command` are shown live in
/// the *opposite* pane's bottom half. `command` is optional — when omitted,
/// the file's raw contents are shown instead of running anything. `command`
/// (when present) is a shell line with placeholders substituted by
/// [`substitute_command`], same as [`FileAction`], minus the `terminal` flag
/// (there is no terminal window here — the output is always captured).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct QuickViewAction {
    pub pattern: String,
    pub label: String,
    #[serde(default)]
    pub command: Option<String>,
}

/// The first configured `quick_view` entry whose `pattern` matches `name`, in
/// config order. Unlike [`matching_file_actions`] (which lists every match for
/// a chooser modal), only one quick-view output can be shown at a time, so the
/// first match wins — this is what lets a catch-all `pattern: "*"` entry act
/// as a fallback after more specific patterns.
pub fn matching_quick_view<'a>(actions: &'a [QuickViewAction], name: &str) -> Option<&'a QuickViewAction> {
    actions.iter().find(|a| glob_match(&a.pattern, name))
}

/// State of the live preview shown in the pane opposite `source_side` (see
/// `App::refresh_quick_view` / `view_pane`). `request_id` and `source_side`
/// let stale debounce fires or task completions (cursor moved on since) be
/// dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickViewState {
    pub request_id: u64,
    pub source_side: Side,
    pub path: std::path::PathBuf,
    pub label: String,
    pub command: Option<String>,
    pub output: QuickViewOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickViewOutput {
    Loading,
    Ready(String),
    Error(String),
}

/// POSIX single-quote a string for safe interpolation into a `sh` command
/// line. Embeds the input verbatim except for `'`, which becomes the canonical
/// `'\''` close-reopen sequence. Single-quoting neutralises `$`, `` ` ``, `"`,
/// `\`, spaces and globs — everything except `'` passes through literally.
pub fn posix_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Expand the placeholders in a custom-action command template against `path`:
///
/// - `{file}` → file name with extension (`report.md`)
/// - `{stem}` → file name without its final extension (`report`)
/// - `{ext}`  → final extension without the dot (`md`; empty if none)
/// - `{path}` → the absolute path as given
/// - `{dir}`  → the parent directory path (empty if none)
///
/// Each substituted value is **POSIX-shell-quoted** ([`posix_quote`]), so file
/// names containing spaces or shell metacharacters (`$(…)`, `;`, `'`, …) are
/// passed as a single literal argument and can't inject commands — the
/// template author must therefore NOT add their own quotes around a
/// placeholder. The surrounding template text is left untouched, so pipes,
/// redirects and `&&` in the template still work. The command runs with the
/// file's parent directory as the working directory.
///
/// Substitution is a single left-to-right pass, so a value that happens to
/// contain a placeholder-looking substring (a file literally named
/// `a{ext}b`) is never re-expanded. Unknown `{…}` tokens are passed through
/// verbatim.
pub fn substitute_command(template: &str, path: &Path) -> String {
    let file = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let path_str = path.to_str().unwrap_or("");
    let dir = path.parent().and_then(|p| p.to_str()).unwrap_or("");

    substitute_tokens(template, |token| match token {
        "file" => Some(posix_quote(file)),
        "stem" => Some(posix_quote(stem)),
        "ext" => Some(posix_quote(ext)),
        "path" => Some(posix_quote(path_str)),
        "dir" => Some(posix_quote(dir)),
        _ => None,
    })
}

/// Multi-selection variant of [`substitute_command`]. `{file}` expands to
/// *every* path in `paths` — each POSIX-shell-quoted, joined by single
/// spaces — so a `quick_view` command runs against a whole selection
/// (e.g. `shasum` over three marked files). `{path}` likewise expands to
/// every absolute path. The single-valued placeholders (`{stem}`, `{ext}`,
/// `{dir}`) resolve against `primary` — the file under the cursor — since
/// they have no meaningful multi-file expansion. With one path this behaves
/// exactly like `substitute_command`.
pub fn substitute_command_multi(template: &str, paths: &[PathBuf], primary: &Path) -> String {
    let join_quoted = |f: &dyn Fn(&Path) -> &str| {
        paths
            .iter()
            .map(|p| posix_quote(f(p)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let files = join_quoted(&|p: &Path| p.file_name().and_then(|s| s.to_str()).unwrap_or(""));
    let full_paths = join_quoted(&|p: &Path| p.to_str().unwrap_or(""));
    let stem = primary.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let ext = primary.extension().and_then(|s| s.to_str()).unwrap_or("");
    let dir = primary.parent().and_then(|p| p.to_str()).unwrap_or("");

    substitute_tokens(template, |token| match token {
        "file" => Some(files.clone()),
        "path" => Some(full_paths.clone()),
        "stem" => Some(posix_quote(stem)),
        "ext" => Some(posix_quote(ext)),
        "dir" => Some(posix_quote(dir)),
        _ => None,
    })
}

/// Shared brace-scanning core for the `substitute_command*` family. Walks
/// `template` left-to-right; for each `{token}` it calls `resolve(token)` and
/// splices in the returned string (which the resolver has already quoted as
/// needed). `resolve` returning `None` — an unknown token — emits the literal
/// `{token}` untouched so the user sees their typo rather than a silent
/// deletion. A single pass means a substituted value that itself contains
/// `{…}` is never re-expanded; an unterminated `{` is emitted verbatim.
fn substitute_tokens(template: &str, resolve: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        // Look for the closing brace after this '{'.
        match rest[open + 1..].find('}') {
            Some(rel_close) => {
                let token = &rest[open + 1..open + 1 + rel_close];
                match resolve(token) {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('{');
                        out.push_str(token);
                        out.push('}');
                    }
                }
                rest = &rest[open + 1 + rel_close + 1..];
            }
            // No closing brace: emit the rest (including this '{') and stop.
            None => {
                out.push_str(&rest[open..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

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
    /// Same shape as `Copy` — pre-filled destination path, no
    /// confirmation. The submit handler runs `move_task` instead of
    /// `copy_task`.
    Move {
        input: String,
    },
    /// `NewFolder`: name for a directory to create inside the active pane's
    /// current (local) folder. Submit runs `make_dir` and reloads. Starts
    /// empty so the user just types a name.
    NewFolder {
        input: String,
    },
    /// `NewFile`: name for an empty file to create inside the active pane's
    /// current (local) folder. Pre-filled with `file.txt`. Submit runs
    /// `make_file`, opens the new file in the editor, and reloads.
    NewFile {
        input: String,
    },
    /// `Compress`: pre-filled destination zip path (defaults to other-pane
    /// + first-mark-stem.zip). On submit, runs `zip -r` on the marks.
    Compress {
        input: String,
    },
    /// `Uncompress`: pre-filled destination directory (defaults to other
    /// pane). On submit, runs `unzip` / `tar -xzf` per marked archive.
    Uncompress {
        input: String,
    },
    Delete {
        /// Backend-aware list — local entries hold a `PathBuf`, remote
        /// entries carry their backend alias so the confirm UI can show
        /// `host:/path` and the dispatch hits the right transport.
        paths: Vec<Location>,
        focus: DeleteFocus,
    },
    /// User pressed Enter on a `.zip` larger than the threshold — ask
    /// before unpacking it into `/tmp` and browsing. Reuses [`DeleteFocus`]
    /// for the Cancel/Confirm button state.
    ConfirmLargeExtract {
        archive_path: PathBuf,
        size_bytes: u64,
        focus: DeleteFocus,
    },
    /// Filesystem watcher noticed new files in a watched folder; ask the user
    /// whether to open the folder in one of the panes.
    NewFiles {
        folder: PathBuf,
        files: Vec<String>,
        focus: NewFilesFocus,
    },
    /// Action picker with a filter input. `actions` is the slate of
    /// currently-listed actions, captured at open time (so runtime gating
    /// like Git: branch availability is stable across the modal session).
    /// `selected` is an index into the *filtered* list (i.e.
    /// `filtered_actions(&actions, &input)`).
    ///
    /// `dropbox_configured` is captured at open time so the renderer can show
    /// `OpenDropbox` as a disabled (greyed, non-activatable) row when no
    /// Dropbox credentials are present — see [`palette_action_enabled`]. We
    /// keep the row visible-but-disabled rather than hiding it so users
    /// discover the feature exists.
    CommandPalette {
        input: String,
        selected: usize,
        actions: Vec<PaletteAction>,
        dropbox_configured: bool,
    },
    /// "Docker containers" action. Shows the currently-running containers
    /// from `docker ps`, each with Kill and Shell buttons. The state goes
    /// Loading → Loaded(list) / Error(msg); kills re-fetch the list. `input`
    /// is the filter text — substring-matched against name + image.
    /// `sort_by` / `sort_dir` drive the clickable column headers.
    Docker {
        state: DockerState,
        input: String,
        sort_by: DockerSortBy,
        sort_dir: SortDir,
    },
    /// "Processes" action. Same shape as Docker but backed by `ps -axo …`.
    /// `input` filters by process name; per-row action is `Kill` (SIGTERM).
    /// Defaults to sorting by CPU descending. `selected` is the
    /// keyboard-navigable highlight (↑/↓), and Enter kills that row.
    Processes {
        state: ProcessesState,
        input: String,
        sort_by: ProcessSortBy,
        sort_dir: SortDir,
        selected: usize,
    },
    /// "Launch Application" action (macOS only). Lists `.app` bundles under
    /// `/Applications` (+ Utilities + `~/Applications`). `selected` is the
    /// keyboard-navigable highlight, used by ↑/↓ + Enter to launch.
    Apps {
        state: AppsState,
        input: String,
        selected: usize,
    },
    /// "Git: branch" action. Lists local branches of the repo containing
    /// `repo_path` (the active pane's path at open time), most-recent commit
    /// first. Per-row action is `Checkout`.
    GitBranches {
        state: GitBranchesState,
        input: String,
        selected: usize,
        repo_path: PathBuf,
    },
    /// Static informational modal — lists keyboard shortcuts sourced from
    /// [`keyboard_shortcuts`]. No interactive state; Esc dismisses.
    KeyboardShortcuts,
    /// "Connect to SSH server" action. Lists `Host` entries from
    /// `~/.ssh/config` (skipping pure-wildcard blocks). Per-row action is
    /// `Connect` — opens a new terminal window running `ssh <alias>`.
    SshServers {
        state: SshServersState,
        input: String,
        selected: usize,
    },
    /// "Open file" chooser, shown when Enter is pressed on a (non-archive)
    /// file. `choices[0]` is always [`FileChoice::OpenDefault`]; any remaining
    /// entries are config-defined [`FileChoice::Custom`] actions whose glob
    /// matched the file's name. `selected` is the keyboard highlight (↑/↓),
    /// Enter activates it. `path` is the absolute file path the action runs on.
    /// `edit` is `None` normally; `Tab` on a `Custom` row expands it and fills
    /// `edit` with the substituted command, editable before running (Enter
    /// submits the edited text verbatim instead of re-substituting the
    /// template). Moving the highlight clears `edit`.
    FileActions {
        path: PathBuf,
        choices: Vec<FileChoice>,
        selected: usize,
        edit: Option<String>,
    },
    /// Connection details for the currently-running FTP server (the singleton
    /// rooted at `info.root`). Buttons: Stop server (tears it down), Close
    /// (dismiss the modal; server keeps running). Used both for the
    /// just-started server (right after `decide_ftp_action` → `StartFresh`)
    /// and as a re-show when the palette action is invoked again from inside
    /// the same root.
    FtpInfo {
        info: FtpServerInfo,
        focus: FtpInfoFocus,
    },
    /// User invoked "FTP server" while one is already running rooted at a
    /// *different* folder. Ask before tearing the old one down. `new_root` is
    /// the active pane's current path that the new server would be rooted at.
    FtpReplace {
        current: FtpServerInfo,
        new_root: PathBuf,
        focus: FtpReplaceFocus,
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

/// Fold a freshly-detected batch of new files into the queue of pending
/// `NewFiles` prompts. Bursts for a folder already queued extend that
/// entry's file list instead of adding a second queued popup, so a folder
/// that keeps receiving new files grows one modal instead of stacking many.
pub fn merge_pending_new_files(
    pending: &mut VecDeque<(PathBuf, Vec<String>)>,
    folder: PathBuf,
    files: Vec<String>,
) {
    if let Some(existing) = pending.iter_mut().find(|(f, _)| *f == folder) {
        existing.1.extend(files);
    } else {
        pending.push_back((folder, files));
    }
}

/// Connection details for a running in-app FTP server. Built by
/// `fs_ops::ftp_start_task` when the server binds, then carried in
/// [`Prompt::FtpInfo`] / [`Prompt::FtpReplace`] and on `App.ftp_server` for
/// the status bar. `username` / `password` are `Some` for the
/// [`FtpAuthMode::Generated`](crate::config::FtpAuthMode) mode and `None`
/// when [`FtpAuthMode::Anonymous`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtpServerInfo {
    /// What we actually bound — `0.0.0.0:2121`, `127.0.0.1:2121`, etc.
    pub bind: std::net::SocketAddr,
    /// Best-effort display hostname for the modal: the local IPv4 if `bind`
    /// is the unspecified `0.0.0.0` and we could resolve one, otherwise the
    /// literal IP we bound (loopback or the configured address).
    pub display_host: String,
    /// The pane folder the server was started on. Identity for
    /// [`decide_ftp_action`].
    pub root: PathBuf,
    pub username: Option<String>,
    pub password: Option<String>,
    pub permissions: crate::config::FtpPerms,
    pub started_at: SystemTime,
}

impl FtpServerInfo {
    /// `host:port` for the modal — pairs `display_host` with the bound port.
    /// Cheap and infallible; no `&self` borrow into the formatter chain.
    pub fn display_addr(&self) -> String {
        format!("{}:{}", self.display_host, self.bind.port())
    }
}

/// Severity of an [`FtpLogEntry`]. Drives the per-row color in the modal —
/// `Info` is plain, `Warn` amber, `Error` red. `Auth` is split out from
/// `Info` so successful logins read as a normal connection event while bad
/// passwords surface as a warning the user can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtpLogLevel {
    Info,
    Auth,
    Warn,
    Error,
}

/// One line in the streaming FTP log shown in the [`Prompt::FtpInfo`]
/// modal. Built by the listeners in `fs_ops` and pushed onto the App's
/// capped ring buffer via [`crate::Message::FtpLogEvent`].
#[derive(Debug, Clone)]
pub struct FtpLogEntry {
    /// When the event was produced. Used to render the leading timestamp.
    pub ts: SystemTime,
    pub level: FtpLogLevel,
    /// Free-form message. Format is `<verb> <subject>` where possible (e.g.
    /// "GET /readme.md (1.2 kB)") so a scan of the log reads as a sequence
    /// of FTP-style commands rather than as a debug log.
    pub message: String,
}

/// Format `ts` as `HH:MM:SS` in the local timezone. Used as the leading
/// column of every log row in the FTP info modal. Stays here in `domain`
/// (rather than next to the renderer) so the formatting can be unit-tested
/// without an iced runtime.
pub fn format_log_timestamp(ts: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = ts.into();
    dt.format("%H:%M:%S").to_string()
}

/// Which button is currently highlighted in the [`Prompt::FtpInfo`] modal.
/// `Close` is the default landing focus — the safe action, matches the
/// `Delete` modal's "Cancel-is-default" convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtpInfoFocus {
    Stop,
    Close,
}

impl FtpInfoFocus {
    pub fn toggle(self) -> Self {
        match self {
            FtpInfoFocus::Stop => FtpInfoFocus::Close,
            FtpInfoFocus::Close => FtpInfoFocus::Stop,
        }
    }
}

/// Which button is highlighted in the [`Prompt::FtpReplace`] modal. `Cancel`
/// is the default landing focus — replacing a running server is destructive
/// (drops live connections) so the safe option wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtpReplaceFocus {
    Cancel,
    Replace,
}

impl FtpReplaceFocus {
    pub fn toggle(self) -> Self {
        match self {
            FtpReplaceFocus::Cancel => FtpReplaceFocus::Replace,
            FtpReplaceFocus::Replace => FtpReplaceFocus::Cancel,
        }
    }
}

/// What invoking the "FTP server" palette action should do, given the
/// current server state and the active pane's path. Encapsulates the
/// three-way decision (start fresh / show info / ask to replace) so it can
/// be unit-tested without dragging the iced runtime in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FtpAction {
    /// No server is running — start one rooted at this path.
    StartFresh(PathBuf),
    /// A server is already running at this exact path — just (re-)show the
    /// info modal.
    ShowInfo,
    /// A server is running on some *other* path; ask the user whether to
    /// stop it and start a new one here.
    AskReplace(PathBuf),
}

/// Decide what should happen when the "FTP server" palette action fires.
/// `current` is the running server's info (if any); `pane_root` is the
/// active pane's directory at invocation time. Paths are compared
/// literally — we don't canonicalize, since panes always store canonical
/// paths anyway and we'd rather not block on a syscall here.
pub fn decide_ftp_action(current: Option<&FtpServerInfo>, pane_root: &Path) -> FtpAction {
    match current {
        None => FtpAction::StartFresh(pane_root.to_path_buf()),
        Some(info) if info.root == pane_root => FtpAction::ShowInfo,
        Some(_) => FtpAction::AskReplace(pane_root.to_path_buf()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    Copy,
    Move,
    Delete,
    Compress,
    Uncompress,
    DockerContainers,
    Processes,
    /// Always present as a variant so `match` arms stay exhaustive without
    /// `cfg` noise, but only listed in [`PaletteAction::ALL`] on macOS — on
    /// other platforms the palette never offers it.
    LaunchApplication,
    /// Runtime-gated: only offered when the active pane is inside a git
    /// repository (see [`available_palette_actions`]).
    GitBranch,
    SshConnect,
    /// Always listed, but rendered disabled unless Dropbox credentials are
    /// present in `~/.rho.yaml` (see [`palette_action_enabled`]). Points the
    /// active pane at the Dropbox account root.
    OpenDropbox,
    OpenClaudeCode,
    /// Open a terminal window in the active pane's folder. The terminal app
    /// is taken from the `terminal_app` setting in `~/.rho.yaml`.
    OpenTerminal,
    /// Open the active pane's folder in an editor. The editor binary is taken
    /// from the `folder_editor` setting in `~/.rho.yaml` (defaults to the VS
    /// Code CLI).
    OpenInEditor,
    /// Start (or manage) the in-app FTP server rooted at the active pane.
    /// Singleton — invoking with a server already running on a different
    /// folder pops the Replace-confirm modal; on the same folder it just
    /// re-shows the FTP info modal. See [`decide_ftp_action`].
    FtpServer,
    /// Reveal the active pane's folder in Finder (`open <path>`). Always
    /// present as a variant so `match` arms stay exhaustive, but only listed
    /// in [`PaletteAction::ALL`] on macOS.
    RevealInFinder,
    /// Prompt for a filename (default `file.txt`), create the empty file in
    /// the active pane's local folder, and open it in the editor.
    NewFile,
    KeyboardShortcuts,
    Exit,
}

impl PaletteAction {
    #[cfg(target_os = "macos")]
    pub const ALL: &'static [PaletteAction] = &[
        PaletteAction::Copy,
        PaletteAction::Move,
        PaletteAction::Delete,
        PaletteAction::Compress,
        PaletteAction::Uncompress,
        PaletteAction::DockerContainers,
        PaletteAction::Processes,
        PaletteAction::LaunchApplication,
        PaletteAction::GitBranch,
        PaletteAction::SshConnect,
        PaletteAction::OpenDropbox,
        PaletteAction::OpenClaudeCode,
        PaletteAction::OpenTerminal,
        PaletteAction::OpenInEditor,
        PaletteAction::FtpServer,
        PaletteAction::RevealInFinder,
        PaletteAction::NewFile,
        PaletteAction::KeyboardShortcuts,
        PaletteAction::Exit,
    ];

    #[cfg(not(target_os = "macos"))]
    pub const ALL: &'static [PaletteAction] = &[
        PaletteAction::Copy,
        PaletteAction::Move,
        PaletteAction::Delete,
        PaletteAction::Compress,
        PaletteAction::Uncompress,
        PaletteAction::DockerContainers,
        PaletteAction::Processes,
        PaletteAction::GitBranch,
        PaletteAction::SshConnect,
        PaletteAction::OpenDropbox,
        PaletteAction::OpenClaudeCode,
        PaletteAction::OpenTerminal,
        PaletteAction::OpenInEditor,
        PaletteAction::FtpServer,
        PaletteAction::NewFile,
        PaletteAction::KeyboardShortcuts,
        PaletteAction::Exit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PaletteAction::Copy => "Copy",
            PaletteAction::Move => "Move",
            PaletteAction::Delete => "Delete",
            PaletteAction::Compress => "Compress",
            PaletteAction::Uncompress => "Uncompress",
            PaletteAction::DockerContainers => "Docker containers",
            PaletteAction::Processes => "Processes",
            PaletteAction::LaunchApplication => "Launch Application",
            PaletteAction::GitBranch => "Git: branch",
            PaletteAction::SshConnect => "Connect to SSH server",
            PaletteAction::OpenDropbox => "Open Dropbox",
            PaletteAction::OpenClaudeCode => "Open Claude Code in this folder",
            PaletteAction::OpenTerminal => "Open Terminal in this folder",
            PaletteAction::OpenInEditor => "Open folder in editor",
            PaletteAction::FtpServer => "FTP server",
            PaletteAction::RevealInFinder => "Open folder in Finder",
            PaletteAction::NewFile => "New file",
            PaletteAction::KeyboardShortcuts => "Keyboard shortcuts",
            PaletteAction::Exit => "Exit",
        }
    }
}

/// Archive formats the Uncompress action understands. Detected via file
/// extension by [`detect_archive_format`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
}

/// Recognize `.zip`, `.tar.gz`, and `.tgz` by extension. Case-insensitive.
pub fn detect_archive_format(filename: &str) -> Option<ArchiveFormat> {
    let lower = filename.to_lowercase();
    if lower.ends_with(".zip") {
        Some(ArchiveFormat::Zip)
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        Some(ArchiveFormat::TarGz)
    } else {
        None
    }
}

/// Default filename for the Compress modal — takes the stem of the first
/// marked path and appends `.zip`. Falls back to `archive.zip` for an empty
/// list. The "stem" strips the longest extension we can (so
/// `report.tar.gz` → `report`, not `report.tar`).
pub fn default_zip_filename(srcs: &[PathBuf]) -> String {
    let Some(first) = srcs.first() else {
        return "archive.zip".to_string();
    };
    let name = first
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".to_string());
    // Strip the longest extension we recognize, then anything after the
    // last `.`. E.g. report.tar.gz → report; image.jpeg → image; LICENSE
    // (no extension) stays LICENSE.
    let lower = name.to_lowercase();
    let stem = if lower.ends_with(".tar.gz") {
        &name[..name.len() - 7]
    } else if lower.ends_with(".tar.bz2") {
        &name[..name.len() - 8]
    } else if let Some(dot) = name.rfind('.') {
        if dot > 0 {
            &name[..dot]
        } else {
            name.as_str()
        }
    } else {
        name.as_str()
    };
    format!("{}.zip", stem)
}

/// Static reference for the in-app "Keyboard shortcuts" modal. Mirror of the
/// docs at `docs/src/keybindings.md`; keep both in sync when adding bindings.
/// Returns a list of `(section_title, bindings)` pairs.
pub fn keyboard_shortcuts() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        (
            "Navigation",
            vec![
                ("↑ / ↓", "Move the cursor one row"),
                ("PageUp / PageDown", "Move by one page"),
                ("Shift + arrow / page", "Extend the selection from the anchor"),
                ("Tab", "Switch the active pane"),
                (
                    "Enter",
                    "Open the cursor row (directory: descend; file: OS default)",
                ),
                (
                    "Backspace",
                    "Go to parent (or delete one char of an active filter)",
                ),
            ],
        ),
        (
            "File actions",
            vec![
                ("F4", "Edit the cursor file in $VISUAL / $EDITOR"),
                ("Space", "Quick Look preview (macOS only)"),
                ("F5", "Open the copy modal"),
                ("F6", "Open the move modal"),
                ("F7", "Create a new folder in the active pane"),
                ("F8 / Delete", "Open the delete-confirm modal"),
                ("F10", "Quit the app immediately"),
            ],
        ),
        (
            "Filtering & sorting",
            vec![
                ("any printable char", "Append to type-to-filter"),
                ("Esc (no modal)", "Clear the active filter"),
                ("click column header", "Sort by that column; click again to flip"),
            ],
        ),
        (
            "Modals & global",
            vec![
                ("⌘P", "Go to folder (filterable list of recent locations)"),
                ("⌘⇧P", "Command palette"),
                ("⌘,", "Open ~/.rho.yaml in the OS default editor"),
                ("Esc", "Cancel current modal / clear the filter"),
            ],
        ),
    ]
}

/// Subset of [`PaletteAction::ALL`] worth *listing* in the palette.
/// `in_git_repo` is the only thing that hides a row today (GitBranch is
/// contextual — meaningless outside a repo, so it's dropped entirely).
/// Capability-style actions like `OpenDropbox` stay listed regardless of
/// config and are instead greyed out by [`palette_action_enabled`], so users
/// discover them. Cfg-gated variants like LaunchApplication are already
/// absent from `ALL` on non-mac builds, so no extra logic needed there.
pub fn available_palette_actions(in_git_repo: bool) -> Vec<PaletteAction> {
    PaletteAction::ALL
        .iter()
        .copied()
        .filter(|a| match a {
            PaletteAction::GitBranch => in_git_repo,
            _ => true,
        })
        .collect()
}

/// Whether a listed palette action can actually be activated right now.
/// Listing (in [`available_palette_actions`]) and enablement are separate:
/// `OpenDropbox` is always listed but only activatable once Dropbox
/// credentials are present in `~/.rho.yaml`. Everything else is always
/// enabled once listed.
pub fn palette_action_enabled(action: PaletteAction, dropbox_configured: bool) -> bool {
    match action {
        PaletteAction::OpenDropbox => dropbox_configured,
        _ => true,
    }
}

/// "Scroll into view" math for the selection modals, mirroring the panes'
/// `ensure_active_visible`: given the highlighted row index, the uniform row
/// height, the viewport height, and the current scroll offset, return the new
/// offset needed to bring the row fully into view — or `None` if it's already
/// visible. Returning `None` is the point: the list stays put on every keypress
/// and only scrolls when the cursor would leave the page. Scrolling up snaps
/// the row to the top edge; scrolling down snaps it to the bottom edge
/// (clamped at 0). Used by `main::modal_scroll_to_selected`.
pub fn modal_scroll_target(selected: usize, row_h: f32, viewport_h: f32, scroll_y: f32) -> Option<f32> {
    let row_top = selected as f32 * row_h;
    let row_bottom = row_top + row_h;
    let view_bottom = scroll_y + viewport_h;
    if row_top < scroll_y {
        Some(row_top)
    } else if row_bottom > view_bottom {
        Some((row_bottom - viewport_h).max(0.0))
    } else {
        None
    }
}

/// Snapshot of a single running container surfaced from `docker ps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
}

/// Sortable columns in the Docker containers modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerSortBy {
    Name,
    Image,
    Status,
}

impl DockerSortBy {
    pub fn label(self) -> &'static str {
        match self {
            DockerSortBy::Name => "Name",
            DockerSortBy::Image => "Image",
            DockerSortBy::Status => "Status",
        }
    }

    /// Direction applied when a column is clicked while it isn't already the
    /// active sort column. Text columns lean ascending — clicking "Name"
    /// from a blank slate should give an A-Z list.
    pub fn initial_dir(self) -> SortDir {
        SortDir::Asc
    }
}

pub fn sort_containers(containers: &mut [DockerContainer], by: DockerSortBy, dir: SortDir) {
    use std::cmp::Reverse;
    match (by, dir) {
        (DockerSortBy::Name, SortDir::Asc) => {
            containers.sort_by_cached_key(|c| c.name.to_lowercase());
        }
        (DockerSortBy::Name, SortDir::Desc) => {
            containers.sort_by_cached_key(|c| Reverse(c.name.to_lowercase()));
        }
        (DockerSortBy::Image, SortDir::Asc) => {
            containers.sort_by_cached_key(|c| c.image.to_lowercase());
        }
        (DockerSortBy::Image, SortDir::Desc) => {
            containers.sort_by_cached_key(|c| Reverse(c.image.to_lowercase()));
        }
        (DockerSortBy::Status, SortDir::Asc) => {
            containers.sort_by_cached_key(|c| c.status.to_lowercase());
        }
        (DockerSortBy::Status, SortDir::Desc) => {
            containers.sort_by_cached_key(|c| Reverse(c.status.to_lowercase()));
        }
    }
}

/// Lifecycle of the Docker containers modal. Drives both the loading
/// placeholder and the post-load list / error states.
#[derive(Debug, Clone)]
pub enum DockerState {
    Loading,
    Loaded(Vec<DockerContainer>),
    Error(String),
}

/// Snapshot of one process from `ps`. Percentages are wall-clock CPU usage
/// and RSS as a fraction of total RAM, both as reported by `ps` at the
/// moment of the call.
#[derive(Debug, Clone, PartialEq)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub mem_percent: f32,
}

/// Lifecycle of the Processes modal. Same shape as [`DockerState`].
#[derive(Debug, Clone)]
pub enum ProcessesState {
    Loading,
    Loaded(Vec<Process>),
    Error(String),
}

/// macOS `.app` bundle discovered under one of the application directories.
/// `icon` is reserved for a future change — v1 leaves it `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    pub path: PathBuf,
    pub name: String,
}

/// Lifecycle of the Launch Application modal. Same shape as [`DockerState`]
/// and [`ProcessesState`].
#[derive(Debug, Clone)]
pub enum AppsState {
    Loading,
    Loaded(Vec<Application>),
    Error(String),
}

/// One local branch returned by `git for-each-ref`. `last_commit` is the
/// formatted date string for the tip of the branch (already short — typically
/// `YYYY-MM-DD`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBranch {
    pub name: String,
    pub last_commit: String,
}

/// Lifecycle of the Git: branch modal. Same shape as [`DockerState`] etc.
#[derive(Debug, Clone)]
pub enum GitBranchesState {
    Loading,
    Loaded(Vec<GitBranch>),
    Error(String),
}

/// A single `Host` entry parsed out of `~/.ssh/config`. Only the four fields
/// the modal cares about are kept; everything else (ProxyJump, Port, etc.) is
/// preserved by `ssh` itself when we shell out — we don't need it here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshServer {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub identity_file: Option<String>,
}

/// Lifecycle of the Connect to SSH server modal. Same shape as the others.
#[derive(Debug, Clone)]
pub enum SshServersState {
    Loading,
    Loaded(Vec<SshServer>),
    Error(String),
}

/// Sortable columns in the Processes modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSortBy {
    Name,
    Pid,
    Cpu,
    Mem,
}

impl ProcessSortBy {
    pub fn label(self) -> &'static str {
        match self {
            ProcessSortBy::Name => "Name",
            ProcessSortBy::Pid => "PID",
            ProcessSortBy::Cpu => "CPU",
            ProcessSortBy::Mem => "MEM",
        }
    }

    /// Default direction when switching to this column. Numeric columns
    /// default to descending so "click CPU" lists the heaviest first; the
    /// Name column defaults to A-Z.
    pub fn initial_dir(self) -> SortDir {
        match self {
            ProcessSortBy::Name => SortDir::Asc,
            ProcessSortBy::Pid | ProcessSortBy::Cpu | ProcessSortBy::Mem => SortDir::Desc,
        }
    }
}

pub fn sort_processes(processes: &mut [Process], by: ProcessSortBy, dir: SortDir) {
    use std::cmp::Ordering;
    use std::cmp::Reverse;
    // f32 isn't Ord, so partial_cmp + NaN guard.
    let cmp_f32 = |a: f32, b: f32| a.partial_cmp(&b).unwrap_or(Ordering::Equal);
    match (by, dir) {
        (ProcessSortBy::Name, SortDir::Asc) => {
            processes.sort_by_cached_key(|p| p.name.to_lowercase());
        }
        (ProcessSortBy::Name, SortDir::Desc) => {
            processes.sort_by_cached_key(|p| Reverse(p.name.to_lowercase()));
        }
        (ProcessSortBy::Pid, SortDir::Asc) => processes.sort_by_key(|p| p.pid),
        (ProcessSortBy::Pid, SortDir::Desc) => processes.sort_by_key(|p| Reverse(p.pid)),
        (ProcessSortBy::Cpu, SortDir::Asc) => {
            processes.sort_by(|a, b| cmp_f32(a.cpu_percent, b.cpu_percent));
        }
        (ProcessSortBy::Cpu, SortDir::Desc) => {
            processes.sort_by(|a, b| cmp_f32(b.cpu_percent, a.cpu_percent));
        }
        (ProcessSortBy::Mem, SortDir::Asc) => {
            processes.sort_by(|a, b| cmp_f32(a.mem_percent, b.mem_percent));
        }
        (ProcessSortBy::Mem, SortDir::Desc) => {
            processes.sort_by(|a, b| cmp_f32(b.mem_percent, a.mem_percent));
        }
    }
}

/// Parse output of `docker ps --format '{{.ID}}|{{.Names}}|{{.Image}}|{{.Status}}'`.
/// Lines with the wrong number of fields are skipped silently — we'd rather
/// show a short list than break the modal on a malformed line.
pub fn parse_docker_ps(stdout: &str) -> Vec<DockerContainer> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, '|');
            let id = parts.next()?.trim().to_string();
            let name = parts.next()?.trim().to_string();
            let image = parts.next()?.trim().to_string();
            let status = parts.next()?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            Some(DockerContainer {
                id,
                name,
                image,
                status,
            })
        })
        .collect()
}

/// Parse output of `ps -axo pid=,pcpu=,pmem=,comm=`. The command name is the
/// last column so anything past the third whitespace block is treated as
/// the name, even if it contains spaces (Mac process names sometimes do).
/// Returns the list sorted by CPU descending so heavy hitters surface first.
pub fn parse_ps_output(stdout: &str) -> Vec<Process> {
    let mut out: Vec<Process> = stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid_str, rest) = line.split_once(char::is_whitespace)?;
            let rest = rest.trim_start();
            let (cpu_str, rest) = rest.split_once(char::is_whitespace)?;
            let rest = rest.trim_start();
            let (mem_str, name) = rest.split_once(char::is_whitespace)?;
            let pid: u32 = pid_str.parse().ok()?;
            let cpu_percent: f32 = cpu_str.parse().ok()?;
            let mem_percent: f32 = mem_str.parse().ok()?;
            let name = name.trim().to_string();
            if name.is_empty() {
                return None;
            }
            Some(Process {
                pid,
                name,
                cpu_percent,
                mem_percent,
            })
        })
        .collect();
    // Descending CPU. partial_cmp on f32 can be None for NaN — fall back to
    // Equal so we get a total order even on weird inputs.
    out.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Case-insensitive substring filter over container name + image.
pub fn filtered_containers<'a>(
    containers: &'a [DockerContainer],
    input: &str,
) -> Vec<&'a DockerContainer> {
    if input.is_empty() {
        return containers.iter().collect();
    }
    let needle = input.to_lowercase();
    containers
        .iter()
        .filter(|c| {
            c.name.to_lowercase().contains(&needle) || c.image.to_lowercase().contains(&needle)
        })
        .collect()
}

/// Case-insensitive substring filter over process name.
pub fn filtered_processes<'a>(processes: &'a [Process], input: &str) -> Vec<&'a Process> {
    if input.is_empty() {
        return processes.iter().collect();
    }
    let needle = input.to_lowercase();
    processes
        .iter()
        .filter(|p| p.name.to_lowercase().contains(&needle))
        .collect()
}

/// Sort applications by name ascending (case-insensitive).
pub fn sort_apps(apps: &mut [Application]) {
    apps.sort_by_cached_key(|a| a.name.to_lowercase());
}

/// Case-insensitive substring filter over application names.
pub fn filtered_apps<'a>(apps: &'a [Application], input: &str) -> Vec<&'a Application> {
    if input.is_empty() {
        return apps.iter().collect();
    }
    let needle = input.to_lowercase();
    apps.iter()
        .filter(|a| a.name.to_lowercase().contains(&needle))
        .collect()
}

/// Parse `git for-each-ref --format='%(refname:short)|%(committerdate:short)'`
/// output. The `git for-each-ref --sort=-committerdate` flag is what produces
/// the most-recent-first ordering, so this function preserves arrival order.
pub fn parse_git_branches(stdout: &str) -> Vec<GitBranch> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, '|');
            let name = parts.next()?.trim().to_string();
            let last_commit = parts.next()?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            Some(GitBranch { name, last_commit })
        })
        .collect()
}

/// Case-insensitive substring filter over branch names.
pub fn filtered_branches<'a>(branches: &'a [GitBranch], input: &str) -> Vec<&'a GitBranch> {
    if input.is_empty() {
        return branches.iter().collect();
    }
    let needle = input.to_lowercase();
    branches
        .iter()
        .filter(|b| b.name.to_lowercase().contains(&needle))
        .collect()
}

/// Parse a (very small) subset of `~/.ssh/config`:
///
/// - Each `Host` line opens a block. Multiple patterns on one `Host` line
///   are split on whitespace; the first non-wildcard pattern becomes the
///   entry's alias. Pure-wildcard blocks (e.g. `Host *`) are skipped — they
///   apply to every host and don't represent a specific server.
/// - Inside a block, `HostName`, `User`, `IdentityFile` are tracked.
/// - SSH config keys are case-insensitive.
/// - `Include` directives are ignored in v1.
///
/// Comments (`#` at start of trimmed line) and blank lines are skipped.
pub fn parse_ssh_config(content: &str) -> Vec<SshServer> {
    let mut servers = Vec::new();
    let mut current: Option<SshServer> = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(char::is_whitespace) {
            Some((k, v)) => (k, v.trim()),
            None => continue,
        };
        let key_lower = key.to_ascii_lowercase();

        if key_lower == "host" {
            if let Some(srv) = current.take() {
                servers.push(srv);
            }
            // Pick the first non-wildcard pattern; if none, this whole block
            // is a defaults block and we just don't open a `current`.
            let alias = value
                .split_whitespace()
                .find(|p| !p.contains('*') && !p.contains('?'));
            if let Some(alias) = alias {
                current = Some(SshServer {
                    alias: alias.to_string(),
                    hostname: None,
                    user: None,
                    identity_file: None,
                });
            }
        } else if let Some(srv) = current.as_mut() {
            match key_lower.as_str() {
                "hostname" => srv.hostname = Some(value.to_string()),
                "user" => srv.user = Some(value.to_string()),
                "identityfile" => srv.identity_file = Some(value.to_string()),
                _ => {}
            }
        }
    }
    if let Some(srv) = current.take() {
        servers.push(srv);
    }
    servers
}

/// Sort SSH servers by alias ascending (case-insensitive). The order in
/// `~/.ssh/config` is grouping-driven, not alphabetic, so we re-sort for
/// the modal to make scanning easier.
pub fn sort_servers(servers: &mut [SshServer]) {
    servers.sort_by_cached_key(|s| s.alias.to_lowercase());
}

/// Case-insensitive substring filter — matches alias OR hostname so a user
/// who only remembers the hostname can still find the entry.
pub fn filtered_servers<'a>(servers: &'a [SshServer], input: &str) -> Vec<&'a SshServer> {
    if input.is_empty() {
        return servers.iter().collect();
    }
    let needle = input.to_lowercase();
    servers
        .iter()
        .filter(|s| {
            s.alias.to_lowercase().contains(&needle)
                || s.hostname
                    .as_ref()
                    .map(|h| h.to_lowercase().contains(&needle))
                    .unwrap_or(false)
        })
        .collect()
}

/// Parse the output of `ls -la --time-style=full-iso` into `Entry`s.
///
/// Each non-header line looks like:
/// ```text
/// -rw-r--r-- 1 user group  158 2026-05-22 14:32:01.000000000 +0000 README.md
/// ```
/// Permission char `d` → directory, `l` → symlink (treated as not-dir,
/// name truncated at the ` -> target` suffix). Sizes are only set for
/// regular files, matching the local listing's behaviour.
///
/// Lines that don't fit the expected shape (blank, the leading
/// `total N`, garbled stderr leaking into stdout) are dropped via
/// `filter_map` rather than failing the whole parse.
pub fn parse_ls_la(stdout: &str) -> Vec<Entry> {
    stdout.lines().filter_map(parse_ls_la_line).collect()
}

fn parse_ls_la_line(line: &str) -> Option<Entry> {
    if line.is_empty() || line.starts_with("total ") {
        return None;
    }

    // Find the byte offset of the 9th whitespace-separated field — that's
    // where the filename starts. Walking once gets us both the splitting
    // boundary and lets the name keep any embedded whitespace.
    let mut in_field = false;
    let mut field_idx = 0usize;
    let mut name_start = None;
    for (i, c) in line.char_indices() {
        let is_ws = c.is_whitespace();
        if !in_field && !is_ws {
            in_field = true;
            field_idx += 1;
            if field_idx == 9 {
                name_start = Some(i);
                break;
            }
        } else if in_field && is_ws {
            in_field = false;
        }
    }
    let name_start = name_start?;
    let mut cols = line[..name_start].split_whitespace();
    let perms = cols.next()?;
    let _nlink = cols.next()?;
    let _owner = cols.next()?;
    let _group = cols.next()?;
    let size_str = cols.next()?;
    let date = cols.next()?;
    let time = cols.next()?;
    let tz = cols.next()?;

    let kind = perms.chars().next()?;
    let is_dir = kind == 'd';
    let is_symlink = kind == 'l';

    let rest = &line[name_start..];
    let name = if is_symlink {
        // `link_name -> target` — keep only the link name. If a link
        // name itself contains ` -> ` we'd truncate early; that's a
        // known MVP limitation.
        rest.split(" -> ").next().unwrap_or(rest).to_string()
    } else {
        rest.to_string()
    };
    if name == "." || name == ".." {
        return None;
    }

    let size = if is_dir || is_symlink {
        None
    } else {
        size_str.parse::<u64>().ok()
    };

    // `--time-style=full-iso` emits `YYYY-MM-DD HH:MM:SS.fffffffff ±HHMM`.
    let combined = format!("{} {} {}", date, time, tz);
    let modified = chrono::DateTime::parse_from_str(&combined, "%Y-%m-%d %H:%M:%S%.f %z")
        .ok()
        .and_then(|dt| {
            let secs = dt.timestamp();
            if secs < 0 {
                return None;
            }
            Some(
                std::time::UNIX_EPOCH
                    + std::time::Duration::new(secs as u64, dt.timestamp_subsec_nanos()),
            )
        });

    Some(Entry {
        name,
        is_dir,
        size,
        modified,
    })
}

// ---------------------------------------------------------------------------
// Dropbox API helpers (pure: path mapping + JSON parsing)
// ---------------------------------------------------------------------------

/// Map a pane [`Location`] path onto the string the Dropbox API expects.
/// Dropbox uses `""` for the account root and `/Folder/file` for
/// everything else (always forward slashes, no trailing slash). Our pane
/// paths are `PathBuf`s anchored at `/`, so the root `"/"` becomes `""`.
pub fn dropbox_api_path(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let trimmed = s.trim_end_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{}", trimmed)
    }
}

/// One page of a `list_folder` (or `list_folder/continue`) response,
/// mapped into our [`Entry`] type. `cursor` is `Some` only when the API
/// reported `has_more` — the caller pages with `list_folder/continue`.
#[derive(Debug, Clone, PartialEq)]
pub struct DropboxListing {
    pub entries: Vec<Entry>,
    pub cursor: Option<String>,
}

/// Parse a Dropbox `list_folder` JSON body into [`DropboxListing`].
/// Unknown `.tag`s (e.g. `deleted`) are skipped. Errors carry the Dropbox
/// `error_summary` when the body is an API error rather than a listing.
pub fn parse_dropbox_list(json: &str) -> Result<DropboxListing, String> {
    #[derive(serde::Deserialize)]
    struct Resp {
        #[serde(default)]
        entries: Vec<RawEntry>,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default)]
        has_more: bool,
        #[serde(default)]
        error_summary: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct RawEntry {
        #[serde(rename = ".tag")]
        tag: String,
        name: String,
        #[serde(default)]
        size: Option<u64>,
        #[serde(default)]
        server_modified: Option<String>,
    }

    let resp: Resp = serde_json::from_str(json).map_err(|e| format!("invalid JSON: {}", e))?;
    if let Some(err) = resp.error_summary {
        return Err(err);
    }
    let entries = resp
        .entries
        .into_iter()
        .filter_map(|raw| match raw.tag.as_str() {
            "folder" => Some(Entry {
                name: raw.name,
                is_dir: true,
                size: None,
                modified: None,
            }),
            "file" => Some(Entry {
                name: raw.name,
                is_dir: false,
                size: raw.size,
                modified: raw.server_modified.as_deref().and_then(parse_rfc3339),
            }),
            _ => None,
        })
        .collect();
    Ok(DropboxListing {
        entries,
        cursor: if resp.has_more { resp.cursor } else { None },
    })
}

/// Parse a Dropbox `oauth2/token` response into `(access_token,
/// expires_in_seconds)`. Token-endpoint errors use OAuth's
/// `error` / `error_description` shape, so we surface those instead.
pub fn parse_dropbox_token(json: &str) -> Result<(String, u64), String> {
    #[derive(serde::Deserialize)]
    struct Resp {
        #[serde(default)]
        access_token: Option<String>,
        #[serde(default)]
        expires_in: Option<u64>,
        #[serde(default)]
        error_description: Option<String>,
        #[serde(default)]
        error: Option<String>,
    }
    let resp: Resp =
        serde_json::from_str(json).map_err(|e| format!("invalid token JSON: {}", e))?;
    match resp.access_token {
        // Dropbox access tokens currently last 4 hours; fall back to that
        // if the field is somehow missing.
        Some(token) => Ok((token, resp.expires_in.unwrap_or(14_400))),
        None => Err(resp
            .error_description
            .or(resp.error)
            .unwrap_or_else(|| "token exchange failed".to_string())),
    }
}

/// Extract Dropbox's `error_summary` from an API error body, if present.
/// Used to turn a 4xx/5xx response into a human-readable message.
pub fn dropbox_error_summary(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.get("error_summary")?
        .as_str()
        .map(|s| s.trim_end_matches(|c| c == '.' || c == '/').to_string())
}

/// Parse an RFC 3339 timestamp (Dropbox `server_modified`, e.g.
/// `2015-05-12T15:50:38Z`) into a `SystemTime`. Pre-epoch times → `None`.
fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    chrono::DateTime::parse_from_rfc3339(s).ok().and_then(|dt| {
        let secs = dt.timestamp();
        if secs < 0 {
            return None;
        }
        Some(std::time::UNIX_EPOCH + std::time::Duration::new(secs as u64, dt.timestamp_subsec_nanos()))
    })
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
    /// Where this pane's listing lives — local path or remote backend.
    pub location: Location,
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
    /// The explicit selection set, in *visible* row space (row 0 = ".." is
    /// never included). This is the single source of truth for what's marked:
    /// a plain click/arrow collapses it to just the cursor row, `Shift`
    /// extends it to the contiguous `anchor..=selected` range, and `Cmd+click`
    /// toggles individual rows in and out without disturbing the rest. The
    /// cursor (`selected`) may sit on a row that isn't in this set — e.g. right
    /// after `Cmd+click` deselects the row under it.
    pub marked: BTreeSet<usize>,
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
    /// Whether a `CLAUDE.md` file exists in this directory.
    pub has_claude_md: bool,
    /// Whether a `.claude/` directory exists in this directory.
    pub has_claude_dir: bool,
    /// Name of an entry the cursor should land on as soon as it appears in
    /// the streaming load. Used to focus on a newly-detected file after the
    /// "new files" modal opens its folder, and to re-home the cursor after an
    /// in-place `reload()`. Cleared on `navigate()`, or when the entry has
    /// been located.
    pub pending_focus: Option<String>,
    /// When `pending_focus` resolves, whether to recenter the scroll on it
    /// (`true` — the "jump to a freshly-detected file" case) or leave the
    /// scroll where it is (`false` — an in-place `reload()`, so an external
    /// change doesn't yank the viewport around).
    pub focus_jump: bool,
}

impl Pane {
    pub fn empty(location: Location) -> Self {
        Self {
            location,
            entries: Vec::new(),
            visible_indices: Vec::new(),
            filter: String::new(),
            selected: 0,
            anchor: 0,
            marked: BTreeSet::new(),
            sort_by: SortBy::Name,
            sort_dir: SortDir::Asc,
            scroll_y: 0.0,
            viewport_height: None,
            loading: true,
            load_generation: 0,
            git_info: None,
            has_claude_md: false,
            has_claude_dir: false,
            pending_focus: None,
            focus_jump: false,
        }
    }

    /// Inner path of this pane's location. Safe for purely-syntactic
    /// path operations (display, `join`, `parent`); I/O code must
    /// dispatch on [`Location`] via `self.location.is_local()` /
    /// `matches!(self.location, …)` because the path alone doesn't
    /// carry the backend.
    pub fn path(&self) -> &Path {
        self.location.path()
    }

    /// True iff this pane's directory has a `CLAUDE.md` file *or* a
    /// `.claude/` subdirectory — the marker the in-app info bar lights up
    /// for.
    pub fn has_claude_marker(&self) -> bool {
        self.has_claude_md || self.has_claude_dir
    }

    /// Formatted label for the Claude info bar (e.g. "claude: CLAUDE.md,
    /// .claude/"). Caller is responsible for checking [`Self::has_claude_marker`]
    /// first — an empty marker returns the bare "claude: " prefix.
    pub fn claude_marker_label(&self) -> String {
        let mut parts = Vec::new();
        if self.has_claude_md {
            parts.push("CLAUDE.md");
        }
        if self.has_claude_dir {
            parts.push(".claude/");
        }
        format!("claude: {}", parts.join(", "))
    }

    /// Reset the pane to "about to load `location`". Returns the new
    /// generation that the matching load_dir_task should tag chunks with.
    pub fn navigate(&mut self, location: Location) -> u64 {
        self.location = location;
        self.entries.clear();
        self.visible_indices.clear();
        self.filter.clear();
        self.selected = 0;
        self.anchor = 0;
        self.marked.clear();
        self.loading = true;
        self.load_generation += 1;
        self.git_info = None;
        // Claude markers are reset here so the old pane's bar doesn't briefly
        // bleed into the new directory; the App layer re-stats and sets them
        // right after this returns.
        self.has_claude_md = false;
        self.has_claude_dir = false;
        // A fresh navigation overrides any in-flight focus request. The caller
        // re-sets pending_focus *after* navigate() if it wants one.
        self.pending_focus = None;
        self.focus_jump = false;
        self.load_generation
    }

    /// Reload the current directory *in place* — re-stream a fresh listing
    /// with a new generation, but unlike `navigate()` keep the filter, sort,
    /// and scroll position, and re-home the cursor onto the same entry by name
    /// (via `pending_focus` with `focus_jump = false`, so the viewport doesn't
    /// jump). Used by the folder watcher to refresh after an external change
    /// without disrupting what the user was doing. If the cursor's entry is
    /// gone the cursor falls back to the top.
    pub fn reload(&mut self) -> u64 {
        self.pending_focus = self.cursor_name();
        self.focus_jump = false;
        self.entries.clear();
        self.visible_indices.clear();
        self.selected = 0;
        self.anchor = 0;
        self.marked.clear();
        self.loading = true;
        self.load_generation += 1;
        self.git_info = None;
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

        // When a filter is active, the cursor should land on a real matching
        // entry rather than the synthetic ".." row. If the preserved cursor
        // didn't survive the filter (or there was none), jump to the first
        // match instead of sitting on "..".
        let new_selected = if !self.filter.is_empty()
            && new_selected == 0
            && !self.visible_indices.is_empty()
        {
            1
        } else {
            new_selected
        };

        self.selected = new_selected;
        self.anchor = new_selected;
        // Visible-row indices are only meaningful within a single listing;
        // a filter/sort/reload reshuffles them, so any prior multi-selection
        // is meaningless now. Collapse to the (re-homed) cursor row.
        self.marked.clear();
        if new_selected != 0 {
            self.marked.insert(new_selected);
        }
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
        self.selected = next;
        if extend {
            // Shift-extend: selection becomes the contiguous range from the
            // anchor to the new cursor.
            self.select_range();
        } else {
            // Plain move/click: collapse to a single-row selection.
            self.anchor = next;
            self.marked.clear();
            if next != 0 {
                self.marked.insert(next);
            }
        }
    }

    /// Cmd+click: toggle a single visible row in or out of the selection
    /// without disturbing the others, and re-home the cursor + shift-anchor
    /// onto it so a following arrow or `Shift`-extend starts from here. The
    /// synthetic ".." row (0) is never selectable.
    pub fn toggle_mark(&mut self, row: usize) {
        if row == 0 {
            return;
        }
        if !self.marked.remove(&row) {
            self.marked.insert(row);
        }
        self.selected = row;
        self.anchor = row;
    }

    /// Rebuild `marked` as the contiguous `anchor..=selected` range, dropping
    /// the ".." row. Used by shift-extended selection.
    fn select_range(&mut self) {
        let (lo, hi) = self.mark_range();
        self.marked = (lo..=hi).filter(|&i| i != 0).collect();
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
        self.marked.contains(&row_index)
    }

    pub fn marked_paths(&self) -> Vec<PathBuf> {
        self.marked
            .iter()
            .filter_map(|&i| self.entry_at(i))
            .map(|e| self.path().join(&e.name))
            .collect()
    }

    /// Backend-aware sibling of [`marked_paths`]: returns each marked
    /// entry as a full [`Location`], preserving the pane's backend.
    /// Use this anywhere the I/O dispatch needs to know whether the
    /// source is local or on a particular remote host.
    pub fn marked_locations(&self) -> Vec<Location> {
        self.marked
            .iter()
            .filter_map(|&i| self.entry_at(i))
            .map(|e| self.location.join(&e.name))
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

/// Case-insensitive substring filter over a slice of palette actions. The
/// slice is supplied by the caller (typically the result of
/// [`available_palette_actions`]) so the modal's runtime-gated entries can
/// drop in or out without `filtered_actions` itself knowing about them.
pub fn filtered_actions(actions: &[PaletteAction], input: &str) -> Vec<PaletteAction> {
    if input.is_empty() {
        return actions.to_vec();
    }
    let needle = input.to_lowercase();
    actions
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
    fn drop_target_side_splits_at_midpoint() {
        // Left half → Left.
        assert_eq!(drop_target_side(0.0, 1000.0), Side::Left);
        assert_eq!(drop_target_side(499.0, 1000.0), Side::Left);
        // Midpoint and right half → Right.
        assert_eq!(drop_target_side(500.0, 1000.0), Side::Right);
        assert_eq!(drop_target_side(999.0, 1000.0), Side::Right);
        // Out-of-bounds x clamps to the nearer side, no panic.
        assert_eq!(drop_target_side(-50.0, 1000.0), Side::Left);
        assert_eq!(drop_target_side(5000.0, 1000.0), Side::Right);
    }

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
    fn merge_pending_new_files_extends_same_folder() {
        let mut pending = VecDeque::new();
        merge_pending_new_files(
            &mut pending,
            PathBuf::from("/tmp/downloads"),
            vec!["a.zip".to_string()],
        );
        merge_pending_new_files(
            &mut pending,
            PathBuf::from("/tmp/downloads"),
            vec!["b.zip".to_string()],
        );
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1, vec!["a.zip".to_string(), "b.zip".to_string()]);
    }

    #[test]
    fn merge_pending_new_files_queues_distinct_folders() {
        let mut pending = VecDeque::new();
        merge_pending_new_files(
            &mut pending,
            PathBuf::from("/tmp/downloads"),
            vec!["a.zip".to_string()],
        );
        merge_pending_new_files(
            &mut pending,
            PathBuf::from("/tmp/other"),
            vec!["c.zip".to_string()],
        );
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].0, PathBuf::from("/tmp/downloads"));
        assert_eq!(pending[1].0, PathBuf::from("/tmp/other"));
    }

    #[test]
    fn format_log_timestamp_matches_hms() {
        let ts = format_log_timestamp(SystemTime::UNIX_EPOCH);
        // We don't pin the exact value (local timezone may shift midnight),
        // but the shape is always `HH:MM:SS` — eight chars, two colons,
        // each segment two digits.
        assert_eq!(ts.len(), 8);
        let bytes = ts.as_bytes();
        assert_eq!(bytes[2], b':');
        assert_eq!(bytes[5], b':');
        assert!(ts.chars().filter(|c| c.is_ascii_digit()).count() == 6);
    }

    #[test]
    fn ftp_focus_toggles_round_trip() {
        assert_eq!(FtpInfoFocus::Close.toggle(), FtpInfoFocus::Stop);
        assert_eq!(FtpInfoFocus::Stop.toggle(), FtpInfoFocus::Close);
        assert_eq!(FtpReplaceFocus::Cancel.toggle(), FtpReplaceFocus::Replace);
        assert_eq!(FtpReplaceFocus::Replace.toggle(), FtpReplaceFocus::Cancel);
    }

    fn ftp_info_for(root: &str) -> FtpServerInfo {
        FtpServerInfo {
            bind: "0.0.0.0:2121".parse().unwrap(),
            display_host: "192.168.1.10".to_string(),
            root: PathBuf::from(root),
            username: Some("rho".to_string()),
            password: Some("abcDEF123456".to_string()),
            permissions: crate::config::FtpPerms::ReadOnly,
            started_at: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn decide_ftp_action_starts_fresh_when_no_server() {
        let act = decide_ftp_action(None, Path::new("/tmp"));
        assert_eq!(act, FtpAction::StartFresh(PathBuf::from("/tmp")));
    }

    #[test]
    fn decide_ftp_action_shows_info_when_same_path() {
        let info = ftp_info_for("/tmp");
        let act = decide_ftp_action(Some(&info), Path::new("/tmp"));
        assert_eq!(act, FtpAction::ShowInfo);
    }

    #[test]
    fn decide_ftp_action_asks_replace_when_paths_differ() {
        let info = ftp_info_for("/tmp");
        let act = decide_ftp_action(Some(&info), Path::new("/var/log"));
        assert_eq!(act, FtpAction::AskReplace(PathBuf::from("/var/log")));
    }

    #[test]
    fn ftp_server_info_display_addr_combines_host_and_port() {
        let info = ftp_info_for("/tmp");
        assert_eq!(info.display_addr(), "192.168.1.10:2121");
    }

    #[test]
    fn ftp_server_palette_action_is_listed_and_filterable() {
        // Always listed in ALL.
        assert!(PaletteAction::ALL.contains(&PaletteAction::FtpServer));
        // Always enabled (no Dropbox gating).
        assert!(palette_action_enabled(PaletteAction::FtpServer, false));
        // Surfaces under "ftp" in the palette filter.
        let out = filtered_actions(PaletteAction::ALL, "ftp");
        assert_eq!(out, vec![PaletteAction::FtpServer]);
    }

    #[test]
    fn location_parses_plain_absolute_path_as_local() {
        let loc: Location = "/Users/ron/code".parse().unwrap();
        assert_eq!(loc, Location::Local(PathBuf::from("/Users/ron/code")));
    }

    #[test]
    fn location_parses_relative_path_as_local() {
        let loc: Location = "src/main.rs".parse().unwrap();
        assert_eq!(loc, Location::Local(PathBuf::from("src/main.rs")));
    }

    #[test]
    fn location_parses_tilde_path_as_local() {
        let loc: Location = "~/Downloads".parse().unwrap();
        assert_eq!(loc, Location::Local(PathBuf::from("~/Downloads")));
    }

    #[test]
    fn location_parses_windows_drive_path_as_local() {
        // Without this branch, "C:" would look like a backend ID.
        let loc: Location = r"C:\Users\ron".parse().unwrap();
        assert_eq!(loc, Location::Local(PathBuf::from(r"C:\Users\ron")));
        let loc: Location = "D:/projects".parse().unwrap();
        assert_eq!(loc, Location::Local(PathBuf::from("D:/projects")));
    }

    #[test]
    fn location_parses_local_filename_with_colon_as_local() {
        // The rest of the string doesn't start with `/` or `~`, so the
        // colon is part of a local filename, not a backend separator.
        let loc: Location = "file:notes.txt".parse().unwrap();
        assert_eq!(loc, Location::Local(PathBuf::from("file:notes.txt")));
    }

    #[test]
    fn location_parses_ssh_alias_form_as_remote() {
        let loc: Location = "alice.dev:/var/log".parse().unwrap();
        assert_eq!(
            loc,
            Location::Remote {
                backend: BackendId::new("alice.dev"),
                path: PathBuf::from("/var/log"),
            }
        );
    }

    #[test]
    fn location_parses_remote_with_tilde_path() {
        let loc: Location = "host:~/dotfiles".parse().unwrap();
        assert_eq!(
            loc,
            Location::Remote {
                backend: BackendId::new("host"),
                path: PathBuf::from("~/dotfiles"),
            }
        );
    }

    #[test]
    fn location_parses_backend_id_with_punctuation() {
        let loc: Location = "my-host_1.example:/x".parse().unwrap();
        assert_eq!(
            loc,
            Location::Remote {
                backend: BackendId::new("my-host_1.example"),
                path: PathBuf::from("/x"),
            }
        );
    }

    #[test]
    fn location_display_round_trips_local() {
        let original = Location::Local(PathBuf::from("/etc/hosts"));
        let s = original.to_string();
        let parsed: Location = s.parse().unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn location_display_round_trips_remote() {
        let original = Location::Remote {
            backend: BackendId::new("work"),
            path: PathBuf::from("/Apps/notes"),
        };
        let s = original.to_string();
        assert_eq!(s, "work:/Apps/notes");
        let parsed: Location = s.parse().unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn location_is_local() {
        assert!(Location::Local(PathBuf::from("/tmp")).is_local());
        assert!(!Location::Remote {
            backend: BackendId::new("x"),
            path: PathBuf::from("/tmp"),
        }
        .is_local());
    }

    #[test]
    fn location_path_returns_inner_path_for_both_variants() {
        assert_eq!(
            Location::Local(PathBuf::from("/tmp")).path(),
            Path::new("/tmp"),
        );
        assert_eq!(
            Location::Remote {
                backend: BackendId::new("x"),
                path: PathBuf::from("/var/log"),
            }
            .path(),
            Path::new("/var/log"),
        );
    }

    #[test]
    fn location_serde_round_trips_via_yaml() {
        let pairs = [
            Location::Local(PathBuf::from("/Users/ron")),
            Location::Remote {
                backend: BackendId::new("alice.dev"),
                path: PathBuf::from("/var/log"),
            },
        ];
        for original in &pairs {
            let yaml = serde_yaml::to_string(original).unwrap();
            let parsed: Location = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(&parsed, original);
        }
    }

    #[test]
    fn location_parent_preserves_backend() {
        let remote = Location::Remote {
            backend: BackendId::new("alice.dev"),
            path: PathBuf::from("/var/log"),
        };
        assert_eq!(
            remote.parent(),
            Some(Location::Remote {
                backend: BackendId::new("alice.dev"),
                path: PathBuf::from("/var"),
            })
        );
        assert_eq!(
            Location::Local(PathBuf::from("/etc/ssh")).parent(),
            Some(Location::Local(PathBuf::from("/etc"))),
        );
    }

    #[test]
    fn location_parent_at_root_returns_none() {
        assert_eq!(Location::Local(PathBuf::from("/")).parent(), None);
        assert_eq!(
            Location::Remote {
                backend: BackendId::new("host"),
                path: PathBuf::from("/"),
            }
            .parent(),
            None,
        );
    }

    #[test]
    fn location_join_preserves_backend() {
        let remote = Location::Remote {
            backend: BackendId::new("host"),
            path: PathBuf::from("/var"),
        };
        assert_eq!(
            remote.join("log"),
            Location::Remote {
                backend: BackendId::new("host"),
                path: PathBuf::from("/var/log"),
            }
        );
        assert_eq!(
            Location::Local(PathBuf::from("/etc")).join("ssh"),
            Location::Local(PathBuf::from("/etc/ssh")),
        );
    }

    #[test]
    fn parse_ls_la_skips_total_and_dot_entries() {
        let stdout = "\
total 12
drwxr-xr-x 3 user group 4096 2026-05-22 14:32:01.000000000 +0000 .
drwxr-xr-x 10 user group 4096 2026-05-22 14:30:15.000000000 +0000 ..
-rw-r--r-- 1 user group 158 2026-05-22 14:32:01.000000000 +0000 README.md
";
        let out = parse_ls_la(stdout);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "README.md");
        assert_eq!(out[0].is_dir, false);
        assert_eq!(out[0].size, Some(158));
        assert!(out[0].modified.is_some());
    }

    #[test]
    fn parse_ls_la_marks_directories() {
        let stdout = "\
drwxr-xr-x 2 user group 4096 2026-05-22 14:31:00.000000000 +0000 src
-rw-r--r-- 1 user group 42 2026-05-22 14:31:00.000000000 +0000 main.rs
";
        let out = parse_ls_la(stdout);
        assert_eq!(out.len(), 2);
        let src = out.iter().find(|e| e.name == "src").unwrap();
        assert!(src.is_dir);
        // Dirs report no size, matching local listings.
        assert_eq!(src.size, None);
        let main = out.iter().find(|e| e.name == "main.rs").unwrap();
        assert!(!main.is_dir);
        assert_eq!(main.size, Some(42));
    }

    #[test]
    fn parse_ls_la_strips_symlink_target() {
        let stdout = "\
lrwxrwxrwx 1 user group 9 2026-05-22 14:31:30.000000000 +0000 link -> README.md
";
        let out = parse_ls_la(stdout);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "link");
        // We don't follow remote symlinks → treat as not-dir, no size.
        assert!(!out[0].is_dir);
        assert_eq!(out[0].size, None);
    }

    #[test]
    fn parse_ls_la_preserves_spaces_in_names() {
        let stdout = "\
-rw-r--r-- 1 user group 7 2026-05-22 14:31:00.000000000 +0000 a file with spaces.txt
";
        let out = parse_ls_la(stdout);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "a file with spaces.txt");
    }

    #[test]
    fn parse_ls_la_handles_full_iso_with_nanos_and_offset() {
        let stdout = "\
-rw-r--r-- 1 user group 1 2026-05-22 14:32:01.123456789 +0200 a
";
        let out = parse_ls_la(stdout);
        assert_eq!(out.len(), 1);
        // We don't assert the exact SystemTime, but the parse must succeed —
        // an unparseable timestamp would leave `modified` as None.
        assert!(out[0].modified.is_some());
    }

    #[test]
    fn parse_ls_la_skips_malformed_lines() {
        let stdout = "\
total 0
this is not an ls line
-rw-r--r-- 1 user group 1 2026-05-22 14:32:01.000000000 +0000 ok.txt
";
        let out = parse_ls_la(stdout);
        // The malformed line is dropped; the real one survives.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "ok.txt");
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
        let mut pane = Pane::empty(Location::Local(PathBuf::from(path)));
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
        let p = Pane::empty(Location::Local(PathBuf::from("/tmp")));
        assert_eq!(p.path(), Path::new("/tmp"));
        assert!(p.location.is_local());
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
        assert!(!p.has_claude_md);
        assert!(!p.has_claude_dir);
        assert!(!p.has_claude_marker());
    }

    #[test]
    fn pane_has_claude_marker_combinations() {
        let mut p = Pane::empty(Location::Local(PathBuf::from("/tmp")));
        assert!(!p.has_claude_marker());
        p.has_claude_md = true;
        assert!(p.has_claude_marker());
        p.has_claude_md = false;
        p.has_claude_dir = true;
        assert!(p.has_claude_marker());
        p.has_claude_md = true;
        assert!(p.has_claude_marker());
    }

    #[test]
    fn pane_claude_marker_label_lists_what_is_present() {
        let mut p = Pane::empty(Location::Local(PathBuf::from("/tmp")));
        p.has_claude_md = true;
        assert_eq!(p.claude_marker_label(), "claude: CLAUDE.md");
        p.has_claude_dir = true;
        assert_eq!(p.claude_marker_label(), "claude: CLAUDE.md, .claude/");
        p.has_claude_md = false;
        assert_eq!(p.claude_marker_label(), "claude: .claude/");
    }

    #[test]
    fn pane_navigate_clears_claude_markers() {
        let mut p = Pane::empty(Location::Local(PathBuf::from("/old")));
        p.has_claude_md = true;
        p.has_claude_dir = true;
        p.navigate(Location::Local(PathBuf::from("/new")));
        assert!(!p.has_claude_md);
        assert!(!p.has_claude_dir);
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

        let gen_after = p.navigate(Location::Local(PathBuf::from("/new")));

        assert_eq!(gen_after, gen_before + 1);
        assert_eq!(p.load_generation, gen_after);
        assert_eq!(p.path(), Path::new("/new"));
        assert!(p.entries.is_empty());
        assert!(p.visible_indices.is_empty());
        assert!(p.filter.is_empty());
        assert_eq!(p.selected, 0);
        assert_eq!(p.anchor, 0);
        assert!(p.loading);
        assert!(p.git_info.is_none());
    }

    #[test]
    fn reload_keeps_filter_and_rehomes_cursor_without_jump() {
        let mut p = pane_with_entries(
            "/dir",
            vec![mk_entry("alpha", false, Some(1)), mk_entry("apex", false, Some(2))],
        );
        p.filter = "ap".to_string();
        p.recompute_visible(None);
        p.selected = 1; // first matching entry (row 0 is "..")
        let cursor = p.cursor_name();
        assert!(cursor.is_some());
        let gen_before = p.load_generation;

        let gen_after = p.reload();

        // Fresh generation + cleared listing, like navigate…
        assert_eq!(gen_after, gen_before + 1);
        assert!(p.entries.is_empty());
        assert!(p.visible_indices.is_empty());
        assert!(p.loading);
        assert!(p.git_info.is_none());
        // …but the filter survives and the cursor is queued for re-homing
        // by name, with the scroll-jump suppressed.
        assert_eq!(p.filter, "ap");
        assert_eq!(p.pending_focus, cursor);
        assert!(!p.focus_jump);
    }

    #[test]
    fn navigate_generation_strictly_increases() {
        let mut p = Pane::empty(Location::Local(PathBuf::from("/")));
        let g0 = p.load_generation;
        let g1 = p.navigate(Location::Local(PathBuf::from("/a")));
        let g2 = p.navigate(Location::Local(PathBuf::from("/b")));
        let g3 = p.navigate(Location::Local(PathBuf::from("/c")));
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
        p.move_to(1, false);
        p.move_to(3, true);
        assert!(p.is_marked(1));
        assert!(p.is_marked(2));
        assert!(p.is_marked(3));
        assert!(!p.is_marked(0));
    }

    #[test]
    fn is_marked_excludes_out_of_range_rows() {
        let mut p = three_entry_pane();
        p.move_to(1, false);
        p.move_to(2, true);
        assert!(p.is_marked(1));
        assert!(p.is_marked(2));
        assert!(!p.is_marked(3));
    }

    #[test]
    fn toggle_mark_builds_noncontiguous_selection() {
        let mut p = three_entry_pane();
        // Cmd+click rows 1 and 3 — row 2 stays unselected.
        p.toggle_mark(1);
        p.toggle_mark(3);
        assert!(p.is_marked(1));
        assert!(!p.is_marked(2));
        assert!(p.is_marked(3));
        // Cursor and shift-anchor followed the last toggle.
        assert_eq!(p.selected, 3);
        assert_eq!(p.anchor, 3);
    }

    #[test]
    fn toggle_mark_deselects_on_second_click() {
        let mut p = three_entry_pane();
        p.toggle_mark(2);
        assert!(p.is_marked(2));
        p.toggle_mark(2);
        assert!(!p.is_marked(2));
        // Cursor still sits on the row even though it's no longer selected.
        assert_eq!(p.selected, 2);
    }

    #[test]
    fn toggle_mark_ignores_dotdot_row() {
        let mut p = three_entry_pane();
        p.toggle_mark(0);
        assert!(!p.is_marked(0));
        assert!(p.marked_paths().is_empty());
    }

    #[test]
    fn plain_move_collapses_prior_multiselection() {
        let mut p = three_entry_pane();
        p.toggle_mark(1);
        p.toggle_mark(3);
        // A plain (non-extend) move clears the Cmd-built selection down to
        // the single new cursor row.
        p.move_to(2, false);
        assert!(!p.is_marked(1));
        assert!(p.is_marked(2));
        assert!(!p.is_marked(3));
    }

    #[test]
    fn shift_extend_after_toggle_ranges_from_last_toggle() {
        let mut p = three_entry_pane();
        p.toggle_mark(1); // cursor + anchor now at row 1
        p.move_to(3, true); // shift-extend from row 1
        assert!(p.is_marked(1));
        assert!(p.is_marked(2));
        assert!(p.is_marked(3));
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
    fn cursor_jumps_to_first_match_when_previously_focused_entry_filtered_out() {
        let mut p = alpha_pane();
        p.selected = 3; // "beta" after sorting
        p.anchor = 3;
        let prior = p.cursor_name();
        assert_eq!(prior.as_deref(), Some("beta"));

        // Filter excludes "beta" but matches "alpha"/"alphabet".
        p.filter = "alpha".to_string();
        p.recompute_visible(prior.as_deref());

        // Cursor jumps to the first match rather than the ".." row.
        assert_eq!(p.selected, 1);
        assert_eq!(p.anchor, 1);
        assert_eq!(p.cursor_name().as_deref(), Some("alpha"));
    }

    #[test]
    fn cursor_jumps_to_first_match_when_starting_from_dotdot() {
        let mut p = alpha_pane();
        // Cursor on the synthetic ".." row, as is typical right after
        // navigating into a folder and then beginning to type a filter.
        p.selected = 0;
        p.anchor = 0;
        assert_eq!(p.cursor_name(), None);

        p.filter = "beta".to_string();
        p.recompute_visible(None);

        // Filter is active and matches "beta" — cursor lands on it, not "..".
        assert_eq!(p.selected, 1);
        assert_eq!(p.cursor_name().as_deref(), Some("beta"));
    }

    #[test]
    fn cursor_stays_on_dotdot_when_filter_matches_nothing() {
        let mut p = alpha_pane();
        p.filter = "no-such-entry".to_string();
        p.recompute_visible(None);

        // No matches → nothing to select → cursor remains on the ".." row.
        assert!(p.visible_indices.is_empty());
        assert_eq!(p.selected, 0);
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
        let mut p = Pane::empty(Location::Local(PathBuf::from("/p")));
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
        let mut p = Pane::empty(Location::Local(PathBuf::from("/p")));
        p.append_chunk(vec![
            mk_entry("a", false, Some(1)),
            mk_entry("b", false, Some(2)),
        ]);
        assert_eq!(p.visible_indices.len(), 2);
    }

    #[test]
    fn append_chunk_respects_existing_filter() {
        let mut p = Pane::empty(Location::Local(PathBuf::from("/p")));
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
        let mut p = Pane::empty(Location::Local(PathBuf::from("/p")));
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
        p.move_to(1, false); // "alpha"
        let paths = p.marked_paths();
        assert_eq!(paths, vec![PathBuf::from("/p").join("alpha")]);
    }

    #[test]
    fn marked_paths_for_range_returns_all_entries_in_range() {
        let mut p = alpha_pane();
        p.move_to(1, false); // alpha
        p.move_to(3, true); // beta (after sort: alpha, alphabet, beta, gamma)
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
        p.move_to(0, false);
        p.move_to(2, true);
        let paths = p.marked_paths();
        // 2 entries returned (alpha + alphabet), not 3 — the ".." row is dropped.
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn marked_paths_with_only_dotdot_selected_is_empty() {
        let mut p = alpha_pane();
        p.move_to(0, false);
        assert!(p.marked_paths().is_empty());
    }

    #[test]
    fn marked_locations_on_local_pane_matches_marked_paths() {
        let mut p = alpha_pane();
        p.move_to(1, false);
        p.move_to(3, true);
        let paths = p.marked_paths();
        let locs = p.marked_locations();
        assert_eq!(paths.len(), locs.len());
        for (path, loc) in paths.iter().zip(locs.iter()) {
            assert_eq!(loc, &Location::Local(path.clone()));
        }
    }

    #[test]
    fn marked_locations_on_remote_pane_keeps_backend() {
        let mut p = Pane::empty(Location::Remote {
            backend: BackendId::new("alice.dev"),
            path: PathBuf::from("/var/log"),
        });
        p.append_chunk(vec![
            mk_entry("syslog", false, Some(10)),
            mk_entry("auth.log", false, Some(20)),
        ]);
        sort_entries(&mut p.entries, p.sort_by, p.sort_dir);
        p.recompute_visible(None);
        p.move_to(1, false);
        p.move_to(2, true);
        let locs = p.marked_locations();
        assert_eq!(locs.len(), 2);
        for loc in &locs {
            match loc {
                Location::Remote { backend, .. } => {
                    assert_eq!(backend.as_str(), "alice.dev");
                }
                _ => panic!("expected Remote, got {:?}", loc),
            }
        }
        // The joined paths still sit under the pane's remote dir.
        let paths: Vec<&Path> = locs.iter().map(|l| l.path()).collect();
        assert!(paths.contains(&Path::new("/var/log/syslog")));
        assert!(paths.contains(&Path::new("/var/log/auth.log")));
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
        let out = filtered_actions(PaletteAction::ALL, "");
        assert_eq!(out.len(), PaletteAction::ALL.len());
    }

    #[test]
    fn filtered_actions_substring_matches_label() {
        let out = filtered_actions(PaletteAction::ALL, "cop");
        assert_eq!(out, vec![PaletteAction::Copy]);
    }

    #[test]
    fn filtered_actions_is_case_insensitive() {
        let out = filtered_actions(PaletteAction::ALL, "EXIT");
        assert_eq!(out, vec![PaletteAction::Exit]);
    }

    #[test]
    fn filtered_actions_no_match_returns_empty() {
        let out = filtered_actions(PaletteAction::ALL, "zzzz");
        assert!(out.is_empty());
    }

    #[test]
    fn filtered_actions_respects_supplied_subset() {
        // Caller-supplied slice without Git: branch — filter "branch" finds
        // nothing even though the variant exists in ALL.
        let subset: Vec<PaletteAction> = PaletteAction::ALL
            .iter()
            .copied()
            .filter(|a| *a != PaletteAction::GitBranch)
            .collect();
        let out = filtered_actions(&subset, "branch");
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

    #[test]
    fn palette_action_docker_label() {
        assert_eq!(PaletteAction::DockerContainers.label(), "Docker containers");
    }

    #[test]
    fn filtered_actions_finds_docker() {
        let out = filtered_actions(PaletteAction::ALL, "docker");
        assert_eq!(out, vec![PaletteAction::DockerContainers]);
    }

    #[test]
    fn parse_docker_ps_basic() {
        let stdout = "abc123|web|nginx:latest|Up 2 hours\n\
                      def456|db|postgres:15|Up 5 minutes (healthy)\n";
        let out = parse_docker_ps(stdout);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "abc123");
        assert_eq!(out[0].name, "web");
        assert_eq!(out[0].image, "nginx:latest");
        assert_eq!(out[0].status, "Up 2 hours");
        assert_eq!(out[1].name, "db");
        assert_eq!(out[1].status, "Up 5 minutes (healthy)");
    }

    #[test]
    fn parse_docker_ps_empty_returns_empty() {
        assert!(parse_docker_ps("").is_empty());
        // Trailing newline shouldn't produce a phantom row.
        assert!(parse_docker_ps("\n").is_empty());
    }

    #[test]
    fn parse_docker_ps_skips_malformed_lines() {
        // Line 2 has fewer than 4 fields → skipped.
        let stdout = "abc|web|nginx|Up 1m\nshort|line\ndef|db|postgres|Up 2m\n";
        let out = parse_docker_ps(stdout);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "abc");
        assert_eq!(out[1].id, "def");
    }

    #[test]
    fn parse_docker_ps_preserves_pipes_after_third() {
        // Status strings can legitimately contain "|", and splitn(4) keeps
        // anything past the third pipe inside the status field.
        let stdout = "abc|web|nginx|Up 2 hours | restarting\n";
        let out = parse_docker_ps(stdout);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].status, "Up 2 hours | restarting");
    }

    #[test]
    fn palette_action_processes_label() {
        assert_eq!(PaletteAction::Processes.label(), "Processes");
    }

    #[test]
    fn filtered_actions_finds_processes() {
        let out = filtered_actions(PaletteAction::ALL, "proc");
        assert_eq!(out, vec![PaletteAction::Processes]);
    }

    #[test]
    fn parse_ps_output_basic_sorted_by_cpu_desc() {
        // Note: kernel-like names in brackets ([kthreadd]) should pass too.
        let stdout = "  100 0.5 1.0 bash\n  101 12.3 4.5 chrome\n  102 0.0 0.1 [kthreadd]\n";
        let out = parse_ps_output(stdout);
        assert_eq!(out.len(), 3);
        // Sort by CPU desc.
        assert_eq!(out[0].name, "chrome");
        assert_eq!(out[0].pid, 101);
        assert!((out[0].cpu_percent - 12.3).abs() < 1e-3);
        assert!((out[0].mem_percent - 4.5).abs() < 1e-3);
        assert_eq!(out[1].name, "bash");
        assert_eq!(out[2].name, "[kthreadd]");
    }

    #[test]
    fn parse_ps_output_handles_name_with_spaces() {
        // On macOS `comm` can contain spaces (e.g. "Google Chrome"). The
        // parser must put the command-name column last so any spaces stay
        // inside it.
        let stdout = "  100 1.0 2.0 Google Chrome Helper (Renderer)\n";
        let out = parse_ps_output(stdout);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Google Chrome Helper (Renderer)");
    }

    #[test]
    fn parse_ps_output_skips_malformed_lines() {
        let stdout = "garbage\n  100 1.0 2.0 valid\nnotanint x x x\n";
        let out = parse_ps_output(stdout);
        // Only the valid row survives.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "valid");
    }

    #[test]
    fn parse_ps_output_empty_returns_empty() {
        assert!(parse_ps_output("").is_empty());
        assert!(parse_ps_output("\n").is_empty());
    }

    #[test]
    fn filtered_containers_matches_name_and_image() {
        let containers = vec![
            DockerContainer {
                id: "1".into(),
                name: "web".into(),
                image: "nginx".into(),
                status: "Up".into(),
            },
            DockerContainer {
                id: "2".into(),
                name: "db".into(),
                image: "postgres".into(),
                status: "Up".into(),
            },
        ];
        // Filter by name.
        let out = filtered_containers(&containers, "web");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "web");
        // Filter by image.
        let out = filtered_containers(&containers, "postgres");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "db");
        // Empty filter returns everything.
        let out = filtered_containers(&containers, "");
        assert_eq!(out.len(), 2);
        // No match.
        let out = filtered_containers(&containers, "zzz");
        assert!(out.is_empty());
    }

    #[test]
    fn filtered_containers_is_case_insensitive() {
        let containers = vec![DockerContainer {
            id: "1".into(),
            name: "Web".into(),
            image: "Nginx".into(),
            status: "Up".into(),
        }];
        assert_eq!(filtered_containers(&containers, "WEB").len(), 1);
        assert_eq!(filtered_containers(&containers, "ngINX").len(), 1);
    }

    #[test]
    fn filtered_processes_substring_matches_name() {
        let procs = vec![
            Process {
                pid: 1,
                name: "bash".into(),
                cpu_percent: 0.0,
                mem_percent: 0.0,
            },
            Process {
                pid: 2,
                name: "chrome".into(),
                cpu_percent: 0.0,
                mem_percent: 0.0,
            },
        ];
        let out = filtered_processes(&procs, "rome");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "chrome");
        // Empty filter returns everything.
        assert_eq!(filtered_processes(&procs, "").len(), 2);
        // No match.
        assert!(filtered_processes(&procs, "zzz").is_empty());
    }

    #[test]
    fn filtered_processes_is_case_insensitive() {
        let procs = vec![Process {
            pid: 1,
            name: "Chrome".into(),
            cpu_percent: 0.0,
            mem_percent: 0.0,
        }];
        assert_eq!(filtered_processes(&procs, "CHROME").len(), 1);
    }

    #[test]
    fn docker_sort_by_initial_dir_is_asc() {
        // All text columns default to ascending so a "click Name" lands on A→Z.
        assert_eq!(DockerSortBy::Name.initial_dir(), SortDir::Asc);
        assert_eq!(DockerSortBy::Image.initial_dir(), SortDir::Asc);
        assert_eq!(DockerSortBy::Status.initial_dir(), SortDir::Asc);
    }

    #[test]
    fn process_sort_by_initial_dir_picks_desc_for_numeric_columns() {
        assert_eq!(ProcessSortBy::Name.initial_dir(), SortDir::Asc);
        assert_eq!(ProcessSortBy::Pid.initial_dir(), SortDir::Desc);
        assert_eq!(ProcessSortBy::Cpu.initial_dir(), SortDir::Desc);
        assert_eq!(ProcessSortBy::Mem.initial_dir(), SortDir::Desc);
    }

    fn mk_container(name: &str, image: &str, status: &str) -> DockerContainer {
        DockerContainer {
            id: format!("{}-id", name),
            name: name.to_string(),
            image: image.to_string(),
            status: status.to_string(),
        }
    }

    #[test]
    fn sort_containers_by_name_case_insensitive() {
        let mut v = vec![
            mk_container("Bravo", "nginx", "Up"),
            mk_container("alpha", "redis", "Up"),
            mk_container("Charlie", "postgres", "Restarting"),
        ];
        sort_containers(&mut v, DockerSortBy::Name, SortDir::Asc);
        assert_eq!(v[0].name, "alpha");
        assert_eq!(v[1].name, "Bravo");
        assert_eq!(v[2].name, "Charlie");
    }

    #[test]
    fn sort_containers_by_image_desc() {
        let mut v = vec![
            mk_container("a", "alpine", "Up"),
            mk_container("b", "ubuntu", "Up"),
            mk_container("c", "nginx", "Up"),
        ];
        sort_containers(&mut v, DockerSortBy::Image, SortDir::Desc);
        assert_eq!(v[0].image, "ubuntu");
        assert_eq!(v[1].image, "nginx");
        assert_eq!(v[2].image, "alpine");
    }

    #[test]
    fn sort_containers_by_status() {
        let mut v = vec![
            mk_container("a", "alpine", "Up 5 min"),
            mk_container("b", "ubuntu", "Restarting"),
            mk_container("c", "nginx", "Exited"),
        ];
        sort_containers(&mut v, DockerSortBy::Status, SortDir::Asc);
        assert_eq!(v[0].status, "Exited");
        assert_eq!(v[1].status, "Restarting");
        assert_eq!(v[2].status, "Up 5 min");
    }

    fn mk_proc(name: &str, pid: u32, cpu: f32, mem: f32) -> Process {
        Process {
            pid,
            name: name.to_string(),
            cpu_percent: cpu,
            mem_percent: mem,
        }
    }

    #[test]
    fn sort_processes_by_cpu_desc() {
        let mut v = vec![
            mk_proc("bash", 100, 0.5, 1.0),
            mk_proc("chrome", 101, 12.3, 4.5),
            mk_proc("kthread", 102, 0.0, 0.1),
        ];
        sort_processes(&mut v, ProcessSortBy::Cpu, SortDir::Desc);
        assert_eq!(v[0].name, "chrome");
        assert_eq!(v[1].name, "bash");
        assert_eq!(v[2].name, "kthread");
    }

    #[test]
    fn sort_processes_by_cpu_asc_is_reverse() {
        let mut v = vec![
            mk_proc("bash", 100, 0.5, 1.0),
            mk_proc("chrome", 101, 12.3, 4.5),
        ];
        sort_processes(&mut v, ProcessSortBy::Cpu, SortDir::Asc);
        assert_eq!(v[0].name, "bash");
        assert_eq!(v[1].name, "chrome");
    }

    #[test]
    fn sort_processes_by_mem_desc() {
        let mut v = vec![
            mk_proc("a", 1, 0.0, 1.0),
            mk_proc("b", 2, 0.0, 10.5),
            mk_proc("c", 3, 0.0, 5.0),
        ];
        sort_processes(&mut v, ProcessSortBy::Mem, SortDir::Desc);
        assert_eq!(v[0].name, "b");
        assert_eq!(v[1].name, "c");
        assert_eq!(v[2].name, "a");
    }

    #[test]
    fn sort_processes_by_pid_desc() {
        let mut v = vec![
            mk_proc("a", 100, 0.0, 0.0),
            mk_proc("b", 50, 0.0, 0.0),
            mk_proc("c", 200, 0.0, 0.0),
        ];
        sort_processes(&mut v, ProcessSortBy::Pid, SortDir::Desc);
        assert_eq!(v[0].pid, 200);
        assert_eq!(v[1].pid, 100);
        assert_eq!(v[2].pid, 50);
    }

    #[test]
    fn sort_processes_by_name_case_insensitive() {
        let mut v = vec![
            mk_proc("Bravo", 1, 0.0, 0.0),
            mk_proc("alpha", 2, 0.0, 0.0),
            mk_proc("Charlie", 3, 0.0, 0.0),
        ];
        sort_processes(&mut v, ProcessSortBy::Name, SortDir::Asc);
        assert_eq!(v[0].name, "alpha");
        assert_eq!(v[1].name, "Bravo");
        assert_eq!(v[2].name, "Charlie");
    }

    #[test]
    fn palette_action_launch_application_label() {
        assert_eq!(
            PaletteAction::LaunchApplication.label(),
            "Launch Application"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn palette_all_includes_launch_application_on_macos() {
        assert!(PaletteAction::ALL.contains(&PaletteAction::LaunchApplication));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn palette_all_excludes_launch_application_off_macos() {
        assert!(!PaletteAction::ALL.contains(&PaletteAction::LaunchApplication));
    }

    #[test]
    fn palette_action_reveal_in_finder_label() {
        assert_eq!(
            PaletteAction::RevealInFinder.label(),
            "Open folder in Finder"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn palette_all_includes_reveal_in_finder_on_macos() {
        assert!(PaletteAction::ALL.contains(&PaletteAction::RevealInFinder));
        assert!(available_palette_actions(false).contains(&PaletteAction::RevealInFinder));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn palette_all_excludes_reveal_in_finder_off_macos() {
        assert!(!PaletteAction::ALL.contains(&PaletteAction::RevealInFinder));
    }

    fn mk_app(name: &str) -> Application {
        Application {
            path: PathBuf::from(format!("/Applications/{}.app", name)),
            name: name.to_string(),
        }
    }

    #[test]
    fn sort_apps_case_insensitive_by_name() {
        let mut v = vec![mk_app("Slack"), mk_app("calculator"), mk_app("Xcode")];
        sort_apps(&mut v);
        assert_eq!(v[0].name, "calculator");
        assert_eq!(v[1].name, "Slack");
        assert_eq!(v[2].name, "Xcode");
    }

    #[test]
    fn filtered_apps_substring_matches() {
        let apps = vec![
            mk_app("Slack"),
            mk_app("Google Chrome"),
            mk_app("Terminal"),
        ];
        let out = filtered_apps(&apps, "chrome");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Google Chrome");
    }

    #[test]
    fn filtered_apps_empty_returns_all() {
        let apps = vec![mk_app("Slack"), mk_app("Terminal")];
        let out = filtered_apps(&apps, "");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn filtered_apps_is_case_insensitive() {
        let apps = vec![mk_app("Visual Studio Code")];
        assert_eq!(filtered_apps(&apps, "VISUAL").len(), 1);
        assert_eq!(filtered_apps(&apps, "code").len(), 1);
    }

    #[test]
    fn filtered_apps_no_match_returns_empty() {
        let apps = vec![mk_app("Slack")];
        assert!(filtered_apps(&apps, "zzz").is_empty());
    }

    #[test]
    fn palette_action_git_branch_label() {
        assert_eq!(PaletteAction::GitBranch.label(), "Git: branch");
    }

    #[test]
    fn available_palette_actions_hides_git_branch_outside_repo() {
        let out = available_palette_actions(false);
        assert!(!out.contains(&PaletteAction::GitBranch));
        // Other actions still present.
        assert!(out.contains(&PaletteAction::Copy));
        assert!(out.contains(&PaletteAction::Exit));
    }

    #[test]
    fn available_palette_actions_includes_git_branch_in_repo() {
        let out = available_palette_actions(true);
        assert!(out.contains(&PaletteAction::GitBranch));
    }

    #[test]
    fn available_palette_actions_always_lists_dropbox() {
        // Listed regardless of credentials — discoverability. Enablement is a
        // separate concern (palette_action_enabled).
        assert!(available_palette_actions(false).contains(&PaletteAction::OpenDropbox));
        assert!(available_palette_actions(true).contains(&PaletteAction::OpenDropbox));
    }

    #[test]
    fn available_palette_actions_always_lists_open_terminal() {
        // Always offered (in or out of a git repo) — it's never contextual.
        assert!(available_palette_actions(false).contains(&PaletteAction::OpenTerminal));
        assert!(available_palette_actions(true).contains(&PaletteAction::OpenTerminal));
        assert_eq!(PaletteAction::OpenTerminal.label(), "Open Terminal in this folder");
    }

    #[test]
    fn available_palette_actions_always_lists_open_in_editor() {
        assert!(available_palette_actions(false).contains(&PaletteAction::OpenInEditor));
        assert!(available_palette_actions(true).contains(&PaletteAction::OpenInEditor));
        assert_eq!(PaletteAction::OpenInEditor.label(), "Open folder in editor");
    }

    #[test]
    fn available_palette_actions_always_lists_new_file() {
        assert!(available_palette_actions(false).contains(&PaletteAction::NewFile));
        assert!(available_palette_actions(true).contains(&PaletteAction::NewFile));
        assert_eq!(PaletteAction::NewFile.label(), "New file");
        // Always activatable — not gated on Dropbox credentials.
        assert!(palette_action_enabled(PaletteAction::NewFile, false));
    }

    #[test]
    fn palette_action_enabled_gates_dropbox_on_credentials() {
        assert!(!palette_action_enabled(PaletteAction::OpenDropbox, false));
        assert!(palette_action_enabled(PaletteAction::OpenDropbox, true));
        // Non-Dropbox actions are enabled regardless of the flag.
        assert!(palette_action_enabled(PaletteAction::Copy, false));
        assert!(palette_action_enabled(PaletteAction::GitBranch, false));
    }

    #[test]
    fn modal_scroll_target_only_moves_when_off_page() {
        // Viewport 100px tall, 20px rows -> rows 0..4 fully visible at top.
        // A row already on the page doesn't move the list.
        assert_eq!(modal_scroll_target(0, 20.0, 100.0, 0.0), None);
        assert_eq!(modal_scroll_target(4, 20.0, 100.0, 0.0), None);
        // Row just below the fold scrolls down so its bottom hits the edge.
        assert_eq!(modal_scroll_target(5, 20.0, 100.0, 0.0), Some(20.0));
        // Row above the current top scrolls up to the row's top.
        assert_eq!(modal_scroll_target(2, 20.0, 100.0, 80.0), Some(40.0));
        // Already-visible row with a non-zero scroll stays put.
        assert_eq!(modal_scroll_target(5, 20.0, 100.0, 40.0), None);
        // Never scrolls past the top.
        assert_eq!(modal_scroll_target(0, 20.0, 100.0, 40.0), Some(0.0));
    }

    #[test]
    fn dropbox_api_path_maps_root_to_empty() {
        assert_eq!(dropbox_api_path(Path::new("/")), "");
        assert_eq!(dropbox_api_path(Path::new("/Photos")), "/Photos");
        assert_eq!(dropbox_api_path(Path::new("/Photos/a.jpg")), "/Photos/a.jpg");
        // Trailing slash is trimmed.
        assert_eq!(dropbox_api_path(Path::new("/Photos/")), "/Photos");
    }

    #[test]
    fn parse_dropbox_list_maps_files_and_folders() {
        let json = r#"{
            "entries": [
                {".tag": "folder", "name": "Photos"},
                {".tag": "file", "name": "report.pdf", "size": 1024,
                 "server_modified": "2015-05-12T15:50:38Z"},
                {".tag": "deleted", "name": "gone.txt"}
            ],
            "cursor": "abc",
            "has_more": false
        }"#;
        let listing = parse_dropbox_list(json).unwrap();
        // The `deleted` entry is dropped.
        assert_eq!(listing.entries.len(), 2);
        let folder = &listing.entries[0];
        assert_eq!(folder.name, "Photos");
        assert!(folder.is_dir);
        assert_eq!(folder.size, None);
        let file = &listing.entries[1];
        assert_eq!(file.name, "report.pdf");
        assert!(!file.is_dir);
        assert_eq!(file.size, Some(1024));
        assert!(file.modified.is_some());
        // has_more false → no continuation cursor.
        assert_eq!(listing.cursor, None);
    }

    #[test]
    fn parse_dropbox_list_returns_cursor_when_has_more() {
        let json = r#"{"entries": [], "cursor": "next-page", "has_more": true}"#;
        let listing = parse_dropbox_list(json).unwrap();
        assert_eq!(listing.cursor.as_deref(), Some("next-page"));
    }

    #[test]
    fn parse_dropbox_list_surfaces_api_error() {
        let json = r#"{"error_summary": "path/not_found/", "error": {".tag": "path"}}"#;
        let err = parse_dropbox_list(json).unwrap_err();
        assert!(err.contains("not_found"));
    }

    #[test]
    fn parse_dropbox_token_extracts_token_and_expiry() {
        let json = r#"{"access_token": "sl.abc", "token_type": "bearer", "expires_in": 14400}"#;
        let (token, expires) = parse_dropbox_token(json).unwrap();
        assert_eq!(token, "sl.abc");
        assert_eq!(expires, 14400);
    }

    #[test]
    fn parse_dropbox_token_surfaces_oauth_error() {
        let json = r#"{"error": "invalid_grant", "error_description": "refresh token is invalid"}"#;
        let err = parse_dropbox_token(json).unwrap_err();
        assert_eq!(err, "refresh token is invalid");
    }

    #[test]
    fn dropbox_error_summary_strips_trailing_dot() {
        let json = r#"{"error_summary": "path/not_found/.", "error": {".tag": "path"}}"#;
        assert_eq!(
            dropbox_error_summary(json).as_deref(),
            Some("path/not_found")
        );
        assert_eq!(dropbox_error_summary("not json"), None);
    }

    #[test]
    fn backend_id_kind_detects_dropbox() {
        assert_eq!(BackendId::new("dropbox").kind(), BackendKind::Dropbox);
        assert!(BackendId::new("dropbox").is_dropbox());
        assert_eq!(BackendId::new("alice.dev").kind(), BackendKind::Ssh);
        assert!(!BackendId::new("alice.dev").is_dropbox());
    }

    #[test]
    fn parse_git_branches_basic() {
        // The `--sort=-committerdate` flag does the ordering before we see
        // the output, so the parser preserves arrival order.
        let stdout = "main|2026-05-21\nfeature/foo|2026-05-15\nold-branch|2024-01-02\n";
        let out = parse_git_branches(stdout);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].name, "main");
        assert_eq!(out[0].last_commit, "2026-05-21");
        assert_eq!(out[1].name, "feature/foo");
        assert_eq!(out[2].name, "old-branch");
        assert_eq!(out[2].last_commit, "2024-01-02");
    }

    #[test]
    fn parse_git_branches_handles_slashes_in_name() {
        let stdout = "users/alice/feat-x|2026-05-20\n";
        let out = parse_git_branches(stdout);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "users/alice/feat-x");
    }

    #[test]
    fn parse_git_branches_skips_malformed_and_empty() {
        let stdout = "\nno-pipe-here\nmain|2026-05-21\n|2026-05-20\n";
        let out = parse_git_branches(stdout);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "main");
    }

    fn mk_branch(name: &str) -> GitBranch {
        GitBranch {
            name: name.to_string(),
            last_commit: "2026-05-21".to_string(),
        }
    }

    #[test]
    fn filtered_branches_substring_matches() {
        let branches = vec![mk_branch("main"), mk_branch("feature/foo"), mk_branch("bugfix")];
        let out = filtered_branches(&branches, "fea");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "feature/foo");
    }

    #[test]
    fn filtered_branches_empty_returns_all() {
        let branches = vec![mk_branch("main"), mk_branch("dev")];
        assert_eq!(filtered_branches(&branches, "").len(), 2);
    }

    #[test]
    fn filtered_branches_is_case_insensitive() {
        let branches = vec![mk_branch("MAIN"), mk_branch("dev")];
        assert_eq!(filtered_branches(&branches, "main").len(), 1);
    }

    #[test]
    fn palette_action_keyboard_shortcuts_label() {
        assert_eq!(
            PaletteAction::KeyboardShortcuts.label(),
            "Keyboard shortcuts"
        );
    }

    #[test]
    fn keyboard_shortcuts_has_sections_and_bindings() {
        let sections = keyboard_shortcuts();
        assert!(!sections.is_empty(), "expected at least one section");
        // Every section must have a title and at least one binding row, and
        // every binding must have non-empty keys and description.
        for (title, bindings) in &sections {
            assert!(!title.is_empty(), "empty section title");
            assert!(
                !bindings.is_empty(),
                "section {} has no bindings",
                title
            );
            for (keys, desc) in bindings {
                assert!(!keys.is_empty(), "empty keys in section {}", title);
                assert!(!desc.is_empty(), "empty description in section {}", title);
            }
        }
    }

    #[test]
    fn keyboard_shortcuts_covers_load_bearing_keys() {
        // Quick smoke check that the modal mirrors the docs — these are the
        // bindings most likely to surprise users by their absence.
        let joined: String = keyboard_shortcuts()
            .into_iter()
            .flat_map(|(_, b)| b.into_iter().map(|(k, _)| k))
            .collect::<Vec<_>>()
            .join(" | ");
        for needle in ["⌘P", "⌘⇧P", "F4", "F5", "F7", "F8", "F10", "Tab", "Esc"] {
            assert!(joined.contains(needle), "missing binding: {}", needle);
        }
    }

    #[test]
    fn palette_action_ssh_connect_label() {
        assert_eq!(
            PaletteAction::SshConnect.label(),
            "Connect to SSH server"
        );
    }

    #[test]
    fn parse_ssh_config_basic() {
        let cfg = "
Host alpha
    HostName alpha.example.com
    User alice
    IdentityFile ~/.ssh/id_rsa

Host beta
    HostName beta.example.com
    User bob
";
        let out = parse_ssh_config(cfg);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].alias, "alpha");
        assert_eq!(out[0].hostname.as_deref(), Some("alpha.example.com"));
        assert_eq!(out[0].user.as_deref(), Some("alice"));
        assert_eq!(out[0].identity_file.as_deref(), Some("~/.ssh/id_rsa"));
        assert_eq!(out[1].alias, "beta");
        assert_eq!(out[1].hostname.as_deref(), Some("beta.example.com"));
        assert_eq!(out[1].identity_file, None);
    }

    #[test]
    fn parse_ssh_config_skips_pure_wildcard_blocks() {
        let cfg = "
Host *
    ServerAliveInterval 30
    IdentityFile ~/.ssh/id_ed25519

Host real
    HostName real.example.com
";
        let out = parse_ssh_config(cfg);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].alias, "real");
        // The defaults block's IdentityFile must NOT leak into `real`.
        assert!(out[0].identity_file.is_none());
    }

    #[test]
    fn parse_ssh_config_picks_first_non_wildcard_pattern() {
        // "Host foo *.bar" → first non-wildcard is "foo".
        let cfg = "Host foo *.bar\n    HostName foo.example.com\n";
        let out = parse_ssh_config(cfg);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].alias, "foo");
    }

    #[test]
    fn parse_ssh_config_handles_comments_and_case() {
        let cfg = "
# my work boxes
Host work
    hostname work.example.com
    USER carol
";
        let out = parse_ssh_config(cfg);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].hostname.as_deref(), Some("work.example.com"));
        assert_eq!(out[0].user.as_deref(), Some("carol"));
    }

    #[test]
    fn parse_ssh_config_empty_returns_empty() {
        assert!(parse_ssh_config("").is_empty());
        assert!(parse_ssh_config("\n# only comments\n").is_empty());
    }

    fn mk_server(alias: &str, host: Option<&str>) -> SshServer {
        SshServer {
            alias: alias.to_string(),
            hostname: host.map(|s| s.to_string()),
            user: None,
            identity_file: None,
        }
    }

    #[test]
    fn sort_servers_case_insensitive_by_alias() {
        let mut v = vec![
            mk_server("Zeta", None),
            mk_server("alpha", None),
            mk_server("Beta", None),
        ];
        sort_servers(&mut v);
        assert_eq!(v[0].alias, "alpha");
        assert_eq!(v[1].alias, "Beta");
        assert_eq!(v[2].alias, "Zeta");
    }

    #[test]
    fn filtered_servers_matches_alias_or_hostname() {
        let v = vec![
            mk_server("work", Some("work.example.com")),
            mk_server("home", Some("home.lan")),
        ];
        // Alias match.
        let out = filtered_servers(&v, "work");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].alias, "work");
        // Hostname match.
        let out = filtered_servers(&v, "lan");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].alias, "home");
        // Empty filter returns all.
        assert_eq!(filtered_servers(&v, "").len(), 2);
        // No match.
        assert!(filtered_servers(&v, "zzz").is_empty());
    }

    #[test]
    fn filtered_servers_is_case_insensitive() {
        let v = vec![mk_server("Work", Some("EXAMPLE.com"))];
        assert_eq!(filtered_servers(&v, "WORK").len(), 1);
        assert_eq!(filtered_servers(&v, "example").len(), 1);
    }

    #[test]
    fn palette_action_open_claude_code_label() {
        assert_eq!(
            PaletteAction::OpenClaudeCode.label(),
            "Open Claude Code in this folder"
        );
    }

    #[test]
    fn palette_action_move_label() {
        assert_eq!(PaletteAction::Move.label(), "Move");
    }

    #[test]
    fn palette_all_includes_move() {
        assert!(PaletteAction::ALL.contains(&PaletteAction::Move));
    }

    #[test]
    fn palette_action_compress_labels() {
        assert_eq!(PaletteAction::Compress.label(), "Compress");
        assert_eq!(PaletteAction::Uncompress.label(), "Uncompress");
    }

    #[test]
    fn detect_archive_format_zip() {
        assert_eq!(detect_archive_format("file.zip"), Some(ArchiveFormat::Zip));
        assert_eq!(detect_archive_format("FILE.ZIP"), Some(ArchiveFormat::Zip));
    }

    #[test]
    fn detect_archive_format_tar_gz() {
        assert_eq!(
            detect_archive_format("file.tar.gz"),
            Some(ArchiveFormat::TarGz)
        );
        assert_eq!(detect_archive_format("file.tgz"), Some(ArchiveFormat::TarGz));
        // Mixed case still matches.
        assert_eq!(
            detect_archive_format("File.Tar.Gz"),
            Some(ArchiveFormat::TarGz)
        );
    }

    #[test]
    fn detect_archive_format_unknown() {
        assert_eq!(detect_archive_format("file.rar"), None);
        assert_eq!(detect_archive_format("file"), None);
        assert_eq!(detect_archive_format(""), None);
    }

    #[test]
    fn default_zip_filename_strips_simple_extension() {
        let srcs = vec![PathBuf::from("/tmp/report.pdf")];
        assert_eq!(default_zip_filename(&srcs), "report.zip");
    }

    #[test]
    fn default_zip_filename_strips_tar_gz() {
        // The full `.tar.gz` extension is stripped, not just `.gz`.
        let srcs = vec![PathBuf::from("/tmp/archive.tar.gz")];
        assert_eq!(default_zip_filename(&srcs), "archive.zip");
    }

    #[test]
    fn default_zip_filename_directory_name_stays() {
        // Directory without an extension keeps its full name + .zip.
        let srcs = vec![PathBuf::from("/tmp/MyFolder")];
        assert_eq!(default_zip_filename(&srcs), "MyFolder.zip");
    }

    #[test]
    fn default_zip_filename_uses_first_marked() {
        // Only the first src's name informs the default.
        let srcs = vec![
            PathBuf::from("/tmp/first.txt"),
            PathBuf::from("/tmp/second.txt"),
        ];
        assert_eq!(default_zip_filename(&srcs), "first.zip");
    }

    #[test]
    fn default_zip_filename_empty_falls_back() {
        assert_eq!(default_zip_filename(&[]), "archive.zip");
    }

    #[test]
    fn default_zip_filename_dotfile_keeps_name() {
        // `.gitignore` has no separating extension — treat the whole thing
        // as the stem so we don't end up with an empty filename.
        let srcs = vec![PathBuf::from("/tmp/.gitignore")];
        assert_eq!(default_zip_filename(&srcs), ".gitignore.zip");
    }

    // ---- file actions ---------------------------------------------------

    #[test]
    fn glob_match_extension_patterns() {
        assert!(glob_match("*.md", "notes.md"));
        assert!(glob_match("*.md", "a.b.md"));
        assert!(!glob_match("*.md", "notes.mdx"));
        assert!(!glob_match("*.md", "mdfile"));
    }

    #[test]
    fn glob_match_is_case_insensitive() {
        assert!(glob_match("*.MD", "notes.md"));
        assert!(glob_match("*.md", "NOTES.MD"));
    }

    #[test]
    fn glob_match_question_mark_is_single_char() {
        assert!(glob_match("a?c.txt", "abc.txt"));
        assert!(!glob_match("a?c.txt", "ac.txt"));
        assert!(!glob_match("a?c.txt", "abbc.txt"));
    }

    #[test]
    fn glob_match_multiple_stars_and_double_extension() {
        assert!(glob_match("*.tar.gz", "archive.tar.gz"));
        assert!(glob_match("*report*", "q3-report-final.pdf"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn matching_file_actions_filters_and_preserves_order() {
        let actions = vec![
            FileAction {
                pattern: "*.md".into(),
                label: "PDF".into(),
                command: "pandoc {file}".into(),
                terminal: false,
            },
            FileAction {
                pattern: "*.png".into(),
                label: "Open".into(),
                command: "view {file}".into(),
                terminal: false,
            },
            FileAction {
                pattern: "*".into(),
                label: "Hash".into(),
                command: "shasum {file}".into(),
                terminal: true,
            },
        ];
        let m = matching_file_actions(&actions, "readme.md");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].label, "PDF");
        assert_eq!(m[1].label, "Hash");
        assert!(file_has_custom_action(&actions, "x.png"));
        // The catch-all `*` means every file has at least one action here.
        assert!(file_has_custom_action(&actions, "anything.xyz"));
    }

    #[test]
    fn matching_quick_view_first_match_wins() {
        let actions = vec![
            QuickViewAction {
                pattern: "*.log".into(),
                label: "Tail".into(),
                command: Some("tail -n 200 {file}".into()),
            },
            QuickViewAction {
                pattern: "*".into(),
                label: "Preview".into(),
                command: None,
            },
        ];
        let m = matching_quick_view(&actions, "app.log").unwrap();
        assert_eq!(m.label, "Tail");
        // Falls through to the catch-all for anything else.
        let m = matching_quick_view(&actions, "notes.txt").unwrap();
        assert_eq!(m.label, "Preview");
        assert_eq!(m.command, None);
    }

    #[test]
    fn matching_quick_view_no_match_returns_none() {
        let actions = vec![QuickViewAction {
            pattern: "*.log".into(),
            label: "Tail".into(),
            command: Some("tail {file}".into()),
        }];
        assert!(matching_quick_view(&actions, "notes.txt").is_none());
        assert!(matching_quick_view(&[], "anything").is_none());
    }

    #[test]
    fn build_file_choices_prepends_default_open() {
        let actions = vec![FileAction {
            pattern: "*.md".into(),
            label: "Convert to PDF".into(),
            command: "pandoc -o {stem}.pdf {file}".into(),
            terminal: false,
        }];
        let choices = build_file_choices(&actions, "notes.md");
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0], FileChoice::OpenDefault);
        assert_eq!(choices[0].label(), "Open with default application");
        assert_eq!(
            choices[1],
            FileChoice::Custom {
                label: "Convert to PDF".into(),
                command: "pandoc -o {stem}.pdf {file}".into(),
                terminal: false,
            }
        );
        // A file with no matching action still gets the default-open choice.
        assert_eq!(build_file_choices(&actions, "photo.png").len(), 1);
    }

    #[test]
    fn posix_quote_wraps_and_escapes() {
        assert_eq!(posix_quote("foo"), "'foo'");
        assert_eq!(posix_quote("/var/log"), "'/var/log'");
        // `it's` → 'it'\''s' (close, escaped quote, reopen).
        assert_eq!(posix_quote("it's"), r"'it'\''s'");
        // $ \ " ` etc. pass through untouched inside the single quotes.
        assert_eq!(posix_quote(r#"a $b \c "d" `e`"#), r#"'a $b \c "d" `e`'"#);
    }

    #[test]
    fn substitute_command_quotes_each_value() {
        let p = Path::new("/Users/ron/docs/report.md");
        // Values are single-quoted; the template text around them is verbatim,
        // so `'report'.pdf` concatenates to report.pdf in the shell.
        assert_eq!(
            substitute_command("pandoc -o {stem}.pdf {file}", p),
            "pandoc -o 'report'.pdf 'report.md'"
        );
        assert_eq!(substitute_command("echo {ext}", p), "echo 'md'");
        assert_eq!(substitute_command("ls {dir}", p), "ls '/Users/ron/docs'");
        assert_eq!(
            substitute_command("cat {path}", p),
            "cat '/Users/ron/docs/report.md'"
        );
        // Pipes / && in the template survive (only values are quoted).
        assert_eq!(
            substitute_command("cat {file} | wc -l", p),
            "cat 'report.md' | wc -l"
        );
    }

    #[test]
    fn substitute_command_dotfile_stem_is_whole_name() {
        // `.gitignore` has no separating extension — stem is the whole name.
        let p = Path::new("/tmp/.gitignore");
        assert_eq!(substitute_command("edit {stem}", p), "edit '.gitignore'");
    }

    #[test]
    fn substitute_command_neutralizes_command_injection_in_filename() {
        // A hostile file name can't break out of the quoting to run commands.
        let p = Path::new("/tmp/$(rm -rf ~).md");
        assert_eq!(
            substitute_command("pandoc {file}", p),
            "pandoc '$(rm -rf ~).md'"
        );
        // A name containing a single quote is escaped, not terminated early.
        let q = Path::new("/tmp/it's a file.md");
        assert_eq!(
            substitute_command("wc -l {file}", q),
            r"wc -l 'it'\''s a file.md'"
        );
    }

    #[test]
    fn substitute_command_single_pass_does_not_re_expand() {
        // A file literally named with `{ext}` in it must not have that token
        // re-expanded by a later pass.
        let p = Path::new("/tmp/a{ext}b.md");
        assert_eq!(substitute_command("echo {file}", p), "echo 'a{ext}b.md'");
    }

    #[test]
    fn substitute_command_passes_through_unknown_tokens() {
        let p = Path::new("/tmp/x.md");
        assert_eq!(
            substitute_command("tool {file} {unknown}", p),
            "tool 'x.md' {unknown}"
        );
        // An unterminated brace is emitted verbatim.
        assert_eq!(substitute_command("echo {file", p), "echo {file");
    }

    #[test]
    fn substitute_command_multi_joins_files_space_separated() {
        let paths = vec![
            PathBuf::from("/p/a.txt"),
            PathBuf::from("/p/b.txt"),
            PathBuf::from("/p/c.txt"),
        ];
        assert_eq!(
            substitute_command_multi("shasum {file}", &paths, &paths[0]),
            "shasum 'a.txt' 'b.txt' 'c.txt'"
        );
    }

    #[test]
    fn substitute_command_multi_expands_path_too() {
        let paths = vec![PathBuf::from("/p/a.txt"), PathBuf::from("/p/b.txt")];
        assert_eq!(
            substitute_command_multi("cat {path}", &paths, &paths[0]),
            "cat '/p/a.txt' '/p/b.txt'"
        );
    }

    #[test]
    fn substitute_command_multi_single_valued_tokens_use_primary() {
        // {stem}/{ext}/{dir} have no multi-file meaning — they resolve against
        // the cursor (primary) file.
        let paths = vec![PathBuf::from("/docs/a.md"), PathBuf::from("/docs/b.md")];
        assert_eq!(
            substitute_command_multi("pandoc -o {stem}.pdf {file}", &paths, &paths[0]),
            "pandoc -o 'a'.pdf 'a.md' 'b.md'"
        );
        assert_eq!(
            substitute_command_multi("echo {ext} {dir}", &paths, &paths[0]),
            "echo 'md' '/docs'"
        );
    }

    #[test]
    fn substitute_command_multi_quotes_each_file_independently() {
        // Spaces / metacharacters in individual names stay isolated — no
        // injection across the join.
        let paths = vec![
            PathBuf::from("/p/my file.txt"),
            PathBuf::from("/p/$(rm -rf).txt"),
        ];
        assert_eq!(
            substitute_command_multi("wc -l {file}", &paths, &paths[0]),
            "wc -l 'my file.txt' '$(rm -rf).txt'"
        );
    }

    #[test]
    fn substitute_command_multi_with_single_path_matches_single_variant() {
        let p = PathBuf::from("/p/only.txt");
        assert_eq!(
            substitute_command_multi("tail {file}", std::slice::from_ref(&p), &p),
            substitute_command("tail {file}", &p)
        );
    }
}
