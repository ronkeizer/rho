//! All filesystem and external-process I/O: directory streaming, copy/delete
//! tasks, git probing, and the `notify`-based watcher subscription. Each task
//! returns `Task<Message>` / `Subscription<Message>` so the App layer can wire
//! the results in via `update()`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::{Subscription, Task};

use crate::config::{Config, DropboxAuth};
use crate::domain::{
    detect_archive_format, dropbox_api_path, parse_docker_ps, parse_dropbox_list,
    parse_dropbox_token, parse_git_branches, parse_ls_la, parse_ps_output, parse_ssh_config,
    Application, ArchiveFormat,
    BackendId, DockerContainer, Entry, GitBranch, GitInfo, Location, Process, Side, SshServer,
};
use crate::Message;

/// Both side-loads (directory entries + git info) for a single pane.
///
/// Dispatches on `location`:
/// - `Local(path)` → local read_dir stream + git probe.
/// - `Remote { backend, path }` → `ssh <backend> ls -la --time-style=full-iso`.
///   No git info (Phase 2 MVP — remote git probing is a later add).
pub fn loading_tasks(side: Side, location: Location, generation: u64) -> Task<Message> {
    match location {
        Location::Local(path) => Task::batch([
            load_dir_task(side, path.clone(), generation),
            git_info_task(side, path, generation),
        ]),
        Location::Remote { backend, path } if backend.is_dropbox() => {
            load_dropbox_dir_task(side, path, generation)
        }
        Location::Remote { backend, path } => {
            load_remote_dir_task(side, backend, path, generation)
        }
    }
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

/// Remote sibling of [`load_dir_task`]: spawn
/// `ssh <backend> ls -la --time-style=full-iso -- <quoted-path>` in a
/// blocking thread, parse the stdout with [`parse_ls_la`], and emit a
/// single `EntriesChunk` (all entries at once) followed by `EntriesDone`.
///
/// Errors (ssh exit non-zero, ssh missing, parse yields zero entries)
/// surface as an empty chunk + done so the receiver still completes
/// and the pane just shows "no entries" — same fail-soft posture as
/// the local task when `read_dir` errors.
///
/// Phase 2 MVP: target hosts need GNU `ls` (i.e. Linux). BSD/macOS hosts
/// won't recognise `--time-style=full-iso` and the parse will return
/// empty; the error appears on stderr for debugging.
pub fn load_remote_dir_task(
    side: Side,
    backend: BackendId,
    path: PathBuf,
    generation: u64,
) -> Task<Message> {
    use iced::futures::stream::{self, StreamExt};

    let entries_msg = async move {
        let entries = tokio::task::spawn_blocking(move || run_remote_ls(&backend, &path))
            .await
            .unwrap_or_else(|e| {
                eprintln!("remote ls task panicked: {}", e);
                Vec::new()
            });
        Message::EntriesChunk(side, generation, entries)
    };
    let chunk = stream::once(entries_msg);
    let done = stream::once(async move { Message::EntriesDone(side, generation) });
    Task::stream(chunk.chain(done))
}

fn run_remote_ls(backend: &BackendId, path: &Path) -> Vec<Entry> {
    let quoted = quote_remote_path(path);
    // Build the remote command as a single argv; ssh will join + the
    // remote shell will re-tokenise, which is why `quoted` is shell-safe
    // (single-quoted, with a `~`-passthrough so home expansion still
    // works).
    let remote_cmd = format!("ls -la --time-style=full-iso -- {}", quoted);
    let output = std::process::Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            backend.as_str(),
            "--",
            remote_cmd.as_str(),
        ])
        .output();
    match output {
        Ok(out) if out.status.success() => parse_ls_la(&String::from_utf8_lossy(&out.stdout)),
        Ok(out) => {
            eprintln!(
                "ssh {} ls failed ({}): {}",
                backend.as_str(),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim(),
            );
            Vec::new()
        }
        Err(e) => {
            eprintln!("failed to spawn ssh for {}: {}", backend.as_str(), e);
            Vec::new()
        }
    }
}

/// POSIX single-quote a string for safe interpolation into a remote
/// shell command. Embeds the input verbatim except for `'`, which is
/// turned into the canonical `'\''` close-reopen sequence.
fn posix_quote(s: &str) -> String {
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

/// Quote a remote path for inclusion in an ssh-dispatched command, but
/// leave a leading `~` / `~user` segment unquoted so the remote shell
/// still expands it. Everything from the first `/` onward is single-
/// quoted by [`posix_quote`].
fn quote_remote_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s_ref: &str = &s;
    if let Some(after_tilde) = s_ref.strip_prefix('~') {
        match after_tilde.split_once('/') {
            None => s.into_owned(),
            Some((tilde_tail, rest)) => format!("~{}/{}", tilde_tail, posix_quote(rest)),
        }
    } else {
        posix_quote(s_ref)
    }
}

/// Quote a remote path for an **sftp batch command** (`get` / `put`).
///
/// Unlike [`quote_remote_path`], there is no remote shell here — the
/// sftp protocol performs no `~` expansion. But its remote working
/// directory already defaults to the login user's home, so rho's
/// default remote location (`~`) maps cleanly onto sftp-relative paths:
///
/// - `~`        → `.`              (the home directory itself)
/// - `~/rest`   → `'rest'`         (relative to home, single-quoted)
/// - `/abs`     → `'/abs'`         (absolute, single-quoted)
///
/// Passing `~/foo` through literally (as `quote_remote_path` does for
/// the shell-backed `cp`/`mv`/`rm`/`ls` calls) makes sftp resolve it as
/// `<home>/~/foo` and fail with "not found" — that's the bug this
/// avoids. `~user/...` can't be resolved without a shell and rho never
/// generates it (panes open at `~`), so it falls through to a literal
/// quote.
fn quote_sftp_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s_ref: &str = &s;
    if s_ref == "~" {
        return ".".to_string();
    }
    if let Some(rest) = s_ref.strip_prefix("~/") {
        return posix_quote(rest);
    }
    posix_quote(s_ref)
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

/// Subscription that watches the panes' currently-open directories
/// (non-recursive) and emits a coalesced [`Message::WatchedDirChanged`] per
/// folder when files are created, removed, or renamed there — so the listing
/// auto-refreshes on external changes. A burst (e.g. 500 files moved in)
/// collapses into a single refresh via a quiet-window debounce, with a hard
/// cap so a sustained stream still refreshes periodically rather than starving.
///
/// Differs from [`file_watch_subscription`] (the configured-`watch_folders`
/// new-file detector) in two ways: it reacts to removes/renames too, and its
/// folder set changes as the user navigates — the caller keys the
/// subscription id off `folders` so iced restarts the watcher on the new set.
pub fn pane_watch_subscription(folders: Vec<PathBuf>) -> Subscription<Message> {
    use iced::futures::SinkExt;
    use iced::stream;
    use notify::{
        event::ModifyKind, recommended_watcher, Event, EventKind, RecursiveMode, Watcher,
    };
    use std::collections::HashSet;
    use std::time::Instant;

    Subscription::run_with_id(
        ("pane-dir-watch", folders.clone()),
        stream::channel(64, move |mut output| async move {
            let (raw_tx, mut raw_rx) = tokio::sync::mpsc::channel::<Event>(256);

            let mut watcher = match recommended_watcher(move |res| {
                if let Ok(event) = res {
                    let _ = raw_tx.blocking_send(event);
                }
            }) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("pane watcher: init failed: {}", e);
                    return;
                }
            };

            for folder in &folders {
                if let Err(e) = watcher.watch(folder, RecursiveMode::NonRecursive) {
                    eprintln!("pane watcher: skipping {}: {}", folder.display(), e);
                }
            }

            // Coalesce changes per folder. `quiet` collapses a burst (the
            // deadline is pushed out on every event); `max_wait` bounds the
            // total delay so a long-running stream of changes still flushes.
            let mut changed: HashSet<PathBuf> = HashSet::new();
            let mut quiet_deadline: Option<Instant> = None;
            let mut hard_deadline: Option<Instant> = None;
            let quiet = Duration::from_millis(300);
            let max_wait = Duration::from_millis(1500);
            let idle_timeout = Duration::from_secs(3600);

            loop {
                let wait = match (quiet_deadline, hard_deadline) {
                    (Some(q), Some(h)) => q.min(h).saturating_duration_since(Instant::now()),
                    _ => idle_timeout,
                };

                tokio::select! {
                    maybe_evt = raw_rx.recv() => {
                        let Some(event) = maybe_evt else { break };
                        // Structural changes to the listing: add / remove /
                        // rename. Content/metadata edits are intentionally
                        // ignored (chatty, and don't change membership).
                        let relevant = matches!(
                            event.kind,
                            EventKind::Create(_)
                                | EventKind::Remove(_)
                                | EventKind::Modify(ModifyKind::Name(_))
                        );
                        if !relevant {
                            continue;
                        }
                        for path in event.paths {
                            if let Some(name) =
                                path.file_name().map(|n| n.to_string_lossy().into_owned())
                            {
                                // Skip noise (.DS_Store, in-progress downloads)
                                // so it doesn't trigger spurious refreshes.
                                if is_ignored_watch_filename(&name) {
                                    continue;
                                }
                            }
                            // Non-recursive: only changes directly inside a
                            // watched folder count (some backends report
                            // adjacent paths).
                            if let Some(parent) = path.parent() {
                                if folders.iter().any(|f| f == parent) {
                                    changed.insert(parent.to_path_buf());
                                }
                            }
                        }
                        if !changed.is_empty() {
                            let now = Instant::now();
                            quiet_deadline = Some(now + quiet);
                            hard_deadline.get_or_insert(now + max_wait);
                        }
                    }
                    _ = tokio::time::sleep(wait), if quiet_deadline.is_some() => {
                        for folder in changed.drain() {
                            let _ = output.send(Message::WatchedDirChanged(folder)).await;
                        }
                        quiet_deadline = None;
                        hard_deadline = None;
                    }
                }
            }

            drop(watcher);
        }),
    )
}

/// Copy a batch of sources into `dst`. By construction the caller pulls
/// `srcs` from the active pane's `marked_locations`, so every source
/// shares the same backend (all `Local`, or all `Remote` with the same
/// `BackendId`). The dispatch matches on `(srcs[0].is_local, dst)`:
///
/// - `Local → Local` → in-process `copy_recursive`.
/// - `Local → Remote` → one `sftp put -r` per source.
/// - `Remote → Local` → one `sftp get -r` per source.
/// - `Remote → Remote` same host → `ssh <alias> cp -r` per source.
/// - `Remote → Remote` cross-host → stage through local `/tmp` via
///   `get` then `put` (slow, but rare).
///
/// Per-source results come back as `(source, Result<(), String>)` so
/// the receiver can log partial failures.
pub fn copy_task(srcs: Vec<Location>, dst: Location) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || run_copy(srcs, dst))
                .await
                .unwrap_or_default()
        },
        Message::CopyFinished,
    )
}

/// Move-with-Location dispatch. Same backend combos as [`copy_task`];
/// when source and destination share a remote backend the implementation
/// short-circuits to `ssh <alias> mv` (atomic, preserves inodes).
/// All other combinations are "copy then delete source on success".
pub fn move_task(srcs: Vec<Location>, dst: Location) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || run_move(srcs, dst))
                .await
                .unwrap_or_default()
        },
        Message::MoveFinished,
    )
}

/// Delete a batch of locations. Local sources go through the existing
/// `delete_path` helper; remote sources fan out to
/// `ssh <alias> rm -rf -- <path>` (one call per source so we can return
/// per-source results).
pub fn delete_task(srcs: Vec<Location>) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || run_delete(srcs))
                .await
                .unwrap_or_default()
        },
        Message::DeleteFinished,
    )
}

/// Which transport a [`Location`] resolves to, carrying the SSH backend
/// id where it matters for same-host shortcuts.
enum Transport {
    Local,
    Ssh(BackendId),
    Dropbox,
}

fn transport(loc: &Location) -> Transport {
    match loc {
        Location::Local(_) => Transport::Local,
        Location::Remote { backend, .. } if backend.is_dropbox() => Transport::Dropbox,
        Location::Remote { backend, .. } => Transport::Ssh(backend.clone()),
    }
}

fn run_copy(srcs: Vec<Location>, dst: Location) -> Vec<(Location, Result<(), String>)> {
    use Transport::*;
    let src_t = match srcs.first() {
        Some(s) => transport(s),
        None => return Vec::new(),
    };
    match (src_t, transport(&dst), &dst) {
        (Local, Local, Location::Local(dst_dir)) => copy_local_to_local(srcs, dst_dir),
        (Local, Ssh(_), Location::Remote { backend, path }) => {
            sftp_put_each(srcs, backend.clone(), path.clone())
        }
        (Local, Dropbox, Location::Remote { path, .. }) => dropbox_upload_each(srcs, path.clone()),
        (Ssh(be), Local, Location::Local(dst_dir)) => sftp_get_each(srcs, be, dst_dir.clone()),
        (Dropbox, Local, Location::Local(dst_dir)) => {
            dropbox_download_each(srcs, dst_dir.clone())
        }
        (Ssh(src_be), Ssh(_), Location::Remote { backend: dst_be, path }) => {
            if src_be == *dst_be {
                ssh_cp_each(srcs, src_be, path.clone())
            } else {
                cross_host_copy_each(srcs, src_be, dst_be.clone(), path.clone())
            }
        }
        (Dropbox, Dropbox, Location::Remote { path, .. }) => {
            dropbox_transfer_each(srcs, path.clone(), DropboxTransfer::Copy)
        }
        // Mixed remote backends (Dropbox ↔ SSH): stage through local /tmp.
        _ => stage_copy_each(srcs, dst),
    }
}

fn run_move(srcs: Vec<Location>, dst: Location) -> Vec<(Location, Result<(), String>)> {
    use Transport::*;
    // Same-backend moves are atomic server-side (ssh mv / Dropbox
    // move_v2) and preserve identity. Local→Local keeps the
    // rename-or-copy+delete primitive. Every other combination falls
    // through to copy-then-delete-source.
    let src_t = match srcs.first() {
        Some(s) => transport(s),
        None => return Vec::new(),
    };
    match (&src_t, transport(&dst), &dst) {
        (Local, Local, Location::Local(dst_dir)) => return move_local_to_local(srcs, dst_dir),
        (Ssh(src_be), Ssh(_), Location::Remote { backend: dst_be, path }) if src_be == dst_be => {
            return ssh_mv_each(srcs, src_be.clone(), path.clone());
        }
        (Dropbox, Dropbox, Location::Remote { path, .. }) => {
            return dropbox_transfer_each(srcs, path.clone(), DropboxTransfer::Move);
        }
        _ => {}
    }
    // Mixed-backend / local↔remote: copy first, then if the copy
    // succeeded delete that source.
    run_copy(srcs, dst)
        .into_iter()
        .map(|(src, res)| match res {
            Ok(()) => {
                let del = delete_one(&src);
                (src, del)
            }
            Err(e) => (src, Err(e)),
        })
        .collect()
}

fn run_delete(srcs: Vec<Location>) -> Vec<(Location, Result<(), String>)> {
    srcs.into_iter()
        .map(|loc| {
            let res = delete_one(&loc);
            (loc, res)
        })
        .collect()
}

fn copy_local_to_local(
    srcs: Vec<Location>,
    dst_dir: &Path,
) -> Vec<(Location, Result<(), String>)> {
    srcs.into_iter()
        .map(|loc| {
            let src_path = match &loc {
                Location::Local(p) => p.clone(),
                _ => return (loc, Err("expected local source".to_string())),
            };
            let name = match src_path.file_name() {
                Some(n) => n.to_owned(),
                None => return (loc, Err("source has no file name".to_string())),
            };
            let target = dst_dir.join(name);
            let res = copy_recursive(&src_path, &target).map_err(|e| e.to_string());
            (loc, res)
        })
        .collect()
}

fn move_local_to_local(
    srcs: Vec<Location>,
    dst_dir: &Path,
) -> Vec<(Location, Result<(), String>)> {
    srcs.into_iter()
        .map(|loc| {
            let src_path = match &loc {
                Location::Local(p) => p.clone(),
                _ => return (loc, Err("expected local source".to_string())),
            };
            let name = match src_path.file_name() {
                Some(n) => n.to_owned(),
                None => return (loc, Err("source has no file name".to_string())),
            };
            let target = dst_dir.join(name);
            let res = move_path(&src_path, &target).map_err(|e| e.to_string());
            (loc, res)
        })
        .collect()
}

fn delete_one(loc: &Location) -> Result<(), String> {
    match loc {
        Location::Local(p) => delete_path(p).map_err(|e| e.to_string()),
        Location::Remote { backend, path } if backend.is_dropbox() => dropbox_delete(path),
        Location::Remote { backend, path } => ssh_rm_recursive(backend, path),
    }
}

// ---------------------------------------------------------------------------
// Remote copy / move / delete dispatchers (sftp + ssh subprocesses)
// ---------------------------------------------------------------------------

fn sftp_put_each(
    srcs: Vec<Location>,
    backend: BackendId,
    dst_path: PathBuf,
) -> Vec<(Location, Result<(), String>)> {
    let dst_quoted = quote_sftp_path(&dst_path);
    srcs.into_iter()
        .map(|loc| {
            let src_path = match &loc {
                Location::Local(p) => p.clone(),
                _ => return (loc, Err("expected local source".to_string())),
            };
            let local_quoted = posix_quote(&src_path.display().to_string());
            let script = build_sftp_put_script(&local_quoted, &dst_quoted);
            let res = run_sftp_batch(&backend, &script);
            (loc, res)
        })
        .collect()
}

fn sftp_get_each(
    srcs: Vec<Location>,
    backend: BackendId,
    dst_dir: PathBuf,
) -> Vec<(Location, Result<(), String>)> {
    let local_dst_quoted = posix_quote(&dst_dir.display().to_string());
    srcs.into_iter()
        .map(|loc| {
            let remote_path = match &loc {
                Location::Remote { path, .. } => path.clone(),
                _ => return (loc, Err("expected remote source".to_string())),
            };
            let remote_quoted = quote_sftp_path(&remote_path);
            let script = build_sftp_get_script(&remote_quoted, &local_dst_quoted);
            let res = run_sftp_batch(&backend, &script);
            (loc, res)
        })
        .collect()
}

fn ssh_cp_each(
    srcs: Vec<Location>,
    backend: BackendId,
    dst_path: PathBuf,
) -> Vec<(Location, Result<(), String>)> {
    let dst_quoted = quote_remote_path(&dst_path);
    srcs.into_iter()
        .map(|loc| {
            let remote_src = match &loc {
                Location::Remote { path, .. } => path.clone(),
                _ => return (loc, Err("expected remote source".to_string())),
            };
            let src_quoted = quote_remote_path(&remote_src);
            let cmd = format!("cp -r -- {} {}", src_quoted, dst_quoted);
            let res = run_ssh_command(&backend, &cmd);
            (loc, res)
        })
        .collect()
}

fn ssh_mv_each(
    srcs: Vec<Location>,
    backend: BackendId,
    dst_path: PathBuf,
) -> Vec<(Location, Result<(), String>)> {
    let dst_quoted = quote_remote_path(&dst_path);
    srcs.into_iter()
        .map(|loc| {
            let remote_src = match &loc {
                Location::Remote { path, .. } => path.clone(),
                _ => return (loc, Err("expected remote source".to_string())),
            };
            let src_quoted = quote_remote_path(&remote_src);
            let cmd = format!("mv -- {} {}", src_quoted, dst_quoted);
            let res = run_ssh_command(&backend, &cmd);
            (loc, res)
        })
        .collect()
}

fn ssh_rm_recursive(backend: &BackendId, path: &Path) -> Result<(), String> {
    let quoted = quote_remote_path(path);
    let cmd = format!("rm -rf -- {}", quoted);
    run_ssh_command(backend, &cmd)
}

/// Cross-host R→R copy: stage each source through a fresh `/tmp`
/// directory on the local machine. Slower than same-host `ssh cp` but
/// avoids depending on `ssh -A` agent forwarding or
/// `ProxyJump`-style routing.
fn cross_host_copy_each(
    srcs: Vec<Location>,
    src_backend: BackendId,
    dst_backend: BackendId,
    dst_path: PathBuf,
) -> Vec<(Location, Result<(), String>)> {
    srcs.into_iter()
        .map(|loc| {
            let remote_src = match &loc {
                Location::Remote { path, .. } => path.clone(),
                _ => return (loc, Err("expected remote source".to_string())),
            };
            let res = cross_host_copy_one(&src_backend, &dst_backend, &remote_src, &dst_path);
            (loc, res)
        })
        .collect()
}

fn cross_host_copy_one(
    src_backend: &BackendId,
    dst_backend: &BackendId,
    remote_src: &Path,
    dst_path: &Path,
) -> Result<(), String> {
    let staging =
        make_staging_dir().map_err(|e| format!("failed to create staging dir: {}", e))?;
    let stage_result = (|| -> Result<(), String> {
        let remote_quoted = quote_sftp_path(remote_src);
        let staging_quoted = posix_quote(&staging.display().to_string());
        let get = build_sftp_get_script(&remote_quoted, &staging_quoted);
        run_sftp_batch(src_backend, &get)?;
        let basename = remote_src
            .file_name()
            .ok_or_else(|| "source has no file name".to_string())?;
        let staged = staging.join(basename);
        let staged_quoted = posix_quote(&staged.display().to_string());
        let dst_quoted = quote_sftp_path(dst_path);
        let put = build_sftp_put_script(&staged_quoted, &dst_quoted);
        run_sftp_batch(dst_backend, &put)
    })();
    // Best-effort cleanup; we don't surface a remove failure if the
    // copy itself succeeded.
    let _ = std::fs::remove_dir_all(&staging);
    stage_result
}

fn make_staging_dir() -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rho-stage-{}-{}", pid, count));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Pure batch-script builders, kept separate so they can be unit-tested
/// without spawning sftp.
fn build_sftp_put_script(local_quoted: &str, remote_quoted: &str) -> String {
    format!("put -r {} {}\n", local_quoted, remote_quoted)
}

fn build_sftp_get_script(remote_quoted: &str, local_quoted: &str) -> String {
    format!("get -r {} {}\n", remote_quoted, local_quoted)
}

fn run_sftp_batch(backend: &BackendId, script: &str) -> Result<(), String> {
    use std::io::Write;
    let mut child = std::process::Command::new("sftp")
        .args(["-b", "-", "-o", "BatchMode=yes", backend.as_str()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn sftp: {}", e))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "sftp stdin missing".to_string())?;
        stdin
            .write_all(script.as_bytes())
            .map_err(|e| format!("failed to write sftp script: {}", e))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("sftp wait failed: {}", e))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("sftp exited with status {}", output.status)
        } else {
            stderr
        })
    }
}

fn run_ssh_command(backend: &BackendId, remote_cmd: &str) -> Result<(), String> {
    let output = std::process::Command::new("ssh")
        .args(["-o", "BatchMode=yes", backend.as_str(), "--", remote_cmd])
        .output()
        .map_err(|e| format!("failed to spawn ssh: {}", e))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("ssh exited with status {}", output.status)
        } else {
            stderr
        })
    }
}

// ---------------------------------------------------------------------------
// Dropbox backend (HTTP API via curl)
//
// Mirrors the SSH backend's "shell out to a subprocess" posture: every
// call spawns `curl`, so there's no async HTTP client in the dep tree.
// Access tokens are minted from the configured refresh token and cached
// in-process. Pure JSON parsing lives in `domain` (and is unit-tested);
// everything here is I/O and therefore exercised manually.
// ---------------------------------------------------------------------------

/// Cached short-lived access token, keyed by nothing (single account).
/// Refreshed on demand once it's within a minute of expiry.
static DROPBOX_TOKEN: std::sync::Mutex<Option<CachedToken>> = std::sync::Mutex::new(None);

struct CachedToken {
    token: String,
    expires_at: std::time::Instant,
}

/// Remote sibling of [`load_remote_dir_task`] for the Dropbox backend:
/// page through `list_folder` in a blocking thread and emit a single
/// `EntriesChunk` + `EntriesDone`. Same fail-soft posture — errors log to
/// stderr and the pane just shows "no entries".
pub fn load_dropbox_dir_task(side: Side, path: PathBuf, generation: u64) -> Task<Message> {
    use iced::futures::stream::{self, StreamExt};

    let entries_msg = async move {
        let entries = tokio::task::spawn_blocking(move || run_dropbox_ls(&path))
            .await
            .unwrap_or_else(|e| {
                eprintln!("dropbox list task panicked: {}", e);
                Vec::new()
            });
        Message::EntriesChunk(side, generation, entries)
    };
    let chunk = stream::once(entries_msg);
    let done = stream::once(async move { Message::EntriesDone(side, generation) });
    Task::stream(chunk.chain(done))
}

fn run_dropbox_ls(path: &Path) -> Vec<Entry> {
    match dropbox_list_api(&dropbox_api_path(path)) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("dropbox list {} failed: {}", path.display(), e);
            Vec::new()
        }
    }
}

/// List every child of `api_path` (`""` for the account root), following
/// `has_more` cursors via `list_folder/continue`.
fn dropbox_list_api(api_path: &str) -> Result<Vec<Entry>, String> {
    let body = serde_json::json!({ "path": api_path, "recursive": false }).to_string();
    let mut listing = parse_dropbox_list(&dropbox_rpc("files/list_folder", &body)?)?;
    let mut entries = listing.entries;
    while let Some(cursor) = listing.cursor {
        let body = serde_json::json!({ "cursor": cursor }).to_string();
        listing = parse_dropbox_list(&dropbox_rpc("files/list_folder/continue", &body)?)?;
        entries.extend(listing.entries);
    }
    Ok(entries)
}

// --- copy / move / delete primitives ---------------------------------------

enum DropboxTransfer {
    Copy,
    Move,
}

fn dropbox_delete(path: &Path) -> Result<(), String> {
    let body = serde_json::json!({ "path": dropbox_api_path(path) }).to_string();
    dropbox_rpc("files/delete_v2", &body).map(|_| ())
}

/// Server-side copy/move of each source into the `dst_dir` folder,
/// preserving its basename. Both sides share the one Dropbox account.
fn dropbox_transfer_each(
    srcs: Vec<Location>,
    dst_dir: PathBuf,
    kind: DropboxTransfer,
) -> Vec<(Location, Result<(), String>)> {
    let dst_dir_api = dropbox_api_path(&dst_dir);
    let endpoint = match kind {
        DropboxTransfer::Copy => "files/copy_v2",
        DropboxTransfer::Move => "files/move_v2",
    };
    srcs.into_iter()
        .map(|loc| {
            let res = (|| {
                let src_path = match &loc {
                    Location::Remote { path, .. } => path.clone(),
                    _ => return Err("expected dropbox source".to_string()),
                };
                let name = src_path
                    .file_name()
                    .ok_or_else(|| "source has no file name".to_string())?
                    .to_string_lossy()
                    .into_owned();
                let body = serde_json::json!({
                    "from_path": dropbox_api_path(&src_path),
                    "to_path": join_dropbox(&dst_dir_api, &name),
                })
                .to_string();
                dropbox_rpc(endpoint, &body).map(|_| ())
            })();
            (loc, res)
        })
        .collect()
}

// --- upload (Local → Dropbox) ----------------------------------------------

fn dropbox_upload_each(
    srcs: Vec<Location>,
    dst_dir: PathBuf,
) -> Vec<(Location, Result<(), String>)> {
    let dst_dir_api = dropbox_api_path(&dst_dir);
    srcs.into_iter()
        .map(|loc| {
            let res = (|| {
                let src_path = match &loc {
                    Location::Local(p) => p.clone(),
                    _ => return Err("expected local source".to_string()),
                };
                let name = src_path
                    .file_name()
                    .ok_or_else(|| "source has no file name".to_string())?
                    .to_string_lossy()
                    .into_owned();
                dropbox_upload_recursive(&src_path, &join_dropbox(&dst_dir_api, &name))
            })();
            (loc, res)
        })
        .collect()
}

fn dropbox_upload_recursive(local: &Path, dbx: &str) -> Result<(), String> {
    let meta = std::fs::metadata(local).map_err(|e| e.to_string())?;
    if meta.is_dir() {
        dropbox_create_folder(dbx)?;
        for entry in std::fs::read_dir(local).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let child_name = entry.file_name().to_string_lossy().into_owned();
            dropbox_upload_recursive(&entry.path(), &join_dropbox(dbx, &child_name))?;
        }
        Ok(())
    } else {
        dropbox_upload_file(local, dbx)
    }
}

/// Single-request upload to the content endpoint. Fine for the typical
/// file-manager payload; files over Dropbox's 150 MB single-shot limit
/// would need `upload_session` (not yet implemented).
fn dropbox_upload_file(local: &Path, dbx: &str) -> Result<(), String> {
    let token = dropbox_access_token()?;
    let auth = format!("Authorization: Bearer {}", token);
    let arg = format!(
        "Dropbox-API-Arg: {}",
        serde_json::json!({ "path": dbx, "mode": "overwrite", "mute": true })
    );
    // `@<path>` makes curl read the request body from the file; passing it
    // as its own argv token keeps paths with spaces intact (no shell).
    let data = format!("@{}", local.display());
    let result = run_curl(&[
        "-X",
        "POST",
        "https://content.dropboxapi.com/2/files/upload",
        "-H",
        &auth,
        "-H",
        &arg,
        "-H",
        "Content-Type: application/octet-stream",
        "--data-binary",
        &data,
    ])?;
    if (200..300).contains(&result.status) {
        Ok(())
    } else {
        Err(dropbox_error_message(&result.body, result.status))
    }
}

fn dropbox_create_folder(dbx: &str) -> Result<(), String> {
    let body = serde_json::json!({ "path": dbx, "autorename": false }).to_string();
    match dropbox_rpc("files/create_folder_v2", &body) {
        Ok(_) => Ok(()),
        // A pre-existing folder is fine for a recursive upload.
        Err(e) if e.contains("conflict") => Ok(()),
        Err(e) => Err(e),
    }
}

// --- download (Dropbox → Local) --------------------------------------------

fn dropbox_download_each(
    srcs: Vec<Location>,
    dst_dir: PathBuf,
) -> Vec<(Location, Result<(), String>)> {
    srcs.into_iter()
        .map(|loc| {
            let res = (|| {
                let src_path = match &loc {
                    Location::Remote { path, .. } => path.clone(),
                    _ => return Err("expected dropbox source".to_string()),
                };
                let name = src_path
                    .file_name()
                    .ok_or_else(|| "source has no file name".to_string())?
                    .to_owned();
                dropbox_download_recursive(&dropbox_api_path(&src_path), &dst_dir.join(name))
            })();
            (loc, res)
        })
        .collect()
}

fn dropbox_download_recursive(dbx: &str, local: &Path) -> Result<(), String> {
    if dropbox_is_folder(dbx)? {
        std::fs::create_dir_all(local).map_err(|e| e.to_string())?;
        for child in dropbox_list_api(dbx)? {
            dropbox_download_recursive(&join_dropbox(dbx, &child.name), &local.join(&child.name))?;
        }
        Ok(())
    } else {
        dropbox_download_file(dbx, local)
    }
}

fn dropbox_is_folder(dbx: &str) -> Result<bool, String> {
    let body = serde_json::json!({ "path": dbx }).to_string();
    let resp = dropbox_rpc("files/get_metadata", &body)?;
    let v: serde_json::Value = serde_json::from_str(&resp).map_err(|e| e.to_string())?;
    Ok(v.get(".tag").and_then(|t| t.as_str()) == Some("folder"))
}

fn dropbox_download_file(dbx: &str, local: &Path) -> Result<(), String> {
    let token = dropbox_access_token()?;
    let auth = format!("Authorization: Bearer {}", token);
    let arg = format!("Dropbox-API-Arg: {}", serde_json::json!({ "path": dbx }));
    let local_str = local.display().to_string();
    let result = run_curl(&[
        "-X",
        "POST",
        "https://content.dropboxapi.com/2/files/download",
        "-H",
        &auth,
        "-H",
        &arg,
        "-o",
        &local_str,
    ])?;
    if (200..300).contains(&result.status) {
        Ok(())
    } else {
        // On error the JSON body was written to the output file; read it
        // back for the message, then remove the bogus file.
        let msg = std::fs::read_to_string(local)
            .ok()
            .and_then(|b| crate::domain::dropbox_error_summary(&b))
            .unwrap_or_else(|| format!("download failed (HTTP {})", result.status));
        let _ = std::fs::remove_file(local);
        Err(msg)
    }
}

// --- cross-backend staging (Dropbox ↔ SSH) ---------------------------------

/// Copy each source by staging it through a local `/tmp` directory: fetch
/// the source down with its own transport, then push it up with the
/// destination's. Used only for mixed remote backends.
fn stage_copy_each(srcs: Vec<Location>, dst: Location) -> Vec<(Location, Result<(), String>)> {
    srcs.into_iter()
        .map(|loc| {
            let res = stage_copy_one(&loc, &dst);
            (loc, res)
        })
        .collect()
}

fn stage_copy_one(src: &Location, dst: &Location) -> Result<(), String> {
    let staging =
        make_staging_dir().map_err(|e| format!("failed to create staging dir: {}", e))?;
    let result = (|| {
        let staged = fetch_to_local(src, &staging)?;
        push_from_local(&staged, dst)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn fetch_to_local(src: &Location, staging: &Path) -> Result<PathBuf, String> {
    let name = src
        .path()
        .file_name()
        .ok_or_else(|| "source has no file name".to_string())?
        .to_owned();
    let staged = staging.join(&name);
    match src {
        Location::Local(p) => {
            copy_recursive(p, &staged).map_err(|e| e.to_string())?;
        }
        Location::Remote { backend, path } if backend.is_dropbox() => {
            dropbox_download_recursive(&dropbox_api_path(path), &staged)?;
        }
        Location::Remote { backend, path } => {
            let remote_quoted = quote_sftp_path(path);
            let staging_quoted = posix_quote(&staging.display().to_string());
            let script = build_sftp_get_script(&remote_quoted, &staging_quoted);
            run_sftp_batch(backend, &script)?;
        }
    }
    Ok(staged)
}

fn push_from_local(local: &Path, dst: &Location) -> Result<(), String> {
    match dst {
        Location::Local(dst_dir) => {
            let name = local
                .file_name()
                .ok_or_else(|| "staged file has no name".to_string())?;
            copy_recursive(local, &dst_dir.join(name)).map_err(|e| e.to_string())
        }
        Location::Remote { backend, path } if backend.is_dropbox() => {
            let name = local
                .file_name()
                .ok_or_else(|| "staged file has no name".to_string())?
                .to_string_lossy()
                .into_owned();
            dropbox_upload_recursive(local, &join_dropbox(&dropbox_api_path(path), &name))
        }
        Location::Remote { backend, path } => {
            let local_quoted = posix_quote(&local.display().to_string());
            let dst_quoted = quote_sftp_path(path);
            let script = build_sftp_put_script(&local_quoted, &dst_quoted);
            run_sftp_batch(backend, &script)
        }
    }
}

// --- transport: tokens + curl ----------------------------------------------

/// Join a Dropbox folder path (`""` for root, else `/Folder`) with a child
/// name, yielding a leading-slash absolute Dropbox path.
fn join_dropbox(dir_api: &str, name: &str) -> String {
    format!("{}/{}", dir_api.trim_end_matches('/'), name)
}

/// POST a JSON body to `https://api.dropboxapi.com/2/<endpoint>` with the
/// current access token. Non-2xx responses become `Err` with Dropbox's
/// `error_summary` where available.
fn dropbox_rpc(endpoint: &str, body: &str) -> Result<String, String> {
    let token = dropbox_access_token()?;
    let url = format!("https://api.dropboxapi.com/2/{}", endpoint);
    let auth = format!("Authorization: Bearer {}", token);
    let result = run_curl(&[
        "-X",
        "POST",
        &url,
        "-H",
        &auth,
        "-H",
        "Content-Type: application/json",
        "-d",
        body,
    ])?;
    if (200..300).contains(&result.status) {
        Ok(result.body)
    } else {
        Err(dropbox_error_message(&result.body, result.status))
    }
}

fn dropbox_error_message(body: &str, status: u16) -> String {
    crate::domain::dropbox_error_summary(body).unwrap_or_else(|| {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            format!("Dropbox API error (HTTP {})", status)
        } else {
            format!("Dropbox API error (HTTP {}): {}", status, trimmed)
        }
    })
}

/// Return a valid access token, refreshing from the configured refresh
/// token when the cached one is missing or about to expire.
fn dropbox_access_token() -> Result<String, String> {
    {
        let cache = DROPBOX_TOKEN.lock().unwrap();
        if let Some(c) = cache.as_ref() {
            if c.expires_at > std::time::Instant::now() + Duration::from_secs(60) {
                return Ok(c.token.clone());
            }
        }
    }
    let auth = Config::load()
        .dropbox_auth()
        .ok_or_else(|| "Dropbox credentials not configured in ~/.rho.yaml".to_string())?;
    let (token, expires_in) = dropbox_exchange_refresh(&auth)?;
    let mut cache = DROPBOX_TOKEN.lock().unwrap();
    *cache = Some(CachedToken {
        token: token.clone(),
        expires_at: std::time::Instant::now() + Duration::from_secs(expires_in),
    });
    Ok(token)
}

fn dropbox_exchange_refresh(auth: &DropboxAuth) -> Result<(String, u64), String> {
    // Credentials are passed as `-d` form fields; on a shared machine
    // they're briefly visible in `ps`. Acceptable for a personal file
    // manager, same trade-off as the ssh subprocess calls.
    let refresh = format!("refresh_token={}", auth.refresh_token);
    let client_id = format!("client_id={}", auth.app_key);
    let mut args: Vec<&str> = vec![
        "-X",
        "POST",
        "https://api.dropbox.com/oauth2/token",
        "-d",
        "grant_type=refresh_token",
        "-d",
        &refresh,
        "-d",
        &client_id,
    ];
    let secret;
    if let Some(s) = &auth.app_secret {
        secret = format!("client_secret={}", s);
        args.push("-d");
        args.push(&secret);
    }
    let result = run_curl(&args)?;
    if !(200..300).contains(&result.status) {
        return Err(dropbox_error_message(&result.body, result.status));
    }
    parse_dropbox_token(&result.body)
}

struct CurlResult {
    status: u16,
    body: String,
}

/// Run `curl` with the given args plus a trailing `-w` that appends the
/// HTTP status code on its own line, so we can recover both the body and
/// the status from one invocation. A status of 0 (connection failure)
/// becomes an `Err` carrying curl's stderr.
fn run_curl(args: &[&str]) -> Result<CurlResult, String> {
    let output = std::process::Command::new("curl")
        .args(["-s", "-S"])
        .args(args)
        .args(["-w", "\n%{http_code}"])
        .output()
        .map_err(|e| format!("failed to spawn curl: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let (body, status) = match stdout.rsplit_once('\n') {
        Some((b, s)) => (b.to_string(), s.trim().parse::<u16>().unwrap_or(0)),
        None => (stdout.to_string(), 0),
    };
    if status == 0 {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "curl request failed".to_string()
        } else {
            stderr
        });
    }
    Ok(CurlResult { status, body })
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

/// Extract `archive` into a freshly-made directory under the OS temp dir
/// and emit `Message::ExtractedToTemp(dest, result)` so the App can
/// navigate the active pane into the new dir on success. Used when the
/// user hits Enter on a `.zip` row.
pub fn extract_to_temp_task(archive: PathBuf) -> Task<Message> {
    Task::perform(
        async move {
            let dest = tmp_extract_dest(&archive);
            let dest_for_task = dest.clone();
            let res = tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(&dest_for_task).map_err(|e| {
                    format!("failed to create {}: {}", dest_for_task.display(), e)
                })?;
                run_extract(&archive, &dest_for_task)
            })
            .await
            .unwrap_or_else(|e| Err(format!("extract task panicked: {}", e)));
            (dest, res)
        },
        |(dest, res)| Message::ExtractedToTemp(dest, res),
    )
}

/// Where the next extraction goes. `/tmp/rho-<sanitized-stem>-<epoch-ms>`
/// on Unix; the OS temp dir on Windows. Sanitization keeps `[A-Za-z0-9_-]`
/// and replaces everything else with `_`, so weird archive names don't
/// produce unusable folder names. The epoch-ms suffix avoids clobbering a
/// previous extraction of the same archive.
pub fn tmp_extract_dest(archive: &Path) -> PathBuf {
    let stem = archive
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("archive");
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let folder_name = format!("rho-{}-{}", safe, ms);
    #[cfg(unix)]
    {
        PathBuf::from("/tmp").join(folder_name)
    }
    #[cfg(not(unix))]
    {
        std::env::temp_dir().join(folder_name)
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
        // `output()` rather than `spawn()`: osascript only *dispatches* the
        // Apple event and exits promptly (the terminal it opens runs
        // independently), so this doesn't block on the session — but it lets us
        // catch a failed dispatch. The important one is a denied Automation
        // permission (`-1743`), which otherwise leaves the user with no
        // terminal and, because the error was swallowed, no explanation.
        let out = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
            .map_err(|e| format!("failed to launch {}: {}", app, e))?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("{} did not open ({})", app, out.status)
        } else {
            // e.g. "Not authorized to send Apple events to Terminal. (-1743)"
            format!("{}: {}", app, stderr)
        })
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

    #[test]
    fn tmp_extract_dest_keeps_safe_chars() {
        let p = tmp_extract_dest(Path::new("/Users/me/my_archive.zip"));
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("rho-my_archive-"), "got: {}", name);
    }

    #[test]
    fn tmp_extract_dest_sanitizes_special_chars() {
        // Spaces / quotes / parens get replaced with `_` so the folder name
        // is safe to pass to shells / file systems without further quoting.
        let p = tmp_extract_dest(Path::new("/tmp/My Project (v2).zip"));
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("rho-My_Project__v2_-"), "got: {}", name);
        // No spaces, quotes, parens in the result.
        for bad in [' ', '\'', '"', '(', ')'] {
            assert!(!name.contains(bad), "char {:?} leaked into {}", bad, name);
        }
    }

    #[test]
    fn tmp_extract_dest_falls_back_for_no_stem() {
        // No extension and weird input — should still produce a usable path.
        let p = tmp_extract_dest(Path::new(""));
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("rho-archive-"), "got: {}", name);
    }

    #[cfg(unix)]
    #[test]
    fn tmp_extract_dest_uses_tmp_on_unix() {
        let p = tmp_extract_dest(Path::new("/foo/x.zip"));
        assert!(p.starts_with("/tmp"));
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

    #[test]
    fn posix_quote_wraps_simple_string() {
        assert_eq!(posix_quote("foo"), "'foo'");
        assert_eq!(posix_quote("/var/log"), "'/var/log'");
    }

    #[test]
    fn posix_quote_escapes_embedded_single_quote() {
        // `it's` → 'it'\''s'  (close, escaped quote, reopen)
        assert_eq!(posix_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn posix_quote_handles_special_chars_passively() {
        // Single-quoting neutralises $ \ " ` etc. — they pass through
        // untouched.
        assert_eq!(posix_quote(r#"a $b \c "d" `e`"#), r#"'a $b \c "d" `e`'"#);
    }

    #[test]
    fn quote_remote_path_quotes_absolute_path() {
        assert_eq!(
            quote_remote_path(Path::new("/var/log")),
            "'/var/log'".to_string(),
        );
    }

    #[test]
    fn quote_remote_path_quotes_path_with_spaces() {
        assert_eq!(
            quote_remote_path(Path::new("/var/My Files/x")),
            "'/var/My Files/x'".to_string(),
        );
    }

    #[test]
    fn quote_remote_path_keeps_leading_tilde_unquoted() {
        // ~/.config → ~/'.config'  so the remote shell expands ~ but
        // the rest is shell-safe even if it contains spaces.
        assert_eq!(
            quote_remote_path(Path::new("~/.config")),
            "~/'.config'".to_string(),
        );
        assert_eq!(
            quote_remote_path(Path::new("~ron/My Files")),
            "~ron/'My Files'".to_string(),
        );
    }

    #[test]
    fn quote_remote_path_bare_tilde_passes_through() {
        // `~` with no `/` after — no path to quote, leave it raw so the
        // remote shell expands to $HOME.
        assert_eq!(quote_remote_path(Path::new("~")), "~".to_string());
        assert_eq!(quote_remote_path(Path::new("~user")), "~user".to_string());
    }

    #[test]
    fn quote_sftp_path_maps_bare_tilde_to_dot() {
        // sftp has no shell + its cwd defaults to home, so `~` is the
        // home dir itself → ".".
        assert_eq!(quote_sftp_path(Path::new("~")), ".".to_string());
    }

    #[test]
    fn quote_sftp_path_strips_leading_tilde_slash() {
        // `~/file` must become the home-relative `'file'`, NOT a literal
        // `~/file` (which sftp resolves as `<home>/~/file` and fails).
        assert_eq!(
            quote_sftp_path(Path::new("~/rho-tilde-test.txt")),
            "'rho-tilde-test.txt'".to_string(),
        );
        assert_eq!(
            quote_sftp_path(Path::new("~/Documents/My File.txt")),
            "'Documents/My File.txt'".to_string(),
        );
    }

    #[test]
    fn quote_sftp_path_quotes_absolute_path_whole() {
        assert_eq!(
            quote_sftp_path(Path::new("/var/log/syslog")),
            "'/var/log/syslog'".to_string(),
        );
    }

    #[test]
    fn build_sftp_put_script_emits_recursive_put_with_newline() {
        assert_eq!(
            build_sftp_put_script("'/local/foo'", "~/'.config'"),
            "put -r '/local/foo' ~/'.config'\n",
        );
    }

    #[test]
    fn build_sftp_get_script_emits_recursive_get_with_newline() {
        assert_eq!(
            build_sftp_get_script("'/remote/log'", "'/tmp/stage'"),
            "get -r '/remote/log' '/tmp/stage'\n",
        );
    }

    #[test]
    fn run_copy_local_to_local_uses_copy_recursive() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        std::fs::write(&src, b"hi").unwrap();
        let dst = dir.path().join("dst");
        std::fs::create_dir(&dst).unwrap();

        let results = run_copy(
            vec![Location::Local(src.clone())],
            Location::Local(dst.clone()),
        );
        assert_eq!(results.len(), 1);
        let (_loc, res) = &results[0];
        assert!(res.is_ok(), "expected ok, got {:?}", res);
        assert!(dst.join("a.txt").exists());
    }

    #[test]
    fn run_move_local_to_local_renames_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        std::fs::write(&src, b"hi").unwrap();
        let dst = dir.path().join("dst");
        std::fs::create_dir(&dst).unwrap();

        let results = run_move(
            vec![Location::Local(src.clone())],
            Location::Local(dst.clone()),
        );
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_ok());
        assert!(!src.exists());
        assert!(dst.join("a.txt").exists());
    }

    #[test]
    fn run_delete_local_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("doomed");
        std::fs::write(&src, b"bye").unwrap();
        let results = run_delete(vec![Location::Local(src.clone())]);
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_ok());
        assert!(!src.exists());
    }
}
