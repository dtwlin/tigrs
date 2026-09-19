// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Git-compatible unified patch text generation.
//!
//! This module renders a [`CommitDiff`] into the exact byte sequence that
//! `git diff-tree -p` produces. It lives in `tigrs-git` rather than the UI
//! layer for two reasons: patch syntax is a Git concern, not a presentation
//! concern, and keeping it here lets the differential test harness compare
//! against upstream Git without going through a terminal.
//!
//! The rules encoded below are subtle and were each established by diffing
//! against real Git output rather than from the documentation:
//!
//! - The `index` line carries a mode suffix only when the mode is unchanged.
//!   For additions and deletions the mode appears on its own `new file mode` /
//!   `deleted file mode` line instead, and a pure mode change uses
//!   `old mode` / `new mode` lines.
//! - A hunk range omits the `,count` suffix when the count is exactly 1.
//! - An empty side is rendered as `-0,0` / `+0,0`.
//! - `\ No newline at end of file` follows the final line of a side whose blob
//!   does not end in a newline.

use crate::diff::{CommitDiff, DiffLineKind, FileChangeStatus, FileDiff};
use std::fmt::Write as _;

/// Controls how object IDs are abbreviated in `index` lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexAbbrev {
    /// Emit the complete 40- (or 64-) character hex ID, matching
    /// `git diff --full-index`. Used by the differential harness because it is
    /// deterministic and free of Git's repo-size-dependent abbreviation logic.
    Full,
    /// Truncate to `n` hex characters for compact display.
    Chars(usize),
}

/// Renders the unified patch body of a commit diff exactly as Git would.
///
/// The output covers only the patch itself, starting at the first
/// `diff --git` line. Commit metadata headers are the caller's concern.
pub fn format_patch(diff: &CommitDiff, abbrev: IndexAbbrev) -> String {
    let mut out = String::new();
    for file in &diff.files {
        format_file_patch(&mut out, file, abbrev);
    }
    out
}

/// Renders the patch for a single file into `out`.
fn format_file_patch(out: &mut String, file: &FileDiff, abbrev: IndexAbbrev) {
    let (old_path, new_path) = patch_paths(file);

    // Git quotes paths containing unusual bytes using C-style escaping.
    let _ = writeln!(
        out,
        "diff --git {} {}",
        quote_path(&format!("a/{old_path}")),
        quote_path(&format!("b/{new_path}"))
    );

    let mode_changed = matches!((file.old_mode, file.new_mode), (Some(o), Some(n)) if o != n);

    match &file.status {
        FileChangeStatus::Added => {
            if let Some(mode) = file.new_mode {
                let _ = writeln!(out, "new file mode {mode:06o}");
            }
        }
        FileChangeStatus::Deleted => {
            if let Some(mode) = file.old_mode {
                let _ = writeln!(out, "deleted file mode {mode:06o}");
            }
        }
        FileChangeStatus::Renamed {
            source_path,
            similarity_pct,
        } => {
            emit_mode_change(out, file, mode_changed);
            let _ = writeln!(out, "similarity index {similarity_pct}%");
            let _ = writeln!(out, "rename from {}", quote_path(source_path));
            let _ = writeln!(out, "rename to {}", quote_path(&file.path));
        }
        FileChangeStatus::Copied {
            source_path,
            similarity_pct,
        } => {
            emit_mode_change(out, file, mode_changed);
            let _ = writeln!(out, "similarity index {similarity_pct}%");
            let _ = writeln!(out, "copy from {}", quote_path(source_path));
            let _ = writeln!(out, "copy to {}", quote_path(&file.path));
        }
        FileChangeStatus::Modified | FileChangeStatus::TypeChanged => {
            emit_mode_change(out, file, mode_changed);
        }
    }

    // A pure mode change with identical content produces no index line at all.
    let content_changed = file.old_id != file.new_id;
    if content_changed {
        emit_index_line(out, file, abbrev, mode_changed);
    }

    if file.is_binary {
        // `Binary files ... differ` never uses tab disambiguation.
        let _ = writeln!(
            out,
            "Binary files {} and {} differ",
            side('a', &old_path, file.old_id.is_some(), false),
            side('b', &new_path, file.new_id.is_some(), false)
        );
        return;
    }

    if file.hunks.is_empty() {
        return;
    }

    let _ = writeln!(
        out,
        "--- {}",
        side('a', &old_path, file.old_id.is_some(), true)
    );
    let _ = writeln!(
        out,
        "+++ {}",
        side('b', &new_path, file.new_id.is_some(), true)
    );

    for hunk in &file.hunks {
        match &hunk.func_context {
            Some(ctx) if !ctx.is_empty() => {
                let _ = writeln!(
                    out,
                    "@@ -{} +{} @@ {ctx}",
                    range(hunk.old_start, hunk.old_len),
                    range(hunk.new_start, hunk.new_len)
                );
            }
            _ => {
                let _ = writeln!(
                    out,
                    "@@ -{} +{} @@",
                    range(hunk.old_start, hunk.old_len),
                    range(hunk.new_start, hunk.new_len)
                );
            }
        }
        for line in &hunk.lines {
            let prefix = match line.kind {
                DiffLineKind::Context => ' ',
                DiffLineKind::Add => '+',
                DiffLineKind::Remove => '-',
            };
            let _ = writeln!(out, "{prefix}{}", line.content);
            if line.no_newline_at_eof {
                let _ = writeln!(out, "\\ No newline at end of file");
            }
        }
    }
}

/// Emits `old mode` / `new mode` lines when a mode changed in place.
fn emit_mode_change(out: &mut String, file: &FileDiff, mode_changed: bool) {
    if !mode_changed {
        return;
    }
    if let (Some(old), Some(new)) = (file.old_mode, file.new_mode) {
        let _ = writeln!(out, "old mode {old:06o}");
        let _ = writeln!(out, "new mode {new:06o}");
    }
}

/// Emits the `index <old>..<new>[ mode]` line.
fn emit_index_line(out: &mut String, file: &FileDiff, abbrev: IndexAbbrev, mode_changed: bool) {
    let hex_len = file
        .old_id
        .or(file.new_id)
        .map_or(40, |id| id.to_hex().to_string().len());
    let null = "0".repeat(hex_len);

    let fmt_id = |id: Option<gix::ObjectId>| -> String {
        let full = id.map_or_else(|| null.clone(), |i| i.to_hex().to_string());
        match abbrev {
            IndexAbbrev::Full => full,
            IndexAbbrev::Chars(n) => full.chars().take(n.min(full.len())).collect(),
        }
    };

    // Git appends the mode only when both sides share it. Additions, deletions
    // and mode changes carry the mode on a dedicated line instead.
    let suffix = if mode_changed {
        String::new()
    } else {
        match (file.old_mode, file.new_mode) {
            (Some(o), Some(n)) if o == n => format!(" {n:06o}"),
            _ => String::new(),
        }
    };

    let _ = writeln!(
        out,
        "index {}..{}{}",
        fmt_id(file.old_id),
        fmt_id(file.new_id),
        suffix
    );
}

/// Resolves the `a/` and `b/` path pair, accounting for renames and copies.
fn patch_paths(file: &FileDiff) -> (String, String) {
    match &file.status {
        FileChangeStatus::Renamed { source_path, .. }
        | FileChangeStatus::Copied { source_path, .. } => (source_path.clone(), file.path.clone()),
        _ => (file.path.clone(), file.path.clone()),
    }
}

/// Formats one side of a `---` / `+++` / `Binary files` line.
///
/// Git substitutes `/dev/null` when the side has no content, and otherwise
/// prefixes the path with `a/` (old) or `b/` (new).
///
/// When the rendered path contains a space but did not require quoting, Git
/// appends a literal tab so that patch parsers can find the end of the
/// filename. Quoted paths already have unambiguous delimiters and get no tab.
/// This applies to `---` / `+++` only, never to `diff --git`.
pub(crate) fn side(prefix: char, path: &str, present: bool, tab_disambiguate: bool) -> String {
    if !present {
        return "/dev/null".to_string();
    }
    let raw = format!("{prefix}/{path}");
    let quoted = quote_path(&raw);
    let was_quoted = quoted.starts_with('"');
    if tab_disambiguate && !was_quoted && raw.contains(' ') {
        format!("{quoted}\t")
    } else {
        quoted
    }
}

/// Formats a hunk range.
///
/// Two Git conventions are encoded here. A unit count is omitted entirely
/// (`@@ -1 +1 @@`), and an empty range is anchored to the line *before* the
/// insertion point, so a file created from nothing reads `-0,0` rather than
/// `-1,0`. `gix` reports the insertion point itself, hence the decrement.
pub(crate) fn range(start: u32, len: u32) -> String {
    match len {
        0 => format!("{},0", start.saturating_sub(1)),
        1 => format!("{start}"),
        _ => format!("{start},{len}"),
    }
}

/// Applies Git's C-style quoting to paths containing control or non-ASCII bytes.
///
/// Git leaves ordinary paths untouched and wraps anything requiring escapes in
/// double quotes, so the quoting decision must be made on the whole path.
pub(crate) fn quote_path(path: &str) -> String {
    quote_path_bytes(path.as_bytes())
}

/// Applies Git's C-style quoting to raw path bytes.
pub(crate) fn quote_path_bytes(path: &[u8]) -> String {
    let needs_quote = path
        .iter()
        .any(|&b| b < 0x20 || b == b'"' || b == b'\\' || b >= 0x7f);
    if !needs_quote && let Ok(s) = std::str::from_utf8(path) {
        return s.to_string();
    }

    let mut out = String::with_capacity(path.len() + 2);
    out.push('"');
    for &b in path {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            0x20..=0x7e => out.push(b as char),
            other => {
                let _ = write!(out, "\\{other:03o}");
            }
        }
    }
    out.push('"');
    out
}

/// Formats one side of a `---` / `+++` header from raw path bytes.
pub(crate) fn side_bytes(prefix: u8, path: &[u8], present: bool, tab_disambiguate: bool) -> String {
    if !present {
        return "/dev/null".to_string();
    }
    let mut raw = Vec::with_capacity(path.len() + 2);
    raw.push(prefix);
    raw.push(b'/');
    raw.extend_from_slice(path);
    let quoted = quote_path_bytes(&raw);
    let was_quoted = quoted.starts_with('"');
    if tab_disambiguate && !was_quoted && raw.contains(&b' ') {
        format!("{quoted}\t")
    } else {
        quoted
    }
}

/// Unquotes a Git diff header path (handling C-style quotes/octal escapes and unquoted tab suffixes)
/// and strips `a/` or `b/` prefix if `strip_ab_prefix` is true.
pub(crate) fn unquote_c_path_bytes(input: &[u8], strip_ab_prefix: bool) -> Vec<u8> {
    let trimmed = input.strip_suffix(b"\r").unwrap_or(input);
    let mut out = if trimmed.len() >= 2 && trimmed.starts_with(b"\"") && trimmed.ends_with(b"\"") {
        let inner = &trimmed[1..trimmed.len() - 1];
        let mut buf = Vec::with_capacity(inner.len());
        let mut i = 0;
        while i < inner.len() {
            if inner[i] == b'\\' && i + 1 < inner.len() {
                i += 1;
                match inner[i] {
                    b'\\' => buf.push(b'\\'),
                    b'"' => buf.push(b'"'),
                    b'n' => buf.push(b'\n'),
                    b't' => buf.push(b'\t'),
                    b'r' => buf.push(b'\r'),
                    b'a' => buf.push(0x07),
                    b'b' => buf.push(0x08),
                    b'f' => buf.push(0x0c),
                    b'v' => buf.push(0x0b),
                    d @ b'0'..=b'7' => {
                        let mut val = u16::from(d - b'0');
                        for _ in 0..2 {
                            if i + 1 < inner.len() && (b'0'..=b'7').contains(&inner[i + 1]) {
                                i += 1;
                                val = (val * 8) + u16::from(inner[i] - b'0');
                            } else {
                                break;
                            }
                        }
                        buf.push((val & 0xff) as u8);
                    }
                    other => buf.push(other),
                }
            } else {
                buf.push(inner[i]);
            }
            i += 1;
        }
        buf
    } else {
        let without_tab = trimmed.split(|&b| b == b'\t').next().unwrap_or(trimmed);
        without_tab.to_vec()
    };

    if strip_ab_prefix && out.len() >= 2 && (out.starts_with(b"a/") || out.starts_with(b"b/")) {
        out.drain(..2);
    }
    out
}

type RawHunkLineKey = (u32, u32, u32, u32, usize, String);
type RawHunkLineQueue = std::collections::VecDeque<(RawHunkLineKey, Vec<u8>)>;

fn raw_hunk_line_store() -> &'static std::sync::Mutex<RawHunkLineQueue> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<RawHunkLineQueue>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(std::collections::VecDeque::with_capacity(256)))
}

/// Records the raw byte slice of a parsed worktree diff line when it differs from its
/// sanitized UI display string (e.g., lines containing form-feed `0x0c` or non-UTF-8 bytes).
pub(crate) fn record_raw_hunk_line(
    old_start: u32,
    old_len: u32,
    new_start: u32,
    new_len: u32,
    line_idx: usize,
    sanitized_content: &str,
    raw_bytes: &[u8],
) {
    if sanitized_content.as_bytes() == raw_bytes {
        return;
    }
    let key = (
        old_start,
        old_len,
        new_start,
        new_len,
        line_idx,
        sanitized_content.to_string(),
    );
    if let Ok(mut guard) = raw_hunk_line_store().lock() {
        guard.retain(|(k, _)| k != &key);
        if guard.len() >= 4096 {
            guard.pop_front();
        }
        guard.push_back((key, raw_bytes.to_vec()));
    }
}

/// Resolves the original raw bytes for a hunk line if recorded, or falls back to `sanitized_content.as_bytes()`.
pub(crate) fn resolve_raw_hunk_line(
    old_start: u32,
    old_len: u32,
    new_start: u32,
    new_len: u32,
    line_idx: usize,
    sanitized_content: &str,
) -> std::borrow::Cow<'_, [u8]> {
    if let Ok(guard) = raw_hunk_line_store().lock() {
        for (k, v) in guard.iter().rev() {
            if k.0 == old_start
                && k.1 == old_len
                && k.2 == new_start
                && k.3 == new_len
                && k.4 == line_idx
                && k.5 == sanitized_content
            {
                return std::borrow::Cow::Owned(v.clone());
            }
        }
    }
    std::borrow::Cow::Borrowed(sanitized_content.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{DiffHunk, DiffSummaryStats, HunkLine};
    use gix::ObjectId;
    use std::sync::Arc;

    fn oid(byte: u8) -> ObjectId {
        ObjectId::from_bytes_or_panic(&[byte; 20])
    }

    fn base_diff(files: Vec<FileDiff>) -> CommitDiff {
        CommitDiff {
            commit_id: oid(1),
            parent_ids: vec![],
            author_name: Arc::from("A"),
            author_email: Arc::from("a@example.com"),
            author_date: String::new(),
            committer_name: Arc::from("A"),
            committer_email: Arc::from("a@example.com"),
            committer_date: String::new(),
            title: Arc::from("t"),
            body: None,
            files,
            stats: DiffSummaryStats::default(),
        }
    }

    #[test]
    fn test_hunk_range_omits_unit_count() {
        assert_eq!(range(5, 1), "5");
        assert_eq!(range(5, 3), "5,3");
        assert_eq!(range(0, 0), "0,0");
    }

    #[test]
    fn test_index_line_carries_mode_only_when_unchanged() {
        let modified = FileDiff {
            path: "f".to_string(),
            status: FileChangeStatus::Modified,
            old_id: Some(oid(1)),
            new_id: Some(oid(2)),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_644),
            is_binary: false,
            additions: 0,
            deletions: 0,
            hunks: vec![],
        };
        let out = format_patch(&base_diff(vec![modified]), IndexAbbrev::Chars(7));
        assert!(out.contains("index 0101010..0202020 100644"), "{out}");

        let mode_change = FileDiff {
            path: "f".to_string(),
            status: FileChangeStatus::Modified,
            old_id: Some(oid(1)),
            new_id: Some(oid(2)),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_755),
            is_binary: false,
            additions: 0,
            deletions: 0,
            hunks: vec![],
        };
        let out = format_patch(&base_diff(vec![mode_change]), IndexAbbrev::Chars(7));
        assert!(out.contains("old mode 100644"), "{out}");
        assert!(out.contains("new mode 100755"), "{out}");
        assert!(
            !out.contains("index 0101010..0202020 100644"),
            "mode must not repeat on index line: {out}"
        );
    }

    #[test]
    fn test_added_file_has_no_mode_on_index_line() {
        let added = FileDiff {
            path: "f".to_string(),
            status: FileChangeStatus::Added,
            old_id: None,
            new_id: Some(oid(2)),
            old_mode: None,
            new_mode: Some(0o100_644),
            is_binary: false,
            additions: 0,
            deletions: 0,
            hunks: vec![],
        };
        let out = format_patch(&base_diff(vec![added]), IndexAbbrev::Chars(7));
        assert!(out.contains("new file mode 100644"), "{out}");
        assert!(out.contains("index 0000000..0202020\n"), "{out}");
    }

    #[test]
    fn test_pure_mode_change_emits_no_index_line() {
        let chmod = FileDiff {
            path: "f".to_string(),
            status: FileChangeStatus::Modified,
            old_id: Some(oid(3)),
            new_id: Some(oid(3)),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_755),
            is_binary: false,
            additions: 0,
            deletions: 0,
            hunks: vec![],
        };
        let out = format_patch(&base_diff(vec![chmod]), IndexAbbrev::Full);
        assert!(out.contains("old mode 100644"), "{out}");
        assert!(!out.contains("index "), "{out}");
    }

    #[test]
    fn test_no_newline_marker_is_emitted() {
        let f = FileDiff {
            path: "f".to_string(),
            status: FileChangeStatus::Modified,
            old_id: Some(oid(1)),
            new_id: Some(oid(2)),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_644),
            is_binary: false,
            additions: 1,
            deletions: 1,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_len: 1,
                new_start: 1,
                new_len: 1,
                func_context: None,
                lines: vec![
                    HunkLine {
                        kind: DiffLineKind::Remove,
                        content: "old".to_string(),
                        no_newline_at_eof: true,
                    },
                    HunkLine {
                        kind: DiffLineKind::Add,
                        content: "new".to_string(),
                        no_newline_at_eof: false,
                    },
                ],
            }],
        };
        let out = format_patch(&base_diff(vec![f]), IndexAbbrev::Full);
        assert!(
            out.contains("-old\n\\ No newline at end of file\n+new\n"),
            "{out}"
        );
        assert!(out.contains("@@ -1 +1 @@"), "{out}");
    }

    #[test]
    fn test_binary_file_rendering() {
        let f = FileDiff {
            path: "img.png".to_string(),
            status: FileChangeStatus::Modified,
            old_id: Some(oid(1)),
            new_id: Some(oid(2)),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_644),
            is_binary: true,
            additions: 0,
            deletions: 0,
            hunks: vec![],
        };
        let out = format_patch(&base_diff(vec![f]), IndexAbbrev::Full);
        assert!(
            out.contains("Binary files a/img.png and b/img.png differ"),
            "{out}"
        );
    }

    #[test]
    fn test_path_quoting() {
        assert_eq!(quote_path("normal/path.rs"), "normal/path.rs");
        assert_eq!(quote_path("with space.rs"), "with space.rs");
        assert!(quote_path("tab\there").starts_with('"'));
        assert_eq!(quote_path("quote\"x"), "\"quote\\\"x\"");
    }
}
