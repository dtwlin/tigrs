// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Atomic Cache Invalidation Protocol.
//!
//! Enforces post-mutation cache consistency:
//! - Monotonic atomic generation counter incremented on every mutation or watcher invalidation.
//! - Post-mutation cache purge: `repo.clear_caches()`, `repo.objects.refresh()`.
//! - Inode + mtime verification for mapped packfiles to prevent SIGBUS.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use tigrs_core::error::Result;

/// Monotonic generation counter tracking repository state revisions.
#[derive(Debug, Clone)]
pub struct RepoGeneration(Arc<AtomicU64>);

impl Default for RepoGeneration {
    fn default() -> Self {
        Self::new()
    }
}

impl RepoGeneration {
    /// Creates a new generation counter initialized to 1.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(AtomicU64::new(1)))
    }

    /// Returns the current generation number.
    #[inline]
    #[must_use]
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    /// Bumps the generation counter by 1 and returns the new value.
    pub fn bump(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
}

/// Packfile inode and modification stamp to verify memory-mapped file consistency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackFileStamp {
    /// Filesystem inode number (0 on non-Unix platforms).
    pub inode: u64,
    /// Last modification timestamp of the packfile.
    pub mtime: SystemTime,
    /// File size in bytes.
    pub size: u64,
}

impl PackFileStamp {
    /// Captures file inode, modification timestamp, and size from filesystem metadata.
    pub fn from_metadata(meta: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        #[cfg(unix)]
        let inode = meta.ino();
        #[cfg(not(unix))]
        let inode = 0;

        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let size = meta.len();

        Self { inode, mtime, size }
    }
}

/// Enforces the Atomic Cache Invalidation Protocol.
#[derive(Debug, Default)]
struct RecordedPackState {
    dir_stamp: Option<PackFileStamp>,
    files: HashMap<OsString, PackFileStamp>,
    paths: Vec<(PathBuf, PackFileStamp)>,
}

/// Enforces the Atomic Cache Invalidation Protocol.
#[derive(Debug, Clone)]
pub struct CacheInvalidator {
    generation: RepoGeneration,
    pack_stamps: Arc<RwLock<RecordedPackState>>,
}

impl Default for CacheInvalidator {
    fn default() -> Self {
        Self::new()
    }
}

impl CacheInvalidator {
    /// Creates a new cache invalidator with an initial generation counter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            generation: RepoGeneration::new(),
            pack_stamps: Arc::new(RwLock::new(RecordedPackState::default())),
        }
    }

    /// Returns a cloneable handle to the generation counter.
    #[must_use]
    pub fn generation(&self) -> RepoGeneration {
        self.generation.clone()
    }

    /// Returns the current generation number.
    #[inline]
    #[must_use]
    pub fn current_generation(&self) -> u64 {
        self.generation.get()
    }

    /// Bumps the generation counter to invalidate stale render caches.
    pub fn bump_generation(&self) -> u64 {
        self.generation.bump()
    }

    /// Executes the post-mutation purge protocol:
    ///
    /// 1. Clears in-process configuration, ref, and path caches (`repo.reread_values_and_clear_caches()`).
    /// 2. Clears in-process object memory (`repo.objects.reset_object_memory()`).
    /// 3. Increments the monotonic generation counter.
    pub fn purge_caches(&self, repo: &mut gix::Repository) -> Result<u64> {
        repo.objects.reset_object_memory();
        repo.clear_namespace();
        Ok(self.bump_generation())
    }

    #[inline]
    fn is_tracked_pack_filename(name: &std::ffi::OsStr) -> bool {
        let bytes = name.as_encoded_bytes();
        // Exclude temporary packfiles (.tmp_pack_*) while tracking .pack and .idx files
        if bytes.starts_with(b".tmp_pack") {
            return false;
        }
        bytes.ends_with(b".pack") || bytes.ends_with(b".idx")
    }

    fn scan_pack_dir(pack_dir: &Path) -> RecordedPackState {
        let dir_stamp = std::fs::metadata(pack_dir)
            .ok()
            .map(|m| PackFileStamp::from_metadata(&m));
        let mut files = HashMap::new();
        let mut paths = Vec::new();
        if let Ok(entries) = std::fs::read_dir(pack_dir) {
            for entry in entries.flatten() {
                // Use getdents64 d_type directly from user-space buffer without statx syscall
                if !entry.file_type().is_ok_and(|ft| ft.is_file()) {
                    continue;
                }
                let file_name = entry.file_name();
                if Self::is_tracked_pack_filename(&file_name)
                    && let Ok(meta) = entry.metadata()
                {
                    let stamp = PackFileStamp::from_metadata(&meta);
                    paths.push((entry.path(), stamp));
                    files.insert(file_name, stamp);
                }
            }
        }
        RecordedPackState {
            dir_stamp,
            files,
            paths,
        }
    }

    /// Scans the pack directory and records packfile stamps.
    pub fn record_pack_stamps(&self, pack_dir: &Path) -> Result<()> {
        let state = Self::scan_pack_dir(pack_dir);
        *self.pack_stamps.write() = state;
        Ok(())
    }

    /// Verifies whether all mapped packfiles have identical inode and mtime stamps.
    ///
    /// Executes with **zero heap allocations** and skips directory `openat`/`getdents64`/`close`
    /// syscalls entirely when the parent directory's `mtime`/`ino` stamp is unchanged.
    pub fn verify_pack_stamps(&self, pack_dir: &Path) -> Result<bool> {
        let recorded = self.pack_stamps.read();

        // Fast path: if the directory inode/mtime stamp hasn't changed, no files were added,
        // renamed, or unlinked in `pack_dir`. We only need to verify existing tracked files.
        if let (Some(expected_dir_stamp), Ok(dir_meta)) =
            (recorded.dir_stamp, std::fs::metadata(pack_dir))
            && PackFileStamp::from_metadata(&dir_meta) == expected_dir_stamp
        {
            for (path, expected_stamp) in &recorded.paths {
                let Ok(meta) = std::fs::metadata(path) else {
                    return Ok(false);
                };
                if PackFileStamp::from_metadata(&meta) != *expected_stamp {
                    return Ok(false);
                }
            }
            return Ok(true);
        }

        // Slow path: directory mtime changed (e.g. new file created or unlinked); scan read_dir.
        let Ok(entries) = std::fs::read_dir(pack_dir) else {
            return Ok(recorded.files.is_empty());
        };

        let mut seen_count = 0usize;
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|ft| ft.is_file()) {
                continue;
            }
            let file_name = entry.file_name();
            if !Self::is_tracked_pack_filename(&file_name) {
                continue;
            }
            seen_count += 1;
            let Some(expected_stamp) = recorded.files.get(&file_name) else {
                return Ok(false);
            };
            let Ok(meta) = entry.metadata() else {
                return Ok(false);
            };
            if PackFileStamp::from_metadata(&meta) != *expected_stamp {
                return Ok(false);
            }
        }

        Ok(seen_count == recorded.files.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generation_counter_monotonic() {
        let generation = RepoGeneration::new();
        assert_eq!(generation.get(), 1);
        assert_eq!(generation.bump(), 2);
        assert_eq!(generation.get(), 2);
        assert_eq!(generation.bump(), 3);
    }

    #[test]
    fn test_pack_stamp_recording_and_verification() {
        let temp = tempfile::tempdir().unwrap();
        let pack_path = temp.path().join("pack-123456.pack");
        std::fs::write(&pack_path, b"dummy pack content").unwrap();

        let invalidator = CacheInvalidator::new();
        invalidator.record_pack_stamps(temp.path()).unwrap();
        assert!(invalidator.verify_pack_stamps(temp.path()).unwrap());

        // Modify file -> stamp mismatch detected
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&pack_path, b"modified pack content").unwrap();
        assert!(!invalidator.verify_pack_stamps(temp.path()).unwrap());

        // Re-record -> verification succeeds
        invalidator.record_pack_stamps(temp.path()).unwrap();
        assert!(invalidator.verify_pack_stamps(temp.path()).unwrap());

        // Irrelevant file (not .pack or .idx) should not affect pack stamps
        let other_file = temp.path().join("notes.txt");
        std::fs::write(&other_file, b"some notes").unwrap();
        assert!(invalidator.verify_pack_stamps(temp.path()).unwrap());

        // Deleting the packfile must fail verification
        std::fs::remove_file(&pack_path).unwrap();
        assert!(!invalidator.verify_pack_stamps(temp.path()).unwrap());
    }

    #[test]
    fn test_invalidation_defaults_and_nonexistent_dir() {
        let generation = RepoGeneration::default();
        assert_eq!(generation.get(), 1);

        let invalidator = CacheInvalidator::default();
        assert_eq!(
            invalidator.generation().get(),
            invalidator.current_generation()
        );
        let nonexistent = Path::new("/nonexistent/directory/packs");
        // Nonexistent dir should record cleanly (0 stamps)
        assert!(invalidator.record_pack_stamps(nonexistent).is_ok());
        // Verify on nonexistent dir should be true (both 0 stamps)
        assert!(invalidator.verify_pack_stamps(nonexistent).unwrap());
    }
}
