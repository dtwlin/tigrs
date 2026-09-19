// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Interactive directory tree view for inspecting repository tree at any revision.

use super::ViewportCursor;
use std::fmt::Write as _;
use std::io::Write;
use tigrs_core::ansi::truncate_display_width;
use tigrs_core::error::Result;
use tigrs_git::{ObjectId, TreeEntry, TreeListing};

/// A row displayed in the Tree View.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeRow {
    /// Upward navigation row (`..`) to parent directory.
    ParentDir,
    /// Normal directory entry (file, directory, symlink, submodule).
    Entry(TreeEntry),
}

impl TreeRow {
    /// Returns the entry name (e.g. `..` or `main.rs`).
    pub fn name(&self) -> &str {
        match self {
            Self::ParentDir => "..",
            Self::Entry(e) => &e.name,
        }
    }

    /// Returns true if this row represents a directory or parent navigation.
    pub fn is_dir(&self) -> bool {
        match self {
            Self::ParentDir => true,
            Self::Entry(e) => e.is_dir(),
        }
    }

    /// Returns the target path if this row represents a file or directory.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::ParentDir => None,
            Self::Entry(e) => Some(&e.path),
        }
    }
}

/// The interactive tree view component.
#[derive(Debug)]
pub struct TreeView {
    /// Commit ID this tree is inspected at.
    commit_oid: ObjectId,
    /// Current path inside tree (`""` for root).
    current_path: String,
    /// Parent path if inside a subdirectory.
    parent_path: Option<String>,
    /// List of display rows.
    rows: Vec<TreeRow>,
    /// Selected row index and viewport scroll.
    nav: ViewportCursor,
}

impl TreeView {
    /// Creates a new `TreeView` from a `TreeListing`.
    pub fn new(listing: TreeListing) -> Self {
        let mut view = Self {
            commit_oid: listing.commit_oid,
            current_path: String::new(),
            parent_path: None,
            rows: Vec::new(),
            nav: ViewportCursor::new(),
        };
        view.set_listing(listing);
        view
    }

    /// Updates the tree listing (e.g. when descending into a folder or ascending to parent).
    pub fn set_listing(&mut self, listing: TreeListing) {
        self.commit_oid = listing.commit_oid;
        self.current_path = tigrs_core::ansi::strip_control_chars(&listing.path).into_owned();
        self.parent_path = listing
            .parent_path
            .map(|p| tigrs_core::ansi::strip_control_chars(&p).into_owned());

        let mut rows = Vec::new();
        // If not in root directory, add ".." parent entry at the top
        if !self.current_path.is_empty() {
            rows.push(TreeRow::ParentDir);
        }
        for entry in listing.entries {
            rows.push(TreeRow::Entry(entry));
        }

        self.rows = rows;
        self.nav.reset();
    }

    /// Returns the commit ID being viewed.
    pub fn commit_oid(&self) -> ObjectId {
        self.commit_oid
    }

    /// Returns the current directory path within the tree.
    pub fn current_path(&self) -> &str {
        &self.current_path
    }

    /// Returns the parent directory path if not at root.
    pub fn parent_path(&self) -> Option<&str> {
        self.parent_path.as_deref()
    }

    /// Returns the currently selected row.
    pub fn selected_row(&self) -> Option<&TreeRow> {
        self.rows.get(self.nav.cursor())
    }

    /// Returns the cursor index.
    pub fn cursor(&self) -> usize {
        self.nav.cursor()
    }

    /// Returns total row count.
    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    /// Returns the textual representation of a row at `idx` for search matching.
    pub fn row_text(&self, idx: usize) -> Option<String> {
        self.rows.get(idx).map(|row| match row {
            TreeRow::ParentDir => "..".to_string(),
            TreeRow::Entry(entry) => {
                if entry.is_dir() {
                    format!("{}/", entry.path)
                } else {
                    entry.path.clone()
                }
            }
        })
    }

    /// Moves cursor up by one row.
    pub fn move_up(&mut self) {
        self.nav.move_up();
    }

    /// Moves cursor up by `n` rows.
    pub fn move_up_by(&mut self, n: usize, visible_height: usize) {
        self.nav.move_up_by(n, visible_height);
    }

    /// Moves cursor down by one row.
    pub fn move_down(&mut self, visible_height: usize) {
        self.nav.move_down(self.rows.len(), visible_height);
    }

    /// Moves cursor down by `n` rows.
    pub fn move_down_by(&mut self, n: usize, visible_height: usize) {
        self.nav.move_down_by(n, self.rows.len(), visible_height);
    }

    /// Moves cursor up by a full page.
    pub fn page_up(&mut self, visible_height: usize) {
        self.nav.page_up(visible_height);
    }

    /// Moves cursor down by a full page.
    pub fn page_down(&mut self, visible_height: usize) {
        self.nav.page_down(self.rows.len(), visible_height);
    }

    /// Moves cursor to the first row.
    pub fn move_to_top(&mut self) {
        self.nav.scroll_top();
    }

    /// Moves cursor to the last row.
    pub fn move_to_bottom(&mut self, visible_height: usize) {
        self.nav.scroll_bottom(self.rows.len(), visible_height);
    }

    /// Sets cursor position with bounds checking.
    pub fn set_cursor(&mut self, cursor: usize, visible_height: usize) {
        self.nav.set_cursor(cursor, self.rows.len(), visible_height);
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
            .scroll_line_down(n, self.rows.len(), visible_height);
    }

    /// Scrolls the viewport up by `n` lines, keeping cursor within the viewport.
    pub fn scroll_line_up(&mut self, n: usize, visible_height: usize) {
        self.nav.scroll_line_up(n, visible_height);
    }

    /// Renders the tree view to the terminal output writer.
    /// Renders the tree view using default options.
    pub fn render<W: Write>(&self, writer: &mut W, width: u16, height: u16) -> Result<()> {
        self.render_with_options(
            writer,
            width,
            height,
            &crate::options::ViewOptions::default(),
        )
    }

    /// Renders the tree view with specified display options.
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
        let display_path = if self.current_path.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", self.current_path)
        };

        // 1. Header bar (Row 1)
        let total_entries = self
            .rows
            .iter()
            .filter(|r| matches!(r, TreeRow::Entry(_)))
            .count();
        let title_raw = format!(
            "[tree] {short_oid}:{display_path} - {total_entries} {}",
            if total_entries == 1 {
                "entry"
            } else {
                "entries"
            }
        );
        let title_line = truncate_display_width(&title_raw, w);
        let title_padded = tigrs_core::ansi::pad_display_width(title_line, w);
        write!(writer, "\x1b[1;1H\x1b[7m\x1b[1m{title_padded}\x1b[0m")?;

        // 2. Body rows (Row 2 .. height - 1)
        let mut formatted_name = String::with_capacity(64);
        let mut row_text = String::with_capacity(128);

        for row_idx in 0..visible_height {
            let term_row = row_idx + 2;
            let line_idx = self.scroll_offset() + row_idx;

            if line_idx < self.rows.len() {
                let row = &self.rows[line_idx];
                let is_cursor = line_idx == self.cursor();
                let (mode, size, name_str, color);
                let formatted_size;

                match row {
                    TreeRow::ParentDir => {
                        mode = "drwxr-xr-x";
                        size = "-";
                        name_str = "..";
                        color = "\x1b[1;34m";
                    }
                    TreeRow::Entry(entry) => {
                        color = match entry.kind {
                            tigrs_git::TreeEntryKind::Tree => "\x1b[1;34m",
                            tigrs_git::TreeEntryKind::Executable => "\x1b[1;32m",
                            tigrs_git::TreeEntryKind::Link => "\x1b[36m",
                            tigrs_git::TreeEntryKind::Commit => "\x1b[35m",
                            _ => "\x1b[0m",
                        };
                        formatted_name.clear();
                        match entry.kind {
                            tigrs_git::TreeEntryKind::Tree => {
                                let _ = write!(formatted_name, "{}/", entry.name);
                            }
                            tigrs_git::TreeEntryKind::Executable => {
                                let _ = write!(formatted_name, "{}*", entry.name);
                            }
                            tigrs_git::TreeEntryKind::Link => {
                                let _ = write!(formatted_name, "{}@", entry.name);
                            }
                            _ => {
                                formatted_name.push_str(&entry.name);
                            }
                        }
                        name_str = formatted_name.as_str();
                        mode = entry.mode_str();
                        formatted_size = entry.formatted_size();
                        size = formatted_size.as_str();
                    }
                }

                row_text.clear();
                if options.file_size {
                    let _ = write!(row_text, "{mode:<10}  {size:>7}  {name_str}");
                } else {
                    let _ = write!(row_text, "{mode:<10}  {name_str}");
                }
                let text_truncated = truncate_display_width(&row_text, w);

                write!(writer, "\x1b[{term_row};1H\x1b[2K")?;

                if is_cursor {
                    let cursor_padded = tigrs_core::ansi::pad_display_width(text_truncated, w);
                    write!(writer, "\x1b[7m{cursor_padded}\x1b[0m")?;
                } else {
                    let prefix_w = 12 + if options.file_size { 9 } else { 0 };
                    if w <= prefix_w {
                        write!(writer, "{text_truncated}")?;
                    } else {
                        let name_truncated = truncate_display_width(name_str, w - prefix_w);
                        if options.file_size {
                            write!(
                                writer,
                                "\x1b[2m{mode:<10}\x1b[0m  {size:>7}  {color}{name_truncated}\x1b[0m"
                            )?;
                        } else {
                            write!(
                                writer,
                                "\x1b[2m{mode:<10}\x1b[0m  {color}{name_truncated}\x1b[0m"
                            )?;
                        }
                    }
                }
            } else {
                write!(writer, "\x1b[{term_row};1H\x1b[2K~")?;
            }
        }

        // 3. Status bar (Row height)
        let total = self.rows.len();
        let current = if total == 0 { 0 } else { self.cursor() + 1 };
        let pct = (current * 100).checked_div(total).unwrap_or(100);

        let selected_desc = match self.selected_row() {
            Some(TreeRow::ParentDir) => "parent directory",
            Some(TreeRow::Entry(e)) if e.is_dir() => "directory",
            Some(TreeRow::Entry(_)) => "file",
            None => "",
        };

        let status_left = if selected_desc.is_empty() {
            format!("[tree] line {current} of {total} ({pct}%) [Enter: open, ,/q: parent/back]")
        } else {
            format!(
                "[tree] line {current} of {total} ({pct}%) - {selected_desc} [Enter: open, ,/q: parent/back]"
            )
        };

        let status_line = truncate_display_width(&status_left, w);
        let status_padded = tigrs_core::ansi::pad_display_width(status_line, w);
        write!(writer, "\x1b[{height};1H\x1b[7m{status_padded}\x1b[0m")?;

        Ok(())
    }
}

impl super::View for TreeView {
    fn kind(&self) -> crate::app::layout::ViewKind {
        crate::app::layout::ViewKind::Tree
    }

    fn line_count(&self) -> usize {
        self.total_rows()
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

    fn selected_commit_id(&self) -> Option<tigrs_git::ObjectId> {
        Some(self.commit_oid())
    }

    fn populate_macro_context(&self, ctx: &mut tigrs_core::macro_ctx::MacroContext) {
        if let Some(path) = self.selected_row().and_then(|r| r.path()) {
            ctx.file = Some(path.to_string());
        }
        ctx.directory = Some(self.current_path().to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigrs_git::{ObjectId, TreeEntry, TreeEntryKind, TreeListing};

    fn make_test_listing() -> TreeListing {
        let oid = ObjectId::from_hex(b"1122334455667788990011223344556677889900").unwrap();
        TreeListing {
            commit_oid: oid,
            path: "src".to_string(),
            parent_path: Some(String::new()),
            entries: vec![
                TreeEntry {
                    name: "view".to_string(),
                    path: "src/view".to_string(),
                    kind: TreeEntryKind::Tree,
                    mode: 0o040_000,
                    oid,
                    size: None,
                },
                TreeEntry {
                    name: "main.rs".to_string(),
                    path: "src/main.rs".to_string(),
                    kind: TreeEntryKind::Blob,
                    mode: 0o100_644,
                    oid,
                    size: Some(1024),
                },
            ],
        }
    }

    #[test]
    fn test_tree_view_navigation() {
        let listing = make_test_listing();
        let mut view = TreeView::new(listing);

        // Rows: ParentDir (0), view (1), main.rs (2)
        assert_eq!(view.total_rows(), 3);
        assert_eq!(view.cursor(), 0);
        assert_eq!(view.selected_row(), Some(&TreeRow::ParentDir));

        view.move_down(10);
        assert_eq!(view.cursor(), 1);
        assert!(matches!(view.selected_row(), Some(TreeRow::Entry(e)) if e.name == "view"));

        view.move_down(10);
        assert_eq!(view.cursor(), 2);
        assert!(matches!(view.selected_row(), Some(TreeRow::Entry(e)) if e.name == "main.rs"));

        // Clamps at bottom
        view.move_down(10);
        assert_eq!(view.cursor(), 2);

        view.move_up();
        assert_eq!(view.cursor(), 1);

        view.move_to_top();
        assert_eq!(view.cursor(), 0);
    }

    #[test]
    fn test_tree_view_render_to_buffer() {
        let listing = make_test_listing();
        let view = TreeView::new(listing);
        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).expect("render");
        let rendered = String::from_utf8_lossy(&buf);

        assert!(rendered.contains("[tree] 1122334:/src - 2 entries"));
        assert!(rendered.contains(".."));
        assert!(rendered.contains("view/"));
        assert!(rendered.contains("main.rs"));
    }

    #[test]
    fn test_tree_view_paging_and_set_cursor() {
        let oid = ObjectId::from_hex(b"1122334455667788990011223344556677889900").unwrap();
        let entries: Vec<TreeEntry> = (0..20)
            .map(|i| TreeEntry {
                name: format!("file_{i}.rs"),
                path: format!("src/file_{i}.rs"),
                kind: TreeEntryKind::Blob,
                mode: 0o100_644,
                oid,
                size: Some(100 + i as u64),
            })
            .collect();
        let listing = TreeListing {
            commit_oid: oid,
            path: "src".to_string(),
            parent_path: Some(String::new()),
            entries,
        };
        let mut view = TreeView::new(listing);
        // 20 entries + 1 ParentDir ("..") = 21 rows
        assert_eq!(view.total_rows(), 21);
        assert_eq!(view.current_path(), "src");
        assert_eq!(view.parent_path(), Some(""));

        assert_eq!(view.row_text(0), Some("..".to_string()));
        assert_eq!(view.row_text(1), Some("src/file_0.rs".to_string()));

        view.page_down(5);
        assert_eq!(view.cursor(), 5);

        view.page_down(5);
        assert_eq!(view.cursor(), 10);

        view.page_up(4);
        assert_eq!(view.cursor(), 6);

        view.set_cursor(15, 5);
        assert_eq!(view.cursor(), 15);

        view.move_to_bottom(5);
        assert_eq!(view.cursor(), 20);

        view.move_to_top();
        assert_eq!(view.cursor(), 0);
    }

    #[test]
    fn test_tree_view_scroll_line_viewport() {
        let oid = ObjectId::from_hex(b"1122334455667788990011223344556677889900").unwrap();
        let entries: Vec<TreeEntry> = (0..25)
            .map(|i| TreeEntry {
                name: format!("mod_{i}.rs"),
                path: format!("src/mod_{i}.rs"),
                kind: TreeEntryKind::Blob,
                mode: 0o100_644,
                oid,
                size: Some(200),
            })
            .collect();
        let listing = TreeListing {
            commit_oid: oid,
            path: "src".to_string(),
            parent_path: Some(String::new()),
            entries,
        };
        let mut view = TreeView::new(listing);

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
    fn test_tree_view_root_empty_and_row_methods() {
        let oid = ObjectId::from_hex(b"1122334455667788990011223344556677889900").unwrap();
        let root_listing = TreeListing {
            commit_oid: oid,
            path: String::new(),
            parent_path: None,
            entries: Vec::new(),
        };
        let mut view = TreeView::new(root_listing);
        assert_eq!(view.total_rows(), 0);
        assert!(view.parent_path().is_none());
        assert!(view.selected_row().is_none());

        view.move_down(5);
        assert_eq!(view.cursor(), 0);
        view.move_up();
        assert_eq!(view.cursor(), 0);
        view.page_down(5);
        assert_eq!(view.cursor(), 0);
        view.page_up(5);
        assert_eq!(view.cursor(), 0);
        view.move_to_bottom(5);
        assert_eq!(view.cursor(), 0);
        view.set_cursor(5, 5);
        assert_eq!(view.cursor(), 0);

        // TreeRow methods
        let parent_row = TreeRow::ParentDir;
        assert_eq!(parent_row.name(), "..");
        assert!(parent_row.is_dir());
        assert_eq!(parent_row.path(), None);

        let file_row = TreeRow::Entry(TreeEntry {
            name: "test.txt".to_string(),
            path: "test.txt".to_string(),
            kind: TreeEntryKind::Blob,
            mode: 0o100_644,
            oid,
            size: Some(12),
        });
        assert_eq!(file_row.name(), "test.txt");
        assert!(!file_row.is_dir());
        assert_eq!(file_row.path(), Some("test.txt"));
    }
}
