// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Reusable viewport and cursor navigation component for list-based TUI views.
//!
//! Encapsulates 1D cursor selection, viewport window clamping, line scrolling,
//! and paging logic to eliminate duplicate navigation arithmetic across all views.

/// Manages 1D item selection and vertical scroll viewport offset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewportCursor {
    /// Currently selected item index (0-based).
    pub cursor: usize,
    /// Viewport top index (0-based).
    pub scroll_offset: usize,
}

impl ViewportCursor {
    /// Creates a new cursor at position 0 with scroll offset 0.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cursor: 0,
            scroll_offset: 0,
        }
    }

    /// Creates a cursor with specific initial coordinates.
    #[must_use]
    pub const fn with_coordinates(cursor: usize, scroll_offset: usize) -> Self {
        Self {
            cursor,
            scroll_offset,
        }
    }

    /// Resets cursor and scroll offset to 0.
    pub fn reset(&mut self) {
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    /// Current cursor index.
    #[inline]
    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// Current scroll offset.
    #[inline]
    #[must_use]
    pub const fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Sets cursor to a specific index, clamping to `total - 1` and adjusting scroll.
    pub fn set_cursor(&mut self, pos: usize, total: usize, visible_height: usize) {
        if total == 0 {
            self.cursor = 0;
            self.scroll_offset = 0;
            return;
        }
        self.cursor = pos.min(total - 1);
        if visible_height > 0 {
            let max_scroll = total.saturating_sub(visible_height);
            if self.scroll_offset > max_scroll {
                self.scroll_offset = max_scroll;
            }
        }
        self.adjust_scroll(visible_height);
    }

    /// Moves cursor up by one item. Returns `true` if the cursor moved.
    pub fn move_up(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            if self.cursor < self.scroll_offset {
                self.scroll_offset = self.cursor;
            }
            true
        } else {
            false
        }
    }

    /// Moves cursor down by one item. Returns `true` if the cursor moved.
    pub fn move_down(&mut self, total: usize, visible_height: usize) -> bool {
        if total == 0 {
            return false;
        }
        if self.cursor + 1 < total {
            self.cursor += 1;
            self.adjust_scroll(visible_height);
            true
        } else {
            false
        }
    }

    /// Moves cursor down by `n` items, clamping to `total - 1` and adjusting scroll.
    pub fn move_down_by(&mut self, n: usize, total: usize, visible_height: usize) {
        if total == 0 {
            return;
        }
        let max_idx = total.saturating_sub(1);
        self.cursor = (self.cursor + n).min(max_idx);
        self.adjust_scroll(visible_height);
    }

    /// Moves cursor up by `n` items and adjusts scroll.
    pub fn move_up_by(&mut self, n: usize, visible_height: usize) {
        self.cursor = self.cursor.saturating_sub(n);
        self.adjust_scroll(visible_height);
    }

    /// Moves cursor up by `visible_height` lines (or at least 1).
    pub fn page_up(&mut self, visible_height: usize) {
        let step = visible_height.max(1);
        self.cursor = self.cursor.saturating_sub(step);
        self.adjust_scroll(visible_height);
    }

    /// Moves cursor down by `visible_height` lines (or at least 1).
    pub fn page_down(&mut self, total: usize, visible_height: usize) {
        if total == 0 {
            return;
        }
        let step = visible_height.max(1);
        self.cursor = (self.cursor + step).min(total.saturating_sub(1));
        self.adjust_scroll(visible_height);
    }

    /// Moves cursor to the first item (index 0).
    pub fn scroll_top(&mut self) {
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    /// Moves cursor to the last item (`total - 1`).
    pub fn scroll_bottom(&mut self, total: usize, visible_height: usize) {
        if total == 0 {
            return;
        }
        self.cursor = total.saturating_sub(1);
        self.adjust_scroll(visible_height);
    }

    /// Scrolls the viewport down by `n` lines, keeping the cursor inside the viewport.
    pub fn scroll_line_down(&mut self, n: usize, total: usize, visible_height: usize) {
        if total == 0 || visible_height == 0 {
            return;
        }
        let max_scroll = total.saturating_sub(visible_height);
        self.scroll_offset = (self.scroll_offset + n).min(max_scroll);
        if self.cursor < self.scroll_offset {
            self.cursor = self.scroll_offset;
        }
    }

    /// Scrolls the viewport up by `n` lines, keeping the cursor inside the viewport.
    pub fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        if visible_height > 0 && self.cursor >= self.scroll_offset + visible_height {
            self.cursor = (self.scroll_offset + visible_height).saturating_sub(1);
        }
    }

    /// Adjusts viewport offset so that `cursor` is visible within `visible_height`.
    pub fn adjust_scroll(&mut self, visible_height: usize) {
        if visible_height == 0 {
            return;
        }
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        } else if self.cursor >= self.scroll_offset + visible_height {
            self.scroll_offset = self.cursor + 1 - visible_height;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_viewport_cursor_initial_state() {
        let nav = ViewportCursor::new();
        assert_eq!(nav.cursor(), 0);
        assert_eq!(nav.scroll_offset(), 0);
    }

    #[test]
    fn test_viewport_cursor_empty_list_safety() {
        let mut nav = ViewportCursor::new();
        assert!(!nav.move_down(0, 10));
        assert!(!nav.move_up());
        nav.page_down(0, 10);
        assert_eq!(nav.cursor(), 0);
        nav.scroll_bottom(0, 10);
        assert_eq!(nav.cursor(), 0);
        nav.scroll_line_down(5, 0, 10);
        assert_eq!(nav.scroll_offset(), 0);
    }

    #[test]
    fn test_viewport_cursor_movement_and_scrolling() {
        let mut nav = ViewportCursor::new();
        let total = 50;
        let visible_height = 10;

        // Move down within viewport
        for i in 1..10 {
            assert!(nav.move_down(total, visible_height));
            assert_eq!(nav.cursor(), i);
            assert_eq!(nav.scroll_offset(), 0);
        }

        // 10th move down scrolls viewport
        assert!(nav.move_down(total, visible_height));
        assert_eq!(nav.cursor(), 10);
        assert_eq!(nav.scroll_offset(), 1);

        // Page down
        nav.page_down(total, visible_height);
        assert_eq!(nav.cursor(), 20);
        assert_eq!(nav.scroll_offset(), 11);

        // Page up
        nav.page_up(visible_height);
        assert_eq!(nav.cursor(), 10);
        assert_eq!(nav.scroll_offset(), 10);

        // Scroll to bottom
        nav.scroll_bottom(total, visible_height);
        assert_eq!(nav.cursor(), 49);
        assert_eq!(nav.scroll_offset(), 40);

        // Scroll to top
        nav.scroll_top();
        assert_eq!(nav.cursor(), 0);
        assert_eq!(nav.scroll_offset(), 0);
    }

    #[test]
    fn test_viewport_cursor_line_scrolling_keeps_cursor_visible() {
        let mut nav = ViewportCursor::new();
        let total = 30;
        let visible_height = 5;

        // Scroll line down by 3
        nav.scroll_line_down(3, total, visible_height);
        assert_eq!(nav.scroll_offset(), 3);
        assert_eq!(nav.cursor(), 3); // Cursor pulled down to viewport top

        // Move cursor to bottom of viewport (index 7)
        nav.cursor = 7;

        // Scroll line up by 2
        nav.scroll_line_up(2, visible_height);
        assert_eq!(nav.scroll_offset(), 1);
        assert_eq!(nav.cursor(), 5); // Cursor pulled up to viewport bottom (1 + 5 - 1 = 5)
    }

    #[test]
    fn test_viewport_cursor_move_by() {
        let mut nav = ViewportCursor::new();
        let total = 20;
        let visible_height = 5;

        nav.move_down_by(4, total, visible_height);
        assert_eq!(nav.cursor(), 4);
        assert_eq!(nav.scroll_offset(), 0);

        nav.move_down_by(3, total, visible_height);
        assert_eq!(nav.cursor(), 7);
        assert_eq!(nav.scroll_offset(), 3);

        nav.move_up_by(2, visible_height);
        assert_eq!(nav.cursor(), 5);
        assert_eq!(nav.scroll_offset(), 3);

        nav.move_up_by(10, visible_height);
        assert_eq!(nav.cursor(), 0);
        assert_eq!(nav.scroll_offset(), 0);
    }
}
