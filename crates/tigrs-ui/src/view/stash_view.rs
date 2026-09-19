// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Interactive stash view listing git stashes.

use super::ViewportCursor;
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};
use std::io::Write;
use tigrs_core::error::Result;
use tigrs_git::StashEntry;
use unicode_width::UnicodeWidthStr;

/// Interactive stash view component.
#[derive(Debug)]
pub struct StashView {
    stashes: Vec<StashEntry>,
    nav: ViewportCursor,
}

impl StashView {
    /// Creates a new `StashView` from a list of stash entries.
    pub fn new(stashes: Vec<StashEntry>) -> Self {
        Self {
            stashes,
            nav: ViewportCursor::new(),
        }
    }

    /// Total number of stashes.
    pub fn line_count(&self) -> usize {
        self.stashes.len()
    }

    /// Returns the currently selected stash entry, if any.
    pub fn selected_stash(&self) -> Option<&StashEntry> {
        self.stashes.get(self.nav.cursor())
    }

    /// Selected row index.
    pub fn cursor(&self) -> usize {
        self.nav.cursor()
    }

    /// Moves cursor down.
    pub fn move_down(&mut self, n: usize, visible_height: usize) {
        self.nav.move_down_by(n, self.stashes.len(), visible_height);
    }

    /// Moves cursor up.
    pub fn move_up(&mut self, n: usize, visible_height: usize) {
        self.nav.move_up_by(n, visible_height);
    }

    /// Moves cursor page down.
    pub fn page_down(&mut self, visible_height: usize) {
        self.nav.page_down(self.stashes.len(), visible_height);
    }

    /// Moves cursor page up.
    pub fn page_up(&mut self, visible_height: usize) {
        self.nav.page_up(visible_height);
    }

    /// Sets cursor position.
    pub fn set_cursor(&mut self, pos: usize, visible_height: usize) {
        self.nav.set_cursor(pos, self.stashes.len(), visible_height);
    }

    /// Returns searchable text for a given row index.
    pub fn row_text(&self, idx: usize) -> Option<String> {
        let s = self.stashes.get(idx)?;
        Some(format!(
            "stash@{{{}}} {} {}",
            s.index, s.commit_id, s.summary
        ))
    }

    /// Scrolls to top.
    pub fn scroll_top(&mut self) {
        self.nav.scroll_top();
    }

    /// Scrolls to bottom.
    pub fn scroll_bottom(&mut self, visible_height: usize) {
        self.nav.scroll_bottom(self.stashes.len(), visible_height);
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
            .scroll_line_down(n, self.stashes.len(), visible_height);
    }

    /// Scrolls the viewport up by `n` lines, keeping cursor within the viewport.
    pub fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.nav.scroll_line_up(n, visible_height);
    }

    /// Renders the stash view into the writer.
    pub fn render<W: Write>(&self, w: &mut W, width: usize, height: usize) -> Result<()> {
        use std::fmt::Write as _;

        let content_height = height.saturating_sub(2);
        let start = self.scroll_offset();
        let end = (start + content_height).min(self.stashes.len());

        if self.stashes.is_empty() {
            super::render_empty_list_placeholder(w, width, content_height, "  No stashes found.")?;
            return Ok(());
        }

        let mut tag_buf = String::with_capacity(16);
        let mut line_buf = String::with_capacity(128);

        for row_idx in 0..content_height {
            let item_idx = start + row_idx;
            queue!(w, MoveTo(0, (row_idx + 1) as u16))?;

            if item_idx < end {
                let s = &self.stashes[item_idx];
                let is_selected = item_idx == self.cursor();

                if is_selected {
                    queue!(w, SetAttribute(Attribute::Reverse))?;
                }

                tag_buf.clear();
                let _ = write!(tag_buf, "stash@{{{}}}", s.index);
                let trunc_tag_len =
                    tigrs_core::ansi::truncate_display_width(&tag_buf, 13.min(width)).len();
                tag_buf.truncate(trunc_tag_len);
                let tag_vis = UnicodeWidthStr::width(tag_buf.as_str());
                let tag_target = 14.min(width);
                for _ in 0..tag_target.saturating_sub(tag_vis) {
                    tag_buf.push(' ');
                }
                let tag_width = tag_vis.max(tag_target);

                if !is_selected {
                    queue!(w, SetForegroundColor(Color::Magenta))?;
                }
                write!(w, "{tag_buf}")?;
                if !is_selected {
                    queue!(w, ResetColor)?;
                }

                let rem_after_tag = width.saturating_sub(tag_width);
                let short_oid = s.commit_id.to_hex_with_len(7);
                let rem = width.saturating_sub(tag_width + 8);
                let summary_col = tigrs_core::ansi::truncate_display_width(&s.summary, rem);

                line_buf.clear();
                let _ = write!(line_buf, "{short_oid:>7} {summary_col}");
                let rendered_rest =
                    tigrs_core::ansi::truncate_display_width(&line_buf, rem_after_tag);
                let line_width = tag_width + UnicodeWidthStr::width(rendered_rest);
                write!(w, "{rendered_rest}")?;
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
}

impl super::View for StashView {
    fn kind(&self) -> crate::app::layout::ViewKind {
        crate::app::layout::ViewKind::Stash
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
        self.selected_stash().map(|entry| entry.commit_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigrs_git::ObjectId;

    fn sample_stashes() -> Vec<StashEntry> {
        vec![
            StashEntry {
                index: 0,
                commit_id: ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap(),
                summary: "WIP on main: work in progress".to_string(),
                time_secs: 1_700_000_000,
            },
            StashEntry {
                index: 1,
                commit_id: ObjectId::from_hex(b"2222222222222222222222222222222222222222").unwrap(),
                summary: "WIP on feature: experimental changes".to_string(),
                time_secs: 1_700_001_000,
            },
        ]
    }

    #[test]
    fn test_stash_view_navigation_and_render() {
        let stashes = sample_stashes();
        let mut view = StashView::new(stashes);
        assert_eq!(view.line_count(), 2);
        assert_eq!(view.cursor(), 0);
        assert_eq!(view.selected_stash().unwrap().index, 0);

        view.move_down(1, 10);
        assert_eq!(view.cursor(), 1);
        assert_eq!(view.selected_stash().unwrap().index, 1);

        view.move_down(10, 10);
        assert_eq!(view.cursor(), 1);

        view.move_up(1, 10);
        assert_eq!(view.cursor(), 0);

        view.scroll_bottom(10);
        assert_eq!(view.cursor(), 1);

        view.scroll_top();
        assert_eq!(view.cursor(), 0);

        assert!(view.row_text(0).unwrap().contains("stash@{0}"));
        assert!(view.row_text(1).unwrap().contains("stash@{1}"));

        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("stash@{0}"));
    }

    #[test]
    fn test_stash_view_empty_state() {
        let mut view = StashView::new(Vec::new());
        assert_eq!(view.line_count(), 0);
        assert_eq!(view.cursor(), 0);
        assert!(view.selected_stash().is_none());
        assert!(view.row_text(0).is_none());

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
        assert!(output.contains("No stashes found"));

        let mut narrow_buf = Vec::new();
        view.render(&mut narrow_buf, 20, 10).unwrap();
    }

    #[test]
    fn test_stash_view_paging_and_set_cursor() {
        let oid = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
        let stashes: Vec<StashEntry> = (0..15)
            .map(|i| StashEntry {
                index: i,
                commit_id: oid,
                summary: format!("WIP entry {i}"),
                time_secs: 1_700_000_000 + i as i64,
            })
            .collect();
        let mut view = StashView::new(stashes);
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
    fn test_stash_view_scroll_line_viewport() {
        let oid = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
        let stashes: Vec<StashEntry> = (0..20)
            .map(|i| StashEntry {
                index: i,
                commit_id: oid,
                summary: format!("WIP entry {i}"),
                time_secs: 1_700_000_000 + i as i64,
            })
            .collect();
        let mut view = StashView::new(stashes);

        view.scroll_line_down(5, 10);
        assert_eq!(view.scroll_offset(), 5);
        assert_eq!(view.cursor(), 5);

        view.scroll_line_up(3, 10);
        assert_eq!(view.scroll_offset(), 2);

        view.scroll_line_up(50, 10);
        assert_eq!(view.scroll_offset(), 0);
    }

    #[test]
    fn test_stash_view_zero_visible_height_and_truncation() {
        let oid = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
        let stashes = vec![StashEntry {
            index: 0,
            commit_id: oid,
            summary: "Extremely long summary message ".repeat(10),
            time_secs: 1_700_000_000,
        }];
        let mut view = StashView::new(stashes);

        view.move_down(1, 0);
        view.move_up(1, 0);
        view.scroll_line_down(1, 0);
        view.scroll_line_up(1, 0);

        let mut buf = Vec::new();
        view.render(&mut buf, 25, 6)
            .expect("render narrow with long summary");
        assert!(!buf.is_empty());
    }
}
