// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Interactive help view displaying active keybindings and commands.

use super::ViewportCursor;
use crate::options::{LineGraphics, ViewOptions};
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, SetAttribute};
use std::io::Write;
use tigrs_core::error::Result;
use unicode_width::UnicodeWidthStr;

/// Width of the left selection indicator gutter (`"  "` or `"▸ "`).
const GUTTER_COL_WIDTH: usize = 2;
/// Fixed display column width for the key shortcut column (`<= 16` chars + `>= 2` spaces).
const KEY_COL_WIDTH: usize = 18;
/// Fixed display column width for the canonical action/command column (`<= 20` chars + `>= 2` spaces).
const ACTION_COL_WIDTH: usize = 22;

/// A row in the help view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpRow {
    /// Section header (e.g. `View Switching`).
    Section(String),
    /// Keybinding description row (`(key, action, description)`).
    Binding {
        /// Formatted key sequence column.
        key: String,
        /// Canonical action name or command column.
        action: String,
        /// Human-readable description of the binding.
        description: String,
    },
    /// Blank spacer line.
    Blank,
}

/// The interactive help view component.
#[derive(Debug)]
pub struct HelpView {
    rows: Vec<HelpRow>,
    nav: ViewportCursor,
}

impl Default for HelpView {
    fn default() -> Self {
        Self::new()
    }
}

impl HelpView {
    /// Creates a new help view populated with canonical tigrs keybindings.
    #[allow(clippy::too_many_lines)]
    pub fn new() -> Self {
        let mut rows = Vec::new();

        let add_section = |rows: &mut Vec<HelpRow>, title: &str| {
            if !rows.is_empty() {
                rows.push(HelpRow::Blank);
            }
            rows.push(HelpRow::Section(title.to_string()));
        };

        let add_binding = |rows: &mut Vec<HelpRow>, key: &str, action: &str, desc: &str| {
            rows.push(HelpRow::Binding {
                key: key.to_string(),
                action: action.to_string(),
                description: desc.to_string(),
            });
        };

        add_section(
            &mut rows,
            &format!("About tigrs {}", tigrs_core::APP_VERSION),
        );
        add_binding(
            &mut rows,
            "Version",
            &format!("tigrs {}", tigrs_core::APP_VERSION),
            "Fast, memory-safe Git TUI in pure Rust (/ to search, q to close)",
        );
        add_binding(
            &mut rows,
            "v, --version",
            ":version",
            "Show application version (v key, CLI flag, or :version prompt)",
        );

        add_section(&mut rows, "View Switching");
        add_binding(
            &mut rows,
            "m",
            "view-main",
            "Open commit history graph and split diff pane",
        );
        add_binding(
            &mut rows,
            "d",
            "view-diff",
            "Open full commit or worktree diff view",
        );
        add_binding(
            &mut rows,
            "l",
            "view-log",
            "Open commit log view with full messages and diffstats",
        );
        add_binding(
            &mut rows,
            "L",
            "view-reflog",
            "Open Git reflog history view for HEAD or branch",
        );
        add_binding(
            &mut rows,
            "t",
            "view-tree",
            "Browse repository directory tree at selected commit",
        );
        add_binding(
            &mut rows,
            "f",
            "view-blob",
            "Open syntax-highlighted file blob content view",
        );
        add_binding(
            &mut rows,
            "b",
            "view-blame",
            "Annotate file lines with originating commit and author",
        );
        add_binding(
            &mut rows,
            "r",
            "view-refs",
            "Browse local branches, remote-tracking refs, and tags",
        );
        add_binding(
            &mut rows,
            "s, S",
            "view-status",
            "Inspect working tree status; stage, unstage, or discard",
        );
        add_binding(
            &mut rows,
            "c",
            "view-stage",
            "Open interactive patch staging view for files and hunks",
        );
        add_binding(
            &mut rows,
            "y",
            "view-stash",
            "Browse and inspect saved Git stash entries",
        );
        add_binding(
            &mut rows,
            "g",
            "view-grep",
            "Search tracked repository files with regex or text",
        );
        add_binding(
            &mut rows,
            "p",
            "view-pager",
            "Open scrollable text pager view",
        );
        add_binding(
            &mut rows,
            "h, ?",
            "view-help",
            "Toggle this interactive keybinding reference panel",
        );
        add_binding(
            &mut rows,
            "q, <Esc>",
            "view-close",
            "Close active view (or exit if on the last open view)",
        );
        add_binding(
            &mut rows,
            "Q, <C-c>",
            "quit",
            "Quit tigrs immediately from any open view",
        );

        add_section(&mut rows, "Navigation");
        add_binding(
            &mut rows,
            "j, <Down>",
            "move-down",
            "Move selection cursor down one row",
        );
        add_binding(
            &mut rows,
            "k, <Up>",
            "move-up",
            "Move selection cursor up one row",
        );
        add_binding(
            &mut rows,
            "J / K",
            "next / previous",
            "Step to next or previous commit while keeping split diff open",
        );
        add_binding(
            &mut rows,
            "<C-d> / <C-u>",
            "half-page-down",
            "Scroll viewport down (<C-d>) or up (<C-u>) by half a page",
        );
        add_binding(
            &mut rows,
            "<PgDn>, <Space>",
            "page-down",
            "Scroll down by one full page",
        );
        add_binding(
            &mut rows,
            "<PgUp>, -",
            "page-up",
            "Scroll up by one full page",
        );
        add_binding(
            &mut rows,
            "g, <Home>",
            "scroll-top",
            "Jump to the first line of the active view (:0)",
        );
        add_binding(
            &mut rows,
            "G, <End>",
            "scroll-bottom",
            "Jump to the last line of the active view",
        );
        add_binding(
            &mut rows,
            "H",
            "goto-head",
            "Jump cursor directly to the HEAD commit in Main view",
        );
        add_binding(
            &mut rows,
            "<Enter>",
            "enter",
            "Open selected commit diff, directory, blob, or fold section",
        );
        add_binding(
            &mut rows,
            "<Tab>",
            "view-next",
            "Switch focus between primary and secondary split panes",
        );
        add_binding(
            &mut rows,
            "<, <Backspace>",
            "back / parent",
            "Navigate to parent directory (Tree) or parent commit (Blame)",
        );
        add_binding(
            &mut rows,
            "<Left> / <Right>",
            "scroll-left / right",
            "Pan horizontally across wide lines and side-by-side diffs",
        );
        add_binding(
            &mut rows,
            "|, 0",
            "scroll-first-col",
            "Reset horizontal pan back to the first column",
        );

        add_section(&mut rows, "Diff & Code Review");
        add_binding(
            &mut rows,
            "v",
            "toggle-diff-layout",
            "Switch between Unified and Side-by-Side diff layout",
        );
        add_binding(
            &mut rows,
            "w / W",
            "toggle-word-diff",
            "Toggle inline word diff (w) or ignore-whitespace mode (W)",
        );
        add_binding(
            &mut rows,
            "S",
            "toggle-syntax",
            "Toggle code syntax highlighting in Diff, Blob, and Blame",
        );
        add_binding(
            &mut rows,
            "T",
            "cycle-syntax-theme",
            "Cycle through code syntax highlighting color themes",
        );
        add_binding(
            &mut rows,
            "M",
            ":toggle color-moved",
            "Toggle moved-block detection and coloring (--color-moved)",
        );
        add_binding(
            &mut rows,
            "za, <Enter>",
            "toggle-fold",
            "Fold/unfold file section, jump from diffstat, or expand @@ hunk",
        );
        add_binding(
            &mut rows,
            "zM / zR",
            "fold-all / unfold",
            "Collapse (zM) or expand (zR) all file sections in diff",
        );
        add_binding(
            &mut rows,
            "} / {, zn / zp",
            "next-file / prev",
            "Jump to next (} / zn) or previous ({ / zp) file section",
        );
        add_binding(
            &mut rows,
            ") / (, @, zj/zk",
            "next-hunk / prev",
            "Jump to next () / @ / zj) or previous (( / zk) diff hunk",
        );
        add_binding(
            &mut rows,
            "+, = / _",
            "expand / shrink",
            "Expand (+/=) or shrink (_) context around current hunk (+/-10)",
        );
        add_binding(
            &mut rows,
            "] / [",
            "toggle-diff-context",
            "Increase (]) or decrease ([) global diff context lines",
        );
        add_binding(
            &mut rows,
            "zW",
            ":toggle wrap-lines",
            "Toggle soft line wrapping for long diff lines",
        );
        add_binding(
            &mut rows,
            "i",
            "file-details",
            "Toggle expanded file mode and object OID metadata header",
        );
        add_binding(
            &mut rows,
            "y",
            "yank-diff",
            "Copy current diff line, hunk, or commit SHA to clipboard",
        );

        add_section(&mut rows, "Status & Staging");
        add_binding(
            &mut rows,
            "u",
            "status-update",
            "Stage or unstage selected file (Status) or diff hunk (Stage)",
        );
        add_binding(
            &mut rows,
            "1",
            "stage-single-line",
            "Stage or unstage the single diff line under the cursor",
        );
        add_binding(
            &mut rows,
            "2 / \\",
            "stage-part / split",
            "Stage contiguous change chunk (2) or split current hunk (\\)",
        );
        add_binding(
            &mut rows,
            "!",
            "status-revert",
            "Discard unstaged working-tree changes in file or hunk",
        );
        add_binding(
            &mut rows,
            "e",
            "edit",
            "Open selected file at cursor line in external $EDITOR",
        );

        add_section(&mut rows, "Options & Configuration");
        add_binding(
            &mut rows,
            "o",
            "options",
            "Open interactive Options & Config drawer (Space/←/→, / search, p save)",
        );
        add_binding(
            &mut rows,
            "O",
            "maximize",
            "Maximize or restore the focused split view pane",
        );
        add_binding(
            &mut rows,
            "#, .",
            "toggle-line-number",
            "Toggle line numbers in the active view",
        );
        add_binding(
            &mut rows,
            "D / A / X",
            "toggle-columns",
            "Cycle Date format (D), Author format (A), or Commit SHA column (X)",
        );
        add_binding(
            &mut rows,
            "G / F",
            "toggle-graph / refs",
            "Toggle revision graph (G) or branch/tag badges (F) in Main view",
        );
        add_binding(
            &mut rows,
            "I / i",
            "toggle-sort",
            "Toggle ascending/descending sort order (I) or sort field (i)",
        );
        add_binding(
            &mut rows,
            "*, &",
            "toggle-spotlight",
            "Highlight commits by the selected commit's author in Main view",
        );
        add_binding(
            &mut rows,
            "% / ^",
            "toggle-filters",
            "Toggle pathspec file filter (%) or revision range filter (^)",
        );
        add_binding(
            &mut rows,
            "~",
            "toggle-graphics",
            "Cycle between UTF-8 box-drawing and ASCII line graphics",
        );
        add_binding(
            &mut rows,
            "R, <F5>",
            "refresh",
            "Reload and re-index repository state",
        );
        add_binding(
            &mut rows,
            "z",
            "stop-loading",
            "Stop active background commit or diff loading task",
        );

        add_section(&mut rows, "Search");
        add_binding(
            &mut rows,
            "/",
            "search",
            "Search forward for regex or substring in active view",
        );
        add_binding(
            &mut rows,
            "?",
            "search-back",
            "Search backward for regex or substring in active view",
        );
        add_binding(
            &mut rows,
            "n / N",
            "find-next / prev",
            "Jump to next (n) or previous (N) search match",
        );

        add_section(&mut rows, "Prompt & Colon Commands");
        add_binding(
            &mut rows,
            ":",
            "prompt",
            "Open command prompt (supports Tab completion and ↑/↓ history)",
        );
        add_binding(
            &mut rows,
            ":set <opt>=<val>",
            ":set",
            "Assign any view or diff option (e.g. :set ui-theme = dracula)",
        );
        add_binding(
            &mut rows,
            ":toggle <opt>",
            ":toggle",
            "Cycle any option value (e.g. :toggle diff-layout)",
        );
        add_binding(
            &mut rows,
            ":save-config",
            ":wconfig",
            "Persist modified options to ~/.config/tigrs/config.toml",
        );
        add_binding(
            &mut rows,
            ":<number>",
            ":goto-line",
            "Jump directly to a 1-based line number (e.g. :42)",
        );
        add_binding(
            &mut rows,
            ":!<command>",
            ":exec",
            "Run external shell command with %(commit)/%(file) expansion",
        );

        Self {
            rows,
            nav: ViewportCursor::new(),
        }
    }

    /// Total number of rows in the help view.
    pub fn line_count(&self) -> usize {
        self.rows.len()
    }

    /// Selected row index.
    pub fn cursor(&self) -> usize {
        self.nav.cursor()
    }

    /// Moves cursor down.
    pub fn move_down(&mut self, n: usize, visible_height: usize) {
        self.nav.move_down_by(n, self.rows.len(), visible_height);
    }

    /// Moves cursor up.
    pub fn move_up(&mut self, n: usize, visible_height: usize) {
        self.nav.move_up_by(n, visible_height);
    }

    /// Scrolls to top.
    pub fn scroll_top(&mut self) {
        self.nav.scroll_top();
    }

    /// Moves cursor page down.
    pub fn page_down(&mut self, visible_height: usize) {
        self.nav.page_down(self.rows.len(), visible_height);
    }

    /// Moves cursor page up.
    pub fn page_up(&mut self, visible_height: usize) {
        self.nav.page_up(visible_height);
    }

    /// Sets cursor position.
    pub fn set_cursor(&mut self, pos: usize, visible_height: usize) {
        self.nav.set_cursor(pos, self.rows.len(), visible_height);
    }

    /// Returns searchable text for a given row index.
    pub fn row_text(&self, idx: usize) -> Option<String> {
        match self.rows.get(idx)? {
            HelpRow::Section(title) => Some(title.clone()),
            HelpRow::Binding {
                key,
                action,
                description,
            } => Some(format!("{key} {action} {description}")),
            HelpRow::Blank => None,
        }
    }

    /// Scrolls to bottom.
    pub fn scroll_bottom(&mut self, visible_height: usize) {
        self.nav.scroll_bottom(self.rows.len(), visible_height);
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

    /// Renders the help view into the writer using default UTF-8 line graphics.
    pub fn render<W: Write>(&self, w: &mut W, width: usize, height: usize) -> Result<()> {
        self.render_with_graphics(w, width, height, LineGraphics::Utf8)
    }

    /// Renders the help view into the writer honoring the active `LineGraphics` mode.
    pub fn render_with_graphics<W: Write>(
        &self,
        w: &mut W,
        width: usize,
        height: usize,
        line_graphics: LineGraphics,
    ) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }

        let ver = tigrs_core::APP_VERSION;
        let title_left = format!("[help] tigrs {ver} - Keybindings & Commands Reference");
        let title_right = " /:Search  o:Options  q:Close ";
        let title_raw = format_bar_with_right_hint(&title_left, title_right, width);
        write!(w, "\x1b[1;1H\x1b[7m\x1b[1m{title_raw}\x1b[0m")?;

        let content_height = height.saturating_sub(2);
        let start = self.scroll_offset();
        let end = (start + content_height).min(self.rows.len());

        let (rule_str, cursor_glyph) = match line_graphics {
            LineGraphics::Utf8 => ("─", "▸ "),
            LineGraphics::Ascii => ("-", "> "),
        };

        for row_idx in 0..content_height {
            let item_idx = start + row_idx;
            queue!(w, MoveTo(0, (row_idx + 1) as u16))?;

            if item_idx < end {
                let is_selected = item_idx == self.cursor();
                if is_selected {
                    queue!(w, SetAttribute(Attribute::Reverse))?;
                }

                match &self.rows[item_idx] {
                    HelpRow::Section(title) => {
                        let gutter = if is_selected {
                            tigrs_core::ansi::truncate_display_width(cursor_glyph, GUTTER_COL_WIDTH)
                        } else {
                            "  "
                        };
                        let header_text = format!("{gutter:<GUTTER_COL_WIDTH$}{title}");
                        let header_trunc =
                            tigrs_core::ansi::truncate_display_width(&header_text, width);
                        let header_w = UnicodeWidthStr::width(header_trunc);

                        if is_selected {
                            write!(w, "\x1b[1m{header_trunc}")?;
                        } else {
                            write!(w, "\x1b[1;36m{header_trunc}")?;
                        }

                        let rem = width.saturating_sub(header_w);
                        if rem > 1 && !is_selected {
                            write!(w, "\x1b[22;2;90m ")?;
                            for _ in 0..(rem - 1) {
                                write!(w, "{rule_str}")?;
                            }
                            write!(w, "\x1b[0m")?;
                        } else {
                            if !is_selected {
                                write!(w, "\x1b[0m")?;
                            }
                            super::write_line_el_or_pad(w, rem, is_selected)?;
                        }
                    }
                    HelpRow::Binding {
                        key,
                        action,
                        description,
                    } => {
                        let prefix_cols = GUTTER_COL_WIDTH + KEY_COL_WIDTH + ACTION_COL_WIDTH;
                        let rem = width.saturating_sub(prefix_cols);
                        let col3 = tigrs_core::ansi::truncate_display_width(description, rem);

                        if is_selected {
                            let gutter = tigrs_core::ansi::truncate_display_width(
                                cursor_glyph,
                                GUTTER_COL_WIDTH,
                            );
                            write!(
                                w,
                                "{gutter:<GUTTER_COL_WIDTH$}{key:<KEY_COL_WIDTH$}{action:<ACTION_COL_WIDTH$}{col3}"
                            )?;
                        } else {
                            write!(
                                w,
                                "  \x1b[1;33m{key:<KEY_COL_WIDTH$}\x1b[22;32m{action:<ACTION_COL_WIDTH$}\x1b[39m{col3}"
                            )?;
                        }

                        let key_w = UnicodeWidthStr::width(key.as_str()).max(KEY_COL_WIDTH);
                        let act_w = UnicodeWidthStr::width(action.as_str()).max(ACTION_COL_WIDTH);
                        let desc_w = UnicodeWidthStr::width(col3);
                        let line_width = GUTTER_COL_WIDTH.min(width) + key_w + act_w + desc_w;
                        super::write_line_el_or_pad(
                            w,
                            width.saturating_sub(line_width),
                            is_selected,
                        )?;
                    }
                    HelpRow::Blank => {
                        super::write_line_el_or_pad(w, width, is_selected)?;
                    }
                }

                if is_selected {
                    queue!(w, SetAttribute(Attribute::Reset))?;
                }
            } else {
                super::write_line_el_or_pad(w, width, false)?;
            }
        }

        if height >= 2 {
            let row_total = self.rows.len();
            let current = if row_total == 0 { 0 } else { self.cursor() + 1 };
            let pct = (current * 100).checked_div(row_total).unwrap_or(100);
            let status_left =
                format!("[help] tigrs {ver} - line {current} of {row_total} ({pct}%)");
            let status_right = " j/k:Move  Space/-:Page  /:Search  n/N:Match  q:Close ";
            let status_padded = format_bar_with_right_hint(&status_left, status_right, width);
            write!(w, "\x1b[{height};1H\x1b[7m{status_padded}\x1b[0m")?;
        }

        Ok(())
    }
}

/// Formats a full-width title or status bar with an optional right-aligned key-hint suffix
/// when `width` has enough room for both without colliding.
fn format_bar_with_right_hint(left: &str, right: &str, width: usize) -> String {
    let left_w = UnicodeWidthStr::width(left);
    let right_w = UnicodeWidthStr::width(right);
    if left_w + right_w + 2 <= width {
        let pad = width - left_w - right_w;
        format!("{left}{}{right}", " ".repeat(pad))
    } else {
        let trunc = tigrs_core::ansi::truncate_display_width(left, width);
        tigrs_core::ansi::pad_display_width(trunc, width).into_owned()
    }
}

impl super::View for HelpView {
    fn kind(&self) -> crate::app::layout::ViewKind {
        crate::app::layout::ViewKind::Help
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
        options: &ViewOptions,
    ) -> tigrs_core::error::Result<()> {
        self.render_with_graphics(
            &mut writer,
            width as usize,
            height as usize,
            options.line_graphics,
        )?;
        Ok(())
    }

    fn matches_search(&self, index: usize, pat: &crate::search::SearchPattern) -> bool {
        self.row_text(index).is_some_and(|text| pat.is_match(&text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_help_view_navigation_and_render() {
        let mut view = HelpView::new();
        assert!(view.line_count() > 10);
        assert_eq!(view.cursor(), 0);

        view.move_down(5, 10);
        assert_eq!(view.cursor(), 5);

        view.move_up(2, 10);
        assert_eq!(view.cursor(), 3);

        let total = view.line_count();
        view.scroll_bottom(10);
        assert_eq!(view.cursor(), total - 1);

        view.scroll_top();
        assert_eq!(view.cursor(), 0);

        assert!(view.row_text(0).is_some());

        let mut buf = Vec::new();
        view.render(&mut buf, 80, 24).unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(
            output.contains("view-main")
                || output.contains("Navigation")
                || output.contains("Views")
        );
    }

    #[test]
    fn test_help_view_column_widths_never_collide() {
        let view = HelpView::new();
        for (idx, row) in view.rows.iter().enumerate() {
            if let HelpRow::Binding {
                key,
                action,
                description,
            } = row
            {
                let key_w = UnicodeWidthStr::width(key.as_str());
                let act_w = UnicodeWidthStr::width(action.as_str());
                assert!(
                    key_w <= KEY_COL_WIDTH - 2,
                    "Row {idx} key {key:?} width {key_w} exceeds max {}",
                    KEY_COL_WIDTH - 2
                );
                assert!(
                    act_w <= ACTION_COL_WIDTH - 2,
                    "Row {idx} action {action:?} width {act_w} exceeds max {}",
                    ACTION_COL_WIDTH - 2
                );
                assert!(
                    !description.is_empty(),
                    "Row {idx} ({key}) must have a non-empty description"
                );
            }
        }
    }

    #[test]
    fn test_help_view_semantic_styling_and_ascii_fallback() {
        let mut view = HelpView::new();
        view.move_down(1, 24); // select row 1 (a Binding row) so row 0 (Section) is unselected

        let mut term_utf8 = crate::headless::HeadlessTerminal::new(100, 24);
        view.render_with_graphics(&mut term_utf8, 100, 24, LineGraphics::Utf8)
            .unwrap();

        // Top bar has right-aligned quick hints on wide terminals
        term_utf8.assert_line_contains(0, "/:Search  o:Options  q:Close");
        // Bottom status bar has right-aligned navigation hints on wide terminals
        term_utf8.assert_line_contains(23, "j/k:Move  Space/-:Page");

        // Unselected section row (y = 1) has Bold Cyan header (without "===") and UTF-8 horizontal rule
        term_utf8.assert_line_contains(1, "About tigrs");
        assert!(!term_utf8.line_text(1).contains("==="));
        term_utf8.assert_line_contains(1, "───");
        assert_eq!(
            term_utf8.cell(2, 1).unwrap().fg,
            Some(crate::headless::Color::Cyan)
        );
        assert!(term_utf8.is_bold(2, 1));

        // Unselected binding row (y = 3, item_idx = 2): key in Bold Yellow, action in Green
        assert_eq!(
            term_utf8.cell(2, 3).unwrap().fg,
            Some(crate::headless::Color::Yellow)
        );
        assert!(term_utf8.is_bold(2, 3));
        assert_eq!(
            term_utf8.cell(2 + KEY_COL_WIDTH, 3).unwrap().fg,
            Some(crate::headless::Color::Green)
        );

        // Selected binding row (y = 2, item_idx = 1) has cursor glyph and Reverse video
        term_utf8.assert_line_contains(2, "▸ ");
        assert!(term_utf8.line_has_reverse(2));

        // ASCII line_graphics uses '-' rule and '> ' cursor glyph
        let mut term_ascii = crate::headless::HeadlessTerminal::new(100, 24);
        view.render_with_graphics(&mut term_ascii, 100, 24, LineGraphics::Ascii)
            .unwrap();
        term_ascii.assert_line_contains(1, "---");
        term_ascii.assert_line_contains(2, "> ");
    }

    #[test]
    fn test_help_view_paging_and_narrow_render() {
        let mut view = HelpView::new();
        let total = view.line_count();

        view.page_down(10);
        assert_eq!(view.cursor(), 10);

        view.page_up(10);
        assert_eq!(view.cursor(), 0);

        // Clamped move down
        view.move_down(100_000, 10);
        assert_eq!(view.cursor(), total - 1);

        view.move_up(100_000, 10);
        assert_eq!(view.cursor(), 0);

        // Narrow render check
        let mut narrow_buf = Vec::new();
        view.render(&mut narrow_buf, 20, 10).unwrap();
        assert!(!narrow_buf.is_empty());
    }

    #[test]
    fn test_help_view_row_text_search_and_sections() {
        let view = HelpView::default();
        let mut found_section = false;
        let mut found_binding = false;
        let mut found_blank = false;
        let mut found_diff_layout = false;

        for i in 0..view.line_count() {
            match view.row_text(i) {
                None => found_blank = true,
                Some(text) => {
                    if text.contains("toggle-diff-layout") {
                        found_diff_layout = true;
                    }
                    if text.contains("View Switching") || text.contains("Navigation") {
                        found_section = true;
                    } else if text.contains("view-") || text.contains("move-") {
                        found_binding = true;
                    }
                }
            }
        }

        assert!(found_section);
        assert!(found_binding);
        assert!(found_blank);
        assert!(found_diff_layout);
        assert!(view.row_text(view.line_count() + 10).is_none());
    }

    #[test]
    fn test_help_view_scroll_line_viewport() {
        let mut view = HelpView::new();
        view.scroll_line_down(5, 10);
        assert_eq!(view.scroll_offset(), 5);
        assert_eq!(view.cursor(), 5);

        view.scroll_line_up(3, 10);
        assert_eq!(view.scroll_offset(), 2);

        view.scroll_line_up(100, 10);
        assert_eq!(view.scroll_offset(), 0);

        // Zero visible height should not panic
        view.scroll_line_down(1, 0);
        view.scroll_line_up(1, 0);
    }
}
