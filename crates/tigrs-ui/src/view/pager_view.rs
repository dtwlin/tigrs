// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Interactive pager view for scrolling and inspecting text streams.

use super::ViewportCursor;
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, SetAttribute};
use std::io::Write;
use tigrs_core::error::Result;

/// Interactive text pager view component.
#[derive(Debug)]
pub struct PagerView {
    title: String,
    lines: Vec<String>,
    nav: ViewportCursor,
}

impl PagerView {
    /// Creates a new `PagerView` with a title and lines of text.
    pub fn new(title: String, lines: Vec<String>) -> Self {
        let clean_title = tigrs_core::ansi::sanitize_string(title);
        let clean_lines = lines
            .into_iter()
            .map(|l| match tigrs_core::ansi::filter_sgr_only(&l) {
                std::borrow::Cow::Borrowed(_) => l,
                std::borrow::Cow::Owned(clean) => clean,
            })
            .collect();
        Self {
            title: clean_title,
            lines: clean_lines,
            nav: ViewportCursor::new(),
        }
    }

    /// Title of the pager view.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Total number of lines.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Selected row index.
    pub fn cursor(&self) -> usize {
        self.nav.cursor()
    }

    /// Moves cursor down.
    pub fn move_down(&mut self, n: usize, visible_height: usize) {
        self.nav.move_down_by(n, self.lines.len(), visible_height);
    }

    /// Moves cursor up.
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

    /// Sets cursor position.
    pub fn set_cursor(&mut self, pos: usize, visible_height: usize) {
        self.nav.set_cursor(pos, self.lines.len(), visible_height);
    }

    /// Returns searchable text for a given row index.
    pub fn row_text(&self, idx: usize) -> Option<String> {
        self.lines.get(idx).cloned()
    }

    /// Scrolls to top.
    pub fn scroll_top(&mut self) {
        self.nav.scroll_top();
    }

    /// Scrolls to bottom.
    pub fn scroll_bottom(&mut self, visible_height: usize) {
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
        self.nav
            .scroll_line_down(n, self.lines.len(), visible_height);
    }

    /// Scrolls the viewport up by `n` lines, keeping cursor within the viewport.
    pub fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.nav.scroll_line_up(n, visible_height);
    }

    /// Renders the pager view into the writer with explicit options.
    pub fn render_with_options<W: Write>(
        &self,
        w: &mut W,
        width: usize,
        height: usize,
        options: &crate::options::ViewOptions,
    ) -> Result<()> {
        use std::fmt::Write as _;

        let content_height = height.saturating_sub(2);
        let start = self.scroll_offset();
        let end = (start + content_height).min(self.lines.len());
        let mut num_buf = String::with_capacity(16);

        for row_idx in 0..content_height {
            let item_idx = start + row_idx;
            queue!(w, MoveTo(0, (row_idx + 1) as u16))?;

            if item_idx < end {
                let text = &self.lines[item_idx];
                let is_selected = item_idx == self.cursor();

                if is_selected {
                    queue!(w, SetAttribute(Attribute::Reverse))?;
                }

                num_buf.clear();
                let _ = write!(num_buf, "{:>5} ", item_idx + 1);
                let num_len = num_buf.len();
                let rem = width.saturating_sub(num_len);

                let expanded = crate::diff::expand_tabs(text, options.tab_size);
                let mut text_col = tigrs_core::ansi::truncate_visible_width(&expanded, rem);
                if is_selected && (text_col.contains("\x1b[0m") || text_col.contains("\x1b[m")) {
                    text_col = text_col
                        .replace("\x1b[0m", "\x1b[0;7m")
                        .replace("\x1b[m", "\x1b[0;7m");
                }

                let line_width = num_len + tigrs_core::ansi::visible_width(&text_col);
                write!(w, "{num_buf}{text_col}")?;
                if is_selected {
                    write!(w, "\x1b[7m")?;
                }
                super::write_line_el_or_pad(w, width.saturating_sub(line_width), is_selected)?;

                if is_selected {
                    queue!(w, SetAttribute(Attribute::Reset))?;
                }
            } else {
                super::write_line_el_or_pad(w, width, false)?;
            }
        }

        Ok(())
    }

    /// Renders the pager view into the writer using default options.
    pub fn render<W: Write>(&self, w: &mut W, width: usize, height: usize) -> Result<()> {
        self.render_with_options(w, width, height, &crate::options::ViewOptions::default())
    }
}

impl super::View for PagerView {
    fn kind(&self) -> crate::app::layout::ViewKind {
        crate::app::layout::ViewKind::Pager
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
        self.render_with_options(&mut writer, width as usize, height as usize, options)?;
        Ok(())
    }

    fn matches_search(&self, index: usize, pat: &crate::search::SearchPattern) -> bool {
        self.lines.get(index).is_some_and(|text| {
            let clean = tigrs_core::ansi::strip_control_chars(text);
            pat.is_match(&clean)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::View;

    #[test]
    fn test_pager_view_navigation_and_render() {
        let lines = vec![
            "line one of output".to_string(),
            "line two of output".to_string(),
            "line three of output".to_string(),
        ];
        let mut view = PagerView::new("pager".to_string(), lines);
        assert_eq!(view.line_count(), 3);
        assert_eq!(view.cursor(), 0);

        view.move_down(1, 10);
        assert_eq!(view.cursor(), 1);

        view.move_down(10, 10);
        assert_eq!(view.cursor(), 2);

        view.move_up(1, 10);
        assert_eq!(view.cursor(), 1);

        view.scroll_bottom(10);
        assert_eq!(view.cursor(), 2);

        view.scroll_top();
        assert_eq!(view.cursor(), 0);

        assert_eq!(view.row_text(0).as_deref(), Some("line one of output"));

        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("line one of output"));
    }

    #[test]
    fn test_pager_view_empty_state() {
        let mut view = PagerView::new("empty pager".to_string(), Vec::new());
        assert_eq!(view.line_count(), 0);
        assert_eq!(view.cursor(), 0);
        assert!(view.row_text(0).is_none());
        assert_eq!(view.title(), "empty pager");

        view.move_down(10, 10);
        assert_eq!(view.cursor(), 0);
        view.move_up(10, 10);
        assert_eq!(view.cursor(), 0);
        view.scroll_bottom(10);
        assert_eq!(view.cursor(), 0);
        view.scroll_top();
        assert_eq!(view.cursor(), 0);

        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).unwrap();
        assert!(!buf.is_empty());

        let mut narrow_buf = Vec::new();
        view.render(&mut narrow_buf, 20, 10).unwrap();
        assert!(!narrow_buf.is_empty());
    }

    #[test]
    fn test_pager_view_paging() {
        let mut view = PagerView::new(
            "log pager".to_string(),
            (0..=20).map(|i| format!("line {i}")).collect(),
        );
        assert_eq!(view.line_count(), 21);

        view.page_down(5);
        assert_eq!(view.cursor(), 5);

        view.page_down(5);
        assert_eq!(view.cursor(), 10);

        view.page_up(4);
        assert_eq!(view.cursor(), 6);

        view.set_cursor(18, 5);
        assert_eq!(view.cursor(), 18);

        // Clamping cursor out-of-bounds
        view.set_cursor(999, 5);
        assert_eq!(view.cursor(), 20);
    }

    #[test]
    fn test_pager_view_scroll_line_viewport() {
        let lines: Vec<String> = (0..30).map(|i| format!("item {i}")).collect();
        let mut view = PagerView::new("scroll test".to_string(), lines);

        view.scroll_line_down(5, 10);
        assert_eq!(view.scroll_offset(), 5);
        assert_eq!(view.cursor(), 5);

        view.scroll_line_down(10, 10);
        assert_eq!(view.scroll_offset(), 15);
        assert_eq!(view.cursor(), 15);

        view.scroll_line_up(5, 10);
        assert_eq!(view.scroll_offset(), 10);

        // Clamping to top
        view.scroll_line_up(50, 10);
        assert_eq!(view.scroll_offset(), 0);
    }

    #[test]
    fn test_pager_view_zero_visible_height_and_truncation() {
        let lines = vec!["a".repeat(120), "short line".to_string()];
        let mut view = PagerView::new("truncation test".to_string(), lines);

        // Zero visible height should never panic
        view.move_down(1, 0);
        view.move_up(1, 0);
        view.scroll_line_down(1, 0);
        view.scroll_line_up(1, 0);

        // Render with narrow width: long lines should be truncated without panicking
        let mut buf = Vec::new();
        view.render(&mut buf, 20, 5).expect("render narrow");
        assert!(!buf.is_empty());
    }

    #[test]
    fn test_pager_view_ansi_sgr_truncation_and_search() {
        let lines = vec![
            "\x1b[38;2;255;0;0mfoo\x1b[0m\x1b[32mbar\x1b[0m baz".to_string(),
            "\x1b[33msecond\x1b[0m line".to_string(),
        ];
        let view = PagerView::new("ansi\x1b[2Jpager".to_string(), lines);
        assert_eq!(view.title(), "ansipager");

        // Search across SGR boundary ("foobar") must match after stripping ANSI
        let pat_foobar = crate::search::SearchPattern::new("foobar", Some(false));
        assert!(view.matches_search(0, &pat_foobar));

        // Search for raw SGR sequence digits ("38;2") must NOT match
        let pat_sgr = crate::search::SearchPattern::new("38;2", Some(false));
        assert!(!view.matches_search(0, &pat_sgr));

        // Render into a 20x5 HeadlessTerminal and verify full visible text and cursor reverse video
        let mut term = crate::headless::HeadlessTerminal::new(20, 5);
        view.render(&mut term, 20, 5).unwrap();
        let row1 = term.line_text(1);
        assert!(
            row1.contains("foobar baz"),
            "ANSI SGR bytes should not consume visible column width: {row1:?}"
        );
        // Selected row (row 1) should retain reverse video across the embedded \x1b[0m reset
        for col in 0..20 {
            let cell = term.cell(col, 1).unwrap();
            assert!(
                cell.attrs.contains(crate::headless::CellAttrs::REVERSE),
                "Cell at col {col} lost REVERSE video after SGR reset: {cell:?}"
            );
        }
    }
}
