// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Interactive blame view for displaying per-line commit annotations and history navigation.

use super::ViewportCursor;
use crate::highlight::{IncrementalHighlighter, MAX_HIGHLIGHT_LINES};
use std::io::Write;
use std::sync::{Arc, Mutex};
use tigrs_core::ansi::truncate_display_width;
use tigrs_core::error::Result;
use tigrs_git::{BlameLine, BlameResult, BlobContent, ObjectId};

/// A saved state in the blame navigation history (for parent commit navigation).
#[derive(Debug, Clone)]
pub struct BlameHistoryEntry {
    /// Commit ID previously blamed.
    pub commit_oid: ObjectId,
    /// Path previously blamed.
    pub path: String,
    /// Cursor line index (0-indexed).
    pub cursor_line: usize,
    /// Scroll offset previously visible.
    pub scroll_offset: usize,
}

/// View displaying file annotations with line-by-line commit information.
#[derive(Debug)]
pub struct BlameView {
    /// Active commit ID being blamed.
    commit_oid: ObjectId,
    /// Path of the file relative to repository root.
    path: String,
    /// All blamed lines in order.
    lines: Vec<BlameLine>,
    /// Incremental highlighter, None if binary or all lines highlighted or exceeds max.
    highlighter: Mutex<Option<IncrementalHighlighter>>,
    /// Syntax-highlighted lines (ANSI `TrueColor` escaped).
    highlighted_lines: Mutex<Vec<String>>,
    /// Active cursor line index and viewport scroll.
    nav: ViewportCursor,
    /// Whether background blame calculation is still running.
    is_loading: bool,
    /// Whether the file is binary.
    is_binary: bool,
    /// Transient status message displayed in status bar.
    status_message: Option<String>,
    /// Navigation history stack for parent jumping (`,`) and jumping back.
    history: Vec<BlameHistoryEntry>,
}

impl BlameView {
    /// Creates a `BlameView` directly from a finished `BlameResult`.
    pub fn from_result(result: BlameResult) -> Self {
        let total = result.lines.len();
        let (highlighter, highlighted_lines) = if result.is_binary || total == 0 {
            (None, Vec::new())
        } else if total > MAX_HIGHLIGHT_LINES {
            (
                None,
                result.lines.iter().map(|l| l.content.clone()).collect(),
            )
        } else {
            let mut inc = IncrementalHighlighter::new(&result.path);
            let initial_count = total.min(128);
            let mut hl = Vec::with_capacity(total);
            for line in &result.lines[..initial_count] {
                hl.push(inc.highlight_next(&line.content));
            }
            let hl_opt = if initial_count < total {
                Some(inc)
            } else {
                None
            };
            (hl_opt, hl)
        };

        Self {
            commit_oid: result.commit_id,
            path: tigrs_core::ansi::strip_control_chars(&result.path).into_owned(),
            lines: result.lines,
            highlighter: Mutex::new(highlighter),
            highlighted_lines: Mutex::new(highlighted_lines),
            nav: ViewportCursor::new(),
            is_loading: false,
            is_binary: result.is_binary,
            status_message: None,
            history: Vec::new(),
        }
    }

    /// Creates an immediate placeholder `BlameView` from file `BlobContent`
    /// while background blame computes.
    pub fn from_blob(commit_oid: ObjectId, blob: BlobContent) -> Self {
        let is_binary = blob.is_binary;
        let raw_path = blob.path;
        let path = tigrs_core::ansi::strip_control_chars(&raw_path).into_owned();
        let total = blob.lines.len();
        let (highlighter, highlighted_lines) =
            if is_binary || total == 0 || total > MAX_HIGHLIGHT_LINES {
                (None, Vec::new())
            } else {
                let mut inc = IncrementalHighlighter::new(&path);
                let initial_count = total.min(128);
                let mut hl = Vec::with_capacity(total);
                for idx in 0..initial_count {
                    hl.push(inc.highlight_next(&blob.lines[idx]));
                }
                let hl_opt = if initial_count < total {
                    Some(inc)
                } else {
                    None
                };
                (hl_opt, hl)
            };

        let placeholder_lines: Vec<BlameLine> = blob
            .lines
            .iter()
            .enumerate()
            .map(|(idx, content)| BlameLine {
                line_number: idx + 1,
                commit_id: commit_oid,
                short_commit_id: Arc::from("........"),
                author: Arc::from("loading..."),
                author_date: Arc::from("....-..-.."),
                summary: Arc::from(""),
                content: content.to_string(),
                is_hunk_start: idx == 0,
                parent_commit_id: None,
                source_path: None,
                source_line_number: idx + 1,
            })
            .collect();

        Self {
            commit_oid,
            path,
            lines: placeholder_lines,
            highlighter: Mutex::new(highlighter),
            highlighted_lines: Mutex::new(highlighted_lines),
            nav: ViewportCursor::new(),
            is_loading: true,
            is_binary,
            status_message: None,
            history: Vec::new(),
        }
    }

    /// Applies computed blame results to the view when available.
    pub fn apply_blame_result(&mut self, result: BlameResult) {
        if result.commit_id != self.commit_oid || result.path != self.path {
            return;
        }

        self.is_binary = result.is_binary;
        self.is_loading = false;

        if !result.is_binary {
            let total = result.lines.len();
            let (highlighter, highlighted_lines) = if total == 0 {
                (None, Vec::new())
            } else if total > MAX_HIGHLIGHT_LINES {
                (
                    None,
                    result.lines.iter().map(|l| l.content.clone()).collect(),
                )
            } else {
                let mut inc = IncrementalHighlighter::new(&result.path);
                let initial_count = total.min(128);
                let mut hl = Vec::with_capacity(total);
                for line in &result.lines[..initial_count] {
                    hl.push(inc.highlight_next(&line.content));
                }
                let hl_opt = if initial_count < total {
                    Some(inc)
                } else {
                    None
                };
                (hl_opt, hl)
            };

            let mut highlighter_guard = match self.highlighter.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            *highlighter_guard = highlighter;

            let mut highlighted_lines_guard = match self.highlighted_lines.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            *highlighted_lines_guard = highlighted_lines;

            self.lines = result.lines;
        }

        if self.lines.is_empty() {
            self.nav.cursor = 0;
            self.nav.scroll_offset = 0;
        } else {
            let max_idx = self.lines.len().saturating_sub(1);
            self.nav.cursor = self.nav.cursor.min(max_idx);
            self.nav.scroll_offset = self.nav.scroll_offset.min(self.nav.cursor);
        }
    }

    /// Rebuilds syntax highlighting for all visible lines when `syntax_highlighting` or `syntax_theme` changes.
    pub fn refresh_highlighting(&mut self, options: &crate::options::ViewOptions) {
        let total = self.lines.len();
        if !options.syntax_highlighting
            || self.is_binary
            || total == 0
            || total > MAX_HIGHLIGHT_LINES
        {
            if let Ok(mut g) = self.highlighter.lock() {
                *g = None;
            }
            if let Ok(mut lines_g) = self.highlighted_lines.lock() {
                lines_g.clear();
            }
            return;
        }

        let mut inc = IncrementalHighlighter::with_theme(&self.path, &options.syntax_theme);
        let initial_count = (self.scroll_offset() + 128).min(total);
        let mut hl = Vec::with_capacity(total);
        for line in &self.lines[..initial_count] {
            hl.push(inc.highlight_next(&line.content));
        }
        let hl_opt = if initial_count < total {
            Some(inc)
        } else {
            None
        };
        if let Ok(mut g) = self.highlighter.lock() {
            *g = hl_opt;
        }
        if let Ok(mut lines_g) = self.highlighted_lines.lock() {
            *lines_g = hl;
        }
    }

    /// Ensures that syntax highlighting is computed up to `target_line` (inclusive).
    pub fn ensure_highlighted(&self, target_line: usize) {
        let mut highlighter_guard = match self.highlighter.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(highlighter) = highlighter_guard.as_mut() else {
            return;
        };

        let mut lines_guard = match self.highlighted_lines.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };

        let current_count = lines_guard.len();
        let target = target_line.min(self.lines.len());
        if current_count >= target {
            return;
        }

        for idx in current_count..target {
            lines_guard.push(highlighter.highlight_next(&self.lines[idx].content));
        }

        if lines_guard.len() >= self.lines.len() {
            *highlighter_guard = None;
        }
    }

    /// Returns the number of lines currently highlighted.
    #[must_use]
    pub fn highlighted_lines_len(&self) -> usize {
        self.highlighted_lines.lock().map_or(0, |l| l.len())
    }

    /// Returns the commit ID being blamed.
    pub fn commit_oid(&self) -> ObjectId {
        self.commit_oid
    }

    /// Returns the slice of blame lines.
    pub fn lines(&self) -> &[BlameLine] {
        &self.lines
    }

    /// Returns the file path being blamed.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns whether the view is still loading annotations in the background.
    pub fn is_loading(&self) -> bool {
        self.is_loading
    }

    /// Returns whether the file is binary.
    pub fn is_binary(&self) -> bool {
        self.is_binary
    }

    /// Returns total lines count.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Returns active cursor index (0-indexed).
    pub fn cursor(&self) -> usize {
        self.nav.cursor()
    }

    /// Returns the currently selected `BlameLine`, if any.
    pub fn selected_line(&self) -> Option<&BlameLine> {
        self.lines.get(self.nav.cursor())
    }

    /// Returns the commit ID responsible for the currently selected line.
    pub fn current_commit_id(&self) -> Option<ObjectId> {
        self.selected_line().map(|l| l.commit_id)
    }

    /// Returns the parent commit ID responsible for the currently selected line.
    pub fn current_parent_commit_id(&self) -> Option<ObjectId> {
        self.selected_line().and_then(|l| l.parent_commit_id)
    }

    /// Sets a transient status message.
    pub fn set_status_message(&mut self, msg: String) {
        self.status_message = Some(msg);
    }

    /// Clears any transient status message.
    pub fn clear_status_message(&mut self) {
        self.status_message = None;
    }

    /// Pushes the current position onto the navigation history stack.
    pub fn push_history(&mut self) {
        self.history.push(BlameHistoryEntry {
            commit_oid: self.commit_oid,
            path: self.path.clone(),
            cursor_line: self.cursor(),
            scroll_offset: self.scroll_offset(),
        });
    }

    /// Pops the top history entry from the navigation history stack.
    pub fn pop_history(&mut self) -> Option<BlameHistoryEntry> {
        self.history.pop()
    }

    /// Returns a reference to the top history entry without popping it.
    pub fn peek_history(&self) -> Option<&BlameHistoryEntry> {
        self.history.last()
    }

    /// Sets cursor line index directly, clamping to valid line bounds.
    pub fn set_cursor(&mut self, cursor: usize, visible_height: usize) {
        self.nav
            .set_cursor(cursor, self.lines.len(), visible_height);
    }

    /// Returns true if there are prior entries in navigation history.
    pub fn has_history(&self) -> bool {
        !self.history.is_empty()
    }

    /// Prepares view to blame a new commit/path, recording history.
    pub fn navigate_to(&mut self, commit_oid: ObjectId, path: String, target_line: usize) {
        self.push_history();
        self.commit_oid = commit_oid;
        self.path = path;
        self.nav = ViewportCursor::with_coordinates(target_line, target_line.saturating_sub(5));
        self.is_loading = true;
        self.clear_status_message();
    }

    /// Restores blame state to a previous history entry without pushing onto the history stack.
    pub fn navigate_back_to(
        &mut self,
        prev: BlameHistoryEntry,
        result: BlameResult,
        visible_height: usize,
    ) {
        self.commit_oid = prev.commit_oid;
        self.path = prev.path;
        self.clear_status_message();
        self.apply_blame_result(result);
        self.set_cursor(prev.cursor_line, visible_height);
    }

    /// Moves cursor up by one line.
    pub fn move_up(&mut self) {
        self.clear_status_message();
        self.nav.move_up();
    }

    /// Moves cursor up by `n` lines and adjusts scroll.
    pub fn move_up_by(&mut self, n: usize, visible_height: usize) {
        self.clear_status_message();
        self.nav.move_up_by(n, visible_height);
    }

    /// Moves cursor down by one line.
    pub fn move_down(&mut self, visible_height: usize) {
        self.clear_status_message();
        self.nav.move_down(self.lines.len(), visible_height);
    }

    /// Moves cursor down by `n` lines and adjusts scroll.
    pub fn move_down_by(&mut self, n: usize, visible_height: usize) {
        self.clear_status_message();
        self.nav.move_down_by(n, self.lines.len(), visible_height);
    }

    /// Moves cursor up by one page.
    pub fn page_up(&mut self, visible_height: usize) {
        self.clear_status_message();
        self.nav.page_up(visible_height);
    }

    /// Moves cursor down by one page.
    pub fn page_down(&mut self, visible_height: usize) {
        self.clear_status_message();
        self.nav.page_down(self.lines.len(), visible_height);
    }

    /// Moves cursor to top of file.
    pub fn move_to_top(&mut self) {
        self.clear_status_message();
        self.nav.scroll_top();
    }

    /// Moves cursor to bottom of file.
    pub fn move_to_bottom(&mut self, visible_height: usize) {
        self.clear_status_message();
        self.nav.scroll_bottom(self.lines.len(), visible_height);
    }

    /// Returns the current vertical scroll offset.
    #[inline]
    #[must_use]
    pub fn scroll_offset(&self) -> usize {
        self.nav.scroll_offset()
    }

    /// Scrolls the viewport down by `n` lines, keeping cursor within the viewport.
    pub fn scroll_line_down(&mut self, n: usize, visible_height: usize) {
        self.clear_status_message();
        self.nav
            .scroll_line_down(n, self.lines.len(), visible_height);
    }

    /// Scrolls the viewport up by `n` lines, keeping cursor within the viewport.
    pub fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.clear_status_message();
        self.nav.scroll_line_up(n, visible_height);
    }

    /// Renders the blame view to the terminal output writer using default options.
    pub fn render<W: Write>(&self, writer: &mut W, width: u16, height: u16) -> Result<()> {
        self.render_with_options(
            writer,
            width,
            height,
            &crate::options::ViewOptions::default(),
        )
    }

    /// Renders the blame view with specified display options.
    pub fn render_with_options<W: Write>(
        &self,
        writer: &mut W,
        width: u16,
        height: u16,
        options: &crate::options::ViewOptions,
    ) -> Result<()> {
        let w = width as usize;
        let h = height as usize;

        if h < 3 || w < 10 {
            return Ok(());
        }

        let visible_height = h.saturating_sub(2);
        let short_oid = self.commit_oid.to_hex_with_len(7).to_string();

        // 1. Header bar (Row 1)
        let total_lines = self.lines.len();
        let loading_str = if self.is_loading { " (loading...)" } else { "" };
        let title_raw = if self.is_binary {
            format!("[blame] {short_oid}:{} (binary){loading_str}", self.path)
        } else {
            format!(
                "[blame] {short_oid}:{} - {total_lines} lines{loading_str}",
                self.path
            )
        };
        let title_line = truncate_display_width(&title_raw, w);
        let title_padded = tigrs_core::ansi::pad_display_width(title_line, w);
        write!(writer, "\x1b[1;1H\x1b[7m\x1b[1m{title_padded}\x1b[0m")?;

        // 2. Body rows (Row 2 .. height - 1)
        if self.is_binary {
            write!(
                writer,
                "\x1b[2;1H\x1b[2K\x1b[33m[Binary file: cannot display blame annotations]\x1b[0m"
            )?;
            for row_idx in 1..visible_height {
                let term_row = row_idx + 2;
                write!(writer, "\x1b[{term_row};1H\x1b[2K~")?;
            }
        } else if self.lines.is_empty() {
            let msg = if self.is_loading {
                "  Loading blame annotations..."
            } else {
                "  Empty file."
            };
            write!(writer, "\x1b[2;1H\x1b[2K{msg}")?;
            for row_idx in 1..visible_height {
                let term_row = row_idx + 2;
                write!(writer, "\x1b[{term_row};1H\x1b[2K~")?;
            }
        } else {
            let gutter_w = total_lines.to_string().len().max(3);
            let prefix_w = if w >= 65 {
                gutter_w + 36
            } else {
                gutter_w + 12
            };
            let content_max_w = w.saturating_sub(prefix_w);

            // Ensure highlighting up to visible viewport plus 32 lines lookahead
            if options.syntax_highlighting {
                self.ensure_highlighted(self.scroll_offset() + visible_height + 32);
            }

            let highlighted = match self.highlighted_lines.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };

            for row_idx in 0..visible_height {
                let term_row = row_idx + 2;
                let line_idx = self.scroll_offset() + row_idx;

                if line_idx < self.lines.len() {
                    let line = &self.lines[line_idx];
                    let line_num = line.line_number;
                    let is_cursor = line_idx == self.cursor();

                    let raw_line = if options.syntax_highlighting {
                        highlighted
                            .get(line_idx)
                            .map_or(line.content.as_str(), String::as_str)
                    } else {
                        line.content.as_str()
                    };
                    let expanded = crate::diff::expand_tabs(raw_line, options.tab_size);
                    let line_content =
                        tigrs_core::ansi::truncate_visible_width(&expanded, content_max_w);

                    write!(writer, "\x1b[{term_row};1H\x1b[2K")?;

                    // Build blame prefix directly into writer without per-row String allocation:
                    // If terminal width >= 65: [hash 8] [author 12] [date 10]
                    // If terminal width < 65: [hash 8]
                    let author_padded = tigrs_core::ansi::pad_display_width(
                        truncate_display_width(&line.author, 12),
                        12,
                    );
                    if is_cursor {
                        // Highlighted cursor row
                        if w >= 65 {
                            write!(
                                writer,
                                "\x1b[7m\x1b[1m{:<8} {author_padded} {:<10} {line_num:>gutter_w$} >\x1b[0m {line_content}\x1b[0m",
                                line.short_commit_id, line.author_date
                            )?;
                        } else {
                            write!(
                                writer,
                                "\x1b[7m\x1b[1m{:<8} {line_num:>gutter_w$} >\x1b[0m {line_content}\x1b[0m",
                                line.short_commit_id
                            )?;
                        }
                    } else if w >= 65 {
                        // Normal row with colored metadata and dim gutter separator
                        write!(
                            writer,
                            "\x1b[36m{:<8} {author_padded} {:<10}\x1b[0m \x1b[2m{line_num:>gutter_w$} │\x1b[0m {line_content}\x1b[0m",
                            line.short_commit_id, line.author_date
                        )?;
                    } else {
                        write!(
                            writer,
                            "\x1b[36m{:<8}\x1b[0m \x1b[2m{line_num:>gutter_w$} │\x1b[0m {line_content}\x1b[0m",
                            line.short_commit_id
                        )?;
                    }
                } else {
                    write!(writer, "\x1b[{term_row};1H\x1b[2K~")?;
                }
            }
        }

        // 3. Status bar (Row height)
        let total = self.lines.len();
        let current = if total == 0 { 0 } else { self.cursor() + 1 };
        let pct = (current * 100).checked_div(total).unwrap_or(100);

        let status_left = if let Some(msg) = &self.status_message {
            format!("[blame] {msg} [q: back]")
        } else if self.is_binary {
            format!("[blame] binary - {} [q: back]", self.path)
        } else {
            let summary = self.selected_line().map_or("", |l| &l.summary);
            if !summary.is_empty() && w >= 80 {
                format!(
                    "[blame] {current}/{total} ({pct}%) - \"{summary}\" [Enter: diff, ,: parent, q: back]"
                )
            } else {
                format!(
                    "[blame] line {current} of {total} ({pct}%) - {} [Enter: diff, ,: parent, q: back]",
                    self.path
                )
            }
        };

        let status_line = truncate_display_width(&status_left, w);
        let status_padded = tigrs_core::ansi::pad_display_width(status_line, w);
        write!(writer, "\x1b[{height};1H\x1b[7m{status_padded}\x1b[0m")?;

        Ok(())
    }
}

impl super::View for BlameView {
    fn kind(&self) -> crate::app::layout::ViewKind {
        crate::app::layout::ViewKind::Blame
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
        options: &crate::options::ViewOptions,
    ) -> tigrs_core::error::Result<()> {
        self.render_with_options(&mut writer, width, height, options)
    }

    fn matches_search(&self, index: usize, pat: &crate::search::SearchPattern) -> bool {
        self.lines().get(index).is_some_and(|l| {
            pat.is_match(&l.content) || pat.is_match(&l.author) || pat.is_match_oid(&l.commit_id)
        })
    }

    fn populate_macro_context(&self, ctx: &mut tigrs_core::macro_ctx::MacroContext) {
        ctx.file = Some(self.path().to_string());
        ctx.lineno = Some(self.cursor() + 1);
    }

    fn cursor(&self) -> usize {
        self.cursor()
    }

    fn scroll_offset(&self) -> usize {
        self.scroll_offset()
    }

    fn set_cursor(&mut self, pos: usize, visible_height: usize) {
        self.set_cursor(pos, visible_height);
    }

    fn move_down_by(&mut self, n: usize, visible_height: usize) {
        self.move_down_by(n, visible_height);
    }

    fn move_up_by(&mut self, n: usize, visible_height: usize) {
        self.move_up_by(n, visible_height);
    }

    fn page_down(&mut self, visible_height: usize) {
        self.page_down(visible_height);
    }

    fn page_up(&mut self, visible_height: usize) {
        self.page_up(visible_height);
    }

    fn scroll_top(&mut self) {
        self.move_to_top();
    }

    fn scroll_bottom(&mut self, visible_height: usize) {
        self.move_to_bottom(visible_height);
    }

    fn scroll_line_down(&mut self, n: usize, visible_height: usize) {
        self.scroll_line_down(n, visible_height);
    }

    fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.scroll_line_up(n, visible_height);
    }

    fn selected_commit_id(&self) -> Option<tigrs_git::ObjectId> {
        self.current_commit_id()
    }

    fn has_content(&self) -> bool {
        self.line_count() > 0 || self.is_binary()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_blame_result() -> BlameResult {
        let oid1 = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
        let oid2 = ObjectId::from_hex(b"2222222222222222222222222222222222222222").unwrap();

        BlameResult {
            commit_id: oid2,
            path: "test.rs".to_string(),
            is_binary: false,
            lines: vec![
                BlameLine {
                    line_number: 1,
                    commit_id: oid1,
                    short_commit_id: Arc::from("11111111"),
                    author: Arc::from("Alice"),
                    author_date: Arc::from("2026-09-10"),
                    summary: Arc::from("Initial commit"),
                    content: "fn main() {".to_string(),
                    is_hunk_start: true,
                    parent_commit_id: None,
                    source_path: None,
                    source_line_number: 1,
                },
                BlameLine {
                    line_number: 2,
                    commit_id: oid2,
                    short_commit_id: Arc::from("22222222"),
                    author: Arc::from("Bob"),
                    author_date: Arc::from("2026-09-12"),
                    summary: Arc::from("Print greeting"),
                    content: "    println!(\"Hello\");".to_string(),
                    is_hunk_start: true,
                    parent_commit_id: Some(oid1),
                    source_path: None,
                    source_line_number: 2,
                },
                BlameLine {
                    line_number: 3,
                    commit_id: oid1,
                    short_commit_id: Arc::from("11111111"),
                    author: Arc::from("Alice"),
                    author_date: Arc::from("2026-09-10"),
                    summary: Arc::from("Initial commit"),
                    content: "}".to_string(),
                    is_hunk_start: true,
                    parent_commit_id: None,
                    source_path: None,
                    source_line_number: 2,
                },
            ],
        }
    }

    #[test]
    fn test_blame_view_navigation() {
        let res = dummy_blame_result();
        let mut view = BlameView::from_result(res);

        assert_eq!(view.cursor(), 0);
        assert_eq!(view.line_count(), 3);
        assert_eq!(
            view.current_commit_id().unwrap().to_hex().to_string(),
            "1111111111111111111111111111111111111111"
        );

        view.move_down(10);
        assert_eq!(view.cursor(), 1);
        assert_eq!(
            view.current_commit_id().unwrap().to_hex().to_string(),
            "2222222222222222222222222222222222222222"
        );
        assert!(view.current_parent_commit_id().is_some());

        view.move_up();
        assert_eq!(view.cursor(), 0);
        assert!(view.current_parent_commit_id().is_none());

        view.move_to_bottom(10);
        assert_eq!(view.cursor(), 2);

        view.move_to_top();
        assert_eq!(view.cursor(), 0);
    }

    #[test]
    fn test_blame_view_history_stack() {
        let res = dummy_blame_result();
        let mut view = BlameView::from_result(res);

        assert!(!view.has_history());
        view.move_down(10);

        let parent_oid = view.current_parent_commit_id().unwrap();
        view.navigate_to(parent_oid, "test.rs".to_string(), 1);

        assert!(view.has_history());
        assert_eq!(view.cursor(), 1);

        let prev = view.pop_history().unwrap();
        assert_eq!(prev.cursor_line, 1);
        assert!(!view.has_history());
    }

    #[test]
    fn test_blame_view_render_to_buffer() {
        let res = dummy_blame_result();
        let view = BlameView::from_result(res);

        let mut buf = Vec::new();
        view.render(&mut buf, 80, 10).unwrap();

        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("[blame]"));
        assert!(output.contains("test.rs"));
        assert!(output.contains("11111111"));
        assert!(output.contains("Alice"));
    }

    #[test]
    fn test_blame_view_lazy_syntax_highlighting() {
        let commit_oid = ObjectId::from_hex(b"0011223344556677889900112233445566778899").unwrap();
        let lines: Vec<BlameLine> = (0..1000)
            .map(|i| BlameLine {
                line_number: i + 1,
                commit_id: commit_oid,
                short_commit_id: Arc::from("12345678"),
                author: Arc::from("Author"),
                author_date: Arc::from("2026-01-01"),
                summary: Arc::from("Commit"),
                content: format!("fn item_{i}() {{ let y = {i}; }}"),
                is_hunk_start: i % 10 == 0,
                parent_commit_id: None,
                source_path: None,
                source_line_number: i + 1,
            })
            .collect();

        let res = BlameResult {
            commit_id: commit_oid,
            path: "src/big_blame.rs".to_string(),
            lines,
            is_binary: false,
        };

        let mut view = BlameView::from_result(res);
        // Initially, only the first 128 lines are highlighted
        assert_eq!(view.highlighted_lines_len(), 128);

        // Rendering top lines should not eagerly highlight all 1000
        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).unwrap();
        assert_eq!(view.highlighted_lines_len(), 128);

        // Moving to line 350
        view.set_cursor(350, 24);
        view.render(&mut buf, 80, 24).unwrap();
        assert!(view.highlighted_lines_len() >= 350);
        assert!(view.highlighted_lines_len() < 1000);

        // Move to bottom
        view.move_to_bottom(24);
        view.render(&mut buf, 80, 24).unwrap();
        assert_eq!(view.highlighted_lines_len(), 1000);
    }

    #[test]
    fn test_blame_view_from_blob_and_apply_blame_result() {
        let commit_oid = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
        let blob = BlobContent {
            oid: commit_oid,
            path: "file.rs".to_string(),
            size: 20,
            lines: vec!["line 1".to_string(), "line 2".to_string()].into(),
            is_binary: false,
        };

        let mut view = BlameView::from_blob(commit_oid, blob);
        assert!(view.is_loading());
        assert!(!view.is_binary());
        assert_eq!(view.lines().len(), 2);
        assert_eq!(&*view.lines()[0].author, "loading...");

        let mut buf = Vec::new();
        view.render(&mut buf, 80, 10).unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("loading..."));

        // Empty blob while loading renders "Loading blame annotations..."
        let empty_blob = BlobContent {
            oid: commit_oid,
            path: "empty.rs".to_string(),
            size: 0,
            lines: tigrs_core::LineBuffer::empty(),
            is_binary: false,
        };
        let empty_view = BlameView::from_blob(commit_oid, empty_blob);
        let mut empty_buf = Vec::new();
        empty_view.render(&mut empty_buf, 80, 10).unwrap();
        assert!(String::from_utf8_lossy(&empty_buf).contains("Loading blame annotations..."));

        // Applying with mismatched OID should be a no-op
        let other_oid = ObjectId::from_hex(b"2222222222222222222222222222222222222222").unwrap();
        let res_mismatched = BlameResult {
            commit_id: other_oid,
            path: "file.rs".to_string(),
            lines: Vec::new(),
            is_binary: false,
        };
        view.apply_blame_result(res_mismatched);
        assert!(view.is_loading());

        // Applying matching result
        let res_matched = BlameResult {
            commit_id: commit_oid,
            path: "file.rs".to_string(),
            lines: vec![
                BlameLine {
                    line_number: 1,
                    commit_id: commit_oid,
                    short_commit_id: Arc::from("11111111"),
                    author: Arc::from("Tester"),
                    author_date: Arc::from("2026-09-14"),
                    summary: Arc::from("blamed"),
                    content: "line 1".to_string(),
                    is_hunk_start: true,
                    parent_commit_id: None,
                    source_path: None,
                    source_line_number: 1,
                },
                BlameLine {
                    line_number: 2,
                    commit_id: commit_oid,
                    short_commit_id: Arc::from("11111111"),
                    author: Arc::from("Tester"),
                    author_date: Arc::from("2026-09-14"),
                    summary: Arc::from("blamed"),
                    content: "line 2".to_string(),
                    is_hunk_start: false,
                    parent_commit_id: None,
                    source_path: None,
                    source_line_number: 2,
                },
            ],
            is_binary: false,
        };
        view.apply_blame_result(res_matched);
        assert!(!view.is_loading());
        assert_eq!(&*view.lines()[0].author, "Tester");
    }

    #[test]
    fn test_blame_view_paging_scrolling_and_status() {
        let res = dummy_blame_result();
        let mut view = BlameView::from_result(res);

        view.set_status_message("Custom status msg".to_string());
        let mut buf = Vec::new();
        view.render(&mut buf, 80, 10).unwrap();
        assert!(String::from_utf8_lossy(&buf).contains("Custom status msg"));
        view.clear_status_message();

        view.page_down(2);
        assert_eq!(view.cursor(), 2);
        view.page_up(1);
        assert_eq!(view.cursor(), 1);

        view.scroll_line_down(1, 2);
        assert_eq!(view.scroll_offset(), 1);
        view.scroll_line_up(1, 2);
        assert_eq!(view.scroll_offset(), 0);

        // Binary blame rendering
        let bin_res = BlameResult {
            commit_id: ObjectId::null(gix::hash::Kind::Sha1),
            path: "data.bin".to_string(),
            lines: Vec::new(),
            is_binary: true,
        };
        let bin_view = BlameView::from_result(bin_res);
        assert!(bin_view.is_binary());
        let mut bin_buf = Vec::new();
        bin_view.render(&mut bin_buf, 80, 10).unwrap();
        assert!(String::from_utf8_lossy(&bin_buf).contains("Binary file"));

        // Empty file blame rendering
        let empty_res = BlameResult {
            commit_id: ObjectId::null(gix::hash::Kind::Sha1),
            path: "empty.txt".to_string(),
            lines: Vec::new(),
            is_binary: false,
        };
        let empty_view = BlameView::from_result(empty_res);
        let mut empty_buf = Vec::new();
        empty_view.render(&mut empty_buf, 80, 10).unwrap();
        assert!(String::from_utf8_lossy(&empty_buf).contains("Empty file"));
    }
}
