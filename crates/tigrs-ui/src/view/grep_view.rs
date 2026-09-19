// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Interactive grep search view for repository text searches.

use super::ViewportCursor;
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};
use std::io::Write;
use tigrs_core::error::Result;
use unicode_width::UnicodeWidthStr;

/// A single grep match entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepMatch {
    /// Repository-relative path of the matched file.
    pub path: String,
    /// 1-based line number of the match.
    pub line_num: usize,
    /// Text content of the matched line.
    pub content: String,
}

/// Interactive grep view component.
#[derive(Debug)]
pub struct GrepView {
    pattern: String,
    matches: Vec<GrepMatch>,
    nav: ViewportCursor,
}

impl GrepView {
    /// Creates a new `GrepView` with a search pattern and matches.
    pub fn new(pattern: impl Into<String>, matches: Vec<GrepMatch>) -> Self {
        let pattern = pattern.into();
        Self {
            pattern: tigrs_core::ansi::strip_control_chars(&pattern).into_owned(),
            matches,
            nav: ViewportCursor::new(),
        }
    }

    /// Search pattern.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Total number of matches.
    pub fn line_count(&self) -> usize {
        self.matches.len()
    }

    /// Returns the currently selected match, if any.
    pub fn selected_match(&self) -> Option<&GrepMatch> {
        self.matches.get(self.nav.cursor())
    }

    /// Selected row index.
    pub fn cursor(&self) -> usize {
        self.nav.cursor()
    }

    /// Moves cursor down.
    pub fn move_down(&mut self, n: usize, visible_height: usize) {
        self.nav.move_down_by(n, self.matches.len(), visible_height);
    }

    /// Moves cursor up.
    pub fn move_up(&mut self, n: usize, visible_height: usize) {
        self.nav.move_up_by(n, visible_height);
    }

    /// Moves cursor page down.
    pub fn page_down(&mut self, visible_height: usize) {
        self.nav.page_down(self.matches.len(), visible_height);
    }

    /// Moves cursor page up.
    pub fn page_up(&mut self, visible_height: usize) {
        self.nav.page_up(visible_height);
    }

    /// Sets cursor position.
    pub fn set_cursor(&mut self, pos: usize, visible_height: usize) {
        self.nav.set_cursor(pos, self.matches.len(), visible_height);
    }

    /// Returns searchable text for a given row index.
    pub fn row_text(&self, idx: usize) -> Option<String> {
        let m = self.matches.get(idx)?;
        Some(format!("{}:{}:{}", m.path, m.line_num, m.content))
    }

    /// Scrolls to top.
    pub fn scroll_top(&mut self) {
        self.nav.scroll_top();
    }

    /// Scrolls to bottom.
    pub fn scroll_bottom(&mut self, visible_height: usize) {
        self.nav.scroll_bottom(self.matches.len(), visible_height);
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
            .scroll_line_down(n, self.matches.len(), visible_height);
    }

    /// Scrolls the viewport up by `n` lines, keeping cursor within the viewport.
    pub fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.nav.scroll_line_up(n, visible_height);
    }

    /// Renders the grep view into the writer with explicit view options.
    pub fn render_with_options<W: Write>(
        &self,
        w: &mut W,
        width: usize,
        height: usize,
        options: &crate::options::ViewOptions,
    ) -> Result<()> {
        let content_height = height.saturating_sub(2);
        let start = self.nav.scroll_offset();
        let end = (start + content_height).min(self.matches.len());

        if self.matches.is_empty() {
            let msg = format!("  No grep matches found for '{}'.", self.pattern);
            super::render_empty_list_placeholder(w, width, content_height, &msg)?;
            return Ok(());
        }

        let mut loc_buf = String::with_capacity(64);
        for row_idx in 0..content_height {
            let item_idx = start + row_idx;
            queue!(w, MoveTo(0, (row_idx + 1) as u16))?;

            if item_idx < end {
                let m = &self.matches[item_idx];
                let is_selected = item_idx == self.nav.cursor();

                if is_selected {
                    queue!(w, SetAttribute(Attribute::Reverse))?;
                }

                loc_buf.clear();
                let _ = std::fmt::Write::write_fmt(
                    &mut loc_buf,
                    format_args!("{}:{}: ", m.path, m.line_num),
                );
                let loc_trunc = tigrs_core::ansi::truncate_display_width(&loc_buf, width);
                let loc_width = UnicodeWidthStr::width(loc_trunc);

                if !is_selected {
                    queue!(w, SetForegroundColor(Color::Yellow))?;
                }
                write!(w, "{loc_trunc}")?;
                if !is_selected {
                    queue!(w, ResetColor)?;
                }

                let rem = width.saturating_sub(loc_width);
                let expanded_content = crate::diff::expand_tabs(&m.content, options.tab_size);
                let content_col = tigrs_core::ansi::truncate_display_width(&expanded_content, rem);

                let line_width = UnicodeWidthStr::width(content_col);
                write!(w, "{content_col}")?;
                super::write_line_el_or_pad(
                    w,
                    width.saturating_sub(loc_width + line_width),
                    is_selected,
                )?;

                if is_selected {
                    queue!(w, SetAttribute(Attribute::Reset))?;
                }
            } else {
                super::write_line_el_or_pad(w, width, false)?;
            }
        }

        Ok(())
    }

    /// Renders the grep view into the writer using default options.
    pub fn render<W: Write>(&self, w: &mut W, width: usize, height: usize) -> Result<()> {
        self.render_with_options(w, width, height, &crate::options::ViewOptions::default())
    }
}

impl super::View for GrepView {
    fn kind(&self) -> crate::app::layout::ViewKind {
        crate::app::layout::ViewKind::Grep
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
        self.row_text(index).is_some_and(|text| pat.is_match(&text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_matches() -> Vec<GrepMatch> {
        vec![
            GrepMatch {
                path: "src/main.rs".to_string(),
                line_num: 42,
                content: "fn main() { println!(\"hello\"); }".to_string(),
            },
            GrepMatch {
                path: "src/lib.rs".to_string(),
                line_num: 15,
                content: "pub fn hello() -> &'static str { \"world\" }".to_string(),
            },
        ]
    }

    #[test]
    fn test_grep_view_navigation_and_render() {
        let matches = sample_matches();
        let mut view = GrepView::new("hello".to_string(), matches);
        assert_eq!(view.line_count(), 2);
        assert_eq!(view.cursor(), 0);
        assert_eq!(view.pattern(), "hello");
        assert_eq!(view.selected_match().unwrap().path, "src/main.rs");

        view.move_down(1, 10);
        assert_eq!(view.cursor(), 1);
        assert_eq!(view.selected_match().unwrap().path, "src/lib.rs");

        view.move_down(10, 10);
        assert_eq!(view.cursor(), 1);

        view.move_up(1, 10);
        assert_eq!(view.cursor(), 0);

        view.scroll_bottom(10);
        assert_eq!(view.cursor(), 1);

        view.scroll_top();
        assert_eq!(view.cursor(), 0);

        assert!(view.row_text(0).unwrap().contains("src/main.rs"));
        assert!(view.row_text(1).unwrap().contains("src/lib.rs"));

        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("src/main.rs:42:"));
    }

    #[test]
    fn test_grep_view_empty_state() {
        let mut view = GrepView::new("not_found".to_string(), Vec::new());
        assert_eq!(view.line_count(), 0);
        assert_eq!(view.cursor(), 0);
        assert!(view.selected_match().is_none());
        assert!(view.row_text(0).is_none());
        assert_eq!(view.pattern(), "not_found");

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
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("No grep matches found for 'not_found'"));

        let mut narrow_buf = Vec::new();
        view.render(&mut narrow_buf, 20, 10).unwrap();
    }

    #[test]
    fn test_grep_view_paging_and_set_cursor() {
        let matches: Vec<GrepMatch> = (0..15)
            .map(|i| GrepMatch {
                path: format!("src/file_{i}.rs"),
                line_num: i * 10 + 1,
                content: format!("let value_{i} = calculate({i});"),
            })
            .collect();
        let mut view = GrepView::new("calculate".to_string(), matches);
        assert_eq!(view.line_count(), 15);

        view.page_down(5);
        assert_eq!(view.cursor(), 5);

        view.page_down(5);
        assert_eq!(view.cursor(), 10);

        view.page_up(4);
        assert_eq!(view.cursor(), 6);

        view.set_cursor(12, 5);
        assert_eq!(view.cursor(), 12);

        view.set_cursor(999, 5);
        assert_eq!(view.cursor(), 14);
    }

    #[test]
    fn test_grep_view_scroll_line_viewport() {
        let matches: Vec<GrepMatch> = (0..20)
            .map(|i| GrepMatch {
                path: format!("src/module_{i}.rs"),
                line_num: i + 1,
                content: format!("println!(\"hit {i}\");"),
            })
            .collect();
        let mut view = GrepView::new("hit".to_string(), matches);

        view.scroll_line_down(5, 10);
        assert_eq!(view.scroll_offset(), 5);
        assert_eq!(view.cursor(), 5);

        view.scroll_line_up(3, 10);
        assert_eq!(view.scroll_offset(), 2);

        view.scroll_line_up(50, 10);
        assert_eq!(view.scroll_offset(), 0);
    }

    #[test]
    fn test_grep_view_zero_visible_height_and_truncation() {
        let matches = vec![GrepMatch {
            path: "deep/nested/path/to/very/long/filename_example.rs".to_string(),
            line_num: 12345,
            content: "very long line content with search term ".repeat(6),
        }];
        let mut view = GrepView::new("search".to_string(), matches);

        view.move_down(1, 0);
        view.move_up(1, 0);
        view.scroll_line_down(1, 0);
        view.scroll_line_up(1, 0);

        let mut buf = Vec::new();
        view.render(&mut buf, 25, 6).expect("render narrow grep");
        assert!(!buf.is_empty());
    }
}
