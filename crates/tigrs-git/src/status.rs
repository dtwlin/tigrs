// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Working tree and index status inspection.
//!
//! Provides two-tier status scanning:
//! - Tier 1: In-process `gix` status (`repo.status()`) on standard repositories.
//! - Tier 2: `git status --porcelain=v2 -z` on reftable repositories or on fallback.
//!
//! Includes parsing of porcelain v2 status records and on-demand diff computation
//! for individual status entries.

use crate::diff::{
    CommitDiff, DiffHunk, DiffLineKind, DiffSummaryStats, FileChangeStatus, FileDiff, HunkLine,
};
use crate::types::RepoInfo;
use gix::ObjectId;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use tigrs_core::ansi::sanitize_string;
use tigrs_core::cancel::CancellationToken;
use tigrs_core::error::{Result, TigError};

/// Git working-tree status section classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusSection {
    /// Changes staged in the index ready to be committed.
    Staged,
    /// Tracked files modified in the working tree but not staged.
    Unstaged,
    /// Files present in the working tree not tracked by Git.
    Untracked,
    /// Merge conflict entries.
    Unmerged,
}

impl StatusSection {
    /// User-visible title for the section in Tig status view.
    pub fn title(self) -> &'static str {
        match self {
            Self::Staged => "Changes to be committed",
            Self::Unstaged => "Changes not staged for commit",
            Self::Untracked => "Untracked files",
            Self::Unmerged => "Unmerged paths",
        }
    }
}

/// A single file entry in the status report.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StatusItem {
    /// Repository-relative path to the file (sanitized for terminal display).
    pub path: String,
    /// Raw repository-relative path bytes for git and filesystem operations.
    pub raw_path: Vec<u8>,
    /// Single-character status code ('M', 'A', 'D', 'R', 'C', 'U', '?').
    pub status_code: char,
    /// The section this item belongs to.
    pub section: StatusSection,
    /// Original path if this item represents a rename or copy (sanitized for terminal display).
    pub old_path: Option<String>,
    /// Raw original path bytes if this item represents a rename or copy.
    pub raw_old_path: Option<Vec<u8>>,
}

/// Four status buckets: (staged, unstaged, untracked, unmerged).
pub type StatusBuckets = (
    Vec<StatusItem>,
    Vec<StatusItem>,
    Vec<StatusItem>,
    Vec<StatusItem>,
);

impl StatusItem {
    /// Creates a new `StatusItem`, automatically sanitizing paths of ANSI control characters.
    pub fn new(
        status_code: char,
        section: StatusSection,
        path: impl Into<String>,
        old_path: Option<String>,
    ) -> Self {
        let raw_str = path.into();
        let raw_path = raw_str.as_bytes().to_vec();
        let raw_old_path = old_path.as_ref().map(|s| s.as_bytes().to_vec());
        Self {
            path: sanitize_string(raw_str),
            raw_path,
            status_code,
            section,
            old_path: old_path.map(sanitize_string),
            raw_old_path,
        }
    }

    /// Creates a new `StatusItem` from raw byte slices, preserving exact bytes in `raw_path`
    /// while sanitizing `path` for terminal display.
    #[must_use]
    pub fn from_raw_bytes(
        status_code: char,
        section: StatusSection,
        raw_path: Vec<u8>,
        raw_old_path: Option<Vec<u8>>,
    ) -> Self {
        let path = sanitize_string(String::from_utf8_lossy(&raw_path).into_owned());
        let old_path = raw_old_path
            .as_ref()
            .map(|b| sanitize_string(String::from_utf8_lossy(b).into_owned()));
        Self {
            path,
            raw_path,
            status_code,
            section,
            old_path,
            raw_old_path,
        }
    }

    /// Returns the raw repository-relative path as an [`std::ffi::OsStr`].
    #[must_use]
    pub fn os_path(&self) -> &std::ffi::OsStr {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            if !self.raw_path.is_empty() {
                return std::ffi::OsStr::from_bytes(&self.raw_path);
            }
        }
        std::ffi::OsStr::new(&self.path)
    }

    /// Returns the raw original path (for renames/copies) as an `Option<&OsStr>`.
    #[must_use]
    pub fn os_old_path(&self) -> Option<&std::ffi::OsStr> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            if let Some(ref b) = self.raw_old_path {
                return Some(std::ffi::OsStr::from_bytes(b));
            }
        }
        self.old_path.as_deref().map(std::ffi::OsStr::new)
    }

    /// Returns the raw repository-relative path bytes.
    #[must_use]
    pub fn raw_path_bytes(&self) -> &[u8] {
        if self.raw_path.is_empty() {
            self.path.as_bytes()
        } else {
            &self.raw_path
        }
    }

    /// Returns a descriptive status label like "modified", "new file", "deleted".
    pub fn label(&self) -> &'static str {
        match self.status_code {
            'A' => "new file",
            'D' => "deleted",
            'R' => "renamed",
            'C' => "copied",
            'T' => "typechange",
            'U' => "unmerged",
            '?' => "untracked",
            _ => "modified",
        }
    }
}

/// Aggregated working tree status report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusReport {
    /// Staged changes.
    pub staged: Vec<StatusItem>,
    /// Unstaged changes.
    pub unstaged: Vec<StatusItem>,
    /// Untracked files.
    pub untracked: Vec<StatusItem>,
    /// Unmerged / conflicted files.
    pub unmerged: Vec<StatusItem>,
    /// Current checked out branch name or symbolic ref.
    pub branch: String,
    /// Commit ID pointed to by HEAD, if resolved.
    pub head_commit: Option<ObjectId>,
}

impl StatusReport {
    /// Returns the total number of items across all status sections.
    pub fn total_count(&self) -> usize {
        self.staged.len() + self.unstaged.len() + self.untracked.len() + self.unmerged.len()
    }

    /// Returns `true` if the working tree and index are completely clean.
    pub fn is_empty(&self) -> bool {
        self.total_count() == 0
    }
}

/// Scans repository status using Tier-1 in-process gix or Tier-2 CLI fallback.
pub fn scan_status(
    repo: &gix::Repository,
    info: &RepoInfo,
    branch: String,
    head_commit: Option<ObjectId>,
    cancel: &CancellationToken,
) -> Result<StatusReport> {
    cancel.check_cancelled()?;

    let work_dir = info
        .work_dir
        .as_deref()
        .unwrap_or_else(|| info.git_dir.parent().unwrap_or(&info.git_dir));

    let (mut staged, mut unstaged, mut untracked, mut unmerged) = if info.is_reftable {
        scan_status_cli(work_dir, cancel)?
    } else if let Ok(res) = scan_status_gix(repo, cancel) {
        res
    } else {
        cancel.check_cancelled()?;
        scan_status_cli(work_dir, cancel)?
    };

    staged.sort_by(|a, b| a.raw_path.cmp(&b.raw_path));
    unstaged.sort_by(|a, b| a.raw_path.cmp(&b.raw_path));
    untracked.sort_by(|a, b| a.raw_path.cmp(&b.raw_path));
    unmerged.sort_by(|a, b| a.raw_path.cmp(&b.raw_path));

    Ok(StatusReport {
        staged,
        unstaged,
        untracked,
        unmerged,
        branch,
        head_commit,
    })
}

/// Processes a single `gix::status::index_worktree::Item` into the appropriate status bucket.
fn record_index_worktree_item(
    item: &gix::status::index_worktree::Item,
    unstaged: &mut Vec<StatusItem>,
    untracked: &mut Vec<StatusItem>,
    unmerged: &mut Vec<StatusItem>,
) {
    use gix::status::index_worktree::iter::Summary;
    let Some(summary) = item.summary() else {
        return;
    };
    let raw_slice: &[u8] = item.rela_path().as_ref();
    let raw_bytes: Vec<u8> = raw_slice.to_vec();
    match summary {
        Summary::Modified => {
            unstaged.push(StatusItem::from_raw_bytes(
                'M',
                StatusSection::Unstaged,
                raw_bytes,
                None,
            ));
        }
        Summary::Removed => {
            unstaged.push(StatusItem::from_raw_bytes(
                'D',
                StatusSection::Unstaged,
                raw_bytes,
                None,
            ));
        }
        Summary::TypeChange => {
            unstaged.push(StatusItem::from_raw_bytes(
                'T',
                StatusSection::Unstaged,
                raw_bytes,
                None,
            ));
        }
        Summary::Renamed => {
            unstaged.push(StatusItem::from_raw_bytes(
                'R',
                StatusSection::Unstaged,
                raw_bytes,
                None,
            ));
        }
        Summary::Copied => {
            unstaged.push(StatusItem::from_raw_bytes(
                'C',
                StatusSection::Unstaged,
                raw_bytes,
                None,
            ));
        }
        Summary::Conflict => {
            unmerged.push(StatusItem::from_raw_bytes(
                'U',
                StatusSection::Unmerged,
                raw_bytes,
                None,
            ));
        }
        Summary::IntentToAdd => {
            unstaged.push(StatusItem::from_raw_bytes(
                'A',
                StatusSection::Unstaged,
                raw_bytes,
                None,
            ));
        }
        Summary::Added => untracked.push(StatusItem::from_raw_bytes(
            '?',
            StatusSection::Untracked,
            raw_bytes,
            None,
        )),
    }
}

/// Returns `true` when `.git/index` carries a valid root `TREE` (`cache-tree`) extension whose
/// cryptographic hash equals `HEAD^{tree}` and covers all index entries with no unmerged or
/// intent-to-add flags (meaning `HEAD^{tree}` and `.git/index` are identical and `staged` is empty).
fn index_matches_head_tree(index: &gix::index::State, head_tree_id: Option<ObjectId>) -> bool {
    match (head_tree_id, index.tree()) {
        (Some(head_tree), Some(cache_tree)) => {
            cache_tree.name.is_empty()
                && cache_tree.id == head_tree
                && cache_tree
                    .num_entries
                    .is_some_and(|n| n as usize == index.entries().len())
                && index.entries().iter().all(|e| {
                    e.stage() == gix::index::entry::Stage::Unconflicted
                        && !e.flags.intersects(
                            gix::index::entry::Flags::INTENT_TO_ADD
                                | gix::index::entry::Flags::REMOVE,
                        )
                })
        }
        _ => false,
    }
}

/// Scans repository status in-process via `gix::Repository::status`.
pub(crate) fn scan_status_gix(
    repo: &gix::Repository,
    cancel: &CancellationToken,
) -> Result<StatusBuckets> {
    let index = repo
        .index_or_empty()
        .map_err(|err| TigError::Git(format!("Failed to load git index: {err}")))?;
    let head_tree_id: Option<ObjectId> = repo.head_tree_id_or_empty().ok().map(Into::into);

    // Fast path: when `.git/index` carries a valid root `TREE` (`cache-tree`) extension whose
    // cryptographic hash equals `HEAD^{tree}` and covers all index entries with no unmerged or
    // intent-to-add flags, `HEAD^{tree}` and `.git/index` are identical (`staged` is empty).
    // Skipping `TreeIndex` avoids `gix::Repository::index_from_tree`, which otherwise inflates
    // every tree object in `HEAD^{tree}` (~220 MB RSS and ~180 ms on a Linux kernel worktree).
    let is_clean_index = index_matches_head_tree(&index, head_tree_id);

    let platform = repo
        .status(gix::progress::Discard)
        .map_err(|err| TigError::Git(format!("Failed to initialize gix status: {err}")))?
        .index(gix::worktree::IndexPersistedOrInMemory::Persisted(index))
        .untracked_files(gix::status::UntrackedFiles::Files)
        .should_interrupt_owned(cancel.clone_flag());

    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();
    let mut unmerged = Vec::new();

    if is_clean_index {
        let iter = platform
            .into_index_worktree_iter(Vec::<gix::bstr::BString>::new())
            .map_err(|err| {
                TigError::Git(format!(
                    "Failed to run gix status into_index_worktree_iter: {err}"
                ))
            })?;
        for item in iter {
            cancel.check_cancelled()?;
            let item =
                item.map_err(|err| TigError::Git(format!("Error reading status item: {err}")))?;
            record_index_worktree_item(&item, &mut unstaged, &mut untracked, &mut unmerged);
        }
        return Ok((staged, unstaged, untracked, unmerged));
    }

    let iter = platform
        .into_iter(Vec::<gix::bstr::BString>::new())
        .map_err(|err| TigError::Git(format!("Failed to run gix status into_iter: {err}")))?;

    for item in iter {
        cancel.check_cancelled()?;
        let item =
            item.map_err(|err| TigError::Git(format!("Error reading status item: {err}")))?;
        match item {
            gix::status::Item::TreeIndex(change) => {
                let (status_code, old_path) = match &change {
                    gix::diff::index::Change::Addition { .. } => ('A', None),
                    gix::diff::index::Change::Deletion { .. } => ('D', None),
                    gix::diff::index::Change::Modification { .. } => ('M', None),
                    gix::diff::index::Change::Rewrite {
                        source_location,
                        copy,
                        ..
                    } => {
                        let code = if *copy { 'C' } else { 'R' };
                        let src_bytes: &[u8] = source_location.as_ref();
                        (code, Some(src_bytes.to_vec()))
                    }
                };
                let loc_bytes: &[u8] = change.location().as_ref();
                staged.push(StatusItem::from_raw_bytes(
                    status_code,
                    StatusSection::Staged,
                    loc_bytes.to_vec(),
                    old_path,
                ));
            }
            gix::status::Item::IndexWorktree(item) => {
                record_index_worktree_item(&item, &mut unstaged, &mut untracked, &mut unmerged);
            }
        }
    }

    Ok((staged, unstaged, untracked, unmerged))
}

/// Best-effort attempt to expand a pipe buffer to 1 MiB (`F_SETPIPE_SZ`) on Linux.
/// Reduces context-switching when streaming large diffs/patches or blame porcelain.
#[cfg(target_os = "linux")]
pub(crate) fn try_set_pipe_size_1mb<Fd: rustix::fd::AsFd>(fd: Fd) {
    let _ = rustix::pipe::fcntl_setpipe_size(fd, 1_048_576);
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn try_set_pipe_size_1mb<Fd>(_fd: Fd) {}

/// Executes `cmd` cooperatively polling `cancel`. Kills child process group on cancellation.
pub(crate) fn run_command_cancellable(
    cmd: &mut Command,
    cancel: &CancellationToken,
) -> Result<std::process::Output> {
    cancel.check_cancelled()?;
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .map_err(|err| TigError::Git(format!("Failed to spawn git subprocess: {err}")))?;

    let kill_child_pgrp = |child: &mut std::process::Child| {
        #[cfg(unix)]
        {
            let pid = child.id() as i32;
            if let Some(p) = rustix::process::Pid::from_raw(pid) {
                let _ = rustix::process::kill_process_group(p, rustix::process::Signal::KILL);
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    };

    let stdout_pipe = child.stdout.take();
    if let Some(ref pipe) = stdout_pipe {
        try_set_pipe_size_1mb(pipe);
    }
    let stderr_pipe = child.stderr.take();
    if let Some(ref pipe) = stderr_pipe {
        try_set_pipe_size_1mb(pipe);
    }

    std::thread::scope(|s| {
        let stderr_handle = s.spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut err) = stderr_pipe {
                let mut bounded = std::io::Read::take(&mut err, 65_536);
                let _ = std::io::Read::read_to_end(&mut bounded, &mut buf);
                let _ = std::io::copy(&mut err, &mut std::io::sink());
            }
            buf
        });

        // Fast path: when the token cannot be cancelled by any external source,
        // read stdout directly on the caller thread and reap via `child.wait()`
        // (woken immediately by the kernel on SIGCHLD with zero polling delay).
        if cancel.is_never_cancelled() {
            let mut stdout = Vec::new();
            if let Some(mut out) = stdout_pipe {
                let mut bounded =
                    std::io::Read::take(&mut out, crate::diff::MAX_DIFF_BLOB_BYTES as u64);
                let _ = std::io::Read::read_to_end(&mut bounded, &mut stdout);
                let _ = std::io::copy(&mut out, &mut std::io::sink());
            }
            let stderr = stderr_handle.join().unwrap_or_default();
            let status = child
                .wait()
                .map_err(|err| TigError::Git(format!("Failed to wait on git subprocess: {err}")))?;
            return Ok(std::process::Output {
                status,
                stdout,
                stderr,
            });
        }

        let stdout_handle = s.spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut out) = stdout_pipe {
                let mut bounded =
                    std::io::Read::take(&mut out, crate::diff::MAX_DIFF_BLOB_BYTES as u64);
                let _ = std::io::Read::read_to_end(&mut bounded, &mut buf);
                let _ = std::io::copy(&mut out, &mut std::io::sink());
            }
            buf
        });

        let mut sleep_us: u64 = 100;
        let wait_res = loop {
            if cancel.is_cancelled() {
                kill_child_pgrp(&mut child);
                break Err(TigError::Cancelled);
            }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => {
                    std::thread::sleep(std::time::Duration::from_micros(sleep_us));
                    sleep_us = (sleep_us * 2).min(2_000);
                }
                Err(err) => {
                    kill_child_pgrp(&mut child);
                    break Err(TigError::Git(format!(
                        "Failed to poll git subprocess: {err}"
                    )));
                }
            }
        };

        let stdout = stdout_handle.join().unwrap_or_default();
        let stderr = stderr_handle.join().unwrap_or_default();

        wait_res.map(|status| std::process::Output {
            status,
            stdout,
            stderr,
        })
    })
}

/// Scans repository status via Git CLI `git status --porcelain=v2 -z`.
pub(crate) fn scan_status_cli(
    work_dir: &Path,
    cancel: &CancellationToken,
) -> Result<StatusBuckets> {
    let mut cmd = crate::path_security::safe_git_command(work_dir);
    cmd.args(["status", "--porcelain=v2", "-z"]);
    let output = run_command_cancellable(&mut cmd, cancel)?;

    if !output.status.success() {
        let stderr_raw = String::from_utf8_lossy(&output.stderr);
        let stderr = tigrs_core::ansi::strip_control_chars(&stderr_raw).into_owned();
        return Err(TigError::Git(format!("git status failed: {stderr}")));
    }

    cancel.check_cancelled()?;
    Ok(parse_porcelain_v2(&output.stdout))
}

/// Parses raw NUL-delimited output from `git status --porcelain=v2 -z`.
pub(crate) fn parse_porcelain_v2(bytes: &[u8]) -> StatusBuckets {
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();
    let mut unmerged = Vec::new();

    let mut iter = bytes.split(|&b| b == 0);
    while let Some(record) = iter.next() {
        if record.is_empty() {
            continue;
        }

        match record[0] {
            b'1' => {
                // Ordinary changed entry:
                // 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
                if let Some((xy, path_bytes)) = parse_ordinary_record_bytes(record) {
                    let x = xy[0] as char;
                    let y = xy[1] as char;
                    if x != '.' {
                        staged.push(StatusItem::from_raw_bytes(
                            x,
                            StatusSection::Staged,
                            path_bytes.clone(),
                            None,
                        ));
                    }
                    if y != '.' {
                        unstaged.push(StatusItem::from_raw_bytes(
                            y,
                            StatusSection::Unstaged,
                            path_bytes,
                            None,
                        ));
                    }
                }
            }
            b'2' => {
                // Renamed or copied entry:
                // 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>\0<origPath>
                if let Some((xy, path_bytes)) = parse_renamed_record_bytes(record) {
                    let orig_path = iter.next().map(<[u8]>::to_vec);

                    let x = xy[0] as char;
                    let y = xy[1] as char;
                    if x != '.' {
                        staged.push(StatusItem::from_raw_bytes(
                            x,
                            StatusSection::Staged,
                            path_bytes.clone(),
                            orig_path.clone(),
                        ));
                    }
                    if y != '.' {
                        unstaged.push(StatusItem::from_raw_bytes(
                            y,
                            StatusSection::Unstaged,
                            path_bytes,
                            orig_path,
                        ));
                    }
                }
            }
            b'u' => {
                // Unmerged entry:
                // u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
                if let Some(path_bytes) = parse_unmerged_record_bytes(record) {
                    unmerged.push(StatusItem::from_raw_bytes(
                        'U',
                        StatusSection::Unmerged,
                        path_bytes,
                        None,
                    ));
                }
            }
            b'?' if record.len() >= 2 && record[1] == b' ' => {
                // Untracked entry:
                // ? <path>
                untracked.push(StatusItem::from_raw_bytes(
                    '?',
                    StatusSection::Untracked,
                    record[2..].to_vec(),
                    None,
                ));
            }
            _ => {}
        }
    }

    (staged, unstaged, untracked, unmerged)
}

/// Finds the index immediately following the `n`-th space in `bytes`.
fn skip_n_spaces(bytes: &[u8], n: usize) -> Option<usize> {
    let mut count = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b' ' {
            count += 1;
            if count == n {
                return Some(i + 1);
            }
        }
    }
    None
}

/// Extracts `<XY>` and raw `<path>` bytes from an ordinary changed entry (`1 ...`).
fn parse_ordinary_record_bytes(record: &[u8]) -> Option<([u8; 2], Vec<u8>)> {
    if record.len() < 4 || record[1] != b' ' {
        return None;
    }
    let xy = [record[2], record[3]];
    let path_start = skip_n_spaces(record, 8)?;
    Some((xy, record[path_start..].to_vec()))
}

/// Extracts `<XY>` and raw `<path>` bytes from a rename/copy entry (`2 ...`).
fn parse_renamed_record_bytes(record: &[u8]) -> Option<([u8; 2], Vec<u8>)> {
    if record.len() < 4 || record[1] != b' ' {
        return None;
    }
    let xy = [record[2], record[3]];
    let path_start = skip_n_spaces(record, 9)?;
    Some((xy, record[path_start..].to_vec()))
}

/// Extracts raw `<path>` bytes from an unmerged entry (`u ...`).
fn parse_unmerged_record_bytes(record: &[u8]) -> Option<Vec<u8>> {
    let path_start = skip_n_spaces(record, 10)?;
    Some(record[path_start..].to_vec())
}

/// Maximum number of diff lines parsed or synthesized for a single status diff.
const MAX_STATUS_DIFF_LINES: usize = 100_000;

/// Computes a viewable [`CommitDiff`] for a selected [`StatusItem`].
pub fn compute_status_item_diff(work_dir: &Path, item: &StatusItem) -> Result<CommitDiff> {
    compute_status_item_diff_cancellable(work_dir, item, &CancellationToken::none())
}

/// Cancellable variant of [`compute_status_item_diff`].
pub fn compute_status_item_diff_cancellable(
    work_dir: &Path,
    item: &StatusItem,
    cancel: &CancellationToken,
) -> Result<CommitDiff> {
    cancel.check_cancelled()?;
    crate::path_security::verify_relative_os_path(item.os_path())?;
    if let Some(parent) = Path::new(item.os_path()).parent()
        && !parent.as_os_str().is_empty()
    {
        crate::path_security::verify_worktree_os_path_safety(work_dir, parent.as_os_str())?;
    }
    match item.section {
        StatusSection::Staged => {
            let mut cmd = crate::path_security::safe_git_command(work_dir);
            cmd.args([
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "--cached",
                "--",
            ])
            .arg(item.os_path());
            let output = run_command_cancellable(&mut cmd, cancel)
                .map_err(|err| TigError::Git(format!("Failed to run git diff --cached: {err}")))?;
            if !output.status.success() {
                let stderr =
                    tigrs_core::ansi::strip_control_chars(&String::from_utf8_lossy(&output.stderr))
                        .into_owned();
                return Err(TigError::Git(format!(
                    "git diff --cached failed: {}",
                    stderr.trim()
                )));
            }
            let files = parse_unified_diff_bytes(
                &output.stdout,
                item.raw_path_bytes(),
                &FileChangeStatus::Modified,
            );
            Ok(wrap_status_diff(
                format!("Changes to be committed: {}", item.path),
                files,
            ))
        }
        StatusSection::Unstaged | StatusSection::Unmerged => {
            let mut cmd = crate::path_security::safe_git_command(work_dir);
            cmd.args([
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "--",
            ])
            .arg(item.os_path());
            let output = run_command_cancellable(&mut cmd, cancel)
                .map_err(|err| TigError::Git(format!("Failed to run git diff: {err}")))?;
            if !output.status.success() {
                let stderr =
                    tigrs_core::ansi::strip_control_chars(&String::from_utf8_lossy(&output.stderr))
                        .into_owned();
                return Err(TigError::Git(format!("git diff failed: {}", stderr.trim())));
            }
            let files = parse_unified_diff_bytes(
                &output.stdout,
                item.raw_path_bytes(),
                &FileChangeStatus::Modified,
            );
            Ok(wrap_status_diff(
                format!("Changes not staged for commit: {}", item.path),
                files,
            ))
        }
        StatusSection::Untracked => {
            if !work_dir.join(item.os_path()).is_symlink() {
                crate::path_security::verify_worktree_os_path_safety(work_dir, item.os_path())?;
            }
            let file_diff = untracked_file_diff_bytes(work_dir, item.raw_path_bytes());
            Ok(wrap_status_diff(
                format!("Untracked file: {}", item.path),
                vec![file_diff],
            ))
        }
    }
}

fn sanitize_diff_line_preserving_cr(line: &str) -> String {
    if let Some(without_cr) = line.strip_suffix('\r') {
        let mut clean = tigrs_core::ansi::strip_control_chars(without_cr).into_owned();
        clean.push('\r');
        clean
    } else {
        tigrs_core::ansi::strip_control_chars(line).into_owned()
    }
}

/// Synthesizes a whole-file addition diff for an untracked path.
///
/// Untracked files are unknown to `git diff`, so their contents are rendered as
/// a single all-additions hunk. Protects against FIFOs, symlinks, intermediate directory
/// symlinks escaping the worktree, and large files.
fn untracked_file_diff_bytes(work_dir: &Path, raw_path: &[u8]) -> FileDiff {
    let display_path =
        tigrs_core::ansi::strip_control_chars(&String::from_utf8_lossy(raw_path)).into_owned();
    let empty_diff = || FileDiff {
        path: display_path.clone(),
        status: FileChangeStatus::Added,
        old_id: None,
        new_id: None,
        old_mode: None,
        new_mode: Some(0o100_644),
        is_binary: false,
        additions: 0,
        deletions: 0,
        hunks: Vec::new(),
    };

    #[cfg(unix)]
    let os_path = {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(raw_path)
    };
    #[cfg(not(unix))]
    let os_path = std::ffi::OsStr::new(&display_path);

    if crate::path_security::verify_relative_os_path(os_path).is_err() {
        return empty_diff();
    }
    if let Some(parent) = Path::new(os_path).parent()
        && !parent.as_os_str().is_empty()
        && crate::path_security::verify_worktree_os_path_safety(work_dir, parent.as_os_str())
            .is_err()
    {
        return empty_diff();
    }

    let full_path = work_dir.join(os_path);
    let Ok(meta) = std::fs::symlink_metadata(&full_path) else {
        return empty_diff();
    };

    let file_type = meta.file_type();
    let (content, mode, is_binary) = if file_type.is_symlink() {
        let target = std::fs::read_link(&full_path)
            .map(|p| p.to_string_lossy().into_owned().into_bytes())
            .unwrap_or_default();
        (target, 0o120_000, false)
    } else if !file_type.is_file() || meta.len() > crate::diff::MAX_DIFF_BLOB_BYTES as u64 {
        // Reject FIFOs, sockets, device nodes, and files > 50 MiB
        (Vec::new(), 0o100_644, true)
    } else if crate::path_security::verify_worktree_os_path_safety(work_dir, os_path).is_err() {
        (Vec::new(), 0o100_644, false)
    } else {
        let bytes = std::fs::read(&full_path).unwrap_or_default();
        let bin = bytes.iter().take(8000).any(|&b| b == 0);
        #[cfg(unix)]
        let file_mode = {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o111 != 0 {
                0o100_755
            } else {
                0o100_644
            }
        };
        #[cfg(not(unix))]
        let file_mode = 0o100_644;
        (bytes, file_mode, bin)
    };

    let hunks = if is_binary || content.is_empty() {
        Vec::new()
    } else {
        let trimmed = content.strip_suffix(b"\n").unwrap_or(&content);
        let lacks_nl = !content.ends_with(b"\n");
        let raw_slices: Vec<&[u8]> = trimmed
            .split(|&b| b == b'\n')
            .take(MAX_STATUS_DIFF_LINES)
            .collect();
        let count_u32 = u32::try_from(raw_slices.len()).unwrap_or(u32::MAX);
        let mut hunk_lines: Vec<HunkLine> = raw_slices
            .into_iter()
            .enumerate()
            .map(|(line_idx, raw)| {
                let text = String::from_utf8_lossy(raw);
                let clean = sanitize_diff_line_preserving_cr(&text);
                crate::patch::record_raw_hunk_line(0, 0, 1, count_u32, line_idx, &clean, raw);
                HunkLine {
                    kind: DiffLineKind::Add,
                    content: clean,
                    no_newline_at_eof: false,
                }
            })
            .collect();
        if lacks_nl && let Some(last) = hunk_lines.last_mut() {
            last.no_newline_at_eof = true;
        }

        vec![DiffHunk {
            old_start: 0,
            old_len: 0,
            new_start: 1,
            new_len: count_u32,
            func_context: None,
            lines: hunk_lines,
        }]
    };

    let additions = hunks.iter().map(|h| h.lines.len()).sum();
    FileDiff {
        path: display_path,
        status: FileChangeStatus::Added,
        old_id: None,
        new_id: None,
        old_mode: None,
        new_mode: Some(mode),
        is_binary,
        additions,
        deletions: 0,
        hunks,
    }
}

/// Computes a viewable [`CommitDiff`] covering an entire worktree section.
///
/// This backs the synthetic "Staged changes" / "Unstaged changes" /
/// "Untracked changes" rows of the main view. Tracked sections are produced by a
/// single `git diff` invocation covering every path, rather than one process per
/// file.
pub fn compute_status_section_diff(
    work_dir: &Path,
    section: StatusSection,
    items: &[StatusItem],
) -> Result<CommitDiff> {
    compute_status_section_diff_cancellable(work_dir, section, items, &CancellationToken::none())
}

/// Cancellable variant of [`compute_status_section_diff`].
pub fn compute_status_section_diff_cancellable(
    work_dir: &Path,
    section: StatusSection,
    items: &[StatusItem],
    cancel: &CancellationToken,
) -> Result<CommitDiff> {
    cancel.check_cancelled()?;
    match section {
        StatusSection::Untracked => {
            let mut files = Vec::with_capacity(items.len());
            for (idx, item) in items.iter().enumerate() {
                if idx % 64 == 0 {
                    cancel.check_cancelled()?;
                }
                files.push(untracked_file_diff_bytes(work_dir, item.raw_path_bytes()));
            }
            Ok(wrap_status_diff("Untracked changes".to_string(), files))
        }
        StatusSection::Staged | StatusSection::Unstaged | StatusSection::Unmerged => {
            let staged = section == StatusSection::Staged;
            let mut args = vec![
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--src-prefix=a/",
                "--dst-prefix=b/",
            ];
            if staged {
                args.push("--cached");
            }
            let mut cmd = crate::path_security::safe_git_command(work_dir);
            cmd.args(&args);
            let output = run_command_cancellable(&mut cmd, cancel)?;
            if !output.status.success() {
                let stderr =
                    tigrs_core::ansi::strip_control_chars(&String::from_utf8_lossy(&output.stderr))
                        .into_owned();
                return Err(TigError::Git(format!("git diff failed: {}", stderr.trim())));
            }

            let files = if output.stdout.iter().all(u8::is_ascii_whitespace) {
                // `parse_unified_diff_bytes` synthesizes a placeholder entry for empty
                // input so single-file callers always get a `FileDiff`. For a
                // whole section that placeholder would be a phantom file.
                Vec::new()
            } else {
                // The real path comes from each `diff --git` header; the default
                // only applies to malformed output.
                parse_unified_diff_bytes(&output.stdout, b"", &FileChangeStatus::Modified)
            };

            let title = if staged {
                "Staged changes"
            } else {
                "Unstaged changes"
            };
            Ok(wrap_status_diff(title.to_string(), files))
        }
    }
}

/// Packages one or more [`FileDiff`] items into a [`CommitDiff`] structure for [`DiffView`].
fn wrap_status_diff(title: String, files: Vec<FileDiff>) -> CommitDiff {
    let mut additions = 0;
    let mut deletions = 0;
    for f in &files {
        additions += f.additions;
        deletions += f.deletions;
    }

    CommitDiff {
        commit_id: ObjectId::empty_tree(gix::hash::Kind::Sha1),
        parent_ids: Vec::new(),
        author_name: Arc::from("Working Tree"),
        author_email: Arc::from("worktree@local"),
        author_date: String::new(),
        committer_name: Arc::from("Working Tree"),
        committer_email: Arc::from("worktree@local"),
        committer_date: String::new(),
        title: Arc::from(title),
        body: None,
        stats: DiffSummaryStats {
            files_changed: files.len(),
            insertions: additions,
            deletions,
        },
        files,
    }
}

fn sanitize_header_path_bytes(raw: &[u8]) -> String {
    let unquoted = crate::patch::unquote_c_path_bytes(raw, false);
    tigrs_core::ansi::strip_control_chars(&String::from_utf8_lossy(&unquoted)).into_owned()
}

/// Parses unified diff output into a list of [`FileDiff`] entries (UTF-8 convenience wrapper).
#[cfg(test)]
pub(crate) fn parse_unified_diff(
    diff_text: &str,
    default_path: &str,
    default_status: &FileChangeStatus,
) -> Vec<FileDiff> {
    parse_unified_diff_bytes(
        diff_text.as_bytes(),
        default_path.as_bytes(),
        default_status,
    )
}

/// Parses raw unified diff bytes into a list of [`FileDiff`] entries while recording raw hunk line bytes.
pub(crate) fn parse_unified_diff_bytes(
    diff_bytes: &[u8],
    default_path_bytes: &[u8],
    default_status: &FileChangeStatus,
) -> Vec<FileDiff> {
    let mut files = Vec::new();
    let mut current_hunks = Vec::new();
    let default_path =
        tigrs_core::ansi::strip_control_chars(&String::from_utf8_lossy(default_path_bytes))
            .into_owned();
    let mut current_path = default_path;
    let mut current_status = default_status.clone();
    let mut is_binary = false;
    let mut in_hunk = false;
    let mut has_file = false;
    let mut current_hunk: Option<DiffHunk> = None;

    // Split strictly on b'\n' so trailing b'\r' on CRLF hunk lines is preserved verbatim,
    // streaming up to MAX_STATUS_DIFF_LINES without allocating an unbounded Vec.
    let trimmed_diff = diff_bytes.strip_suffix(b"\n").unwrap_or(diff_bytes);
    for raw_line in trimmed_diff
        .split(|&b| b == b'\n')
        .take(MAX_STATUS_DIFF_LINES)
    {
        let header_bytes = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        if let Some(rest) = header_bytes.strip_prefix(b"diff --git ") {
            if let Some(hunk) = current_hunk.take() {
                current_hunks.push(hunk);
            }
            if has_file {
                let additions = current_hunks
                    .iter()
                    .map(|h: &DiffHunk| {
                        h.lines
                            .iter()
                            .filter(|l| l.kind == DiffLineKind::Add)
                            .count()
                    })
                    .sum();
                let deletions = current_hunks
                    .iter()
                    .map(|h: &DiffHunk| {
                        h.lines
                            .iter()
                            .filter(|l| l.kind == DiffLineKind::Remove)
                            .count()
                    })
                    .sum();

                files.push(FileDiff {
                    path: current_path.clone(),
                    status: current_status.clone(),
                    old_id: None,
                    new_id: None,
                    old_mode: None,
                    new_mode: None,
                    is_binary,
                    additions,
                    deletions,
                    hunks: std::mem::take(&mut current_hunks),
                });
            }
            has_file = true;
            if let Some(pos) = rest.windows(3).rposition(|w| w == b" b/") {
                current_path = sanitize_header_path_bytes(&rest[pos + 3..]);
            } else if let Some(pos) = rest.windows(4).rposition(|w| w == b" \"b/") {
                let unquoted = crate::patch::unquote_c_path_bytes(&rest[pos + 1..], true);
                current_path =
                    tigrs_core::ansi::strip_control_chars(&String::from_utf8_lossy(&unquoted))
                        .into_owned();
            }
            current_status = default_status.clone();
            in_hunk = false;
            is_binary = false;
        } else if !in_hunk && header_bytes.starts_with(b"new file mode ") {
            current_status = FileChangeStatus::Added;
        } else if !in_hunk && header_bytes.starts_with(b"deleted file mode ") {
            current_status = FileChangeStatus::Deleted;
        } else if !in_hunk && let Some(src) = header_bytes.strip_prefix(b"rename from ") {
            let safe_src = sanitize_header_path_bytes(src);
            current_status = FileChangeStatus::Renamed {
                source_path: safe_src,
                similarity_pct: 100,
            };
        } else if !in_hunk && let Some(dest) = header_bytes.strip_prefix(b"rename to ") {
            current_path = sanitize_header_path_bytes(dest);
            if !matches!(current_status, FileChangeStatus::Renamed { .. }) {
                current_status = FileChangeStatus::Renamed {
                    source_path: String::new(),
                    similarity_pct: 100,
                };
            }
        } else if !in_hunk && let Some(plus_rest) = header_bytes.strip_prefix(b"+++ ") {
            let unquoted = crate::patch::unquote_c_path_bytes(plus_rest, true);
            if unquoted != b"/dev/null" {
                current_path =
                    tigrs_core::ansi::strip_control_chars(&String::from_utf8_lossy(&unquoted))
                        .into_owned();
            }
        } else if !in_hunk && header_bytes.starts_with(b"Binary files ") {
            is_binary = true;
        } else if !in_hunk && header_bytes.starts_with(b"--- ") {
            // Path header outside hunk
        } else if header_bytes.starts_with(b"@@ ") {
            if let Some(hunk) = current_hunk.take() {
                current_hunks.push(hunk);
            }
            in_hunk = true;
            let header_str = String::from_utf8_lossy(header_bytes);
            if let Some(hunk) = parse_hunk_header(&header_str) {
                current_hunk = Some(hunk);
            }
        } else if in_hunk && let Some(ref mut hunk) = current_hunk {
            if let Some(raw_rest) = raw_line.strip_prefix(b"+") {
                let text = String::from_utf8_lossy(raw_rest);
                let content = sanitize_diff_line_preserving_cr(&text);
                let line_idx = hunk.lines.len();
                crate::patch::record_raw_hunk_line(
                    hunk.old_start,
                    hunk.old_len,
                    hunk.new_start,
                    hunk.new_len,
                    line_idx,
                    &content,
                    raw_rest,
                );
                hunk.lines.push(HunkLine {
                    kind: DiffLineKind::Add,
                    content,
                    no_newline_at_eof: false,
                });
            } else if let Some(raw_rest) = raw_line.strip_prefix(b"-") {
                let text = String::from_utf8_lossy(raw_rest);
                let content = sanitize_diff_line_preserving_cr(&text);
                let line_idx = hunk.lines.len();
                crate::patch::record_raw_hunk_line(
                    hunk.old_start,
                    hunk.old_len,
                    hunk.new_start,
                    hunk.new_len,
                    line_idx,
                    &content,
                    raw_rest,
                );
                hunk.lines.push(HunkLine {
                    kind: DiffLineKind::Remove,
                    content,
                    no_newline_at_eof: false,
                });
            } else if let Some(raw_rest) = raw_line.strip_prefix(b" ") {
                let text = String::from_utf8_lossy(raw_rest);
                let content = sanitize_diff_line_preserving_cr(&text);
                let line_idx = hunk.lines.len();
                crate::patch::record_raw_hunk_line(
                    hunk.old_start,
                    hunk.old_len,
                    hunk.new_start,
                    hunk.new_len,
                    line_idx,
                    &content,
                    raw_rest,
                );
                hunk.lines.push(HunkLine {
                    kind: DiffLineKind::Context,
                    content,
                    no_newline_at_eof: false,
                });
            } else if raw_line.is_empty() || raw_line == b"\r" {
                let text = String::from_utf8_lossy(raw_line);
                let content = sanitize_diff_line_preserving_cr(&text);
                let line_idx = hunk.lines.len();
                crate::patch::record_raw_hunk_line(
                    hunk.old_start,
                    hunk.old_len,
                    hunk.new_start,
                    hunk.new_len,
                    line_idx,
                    &content,
                    raw_line,
                );
                hunk.lines.push(HunkLine {
                    kind: DiffLineKind::Context,
                    content,
                    no_newline_at_eof: false,
                });
            } else if header_bytes.starts_with(b"\\ No newline at end of file")
                && let Some(last) = hunk.lines.last_mut()
            {
                last.no_newline_at_eof = true;
            }
        }
    }

    if let Some(hunk) = current_hunk.take() {
        current_hunks.push(hunk);
    }
    if has_file || !current_hunks.is_empty() || is_binary || files.is_empty() {
        let additions = current_hunks
            .iter()
            .map(|h: &DiffHunk| {
                h.lines
                    .iter()
                    .filter(|l| l.kind == DiffLineKind::Add)
                    .count()
            })
            .sum();
        let deletions = current_hunks
            .iter()
            .map(|h: &DiffHunk| {
                h.lines
                    .iter()
                    .filter(|l| l.kind == DiffLineKind::Remove)
                    .count()
            })
            .sum();

        files.push(FileDiff {
            path: current_path,
            status: current_status,
            old_id: None,
            new_id: None,
            old_mode: None,
            new_mode: None,
            is_binary,
            additions,
            deletions,
            hunks: current_hunks,
        });
    }

    files
}

/// Parses a hunk header line like `@@ -1,5 +1,6 @@ optional context`.
fn parse_hunk_header(line: &str) -> Option<DiffHunk> {
    let trimmed = line.strip_prefix("@@ -")?;
    let at_idx = trimmed.find(" @@")?;
    let ranges_part = &trimmed[..at_idx];
    let func_context = trimmed[at_idx + 3..].trim();
    let func_context = if func_context.is_empty() {
        None
    } else {
        Some(tigrs_core::ansi::strip_control_chars(func_context).into_owned())
    };

    let mut parts = ranges_part.split(' ');
    let old_part = parts.next()?;
    let new_part = parts.next()?.strip_prefix('+')?;

    let (old_start, old_len) = parse_range(old_part)?;
    let (new_start, new_len) = parse_range(new_part)?;

    Some(DiffHunk {
        old_start,
        old_len,
        new_start,
        new_len,
        func_context,
        lines: Vec::new(),
    })
}

/// Parses a `start,len` or `start` range.
fn parse_range(s: &str) -> Option<(u32, u32)> {
    if let Some((start, len)) = s.split_once(',') {
        Some((start.parse().ok()?, len.parse().ok()?))
    } else {
        Some((s.parse().ok()?, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_porcelain_v2_ordinary_and_untracked() {
        let input = b"1 .M N... 100644 100644 100644 93aee31 93aee31 src/lib.rs\0\
                      1 M. N... 100644 100644 100644 93aee31 93aee31 src/staged.rs\0\
                      1 MM N... 100644 100644 100644 93aee31 93aee31 src/both.rs\0\
                      ? untracked_file.txt\0";

        let (staged, unstaged, untracked, unmerged) = parse_porcelain_v2(input);

        assert_eq!(staged.len(), 2);
        assert_eq!(staged[0].path, "src/staged.rs");
        assert_eq!(staged[0].status_code, 'M');
        assert_eq!(staged[1].path, "src/both.rs");
        assert_eq!(staged[1].status_code, 'M');

        assert_eq!(unstaged.len(), 2);
        assert_eq!(unstaged[0].path, "src/lib.rs");
        assert_eq!(unstaged[0].status_code, 'M');
        assert_eq!(unstaged[1].path, "src/both.rs");
        assert_eq!(unstaged[1].status_code, 'M');

        assert_eq!(untracked.len(), 1);
        assert_eq!(untracked[0].path, "untracked_file.txt");
        assert_eq!(untracked[0].status_code, '?');

        assert!(unmerged.is_empty());
    }

    #[test]
    fn test_parse_porcelain_v2_renamed() {
        let input =
            b"2 R. N... 100644 100644 100644 93aee31 93aee31 R100 new_name.rs\0old_name.rs\0";
        let (staged, unstaged, untracked, unmerged) = parse_porcelain_v2(input);

        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].path, "new_name.rs");
        assert_eq!(staged[0].status_code, 'R');
        assert_eq!(staged[0].old_path.as_deref(), Some("old_name.rs"));

        assert!(unstaged.is_empty());
        assert!(untracked.is_empty());
        assert!(unmerged.is_empty());
    }

    #[test]
    fn test_parse_porcelain_v2_with_spaces_in_path() {
        let input = b"1 .M N... 100644 100644 100644 93aee31 93aee31 a path with spaces.txt\0\
                      ? another path with spaces.txt\0";
        let (staged, unstaged, untracked, _) = parse_porcelain_v2(input);

        assert!(staged.is_empty());
        assert_eq!(unstaged.len(), 1);
        assert_eq!(unstaged[0].path, "a path with spaces.txt");
        assert_eq!(untracked.len(), 1);
        assert_eq!(untracked[0].path, "another path with spaces.txt");
    }

    #[test]
    fn test_parse_unified_diff_hunk() {
        let diff = r"diff --git a/foo.rs b/foo.rs
index 1234567..89abcde 100644
--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +1,4 @@ fn main()
 line1
+line2
 line3
\ No newline at end of file
";

        let files = parse_unified_diff(diff, "foo.rs", &FileChangeStatus::Modified);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "foo.rs");
        assert_eq!(files[0].hunks.len(), 1);

        let hunk = &files[0].hunks[0];
        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_len, 3);
        assert_eq!(hunk.new_start, 1);
        assert_eq!(hunk.new_len, 4);
        assert_eq!(hunk.func_context.as_deref(), Some("fn main()"));
        assert_eq!(hunk.lines.len(), 3);
        assert_eq!(hunk.lines[0].kind, DiffLineKind::Context);
        assert_eq!(hunk.lines[1].kind, DiffLineKind::Add);
        assert_eq!(hunk.lines[2].kind, DiffLineKind::Context);
        assert!(hunk.lines[2].no_newline_at_eof);
    }

    #[test]
    fn test_scan_status_real_repo_and_diffs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path();

        let run = |args: &[&str]| {
            let st = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(path)
                .status()
                .expect("git status");
            assert!(st.success());
        };

        run(&["init"]);
        run(&["config", "user.name", "Tester"]);
        run(&["config", "user.email", "tester@test.com"]);

        // 1. Base commit with tracked.txt
        std::fs::write(path.join("tracked.txt"), "line 1\nline 2\n").expect("write");
        run(&["add", "tracked.txt"]);
        run(&["commit", "-m", "initial"]);

        // 2. Modify tracked.txt (unstaged)
        std::fs::write(path.join("tracked.txt"), "line 1\nline 2 modified\n").expect("write");

        // 3. Add staged.txt (staged)
        std::fs::write(path.join("staged.txt"), "new file content\n").expect("write");
        run(&["add", "staged.txt"]);

        // 4. Add untracked.txt (untracked)
        std::fs::write(path.join("untracked.txt"), "hello untracked\n").expect("write");

        let engine = crate::GitEngine::open(Some(path)).expect("open engine");
        let (_src, token) = CancellationToken::new();

        let report = engine.load_status(&token).expect("load status");
        assert_eq!(report.staged.len(), 1);
        assert_eq!(report.staged[0].path, "staged.txt");
        assert_eq!(report.staged[0].status_code, 'A');
        assert_eq!(report.staged[0].section, StatusSection::Staged);

        assert_eq!(report.unstaged.len(), 1);
        assert_eq!(report.unstaged[0].path, "tracked.txt");
        assert_eq!(report.unstaged[0].status_code, 'M');
        assert_eq!(report.unstaged[0].section, StatusSection::Unstaged);

        assert_eq!(report.untracked.len(), 1);
        assert_eq!(report.untracked[0].path, "untracked.txt");
        assert_eq!(report.untracked[0].status_code, '?');
        assert_eq!(report.untracked[0].section, StatusSection::Untracked);

        assert_eq!(report.total_count(), 3);
        assert!(!report.is_empty());

        // Test diff computation for each section
        let staged_diff = engine
            .compute_status_item_diff(&report.staged[0])
            .expect("staged diff");
        assert_eq!(staged_diff.files.len(), 1);
        assert_eq!(staged_diff.files[0].path, "staged.txt");

        let unstaged_diff = engine
            .compute_status_item_diff(&report.unstaged[0])
            .expect("unstaged diff");
        assert_eq!(unstaged_diff.files.len(), 1);
        assert_eq!(unstaged_diff.files[0].path, "tracked.txt");

        let untracked_diff = engine
            .compute_status_item_diff(&report.untracked[0])
            .expect("untracked diff");
        assert_eq!(untracked_diff.files.len(), 1);
        assert_eq!(untracked_diff.files[0].path, "untracked.txt");
        assert_eq!(untracked_diff.files[0].status, FileChangeStatus::Added);
        assert_eq!(untracked_diff.files[0].additions, 1);
    }

    #[test]
    fn test_status_section_and_item_descriptions() {
        assert_eq!(StatusSection::Staged.title(), "Changes to be committed");
        assert_eq!(
            StatusSection::Unstaged.title(),
            "Changes not staged for commit"
        );
        assert_eq!(StatusSection::Untracked.title(), "Untracked files");
        assert_eq!(StatusSection::Unmerged.title(), "Unmerged paths");

        let check_desc = |code: char, expected: &str| {
            let item = StatusItem::new(code, StatusSection::Staged, "file.txt", None);
            assert_eq!(item.label(), expected);
        };

        check_desc('M', "modified");
        check_desc('A', "new file");
        check_desc('D', "deleted");
        check_desc('R', "renamed");
        check_desc('C', "copied");
        check_desc('T', "typechange");
        check_desc('U', "unmerged");
        check_desc('?', "untracked");
        check_desc('Z', "modified");
    }

    #[test]
    fn test_parse_porcelain_v2_unmerged_and_edge_cases() {
        // u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
        // 10 spaces precede the path
        let input = b"u UU N... 100644 100644 100644 100644 1111111 2222222 3333333 conflict.txt\0\
                      2 RM N... 100644 100644 100644 93aee31 93aee31 R100 dest.txt\0src.txt\0\
                      1 \0\
                      2 \0\
                      u \0\
                      # branch.oid 123456\0";

        let (staged, unstaged, untracked, unmerged) = parse_porcelain_v2(input);
        assert_eq!(unmerged.len(), 1);
        assert_eq!(unmerged[0].path, "conflict.txt");
        assert_eq!(unmerged[0].section, StatusSection::Unmerged);
        assert_eq!(unmerged[0].status_code, 'U');

        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].path, "dest.txt");
        assert_eq!(staged[0].old_path.as_deref(), Some("src.txt"));
        assert_eq!(staged[0].status_code, 'R');

        assert_eq!(unstaged.len(), 1);
        assert_eq!(unstaged[0].path, "dest.txt");
        assert_eq!(unstaged[0].old_path.as_deref(), Some("src.txt"));
        assert_eq!(unstaged[0].status_code, 'M');

        assert!(untracked.is_empty());
    }

    #[test]
    fn test_parse_unified_diff_multiple_files_deleted_and_binary() {
        let diff = r"diff --git a/binary.bin b/binary.bin
index 1234567..89abcde 100644
Binary files a/binary.bin and b/binary.bin differ
diff --git a/deleted.txt b/deleted.txt
deleted file mode 100644
index 1234567..0000000
--- a/deleted.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-del1
-del2
";
        let files = parse_unified_diff(diff, "binary.bin", &FileChangeStatus::Modified);
        assert_eq!(files.len(), 2);

        assert_eq!(files[0].path, "binary.bin");
        assert!(files[0].is_binary);

        assert_eq!(files[1].path, "deleted.txt");
        assert_eq!(files[1].status, FileChangeStatus::Deleted);
        assert_eq!(files[1].deletions, 2);
        assert_eq!(files[1].additions, 0);
    }

    #[test]
    fn test_scan_status_gix_pure_files_repo() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path();

        let run = |args: &[&str]| {
            let st = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(path)
                .status()
                .expect("git status");
            assert!(st.success());
        };

        let init_files = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init", "--ref-format=files"])
            .current_dir(path)
            .status()
            .expect("git init");
        if !init_files.success() {
            run(&["init"]);
        }
        run(&["config", "user.name", "Tester"]);
        run(&["config", "user.email", "tester@test.com"]);

        // Commit initial files
        std::fs::write(path.join("to_modify.txt"), "line 1\n").expect("write");
        std::fs::write(path.join("to_delete.txt"), "line 1\n").expect("write");
        std::fs::write(path.join("unstaged_mod.txt"), "line 1\n").expect("write");
        std::fs::write(path.join("unstaged_del.txt"), "line 1\n").expect("write");
        run(&["add", "."]);
        run(&["commit", "-m", "init"]);

        // Staged changes:
        // 1. Modify to_modify.txt
        std::fs::write(path.join("to_modify.txt"), "line 1\nline 2\n").expect("write");
        run(&["add", "to_modify.txt"]);

        // 2. Delete to_delete.txt
        run(&["rm", "to_delete.txt"]);

        // 3. Add new staged file
        std::fs::write(path.join("staged_add.txt"), "staged\n").expect("write");
        run(&["add", "staged_add.txt"]);

        // Unstaged changes:
        // 4. Modify unstaged_mod.txt
        std::fs::write(path.join("unstaged_mod.txt"), "modified\n").expect("write");

        // 5. Delete unstaged_del.txt
        std::fs::remove_file(path.join("unstaged_del.txt")).expect("remove");

        // 6. Untracked file
        std::fs::write(path.join("untracked.txt"), "untracked\n").expect("write");

        let repo = gix::open(path).expect("open gix repo");
        let (cancel_src, cancel) = CancellationToken::new();

        let (staged, unstaged, untracked, _unmerged) =
            scan_status_gix(&repo, &cancel).expect("scan_status_gix");

        assert!(
            staged
                .iter()
                .any(|i| i.path == "staged_add.txt" && i.status_code == 'A')
        );
        assert!(
            staged
                .iter()
                .any(|i| i.path == "to_delete.txt" && i.status_code == 'D')
        );
        assert!(
            staged
                .iter()
                .any(|i| i.path == "to_modify.txt" && i.status_code == 'M')
        );

        assert!(
            unstaged
                .iter()
                .any(|i| i.path == "unstaged_mod.txt" && i.status_code == 'M')
        );
        assert!(
            unstaged
                .iter()
                .any(|i| i.path == "unstaged_del.txt" && i.status_code == 'D')
        );

        assert!(
            untracked
                .iter()
                .any(|i| i.path == "untracked.txt" && i.status_code == '?')
        );

        // Test cancellation
        cancel_src.cancel();
        let err = scan_status_gix(&repo, &cancel);
        assert!(err.is_err());
    }

    #[test]
    fn test_status_diff_edge_cases_and_cli_errors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path();

        let (_src, token) = CancellationToken::new();

        // Calling scan_status_cli on non-git dir should fail
        let err = scan_status_cli(path, &token);
        assert!(err.is_err());

        // Test untracked diff with empty file, binary file, and file with no trailing newline
        std::fs::write(path.join("empty.txt"), b"").expect("write");
        std::fs::write(path.join("binary.bin"), b"hello\0world").expect("write");
        std::fs::write(path.join("no_newline.txt"), b"one line").expect("write");

        let empty_item = StatusItem::new('?', StatusSection::Untracked, "empty.txt", None);
        let bin_item = StatusItem::new('?', StatusSection::Untracked, "binary.bin", None);
        let no_nl_item = StatusItem::new('?', StatusSection::Untracked, "no_newline.txt", None);

        let empty_diff = compute_status_item_diff(path, &empty_item).expect("empty diff");
        assert_eq!(empty_diff.files[0].hunks.len(), 0);

        let bin_diff = compute_status_item_diff(path, &bin_item).expect("bin diff");
        assert!(bin_diff.files[0].is_binary);
        assert_eq!(bin_diff.files[0].hunks.len(), 0);

        let no_nl_diff = compute_status_item_diff(path, &no_nl_item).expect("no nl diff");
        assert_eq!(no_nl_diff.files[0].hunks.len(), 1);
        assert!(no_nl_diff.files[0].hunks[0].lines[0].no_newline_at_eof);

        // Test compute_status_section_diff for Untracked
        let section_diff = compute_status_section_diff(
            path,
            StatusSection::Untracked,
            &[empty_item, bin_item, no_nl_item],
        )
        .expect("section diff");
        assert_eq!(section_diff.files.len(), 3);
        assert_eq!(section_diff.title.as_ref(), "Untracked changes");
    }

    #[test]
    fn test_scan_status_gix_cache_tree_fast_path_and_invalidation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path();

        let init = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init", "-b", "main", "--ref-format=files"])
            .current_dir(path)
            .output()
            .expect("git init");
        if !init.status.success() {
            return;
        }

        let run = |args: &[&str]| {
            let status = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(path)
                .status()
                .expect("git cmd");
            assert!(status.success(), "git {args:?} failed");
        };

        run(&["config", "user.name", "Alice Developer"]);
        run(&["config", "user.email", "alice@example.com"]);

        std::fs::write(path.join("tracked_a.txt"), "alpha\n").unwrap();
        std::fs::write(path.join("tracked_b.txt"), "beta\n").unwrap();
        run(&["add", "tracked_a.txt", "tracked_b.txt"]);
        run(&["commit", "-m", "initial commit"]);

        let repo = gix::open(path).expect("open gix repo");
        let head_tree_id = repo
            .head_commit()
            .expect("head commit")
            .tree_id()
            .expect("tree id")
            .detach();

        // Phase 1: Immediately after `git commit`, `.git/index` has a valid root `cache-tree`
        // matching `HEAD^{tree}`. Modify worktree files without staging: `index_matches_head_tree`
        // must return `true` (fast path taken), and `scan_status_gix` must still report all
        // unstaged modifications/deletions and untracked files while `staged` is empty.
        std::fs::write(path.join("tracked_a.txt"), "alpha modified in worktree\n").unwrap();
        std::fs::remove_file(path.join("tracked_b.txt")).unwrap();
        std::fs::write(path.join("untracked_new.txt"), "untracked\n").unwrap();

        {
            let index = repo.index_or_empty().expect("index");
            assert!(
                index_matches_head_tree(&index, Some(head_tree_id)),
                "Clean index after git commit must take the cache-tree fast path"
            );
        }

        let (_src, cancel) = CancellationToken::new();
        let (staged, unstaged, untracked, _unmerged) =
            scan_status_gix(&repo, &cancel).expect("scan phase 1");
        assert!(
            staged.is_empty(),
            "Fast path must report zero staged items, got: {staged:?}"
        );
        assert!(
            unstaged
                .iter()
                .any(|i| i.path == "tracked_a.txt" && i.status_code == 'M')
        );
        assert!(
            unstaged
                .iter()
                .any(|i| i.path == "tracked_b.txt" && i.status_code == 'D')
        );
        assert!(
            untracked
                .iter()
                .any(|i| i.path == "untracked_new.txt" && i.status_code == '?')
        );

        // Phase 2: `git add -N` (`INTENT_TO_ADD`) must invalidate the `cache-tree` fast path.
        run(&["checkout", "--", "tracked_b.txt"]);
        run(&["add", "-N", "untracked_new.txt"]);
        {
            let repo_reopened = gix::open(path).expect("reopen gix repo");
            let index = repo_reopened.index_or_empty().expect("index");
            assert!(
                !index_matches_head_tree(&index, Some(head_tree_id)),
                "git add -N (INTENT_TO_ADD) must bypass the cache-tree fast path"
            );
        }

        // Phase 3: Staging a modification and then modifying the same file again in the worktree (`MM`)
        // must bypass the fast path and report `tracked_a.txt` in BOTH `staged` and `unstaged`.
        run(&["add", "tracked_a.txt"]);
        std::fs::write(
            path.join("tracked_a.txt"),
            "alpha modified again after staging\n",
        )
        .unwrap();
        let repo_mm = gix::open(path).expect("reopen gix repo for MM");
        {
            let index = repo_mm.index_or_empty().expect("index");
            assert!(
                !index_matches_head_tree(&index, Some(head_tree_id)),
                "Staged modification must invalidate cache-tree fast path"
            );
        }
        let (staged_mm, unstaged_mm, _untracked_mm, _unmerged_mm) =
            scan_status_gix(&repo_mm, &cancel).expect("scan MM");
        assert!(
            staged_mm
                .iter()
                .any(|i| i.path == "tracked_a.txt" && i.status_code == 'M'),
            "Expected tracked_a.txt in staged items, got: {staged_mm:?}"
        );
        assert!(
            unstaged_mm
                .iter()
                .any(|i| i.path == "tracked_a.txt" && i.status_code == 'M'),
            "Expected tracked_a.txt in unstaged items, got: {unstaged_mm:?}"
        );
    }
}
