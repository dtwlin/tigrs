// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Commit, tree, and working-tree diff computation, hunk extraction, and diffstat calculation.

use crate::userdiff::{DriverResolver, FuncMatcher};
use gix::ObjectId;
use gix::bstr::ByteSlice;
use gix::diff::blob::InternedInput;
use gix::diff::blob::platform::prepare_diff::Operation;
use gix::diff::blob::unified_diff::{
    ConsumeHunk, ContextSize, DiffLineKind as GixDiffLineKind, HunkHeader,
};
use std::rc::Rc;
use std::sync::Arc;
use tigrs_core::ansi::strip_control_chars;
use tigrs_core::error::{Result, TigError};

/// Maximum blob size (in bytes) loaded for diff computation (50 MB).
pub const MAX_DIFF_BLOB_BYTES: usize = 50 * 1024 * 1024;

/// Represents the status of a changed file in a commit diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChangeStatus {
    /// File was created.
    Added,
    /// File was deleted.
    Deleted,
    /// File content or mode was modified.
    Modified,
    /// File was renamed from another path.
    Renamed {
        /// Source path before rename.
        source_path: String,
        /// Similarity percentage (0..100).
        similarity_pct: u32,
    },
    /// File was copied from another path.
    Copied {
        /// Source path copied from.
        source_path: String,
        /// Similarity percentage (0..100).
        similarity_pct: u32,
    },
    /// Entry type changed (e.g. file to symlink).
    TypeChanged,
}

/// A line in a diff hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// Unchanged context line.
    Context,
    /// Added line (+).
    Add,
    /// Removed line (-).
    Remove,
}

/// A single line in a diff hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkLine {
    /// Whether this is an addition, deletion, or context line.
    pub kind: DiffLineKind,
    /// Text content of the line without line terminator.
    pub content: String,
    /// True when this is the final line of its side's blob and that blob does
    /// not end in a newline. Git renders `\ No newline at end of file` after it.
    pub no_newline_at_eof: bool,
}

/// A unified diff hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    /// 1-based start line in the old file.
    pub old_start: u32,
    /// Number of lines in the old hunk.
    pub old_len: u32,
    /// 1-based start line in the new file.
    pub new_start: u32,
    /// Number of lines in the new hunk.
    pub new_len: u32,
    /// Enclosing "function" line shown after the `@@ ... @@` marker, if any.
    pub func_context: Option<String>,
    /// Lines comprising the hunk.
    pub lines: Vec<HunkLine>,
}

/// Diff for a single file in a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Relative path of the file in the repository.
    pub path: String,
    /// Kind of change (addition, deletion, modification, rename).
    pub status: FileChangeStatus,
    /// Old object ID, if available.
    pub old_id: Option<ObjectId>,
    /// New object ID, if available.
    pub new_id: Option<ObjectId>,
    /// Old file mode, if available.
    pub old_mode: Option<u32>,
    /// New file mode, if available.
    pub new_mode: Option<u32>,
    /// True if either side is detected as binary.
    pub is_binary: bool,
    /// Number of added lines.
    pub additions: usize,
    /// Number of deleted lines.
    pub deletions: usize,
    /// Diff hunks for text files.
    pub hunks: Vec<DiffHunk>,
}

/// Aggregated diff statistics for a commit.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiffSummaryStats {
    /// Total number of files changed.
    pub files_changed: usize,
    /// Total insertions across all files.
    pub insertions: usize,
    /// Total deletions across all files.
    pub deletions: usize,
}

/// Full diff information for a single commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDiff {
    /// Object ID of the commit.
    pub commit_id: ObjectId,
    /// Parent commit IDs.
    pub parent_ids: Vec<ObjectId>,
    /// Author name.
    pub author_name: Arc<str>,
    /// Author email.
    pub author_email: Arc<str>,
    /// Formatted author date (e.g. "Tue Jul 28 01:00:47 2026 +0200").
    pub author_date: String,
    /// Committer name.
    pub committer_name: Arc<str>,
    /// Committer email.
    pub committer_email: Arc<str>,
    /// Formatted committer date.
    pub committer_date: String,
    /// Commit subject (first line of commit message).
    pub title: Arc<str>,
    /// Remaining body of the commit message, if non-empty.
    pub body: Option<String>,
    /// List of file diffs.
    pub files: Vec<FileDiff>,
    /// Aggregated diff statistics.
    pub stats: DiffSummaryStats,
}

/// Counts lines in a blob using Git's tokenization: a trailing newline does not
/// start a new line, and an empty blob has zero lines.
fn count_lines(buf: &[u8]) -> u32 {
    if buf.is_empty() {
        return 0;
    }
    let newlines = u32::try_from(memchr::memchr_iter(b'\n', buf).count()).unwrap_or(u32::MAX);
    if buf.ends_with(b"\n") {
        newlines
    } else {
        newlines.saturating_add(1)
    }
}

/// Whether a blob is non-empty and lacks a trailing newline.
fn lacks_trailing_newline(buf: &[u8]) -> bool {
    !buf.is_empty() && !buf.ends_with(b"\n")
}

/// Splits a blob into lines with terminators removed, matching Git's tokenizer.
pub fn split_lines(buf: &[u8]) -> Vec<&[u8]> {
    if buf.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<&[u8]> = buf.split(|&b| b == b'\n').collect();
    // A trailing newline yields a final empty element that is not a real line.
    if buf.ends_with(b"\n") {
        out.pop();
    }
    out
}

/// Finds the function context Git attaches to a hunk starting at
/// `hunk_start_1based` in the old file.
///
/// The scan runs backwards from the line immediately preceding the hunk all
/// the way to the start of the file, and is deliberately *not* bounded by the
/// previous hunk: verified against `git diff-tree`, two hunks 20 lines apart
/// with only one identifier line above them both report that same name.
///
/// To avoid `O(N^2)` rescanning when a file has many hunks, `last_func_scan`
/// memoizes `(highest_scanned_idx, result_at_or_above_idx)` so each line index
/// in `old_lines` is evaluated against `matcher` at most once (`O(N)` total).
fn find_func_context(
    matcher: &FuncMatcher,
    old_lines: &[&[u8]],
    hunk_start_1based: u32,
    last_func_scan: &mut Option<(usize, Option<String>)>,
    cancel: &tigrs_core::cancel::CancellationToken,
) -> Option<String> {
    let first_idx = usize::try_from(hunk_start_1based).ok()?.checked_sub(1)?;
    let start_idx = first_idx.checked_sub(1)?;
    let mut idx = start_idx;
    let result = loop {
        if (start_idx - idx).is_multiple_of(1024) && cancel.is_cancelled() {
            return None;
        }
        if let Some((prev_scanned_idx, ref prev_result)) = *last_func_scan
            && idx <= prev_scanned_idx
        {
            break prev_result.clone();
        }
        if let Some(found) = old_lines.get(idx).and_then(|line| matcher.find(line)) {
            break Some(strip_control_chars(&found).into_owned());
        }
        let Some(next_idx) = idx.checked_sub(1) else {
            break None;
        };
        idx = next_idx;
    };
    *last_func_scan = Some((start_idx, result.clone()));
    result
}

/// Builds the single hunk representing an entire blob being added or removed.
///
/// Used for type changes, where Git renders the old object as a full deletion
/// and the new object as a full addition rather than diffing them against each
/// other. Returns the hunk list and the number of lines involved.
fn whole_file_hunk(buf: &[u8], is_add: bool) -> (Vec<DiffHunk>, usize) {
    let lines = split_lines(buf);
    if lines.is_empty() {
        return (Vec::new(), 0);
    }
    let no_newline = lacks_trailing_newline(buf);
    let count = lines.len();
    let last = count - 1;

    let hunk_lines: Vec<HunkLine> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| HunkLine {
            kind: if is_add {
                DiffLineKind::Add
            } else {
                DiffLineKind::Remove
            },
            content: String::from_utf8_lossy(line).to_string(),
            no_newline_at_eof: i == last && no_newline,
        })
        .collect();

    let len = u32::try_from(count).unwrap_or(u32::MAX);
    let hunk = if is_add {
        DiffHunk {
            old_start: 1,
            old_len: 0,
            new_start: 1,
            new_len: len,
            func_context: None,
            lines: hunk_lines,
        }
    } else {
        DiffHunk {
            old_start: 1,
            old_len: len,
            new_start: 1,
            new_len: 0,
            func_context: None,
            lines: hunk_lines,
        }
    };
    (vec![hunk], count)
}

/// Helper adapter to collect hunks from `gix::diff::blob::UnifiedDiff`.
struct HunkCollector<'a> {
    hunks: Vec<DiffHunk>,
    additions: usize,
    deletions: usize,
    /// Lines of the old blob, split on `\n` only so `\r` is preserved.
    old_lines: Vec<&'a [u8]>,
    /// Lines of the new blob, split on `\n` only so `\r` is preserved.
    new_lines: Vec<&'a [u8]>,
    /// Total line count of the old blob, for end-of-file detection.
    old_total_lines: u32,
    /// Total line count of the new blob, for end-of-file detection.
    new_total_lines: u32,
    /// Old blob is non-empty and lacks a trailing newline.
    old_no_newline: bool,
    /// New blob is non-empty and lacks a trailing newline.
    new_no_newline: bool,
    /// The funcname matcher this file's `diff` attribute selected.
    matcher: Rc<FuncMatcher>,
    /// Memoized `(highest_scanned_idx, cached_func_context)` for `O(N)` backward scanning.
    last_func_scan: Option<(usize, Option<String>)>,
    /// Cooperative cancellation token checked across hunks and backward scans.
    cancel: &'a tigrs_core::cancel::CancellationToken,
}

impl ConsumeHunk for HunkCollector<'_> {
    type Out = (Vec<DiffHunk>, usize, usize);

    fn consume_hunk(
        &mut self,
        header: HunkHeader,
        lines: &[(GixDiffLineKind, &[u8])],
    ) -> std::io::Result<()> {
        if self.cancel.is_cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "diff cancelled",
            ));
        }
        let mut hunk_lines = Vec::with_capacity(lines.len());

        // Track absolute line numbers for two reasons: to locate the final line
        // of each side (the only place Git prints the no-newline marker), and to
        // recover each line's original bytes from the source blob.
        let mut old_line = header.before_hunk_start;
        let mut new_line = header.after_hunk_start;

        for (kind, token_bytes) in lines {
            let hunk_kind = match kind {
                GixDiffLineKind::Context => DiffLineKind::Context,
                GixDiffLineKind::Add => DiffLineKind::Add,
                GixDiffLineKind::Remove => DiffLineKind::Remove,
            };

            // `gix`'s tokenizer strips a trailing `\r` along with the `\n`, but
            // Git treats a lone `\r` as ordinary line content and echoes it into
            // the patch. Reading the line back from the source blob preserves it.
            let source_line = |lines: &[&[u8]], line_no: u32| -> Option<String> {
                let idx = usize::try_from(line_no).ok()?.checked_sub(1)?;
                lines
                    .get(idx)
                    .map(|l| String::from_utf8_lossy(l).into_owned())
            };

            let (content, no_newline_at_eof) = match hunk_kind {
                DiffLineKind::Add => {
                    let is_last = new_line == self.new_total_lines;
                    let content = source_line(&self.new_lines, new_line);
                    new_line = new_line.saturating_add(1);
                    self.additions += 1;
                    (content, is_last && self.new_no_newline)
                }
                DiffLineKind::Remove => {
                    let is_last = old_line == self.old_total_lines;
                    let content = source_line(&self.old_lines, old_line);
                    old_line = old_line.saturating_add(1);
                    self.deletions += 1;
                    (content, is_last && self.old_no_newline)
                }
                DiffLineKind::Context => {
                    // A context line is the last line of both sides at once; the
                    // marker is emitted when either side lacks the newline.
                    let is_last_old = old_line == self.old_total_lines;
                    let is_last_new = new_line == self.new_total_lines;
                    let content = source_line(&self.old_lines, old_line);
                    old_line = old_line.saturating_add(1);
                    new_line = new_line.saturating_add(1);
                    (
                        content,
                        (is_last_old && self.old_no_newline)
                            || (is_last_new && self.new_no_newline),
                    )
                }
            };

            hunk_lines.push(HunkLine {
                kind: hunk_kind,
                content: content
                    .unwrap_or_else(|| String::from_utf8_lossy(token_bytes).to_string()),
                no_newline_at_eof,
            });
        }

        let func_context = find_func_context(
            &self.matcher,
            &self.old_lines,
            header.before_hunk_start,
            &mut self.last_func_scan,
            self.cancel,
        );

        self.hunks.push(DiffHunk {
            old_start: header.before_hunk_start,
            old_len: header.before_hunk_len,
            new_start: header.after_hunk_start,
            new_len: header.after_hunk_len,
            func_context,
            lines: hunk_lines,
        });
        Ok(())
    }

    fn finish(self) -> Self::Out {
        (self.hunks, self.additions, self.deletions)
    }
}

/// Extracts the first-line subject (`title`) and the complete remaining commit
/// message (`body`, preserving all body paragraphs and Git trailers) from the
/// raw commit message bytes.
///
/// Note: `gix::objs::commit::MessageRef::body()` returns a `BodyRef` whose
/// `Deref`/`as_bytes()` strips the trailing Git trailer paragraph (`Signed-off-by:`,
/// `Change-Id:`, `Reviewed-by:`, `Tested-by:`, etc.) and whose `title` spans up to
/// the first `\n\n` rather than the first `\n`. Parsing `message_raw()` directly
/// guarantees that the entire commit message is preserved verbatim.
#[must_use]
pub fn parse_commit_title_and_body(raw_message: &[u8]) -> (Arc<str>, Option<String>) {
    let raw_str = String::from_utf8_lossy(raw_message);
    let trimmed = raw_str.trim_start_matches(['\r', '\n']).trim_end();
    if trimmed.is_empty() {
        return (Arc::from(""), None);
    }

    let (first_line, remainder) = match trimmed.split_once('\n') {
        Some((first, rest)) => (first.trim_end_matches('\r').trim(), Some(rest)),
        None => (trimmed.trim(), None),
    };

    let title: Arc<str> = Arc::from(&*strip_control_chars(first_line));
    let body = remainder
        .map(|rest| {
            let after_sep = rest
                .strip_prefix("\r\n")
                .or_else(|| rest.strip_prefix('\n'))
                .unwrap_or(rest)
                .trim_end();
            strip_control_chars(after_sep).into_owned()
        })
        .filter(|b| !b.trim().is_empty());

    (title, body)
}

/// Computes the full commit diff for the given `commit_id`.
pub fn compute_commit_diff(repo: &gix::Repository, commit_id: ObjectId) -> Result<CommitDiff> {
    compute_commit_diff_cancellable(repo, commit_id, &tigrs_core::CancellationToken::none())
}

/// Cancellable variant of [`compute_commit_diff`].
pub fn compute_commit_diff_cancellable(
    repo: &gix::Repository,
    commit_id: ObjectId,
    cancel: &tigrs_core::CancellationToken,
) -> Result<CommitDiff> {
    cancel.check_cancelled()?;
    let commit = repo
        .find_commit(commit_id)
        .map_err(|e| TigError::Git(format!("Commit {commit_id} not found: {e}")))?;

    let raw_message = commit
        .message_raw()
        .map_err(|e| TigError::Git(format!("Failed to parse commit message: {e}")))?;
    let (title, body) = parse_commit_title_and_body(raw_message.as_ref());

    let author = commit
        .author()
        .map_err(|e| TigError::Git(format!("Failed to parse author: {e}")))?;
    let author_name: Arc<str> = Arc::from(&*strip_control_chars(&author.name.to_str_lossy()));
    let author_email: Arc<str> = Arc::from(&*strip_control_chars(&author.email.to_str_lossy()));
    let author_date = author.time().map_or_else(
        |_| String::new(),
        |t| t.format_or_unix(gix::date::time::format::DEFAULT),
    );

    let committer = commit
        .committer()
        .map_err(|e| TigError::Git(format!("Failed to parse committer: {e}")))?;
    let committer_name: Arc<str> = Arc::from(&*strip_control_chars(&committer.name.to_str_lossy()));
    let committer_email: Arc<str> =
        Arc::from(&*strip_control_chars(&committer.email.to_str_lossy()));
    let committer_date = committer.time().map_or_else(
        |_| String::new(),
        |t| t.format_or_unix(gix::date::time::format::DEFAULT),
    );

    let parent_ids: Vec<ObjectId> = commit.parent_ids().map(gix::Id::detach).collect();

    // In Git / tig, diff is computed against the first parent, or empty tree for root commits.
    let old_tree = match parent_ids.first() {
        Some(first_parent) => repo
            .find_commit(*first_parent)
            .map_err(|e| TigError::Git(format!("Parent commit {first_parent} not found: {e}")))?
            .tree()
            .map_err(|e| TigError::Git(format!("Failed to read parent tree: {e}")))?,
        None => repo.empty_tree(),
    };
    let new_tree = commit
        .tree()
        .map_err(|e| TigError::Git(format!("Failed to read commit tree: {e}")))?;

    let empty_index = gix::index::State::new(repo.object_hash());
    let attr_stack = if repo.workdir().is_some() {
        repo.attributes_only(
            &empty_index,
            gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
        )
    } else {
        let index = repo
            .index_or_empty()
            .map_err(|e| TigError::Git(format!("Failed to read index: {e}")))?;
        repo.attributes_only(
            &index,
            gix::worktree::stack::state::attributes::Source::IdMapping,
        )
    }
    .map_err(|e| TigError::Git(format!("Failed to build attribute stack: {e}")))?;
    let mut cache = gix::diff::resource_cache(
        repo,
        gix::diff::blob::pipeline::Mode::ToGit,
        attr_stack.detach(),
        gix::diff::blob::pipeline::WorktreeRoots::default(),
    )
    .map_err(|e| TigError::Git(format!("Failed to create diff resource cache: {e}")))?;

    let mut old_changes = old_tree
        .changes()
        .map_err(|e| TigError::Git(format!("Failed to initialize tree diff: {e}")))?;

    // Built once: it holds the gitattributes stack and a cache of compiled
    // driver regexes, both far too expensive to rebuild per file.
    let mut drivers = DriverResolver::new(repo);

    let mut files = Vec::new();
    let mut total_insertions = 0;
    let mut total_deletions = 0;
    let mut cancelled = false;

    old_changes
        .for_each_to_obtain_tree(&new_tree, |change| {
            if cancel.is_cancelled() {
                cancelled = true;
                return Ok::<_, std::convert::Infallible>(std::ops::ControlFlow::Break(()));
            }
            // Ignore directory tree entries; process blobs, symlinks, and submodule commits (0o160000).
            if change.entry_mode().is_tree() {
                return Ok::<_, std::convert::Infallible>(std::ops::ControlFlow::Continue(()));
            }

            let path = strip_control_chars(&change.location().to_str_lossy()).into_owned();
            let (status, old_id, new_id, old_mode, new_mode) = match change {
                gix::object::tree::diff::Change::Addition { entry_mode, id, .. } => (
                    FileChangeStatus::Added,
                    None,
                    Some(id.detach()),
                    None,
                    Some(u32::from(entry_mode.value())),
                ),
                gix::object::tree::diff::Change::Deletion { entry_mode, id, .. } => (
                    FileChangeStatus::Deleted,
                    Some(id.detach()),
                    None,
                    Some(u32::from(entry_mode.value())),
                    None,
                ),
                gix::object::tree::diff::Change::Modification {
                    previous_entry_mode,
                    previous_id,
                    entry_mode,
                    id,
                    ..
                } => {
                    let st = if previous_entry_mode == entry_mode {
                        FileChangeStatus::Modified
                    } else {
                        FileChangeStatus::TypeChanged
                    };
                    (
                        st,
                        Some(previous_id.detach()),
                        Some(id.detach()),
                        Some(u32::from(previous_entry_mode.value())),
                        Some(u32::from(entry_mode.value())),
                    )
                }
                gix::object::tree::diff::Change::Rewrite {
                    source_location,
                    source_entry_mode,
                    source_id,
                    entry_mode,
                    id,
                    copy,
                    diff,
                    ..
                } => {
                    let pct = diff.map_or(100, |d| {
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        {
                            (d.similarity * 100.0) as u32
                        }
                    });
                    let clean_source_path =
                        strip_control_chars(&source_location.to_str_lossy()).into_owned();
                    let st = if copy {
                        FileChangeStatus::Copied {
                            source_path: clean_source_path,
                            similarity_pct: pct,
                        }
                    } else {
                        FileChangeStatus::Renamed {
                            source_path: clean_source_path,
                            similarity_pct: pct,
                        }
                    };
                    (
                        st,
                        Some(source_id.detach()),
                        Some(id.detach()),
                        Some(u32::from(source_entry_mode.value())),
                        Some(u32::from(entry_mode.value())),
                    )
                }
            };

            let mut hunks = Vec::new();
            let mut additions = 0;
            let mut deletions = 0;
            let mut is_binary = false;
            // Populated only when the entry type changed, so the delete/add
            // pair can be synthesized from the original blob contents.
            let mut type_change_buffers: Option<(Vec<u8>, Vec<u8>)> = None;

            let old_is_sub = old_mode == Some(0o160_000);
            let new_is_sub = new_mode == Some(0o160_000);
            if old_is_sub || new_is_sub {
                let mut sub_lines = Vec::new();
                if let (true, Some(oid)) = (old_is_sub, old_id) {
                    sub_lines.push(HunkLine {
                        kind: DiffLineKind::Remove,
                        content: format!("Subproject commit {oid}"),
                        no_newline_at_eof: false,
                    });
                    deletions += 1;
                }
                if let (true, Some(oid)) = (new_is_sub, new_id) {
                    sub_lines.push(HunkLine {
                        kind: DiffLineKind::Add,
                        content: format!("Subproject commit {oid}"),
                        no_newline_at_eof: false,
                    });
                    additions += 1;
                }
                hunks.push(DiffHunk {
                    old_start: u32::from(old_is_sub),
                    old_len: u32::from(old_is_sub),
                    new_start: u32::from(new_is_sub),
                    new_len: u32::from(new_is_sub),
                    func_context: None,
                    lines: sub_lines,
                });
            } else if let Ok(diff_platform) = change.diff(&mut cache) {
                diff_platform
                    .resource_cache
                    .options
                    .skip_internal_diff_if_external_is_configured = false;
                if let Ok(prep) = diff_platform.resource_cache.prepare_diff() {
                    match prep.operation {
                        Operation::InternalDiff { algorithm } => {
                            // Git only prints `\ No newline at end of file` for
                            // the final line of a side, so the collector needs
                            // both the line count and the terminator state of
                            // each blob to place the marker correctly.
                            let old_buf = prep.old.data.as_slice().unwrap_or_default();
                            let new_buf = prep.new.data.as_slice().unwrap_or_default();

                            if old_buf.len() > MAX_DIFF_BLOB_BYTES
                                || new_buf.len() > MAX_DIFF_BLOB_BYTES
                            {
                                is_binary = true;
                            } else {
                                // A symlink/file swap needs both blobs verbatim so
                                // the delete/add pair can be rebuilt below.
                                if matches!((old_mode, new_mode), (Some(o), Some(n))
                                    if (o & 0o170_000) != (n & 0o170_000))
                                {
                                    type_change_buffers =
                                        Some((old_buf.to_vec(), new_buf.to_vec()));
                                }

                                let is_gen = drivers.is_linguist_generated(&path);
                                crate::userdiff::mark_path_linguist_generated(&path, is_gen);

                                if old_buf.is_empty() {
                                    let (h, a) = whole_file_hunk(new_buf, true);
                                    hunks = h;
                                    additions = a;
                                } else if new_buf.is_empty() {
                                    let (h, d) = whole_file_hunk(old_buf, false);
                                    hunks = h;
                                    deletions = d;
                                } else {
                                    let old_lines = split_lines(old_buf);
                                    let new_lines = split_lines(new_buf);

                                    // Tokenize here rather than using `prep.interned_input()`:
                                    // `gix`'s line tokenizer strips a trailing `\r` along with
                                    // the `\n`, so `foo\r\n` and `foo\n` intern to the *same*
                                    // token and a pure LF->CRLF conversion produces no hunks at
                                    // all. Git treats `\r` as ordinary content, so the lines
                                    // must differ. Splitting on `\n` only reproduces that.
                                    let mut input: InternedInput<&[u8]> = InternedInput::default();
                                    input.update_before(old_lines.iter().copied());
                                    input.update_after(new_lines.iter().copied());

                                    let diff = gix::diff::blob::diff_with_slider_heuristics(
                                        algorithm, &input,
                                    );
                                    let collector = HunkCollector {
                                        hunks: Vec::new(),
                                        additions: 0,
                                        deletions: 0,
                                        old_total_lines: count_lines(old_buf),
                                        new_total_lines: count_lines(new_buf),
                                        old_no_newline: lacks_trailing_newline(old_buf),
                                        new_no_newline: lacks_trailing_newline(new_buf),
                                        old_lines,
                                        new_lines,
                                        matcher: drivers.matcher_for(&path),
                                        last_func_scan: None,
                                        cancel,
                                    };
                                    let unified = gix::diff::blob::UnifiedDiff::new(
                                        &diff,
                                        &input,
                                        collector,
                                        ContextSize::symmetrical(3),
                                    );

                                    if let Ok((h, a, d)) = unified.consume() {
                                        hunks = h;
                                        additions = a;
                                        deletions = d;
                                    }
                                }
                            }
                        }
                        Operation::SourceOrDestinationIsBinary => {
                            is_binary = true;
                        }
                        Operation::ExternalCommand { .. } => {}
                    }
                }
            }

            total_insertions += additions;
            total_deletions += deletions;

            // Git never renders a symlink/regular-file swap as a single
            // modification. It emits a deletion of the old object followed by
            // an addition of the new one, because the two blobs are not
            // meaningfully comparable line-by-line.
            let is_type_change = match (old_mode, new_mode) {
                (Some(o), Some(n)) => (o & 0o170_000) != (n & 0o170_000),
                _ => false,
            };

            if is_type_change && let Some((old_content, new_content)) = type_change_buffers.take() {
                let (del_hunks, del_count) = whole_file_hunk(&old_content, false);
                let (add_hunks, add_count) = whole_file_hunk(&new_content, true);

                // Replace the single modification's counts with the
                // delete-plus-add totals the split representation implies.
                total_insertions = total_insertions - additions + add_count;
                total_deletions = total_deletions - deletions + del_count;

                files.push(FileDiff {
                    path: path.clone(),
                    status: FileChangeStatus::Deleted,
                    old_id,
                    new_id: None,
                    old_mode,
                    new_mode: None,
                    is_binary,
                    additions: 0,
                    deletions: del_count,
                    hunks: del_hunks,
                });
                files.push(FileDiff {
                    path,
                    status: FileChangeStatus::Added,
                    old_id: None,
                    new_id,
                    old_mode: None,
                    new_mode,
                    is_binary,
                    additions: add_count,
                    deletions: 0,
                    hunks: add_hunks,
                });

                cache.clear_resource_cache_keep_allocation();
                return Ok::<_, std::convert::Infallible>(std::ops::ControlFlow::Continue(()));
            }

            files.push(FileDiff {
                path,
                status,
                old_id,
                new_id,
                old_mode,
                new_mode,
                is_binary,
                additions,
                deletions,
                hunks,
            });

            cache.clear_resource_cache_keep_allocation();
            Ok::<_, std::convert::Infallible>(std::ops::ControlFlow::Continue(()))
        })
        .map_err(|e| TigError::Git(format!("Diff traversal failed: {e}")))?;

    if cancelled {
        return Err(TigError::Cancelled);
    }

    // Git emits patch entries in path order. Its tree walk recurses in place, so
    // a byte-wise compare of full paths reproduces it exactly: a directory's
    // children are reached through the `/` separator, which sorts below every
    // character that could follow the directory name in a sibling's name
    // (`sub/inner.txt` precedes `sub_file.txt`). `gix` instead yields all
    // entries of a tree level before descending, so the order must be restored.
    //
    // Renames sort under their *destination* path — verified against `git
    // diff-tree -M`, where a `zzz.txt -> aaa.txt` rename is printed before an
    // unrelated `mmm.txt` modification. The sort is stable, so the
    // deletion/addition pair synthesized for a type change keeps its order.
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let stats = DiffSummaryStats {
        files_changed: files.len(),
        insertions: total_insertions,
        deletions: total_deletions,
    };

    Ok(CommitDiff {
        commit_id,
        parent_ids,
        author_name,
        author_email,
        author_date,
        committer_name,
        committer_email,
        committer_date,
        title,
        body,
        files,
        stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use std::process::Command;
    use tempfile::TempDir;

    fn create_test_repo() -> (TempDir, gix::ObjectId, gix::ObjectId) {
        let dir = TempDir::new().unwrap();
        let path = dir.path();

        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init", "-b", "main"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["config", "user.name", "Diff Author"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["config", "user.email", "author@example.com"])
            .current_dir(path)
            .output()
            .unwrap();

        // Commit 1: Initial commit with two files
        let file1 = path.join("file1.txt");
        let mut f1 = File::create(&file1).unwrap();
        writeln!(f1, "Hello world\nLine 2\nLine 3").unwrap();

        let file2 = path.join("file2.txt");
        let mut f2 = File::create(&file2).unwrap();
        writeln!(f2, "Constant content").unwrap();

        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "commit",
                "-m",
                "Initial commit\n\nDetailed explanation of commit.",
            ])
            .current_dir(path)
            .output()
            .unwrap();

        let rev1 = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap();
        let hex1 = String::from_utf8_lossy(&rev1.stdout).trim().to_string();
        let c1 = gix::ObjectId::from_hex(hex1.as_bytes()).unwrap();

        // Commit 2: Modify file1, delete file2, add file3
        let mut f1_mod = File::create(&file1).unwrap();
        writeln!(f1_mod, "Hello world modified\nLine 2\nLine 3\nLine 4 added").unwrap();

        fs::remove_file(&file2).unwrap();

        let file3 = path.join("file3.txt");
        let mut f3 = File::create(&file3).unwrap();
        writeln!(f3, "Brand new file").unwrap();

        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["add", "-A"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "Second commit"])
            .current_dir(path)
            .output()
            .unwrap();

        let rev2 = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap();
        let hex2 = String::from_utf8_lossy(&rev2.stdout).trim().to_string();
        let c2 = gix::ObjectId::from_hex(hex2.as_bytes()).unwrap();

        (dir, c1, c2)
    }

    #[test]
    fn test_compute_diff_initial_root_commit() {
        let (dir, c1, _c2) = create_test_repo();
        let repo = gix::open(dir.path()).unwrap();

        let diff = compute_commit_diff(&repo, c1).expect("diff c1");
        assert_eq!(diff.commit_id, c1);
        assert!(diff.parent_ids.is_empty());
        assert_eq!(&*diff.author_name, "Diff Author");
        assert_eq!(&*diff.author_email, "author@example.com");
        assert_eq!(&*diff.title, "Initial commit");
        assert_eq!(
            diff.body.as_deref(),
            Some("Detailed explanation of commit.")
        );
        assert_eq!(diff.stats.files_changed, 2);
        assert!(diff.stats.insertions > 0);
        assert_eq!(diff.stats.deletions, 0);

        for file in &diff.files {
            assert_eq!(file.status, FileChangeStatus::Added);
            assert!(!file.hunks.is_empty());
        }
    }

    #[test]
    fn test_compute_diff_modifications_additions_deletions() {
        let (dir, c1, c2) = create_test_repo();
        let repo = gix::open(dir.path()).unwrap();

        let diff = compute_commit_diff(&repo, c2).expect("diff c2");
        assert_eq!(diff.commit_id, c2);
        assert_eq!(diff.parent_ids, vec![c1]);
        assert_eq!(&*diff.title, "Second commit");
        assert_eq!(diff.stats.files_changed, 3);

        let mut modified = false;
        let mut added = false;
        let mut deleted = false;

        for file in &diff.files {
            match file.status {
                FileChangeStatus::Modified => {
                    assert_eq!(file.path, "file1.txt");
                    assert!(file.additions > 0);
                    assert!(file.deletions > 0);
                    modified = true;
                }
                FileChangeStatus::Added => {
                    assert_eq!(file.path, "file3.txt");
                    assert!(file.additions > 0);
                    assert_eq!(file.deletions, 0);
                    added = true;
                }
                FileChangeStatus::Deleted => {
                    assert_eq!(file.path, "file2.txt");
                    assert_eq!(file.additions, 0);
                    assert!(file.deletions > 0);
                    deleted = true;
                }
                _ => {}
            }
        }

        assert!(modified, "file1.txt should be modified");
        assert!(added, "file3.txt should be added");
        assert!(deleted, "file2.txt should be deleted");
    }

    #[test]
    fn test_compute_diff_rename() {
        let dir = TempDir::new().unwrap();
        let path = dir.path();

        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init", "-b", "main"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["config", "user.name", "Diff Author"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["config", "user.email", "author@example.com"])
            .current_dir(path)
            .output()
            .unwrap();

        let original = path.join("original.txt");
        fs::write(
            &original,
            "Significant content that stays mostly the same\nLine 2\nLine 3\nLine 4\n",
        )
        .unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "First"])
            .current_dir(path)
            .output()
            .unwrap();

        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["mv", "original.txt", "renamed.txt"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "Rename"])
            .current_dir(path)
            .output()
            .unwrap();

        let rev = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap();
        let hex = String::from_utf8_lossy(&rev.stdout).trim().to_string();
        let c = gix::ObjectId::from_hex(hex.as_bytes()).unwrap();

        let repo = gix::open(path).unwrap();
        let diff = compute_commit_diff(&repo, c).expect("diff rename");
        assert_eq!(diff.files.len(), 1);
        let f = &diff.files[0];
        assert_eq!(f.path, "renamed.txt");
        match &f.status {
            FileChangeStatus::Renamed {
                source_path,
                similarity_pct,
            } => {
                assert_eq!(source_path, "original.txt");
                assert!(*similarity_pct >= 90);
            }
            other => panic!("Expected Renamed status, got {other:?}"),
        }
    }

    #[test]
    fn test_compute_diff_binary_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path();

        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init", "-b", "main"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["config", "user.name", "Diff Author"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["config", "user.email", "author@example.com"])
            .current_dir(path)
            .output()
            .unwrap();

        let bin_file = path.join("image.bin");
        fs::write(&bin_file, b"GIF89a\x00\x00\x01\x00\x80\x00\x00\x00").unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "Add binary"])
            .current_dir(path)
            .output()
            .unwrap();

        let rev = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap();
        let hex = String::from_utf8_lossy(&rev.stdout).trim().to_string();
        let c = gix::ObjectId::from_hex(hex.as_bytes()).unwrap();

        let repo = gix::open(path).unwrap();
        let diff = compute_commit_diff(&repo, c).expect("diff binary");
        assert_eq!(diff.files.len(), 1);
        assert!(
            diff.files[0].is_binary,
            "GIF/binary content must be detected as binary"
        );
        assert!(
            diff.files[0].hunks.is_empty(),
            "Binary files must not have text hunks"
        );
    }

    #[test]
    fn test_complete_commit_message_preserves_trailers() {
        let sample = b"net: socket: make receive ring buffer capacity configurable\n\n\
Previously the ring buffer used a fixed array of 4096 bytes,\n\
which could overflow under high packet burst loads.\n\n\
SKIP_CI: Flaky integration suite on arm64 runner\n\
Manual verification complete\n\
Tested: on local testbed https://ci.example.com/runs/10492\n\
with benchmark profile https://git.example.com/perf/profiles/42\n\
Issue-Id: 100200\n\
Issue-Id: 100201\n\
Change-Id: I1234567890abcdef1234567890abcdef12345678\n\
Signed-off-by: Alice Developer <alice@example.com>\n";

        let (title, body) = parse_commit_title_and_body(sample);
        assert_eq!(
            &*title,
            "net: socket: make receive ring buffer capacity configurable"
        );
        let body = body.expect("body must be present");
        assert!(body.contains("Previously the ring buffer used a fixed array of 4096 bytes"));
        assert!(body.contains("SKIP_CI: Flaky integration suite on arm64 runner"));
        assert!(body.contains("Manual verification complete"));
        assert!(body.contains("Tested: on local testbed"));
        assert!(body.contains("Issue-Id: 100200"));
        assert!(body.contains("Change-Id: I1234567890abcdef1234567890abcdef12345678"));
        assert!(body.contains("Signed-off-by: Alice Developer <alice@example.com>"));
    }
}
