//! All filesystem and external-process I/O: directory streaming, copy/delete
//! tasks, git probing, and the `notify`-based watcher subscription. Each task
//! returns `Task<Message>` / `Subscription<Message>` so the App layer can wire
//! the results in via `update()`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::{Subscription, Task};

use crate::domain::{Entry, GitInfo, Side};
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
}
