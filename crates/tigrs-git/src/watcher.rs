// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Background repository file watcher with mtime heartbeat fallback.
//!
//! Enforces:
//! - Watcher paths:
//!   - `git_dir/HEAD`, `git_dir/index`, `git_dir/MERGE_HEAD`, `git_dir/rebase-merge/`
//!   - `common_dir/packed-refs`, `common_dir/refs/`, `common_dir/reftable/`, `common_dir/objects/pack/`
//! - 1.5 s mtime heartbeat fallback for filesystems where inotify is unreliable.
//! - Non-blocking event streaming through crossbeam channel.

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};
use tigrs_core::error::{Result, TigError};

/// Default interval for the mtime heartbeat fallback (1.5 seconds).
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(1500);

/// Granular category of repository file modifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepoChangeEvent {
    /// HEAD pointer or current branch changed.
    Head,
    /// Staging index file modified.
    Index,
    /// References modified (`packed-refs`, `refs/`, or `reftable/`).
    Refs,
    /// Working tree status changed (`MERGE_HEAD`, `rebase-merge/`).
    Status,
    /// Object database modified (`objects/pack/`, repack, fetch).
    Objects,
    /// General repository file change.
    Generic,
}

impl RepoChangeEvent {
    /// Returns `true` if a `.git` path is a transient lockfile, reflog, or git internal scratch file
    /// that should not trigger UI refresh / status rescans.
    #[must_use]
    pub fn should_ignore_git_path(path: &Path) -> bool {
        let name = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
        if matches!(
            name,
            "index.lock"
                | "HEAD.lock"
                | "config.lock"
                | "packed-refs.lock"
                | "COMMIT_EDITMSG"
                | "FETCH_HEAD"
                | "ORIG_HEAD"
                | "AUTO_MERGE"
                | "gc.pid"
                | "gc.log"
        ) || Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("lock"))
        {
            return true;
        }
        path.components().any(|c| c.as_os_str() == "logs")
    }

    /// Classifies a path change relative to git directory and common directory.
    #[must_use]
    pub fn from_path(path: &Path) -> Self {
        let name = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
        let s = path.to_string_lossy();
        if name == "HEAD" || name == "HEAD.lock" {
            Self::Head
        } else if name == "index" || name == "index.lock" {
            Self::Index
        } else if s.contains("MERGE_HEAD")
            || s.contains("rebase-merge")
            || s.contains("rebase-apply")
            || s.contains("CHERRY_PICK_HEAD")
            || s.contains("REVERT_HEAD")
        {
            Self::Status
        } else if s.contains("packed-refs") || s.contains("refs") || s.contains("reftable") {
            Self::Refs
        } else if s.contains("objects") || s.contains("pack-") {
            Self::Objects
        } else {
            Self::Generic
        }
    }
}

/// Maximum number of subdirectories under `refs/` to register with inotify.
const MAX_WATCHED_REF_DIRS: usize = 512;

/// Walks `refs_dir` without following symlinks and registers `NonRecursive` watches on up to
/// `MAX_WATCHED_REF_DIRS` real directories.
fn watch_refs_bounded(watcher: &mut RecommendedWatcher, refs_dir: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(refs_dir) else {
        return;
    };
    if !meta.file_type().is_dir() {
        return;
    }
    let mut stack = vec![refs_dir.to_path_buf()];
    let mut count = 0usize;
    while let Some(dir) = stack.pop() {
        if count >= MAX_WATCHED_REF_DIRS {
            break;
        }
        let _ = watcher.watch(&dir, RecursiveMode::NonRecursive);
        count += 1;
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type()
                    && ft.is_dir()
                {
                    stack.push(entry.path());
                }
            }
        }
    }
}

/// Active filesystem watcher monitoring repository paths for modifications.
pub struct RepoWatcher {
    event_rx: Receiver<RepoChangeEvent>,
    shutdown_tx: Option<Sender<()>>,
    heartbeat_handle: Option<JoinHandle<()>>,
}

impl RepoWatcher {
    /// Starts watching the specified repository directories with inotify and mtime fallback.
    pub fn start(git_dir: &Path, common_dir: &Path) -> Result<Self> {
        let (tx, rx) = unbounded::<RepoChangeEvent>();
        let (shutdown_tx, shutdown_rx) = bounded::<()>(0);

        // 1. Configure inotify / OS filesystem watcher (degrading gracefully to the
        // 1.5s heartbeat fallback if `inotify_init1` hits `EMFILE`).
        let tx_notify = tx.clone();
        let mut watcher = RecommendedWatcher::new(
            move |res: std::result::Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    match event.kind {
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                            let mut batch_seen_mask: u8 = 0;
                            for path in &event.paths {
                                if RepoChangeEvent::should_ignore_git_path(path) {
                                    continue;
                                }
                                let change = RepoChangeEvent::from_path(path);
                                let bit = 1u8 << (change as u8);
                                if (batch_seen_mask & bit) == 0 {
                                    batch_seen_mask |= bit;
                                    let _ = tx_notify.send(change);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            },
            Config::default(),
        )
        .ok();

        // Watch primary top-level git_dir immediately (single inotify_add_watch syscall)
        // and defer recursive `.git/refs` directory registration to the background thread.
        if let Some(ref mut w) = watcher {
            let _ = w.watch(git_dir, RecursiveMode::NonRecursive);
        }

        // 2. Spawn background registration + 1.5-second mtime heartbeat fallback thread
        let tx_heartbeat = tx;
        let gd = git_dir.to_path_buf();
        let cd = common_dir.to_path_buf();

        let heartbeat_handle = thread::Builder::new()
            .name("tigrs-watch-hb".to_string())
            .spawn(move || {
                let mut watcher = watcher;
                if let Some(ref mut w) = watcher {
                    if cd != gd && cd.exists() {
                        let _ = w.watch(&cd, RecursiveMode::NonRecursive);
                        watch_refs_bounded(w, &gd.join("refs"));
                        let wt_reftable = gd.join("reftable");
                        if wt_reftable.is_dir() {
                            let _ = w.watch(&wt_reftable, RecursiveMode::NonRecursive);
                        }
                    }

                    watch_refs_bounded(w, &cd.join("refs"));
                    let reftable_dir = cd.join("reftable");
                    if reftable_dir.is_dir() {
                        let _ = w.watch(&reftable_dir, RecursiveMode::NonRecursive);
                    }
                    let pack_dir = cd.join("objects").join("pack");
                    if pack_dir.is_dir() {
                        let _ = w.watch(&pack_dir, RecursiveMode::NonRecursive);
                    }
                }

                let mut stamps: HashMap<PathBuf, SystemTime> = HashMap::new();

                // Initial poll
                poll_heartbeat_targets(&gd, &cd, &mut stamps);

                // Wakes immediately when `shutdown_tx` is dropped in `Drop::drop`
                // and polls strictly every `HEARTBEAT_INTERVAL` (1.5s).
                while matches!(
                    shutdown_rx.recv_timeout(HEARTBEAT_INTERVAL),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout)
                ) {
                    let changes = poll_heartbeat_targets(&gd, &cd, &mut stamps);
                    for change in changes {
                        let _ = tx_heartbeat.try_send(change);
                    }
                }
                drop(watcher);
            })
            .map_err(TigError::Io)?;

        Ok(Self {
            event_rx: rx,
            shutdown_tx: Some(shutdown_tx),
            heartbeat_handle: Some(heartbeat_handle),
        })
    }

    /// Returns a reference to the event receiver channel.
    #[inline]
    #[must_use]
    pub fn receiver(&self) -> &Receiver<RepoChangeEvent> {
        &self.event_rx
    }

    /// Drains all currently queued change events, deduplicating them into a vector.
    pub fn drain_events(&self) -> Vec<RepoChangeEvent> {
        let mut events = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            if !events.contains(&ev) {
                events.push(ev);
            }
        }
        events
    }
}

impl Drop for RepoWatcher {
    fn drop(&mut self) {
        // Dropping shutdown_tx immediately wakes recv_timeout in the heartbeat thread.
        self.shutdown_tx.take();
        if let Some(handle) = self.heartbeat_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Polls target file paths and returns events for any files whose modification time changed.
fn poll_heartbeat_targets(
    git_dir: &Path,
    common_dir: &Path,
    stamps: &mut HashMap<PathBuf, SystemTime>,
) -> Vec<RepoChangeEvent> {
    let targets = [
        git_dir.join("HEAD"),
        git_dir.join("index"),
        git_dir.join("MERGE_HEAD"),
        common_dir.join("packed-refs"),
        common_dir.join("reftable"),
        common_dir.join("objects").join("pack"),
    ];

    let mut changes = Vec::new();
    for target in &targets {
        if let Ok(meta) = std::fs::metadata(target)
            && let Ok(mtime) = meta.modified()
        {
            if let Some(prev) = stamps.get(target) {
                if *prev != mtime {
                    stamps.insert(target.clone(), mtime);
                    changes.push(RepoChangeEvent::from_path(target));
                }
            } else {
                stamps.insert(target.clone(), mtime);
            }
        }
    }

    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_change_event_classification() {
        assert_eq!(
            RepoChangeEvent::from_path(Path::new(".git/HEAD")),
            RepoChangeEvent::Head
        );
        assert_eq!(
            RepoChangeEvent::from_path(Path::new(".git/index")),
            RepoChangeEvent::Index
        );
        assert_eq!(
            RepoChangeEvent::from_path(Path::new(".git/packed-refs")),
            RepoChangeEvent::Refs
        );
        assert_eq!(
            RepoChangeEvent::from_path(Path::new(".git/refs/heads/main")),
            RepoChangeEvent::Refs
        );
        assert_eq!(
            RepoChangeEvent::from_path(Path::new(".git/reftable/tables.list")),
            RepoChangeEvent::Refs
        );
        assert_eq!(
            RepoChangeEvent::from_path(Path::new(".git/MERGE_HEAD")),
            RepoChangeEvent::Status
        );
        assert_eq!(
            RepoChangeEvent::from_path(Path::new(".git/objects/pack/pack-123.pack")),
            RepoChangeEvent::Objects
        );
        assert_eq!(
            RepoChangeEvent::from_path(Path::new(".git/rebase-merge/head-name")),
            RepoChangeEvent::Status
        );
        assert_eq!(
            RepoChangeEvent::from_path(Path::new(".git/rebase-apply/patch")),
            RepoChangeEvent::Status
        );
        assert_eq!(
            RepoChangeEvent::from_path(Path::new(".git/CHERRY_PICK_HEAD")),
            RepoChangeEvent::Status
        );
        assert_eq!(
            RepoChangeEvent::from_path(Path::new(".git/REVERT_HEAD")),
            RepoChangeEvent::Status
        );
        assert_eq!(
            RepoChangeEvent::from_path(Path::new(".git/COMMIT_EDITMSG")),
            RepoChangeEvent::Generic
        );
        assert_eq!(
            RepoChangeEvent::from_path(Path::new(".git/config")),
            RepoChangeEvent::Generic
        );
    }

    #[test]
    fn test_watcher_lifecycle_and_drain() {
        let temp = tempfile::tempdir().unwrap();
        let git_dir = temp.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let watcher = RepoWatcher::start(&git_dir, &git_dir).expect("Watcher start");
        assert!(watcher.drain_events().is_empty());

        // Modify HEAD file
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/feature\n").unwrap();

        // Wait up to 2s (exceeding the 1.5s HEARTBEAT_INTERVAL fallback) for event delivery
        let mut got_head = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            for ev in watcher.drain_events() {
                if ev == RepoChangeEvent::Head || ev == RepoChangeEvent::Generic {
                    got_head = true;
                    break;
                }
            }
            if got_head {
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }

        assert!(got_head, "Watcher must observe HEAD file modification");
    }

    #[test]
    fn test_poll_heartbeat_targets_and_split_common_dir() {
        let temp = tempfile::tempdir().unwrap();
        let git_dir = temp.path().join(".git");
        let common_dir = temp.path().join("common_git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::create_dir_all(&common_dir).unwrap();

        let head_file = git_dir.join("HEAD");
        std::fs::write(&head_file, "ref: refs/heads/main\n").unwrap();

        let mut stamps = std::collections::HashMap::new();
        // Initial scan records mtime
        let changes1 = poll_heartbeat_targets(&git_dir, &common_dir, &mut stamps);
        assert!(changes1.is_empty());
        assert!(stamps.contains_key(&head_file));

        // Sleep briefly and update HEAD mtime
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(&head_file, "ref: refs/heads/dev\n").unwrap();

        let changes2 = poll_heartbeat_targets(&git_dir, &common_dir, &mut stamps);
        assert_eq!(changes2.len(), 1);
        assert_eq!(changes2[0], RepoChangeEvent::Head);

        // Start watcher with different common_dir and git_dir
        let watcher =
            RepoWatcher::start(&git_dir, &common_dir).expect("Watcher start separate dirs");
        let _ = watcher.drain_events();
    }
}
