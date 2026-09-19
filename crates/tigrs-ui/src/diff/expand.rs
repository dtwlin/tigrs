// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Context expansion and full-file blob splicing.
//!
//! Provides display-only context trimming and blob gap splicing while preserving
//! the canonical `-U3` `CommitDiff` hunks intact for staging operations.

use super::document::{HunkLocation, LineMarker};
use std::sync::Arc;
use tigrs_git::{DiffHunk, DiffLineKind, FileDiff};

/// A display line produced by context expansion or trimming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedLine {
    /// Line marker (+/-/space/none).
    pub marker: LineMarker,
    /// Line text content.
    pub content: Arc<str>,
    /// Old file line number (1-based), if applicable.
    pub old_lineno: Option<u32>,
    /// New file line number (1-based), if applicable.
    pub new_lineno: Option<u32>,
    /// Anchor pointing into canonical `-U3` hunk for staging, or `None` for spliced lines.
    pub anchor: Option<HunkLocation>,
    /// True if line has no trailing newline at end of file.
    pub no_newline_at_eof: bool,
}

/// An expanded display item within a file (either a hunk header or a diff line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandedItem {
    /// Hunk header marker with hunk index.
    HunkHeader {
        /// 0-based index of the diff hunk within the file.
        hunk_idx: usize,
    },
    /// Expanded line (content or spliced).
    Line(ExpandedLine),
}

/// Expands or trims hunks for a file according to `context` and `is_full`,
/// returning a sequence of items (hunk headers and lines) in display order.
#[must_use]
pub fn expand_file_items(
    file_idx: usize,
    file: &FileDiff,
    blob_lines: Option<&[Arc<str>]>,
    context: usize,
    is_full: bool,
) -> Vec<ExpandedItem> {
    expand_file_items_with_hunk_context(file_idx, file, blob_lines, context, is_full, None)
}

#[inline]
fn sanitize_blob_line(line: &Arc<str>) -> Arc<str> {
    match tigrs_core::ansi::strip_control_chars(line) {
        std::borrow::Cow::Borrowed(_) => Arc::clone(line),
        std::borrow::Cow::Owned(clean) => Arc::from(clean),
    }
}

/// Expands or trims hunks for a file according to `context`, `is_full`, and optional
/// per-hunk extra context overrides `hunk_extra_context` keyed by `(file_idx, hunk_idx)`.
#[must_use]
pub fn expand_file_items_with_hunk_context(
    file_idx: usize,
    file: &FileDiff,
    blob_lines: Option<&[Arc<str>]>,
    context: usize,
    is_full: bool,
    hunk_extra_context: Option<&std::collections::BTreeMap<(usize, usize), usize>>,
) -> Vec<ExpandedItem> {
    if file.hunks.is_empty() {
        return Vec::new();
    }

    let hunk_ctx = |h_idx: usize| -> usize {
        let extra = hunk_extra_context
            .and_then(|m| m.get(&(file_idx, h_idx)).copied())
            .unwrap_or(0);
        context.saturating_add(extra)
    };

    // Check if we can/should splice from the blob
    let can_splice = is_full || (0..file.hunks.len()).any(|h_idx| hunk_ctx(h_idx) > 3);
    let validated_blob = if can_splice {
        blob_lines.filter(|lines| validate_blob_match(file, lines))
    } else {
        None
    };

    let mut out = Vec::new();

    // Splicing before first hunk
    if let Some(blob) = validated_blob {
        let first_hunk = &file.hunks[0];
        let first_new_start = first_hunk.new_start as usize;
        let prefix_count = if is_full {
            first_new_start.saturating_sub(1)
        } else {
            let extra = hunk_ctx(0).saturating_sub(3);
            extra.min(first_new_start.saturating_sub(1))
        };

        let end_0 = first_new_start.saturating_sub(1).min(blob.len());
        let start_0 = end_0.saturating_sub(prefix_count);
        for (offset, line) in blob[start_0..end_0].iter().enumerate() {
            let lineno = (start_0 + offset + 1) as u32;
            out.push(ExpandedItem::Line(ExpandedLine {
                marker: LineMarker::Context,
                content: sanitize_blob_line(line),
                old_lineno: Some(lineno),
                new_lineno: Some(lineno),
                anchor: None,
                no_newline_at_eof: false,
            }));
        }
    }

    for (h_idx, hunk) in file.hunks.iter().enumerate() {
        // Emit hunk header
        out.push(ExpandedItem::HunkHeader { hunk_idx: h_idx });

        // If this hunk (h_idx > 0) requested extra leading context that was not covered
        // by the previous hunk's trailing gap splice, emit its leading gap lines here.
        if h_idx > 0
            && !is_full
            && let Some(blob) = validated_blob
        {
            let prev_hunk = &file.hunks[h_idx - 1];
            let gap_start = (prev_hunk.new_start + prev_hunk.new_len) as usize;
            let gap_end = hunk.new_start as usize;
            if gap_end > gap_start {
                let total_gap = gap_end - gap_start;
                let prev_extra = hunk_ctx(h_idx - 1).saturating_sub(3);
                let curr_extra = hunk_ctx(h_idx).saturating_sub(3);
                if curr_extra > 0 && prev_extra.saturating_add(curr_extra) < total_gap {
                    let splice_start = gap_end.saturating_sub(curr_extra).max(gap_start);
                    let start_0 = splice_start.saturating_sub(1);
                    let end_0 = gap_end.saturating_sub(1).min(blob.len());
                    if end_0 > start_0 {
                        for (offset, line) in blob[start_0..end_0].iter().enumerate() {
                            let lineno = (start_0 + offset + 1) as u32;
                            out.push(ExpandedItem::Line(ExpandedLine {
                                marker: LineMarker::Context,
                                content: sanitize_blob_line(line),
                                old_lineno: Some(lineno),
                                new_lineno: Some(lineno),
                                anchor: None,
                                no_newline_at_eof: false,
                            }));
                        }
                    }
                }
            }
        }

        // Collect lines for this hunk
        let hunk_expanded = expand_single_hunk(file_idx, h_idx, hunk, hunk_ctx(h_idx));
        out.extend(hunk_expanded.into_iter().map(ExpandedItem::Line));

        // Splice gap between this hunk and next hunk
        if let Some(blob) = validated_blob
            && let Some(next_hunk) = file.hunks.get(h_idx + 1)
        {
            let gap_start = (hunk.new_start + hunk.new_len) as usize;
            let gap_end = next_hunk.new_start as usize;
            if gap_end > gap_start {
                let total_gap = gap_end - gap_start;
                let (splice_start, splice_end) = if is_full {
                    (gap_start, gap_end)
                } else {
                    let extra = hunk_ctx(h_idx).saturating_sub(3);
                    let next_extra = hunk_ctx(h_idx + 1).saturating_sub(3);
                    if extra.saturating_add(next_extra) >= total_gap {
                        (gap_start, gap_end)
                    } else {
                        (gap_start, (gap_start + extra).min(gap_end))
                    }
                };

                let start_0 = splice_start.saturating_sub(1);
                let end_0 = splice_end.saturating_sub(1).min(blob.len());
                if end_0 > start_0 {
                    for (offset, line) in blob[start_0..end_0].iter().enumerate() {
                        let lineno = (start_0 + offset + 1) as u32;
                        out.push(ExpandedItem::Line(ExpandedLine {
                            marker: LineMarker::Context,
                            content: sanitize_blob_line(line),
                            old_lineno: Some(lineno),
                            new_lineno: Some(lineno),
                            anchor: None,
                            no_newline_at_eof: false,
                        }));
                    }
                }
            }
        }
    }

    // Splice suffix after last hunk
    if let Some(blob) = validated_blob
        && let Some(last_hunk) = file.hunks.last()
    {
        let last_idx = file.hunks.len().saturating_sub(1);
        let last_new_end = (last_hunk.new_start + last_hunk.new_len) as usize;
        if last_new_end <= blob.len() {
            let suffix_count = if is_full {
                blob.len().saturating_sub(last_new_end.saturating_sub(1))
            } else {
                let extra = hunk_ctx(last_idx).saturating_sub(3);
                extra.min(blob.len().saturating_sub(last_new_end.saturating_sub(1)))
            };

            let start_0 = last_new_end.saturating_sub(1);
            let end_0 = (start_0 + suffix_count).min(blob.len());
            if end_0 > start_0 {
                for (offset, line) in blob[start_0..end_0].iter().enumerate() {
                    let lineno = (start_0 + offset + 1) as u32;
                    out.push(ExpandedItem::Line(ExpandedLine {
                        marker: LineMarker::Context,
                        content: sanitize_blob_line(line),
                        old_lineno: Some(lineno),
                        new_lineno: Some(lineno),
                        anchor: None,
                        no_newline_at_eof: false,
                    }));
                }
            }
        }
    }

    out
}

/// Expands or trims hunks for a file according to `context` and `is_full`.
///
/// If `blob_lines` is provided and valid, gaps between hunks and surrounding file
/// contents are spliced in with `anchor: None`.
#[must_use]
pub fn expand_file_lines(
    file_idx: usize,
    file: &FileDiff,
    blob_lines: Option<&[Arc<str>]>,
    context: usize,
    is_full: bool,
) -> Vec<ExpandedLine> {
    expand_file_items(file_idx, file, blob_lines, context, is_full)
        .into_iter()
        .filter_map(|item| match item {
            ExpandedItem::Line(l) => Some(l),
            ExpandedItem::HunkHeader { .. } => None,
        })
        .collect()
}

/// Expands or trims a single hunk's lines, computing line numbers and anchors.
fn expand_single_hunk(
    file_idx: usize,
    hunk_idx: usize,
    hunk: &DiffHunk,
    context: usize,
) -> Vec<ExpandedLine> {
    let mut out = Vec::with_capacity(hunk.lines.len());
    let mut cur_old = hunk.old_start;
    let mut cur_new = hunk.new_start;

    // In context trimming (context < 3), identify leading/trailing context runs
    let leading_context_len = hunk
        .lines
        .iter()
        .take_while(|l| l.kind == DiffLineKind::Context)
        .count();
    let trailing_context_len = hunk
        .lines
        .iter()
        .rev()
        .take_while(|l| l.kind == DiffLineKind::Context)
        .count();

    let total = hunk.lines.len();

    for (l_idx, line) in hunk.lines.iter().enumerate() {
        let (marker, old_num, new_num) = match line.kind {
            DiffLineKind::Context => {
                let old = cur_old;
                let new = cur_new;
                cur_old = cur_old.saturating_add(1);
                cur_new = cur_new.saturating_add(1);
                (LineMarker::Context, Some(old), Some(new))
            }
            DiffLineKind::Add => {
                let new = cur_new;
                cur_new = cur_new.saturating_add(1);
                (LineMarker::Add, None, Some(new))
            }
            DiffLineKind::Remove => {
                let old = cur_old;
                cur_old = cur_old.saturating_add(1);
                (LineMarker::Del, Some(old), None)
            }
        };

        // Context trimming for context < 3
        if context < 3 && line.kind == DiffLineKind::Context {
            if l_idx < leading_context_len {
                let skip_count = leading_context_len.saturating_sub(context);
                if l_idx < skip_count {
                    continue;
                }
            } else if l_idx >= total.saturating_sub(trailing_context_len) {
                let keep_start = total.saturating_sub(context);
                if l_idx < keep_start {
                    continue;
                }
            }
        }

        let clean = tigrs_core::ansi::strip_control_chars(&line.content);
        out.push(ExpandedLine {
            marker,
            content: Arc::from(clean.as_ref()),
            old_lineno: old_num,
            new_lineno: new_num,
            anchor: Some(HunkLocation {
                file_idx,
                hunk_idx,
                line_idx: Some(l_idx),
            }),
            no_newline_at_eof: line.no_newline_at_eof,
        });
    }

    out
}

/// Verifies that the blob lines match the diff hunks by checking the first hunk line.
fn validate_blob_match(file: &FileDiff, blob: &[Arc<str>]) -> bool {
    let Some(first_hunk) = file.hunks.first() else {
        return true;
    };
    if first_hunk.new_start == 0 {
        return true;
    }
    let check_idx = (first_hunk.new_start as usize).saturating_sub(1);
    let Some(blob_line) = blob.get(check_idx) else {
        return false;
    };
    if let Some(first_line) = first_hunk.lines.first()
        && first_line.kind != tigrs_git::DiffLineKind::Remove
    {
        return blob_line.as_ref()
            == tigrs_core::ansi::strip_control_chars(&first_line.content).as_ref();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigrs_git::{DiffHunk, DiffLineKind, HunkLine};

    fn make_test_file() -> FileDiff {
        FileDiff {
            path: "foo.rs".to_string(),
            old_id: Some(gix::ObjectId::empty_tree(gix::hash::Kind::Sha1)),
            new_id: Some(gix::ObjectId::empty_tree(gix::hash::Kind::Sha1)),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_644),
            status: tigrs_git::FileChangeStatus::Modified,
            is_binary: false,
            additions: 2,
            deletions: 1,
            hunks: vec![DiffHunk {
                old_start: 3,
                old_len: 3,
                new_start: 3,
                new_len: 4,
                func_context: Some("fn main()".to_string()),
                lines: vec![
                    HunkLine {
                        kind: DiffLineKind::Context,
                        content: "line 3".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Remove,
                        content: "old 4".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Add,
                        content: "new 4a".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Add,
                        content: "new 4b".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Context,
                        content: "line 5".to_string(),
                        no_newline_at_eof: false,
                    },
                ],
            }],
        }
    }

    #[test]
    fn test_expand_preserves_canonical_anchors() {
        let file = make_test_file();
        let expanded = expand_file_lines(0, &file, None, 3, false);
        assert_eq!(expanded.len(), 5);

        // Anchors point to canonical line indices
        assert_eq!(expanded[0].anchor.unwrap().line_idx, Some(0));
        assert_eq!(expanded[1].anchor.unwrap().line_idx, Some(1));
        assert_eq!(expanded[2].anchor.unwrap().line_idx, Some(2));
        assert_eq!(expanded[3].anchor.unwrap().line_idx, Some(3));
        assert_eq!(expanded[4].anchor.unwrap().line_idx, Some(4));
    }

    #[test]
    fn test_expand_full_file_splices_unanchored_lines() {
        let file = make_test_file();
        let blob: Vec<Arc<str>> = vec![
            Arc::from("line 1"),
            Arc::from("line 2"),
            Arc::from("line 3"),
            Arc::from("new 4a"),
            Arc::from("new 4b"),
            Arc::from("line 5"),
            Arc::from("line 6"),
            Arc::from("line 7"),
        ];

        let expanded = expand_file_lines(0, &file, Some(&blob), 3, true);
        // Spliced prefix (line 1, 2) + hunk (5 lines) + spliced suffix (line 6, 7) = 9 lines
        assert_eq!(expanded.len(), 9);

        // Spliced lines must have None anchor
        assert_eq!(expanded[0].anchor, None);
        assert_eq!(expanded[0].content.as_ref(), "line 1");
        assert_eq!(expanded[1].anchor, None);
        assert_eq!(expanded[1].content.as_ref(), "line 2");

        // Hunk lines must have real anchor
        assert!(expanded[2].anchor.is_some());
        assert_eq!(expanded[2].content.as_ref(), "line 3");

        // Suffix spliced lines
        assert_eq!(expanded[7].anchor, None);
        assert_eq!(expanded[7].content.as_ref(), "line 6");
        assert_eq!(expanded[8].anchor, None);
        assert_eq!(expanded[8].content.as_ref(), "line 7");
    }

    #[test]
    fn test_expand_file_items_out_of_bounds_blob_does_not_panic() {
        let mut file = make_test_file();
        file.hunks[0].new_start = 25;
        // Short blob with only 2 lines where new_start is 25
        let short_blob: Vec<Arc<str>> = vec![Arc::from("short 1"), Arc::from("short 2")];
        let items = expand_file_items(0, &file, Some(&short_blob), 6, true);
        assert!(!items.is_empty());
    }
}
