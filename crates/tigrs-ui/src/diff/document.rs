// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! `DiffDocument` and core row representations.
//!
//! Models a unified or side-by-side diff document with parallel navigation
//! tables and invariant assertions for staging safety.

use std::sync::Arc;

/// Location of a diff hunk and optional line within a `CommitDiff`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HunkLocation {
    /// Index of the file within `diff.files`.
    pub file_idx: usize,
    /// Index of the hunk within `file.hunks`.
    pub hunk_idx: usize,
    /// Index of the line within `hunk.lines`, if this is a diff content line.
    pub line_idx: Option<usize>,
}

/// Decoupled line marker indicating addition, deletion, or context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineMarker {
    /// No marker (headers, banners, metadata).
    #[default]
    None,
    /// Context line (` `).
    Context,
    /// Added line (`+`).
    Add,
    /// Deleted line (`-`).
    Del,
}

impl LineMarker {
    /// Returns the standard single-character marker representation.
    #[must_use]
    pub fn as_char(self) -> Option<char> {
        match self {
            Self::None => None,
            Self::Context => Some(' '),
            Self::Add => Some('+'),
            Self::Del => Some('-'),
        }
    }
}

/// Classification of a diff display line for syntax coloring and navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffLineType {
    /// Generic diff header line.
    DiffHeader,
    /// Commit ID header (`commit <hash>`).
    CommitHeader,
    /// Merge commit parents header (`Merge: <parents>`).
    MergeHeader,
    /// Author name and email header (`Author: <name> <email>`).
    AuthorHeader,
    /// Committer name and email header (`Commit: <name> <email>`).
    CommitterHeader,
    /// Author / committer date headers.
    DateHeader,
    /// First line of commit message (subject).
    MessageTitle,
    /// Body lines of commit message.
    MessageBody,
    /// Git commit message trailer (e.g. `Signed-off-by:`, `Reviewed-by:`, `Fixes:`).
    CommitTrailer,
    /// Single file line in the diffstat summary table.
    StatFile,
    /// Aggregated files changed, insertions, and deletions summary line.
    StatSummary,
    /// File unified diff header (`diff --git a/... b/...`).
    FileHeader,
    /// Mode, index, or rename/copy metadata lines.
    FileMeta,
    /// Old file path header (`--- a/path`).
    FileOld,
    /// New file path header (`+++ b/path`).
    FileNew,
    /// Diff hunk range header (`@@ ... @@`).
    HunkHeader,
    /// Unchanged context line.
    DiffContext,
    /// Added line in patch.
    DiffAdd,
    /// Removed line in patch.
    DiffDel,
    /// Horizontal divider or banner rule.
    Delimiter,
    /// Binary file change indicator.
    BinaryNote,
    /// File mode change indicator.
    ModeChange,
    /// Status staging hints and instructions.
    StatusHint,
    /// Empty padding line.
    #[default]
    Empty,
}

/// A single cell (left or right side) in a diff row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowCell {
    /// Line marker (+/-/space/none).
    pub marker: LineMarker,
    /// Raw line text content without marker or trailing newline.
    pub text: Arc<str>,
    /// 1-based original file line number, if applicable.
    pub lineno: Option<u32>,
    /// Word-level emphasis character ranges `(start, end)` in content coordinates.
    pub emphasis: Vec<(u32, u32)>,
    /// Syntax highlighting token spans (empty when syntax highlighting is disabled or plain text).
    pub syntax_spans: Arc<[crate::highlight::SyntaxSpan]>,
    /// True if the line was empty or whitespace-only when added or removed.
    pub is_empty_line: bool,
    /// True if this line belongs to a relocated/moved code block (`color-moved`).
    pub is_moved: bool,
}

impl RowCell {
    /// Creates a row cell from text and marker.
    #[must_use]
    pub fn new(marker: LineMarker, text: impl Into<Arc<str>>, lineno: Option<u32>) -> Self {
        let raw_arc: Arc<str> = text.into();
        let text_arc: Arc<str> = match tigrs_core::ansi::strip_control_chars(&raw_arc) {
            std::borrow::Cow::Borrowed(_) => raw_arc,
            std::borrow::Cow::Owned(clean) => Arc::from(clean.as_str()),
        };
        let is_empty_line = text_arc.trim().is_empty();
        Self {
            marker,
            text: text_arc,
            lineno,
            emphasis: Vec::new(),
            syntax_spans: Arc::from([]),
            is_empty_line,
            is_moved: false,
        }
    }
}

/// A row in the diff document, supporting either unified (single cell) or side-by-side (paired cells).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowPair {
    /// Left pane cell (old side in side-by-side, or primary line in unified).
    pub left: Option<RowCell>,
    /// Right pane cell (new side in side-by-side, or unused in unified).
    pub right: Option<RowCell>,
    /// Anchor into canonical `-U3` `CommitDiff` for staging actions (`u`, `1`).
    /// Spliced or synthetic rows have `anchor: None`.
    pub anchor: Option<HunkLocation>,
    /// Secondary anchor when a paired side-by-side row contains both a deletion (`left`) and addition (`right`).
    pub paired_anchor: Option<HunkLocation>,
    /// Line classification type.
    pub row_type: DiffLineType,
    /// Cached searchable text representation including marker.
    pub search_text: Arc<str>,
}

impl RowPair {
    /// Constructs a unified row with a single cell on the left.
    #[must_use]
    pub fn unified(cell: RowCell, anchor: Option<HunkLocation>, row_type: DiffLineType) -> Self {
        let mut st = String::with_capacity(cell.text.len() + 1);
        if let Some(m) = cell.marker.as_char() {
            st.push(m);
        }
        st.push_str(&cell.text);
        Self {
            left: Some(cell),
            right: None,
            anchor,
            paired_anchor: None,
            row_type,
            search_text: Arc::from(st.into_boxed_str()),
        }
    }

    /// Constructs a side-by-side dual cell row.
    #[must_use]
    pub fn paired(
        left: Option<RowCell>,
        right: Option<RowCell>,
        anchor: Option<HunkLocation>,
        row_type: DiffLineType,
    ) -> Self {
        Self::paired_with_anchors(left, right, anchor, None, row_type)
    }

    /// Constructs a side-by-side dual cell row with both primary and paired secondary anchors.
    #[must_use]
    pub fn paired_with_anchors(
        left: Option<RowCell>,
        right: Option<RowCell>,
        anchor: Option<HunkLocation>,
        paired_anchor: Option<HunkLocation>,
        row_type: DiffLineType,
    ) -> Self {
        let cap = left.as_ref().map_or(0, |l| l.text.len() + 2)
            + right.as_ref().map_or(0, |r| r.text.len() + 2);
        let mut st = String::with_capacity(cap);
        if let Some(ref l) = left {
            if let Some(m) = l.marker.as_char() {
                st.push(m);
            }
            st.push_str(&l.text);
        }
        if let Some(ref r) = right {
            if !st.is_empty() {
                st.push(' ');
            }
            if let Some(m) = r.marker.as_char() {
                st.push(m);
            }
            st.push_str(&r.text);
        }
        Self {
            left,
            right,
            anchor,
            paired_anchor,
            row_type,
            search_text: Arc::from(st.into_boxed_str()),
        }
    }

    /// Returns searchable text string including marker.
    #[must_use]
    pub fn search_text(&self) -> &str {
        &self.search_text
    }

    /// Extracts `(old_lineno, new_lineno)` represented by this row.
    #[must_use]
    pub fn line_numbers(&self) -> (Option<u32>, Option<u32>) {
        if self.right.is_some() {
            (
                self.left.as_ref().and_then(|c| c.lineno),
                self.right.as_ref().and_then(|c| c.lineno),
            )
        } else if let Some(ref left) = self.left {
            match left.marker {
                LineMarker::Del => (left.lineno, None),
                LineMarker::Add => (None, left.lineno),
                LineMarker::Context | LineMarker::None => (left.lineno, left.lineno),
            }
        } else {
            (None, None)
        }
    }
}

/// A fully structured diff document containing display rows and navigation indices.
#[derive(Debug, Clone, Default)]
pub struct DiffDocument {
    /// All display rows in document order.
    pub rows: Vec<RowPair>,
    /// Row indices of all file headers for quick file-to-file jumping (`}`/`{`).
    pub file_indices: Vec<usize>,
    /// Row indices of all hunk headers for quick hunk-to-hunk jumping (`)`/`(` or `@`).
    pub hunk_indices: Vec<usize>,
    /// Maps each display row index to the file index within `diff.files`, if any.
    pub line_to_file: Vec<Option<u32>>,
    /// Maps each display row index to the hunk and line location in scope, if any.
    pub line_to_hunk: Vec<Option<HunkLocation>>,
    /// Maximum base-10 digit width across all line numbers in this document (at least 4).
    pub max_lineno_digits: usize,
}

impl DiffDocument {
    /// Creates a new `DiffDocument` and verifies parallel index invariants.
    #[must_use]
    pub fn new(
        rows: Vec<RowPair>,
        file_indices: Vec<usize>,
        hunk_indices: Vec<usize>,
        line_to_file: Vec<Option<u32>>,
        line_to_hunk: Vec<Option<HunkLocation>>,
    ) -> Self {
        debug_assert_eq!(
            rows.len(),
            line_to_file.len(),
            "DiffDocument invariant violated: rows ({}) != line_to_file ({})",
            rows.len(),
            line_to_file.len()
        );
        debug_assert_eq!(
            rows.len(),
            line_to_hunk.len(),
            "DiffDocument invariant violated: rows ({}) != line_to_hunk ({})",
            rows.len(),
            line_to_hunk.len()
        );

        let max_lineno: u32 = rows
            .iter()
            .map(|r| {
                let l = r.left.as_ref().and_then(|c| c.lineno).unwrap_or(0);
                let rt = r.right.as_ref().and_then(|c| c.lineno).unwrap_or(0);
                l.max(rt)
            })
            .max()
            .unwrap_or(0);
        let max_lineno_digits = max_lineno
            .checked_ilog10()
            .map_or(1, |d| d as usize + 1)
            .max(4);

        Self {
            rows,
            file_indices,
            hunk_indices,
            line_to_file,
            line_to_hunk,
            max_lineno_digits,
        }
    }

    /// Total number of rows in the document.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Returns true if the document contains no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Resolves the hunk location for the line at `cursor` (0-based).
    #[must_use]
    pub fn selected_hunk(&self, cursor: usize) -> Option<HunkLocation> {
        self.line_to_hunk.get(cursor).copied().flatten()
    }

    /// Resolves the specific line location for the line at `cursor` (0-based).
    #[must_use]
    pub fn selected_line(&self, cursor: usize) -> Option<HunkLocation> {
        let loc = self.line_to_hunk.get(cursor).copied().flatten()?;
        if loc.line_idx.is_some() {
            Some(loc)
        } else {
            None
        }
    }

    /// Finds the line index of the previous hunk header before `cursor`.
    #[must_use]
    pub fn find_prev_hunk(&self, cursor: usize) -> usize {
        self.hunk_indices
            .iter()
            .copied()
            .rfind(|&idx| idx < cursor)
            .unwrap_or(cursor)
    }

    /// Finds the line index of the next hunk header after `cursor`.
    #[must_use]
    pub fn find_next_hunk(&self, cursor: usize) -> usize {
        self.hunk_indices
            .iter()
            .copied()
            .find(|&idx| idx > cursor)
            .unwrap_or(cursor)
    }

    /// Finds the line index of the previous file header before `cursor`.
    #[must_use]
    pub fn find_prev_file(&self, cursor: usize) -> usize {
        self.file_indices
            .iter()
            .copied()
            .rfind(|&idx| idx < cursor)
            .unwrap_or(cursor)
    }

    /// Finds the line index of the next file header after `cursor`.
    #[must_use]
    pub fn find_next_file(&self, cursor: usize) -> usize {
        self.file_indices
            .iter()
            .copied()
            .find(|&idx| idx > cursor)
            .unwrap_or(cursor)
    }

    /// Resolves all line indices associated with the row at `cursor` (e.g. both deletion and addition on a paired side-by-side row).
    #[must_use]
    pub fn selected_lines(&self, cursor: usize) -> Option<(usize, usize, Vec<usize>)> {
        let row = self.rows.get(cursor)?;
        let primary = row
            .anchor
            .or_else(|| self.line_to_hunk.get(cursor).copied().flatten())?;
        let primary_line_idx = primary.line_idx?;
        let mut indices = vec![primary_line_idx];
        if let Some(paired) = row.paired_anchor
            && paired.file_idx == primary.file_idx
            && paired.hunk_idx == primary.hunk_idx
            && let Some(p_idx) = paired.line_idx
            && !indices.contains(&p_idx)
        {
            indices.push(p_idx);
        }
        indices.sort_unstable();
        Some((primary.file_idx, primary.hunk_idx, indices))
    }

    /// Maps a cursor row index from `self` to the semantically equivalent row index in `target`.
    ///
    /// Uses a 5-tier priority match:
    /// 1. Exact `HunkLocation` match (including paired anchors on side-by-side rows).
    /// 2. Same file & hunk with matching `old_lineno` or `new_lineno`.
    /// 3. First row in the same hunk `(file_idx, hunk_idx)`.
    /// 4. First row in the same file `file_idx`.
    /// 5. Clamped index within document bounds.
    #[must_use]
    pub fn map_cursor_to(&self, current_cursor: usize, target: &Self) -> usize {
        if target.is_empty() {
            return 0;
        }
        let max_target = target.len().saturating_sub(1);
        let Some(src_row) = self.rows.get(current_cursor) else {
            return current_cursor.min(max_target);
        };

        let src_hunk = src_row
            .anchor
            .or_else(|| self.line_to_hunk.get(current_cursor).copied().flatten());
        let src_paired = src_row.paired_anchor;
        let src_file = self.line_to_file.get(current_cursor).copied().flatten();
        let (src_old_ln, src_new_ln) = src_row.line_numbers();

        // Tier 1: Exact HunkLocation match (where line_idx is present)
        let candidate_locs: Vec<HunkLocation> = [src_hunk, src_paired]
            .into_iter()
            .flatten()
            .filter(|loc| loc.line_idx.is_some())
            .collect();

        if !candidate_locs.is_empty() {
            for (idx, tgt_row) in target.rows.iter().enumerate() {
                let tgt_primary = tgt_row
                    .anchor
                    .or_else(|| target.line_to_hunk.get(idx).copied().flatten());
                let tgt_secondary = tgt_row.paired_anchor;
                for loc in &candidate_locs {
                    if tgt_primary == Some(*loc) || tgt_secondary == Some(*loc) {
                        return idx;
                    }
                }
            }
        }

        // Tier 2: Same file & hunk with matching old_lineno or new_lineno
        if src_old_ln.is_some() || src_new_ln.is_some() {
            for (idx, tgt_row) in target.rows.iter().enumerate() {
                let tgt_file = target.line_to_file.get(idx).copied().flatten();
                if src_file.is_some() && tgt_file != src_file {
                    continue;
                }
                if let Some(sh) = src_hunk
                    && let Some(th) = target.line_to_hunk.get(idx).copied().flatten()
                    && th.hunk_idx != sh.hunk_idx
                {
                    continue;
                }
                let (tgt_old_ln, tgt_new_ln) = tgt_row.line_numbers();
                if (src_old_ln.is_some() && src_old_ln == tgt_old_ln)
                    || (src_new_ln.is_some() && src_new_ln == tgt_new_ln)
                {
                    return idx;
                }
            }
        }

        // Tier 3: First row matching same (file_idx, hunk_idx)
        if let Some(sh) = src_hunk {
            for (idx, loc_opt) in target.line_to_hunk.iter().enumerate() {
                if let Some(loc) = loc_opt
                    && loc.file_idx == sh.file_idx
                    && loc.hunk_idx == sh.hunk_idx
                {
                    return idx;
                }
            }
        }

        // Tier 4: First row matching same file_idx (and matching row_type if header)
        if let Some(sf) = src_file {
            // Try matching same row_type within that file first (e.g., FileHeader -> FileHeader)
            for (idx, tgt_row) in target.rows.iter().enumerate() {
                if target.line_to_file.get(idx).copied().flatten() == Some(sf)
                    && tgt_row.row_type == src_row.row_type
                {
                    return idx;
                }
            }
            // Otherwise first row of that file
            for (idx, f_opt) in target.line_to_file.iter().enumerate() {
                if *f_opt == Some(sf) {
                    return idx;
                }
            }
        }

        // Tier 5: Clamped index within document bounds
        current_cursor.min(max_target)
    }

    /// Returns the searchable text representation of row `idx`.
    #[must_use]
    pub fn row_search_text(&self, idx: usize) -> Option<&str> {
        self.rows.get(idx).map(RowPair::search_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_marker_and_row_cell_creation() {
        assert_eq!(LineMarker::None.as_char(), None);
        assert_eq!(LineMarker::Context.as_char(), Some(' '));
        assert_eq!(LineMarker::Add.as_char(), Some('+'));
        assert_eq!(LineMarker::Del.as_char(), Some('-'));

        let cell_empty = RowCell::new(LineMarker::Add, "   ", Some(10));
        assert!(cell_empty.is_empty_line);
        assert_eq!(cell_empty.lineno, Some(10));

        let cell_code = RowCell::new(LineMarker::Context, "fn main() {}", Some(11));
        assert!(!cell_code.is_empty_line);
    }

    #[test]
    fn test_row_pair_unified_and_paired_line_numbers_and_search() {
        let unified_add = RowPair::unified(
            RowCell::new(LineMarker::Add, "let x = 1;", Some(42)),
            None,
            DiffLineType::DiffAdd,
        );
        assert_eq!(unified_add.line_numbers(), (None, Some(42)));
        assert_eq!(unified_add.search_text(), "+let x = 1;");

        let split_pair = RowPair::paired(
            Some(RowCell::new(LineMarker::Del, "old_call()", Some(10))),
            Some(RowCell::new(LineMarker::Add, "new_call()", Some(12))),
            None,
            DiffLineType::DiffContext,
        );
        assert_eq!(split_pair.line_numbers(), (Some(10), Some(12)));
        assert!(split_pair.search_text().contains("old_call()"));
        assert!(split_pair.search_text().contains("new_call()"));
    }

    #[test]
    fn test_diff_document_cursor_anchor_mapping_all_tiers() {
        let hunk0 = HunkLocation {
            file_idx: 0,
            hunk_idx: 0,
            line_idx: None,
        };
        let hunk0_line0 = HunkLocation {
            file_idx: 0,
            hunk_idx: 0,
            line_idx: Some(0),
        };

        let doc_a = DiffDocument::new(
            vec![
                RowPair::unified(
                    RowCell::new(LineMarker::None, "diff --git a/a.rs b/a.rs", None),
                    None,
                    DiffLineType::FileHeader,
                ),
                RowPair::unified(
                    RowCell::new(LineMarker::None, "@@ -1,2 +1,2 @@", None),
                    Some(hunk0),
                    DiffLineType::HunkHeader,
                ),
                RowPair::paired(
                    Some(RowCell::new(LineMarker::Context, "line 10", Some(10))),
                    Some(RowCell::new(LineMarker::Context, "line 10", Some(10))),
                    Some(hunk0_line0),
                    DiffLineType::DiffContext,
                ),
            ],
            vec![0],
            vec![1],
            vec![Some(0), Some(0), Some(0)],
            vec![None, Some(hunk0), Some(hunk0_line0)],
        );

        // Target document B has extra rows inserted above, shifting indices
        let doc_b = DiffDocument::new(
            vec![
                RowPair::unified(
                    RowCell::new(LineMarker::None, "Commit Subject", None),
                    None,
                    DiffLineType::MessageTitle,
                ),
                RowPair::unified(
                    RowCell::new(LineMarker::None, "diff --git a/a.rs b/a.rs", None),
                    None,
                    DiffLineType::FileHeader,
                ),
                RowPair::unified(
                    RowCell::new(LineMarker::None, "@@ -1,2 +1,2 @@", None),
                    Some(hunk0),
                    DiffLineType::HunkHeader,
                ),
                RowPair::paired(
                    Some(RowCell::new(LineMarker::Context, "line 10", Some(10))),
                    Some(RowCell::new(LineMarker::Context, "line 10", Some(10))),
                    Some(hunk0_line0),
                    DiffLineType::DiffContext,
                ),
            ],
            vec![1],
            vec![2],
            vec![None, Some(0), Some(0), Some(0)],
            vec![None, None, Some(hunk0), Some(hunk0_line0)],
        );

        // Tier 1 match: Row 2 in doc_a -> Row 3 in doc_b
        assert_eq!(doc_a.map_cursor_to(2, &doc_b), 3);
        // Tier 4 match: FileHeader Row 0 in doc_a -> Row 1 in doc_b
        assert_eq!(doc_a.map_cursor_to(0, &doc_b), 1);

        // Tier 5 fallback on empty target
        let empty_doc = DiffDocument::default();
        assert_eq!(doc_a.map_cursor_to(2, &empty_doc), 0);
    }
}
