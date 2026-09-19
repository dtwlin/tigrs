// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! TUI view modules (main commit log, pager, diff, status).

pub mod blame_view;
pub mod blob_view;
pub mod cursor;
pub mod diff_view;
pub mod grep_view;
pub mod help_view;
pub mod log_view;
pub mod main_view;
pub mod pager_view;
pub mod reflog_view;
pub mod refs_view;
pub mod stash_view;
pub mod status_view;
pub mod tree_view;

pub use blame_view::{BlameHistoryEntry, BlameView};
pub use blob_view::BlobView;
pub use cursor::ViewportCursor;
pub use diff_view::{DiffLineType, DiffView, DiffViewLine};
pub use grep_view::{GrepMatch, GrepView};
pub use help_view::{HelpRow, HelpView};
pub use log_view::{LogLine, LogLineKind, LogView};
pub use main_view::{ChangesKind, ChangesRow, LARGE_HISTORY_THRESHOLD, MainRow, MainView};
pub use pager_view::PagerView;
pub use reflog_view::ReflogView;
pub use refs_view::RefsView;
pub use stash_view::StashView;
pub use status_view::{StatusRow, StatusView};
pub use tree_view::{TreeRow, TreeView};

/// Common polymorphic interface for all list- and document-based TUI views.
///
/// Implementors only supply the four required accessors plus [`View::render`];
/// every cursor and viewport motion is derived from [`ViewportCursor`] by the
/// default method bodies below. Views with bespoke selection rules (for
/// example [`StatusView`], which skips non-selectable section headers) override
/// the individual motions they need.
pub trait View {
    /// Returns the semantic view kind.
    fn kind(&self) -> crate::app::layout::ViewKind;

    /// Total number of items or lines in the view.
    fn line_count(&self) -> usize;

    /// Returns the view's cursor and scroll viewport.
    fn nav(&self) -> &ViewportCursor;

    /// Returns the view's cursor and scroll viewport mutably.
    fn nav_mut(&mut self) -> &mut ViewportCursor;

    /// Renders the view into `writer` within a `width` x `height` pane.
    fn render(
        &self,
        writer: &mut dyn std::io::Write,
        width: u16,
        height: u16,
        options: &crate::options::ViewOptions,
    ) -> tigrs_core::error::Result<()>;

    /// Current zero-based cursor index.
    fn cursor(&self) -> usize {
        self.nav().cursor
    }

    /// Current vertical scroll offset.
    fn scroll_offset(&self) -> usize {
        self.nav().scroll_offset
    }

    /// Positions cursor at row `pos` and adjusts scroll viewport.
    fn set_cursor(&mut self, pos: usize, visible_height: usize) {
        let total = self.line_count();
        self.nav_mut().set_cursor(pos, total, visible_height);
    }

    /// Moves cursor down by `n` rows.
    fn move_down_by(&mut self, n: usize, visible_height: usize) {
        let total = self.line_count();
        self.nav_mut().move_down_by(n, total, visible_height);
    }

    /// Moves cursor up by `n` rows.
    fn move_up_by(&mut self, n: usize, visible_height: usize) {
        self.nav_mut().move_up_by(n, visible_height);
    }

    /// Moves cursor down by one full page (`visible_height`).
    fn page_down(&mut self, visible_height: usize) {
        let total = self.line_count();
        self.nav_mut().page_down(total, visible_height);
    }

    /// Moves cursor up by one full page (`visible_height`).
    fn page_up(&mut self, visible_height: usize) {
        self.nav_mut().page_up(visible_height);
    }

    /// Moves cursor to the very top (first row).
    fn scroll_top(&mut self) {
        self.nav_mut().scroll_top();
    }

    /// Moves cursor to the very bottom (last row).
    fn scroll_bottom(&mut self, visible_height: usize) {
        let total = self.line_count();
        self.nav_mut().scroll_bottom(total, visible_height);
    }

    /// Scrolls viewport down by `n` rows, keeping cursor in view.
    fn scroll_line_down(&mut self, n: usize, visible_height: usize) {
        let total = self.line_count();
        self.nav_mut().scroll_line_down(n, total, visible_height);
    }

    /// Scrolls viewport up by `n` rows, keeping cursor in view.
    fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.nav_mut().scroll_line_up(n, visible_height);
    }

    /// Scrolls the viewport `n` columns to the left. No-op for views without
    /// horizontal scrolling.
    fn scroll_left(&mut self, _n: usize) {}

    /// Scrolls the viewport `n` columns to the right. No-op for views without
    /// horizontal scrolling.
    fn scroll_right(&mut self, _n: usize) {}

    /// Resets the horizontal scroll to the first column. No-op for views
    /// without horizontal scrolling.
    fn scroll_first_col(&mut self) {}

    /// Returns the commit ID currently selected under the cursor, if applicable.
    fn selected_commit_id(&self) -> Option<tigrs_git::ObjectId> {
        None
    }

    /// Reports whether the view currently has anything worth painting.
    ///
    /// Drives the "first frame with content" heuristic in the render loop, so
    /// views that are meaningful while empty (a binary blob, for instance)
    /// override this.
    fn has_content(&self) -> bool {
        self.line_count() > 0
    }

    /// Returns `true` if row `index` matches `pat`.
    fn matches_search(&self, _index: usize, _pat: &crate::search::SearchPattern) -> bool {
        false
    }

    /// Populates contextual file/directory/lineno variables in `ctx` for macro expansion.
    fn populate_macro_context(&self, _ctx: &mut tigrs_core::macro_ctx::MacroContext) {}
}

/// Renders an empty-state placeholder message on the first content row and clears the remaining
/// viewport rows using EL (`\x1b[K`).
pub(crate) fn render_empty_list_placeholder<W: std::io::Write>(
    w: &mut W,
    width: usize,
    content_height: usize,
    empty_msg: &str,
) -> std::io::Result<()> {
    use crossterm::{cursor::MoveTo, queue};
    use unicode_width::UnicodeWidthStr;

    for row_idx in 0..content_height {
        queue!(w, MoveTo(0, (row_idx + 1) as u16))?;
        if row_idx == 0 {
            let msg_trunc = tigrs_core::ansi::truncate_display_width(empty_msg, width);
            write!(w, "{msg_trunc}")?;
            let msg_w = UnicodeWidthStr::width(msg_trunc);
            write_line_el_or_pad(w, width.saturating_sub(msg_w), false)?;
        } else {
            write_line_el_or_pad(w, width, false)?;
        }
    }
    Ok(())
}

/// Helper to clear the remainder of a line using spaces (if reverse/background active)
/// or EL (`\x1b[K`) if default terminal background.
///
/// Avoids heap allocations from `" ".repeat(...)` and reduces terminal frame bandwidth.
#[inline]
pub(crate) fn write_line_el_or_pad<W: std::io::Write>(
    w: &mut W,
    remaining_cols: usize,
    pad_with_spaces: bool,
) -> std::io::Result<()> {
    static SPACES: &[u8; 256] = &[b' '; 256];
    if pad_with_spaces {
        let mut rem = remaining_cols;
        while rem > 0 {
            let chunk = rem.min(SPACES.len());
            w.write_all(&SPACES[..chunk])?;
            rem -= chunk;
        }
    } else {
        w.write_all(b"\x1b[K")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_empty_list_placeholder_and_line_clearing() {
        let mut out = Vec::new();
        render_empty_list_placeholder(&mut out, 40, 4, "No entries available.").unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("No entries available."));
        assert!(text.contains("\x1b[K"));
    }

    #[test]
    fn test_write_line_el_or_pad_modes() {
        let mut el_buf = Vec::new();
        write_line_el_or_pad(&mut el_buf, 20, false).unwrap();
        assert_eq!(el_buf, b"\x1b[K");

        let mut pad_buf = Vec::new();
        write_line_el_or_pad(&mut pad_buf, 300, true).unwrap();
        assert_eq!(pad_buf.len(), 300);
        assert!(pad_buf.iter().all(|&b| b == b' '));
    }
}
