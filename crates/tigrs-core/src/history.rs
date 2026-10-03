// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Persistent command and search history manager for tigrs.
//!
//! Manages historical command and search prompt inputs with deduplication,
//! bounded capacity, interactive navigation (`Up`/`Down`), and persistent
//! disk storage matching Tig's standard paths (`$XDG_DATA_HOME/tig/history` or `~/.tig_history`).

use crate::error::{Result, TigError};
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// Default maximum number of history entries retained in memory and on disk.
pub const DEFAULT_HISTORY_SIZE: usize = 500;

/// Manages interactive command and search prompt history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryManager {
    entries: Vec<String>,
    max_entries: usize,
    /// Navigation cursor index:
    /// `None` when at the prompt draft line.
    /// `Some(idx)` when browsing entry at `entries[idx]`.
    cursor: Option<usize>,
    /// Saved draft input before user started pressing Up.
    draft: String,
    /// Path to file where history is persisted.
    path: Option<PathBuf>,
}

impl Default for HistoryManager {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY_SIZE)
    }
}

impl HistoryManager {
    /// Creates a new in-memory [`HistoryManager`] with the given capacity limit.
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries: max_entries.max(1),
            cursor: None,
            draft: String::new(),
            path: None,
        }
    }

    /// Creates a [`HistoryManager`] bound to a persistent file path.
    pub fn with_path(path: PathBuf, max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries: max_entries.max(1),
            cursor: None,
            draft: String::new(),
            path: Some(path),
        }
    }

    /// Resolves the default persistent history file path according to Tig conventions:
    ///
    /// 1. `$TIG_HISTORY` environment variable if set.
    /// 2. `$XDG_DATA_HOME/tig/history` if `$XDG_DATA_HOME` is set.
    /// 3. `$HOME/.local/share/tig/history` if `$HOME` is set and directory exists or can be created.
    /// 4. Fallback to `$HOME/.tig_history`.
    pub fn default_history_path() -> Option<PathBuf> {
        if let Ok(tig_hist) = env::var("TIG_HISTORY") {
            let trimmed = tig_hist.trim();
            if !trimmed.is_empty() {
                let p = PathBuf::from(trimmed);
                if p.is_absolute() {
                    return Some(p);
                }
            }
        }

        if let Ok(xdg) = env::var("XDG_DATA_HOME") {
            let trimmed = xdg.trim();
            if !trimmed.is_empty() {
                let base = PathBuf::from(trimmed);
                if base.is_absolute() {
                    return Some(base.join("tig").join("history"));
                }
            }
        }

        if let Ok(home) = env::var("HOME") {
            let trimmed = home.trim();
            if !trimmed.is_empty() {
                let home_path = PathBuf::from(trimmed);
                if home_path.is_absolute() {
                    let xdg_fallback = home_path
                        .join(".local")
                        .join("share")
                        .join("tig")
                        .join("history");
                    if xdg_fallback.parent().is_some_and(Path::is_dir) {
                        return Some(xdg_fallback);
                    }
                    return Some(home_path.join(".tig_history"));
                }
            }
        }

        None
    }

    /// Returns the active persistent file path, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Returns all history entries from oldest to newest.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Returns the number of history entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clears all history entries and resets navigation.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.cursor = None;
        self.draft.clear();
    }

    /// Adds a new command or search query to the history.
    ///
    /// - Trims leading and trailing whitespace.
    /// - Ignores empty entries.
    /// - Deduplicates consecutive identical entries.
    /// - Discards oldest entries if `max_entries` is exceeded.
    /// - Resets navigation state.
    pub fn add(&mut self, entry: &str) {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            return;
        }

        // Avoid duplicate consecutive entries
        if self.entries.last().map(String::as_str) == Some(trimmed) {
            self.cursor = None;
            self.draft.clear();
            return;
        }

        self.entries.push(trimmed.to_string());
        if self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }

        self.cursor = None;
        self.draft.clear();
    }

    /// Navigates to the previous (older) history entry.
    ///
    /// If currently at the draft line, saves `current_input` and navigates to the
    /// most recent entry. Subsequent calls move progressively backward.
    pub fn prev(&mut self, current_input: &str) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }

        match self.cursor {
            None => {
                self.draft = current_input.to_string();
                let last_idx = self.entries.len() - 1;
                self.cursor = Some(last_idx);
                Some(&self.entries[last_idx])
            }
            Some(0) => {
                // Already at oldest entry
                Some(&self.entries[0])
            }
            Some(idx) => {
                let prev_idx = idx - 1;
                self.cursor = Some(prev_idx);
                Some(&self.entries[prev_idx])
            }
        }
    }

    /// Navigates to the next (newer) history entry.
    ///
    /// If returning past the newest entry, restores the saved draft input.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }

        match self.cursor {
            None => None,
            Some(idx) if idx + 1 < self.entries.len() => {
                let next_idx = idx + 1;
                self.cursor = Some(next_idx);
                Some(&self.entries[next_idx])
            }
            Some(_) => {
                // Return to draft input
                self.cursor = None;
                Some(&self.draft)
            }
        }
    }

    /// Resets the navigation cursor back to the draft line.
    pub fn reset_navigation(&mut self) {
        self.cursor = None;
        self.draft.clear();
    }

    /// Loads history entries from the configured persistent file path.
    pub fn load(&mut self) -> Result<()> {
        if let Some(path) = self.path.clone() {
            self.load_from_path(&path)
        } else {
            Ok(())
        }
    }

    /// Maximum allowed history file size (4 MiB) to prevent OOM or special-file hangs.
    pub const MAX_HISTORY_FILE_BYTES: u64 = 4 * 1024 * 1024;

    /// Loads history entries from a specific file path.
    pub fn load_from_path(&mut self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }

        let file = File::open(path).map_err(|e| {
            TigError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to open history file '{}': {e}", path.display()),
            ))
        })?;
        let meta = file.metadata().map_err(|e| {
            TigError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to stat history file '{}': {e}", path.display()),
            ))
        })?;
        if !meta.is_file() {
            return Err(TigError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "Failed to open history file '{}': not a regular file",
                    path.display()
                ),
            )));
        }
        if meta.len() > Self::MAX_HISTORY_FILE_BYTES {
            return Err(TigError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Failed to read history file '{}': file size ({} bytes) exceeds 4 MiB limit",
                    path.display(),
                    meta.len()
                ),
            )));
        }

        let mut reader = BufReader::new(std::io::Read::take(file, Self::MAX_HISTORY_FILE_BYTES));
        let mut loaded = Vec::new();
        let mut raw_line = Vec::new();

        loop {
            raw_line.clear();
            let n = reader.read_until(b'\n', &mut raw_line).map_err(|e| {
                TigError::Io(std::io::Error::new(
                    e.kind(),
                    format!("Failed to read history line from '{}': {e}", path.display()),
                ))
            })?;
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&raw_line);
            let trimmed = line.trim();
            if !trimmed.is_empty() && loaded.last().map(String::as_str) != Some(trimmed) {
                loaded.push(trimmed.to_string());
            }
        }

        if loaded.len() > self.max_entries {
            let start = loaded.len() - self.max_entries;
            loaded.drain(0..start);
        }

        self.entries = loaded;
        self.cursor = None;
        self.draft.clear();
        Ok(())
    }

    /// Saves the current history entries to the configured persistent file path.
    pub fn save(&self) -> Result<()> {
        if let Some(path) = &self.path {
            self.save_to_path(path)
        } else {
            Ok(())
        }
    }

    /// Saves the current history entries atomically to a specific file path with `0600` permissions.
    pub fn save_to_path(&self, path: &Path) -> Result<()> {
        let target_path = crate::config::resolve_symlink_target(path);
        let existing_meta = fs::metadata(&target_path).ok();

        let parent = target_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if !parent.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                let _ = fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent);
            }
            #[cfg(not(unix))]
            {
                let _ = fs::create_dir_all(parent);
            }
        }

        let file_name = target_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("history");
        let prefix = format!(".{file_name}.tmp.");
        let mut builder = tempfile::Builder::new();
        builder.prefix(&prefix);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = existing_meta
                .map_or_else(|| fs::Permissions::from_mode(0o600), |m| m.permissions());
            builder.permissions(perms);
        }

        let mut tmp = builder.tempfile_in(parent).map_err(|e| {
            TigError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "Failed to open history file '{}' for writing: {e}",
                    target_path.display()
                ),
            ))
        })?;

        let mut buf = String::new();
        for entry in &self.entries {
            buf.push_str(entry);
            buf.push('\n');
        }
        tmp.write_all(buf.as_bytes()).map_err(|e| {
            TigError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "Failed to write history entry to '{}': {e}",
                    target_path.display()
                ),
            ))
        })?;
        tmp.as_file().sync_all().map_err(|e| {
            TigError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "Failed to sync history file '{}': {e}",
                    target_path.display()
                ),
            ))
        })?;
        tmp.persist(&target_path).map_err(|e| {
            TigError::Io(std::io::Error::new(
                e.error.kind(),
                format!(
                    "Failed to atomically replace history file '{}': {}",
                    target_path.display(),
                    e.error
                ),
            ))
        })?;
        if let Ok(dir_file) = File::open(parent) {
            let _ = dir_file.sync_all();
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_dedup_and_bounds() {
        let mut hist = HistoryManager::new(3);
        hist.add("cmd1");
        hist.add("cmd1"); // consecutive duplicate ignored
        assert_eq!(hist.len(), 1);

        hist.add("cmd2");
        hist.add("cmd3");
        assert_eq!(hist.len(), 3);
        assert_eq!(hist.entries(), &["cmd1", "cmd2", "cmd3"]);

        hist.add("cmd4"); // exceeds capacity 3, drops cmd1
        assert_eq!(hist.len(), 3);
        assert_eq!(hist.entries(), &["cmd2", "cmd3", "cmd4"]);
    }

    #[test]
    fn test_history_navigation_and_draft() {
        let mut hist = HistoryManager::new(10);
        hist.add("alpha");
        hist.add("beta");
        hist.add("gamma");

        // Start typing a draft and press Up
        assert_eq!(hist.prev("my-draft"), Some("gamma"));
        assert_eq!(hist.prev("gamma"), Some("beta"));
        assert_eq!(hist.prev("beta"), Some("alpha"));
        // Oldest entry reached, stays at alpha
        assert_eq!(hist.prev("alpha"), Some("alpha"));

        // Now press Down to navigate back forward
        assert_eq!(hist.next(), Some("beta"));
        assert_eq!(hist.next(), Some("gamma"));
        // Returning past gamma restores original draft
        assert_eq!(hist.next(), Some("my-draft"));
        // Past draft returns None
        assert_eq!(hist.next(), None);
    }

    #[test]
    fn test_history_file_roundtrip() {
        let temp_dir = std::env::temp_dir().join(format!("tigrs_hist_test_{}", std::process::id()));
        let hist_file = temp_dir.join("history");

        let mut hist = HistoryManager::with_path(hist_file.clone(), 5);
        hist.add("echo hello");
        hist.add("git status");
        hist.add("git log");
        hist.save().unwrap();

        let mut reloaded = HistoryManager::with_path(hist_file.clone(), 5);
        reloaded.load().unwrap();
        assert_eq!(reloaded.entries(), &["echo hello", "git status", "git log"]);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_empty_history_and_whitespace_handling() {
        let mut hist = HistoryManager::new(5);
        assert!(hist.is_empty());
        assert_eq!(hist.len(), 0);

        // Blank and empty entries must be ignored
        hist.add("");
        hist.add("   \t  ");
        assert!(hist.is_empty());

        // Navigation on empty history
        assert_eq!(hist.prev("draft"), None);
        assert_eq!(hist.next(), None);
    }

    #[test]
    fn test_history_reset_cursor_and_no_path() {
        let mut hist = HistoryManager::new(5);
        hist.add("entry1");
        hist.add("entry2");

        assert_eq!(hist.prev("draft"), Some("entry2"));
        hist.reset_navigation();
        assert_eq!(hist.next(), None);

        // Saving without path is a clean no-op
        assert!(hist.save().is_ok());
        // Loading without path is a clean no-op
        assert!(hist.load().is_ok());
    }

    #[test]
    fn test_history_capacity_one() {
        let mut hist = HistoryManager::new(1);
        hist.add("first");
        hist.add("second");
        assert_eq!(hist.len(), 1);
        assert_eq!(hist.entries(), &["second"]);
    }

    #[test]
    fn test_history_default_path_and_clear() {
        let _ = HistoryManager::default_history_path();

        let new_path = PathBuf::from("/tmp/custom_history_test");
        let mut hist = HistoryManager::with_path(new_path.clone(), 10);
        hist.add("cmd1");
        hist.add("cmd2");
        assert_eq!(hist.len(), 2);
        assert!(!hist.is_empty());
        assert_eq!(hist.path(), Some(new_path.as_path()));

        hist.clear();
        assert_eq!(hist.len(), 0);
        assert!(hist.is_empty());
        assert_eq!(hist.next(), None);
    }

    #[test]
    fn test_history_load_exceeds_max_entries() {
        let temp_dir =
            std::env::temp_dir().join(format!("tigrs_hist_trunc_{}", std::process::id()));
        let hist_file = temp_dir.join("history");
        fs::create_dir_all(&temp_dir).unwrap();

        // Write 10 entries to the file directly
        let entries = (0..10)
            .map(|i| format!("command_{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&hist_file, entries).unwrap();

        // Load with max_entries = 3
        let mut hist = HistoryManager::with_path(hist_file, 3);
        hist.load().unwrap();
        assert_eq!(hist.len(), 3);
        assert_eq!(hist.entries(), &["command_7", "command_8", "command_9"]);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_history_io_errors() {
        let temp_dir = std::env::temp_dir().join(format!("tigrs_hist_err_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).unwrap();

        // Loading from a directory instead of a file produces an error
        let mut hist_dir = HistoryManager::with_path(temp_dir.clone(), 5);
        assert!(hist_dir.load().is_err());

        // Saving to an invalid path (where a parent component is a file, not a directory)
        let blocker_file = temp_dir.join("blocker");
        fs::write(&blocker_file, "not a dir").unwrap();
        let invalid_save_path = blocker_file.join("sub").join("history");
        let mut hist_invalid = HistoryManager::with_path(invalid_save_path, 5);
        hist_invalid.add("test entry");
        assert!(hist_invalid.save().is_err());

        // Oversized history file (> 4 MiB) is rejected before reading into memory
        let oversize_path = temp_dir.join("oversize_history");
        let f = File::create(&oversize_path).unwrap();
        f.set_len(HistoryManager::MAX_HISTORY_FILE_BYTES + 1)
            .unwrap();
        drop(f);
        let mut hist_oversize = HistoryManager::default();
        let err = hist_oversize.load_from_path(&oversize_path).unwrap_err();
        assert!(err.to_string().contains("exceeds 4 MiB limit"));

        // Non-existent file is a clean no-op
        assert!(
            hist_oversize
                .load_from_path(&temp_dir.join("missing_history"))
                .is_ok()
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
