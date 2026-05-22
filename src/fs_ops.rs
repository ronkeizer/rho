//! All filesystem and external-process I/O: directory streaming, copy/delete
//! tasks, git probing, and the `notify`-based watcher subscription. Each task
//! returns `Task<Message>` / `Subscription<Message>` so the App layer can wire
//! the results in via `update()`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::{Subscription, Task};

use crate::domain::{
    detect_archive_format, parse_docker_ps, parse_git_branches, parse_ps_output, parse_ssh_config,
    Application, ArchiveFormat, DockerContainer, Entry, GitBranch, GitInfo, Process, Side,
    SshServer,
};
use crate::Message;

/// Both side-loads (directory entries + git info) for a single pane.
pub fn loading_tasks(side: Side, path: PathBuf, generation: u64) -> Task<Message> {
    Task::batch([
        load_dir_task(side, path.clone(), generation),
        git_info_task(side, path, generation),
    ])
}

/// Read `path` in a blocking thread and stream batches of `Entry` back to the
/// app as `EntriesChunk` messages, followed by a final `EntriesDone`. The
/// `generation` tag lets the receiver discard chunks from a load that's been
/// superseded by a later navigation.
pub fn load_dir_task(side: Side, path: PathBuf, generation: u64) -> Task<Message> {
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
pub fn file_watch_subscription(folders: Vec<PathBuf>) -> Subscription<Message> {
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

pub fn copy_task(srcs: Vec<PathBuf>, dest_dir: PathBuf) -> Task<Message> {
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

pub fn move_task(srcs: Vec<PathBuf>, dest_dir: PathBuf) -> Task<Message> {
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
                        let res = move_path(&src, &target).map_err(|e| e.to_string());
                        (src, res)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default()
        },
        Message::MoveFinished,
    )
}

pub fn delete_task(paths: Vec<PathBuf>) -> Task<Message> {
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

pub fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
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

pub fn delete_path(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Move `src` to `dst`. Tries `fs::rename` first (atomic on the same
/// filesystem) and falls back to `copy_recursive` + `delete_path` when
/// rename fails with EXDEV (Linux/macOS = 18, Windows = 17 / ERROR_NOT_
/// SAME_DEVICE). Any other rename error is propagated unchanged — we
/// don't want to mask, say, a permissions error with a misleading copy
/// failure later in the fallback.
pub fn move_path(src: &Path, dst: &Path) -> std::io::Result<()> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if matches!(e.raw_os_error(), Some(17) | Some(18)) => {
            copy_recursive(src, dst)?;
            delete_path(src)
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Compress / uncompress (zip + tar.gz)
// ---------------------------------------------------------------------------

/// `zip -r <output> <srcs basenames…>` invoked with the current directory
/// set to the active pane's path, so paths inside the archive are relative
/// (you get `report.pdf` in the zip, not `/Users/me/proj/report.pdf`). All
/// srcs are bundled into one output archive — `CompressFinished` carries a
/// single Result<PathBuf>.
pub fn compress_task(
    srcs: Vec<PathBuf>,
    output: PathBuf,
    working_dir: PathBuf,
) -> Task<Message> {
    Task::perform(
        async move {
            let result_output = output.clone();
            tokio::task::spawn_blocking(move || run_zip(&srcs, &output, &working_dir))
                .await
                .unwrap_or_else(|e| Err(format!("zip task panicked: {}", e)))
                .map(|()| result_output)
        },
        Message::CompressFinished,
    )
}

fn run_zip(srcs: &[PathBuf], output: &Path, working_dir: &Path) -> Result<(), String> {
    let mut cmd = std::process::Command::new("zip");
    cmd.current_dir(working_dir).arg("-r").arg(output);
    for src in srcs {
        let Some(name) = src.file_name() else {
            return Err(format!("source has no file name: {}", src.display()));
        };
        cmd.arg(name);
    }
    let proc_out = cmd.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            "`zip` isn't installed (not found in PATH).".to_string()
        }
        _ => format!("failed to run `zip`: {}", e),
    })?;
    if !proc_out.status.success() {
        let stderr = String::from_utf8_lossy(&proc_out.stderr).into_owned();
        return Err(if stderr.trim().is_empty() {
            format!("`zip` exited with status {}", proc_out.status)
        } else {
            stderr.trim().to_string()
        });
    }
    Ok(())
}

/// Per-archive extraction: each `.zip` runs `unzip -d <dest>`, each
/// `.tar.gz` / `.tgz` runs `tar -xzf -C <dest>`. Unknown extensions return a
/// per-archive error so the rest still process. The `UncompressFinished`
/// message carries the full list of `(archive_path, result)`.
pub fn uncompress_task(archives: Vec<PathBuf>, dest_dir: PathBuf) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                archives
                    .into_iter()
                    .map(|archive| {
                        let res = run_extract(&archive, &dest_dir);
                        (archive, res)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default()
        },
        Message::UncompressFinished,
    )
}

fn run_extract(archive: &Path, dest: &Path) -> Result<(), String> {
    let name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let Some(format) = detect_archive_format(name) else {
        return Err(format!(
            "unsupported archive format: {} (only .zip / .tar.gz / .tgz)",
            name
        ));
    };
    let (prog, args): (&str, Vec<&std::ffi::OsStr>) = match format {
        ArchiveFormat::Zip => (
            "unzip",
            vec![archive.as_os_str(), "-d".as_ref(), dest.as_os_str()],
        ),
        ArchiveFormat::TarGz => (
            "tar",
            vec![
                "-xzf".as_ref(),
                archive.as_os_str(),
                "-C".as_ref(),
                dest.as_os_str(),
            ],
        ),
    };
    let proc_out = std::process::Command::new(prog)
        .args(&args)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                format!("`{}` isn't installed (not found in PATH).", prog)
            }
            _ => format!("failed to run `{}`: {}", prog, e),
        })?;
    if !proc_out.status.success() {
        let stderr = String::from_utf8_lossy(&proc_out.stderr).into_owned();
        return Err(if stderr.trim().is_empty() {
            format!("`{}` exited with status {}", prog, proc_out.status)
        } else {
            stderr.trim().to_string()
        });
    }
    Ok(())
}

/// Probe the directory for git status. Returns None when `path` isn't inside
/// a git repository (or when `git` is missing from PATH).
pub fn git_info_task(side: Side, path: PathBuf, generation: u64) -> Task<Message> {
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
// Docker
// ---------------------------------------------------------------------------

/// Output format passed to `docker ps`. Keep in sync with [`parse_docker_ps`]
/// — the field order and the literal `|` separator are load-bearing.
const DOCKER_PS_FORMAT: &str = "{{.ID}}|{{.Names}}|{{.Image}}|{{.Status}}";

/// Fetch the list of currently-running containers. Errors (docker not
/// installed, daemon not running) come back as `Err(message)` so the modal
/// can show a friendly explanation instead of an empty list.
pub fn docker_ps_task() -> Task<Message> {
    Task::perform(
        async {
            tokio::task::spawn_blocking(run_docker_ps)
                .await
                .unwrap_or_else(|e| Err(format!("docker ps task panicked: {}", e)))
        },
        Message::DockerListLoaded,
    )
}

fn run_docker_ps() -> Result<Vec<DockerContainer>, String> {
    let output = std::process::Command::new("docker")
        .args(["ps", "--format", DOCKER_PS_FORMAT])
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                "Docker doesn't appear to be installed (no `docker` binary in PATH).".to_string()
            }
            _ => format!("failed to run `docker ps`: {}", e),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let msg = if stderr.trim().is_empty() {
            format!("`docker ps` exited with status {}", output.status)
        } else {
            stderr.trim().to_string()
        };
        return Err(msg);
    }
    Ok(parse_docker_ps(&String::from_utf8_lossy(&output.stdout)))
}

/// `docker kill <id>`. Errors (e.g. container already gone) are mapped to a
/// string the App can surface.
pub fn docker_kill_task(id: String) -> Task<Message> {
    Task::perform(
        async move {
            let id_for_task = id.clone();
            let res = tokio::task::spawn_blocking(move || run_docker_kill(&id_for_task))
                .await
                .unwrap_or_else(|e| Err(format!("docker kill task panicked: {}", e)));
            (id, res)
        },
        |(id, res)| Message::DockerKillFinished(id, res),
    )
}

fn run_docker_kill(id: &str) -> Result<(), String> {
    let output = std::process::Command::new("docker")
        .args(["kill", id])
        .output()
        .map_err(|e| format!("failed to run `docker kill`: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(if stderr.trim().is_empty() {
            format!("`docker kill` exited with status {}", output.status)
        } else {
            stderr.trim().to_string()
        });
    }
    Ok(())
}

/// Open a new terminal window running `docker exec -it <id> /bin/sh`.
/// `/bin/sh` is used because it's universally available (Alpine images
/// typically lack bash).
pub fn docker_shell(id: &str, terminal_app: Option<&str>) -> Result<(), String> {
    spawn_terminal_with_command(
        "docker",
        &["exec", "-it", id, "/bin/sh"],
        terminal_app,
    )
}

/// Spawn a new terminal window that runs `prog <args>`. Shared by
/// `docker_shell` and `ssh_connect`. Returns once the terminal is *spawned*
/// — we don't follow its lifetime.
///
/// Per OS:
/// - macOS: `osascript`; the AppleScript dialect depends on the resolved
///   terminal app (see [`resolve_macos_terminal_app`]). iTerm gets the
///   modern `create window with default profile command "..."` form so
///   the command becomes the session's main process. Terminal.app falls
///   back to `do script "exec ..."`. Both arg-quote through
///   [`shell_quote`].
/// - Linux: `x-terminal-emulator -e prog arg1 arg2 …`. The terminal_app
///   setting is ignored for v1 (Linux terminals' flags aren't uniform).
/// - Windows: `cmd /C start cmd /K "prog arg1 arg2 …"`. Setting ignored.
fn spawn_terminal_with_command(
    prog: &str,
    args: &[&str],
    terminal_app: Option<&str>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = shell_quote(prog);
        for a in args {
            cmd.push(' ');
            cmd.push_str(&shell_quote(a));
        }
        let app = resolve_macos_terminal_app(terminal_app);
        let script = macos_terminal_apple_script(&app, &cmd);
        std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("failed to launch {}: {}", app, e))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = terminal_app;
        #[cfg(target_os = "linux")]
        {
            let mut full: Vec<&str> = vec!["-e", prog];
            full.extend_from_slice(args);
            std::process::Command::new("x-terminal-emulator")
                .args(&full)
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("failed to launch x-terminal-emulator: {}", e))
        }
        #[cfg(target_os = "windows")]
        {
            let mut joined = prog.to_string();
            for a in args {
                joined.push(' ');
                joined.push_str(a);
            }
            std::process::Command::new("cmd")
                .args(["/C", "start", "cmd", "/K"])
                .arg(joined)
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("failed to launch cmd: {}", e))
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let _ = (prog, args);
            Err("opening a terminal isn't supported on this platform".to_string())
        }
    }
}

/// Minimal escaping for embedding into AppleScript `do script "..."`.
/// Container IDs / SSH aliases are usually safe `[A-Za-z0-9_.-]+`, but
/// defense in depth doesn't cost us anything.
#[cfg(target_os = "macos")]
fn shell_quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Resolve which macOS terminal app to use. User-supplied setting wins;
/// otherwise prefer iTerm if installed, falling back to Terminal.app.
#[cfg(target_os = "macos")]
fn resolve_macos_terminal_app(setting: Option<&str>) -> String {
    if let Some(s) = setting {
        let s = s.trim();
        if !s.is_empty() {
            return s.to_string();
        }
    }
    if std::path::Path::new("/Applications/iTerm.app").exists() {
        "iTerm".to_string()
    } else {
        "Terminal".to_string()
    }
}

/// Build the AppleScript that launches `cmd` in the named terminal app.
/// `cmd` is the already-shell-quoted command string (e.g. `"ssh foo"`).
///
/// - **iTerm / iTerm2**: uses `create window with default profile command
///   "..."` (iTerm 3.x+). The command becomes the session's main process
///   — no shell wrapper, no shell prompt visible before it starts.
/// - **Anything else** (including `Terminal`): falls back to `do script
///   "exec ..."`. `do script` always wraps in a shell, but `exec`
///   immediately replaces it with the command so the shell flicker is
///   minimized.
fn macos_terminal_apple_script(app: &str, cmd: &str) -> String {
    let lowered = app.to_ascii_lowercase();
    if lowered == "iterm" || lowered == "iterm2" {
        format!(
            "tell application \"{app}\"\n    \
                 activate\n    \
                 create window with default profile command \"{cmd}\"\n\
             end tell"
        )
    } else {
        format!("tell application \"{app}\" to do script \"exec {cmd}\"")
    }
}

// ---------------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------------

/// Snapshot of currently-running processes via `ps -axo pid=,pcpu=,pmem=,comm=`.
/// Unix-only — Windows would need `tasklist`/WMIC and a different parser.
pub fn ps_task() -> Task<Message> {
    Task::perform(
        async {
            tokio::task::spawn_blocking(run_ps)
                .await
                .unwrap_or_else(|e| Err(format!("ps task panicked: {}", e)))
        },
        Message::ProcessesListLoaded,
    )
}

#[cfg(unix)]
fn run_ps() -> Result<Vec<Process>, String> {
    let output = std::process::Command::new("ps")
        // `=` empty header suppresses the header row. Order is load-bearing:
        // the variable-width `comm` column must come last so embedded spaces
        // in a Mac-style command name don't break the parser.
        .args(["-axo", "pid=,pcpu=,pmem=,comm="])
        .output()
        .map_err(|e| format!("failed to run `ps`: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(if stderr.trim().is_empty() {
            format!("`ps` exited with status {}", output.status)
        } else {
            stderr.trim().to_string()
        });
    }
    Ok(parse_ps_output(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(not(unix))]
fn run_ps() -> Result<Vec<Process>, String> {
    Err("Process listing isn't supported on this platform yet (needs tasklist/WMIC plumbing).".to_string())
}

/// Send SIGTERM (`kill <pid>`) to the given process. Unix-only.
pub fn kill_process_task(pid: u32) -> Task<Message> {
    Task::perform(
        async move {
            let res = tokio::task::spawn_blocking(move || run_kill(pid))
                .await
                .unwrap_or_else(|e| Err(format!("kill task panicked: {}", e)));
            (pid, res)
        },
        |(pid, res)| Message::ProcessKillFinished(pid, res),
    )
}

#[cfg(unix)]
fn run_kill(pid: u32) -> Result<(), String> {
    let output = std::process::Command::new("kill")
        .arg(pid.to_string())
        .output()
        .map_err(|e| format!("failed to run `kill`: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(if stderr.trim().is_empty() {
            format!("`kill` exited with status {}", output.status)
        } else {
            stderr.trim().to_string()
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn run_kill(_pid: u32) -> Result<(), String> {
    Err("Killing processes isn't supported on this platform yet.".to_string())
}

// ---------------------------------------------------------------------------
// Launch Application (macOS)
// ---------------------------------------------------------------------------

/// Discover `.app` bundles under the standard macOS application directories.
/// Sorted later by the App layer via `sort_apps`. macOS-only; other platforms
/// surface a friendly error in the modal.
pub fn apps_task() -> Task<Message> {
    Task::perform(
        async {
            tokio::task::spawn_blocking(scan_applications)
                .await
                .unwrap_or_else(|e| Err(format!("apps scan panicked: {}", e)))
        },
        Message::AppsListLoaded,
    )
}

#[cfg(target_os = "macos")]
fn scan_applications() -> Result<Vec<Application>, String> {
    let mut dirs: Vec<PathBuf> = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
    ];
    let home_apps = crate::config::home_dir().join("Applications");
    if home_apps.is_dir() {
        dirs.push(home_apps);
    }

    let mut out: Vec<Application> = Vec::new();
    for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("app") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            out.push(Application {
                path: path.clone(),
                name: name.to_string(),
            });
        }
    }
    if out.is_empty() {
        return Err("No .app bundles found in /Applications.".to_string());
    }
    Ok(out)
}

#[cfg(not(target_os = "macos"))]
fn scan_applications() -> Result<Vec<Application>, String> {
    Err("Launching applications is macOS-only.".to_string())
}

/// Open an application bundle via macOS's `open` command. Returns once
/// `open` has been spawned; we don't follow the launched app's lifetime.
pub fn launch_app(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("failed to launch {}: {}", path.display(), e))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err("Launching applications isn't supported on this platform.".to_string())
    }
}

// ---------------------------------------------------------------------------
// Git branches
// ---------------------------------------------------------------------------

/// `git for-each-ref --sort=-committerdate refs/heads/` for the repo
/// containing `repo_path`. Returns the branches most-recent-commit first;
/// the parser preserves arrival order so we don't need to sort again.
pub fn git_branches_task(repo_path: PathBuf) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || run_git_branches(&repo_path))
                .await
                .unwrap_or_else(|e| Err(format!("git branches task panicked: {}", e)))
        },
        Message::GitBranchesLoaded,
    )
}

fn run_git_branches(repo_path: &Path) -> Result<Vec<GitBranch>, String> {
    // `committerdate:short` keeps the per-row width small (YYYY-MM-DD).
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args([
            "for-each-ref",
            "--sort=-committerdate",
            "refs/heads/",
            "--format=%(refname:short)|%(committerdate:short)",
        ])
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                "`git` isn't installed (not found in PATH).".to_string()
            }
            _ => format!("failed to run `git for-each-ref`: {}", e),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(if stderr.trim().is_empty() {
            format!("`git for-each-ref` exited with status {}", output.status)
        } else {
            stderr.trim().to_string()
        });
    }
    Ok(parse_git_branches(&String::from_utf8_lossy(&output.stdout)))
}

/// `git -C <repo_path> checkout <branch>`. Errors (dirty working tree,
/// non-existent branch) come back as `Err(stderr)`.
pub fn git_checkout_task(repo_path: PathBuf, branch: String) -> Task<Message> {
    Task::perform(
        async move {
            let branch_clone = branch.clone();
            let res = tokio::task::spawn_blocking(move || {
                run_git_checkout(&repo_path, &branch_clone)
            })
            .await
            .unwrap_or_else(|e| Err(format!("git checkout task panicked: {}", e)));
            (branch, res)
        },
        |(branch, res)| Message::GitCheckoutFinished(branch, res),
    )
}

fn run_git_checkout(repo_path: &Path, branch: &str) -> Result<(), String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["checkout", branch])
        .output()
        .map_err(|e| format!("failed to run `git checkout`: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(if stderr.trim().is_empty() {
            format!("`git checkout` exited with status {}", output.status)
        } else {
            stderr.trim().to_string()
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// SSH servers
// ---------------------------------------------------------------------------

/// Read `~/.ssh/config` and parse it via [`parse_ssh_config`]. Returns the
/// list sorted by alias. Errors (missing file, unreadable) surface as
/// `Err(message)` so the modal shows a friendly explanation instead of an
/// empty list.
pub fn ssh_servers_task() -> Task<Message> {
    Task::perform(
        async {
            tokio::task::spawn_blocking(read_ssh_config)
                .await
                .unwrap_or_else(|e| Err(format!("ssh servers task panicked: {}", e)))
        },
        Message::SshServersLoaded,
    )
}

fn read_ssh_config() -> Result<Vec<SshServer>, String> {
    let path = crate::config::home_dir().join(".ssh").join("config");
    let contents = std::fs::read_to_string(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("No {} found.", path.display()),
        _ => format!("Failed to read {}: {}", path.display(), e),
    })?;
    let mut servers = parse_ssh_config(&contents);
    if servers.is_empty() {
        return Err(format!(
            "{} has no Host entries (or only wildcard defaults).",
            path.display()
        ));
    }
    crate::domain::sort_servers(&mut servers);
    Ok(servers)
}

/// Open a new terminal window running `ssh <alias>`. The alias is the
/// `Host` line from `~/.ssh/config`, so ssh itself resolves the actual
/// HostName / User / Port / etc. `terminal_app` is honored on macOS — see
/// [`spawn_terminal_with_command`] / [`macos_terminal_apple_script`].
pub fn ssh_connect(alias: &str, terminal_app: Option<&str>) -> Result<(), String> {
    spawn_terminal_with_command("ssh", &[alias], terminal_app)
}

// ---------------------------------------------------------------------------
// Open Claude Code
// ---------------------------------------------------------------------------

/// Open a new terminal window with `claude` running in `path`. Doesn't go
/// through [`spawn_terminal_with_command`] because that helper has no cwd
/// support and assumes whitespace-free args — both of which are awkward
/// when you need `cd '<path with spaces>' && exec claude`.
pub fn open_claude_code(path: &Path, terminal_app: Option<&str>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let app = resolve_macos_terminal_app(terminal_app);
        let script = macos_claude_apple_script(&app, &path.display().to_string());
        std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("failed to launch {}: {}", app, e))
    }
    #[cfg(target_os = "linux")]
    {
        // Spawning x-terminal-emulator with current_dir set means the
        // terminal — and the `claude` it exec's into — inherits that cwd.
        // No shell wrapper needed.
        let _ = terminal_app;
        std::process::Command::new("x-terminal-emulator")
            .current_dir(path)
            .args(["-e", "claude"])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("failed to launch x-terminal-emulator: {}", e))
    }
    #[cfg(target_os = "windows")]
    {
        let _ = terminal_app;
        std::process::Command::new("cmd")
            .current_dir(path)
            .args(["/C", "start", "cmd", "/K", "claude"])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("failed to launch cmd: {}", e))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (path, terminal_app);
        Err("opening a terminal isn't supported on this platform".to_string())
    }
}

/// Build the AppleScript that opens `app` with a session that has cd'd into
/// `path` and then exec'd `claude`. Same dispatch as
/// [`macos_terminal_apple_script`] (iTerm gets `create window … command`;
/// everything else falls back to Terminal-style `do script`).
///
/// `path` is single-quoted for the inner shell command so spaces / `$` / etc.
/// are safe. Single quotes in the path itself are escaped via the standard
/// `'\''` close-escape-reopen idiom. The whole shell command is then
/// AppleScript-escaped for the surrounding `"..."`.
fn macos_claude_apple_script(app: &str, path: &str) -> String {
    let single_quoted_path = path.replace('\'', "'\\''");
    let shell_cmd = format!("cd '{}' && exec claude", single_quoted_path);
    let escaped = shell_cmd.replace('\\', "\\\\").replace('"', "\\\"");
    let lowered = app.to_ascii_lowercase();
    if lowered == "iterm" || lowered == "iterm2" {
        // iTerm runs `command` as the session's argv (no shell wrapper), so
        // && / ' / etc. aren't interpreted. Wrap with `sh -c "<inner>"`.
        format!(
            "tell application \"{app}\"\n    \
                 activate\n    \
                 create window with default profile command \"sh -c \\\"{escaped}\\\"\"\n\
             end tell"
        )
    } else {
        // Terminal `do script` types into the user's shell, so && works as-is.
        format!("tell application \"{app}\" to do script \"{escaped}\"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn move_path_renames_a_file_within_same_dir() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&src, b"contents").unwrap();
        move_path(&src, &dst).unwrap();
        assert!(!src.exists(), "source should be gone after move");
        assert!(dst.exists(), "destination should exist");
        assert_eq!(std::fs::read(&dst).unwrap(), b"contents");
    }

    #[test]
    fn move_path_works_on_directory_trees() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir(&src).unwrap();
        std::fs::create_dir(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), b"a").unwrap();
        std::fs::write(src.join("sub/b.txt"), b"b").unwrap();

        move_path(&src, &dst).unwrap();

        assert!(!src.exists());
        assert!(dst.join("a.txt").exists());
        assert!(dst.join("sub/b.txt").exists());
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"a");
    }

    #[test]
    fn move_path_missing_source_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        let dst = dir.path().join("dst");
        assert!(move_path(&missing, &dst).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn shell_quote_escapes_quotes_and_backslashes() {
        // Plain container IDs round-trip unchanged.
        assert_eq!(shell_quote("abc123"), "abc123");
        // Embedded double-quote / backslash are escaped so the surrounding
        // AppleScript string stays well-formed.
        assert_eq!(shell_quote(r#"a"b"#), r#"a\"b"#);
        assert_eq!(shell_quote(r"a\b"), r"a\\b");
    }

    #[test]
    fn macos_terminal_script_iterm_uses_create_window_with_command() {
        let script = macos_terminal_apple_script("iTerm", "ssh alpha");
        // iTerm path: command becomes the session's main process (no shell wrapper).
        assert!(script.contains("tell application \"iTerm\""));
        assert!(script.contains("create window with default profile command \"ssh alpha\""));
        assert!(!script.contains("do script"));
    }

    #[test]
    fn macos_terminal_script_iterm2_alias_works_too() {
        // "iTerm2" should map to the iTerm dialect — same dispatch.
        let script = macos_terminal_apple_script("iTerm2", "ssh alpha");
        assert!(script.contains("create window with default profile command"));
    }

    #[test]
    fn macos_terminal_script_terminal_uses_do_script_with_exec() {
        let script = macos_terminal_apple_script("Terminal", "ssh alpha");
        // Terminal path: do script wraps in shell, exec replaces it ASAP.
        assert_eq!(
            script,
            "tell application \"Terminal\" to do script \"exec ssh alpha\""
        );
    }

    #[test]
    fn macos_terminal_script_unknown_app_falls_back_to_do_script() {
        // Best-effort for unknown app names: same do-script form, just with
        // their tell-application target.
        let script = macos_terminal_apple_script("Kitty", "ssh alpha");
        assert!(script.contains("tell application \"Kitty\""));
        assert!(script.contains("do script \"exec ssh alpha\""));
    }

    #[test]
    fn macos_claude_script_terminal_uses_do_script_with_cd_and_exec() {
        let script = macos_claude_apple_script("Terminal", "/Users/me/project");
        assert_eq!(
            script,
            "tell application \"Terminal\" to do script \"cd '/Users/me/project' && exec claude\""
        );
    }

    #[test]
    fn macos_claude_script_iterm_wraps_in_sh_dash_c() {
        let script = macos_claude_apple_script("iTerm", "/Users/me/project");
        // iTerm's `command` doesn't run through a shell, so && + single
        // quotes need a `sh -c "..."` wrapper for correct interpretation.
        assert!(script.contains("create window with default profile command"));
        assert!(script.contains("sh -c \\\"cd '/Users/me/project' && exec claude\\\""));
    }

    #[test]
    fn macos_claude_script_quotes_path_with_spaces() {
        let script = macos_claude_apple_script("Terminal", "/Users/me/My Projects/rho");
        // The path is single-quoted in the shell command so spaces stay
        // inside a single shell-argument.
        assert!(script.contains("cd '/Users/me/My Projects/rho' && exec claude"));
    }

    #[test]
    fn macos_claude_script_escapes_single_quote_in_path() {
        // Path containing a literal ' uses the close-escape-reopen idiom.
        let script = macos_claude_apple_script("Terminal", "/Users/me/it's/here");
        // Shell-level: cd 'it'\''s'  — at AppleScript layer the backslash is
        // doubled because shell_quote escapes \ for AppleScript embedding.
        assert!(script.contains("'/Users/me/it'\\\\''s/here'"));
    }
}
