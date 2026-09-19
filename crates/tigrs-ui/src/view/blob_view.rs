// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Interactive blob view for displaying file contents with syntax highlighting and line numbers.

use super::ViewportCursor;
use crate::highlight::{IncrementalHighlighter, MAX_HIGHLIGHT_LINES};
use std::io::Write;
use std::sync::Mutex;
use tigrs_core::ansi::truncate_display_width;
use tigrs_core::error::Result;
use tigrs_git::{BlobContent, ObjectId, format_human_size};

/// View displaying file contents at a given commit.
#[derive(Debug)]
pub struct BlobView {
    /// Commit ID the file was loaded from.
    commit_oid: ObjectId,
    /// Path of the file relative to repository root.
    path: String,
    /// Blob object ID.
    blob_oid: ObjectId,
    /// Size in bytes.
    size: usize,
    /// Whether the file is binary.
    is_binary: bool,
    /// Zero-allocation contiguous line buffer of text.
    raw_lines: tigrs_core::LineBuffer,
    /// Incremental highlighter, None if binary or all lines highlighted or exceeds max.
    highlighter: Mutex<Option<IncrementalHighlighter>>,
    /// Syntax-highlighted lines (ANSI `TrueColor` escaped).
    highlighted_lines: Mutex<Vec<String>>,
    /// Active cursor line index and scroll offset.
    nav: ViewportCursor,
}

impl BlobView {
    /// Creates a new `BlobView` from a `BlobContent` object and associated commit OID.
    pub fn new(commit_oid: ObjectId, blob: BlobContent) -> Self {
        Self::new_with_options(commit_oid, blob, &crate::options::ViewOptions::default())
    }

    /// Creates a new `BlobView` using explicit `ViewOptions` (for syntax highlighting and theme).
    pub fn new_with_options(
        commit_oid: ObjectId,
        blob: BlobContent,
        options: &crate::options::ViewOptions,
    ) -> Self {
        let total = blob.lines.len();
        let (highlighter, highlighted_lines) = if !options.syntax_highlighting
            || blob.is_binary
            || total == 0
            || total > MAX_HIGHLIGHT_LINES
        {
            (None, Vec::new())
        } else {
            let mut inc = IncrementalHighlighter::with_theme(&blob.path, &options.syntax_theme);
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

        Self {
            commit_oid,
            path: tigrs_core::ansi::sanitize_string(blob.path),
            blob_oid: blob.oid,
            size: blob.size,
            is_binary: blob.is_binary,
            raw_lines: blob.lines,
            highlighter: Mutex::new(highlighter),
            highlighted_lines: Mutex::new(highlighted_lines),
            nav: ViewportCursor::new(),
        }
    }

    /// Rebuilds syntax highlighting for all visible lines when `syntax_highlighting` or `syntax_theme` changes.
    pub fn refresh_highlighting(&mut self, options: &crate::options::ViewOptions) {
        let total = self.raw_lines.len();
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
        for idx in 0..initial_count {
            hl.push(inc.highlight_next(&self.raw_lines[idx]));
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
        let target = target_line.min(self.raw_lines.len());
        if current_count >= target {
            return;
        }

        for idx in current_count..target {
            lines_guard.push(highlighter.highlight_next(&self.raw_lines[idx]));
        }

        if lines_guard.len() >= self.raw_lines.len() {
            *highlighter_guard = None;
        }
    }

    /// Returns the number of lines currently highlighted.
    #[must_use]
    pub fn highlighted_lines_len(&self) -> usize {
        self.highlighted_lines.lock().map_or(0, |l| l.len())
    }

    /// Returns the commit ID.
    pub fn commit_oid(&self) -> ObjectId {
        self.commit_oid
    }

    /// Returns the file path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the blob OID.
    pub fn blob_oid(&self) -> ObjectId {
        self.blob_oid
    }

    /// Returns the file size in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Returns whether the file is binary.
    pub fn is_binary(&self) -> bool {
        self.is_binary
    }

    /// Returns total line count.
    pub fn line_count(&self) -> usize {
        self.raw_lines.len()
    }

    /// Returns current cursor line index.
    pub fn cursor(&self) -> usize {
        self.nav.cursor()
    }

    /// Returns the zero-allocation contiguous line buffer of raw text lines.
    pub fn raw_lines(&self) -> &tigrs_core::LineBuffer {
        &self.raw_lines
    }

    /// Moves cursor up by one line.
    pub fn move_up(&mut self) {
        self.nav.move_up();
    }

    /// Moves cursor up by `n` lines and adjusts scroll.
    pub fn move_up_by(&mut self, n: usize, visible_height: usize) {
        self.nav.move_up_by(n, visible_height);
    }

    /// Moves cursor down by one line.
    pub fn move_down(&mut self, visible_height: usize) {
        self.nav.move_down(self.raw_lines.len(), visible_height);
    }

    /// Moves cursor down by `n` lines and adjusts scroll.
    pub fn move_down_by(&mut self, n: usize, visible_height: usize) {
        self.nav
            .move_down_by(n, self.raw_lines.len(), visible_height);
    }

    /// Moves cursor up by a page.
    pub fn page_up(&mut self, visible_height: usize) {
        self.nav.page_up(visible_height);
    }

    /// Moves cursor down by a page.
    pub fn page_down(&mut self, visible_height: usize) {
        self.nav.page_down(self.raw_lines.len(), visible_height);
    }

    /// Moves cursor to top of file.
    pub fn move_to_top(&mut self) {
        self.nav.scroll_top();
    }

    /// Moves cursor to bottom of file.
    pub fn move_to_bottom(&mut self, visible_height: usize) {
        self.nav.scroll_bottom(self.raw_lines.len(), visible_height);
    }

    /// Sets cursor position with bounds checking.
    pub fn set_cursor(&mut self, cursor: usize, visible_height: usize) {
        self.nav
            .set_cursor(cursor, self.raw_lines.len(), visible_height);
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
            .scroll_line_down(n, self.raw_lines.len(), visible_height);
    }

    /// Scrolls the viewport up by `n` lines, keeping cursor within the viewport.
    pub fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.nav.scroll_line_up(n, visible_height);
    }

    /// Renders the blob view to the terminal using default options.
    pub fn render<W: Write>(&self, writer: &mut W, width: u16, height: u16) -> Result<()> {
        self.render_with_options(
            writer,
            width,
            height,
            &crate::options::ViewOptions::default(),
        )
    }

    /// Renders the blob view to the terminal with specified options.
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
        let formatted_sz = format_human_size(self.size as u64);

        // 1. Header bar (Row 1)
        let total_lines = self.raw_lines.len();
        let title_raw = if self.is_binary {
            format!("[blob] {short_oid}:{} (binary, {formatted_sz})", self.path)
        } else {
            format!(
                "[blob] {short_oid}:{} - {total_lines} lines ({formatted_sz})",
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
                "\x1b[2;1H\x1b[2K\x1b[33m[Binary file: {} bytes - cannot display]\x1b[0m",
                self.size
            )?;
            for row_idx in 1..visible_height {
                let term_row = row_idx + 2;
                write!(writer, "\x1b[{term_row};1H\x1b[2K~")?;
            }
        } else {
            let gutter_w = total_lines.to_string().len().max(3);
            let content_max_w = w.saturating_sub(gutter_w + 3);

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

                if line_idx < self.raw_lines.len() {
                    let line_num = line_idx + 1;
                    let is_cursor = line_idx == self.cursor();
                    let raw_line = if options.syntax_highlighting {
                        highlighted
                            .get(line_idx)
                            .map_or(&self.raw_lines[line_idx], String::as_str)
                    } else {
                        &self.raw_lines[line_idx]
                    };
                    let expanded = crate::diff::expand_tabs(raw_line, options.tab_size);
                    let line_content =
                        tigrs_core::ansi::truncate_visible_width(&expanded, content_max_w);

                    write!(writer, "\x1b[{term_row};1H\x1b[2K")?;

                    if is_cursor {
                        // Highlighted gutter with cursor indicator
                        write!(
                            writer,
                            "\x1b[7m\x1b[1m{line_num:>gutter_w$} >\x1b[0m {line_content}\x1b[0m"
                        )?;
                    } else {
                        // Dim gutter with vertical separator bar (uses terminal default foreground + DIM
                        // so line numbers automatically adapt to both dark and light terminal themes)
                        let sep = match options.line_graphics {
                            tigrs_core::config::LineGraphics::Ascii => '|',
                            tigrs_core::config::LineGraphics::Utf8 => '│',
                        };
                        write!(
                            writer,
                            "\x1b[2m{line_num:>gutter_w$} {sep}\x1b[0m {line_content}\x1b[0m"
                        )?;
                    }
                } else {
                    write!(writer, "\x1b[{term_row};1H\x1b[2K~")?;
                }
            }
        }

        // 3. Status bar (Row height)
        let total = self.raw_lines.len();
        let current = if total == 0 { 0 } else { self.cursor() + 1 };
        let pct = (current * 100).checked_div(total).unwrap_or(100);

        let status_left = if self.is_binary {
            format!("[blob] binary ({formatted_sz}) - {} [q: back]", self.path)
        } else {
            format!(
                "[blob] line {current} of {total} ({pct}%) - {} [q: back]",
                self.path
            )
        };

        let status_line = truncate_display_width(&status_left, w);
        let status_padded = tigrs_core::ansi::pad_display_width(status_line, w);
        write!(writer, "\x1b[{height};1H\x1b[7m{status_padded}\x1b[0m")?;

        Ok(())
    }
}

impl super::View for BlobView {
    fn kind(&self) -> crate::app::layout::ViewKind {
        crate::app::layout::ViewKind::Blob
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
        self.raw_lines()
            .get(index)
            .is_some_and(|line| pat.is_match(line))
    }

    fn selected_commit_id(&self) -> Option<tigrs_git::ObjectId> {
        Some(self.commit_oid())
    }

    fn has_content(&self) -> bool {
        self.line_count() > 0 || self.is_binary()
    }

    fn populate_macro_context(&self, ctx: &mut tigrs_core::macro_ctx::MacroContext) {
        ctx.file = Some(self.path().to_string());
        ctx.lineno = Some(self.cursor() + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigrs_git::{BlobContent, ObjectId};

    fn make_test_blob(is_binary: bool) -> BlobContent {
        let oid = ObjectId::from_hex(b"1122334455667788990011223344556677889900").unwrap();
        if is_binary {
            BlobContent {
                oid,
                path: "image.png".to_string(),
                size: 1024,
                is_binary: true,
                lines: tigrs_core::LineBuffer::empty(),
            }
        } else {
            BlobContent {
                oid,
                path: "hello.rs".to_string(),
                size: 40,
                is_binary: false,
                lines: vec![
                    "fn main() {".to_string(),
                    "    println!(\"hello\");".to_string(),
                    "}".to_string(),
                ]
                .into(),
            }
        }
    }

    #[test]
    fn test_blob_view_navigation() {
        let blob = make_test_blob(false);
        let commit_oid = ObjectId::from_hex(b"0011223344556677889900112233445566778899").unwrap();
        let mut view = BlobView::new(commit_oid, blob);

        assert_eq!(view.line_count(), 3);
        assert_eq!(view.cursor(), 0);

        view.move_down(10);
        assert_eq!(view.cursor(), 1);

        view.move_down(10);
        assert_eq!(view.cursor(), 2);

        // Clamped at bottom
        view.move_down(10);
        assert_eq!(view.cursor(), 2);

        view.move_up();
        assert_eq!(view.cursor(), 1);

        view.move_to_top();
        assert_eq!(view.cursor(), 0);
    }

    #[test]
    fn test_blob_view_render_text_and_binary() {
        let commit_oid = ObjectId::from_hex(b"0011223344556677889900112233445566778899").unwrap();

        // Text rendering
        let text_blob = make_test_blob(false);
        let text_view = BlobView::new(commit_oid, text_blob);
        let mut buf = Vec::new();
        text_view.render(&mut buf, 80, 24).expect("render text");
        let rendered_text = String::from_utf8_lossy(&buf);

        assert!(rendered_text.contains("[blob] 0011223:hello.rs - 3 lines"));
        assert!(rendered_text.contains("1 >"));
        let clean = tigrs_core::ansi::strip_control_chars(&rendered_text);
        assert!(clean.contains("fn main()"));

        // Binary rendering
        let bin_blob = make_test_blob(true);
        let bin_view = BlobView::new(commit_oid, bin_blob);
        let mut bin_buf = Vec::new();
        bin_view.render(&mut bin_buf, 80, 24).expect("render bin");
        let rendered_bin = String::from_utf8_lossy(&bin_buf);

        assert!(rendered_bin.contains("Binary file: 1024 bytes"));
    }

    #[test]
    fn test_blob_view_paging_and_set_cursor() {
        let commit_oid = ObjectId::from_hex(b"0011223344556677889900112233445566778899").unwrap();
        let lines: Vec<String> = (0..25).map(|i| format!("println!(\"{i}\");")).collect();
        let blob = BlobContent {
            oid: commit_oid,
            path: "src/many_lines.rs".to_string(),
            size: 500,
            is_binary: false,
            lines: lines.into(),
        };
        let mut view = BlobView::new(commit_oid, blob);
        assert_eq!(view.line_count(), 25);
        assert_eq!(view.path(), "src/many_lines.rs");
        assert_eq!(view.size(), 500);
        assert!(!view.is_binary());
        assert_eq!(view.raw_lines().len(), 25);

        view.page_down(5);
        assert_eq!(view.cursor(), 5);

        view.page_down(5);
        assert_eq!(view.cursor(), 10);

        view.page_up(4);
        assert_eq!(view.cursor(), 6);

        view.set_cursor(18, 5);
        assert_eq!(view.cursor(), 18);

        view.move_to_bottom(5);
        assert_eq!(view.cursor(), 24);

        view.move_to_top();
        assert_eq!(view.cursor(), 0);
    }

    #[test]
    fn test_blob_view_scroll_line_viewport() {
        let commit_oid = ObjectId::from_hex(b"0011223344556677889900112233445566778899").unwrap();
        let lines: Vec<String> = (0..30).map(|i| format!("code line {i}")).collect();
        let blob = BlobContent {
            oid: commit_oid,
            path: "src/scroll.rs".to_string(),
            size: 600,
            is_binary: false,
            lines: lines.into(),
        };
        let mut view = BlobView::new(commit_oid, blob);

        view.scroll_line_down(5, 10);
        assert_eq!(view.scroll_offset(), 5);
        assert_eq!(view.cursor(), 5);

        view.scroll_line_up(3, 10);
        assert_eq!(view.scroll_offset(), 2);

        view.scroll_line_up(100, 10);
        assert_eq!(view.scroll_offset(), 0);
    }

    #[test]
    fn test_blob_view_empty_and_zero_height() {
        let commit_oid = ObjectId::from_hex(b"0011223344556677889900112233445566778899").unwrap();
        let blob = BlobContent {
            oid: commit_oid,
            path: "empty.rs".to_string(),
            size: 0,
            is_binary: false,
            lines: tigrs_core::LineBuffer::empty(),
        };
        let mut view = BlobView::new(commit_oid, blob);

        assert_eq!(view.line_count(), 0);
        view.move_down(10);
        assert_eq!(view.cursor(), 0);
        view.move_up();
        assert_eq!(view.cursor(), 0);
        view.page_down(5);
        assert_eq!(view.cursor(), 0);
        view.page_up(5);
        assert_eq!(view.cursor(), 0);
        view.move_to_bottom(5);
        assert_eq!(view.cursor(), 0);
        view.set_cursor(10, 5);
        assert_eq!(view.cursor(), 0);

        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).expect("render empty");
        assert!(!buf.is_empty());
    }

    #[test]
    fn test_blob_view_lazy_syntax_highlighting() {
        let commit_oid = ObjectId::from_hex(b"0011223344556677889900112233445566778899").unwrap();
        // Generate 1000 lines of Rust code
        let lines: Vec<String> = (0..1000)
            .map(|i| format!("fn test_{i}() {{ let val = {i} * 2; }}"))
            .collect();

        let blob = BlobContent {
            oid: commit_oid,
            path: "src/big_file.rs".to_string(),
            size: 30_000,
            is_binary: false,
            lines: lines.into(),
        };

        let mut view = BlobView::new(commit_oid, blob);
        // On initial construction, only initial chunk (128 lines) is highlighted
        assert_eq!(view.highlighted_lines_len(), 128);

        // Rendering viewport of 24 lines should not need to highlight much beyond 128
        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).unwrap();
        assert_eq!(view.highlighted_lines_len(), 128);

        // Scroll down to line 300
        view.set_cursor(300, 24);
        view.render(&mut buf, 80, 24).unwrap();
        // Highlighting has lazily advanced past line 300
        assert!(view.highlighted_lines_len() >= 300);
        assert!(view.highlighted_lines_len() < 1000);

        // Move to the very bottom
        view.move_to_bottom(24);
        view.render(&mut buf, 80, 24).unwrap();
        // Now all 1000 lines have been highlighted lazily
        assert_eq!(view.highlighted_lines_len(), 1000);
    }
}
