// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Interactive revision log view presenting rich commit metadata and diffstat summary changes.
//!
//! Provides on-par upstream Tig log view (`src/log.c`) parity:
//! - Full commit header (`commit <id>`, `Merge: ...`, `Author: ...`, `Date: ...`)
//! - Complete commit message (subject and body)
//! - Per-commit summary changes (diffstat file list with scaled additions `+` in green
//!   and deletions `-` in red, plus binary markers)
//! - Summary line (`X files changed, Y insertions(+), Z deletions(-)`)
//! - Interactive navigation, commit jumping, search, and opening diffs (`Enter`) or files (`e`).

use super::ViewportCursor;
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};
use std::io::Write;
use tigrs_core::ansi::truncate_display_width;
use tigrs_core::error::Result;
use tigrs_git::{CommitDiff, ObjectId};
use unicode_width::UnicodeWidthStr;

/// Maximum width allocated to the file path column in diffstat lines.
pub const MAX_DIFFSTAT_PATH_WIDTH: usize = 50;

/// Maximum number of `+` and `-` glyphs rendered in the diffstat bar.
pub const MAX_DIFFSTAT_BAR_WIDTH: usize = 35;

/// Semantic classification of each rendered line in the log view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogLineKind {
    /// Commit header line: `commit <oid> (<ref_names>)`
    Commit {
        /// Commit object ID.
        oid: ObjectId,
        /// Formatted reference decoration suffix.
        refs: String,
    },
    /// Merge parent line: `Merge: <id1> <id2>`
    Merge {
        /// Parent commit object IDs of the merge commit.
        parents: Vec<ObjectId>,
    },
    /// Author line: `Author: <name> <<email>>`
    Author {
        /// Author display name.
        name: String,
        /// Author email address.
        email: String,
    },
    /// Date line: `Date:   <formatted_date>`
    Date {
        /// Formatted commit timestamp string.
        date: String,
    },
    /// Commit subject: indented 4 spaces
    Subject(String),
    /// Commit body line: indented 4 spaces
    Body(String),
    /// Commit trailer / metadata tag line: indented 4 spaces
    Trailer(String),
    /// Diffstat file line:
    /// ` <path> | <count> <plus_bars><minus_bars>` or ` <path> | Bin`
    DiffStatFile {
        /// Repository-relative file path.
        path: String,
        /// Total changed lines (`additions + deletions`).
        count: usize,
        /// Scaled count of `+` histogram bars.
        plus_count: usize,
        /// Scaled count of `-` histogram bars.
        minus_count: usize,
        /// Whether the file is binary.
        is_binary: bool,
    },
    /// Diffstat aggregate summary line:
    /// ` <N> files changed, <X> insertions(+), <Y> deletions(-)`
    DiffStatSummary(String),
    /// Blank separating line
    Empty,
}

/// A single logical line inside the log view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// Associated commit object ID.
    pub commit_id: ObjectId,
    /// Semantic line kind.
    pub kind: LogLineKind,
    /// Relative path if this line corresponds to a diffstat file entry.
    pub file_path: Option<String>,
    /// Unformatted plain-text representation (used for searching and layout width).
    pub raw_text: String,
}

/// Interactive log view displaying rich commit history and diffstat summary changes.
#[derive(Debug)]
pub struct LogView {
    lines: Vec<LogLine>,
    nav: ViewportCursor,
    /// Indices into `lines` where each commit header begins.
    commit_indices: Vec<usize>,
    /// Fast O(1) lookup from commit OID to index within `commit_indices`.
    commit_pos_by_id: std::collections::HashMap<ObjectId, usize>,
    branch_name: String,
}

impl LogView {
    /// Creates an empty `LogView`.
    pub fn new(branch_name: impl Into<String>) -> Self {
        let raw_branch = branch_name.into();
        Self {
            lines: Vec::new(),
            nav: ViewportCursor::new(),
            commit_indices: Vec::new(),
            commit_pos_by_id: std::collections::HashMap::new(),
            branch_name: tigrs_core::ansi::strip_control_chars(&raw_branch).into_owned(),
        }
    }

    /// Associated branch or reference name.
    pub fn branch_name(&self) -> &str {
        &self.branch_name
    }

    /// Total number of lines.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Whether the log view contains any lines.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Currently selected row index.
    pub fn cursor(&self) -> usize {
        self.nav.cursor()
    }

    /// Current vertical scroll offset.
    pub fn scroll_offset(&self) -> usize {
        self.nav.scroll_offset()
    }

    /// Returns the currently selected line, if any.
    pub fn selected_line(&self) -> Option<&LogLine> {
        self.lines.get(self.nav.cursor())
    }

    /// Returns the commit ID associated with the current cursor position.
    pub fn selected_commit_id(&self) -> Option<ObjectId> {
        self.selected_line().map(|l| l.commit_id)
    }

    /// Returns the file path and commit ID if the cursor is currently resting on a diffstat file line.
    pub fn selected_file(&self) -> Option<(&str, ObjectId)> {
        let line = self.selected_line()?;
        line.file_path.as_deref().map(|path| (path, line.commit_id))
    }

    /// Returns all commit indices in the view.
    pub fn commit_indices(&self) -> &[usize] {
        &self.commit_indices
    }

    /// Clears all lines and resets the cursor.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.commit_indices.clear();
        self.commit_pos_by_id.clear();
        self.nav.reset();
    }

    /// Appends a lightweight commit header placeholder row with no diffstat yet.
    ///
    /// `commit_indices` is updated immediately so `jump_to_commit` works before
    /// diffstats are streamed in via [`Self::fill_commit_diff`].
    pub fn append_commit_placeholder(&mut self, commit_id: ObjectId, ref_names: Option<&[String]>) {
        let start_idx = self.lines.len();
        let commit_pos = self.commit_indices.len();
        self.commit_indices.push(start_idx);
        self.commit_pos_by_id.insert(commit_id, commit_pos);

        let refs_str = match ref_names {
            Some(names) if !names.is_empty() => {
                let joined = names.join(", ");
                format!(" ({})", tigrs_core::ansi::strip_control_chars(&joined))
            }
            _ => String::new(),
        };
        let commit_text = format!("commit {commit_id}{refs_str}");
        self.lines.push(LogLine {
            commit_id,
            kind: LogLineKind::Commit {
                oid: commit_id,
                refs: refs_str,
            },
            file_path: None,
            raw_text: commit_text,
        });
        self.lines.push(LogLine {
            commit_id,
            kind: LogLineKind::Empty,
            file_path: None,
            raw_text: String::new(),
        });
    }

    /// Replaces a placeholder commit block (or updates an existing block) with full diffstat lines.
    ///
    /// Adjusts subsequent `commit_indices` and shifts `cursor`/`scroll_offset` if the updated
    /// commit lies above the active cursor position so the viewport remains anchored.
    pub fn fill_commit_diff(&mut self, diff: &CommitDiff, ref_names: Option<&[String]>) -> bool {
        let commit_id = diff.commit_id;
        let Some(&commit_pos) = self.commit_pos_by_id.get(&commit_id) else {
            self.append_commit_diff(diff, ref_names);
            return true;
        };

        let start_idx = self.commit_indices[commit_pos];
        let end_idx = if commit_pos + 1 < self.commit_indices.len() {
            self.commit_indices[commit_pos + 1]
        } else {
            self.lines.len()
        };
        let old_len = end_idx.saturating_sub(start_idx);
        let new_lines = Self::build_commit_lines(diff, ref_names);
        let new_len = new_lines.len();

        self.lines.splice(start_idx..end_idx, new_lines);

        let delta = new_len as isize - old_len as isize;
        if delta != 0 {
            for idx in &mut self.commit_indices[commit_pos + 1..] {
                *idx = (*idx as isize + delta).max(0) as usize;
            }
            if start_idx < self.nav.cursor() {
                self.nav.cursor = (self.nav.cursor as isize + delta).max(0) as usize;
            }
            if start_idx < self.nav.scroll_offset() {
                self.nav.scroll_offset = (self.nav.scroll_offset as isize + delta).max(0) as usize;
            }
        }
        true
    }

    /// Appends a commit diff and its diffstat summary changes to the log view.
    pub fn append_commit_diff(&mut self, diff: &CommitDiff, ref_names: Option<&[String]>) {
        let start_idx = self.lines.len();
        let commit_pos = self.commit_indices.len();
        self.commit_indices.push(start_idx);
        self.commit_pos_by_id.insert(diff.commit_id, commit_pos);
        self.lines.extend(Self::build_commit_lines(diff, ref_names));
    }

    fn build_commit_lines(diff: &CommitDiff, ref_names: Option<&[String]>) -> Vec<LogLine> {
        let commit_id = diff.commit_id;
        let mut lines = Vec::new();

        // 1. Commit Header: `commit <oid> (<ref_names>)`
        let refs_str = match ref_names {
            Some(names) if !names.is_empty() => {
                let joined = names.join(", ");
                format!(" ({})", tigrs_core::ansi::strip_control_chars(&joined))
            }
            _ => String::new(),
        };
        let commit_text = format!("commit {commit_id}{refs_str}");
        lines.push(LogLine {
            commit_id,
            kind: LogLineKind::Commit {
                oid: commit_id,
                refs: refs_str,
            },
            file_path: None,
            raw_text: commit_text,
        });

        // 2. Merge Header (if 2+ parents)
        if diff.parent_ids.len() > 1 {
            let p_hexes: Vec<String> = diff
                .parent_ids
                .iter()
                .map(|p| {
                    let hex = p.to_hex().to_string();
                    hex[..7.min(hex.len())].to_string()
                })
                .collect();
            let merge_text = format!("Merge: {}", p_hexes.join(" "));
            lines.push(LogLine {
                commit_id,
                kind: LogLineKind::Merge {
                    parents: diff.parent_ids.clone(),
                },
                file_path: None,
                raw_text: merge_text,
            });
        }

        // 3. Author Line
        let safe_author_name =
            tigrs_core::ansi::strip_control_chars(&diff.author_name).into_owned();
        let safe_author_email =
            tigrs_core::ansi::strip_control_chars(&diff.author_email).into_owned();
        let author_text = format!("Author: {safe_author_name} <{safe_author_email}>");
        lines.push(LogLine {
            commit_id,
            kind: LogLineKind::Author {
                name: safe_author_name,
                email: safe_author_email,
            },
            file_path: None,
            raw_text: author_text,
        });

        // 4. Date Line
        let safe_author_date =
            tigrs_core::ansi::strip_control_chars(&diff.author_date).into_owned();
        let date_text = format!("Date:   {safe_author_date}");
        lines.push(LogLine {
            commit_id,
            kind: LogLineKind::Date {
                date: safe_author_date,
            },
            file_path: None,
            raw_text: date_text,
        });

        // 5. Blank line before commit message
        lines.push(LogLine {
            commit_id,
            kind: LogLineKind::Empty,
            file_path: None,
            raw_text: String::new(),
        });

        // 6. Subject (indented 4 spaces)
        let safe_title = tigrs_core::ansi::strip_control_chars(&diff.title).into_owned();
        let subject_text = format!("    {safe_title}");
        lines.push(LogLine {
            commit_id,
            kind: LogLineKind::Subject(safe_title),
            file_path: None,
            raw_text: subject_text,
        });

        // 7. Body (if present)
        if let Some(ref body) = diff.body {
            let trimmed = body.trim();
            if !trimmed.is_empty() {
                lines.push(LogLine {
                    commit_id,
                    kind: LogLineKind::Empty,
                    file_path: None,
                    raw_text: String::new(),
                });
                let safe_lines: Vec<String> = trimmed
                    .lines()
                    .map(|l| tigrs_core::ansi::strip_control_chars(l).into_owned())
                    .collect();
                let clean_refs: Vec<&str> = safe_lines
                    .iter()
                    .map(|l| l.strip_prefix("    ").unwrap_or(l.as_str()))
                    .collect();
                let trailer_flags = crate::diff::classify_commit_body_lines(&clean_refs);
                for (safe_line, is_trailer) in safe_lines.into_iter().zip(trailer_flags) {
                    let text = if safe_line.is_empty() {
                        String::new()
                    } else {
                        format!("    {safe_line}")
                    };
                    let kind = if is_trailer && !safe_line.trim().is_empty() {
                        LogLineKind::Trailer(safe_line)
                    } else {
                        LogLineKind::Body(safe_line)
                    };
                    lines.push(LogLine {
                        commit_id,
                        kind,
                        file_path: None,
                        raw_text: text,
                    });
                }
            }
        }

        // 8. Diffstat summary changes (if files are present)
        if !diff.files.is_empty() {
            // Blank line between commit message and diffstat
            lines.push(LogLine {
                commit_id,
                kind: LogLineKind::Empty,
                file_path: None,
                raw_text: String::new(),
            });

            // Determine column widths
            let max_path_len = diff
                .files
                .iter()
                .map(|f| {
                    UnicodeWidthStr::width(tigrs_core::ansi::strip_control_chars(&f.path).as_ref())
                })
                .max()
                .unwrap_or(0)
                .min(MAX_DIFFSTAT_PATH_WIDTH);

            let max_count = diff
                .files
                .iter()
                .map(|f| f.additions.saturating_add(f.deletions))
                .max()
                .unwrap_or(0);
            let count_width = format!("{max_count}").len().max(1);

            for file in &diff.files {
                let path = tigrs_core::ansi::strip_control_chars(&file.path).into_owned();
                let file_path_clone = Some(path.clone());

                if file.is_binary {
                    let raw_text = format!(" {path:<max_path_len$} | Bin");
                    lines.push(LogLine {
                        commit_id,
                        kind: LogLineKind::DiffStatFile {
                            path: path.clone(),
                            count: 0,
                            plus_count: 0,
                            minus_count: 0,
                            is_binary: true,
                        },
                        file_path: file_path_clone,
                        raw_text,
                    });
                } else {
                    let total = file.additions.saturating_add(file.deletions);
                    let (plus_count, minus_count) =
                        scale_diffstat_bars(file.additions, file.deletions, MAX_DIFFSTAT_BAR_WIDTH);
                    let bars = format!("{}{}", "+".repeat(plus_count), "-".repeat(minus_count));
                    let raw_text = format!(" {path:<max_path_len$} | {total:>count_width$} {bars}");
                    lines.push(LogLine {
                        commit_id,
                        kind: LogLineKind::DiffStatFile {
                            path: path.clone(),
                            count: total,
                            plus_count,
                            minus_count,
                            is_binary: false,
                        },
                        file_path: file_path_clone,
                        raw_text,
                    });
                }
            }

            // Summary line
            let files_changed = diff.stats.files_changed.max(diff.files.len());
            let files_str = if files_changed == 1 {
                "1 file changed".to_string()
            } else {
                format!("{files_changed} files changed")
            };

            let ins_str = if diff.stats.insertions > 0 {
                if diff.stats.insertions == 1 {
                    Some("1 insertion(+)".to_string())
                } else {
                    Some(format!("{} insertions(+)", diff.stats.insertions))
                }
            } else {
                None
            };

            let del_str = if diff.stats.deletions > 0 {
                if diff.stats.deletions == 1 {
                    Some("1 deletion(-)".to_string())
                } else {
                    Some(format!("{} deletions(-)", diff.stats.deletions))
                }
            } else {
                None
            };

            let summary_text = match (ins_str, del_str) {
                (Some(ins), Some(del)) => format!(" {files_str}, {ins}, {del}"),
                (Some(ins), None) => format!(" {files_str}, {ins}"),
                (None, Some(del)) => format!(" {files_str}, {del}"),
                (None, None) => format!(" {files_str}"),
            };

            lines.push(LogLine {
                commit_id,
                kind: LogLineKind::DiffStatSummary(summary_text.clone()),
                file_path: None,
                raw_text: summary_text,
            });
        }

        // Blank line separating this commit from the next
        lines.push(LogLine {
            commit_id,
            kind: LogLineKind::Empty,
            file_path: None,
            raw_text: String::new(),
        });
        lines
    }

    /// Moves cursor down by `n` lines.
    pub fn move_down(&mut self, n: usize, visible_height: usize) {
        self.nav.move_down_by(n, self.lines.len(), visible_height);
    }

    /// Moves cursor up by `n` lines.
    pub fn move_up(&mut self, n: usize, visible_height: usize) {
        self.nav.move_up_by(n, visible_height);
    }

    /// Moves cursor page down.
    pub fn page_down(&mut self, visible_height: usize) {
        self.nav.page_down(self.lines.len(), visible_height);
    }

    /// Moves cursor page up.
    pub fn page_up(&mut self, visible_height: usize) {
        self.nav.page_up(visible_height);
    }

    /// Scrolls to the very top.
    pub fn home(&mut self) {
        self.nav.scroll_top();
    }

    /// Scrolls to the very end.
    pub fn end(&mut self, visible_height: usize) {
        self.nav.scroll_bottom(self.lines.len(), visible_height);
    }

    /// Sets cursor to a specific line index.
    pub fn set_cursor(&mut self, pos: usize, visible_height: usize) {
        self.nav.set_cursor(pos, self.lines.len(), visible_height);
    }

    /// Scrolls viewport down by `n` lines while keeping cursor inside viewport.
    pub fn scroll_line_down(&mut self, n: usize, visible_height: usize) {
        self.nav
            .scroll_line_down(n, self.lines.len(), visible_height);
    }

    /// Scrolls viewport up by `n` lines while keeping cursor inside viewport.
    pub fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.nav.scroll_line_up(n, visible_height);
    }

    /// Jumps cursor to the commit header with object ID matching `id`.
    pub fn jump_to_commit(&mut self, id: &ObjectId, visible_height: usize) -> bool {
        for &idx in &self.commit_indices {
            if let Some(line) = self.lines.get(idx)
                && line.commit_id == *id
            {
                self.set_cursor(idx, visible_height);
                return true;
            }
        }
        false
    }

    /// Returns searchable text for a line index.
    pub fn row_text(&self, idx: usize) -> Option<String> {
        self.lines.get(idx).map(|l| l.raw_text.clone())
    }

    /// Renders the log view with title bar, content lines, and status bar.
    pub fn render<W: Write>(&self, w: &mut W, width: usize, height: usize) -> Result<()> {
        if height < 3 || width < 10 {
            return Ok(());
        }

        let content_height = height.saturating_sub(2);
        let start = self.scroll_offset();
        let end = (start + content_height).min(self.lines.len());

        // 1. Title bar (Row 0)
        queue!(w, MoveTo(0, 0), SetAttribute(Attribute::Bold))?;
        let selected_short_oid = self.selected_commit_id().map(|oid| {
            let hex = oid.to_hex().to_string();
            hex[..7.min(hex.len())].to_string()
        });
        let title_raw = match selected_short_oid {
            Some(short_oid) => {
                format!(
                    " [log] {} - commit {} (line {} of {})",
                    self.branch_name,
                    short_oid,
                    if self.lines.is_empty() {
                        0
                    } else {
                        self.cursor() + 1
                    },
                    self.lines.len()
                )
            }
            None => format!(" [log] {} - 0 commits", self.branch_name),
        };
        let title_display = truncate_display_width(&title_raw, width);
        let title_width = UnicodeWidthStr::width(title_display);
        write!(w, "{title_display}")?;
        super::write_line_el_or_pad(w, width.saturating_sub(title_width), false)?;
        queue!(w, SetAttribute(Attribute::Reset))?;

        // 2. Viewport Content (Rows 1 .. height - 1)
        if self.lines.is_empty() {
            for row_idx in 0..content_height {
                queue!(w, MoveTo(0, (row_idx + 1) as u16))?;
                if row_idx == 0 {
                    let msg = "  No commits found.";
                    write!(w, "{msg}")?;
                    super::write_line_el_or_pad(w, width.saturating_sub(msg.len()), false)?;
                } else {
                    super::write_line_el_or_pad(w, width, false)?;
                }
            }
        } else {
            for row_idx in 0..content_height {
                let item_idx = start + row_idx;
                queue!(w, MoveTo(0, (row_idx + 1) as u16))?;

                if item_idx < end {
                    let line = &self.lines[item_idx];
                    let is_selected = item_idx == self.cursor();

                    if is_selected {
                        queue!(w, SetAttribute(Attribute::Reverse))?;
                        let display_text = truncate_display_width(&line.raw_text, width);
                        write!(w, "{display_text}")?;
                        let rem = width.saturating_sub(UnicodeWidthStr::width(display_text));
                        super::write_line_el_or_pad(w, rem, true)?;
                        queue!(w, SetAttribute(Attribute::Reset))?;
                    } else {
                        Self::render_styled_line(w, line, width)?;
                    }
                } else {
                    super::write_line_el_or_pad(w, width, false)?;
                }
            }
        }

        // 3. Status bar (Row height - 1)
        queue!(
            w,
            MoveTo(0, (height - 1) as u16),
            SetAttribute(Attribute::Reverse)
        )?;
        let total = self.lines.len();
        let current = if total == 0 { 0 } else { self.cursor() + 1 };
        let pct = (current * 100).checked_div(total).unwrap_or(100);
        let status_left = if let Some((file, _)) = self.selected_file() {
            format!(
                " [{}] line {current} of {total} ({pct}%) - file: {file} - press 'Enter' for diff, 'e' to edit",
                self.branch_name
            )
        } else {
            format!(
                " [{}] line {current} of {total} ({pct}%) - press 'q' to quit, 'Enter' for diff, 'j'/'k' to move",
                self.branch_name
            )
        };
        let display_status = truncate_display_width(&status_left, width);
        write!(w, "{display_status}")?;
        let rem = width.saturating_sub(UnicodeWidthStr::width(display_status));
        super::write_line_el_or_pad(w, rem, true)?;
        queue!(w, SetAttribute(Attribute::Reset))?;

        Ok(())
    }

    /// Renders a single non-selected line with syntax highlighting.
    fn render_styled_line<W: Write>(w: &mut W, line: &LogLine, width: usize) -> Result<()> {
        match &line.kind {
            LogLineKind::Commit { oid, refs } => {
                let prefix = "commit ";
                let prefix_display = truncate_display_width(prefix, width);
                let mut used_w = UnicodeWidthStr::width(prefix_display);
                queue!(
                    w,
                    SetForegroundColor(Color::Green),
                    SetAttribute(Attribute::Bold)
                )?;
                write!(w, "{prefix_display}")?;

                let oid_hex = oid.to_string();
                let oid_display = truncate_display_width(&oid_hex, width.saturating_sub(used_w));
                used_w += UnicodeWidthStr::width(oid_display);
                queue!(w, SetForegroundColor(Color::Yellow))?;
                write!(w, "{oid_display}")?;

                if !refs.is_empty() {
                    let refs_display = truncate_display_width(refs, width.saturating_sub(used_w));
                    used_w += UnicodeWidthStr::width(refs_display);
                    queue!(w, SetForegroundColor(Color::Cyan))?;
                    write!(w, "{refs_display}")?;
                }
                queue!(w, ResetColor, SetAttribute(Attribute::Reset))?;
                super::write_line_el_or_pad(w, width.saturating_sub(used_w), false)?;
            }
            LogLineKind::Merge { .. } => {
                let display = truncate_display_width(&line.raw_text, width);
                queue!(w, SetForegroundColor(Color::Cyan))?;
                write!(w, "{display}")?;
                queue!(w, ResetColor)?;
                let text_w = UnicodeWidthStr::width(display);
                super::write_line_el_or_pad(w, width.saturating_sub(text_w), false)?;
            }
            LogLineKind::Author { .. } => {
                let prefix = "Author: ";
                let prefix_display = truncate_display_width(prefix, width);
                let mut used_w = UnicodeWidthStr::width(prefix_display);
                queue!(w, SetForegroundColor(Color::Cyan))?;
                write!(w, "{prefix_display}")?;
                queue!(w, ResetColor)?;
                let rest = line.raw_text.strip_prefix("Author: ").unwrap_or("");
                let rest_display = truncate_display_width(rest, width.saturating_sub(used_w));
                used_w += UnicodeWidthStr::width(rest_display);
                write!(w, "{rest_display}")?;
                super::write_line_el_or_pad(w, width.saturating_sub(used_w), false)?;
            }
            LogLineKind::Date { .. } => {
                let prefix = "Date:   ";
                let prefix_display = truncate_display_width(prefix, width);
                let mut used_w = UnicodeWidthStr::width(prefix_display);
                queue!(w, SetForegroundColor(Color::Yellow))?;
                write!(w, "{prefix_display}")?;
                queue!(w, ResetColor)?;
                let rest = line.raw_text.strip_prefix("Date:   ").unwrap_or("");
                let rest_display = truncate_display_width(rest, width.saturating_sub(used_w));
                used_w += UnicodeWidthStr::width(rest_display);
                write!(w, "{rest_display}")?;
                super::write_line_el_or_pad(w, width.saturating_sub(used_w), false)?;
            }
            LogLineKind::Subject(_) => {
                queue!(w, SetAttribute(Attribute::Bold))?;
                let display = truncate_display_width(&line.raw_text, width);
                write!(w, "{display}")?;
                queue!(w, SetAttribute(Attribute::Reset))?;
                let text_w = UnicodeWidthStr::width(display);
                super::write_line_el_or_pad(w, width.saturating_sub(text_w), false)?;
            }
            LogLineKind::Body(_) => {
                let display = truncate_display_width(&line.raw_text, width);
                write!(w, "{display}")?;
                let text_w = UnicodeWidthStr::width(display);
                super::write_line_el_or_pad(w, width.saturating_sub(text_w), false)?;
            }
            LogLineKind::Trailer(_) => {
                let display = truncate_display_width(&line.raw_text, width);
                queue!(w, SetForegroundColor(Color::Yellow))?;
                write!(w, "{display}")?;
                queue!(w, ResetColor)?;
                let text_w = UnicodeWidthStr::width(display);
                super::write_line_el_or_pad(w, width.saturating_sub(text_w), false)?;
            }
            LogLineKind::DiffStatFile {
                path,
                plus_count,
                minus_count,
                is_binary,
                ..
            } => {
                // Locate the diffstat separator `|` strictly after ` <path>`, so a
                // file path containing `|`, `+`, or `-` cannot spoof the diffstat
                // bar rendering (CWE-451).
                let path_prefix_end = (1 + path.len()).min(line.raw_text.len());
                if let Some(rel_pipe) = line.raw_text[path_prefix_end..].find('|') {
                    let pipe_pos = path_prefix_end + rel_pipe;
                    let before_pipe = &line.raw_text[..pipe_pos];
                    let after_pipe = &line.raw_text[pipe_pos + 1..];

                    let path_display = truncate_display_width(before_pipe, width);
                    let mut used_w = UnicodeWidthStr::width(path_display);
                    write!(w, "{path_display}")?;

                    let pipe_display = truncate_display_width("|", width.saturating_sub(used_w));
                    used_w += UnicodeWidthStr::width(pipe_display);
                    queue!(w, SetForegroundColor(Color::DarkGrey))?;
                    write!(w, "{pipe_display}")?;
                    queue!(w, ResetColor)?;

                    if *is_binary {
                        let bin_display =
                            truncate_display_width(after_pipe, width.saturating_sub(used_w));
                        used_w += UnicodeWidthStr::width(bin_display);
                        queue!(w, SetForegroundColor(Color::Magenta))?;
                        write!(w, "{bin_display}")?;
                        queue!(w, ResetColor)?;
                    } else {
                        let total_bars = plus_count.saturating_add(*minus_count);
                        let count_part_len = after_pipe.len().saturating_sub(total_bars);
                        let count_part = &after_pipe[..count_part_len];
                        let count_display =
                            truncate_display_width(count_part, width.saturating_sub(used_w));
                        used_w += UnicodeWidthStr::width(count_display);
                        write!(w, "{count_display}")?;

                        if *plus_count > 0 {
                            let plus_str = "+".repeat(*plus_count);
                            let plus_display =
                                truncate_display_width(&plus_str, width.saturating_sub(used_w));
                            used_w += UnicodeWidthStr::width(plus_display);
                            queue!(w, SetForegroundColor(Color::Green))?;
                            write!(w, "{plus_display}")?;
                        }
                        if *minus_count > 0 {
                            let minus_str = "-".repeat(*minus_count);
                            let minus_display =
                                truncate_display_width(&minus_str, width.saturating_sub(used_w));
                            used_w += UnicodeWidthStr::width(minus_display);
                            queue!(w, SetForegroundColor(Color::Red))?;
                            write!(w, "{minus_display}")?;
                        }
                        queue!(w, ResetColor)?;
                    }
                    super::write_line_el_or_pad(w, width.saturating_sub(used_w), false)?;
                } else {
                    let display = truncate_display_width(&line.raw_text, width);
                    write!(w, "{display}")?;
                    let text_w = UnicodeWidthStr::width(display);
                    super::write_line_el_or_pad(w, width.saturating_sub(text_w), false)?;
                }
            }
            LogLineKind::DiffStatSummary(_) => {
                let display = truncate_display_width(&line.raw_text, width);
                queue!(w, SetForegroundColor(Color::Cyan))?;
                write!(w, "{display}")?;
                queue!(w, ResetColor)?;
                let text_w = UnicodeWidthStr::width(display);
                super::write_line_el_or_pad(w, width.saturating_sub(text_w), false)?;
            }
            LogLineKind::Empty => {
                super::write_line_el_or_pad(w, width, false)?;
            }
        }
        Ok(())
    }
}

/// Computes the number of `+` and `-` glyphs for the diffstat bar, scaling down
/// proportionally if `additions + deletions > max_bar_width`.
pub fn scale_diffstat_bars(
    additions: usize,
    deletions: usize,
    max_bar_width: usize,
) -> (usize, usize) {
    let add_u128 = additions as u128;
    let del_u128 = deletions as u128;
    let max_u128 = max_bar_width as u128;
    let total = add_u128.saturating_add(del_u128);
    if total == 0 || max_bar_width == 0 {
        return (0, 0);
    }
    if total <= max_u128 {
        return (additions, deletions);
    }

    let plus = if additions > 0 {
        ((add_u128.saturating_mul(max_u128) / total) as usize).max(1)
    } else {
        0
    };
    let minus = if deletions > 0 {
        ((del_u128.saturating_mul(max_u128) / total) as usize).max(1)
    } else {
        0
    };

    if plus.saturating_add(minus) > max_bar_width {
        if plus >= minus {
            (max_bar_width.saturating_sub(minus), minus)
        } else {
            (plus, max_bar_width.saturating_sub(plus))
        }
    } else {
        (plus, minus)
    }
}

impl super::View for LogView {
    fn kind(&self) -> crate::app::layout::ViewKind {
        crate::app::layout::ViewKind::Log
    }

    fn line_count(&self) -> usize {
        self.line_count()
    }

    fn nav(&self) -> &ViewportCursor {
        &self.nav
    }

    fn nav_mut(&mut self) -> &mut ViewportCursor {
        &mut self.nav
    }

    fn render(
        &self,
        mut writer: &mut dyn std::io::Write,
        width: u16,
        height: u16,
        _options: &crate::options::ViewOptions,
    ) -> tigrs_core::error::Result<()> {
        self.render(&mut writer, width as usize, height as usize)?;
        Ok(())
    }

    fn matches_search(&self, index: usize, pat: &crate::search::SearchPattern) -> bool {
        self.row_text(index).is_some_and(|text| pat.is_match(&text))
    }

    fn selected_commit_id(&self) -> Option<tigrs_git::ObjectId> {
        self.selected_commit_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tigrs_git::{DiffSummaryStats, FileChangeStatus, FileDiff};

    fn sample_commit_diff() -> CommitDiff {
        let oid = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
        let parent = ObjectId::from_hex(b"0000000000000000000000000000000000000000").unwrap();
        CommitDiff {
            commit_id: oid,
            parent_ids: vec![parent],
            author_name: Arc::from("Alice Developer"),
            author_email: Arc::from("alice@example.com"),
            author_date: "Mon Jan 1 12:00:00 2026 +0000".to_string(),
            committer_name: Arc::from("Alice Developer"),
            committer_email: Arc::from("alice@example.com"),
            committer_date: "Mon Jan 1 12:00:00 2026 +0000".to_string(),
            title: Arc::from("feat(core): implement log view summary changes"),
            body: Some(
                "This commit adds full summary changes parity.\nIt scales diffstats correctly."
                    .to_string(),
            ),
            files: vec![
                FileDiff {
                    path: "src/log.rs".to_string(),
                    status: FileChangeStatus::Modified,
                    old_id: None,
                    new_id: None,
                    old_mode: None,
                    new_mode: None,
                    is_binary: false,
                    additions: 25,
                    deletions: 5,
                    hunks: Vec::new(),
                },
                FileDiff {
                    path: "images/logo.png".to_string(),
                    status: FileChangeStatus::Added,
                    old_id: None,
                    new_id: None,
                    old_mode: None,
                    new_mode: None,
                    is_binary: true,
                    additions: 0,
                    deletions: 0,
                    hunks: Vec::new(),
                },
            ],
            stats: DiffSummaryStats {
                files_changed: 2,
                insertions: 25,
                deletions: 5,
            },
        }
    }

    #[test]
    fn test_scale_diffstat_bars() {
        assert_eq!(scale_diffstat_bars(10, 5, 35), (10, 5));
        assert_eq!(scale_diffstat_bars(0, 0, 35), (0, 0));
        assert_eq!(scale_diffstat_bars(100, 0, 35), (35, 0));
        assert_eq!(scale_diffstat_bars(0, 100, 35), (0, 35));
        let (p, m) = scale_diffstat_bars(500, 500, 35);
        assert!(p + m <= 35);
        assert!(p > 0 && m > 0);
    }

    #[test]
    fn test_log_view_commit_formatting_and_summary_changes() {
        let mut view = LogView::new("main");
        assert!(view.is_empty());
        assert_eq!(view.branch_name(), "main");

        let diff = sample_commit_diff();
        let ref_names = vec!["HEAD".to_string(), "main".to_string()];
        view.append_commit_diff(&diff, Some(&ref_names));

        assert!(!view.is_empty());
        assert_eq!(view.commit_indices().len(), 1);
        assert_eq!(view.selected_commit_id(), Some(diff.commit_id));

        // Check header line
        let first_line = &view.lines[0];
        assert!(first_line.raw_text.contains("commit 1111111"));
        assert!(first_line.raw_text.contains("(HEAD, main)"));

        // Check author and date
        assert!(
            view.lines
                .iter()
                .any(|l| l.raw_text.contains("Author: Alice Developer"))
        );
        assert!(
            view.lines
                .iter()
                .any(|l| l.raw_text.contains("Date:   Mon Jan 1"))
        );

        // Check subject and body
        assert!(
            view.lines
                .iter()
                .any(|l| l.raw_text.contains("feat(core): implement log view"))
        );
        assert!(
            view.lines
                .iter()
                .any(|l| l.raw_text.contains("This commit adds full summary"))
        );

        // Check diffstat file lines
        let stat_file = view
            .lines
            .iter()
            .find(|l| l.file_path.as_deref() == Some("src/log.rs"))
            .unwrap();
        assert!(stat_file.raw_text.contains("src/log.rs"));
        assert!(stat_file.raw_text.contains('|'));
        assert!(stat_file.raw_text.contains("30"));
        assert!(stat_file.raw_text.contains('+'));
        assert!(stat_file.raw_text.contains('-'));

        // Check binary file line
        let bin_file = view
            .lines
            .iter()
            .find(|l| l.file_path.as_deref() == Some("images/logo.png"))
            .unwrap();
        assert!(bin_file.raw_text.contains("images/logo.png"));
        assert!(bin_file.raw_text.contains("Bin"));

        // Check diffstat summary line
        let summary_line = view
            .lines
            .iter()
            .find(|l| matches!(l.kind, LogLineKind::DiffStatSummary(_)))
            .unwrap();
        assert!(
            summary_line
                .raw_text
                .contains("2 files changed, 25 insertions(+), 5 deletions(-)")
        );
    }

    #[test]
    fn test_log_view_navigation_and_selection() {
        let mut view = LogView::new("main");
        let diff = sample_commit_diff();
        view.append_commit_diff(&diff, None);

        let total = view.line_count();
        assert!(total > 5);

        // Move down and up
        view.move_down(2, 20);
        assert_eq!(view.cursor(), 2);
        view.move_up(1, 20);
        assert_eq!(view.cursor(), 1);

        // Home and end
        view.end(20);
        assert_eq!(view.cursor(), total - 1);
        view.home();
        assert_eq!(view.cursor(), 0);

        // Selected file when resting on a diffstat line
        let stat_idx = view
            .lines
            .iter()
            .position(|l| l.file_path.as_deref() == Some("src/log.rs"))
            .unwrap();
        view.set_cursor(stat_idx, 20);
        assert_eq!(view.selected_file(), Some(("src/log.rs", diff.commit_id)));

        // Jump to commit
        let jumped = view.jump_to_commit(&diff.commit_id, 20);
        assert!(jumped);
        assert_eq!(view.cursor(), 0);
    }

    #[test]
    fn test_log_view_render_to_buffer() {
        let mut view = LogView::new("main");
        let diff = sample_commit_diff();
        view.append_commit_diff(&diff, Some(&["main".to_string()]));

        let mut buf = Vec::new();
        let res = view.render(&mut buf, 80, 24);
        assert!(res.is_ok());
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("[log]"));
        assert!(output.contains("commit 1111111"));
        assert!(output.contains("src/log.rs"));
        assert!(output.contains("2 files changed"));
    }

    #[test]
    fn test_log_view_merge_commits_clear_and_navigation() {
        let mut view = LogView::new("main");
        let mut diff1 = sample_commit_diff();
        // Add 2 parents to test Merge header rendering
        diff1.parent_ids = vec![
            ObjectId::from_bytes_or_panic(&[0xaa; 20]),
            ObjectId::from_bytes_or_panic(&[0xbb; 20]),
        ];
        view.append_commit_diff(&diff1, None);

        // Append second commit with deletions-only stats
        let mut diff2 = sample_commit_diff();
        diff2.commit_id = ObjectId::from_bytes_or_panic(&[0x22; 20]);
        diff2.stats.insertions = 0;
        diff2.stats.deletions = 7;
        view.append_commit_diff(&diff2, None);

        // Append third commit with 0 insertions and 0 deletions
        let mut diff3 = sample_commit_diff();
        diff3.commit_id = ObjectId::from_bytes_or_panic(&[0x33; 20]);
        diff3.stats.insertions = 0;
        diff3.stats.deletions = 0;
        view.append_commit_diff(&diff3, None);

        // Render and verify Merge line is rendered
        let mut buf = Vec::new();
        view.render(&mut buf, 80, 40).unwrap();
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("Merge: aaaaaaa bbbbbbb"));
        assert!(out.contains("7 deletions(-)"));

        // Page down / up and scroll down / up
        view.page_down(5);
        assert!(view.cursor() > 0);
        view.page_up(5);
        view.scroll_line_down(2, 5);
        assert!(view.scroll_offset() > 0);
        view.scroll_line_up(1, 5);

        // Clear
        view.clear();
        assert_eq!(view.line_count(), 0);
    }

    #[test]
    fn test_log_view_commit_trailer_classification_and_highlight() {
        let mut view = LogView::new("main");
        let mut diff = sample_commit_diff();
        diff.body = Some(
            "Detailed explanation of the fix.\nNote: this body line is not a trailer.\n\nTested: cargo test --workspace\nTracker-Bug-Id: 424242\nChange-Id: I1234567890abcdef\nSigned-off-by: Alice Developer <alice@example.com>".to_string(),
        );
        view.append_commit_diff(&diff, None);

        assert!(view.lines.iter().any(|l| {
            matches!(&l.kind, LogLineKind::Body(s) if s == "Note: this body line is not a trailer.")
        }));
        for expected_trailer in [
            "Tested: cargo test --workspace",
            "Tracker-Bug-Id: 424242",
            "Change-Id: I1234567890abcdef",
            "Signed-off-by: Alice Developer <alice@example.com>",
        ] {
            assert!(
                view.lines.iter().any(|l| {
                    matches!(&l.kind, LogLineKind::Trailer(s) if s == expected_trailer)
                }),
                "Expected trailer line {expected_trailer:?} to be classified as LogLineKind::Trailer"
            );
        }
    }
}
