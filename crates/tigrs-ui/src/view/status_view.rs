// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Status view component rendering working tree status sections and items.

use super::ViewportCursor;
use std::io::Write;
use tigrs_core::ansi::truncate_display_width;
use tigrs_core::error::Result;
use tigrs_git::{StatusItem, StatusReport, StatusSection};

/// Type classification for lines rendered in the status view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusRow {
    /// Section header indicating a status group and item count.
    Header {
        /// Working-tree status section (`Staged`, `Unstaged`, or `Untracked`).
        section: StatusSection,
        /// Number of items in this section.
        count: usize,
    },
    /// A single selectable status file entry.
    Item(StatusItem),
    /// An empty separator row.
    Empty,
    /// Message displayed when working tree is completely clean.
    CleanNotice,
}

/// Full-screen scrollable status view displaying staged, unstaged, and untracked files.
pub struct StatusView {
    /// Underlying status report.
    report: StatusReport,
    /// Flattened display rows including headers, items, and spacing.
    rows: Vec<StatusRow>,
    /// Index of the currently highlighted row and viewport scroll.
    nav: ViewportCursor,
}

impl StatusView {
    /// Creates a new `StatusView` from a `StatusReport`.
    pub fn new(report: StatusReport) -> Self {
        let mut rows = Vec::new();
        build_status_rows(&report, &mut rows);

        // Find initial cursor: prefer the first item row if present, otherwise row 0.
        let initial_cursor = rows
            .iter()
            .position(|r| matches!(r, StatusRow::Item(_)))
            .unwrap_or(0);

        Self {
            report,
            rows,
            nav: ViewportCursor::with_coordinates(initial_cursor, 0),
        }
    }

    /// Returns a reference to the underlying `StatusReport`.
    pub fn report(&self) -> &StatusReport {
        &self.report
    }

    /// Returns the total number of display rows.
    pub fn line_count(&self) -> usize {
        self.rows.len()
    }

    /// Returns the textual representation of a row at `idx` for search matching.
    pub fn row_text(&self, idx: usize) -> Option<String> {
        self.rows.get(idx).map(|r| format_row(r).0)
    }

    /// Returns the current cursor row index.
    pub fn cursor_index(&self) -> usize {
        self.nav.cursor()
    }

    /// Returns a reference to the currently highlighted `StatusItem`, if any.
    pub fn selected_item(&self) -> Option<&StatusItem> {
        let cursor = self.nav.cursor();
        if cursor < self.rows.len() {
            match &self.rows[cursor] {
                StatusRow::Item(item) => Some(item),
                _ => None,
            }
        } else {
            None
        }
    }

    /// Returns the `StatusSection` of the currently highlighted section header row, if any.
    pub fn selected_section(&self) -> Option<StatusSection> {
        let cursor = self.nav.cursor();
        if cursor < self.rows.len() {
            match &self.rows[cursor] {
                StatusRow::Header { section, .. } => Some(*section),
                _ => None,
            }
        } else {
            None
        }
    }

    /// Moves cursor down by `n` lines, skipping empty separator rows.
    pub fn move_down(&mut self, n: usize, visible_height: usize) {
        if self.rows.is_empty() {
            return;
        }
        let max_idx = self.rows.len().saturating_sub(1);
        let mut new_cursor = (self.nav.cursor() + n).min(max_idx);

        // Skip landing on an empty row if possible
        if matches!(self.rows.get(new_cursor), Some(StatusRow::Empty)) && new_cursor < max_idx {
            new_cursor += 1;
        }

        self.nav.cursor = new_cursor;
        self.nav.adjust_scroll(visible_height);
    }

    /// Moves cursor up by `n` lines, skipping empty separator rows.
    pub fn move_up(&mut self, n: usize, visible_height: usize) {
        if self.rows.is_empty() {
            return;
        }
        let mut new_cursor = self.nav.cursor().saturating_sub(n);

        // Skip landing on an empty row if possible
        if matches!(self.rows.get(new_cursor), Some(StatusRow::Empty)) && new_cursor > 0 {
            new_cursor -= 1;
        }

        self.nav.cursor = new_cursor;
        self.nav.adjust_scroll(visible_height);
    }

    /// Scrolls down by half or full page.
    pub fn page_down(&mut self, visible_height: usize) {
        let step = visible_height.saturating_sub(2).max(1);
        self.move_down(step, visible_height);
    }

    /// Scrolls up by half or full page.
    pub fn page_up(&mut self, visible_height: usize) {
        let step = visible_height.saturating_sub(2).max(1);
        self.move_up(step, visible_height);
    }

    /// Jumps cursor to the very first item or row.
    pub fn home(&mut self) {
        let first_item = self
            .rows
            .iter()
            .position(|r| matches!(r, StatusRow::Item(_)))
            .unwrap_or(0);
        self.nav.cursor = first_item;
        self.nav.scroll_offset = 0;
    }

    /// Jumps cursor to the last selectable item.
    pub fn end(&mut self, visible_height: usize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self
            .rows
            .iter()
            .rposition(|r| matches!(r, StatusRow::Item(_)))
            .unwrap_or_else(|| self.rows.len().saturating_sub(1));
        self.nav.cursor = last;
        self.nav.adjust_scroll(visible_height);
    }

    /// Sets cursor position directly, skipping empty separator rows and clamping to valid range.
    pub fn set_cursor(&mut self, cursor: usize, visible_height: usize) {
        if self.rows.is_empty() {
            self.nav.cursor = 0;
            self.nav.scroll_offset = 0;
            return;
        }
        let max_idx = self.rows.len().saturating_sub(1);
        let mut new_cursor = cursor.min(max_idx);
        if matches!(self.rows.get(new_cursor), Some(StatusRow::Empty)) {
            if new_cursor < max_idx {
                new_cursor += 1;
            } else {
                new_cursor = new_cursor.saturating_sub(1);
            }
        }
        self.nav.cursor = new_cursor;
        if visible_height > 0 {
            let max_scroll = self.rows.len().saturating_sub(visible_height);
            if self.nav.scroll_offset > max_scroll {
                self.nav.scroll_offset = max_scroll;
            }
        }
        self.nav.adjust_scroll(visible_height);
    }

    /// Returns the current vertical scroll offset.
    #[inline]
    #[must_use]
    pub fn scroll_offset(&self) -> usize {
        self.nav.scroll_offset()
    }

    /// Scrolls the viewport down by `n` lines, keeping the cursor within the viewport.
    pub fn scroll_line_down(&mut self, n: usize, visible_height: usize) {
        if self.rows.is_empty() || visible_height == 0 {
            return;
        }
        let max_scroll = self.rows.len().saturating_sub(visible_height);
        self.nav.scroll_offset = (self.nav.scroll_offset + n).min(max_scroll);
        if self.nav.cursor < self.nav.scroll_offset {
            self.nav.cursor = self.nav.scroll_offset;
            if matches!(self.rows.get(self.nav.cursor), Some(StatusRow::Empty))
                && self.nav.cursor + 1 < self.rows.len()
            {
                self.nav.cursor += 1;
            }
        }
    }

    /// Scrolls the viewport up by `n` lines, keeping the cursor within the viewport.
    pub fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.nav.scroll_offset = self.nav.scroll_offset.saturating_sub(n);
        if visible_height > 0 && self.nav.cursor >= self.nav.scroll_offset + visible_height {
            self.nav.cursor = (self.nav.scroll_offset + visible_height).saturating_sub(1);
            if matches!(self.rows.get(self.nav.cursor), Some(StatusRow::Empty))
                && self.nav.cursor > 0
            {
                self.nav.cursor -= 1;
            }
        }
    }

    /// Renders the complete status view to the terminal using default options.
    pub fn render<W: Write>(&self, writer: &mut W, width: u16, height: u16) -> Result<()> {
        self.render_with_options(
            writer,
            width,
            height,
            &crate::options::ViewOptions::default(),
        )
    }

    /// Renders the complete status view with specified display options.
    pub fn render_with_options<W: Write>(
        &self,
        writer: &mut W,
        width: u16,
        height: u16,
        options: &crate::options::ViewOptions,
    ) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }

        let w = width as usize;
        let visible_height = (height as usize).saturating_sub(2);

        // 1. Title bar (Row 1)
        let branch_clean = if self.report.branch.is_empty() {
            std::borrow::Cow::Borrowed("HEAD")
        } else {
            tigrs_core::ansi::strip_control_chars(&self.report.branch)
        };
        let total = self.report.total_count();
        let change_str = if total == 1 { "change" } else { "changes" };
        let title_raw = format!("[status] {branch_clean} - {total} {change_str}");
        let title_line = truncate_display_width(&title_raw, w);
        let title_padded = tigrs_core::ansi::pad_display_width(title_line, w);
        write!(writer, "\x1b[1;1H\x1b[7m\x1b[1m{title_padded}\x1b[0m")?;

        // 2. Viewport body (Rows 2 .. height - 1)
        for row_idx in 0..visible_height {
            let term_row = row_idx + 2;
            let line_idx = self.nav.scroll_offset + row_idx;

            let _ = options;
            if line_idx < self.rows.len() {
                let row = &self.rows[line_idx];
                let is_cursor = line_idx == self.nav.cursor;
                let (text, color) = format_row(row);
                let text_truncated = truncate_display_width(&text, w);

                write!(writer, "\x1b[{term_row};1H\x1b[2K")?;

                if is_cursor {
                    // Reversed background for highlighted row
                    let cursor_padded = tigrs_core::ansi::pad_display_width(text_truncated, w);
                    write!(writer, "\x1b[7m{cursor_padded}\x1b[0m")?;
                } else {
                    write!(writer, "{color}{text_truncated}\x1b[0m")?;
                }
            } else {
                write!(writer, "\x1b[{term_row};1H\x1b[2K~")?;
            }
        }

        // 3. Status bar (Row height)
        let row_total = self.rows.len();
        let current = if row_total == 0 {
            0
        } else {
            self.nav.cursor + 1
        };
        let pct = (current * 100).checked_div(row_total).unwrap_or(100);

        let status_left = if let Some(item) = self.selected_item() {
            let action_hint = match item.section {
                StatusSection::Staged => "u: unstage",
                StatusSection::Unstaged => "u: stage, !: discard",
                StatusSection::Untracked => "u: stage, !: delete",
                StatusSection::Unmerged => "u: stage",
            };
            let clean_path = tigrs_core::ansi::strip_control_chars(&item.path);
            format!(
                "[status] line {current} of {row_total} ({pct}%) - {} {clean_path} [{action_hint}]",
                item.label()
            )
        } else {
            format!("[status] line {current} of {row_total} ({pct}%)")
        };

        let status_line = truncate_display_width(&status_left, w);
        let status_padded = tigrs_core::ansi::pad_display_width(status_line, w);
        write!(writer, "\x1b[{height};1H\x1b[7m{status_padded}\x1b[0m")?;

        Ok(())
    }
}

/// Converts a [`StatusReport`] into an ordered sequence of display rows.
fn build_status_rows(report: &StatusReport, rows: &mut Vec<StatusRow>) {
    if report.is_empty() {
        rows.push(StatusRow::CleanNotice);
        return;
    }

    let mut has_previous_section = false;

    // 1. Changes to be committed (Staged)
    if !report.staged.is_empty() {
        rows.push(StatusRow::Header {
            section: StatusSection::Staged,
            count: report.staged.len(),
        });
        for item in &report.staged {
            rows.push(StatusRow::Item(item.clone()));
        }
        has_previous_section = true;
    }

    // 2. Unmerged paths (Conflicts)
    if !report.unmerged.is_empty() {
        if has_previous_section {
            rows.push(StatusRow::Empty);
        }
        rows.push(StatusRow::Header {
            section: StatusSection::Unmerged,
            count: report.unmerged.len(),
        });
        for item in &report.unmerged {
            rows.push(StatusRow::Item(item.clone()));
        }
        has_previous_section = true;
    }

    // 3. Changes not staged for commit (Unstaged)
    if !report.unstaged.is_empty() {
        if has_previous_section {
            rows.push(StatusRow::Empty);
        }
        rows.push(StatusRow::Header {
            section: StatusSection::Unstaged,
            count: report.unstaged.len(),
        });
        for item in &report.unstaged {
            rows.push(StatusRow::Item(item.clone()));
        }
        has_previous_section = true;
    }

    // 4. Untracked files
    if !report.untracked.is_empty() {
        if has_previous_section {
            rows.push(StatusRow::Empty);
        }
        rows.push(StatusRow::Header {
            section: StatusSection::Untracked,
            count: report.untracked.len(),
        });
        for item in &report.untracked {
            rows.push(StatusRow::Item(item.clone()));
        }
    }
}

/// Formats a [`StatusRow`] into display text and ANSI color styling.
fn format_row(row: &StatusRow) -> (String, &'static str) {
    match row {
        StatusRow::Header { section, count } => {
            let text = format!("{}: ({count})", section.title());
            let color = match section {
                StatusSection::Staged => "\x1b[1;32m",    // Bold Green
                StatusSection::Unstaged => "\x1b[1;36m",  // Bold Cyan
                StatusSection::Untracked => "\x1b[1;31m", // Bold Red
                StatusSection::Unmerged => "\x1b[1;33m",  // Bold Yellow
            };
            (text, color)
        }
        StatusRow::Item(item) => {
            let clean_path = tigrs_core::ansi::strip_control_chars(&item.path);
            let text = if let Some(ref old) = item.old_path {
                let clean_old = tigrs_core::ansi::strip_control_chars(old);
                format!("    {:<12} {clean_old} -> {clean_path}", item.label())
            } else {
                format!("    {:<12} {clean_path}", item.label())
            };
            let color = match item.section {
                StatusSection::Staged => "\x1b[32m", // Green
                StatusSection::Unstaged => match item.status_code {
                    'D' => "\x1b[31m", // Red for deleted
                    _ => "\x1b[36m",   // Cyan for modified/typechange
                },
                StatusSection::Untracked => "\x1b[31m", // Red
                StatusSection::Unmerged => "\x1b[33m",  // Yellow
            };
            (text, color)
        }
        StatusRow::Empty => (String::new(), "\x1b[0m"),
        StatusRow::CleanNotice => (
            "    nothing to commit, working tree clean".to_string(),
            "\x1b[32m", // Green
        ),
    }
}

impl super::View for StatusView {
    fn kind(&self) -> crate::app::layout::ViewKind {
        crate::app::layout::ViewKind::Status
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
        self.row_text(index).is_some_and(|text| pat.is_match(&text))
    }

    fn populate_macro_context(&self, ctx: &mut tigrs_core::macro_ctx::MacroContext) {
        if let Some(item) = self.selected_item() {
            ctx.file = Some(item.path.clone());
            ctx.file_old.clone_from(&item.old_path);
        }
    }

    fn cursor(&self) -> usize {
        self.cursor_index()
    }

    fn scroll_offset(&self) -> usize {
        self.scroll_offset()
    }

    fn set_cursor(&mut self, pos: usize, visible_height: usize) {
        self.set_cursor(pos, visible_height);
    }

    fn move_down_by(&mut self, n: usize, visible_height: usize) {
        self.move_down(n, visible_height);
    }

    fn move_up_by(&mut self, n: usize, visible_height: usize) {
        self.move_up(n, visible_height);
    }

    fn page_down(&mut self, visible_height: usize) {
        self.page_down(visible_height);
    }

    fn page_up(&mut self, visible_height: usize) {
        self.page_up(visible_height);
    }

    fn scroll_top(&mut self) {
        self.home();
    }

    fn scroll_bottom(&mut self, visible_height: usize) {
        self.end(visible_height);
    }

    fn scroll_line_down(&mut self, n: usize, visible_height: usize) {
        self.scroll_line_down(n, visible_height);
    }

    fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.scroll_line_up(n, visible_height);
    }

    fn selected_commit_id(&self) -> Option<tigrs_git::ObjectId> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ViewOptions;

    fn sample_report() -> StatusReport {
        StatusReport {
            staged: vec![StatusItem::new(
                'M',
                StatusSection::Staged,
                "src/main.rs",
                None,
            )],
            unstaged: vec![StatusItem::new(
                'D',
                StatusSection::Unstaged,
                "README.md",
                None,
            )],
            untracked: vec![StatusItem::new(
                '?',
                StatusSection::Untracked,
                "scratch.txt",
                None,
            )],
            unmerged: Vec::new(),
            branch: "feature".to_string(),
            head_commit: None,
        }
    }

    #[test]
    fn test_status_view_initial_cursor_and_selection() {
        let view = StatusView::new(sample_report());
        // Initial cursor should point to first item (index 1, below header at 0)
        assert_eq!(view.cursor_index(), 1);
        let selected = view.selected_item().expect("selected item");
        assert_eq!(selected.path, "src/main.rs");
        assert_eq!(selected.section, StatusSection::Staged);
    }

    #[test]
    fn test_status_view_navigation_skips_empty() {
        let mut view = StatusView::new(sample_report());
        // row 0: Header (staged)
        // row 1: Item (src/main.rs)
        // row 2: Empty
        // row 3: Header (unstaged)
        // row 4: Item (README.md)
        // row 5: Empty
        // row 6: Header (untracked)
        // row 7: Item (scratch.txt)

        assert_eq!(view.cursor_index(), 1);
        view.move_down(1, 10);
        // Should land on row 3 (Header unstaged), skipping row 2 (Empty)
        assert_eq!(view.cursor_index(), 3);

        view.move_down(1, 10);
        assert_eq!(view.cursor_index(), 4);
        assert_eq!(view.selected_item().unwrap().path, "README.md");

        view.home();
        assert_eq!(view.cursor_index(), 1);

        view.end(10);
        assert_eq!(view.cursor_index(), 7);
        assert_eq!(view.selected_item().unwrap().path, "scratch.txt");
    }

    #[test]
    fn test_status_view_clean_working_tree() {
        let clean = StatusReport::default();
        let view = StatusView::new(clean);
        assert_eq!(view.line_count(), 1);
        assert_eq!(view.selected_item(), None);
    }

    #[test]
    fn test_status_view_render_buffer() {
        let view = StatusView::new(sample_report());
        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).expect("render");
        let rendered = String::from_utf8_lossy(&buf);
        assert!(rendered.contains("[status] feature - 3 changes"));
        assert!(rendered.contains("Changes to be committed"));
        assert!(rendered.contains("src/main.rs"));
        assert!(rendered.contains("README.md"));
        assert!(rendered.contains("scratch.txt"));
    }

    #[test]
    fn test_status_view_paging_and_set_cursor() {
        let mut view = StatusView::new(sample_report());
        // row 0: Header (staged)
        // row 1: Item (src/main.rs)
        // row 2: Empty
        // row 3: Header (unstaged)
        // row 4: Item (README.md)
        // row 5: Empty
        // row 6: Header (untracked)
        // row 7: Item (scratch.txt)

        assert_eq!(view.cursor_index(), 1);

        view.page_down(4);
        assert_eq!(view.cursor_index(), 3);

        view.page_up(4);
        assert_eq!(view.cursor_index(), 1);

        // Setting cursor on empty row 2 should advance to 3
        view.set_cursor(2, 10);
        assert_eq!(view.cursor_index(), 3);

        // Setting cursor out of bounds should clamp to 7
        view.set_cursor(999, 10);
        assert_eq!(view.cursor_index(), 7);
    }

    #[test]
    fn test_status_view_scroll_line_viewport() {
        let mut view = StatusView::new(sample_report());

        view.scroll_line_down(3, 4);
        assert_eq!(view.scroll_offset(), 3);
        assert!(view.cursor_index() >= 3);

        view.scroll_line_up(2, 4);
        assert_eq!(view.scroll_offset(), 1);

        view.scroll_line_up(50, 4);
        assert_eq!(view.scroll_offset(), 0);

        // Zero visible height
        view.scroll_line_down(1, 0);
        view.scroll_line_up(1, 0);
    }

    #[test]
    fn test_status_view_row_text_search() {
        let view = StatusView::new(sample_report());
        assert!(
            view.row_text(0)
                .unwrap()
                .contains("Changes to be committed")
        );
        assert!(view.row_text(1).unwrap().contains("src/main.rs"));
        assert!(view.row_text(2).unwrap().is_empty());
        assert!(view.row_text(4).unwrap().contains("README.md"));
        assert!(view.row_text(999).is_none());

        let clean_view = StatusView::new(StatusReport::default());
        assert!(
            clean_view
                .row_text(0)
                .unwrap()
                .contains("working tree clean")
        );
    }

    #[test]
    fn test_status_view_unmerged_rename_and_options() {
        let report = StatusReport {
            staged: vec![StatusItem::new(
                'R',
                StatusSection::Staged,
                "renamed_dest.rs",
                Some("renamed_src.rs".to_string()),
            )],
            unstaged: vec![],
            untracked: vec![StatusItem::new(
                '?',
                StatusSection::Untracked,
                "untracked.txt",
                None,
            )],
            unmerged: vec![StatusItem::new(
                'U',
                StatusSection::Unmerged,
                "conflict.txt",
                None,
            )],
            branch: "main".to_string(),
            head_commit: None,
        };

        let mut view = StatusView::new(report);
        let opts = ViewOptions {
            line_number: true,
            ..Default::default()
        };
        let mut buf = Vec::new();
        view.render_with_options(&mut buf, 100, 24, &opts)
            .expect("render");
        let rendered = String::from_utf8_lossy(&buf);
        assert!(rendered.contains("renamed_src.rs -> renamed_dest.rs"));
        assert!(rendered.contains("Unmerged paths"));
        assert!(rendered.contains("conflict.txt"));

        // Test page_down to bottom and page_up to top
        view.page_down(20);
        assert_eq!(view.cursor_index(), view.line_count() - 1);
        view.page_up(20);
        assert_eq!(view.cursor_index(), 0);

        // Test scroll_line_down past cursor
        view.scroll_line_down(10, 5);
        assert!(view.scroll_offset() > 0);

        // Test empty render dimension
        let mut empty_buf = Vec::new();
        assert!(view.render(&mut empty_buf, 0, 0).is_ok());
    }
}
