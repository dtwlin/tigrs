// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! References (`refs`) view displaying local branches, remote branches, tags, and stashes.

use super::ViewportCursor;
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};
use std::io::Write;
use tigrs_core::error::Result;
use tigrs_git::{RefEntry, RefKind};
use unicode_width::UnicodeWidthStr;

/// Interactive references view component.
#[derive(Debug)]
pub struct RefsView {
    refs: Vec<RefEntry>,
    nav: ViewportCursor,
}

impl RefsView {
    /// Creates a new `RefsView` from a list of references.
    pub fn new(refs: Vec<RefEntry>) -> Self {
        Self {
            refs,
            nav: ViewportCursor::new(),
        }
    }

    /// Total number of references.
    pub fn line_count(&self) -> usize {
        self.refs.len()
    }

    /// Returns the currently selected reference, if any.
    pub fn selected_ref(&self) -> Option<&RefEntry> {
        self.refs.get(self.nav.cursor())
    }

    /// Selected row index.
    pub fn cursor(&self) -> usize {
        self.nav.cursor()
    }

    /// Moves cursor down.
    pub fn move_down(&mut self, n: usize, visible_height: usize) {
        if self.refs.is_empty() {
            return;
        }
        let max_idx = self.refs.len().saturating_sub(1);
        self.nav.cursor = (self.nav.cursor + n).min(max_idx);
        self.nav.adjust_scroll(visible_height);
    }

    /// Moves cursor up.
    pub fn move_up(&mut self, n: usize, visible_height: usize) {
        self.nav.cursor = self.nav.cursor.saturating_sub(n);
        self.nav.adjust_scroll(visible_height);
    }

    /// Moves cursor page down.
    pub fn page_down(&mut self, visible_height: usize) {
        self.nav.page_down(self.refs.len(), visible_height);
    }

    /// Moves cursor page up.
    pub fn page_up(&mut self, visible_height: usize) {
        self.nav.page_up(visible_height);
    }

    /// Sets cursor position.
    pub fn set_cursor(&mut self, pos: usize, visible_height: usize) {
        self.nav.set_cursor(pos, self.refs.len(), visible_height);
    }

    /// Returns searchable text for a given row index.
    pub fn row_text(&self, idx: usize) -> Option<String> {
        let r = self.refs.get(idx)?;
        Some(format!(
            "{} {} {} {}",
            r.name, r.commit_id, r.summary, r.author_name
        ))
    }

    /// Scrolls to top.
    pub fn scroll_top(&mut self) {
        self.nav.scroll_top();
    }

    /// Scrolls to bottom.
    pub fn scroll_bottom(&mut self, visible_height: usize) {
        self.nav.scroll_bottom(self.refs.len(), visible_height);
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
            .scroll_line_down(n, self.refs.len(), visible_height);
    }

    /// Scrolls the viewport up by `n` lines, keeping cursor within the viewport.
    pub fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.nav.scroll_line_up(n, visible_height);
    }

    /// Renders the references view into the writer.
    pub fn render<W: Write>(&self, w: &mut W, width: usize, height: usize) -> Result<()> {
        use std::fmt::Write as _;

        let content_height = height.saturating_sub(2);
        let start = self.scroll_offset();
        let end = (start + content_height).min(self.refs.len());

        if self.refs.is_empty() {
            super::render_empty_list_placeholder(
                w,
                width,
                content_height,
                "  No references found.",
            )?;
            return Ok(());
        }

        let mut badge_buf = String::with_capacity(32);
        let mut line_buf = String::with_capacity(128);

        for row_idx in 0..content_height {
            let item_idx = start + row_idx;
            queue!(w, MoveTo(0, (row_idx + 1) as u16))?;

            if item_idx < end {
                let r = &self.refs[item_idx];
                let is_selected = item_idx == self.cursor();

                if is_selected {
                    queue!(w, SetAttribute(Attribute::Reverse))?;
                }

                badge_buf.clear();
                match r.kind {
                    RefKind::LocalBranch | RefKind::RemoteBranch => {
                        let _ = write!(badge_buf, "[{}]", r.name);
                    }
                    RefKind::Tag => {
                        let _ = write!(badge_buf, "<{}>", r.name);
                    }
                    RefKind::Stash => {
                        badge_buf.push_str("{stash}");
                    }
                    RefKind::Other => {
                        let _ = write!(badge_buf, "({})", r.name);
                    }
                }

                let color = match r.kind {
                    RefKind::LocalBranch => Color::Green,
                    RefKind::RemoteBranch => Color::Red,
                    RefKind::Tag => Color::Yellow,
                    RefKind::Stash => Color::Magenta,
                    RefKind::Other => Color::Cyan,
                };

                let trunc_badge_len =
                    tigrs_core::ansi::truncate_display_width(&badge_buf, 23.min(width)).len();
                badge_buf.truncate(trunc_badge_len);
                let badge_vis = UnicodeWidthStr::width(badge_buf.as_str());
                let badge_target = 24.min(width);
                for _ in 0..badge_target.saturating_sub(badge_vis) {
                    badge_buf.push(' ');
                }
                let badge_width = badge_vis.max(badge_target);

                if !is_selected {
                    queue!(w, SetForegroundColor(color))?;
                }
                write!(w, "{badge_buf}")?;
                if !is_selected {
                    queue!(w, ResetColor)?;
                }

                let rem_after_badge = width.saturating_sub(badge_width);
                let short_oid = r.commit_id.to_hex_with_len(7);
                let trunc_author = tigrs_core::ansi::truncate_display_width(&r.author_name, 16);
                let author_vis = UnicodeWidthStr::width(trunc_author);
                line_buf.clear();
                let _ = write!(line_buf, "{short_oid:>7} {trunc_author}");
                for _ in 0..16usize.saturating_sub(author_vis) {
                    line_buf.push(' ');
                }
                line_buf.push(' ');
                let prefix_width = UnicodeWidthStr::width(line_buf.as_str()) + badge_width;
                let rem = width.saturating_sub(prefix_width);
                let summary_col = tigrs_core::ansi::truncate_display_width(&r.summary, rem);
                line_buf.push_str(summary_col);
                let rendered_rest =
                    tigrs_core::ansi::truncate_display_width(&line_buf, rem_after_badge);

                let line_width = badge_width + UnicodeWidthStr::width(rendered_rest);
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

impl super::View for RefsView {
    fn kind(&self) -> crate::app::layout::ViewKind {
        crate::app::layout::ViewKind::Refs
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

    fn move_down_by(&mut self, n: usize, visible_height: usize) {
        self.move_down(n, visible_height);
    }

    fn move_up_by(&mut self, n: usize, visible_height: usize) {
        self.move_up(n, visible_height);
    }

    fn selected_commit_id(&self) -> Option<tigrs_git::ObjectId> {
        self.selected_ref().map(|entry| entry.commit_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigrs_git::ObjectId;

    fn sample_refs() -> Vec<RefEntry> {
        vec![
            RefEntry {
                name: "main".to_string(),
                full_name: "refs/heads/main".to_string(),
                kind: RefKind::LocalBranch,
                commit_id: ObjectId::from_hex(b"0123456789abcdef0123456789abcdef01234567").unwrap(),
                summary: "Initial commit".to_string(),
                author_name: "Alice".to_string(),
                author_time_secs: 1_700_000_000,
            },
            RefEntry {
                name: "v1.0".to_string(),
                full_name: "refs/tags/v1.0".to_string(),
                kind: RefKind::Tag,
                commit_id: ObjectId::from_hex(b"abcdef0123456789abcdef0123456789abcdef01").unwrap(),
                summary: "Release 1.0".to_string(),
                author_name: "Bob".to_string(),
                author_time_secs: 1_700_001_000,
            },
        ]
    }

    #[test]
    fn test_refs_view_navigation_and_render() {
        let refs = sample_refs();
        let mut view = RefsView::new(refs);
        assert_eq!(view.line_count(), 2);
        assert_eq!(view.cursor(), 0);
        assert_eq!(view.selected_ref().unwrap().name, "main");

        view.move_down(1, 10);
        assert_eq!(view.cursor(), 1);
        assert_eq!(view.selected_ref().unwrap().name, "v1.0");

        // Bounds check
        view.move_down(10, 10);
        assert_eq!(view.cursor(), 1);

        view.move_up(1, 10);
        assert_eq!(view.cursor(), 0);

        view.scroll_bottom(10);
        assert_eq!(view.cursor(), 1);

        view.scroll_top();
        assert_eq!(view.cursor(), 0);

        assert!(view.row_text(0).unwrap().contains("main"));
        assert!(view.row_text(1).unwrap().contains("v1.0"));

        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("[main]"));
    }

    #[test]
    fn test_refs_view_empty_state() {
        let mut view = RefsView::new(Vec::new());
        assert_eq!(view.line_count(), 0);
        assert_eq!(view.cursor(), 0);
        assert!(view.selected_ref().is_none());
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
        assert!(output.contains("No references found"));
    }

    #[test]
    fn test_refs_view_badge_kinds() {
        let oid = ObjectId::from_hex(b"0123456789abcdef0123456789abcdef01234567").unwrap();
        let entries = vec![
            RefEntry {
                name: "origin/feat".to_string(),
                full_name: "refs/remotes/origin/feat".to_string(),
                kind: RefKind::RemoteBranch,
                commit_id: oid,
                summary: "Remote commit".to_string(),
                author_name: "Charlie".to_string(),
                author_time_secs: 1_700_000_000,
            },
            RefEntry {
                name: "stash@{0}".to_string(),
                full_name: "refs/stash".to_string(),
                kind: RefKind::Stash,
                commit_id: oid,
                summary: "WIP on main".to_string(),
                author_name: "Dave".to_string(),
                author_time_secs: 1_700_000_000,
            },
        ];
        let view = RefsView::new(entries);
        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("[origin/feat]"));
        assert!(output.contains("{stash}"));
    }

    #[test]
    fn test_refs_view_paging_and_set_cursor() {
        let oid = ObjectId::from_hex(b"0123456789abcdef0123456789abcdef01234567").unwrap();
        let entries: Vec<RefEntry> = (0..15)
            .map(|i| RefEntry {
                name: format!("branch_{i}"),
                full_name: format!("refs/heads/branch_{i}"),
                kind: RefKind::LocalBranch,
                commit_id: oid,
                summary: format!("Summary {i}"),
                author_name: "Author".to_string(),
                author_time_secs: 1_700_000_000 + i64::from(i),
            })
            .collect();
        let mut view = RefsView::new(entries);
        assert_eq!(view.line_count(), 15);

        view.page_down(5);
        assert_eq!(view.cursor(), 5);

        view.page_down(5);
        assert_eq!(view.cursor(), 10);

        view.page_up(4);
        assert_eq!(view.cursor(), 6);

        view.set_cursor(12, 5);
        assert_eq!(view.cursor(), 12);
        assert_eq!(view.selected_ref().unwrap().name, "branch_12");

        view.set_cursor(999, 5);
        assert_eq!(view.cursor(), 14);
    }

    #[test]
    fn test_refs_view_scroll_line_viewport() {
        let oid = ObjectId::from_hex(b"0123456789abcdef0123456789abcdef01234567").unwrap();
        let entries: Vec<RefEntry> = (0..20)
            .map(|i| RefEntry {
                name: format!("tag_{i}"),
                full_name: format!("refs/tags/tag_{i}"),
                kind: RefKind::Tag,
                commit_id: oid,
                summary: format!("Release {i}"),
                author_name: "Author".to_string(),
                author_time_secs: 1_700_000_000 + i64::from(i),
            })
            .collect();
        let mut view = RefsView::new(entries);

        view.scroll_line_down(5, 10);
        assert_eq!(view.scroll_offset(), 5);
        assert_eq!(view.cursor(), 5);

        view.scroll_line_up(3, 10);
        assert_eq!(view.scroll_offset(), 2);

        view.scroll_line_up(50, 10);
        assert_eq!(view.scroll_offset(), 0);

        // Zero visible height
        view.scroll_line_down(1, 0);
        view.scroll_line_up(1, 0);
    }

    #[test]
    fn test_refs_view_zero_visible_height_and_narrow_render() {
        let oid = ObjectId::from_hex(b"0123456789abcdef0123456789abcdef01234567").unwrap();
        let entries = vec![RefEntry {
            name: "feature/super-long-branch-name-with-many-words".to_string(),
            full_name: "refs/heads/feature/super-long-branch-name-with-many-words".to_string(),
            kind: RefKind::LocalBranch,
            commit_id: oid,
            summary: "Extremely long commit summary on branch ".repeat(5),
            author_name: "Developer".to_string(),
            author_time_secs: 1_700_000_000,
        }];
        let mut view = RefsView::new(entries);

        view.move_down(1, 0);
        view.move_up(1, 0);

        let mut buf = Vec::new();
        view.render(&mut buf, 25, 6).expect("render narrow refs");
        assert!(!buf.is_empty());

        let mut term = crate::headless::HeadlessTerminal::new(25, 6);
        view.render(&mut term, 25, 6).unwrap();
        assert!(
            term.line_text(1).starts_with("[feature/super-long-br"),
            "Long branch badge should truncate cleanly to 23 cols without wrapping: {:?}",
            term.line_text(1)
        );
        assert_eq!(
            term.line_text(2).trim(),
            "",
            "Row 1 must not wrap into row 2 on narrow terminals"
        );
    }
}
