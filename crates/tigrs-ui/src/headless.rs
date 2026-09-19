// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Headless terminal emulator for testing and snapshot verification.
//!
//! Provides an in-memory 2D screen grid that interprets ANSI escape sequences
//! (cursor positioning, screen/line clearing, SGR styles, truecolor and 256 colors)
//! to enable robust headless testing of TUI views without an attached terminal.

use std::io::Write;
use unicode_width::UnicodeWidthChar;

/// An ANSI color attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    /// Standard ANSI black (index 0).
    Black,
    /// Standard ANSI red (index 1).
    Red,
    /// Standard ANSI green (index 2).
    Green,
    /// Standard ANSI yellow (index 3).
    Yellow,
    /// Standard ANSI blue (index 4).
    Blue,
    /// Standard ANSI magenta (index 5).
    Magenta,
    /// Standard ANSI cyan (index 6).
    Cyan,
    /// Standard ANSI white (index 7).
    White,
    /// Bright ANSI black / dark gray (index 8).
    BrightBlack,
    /// Bright ANSI red (index 9).
    BrightRed,
    /// Bright ANSI green (index 10).
    BrightGreen,
    /// Bright ANSI yellow (index 11).
    BrightYellow,
    /// Bright ANSI blue (index 12).
    BrightBlue,
    /// Bright ANSI magenta (index 13).
    BrightMagenta,
    /// Bright ANSI cyan (index 14).
    BrightCyan,
    /// Bright ANSI white (index 15).
    BrightWhite,
    /// 8-bit 256-color palette index (`0..=255`).
    Ansi256(u8),
    /// 24-bit truecolor `(r, g, b)`.
    Rgb(u8, u8, u8),
}

impl Color {
    /// Parses a color name or specifier string into a `Color`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        if s == "default" || s.is_empty() {
            return None;
        }
        match s.as_str() {
            "black" => Some(Self::Black),
            "red" => Some(Self::Red),
            "green" => Some(Self::Green),
            "yellow" => Some(Self::Yellow),
            "blue" => Some(Self::Blue),
            "magenta" => Some(Self::Magenta),
            "cyan" => Some(Self::Cyan),
            "white" => Some(Self::White),
            "brightblack" | "bright-black" | "gray" | "grey" => Some(Self::BrightBlack),
            "brightred" | "bright-red" => Some(Self::BrightRed),
            "brightgreen" | "bright-green" => Some(Self::BrightGreen),
            "brightyellow" | "bright-yellow" => Some(Self::BrightYellow),
            "brightblue" | "bright-blue" => Some(Self::BrightBlue),
            "brightmagenta" | "bright-magenta" => Some(Self::BrightMagenta),
            "brightcyan" | "bright-cyan" => Some(Self::BrightCyan),
            "brightwhite" | "bright-white" => Some(Self::BrightWhite),
            other => {
                if let Some(num_str) = other.strip_prefix("color")
                    && let Ok(n) = num_str.parse::<u8>()
                {
                    return Some(Self::Ansi256(n));
                }
                if let Some(hex_str) = other.strip_prefix('#').or_else(|| other.strip_prefix("0x"))
                    && hex_str.len() == 6
                    && hex_str.is_ascii()
                    && let (Ok(r), Ok(g), Ok(b)) = (
                        u8::from_str_radix(&hex_str[0..2], 16),
                        u8::from_str_radix(&hex_str[2..4], 16),
                        u8::from_str_radix(&hex_str[4..6], 16),
                    )
                {
                    return Some(Self::Rgb(r, g, b));
                }
                if let Ok(n) = other.parse::<u8>() {
                    return Some(Self::Ansi256(n));
                }
                None
            }
        }
    }

    /// Maps an ANSI 16-color index (`0..=15` or SGR code `30..=37` / `90..=97`) to a `Color`.
    #[must_use]
    pub const fn from_ansi16(code: u8) -> Self {
        match code {
            0 | 30 | 40 => Self::Black,
            1 | 31 | 41 => Self::Red,
            2 | 32 | 42 => Self::Green,
            3 | 33 | 43 => Self::Yellow,
            4 | 34 | 44 => Self::Blue,
            5 | 35 | 45 => Self::Magenta,
            6 | 36 | 46 => Self::Cyan,
            7 | 37 | 47 => Self::White,
            8 | 90 | 100 => Self::BrightBlack,
            9 | 91 | 101 => Self::BrightRed,
            10 | 92 | 102 => Self::BrightGreen,
            11 | 93 | 103 => Self::BrightYellow,
            12 | 94 | 104 => Self::BrightBlue,
            13 | 95 | 105 => Self::BrightMagenta,
            14 | 96 | 106 => Self::BrightCyan,
            _ => Self::BrightWhite,
        }
    }

    /// Downsamples this color to the target terminal color capability profile.
    #[must_use]
    pub fn downsample(self, profile: crate::term_cap::ColorProfile) -> Self {
        use crate::term_cap::{ColorProfile, ansi256_to_ansi16, rgb_to_ansi256};
        match profile {
            ColorProfile::Ansi256 => match self {
                Self::Rgb(r, g, b) => Self::Ansi256(rgb_to_ansi256(r, g, b)),
                other => other,
            },
            ColorProfile::Ansi16 => match self {
                Self::Rgb(r, g, b) => {
                    let idx = rgb_to_ansi256(r, g, b);
                    Self::from_ansi16(ansi256_to_ansi16(idx, false))
                }
                Self::Ansi256(idx) => Self::from_ansi16(ansi256_to_ansi16(idx, false)),
                other => other,
            },
            ColorProfile::TrueColor | ColorProfile::Monochrome => self,
        }
    }
}

bitflags::bitflags! {
    /// SGR text attributes carried by a terminal cell or a theme style.
    ///
    /// Collapsing these into one byte keeps [`Cell`] small (the grid holds one
    /// per character position) and reduces attribute comparison — performed for
    /// every cell of every frame by the damage-tracking renderer — to a single
    /// integer compare.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
    pub struct CellAttrs: u8 {
        /// Bold (SGR 1).
        const BOLD = 1 << 0;
        /// Dim / faint (SGR 2).
        const DIM = 1 << 1;
        /// Italic (SGR 3).
        const ITALIC = 1 << 2;
        /// Underline (SGR 4).
        const UNDERLINE = 1 << 3;
        /// Reverse video / inverted colors (SGR 7).
        const REVERSE = 1 << 4;
        /// Strikethrough (SGR 9).
        const STRIKE = 1 << 5;
    }
}

impl CellAttrs {
    /// Each attribute paired with its SGR "on" parameter, in emission order.
    ///
    /// This is the single source of truth shared by the ANSI parser, the diff
    /// theme's escape emitter, and the live terminal renderer.
    pub const SGR_PARAMS: [(Self, u16); 6] = [
        (Self::BOLD, 1),
        (Self::DIM, 2),
        (Self::ITALIC, 3),
        (Self::UNDERLINE, 4),
        (Self::REVERSE, 7),
        (Self::STRIKE, 9),
    ];

    /// Emits SGR escape codes turning on every active attribute.
    pub fn write_sgr<W: std::io::Write>(self, w: &mut W) -> std::io::Result<()> {
        let mut buf = itoa::Buffer::new();
        for (flag, param) in Self::SGR_PARAMS {
            if self.contains(flag) {
                w.write_all(b"\x1b[")?;
                w.write_all(buf.format(param).as_bytes())?;
                w.write_all(b"m")?;
            }
        }
        Ok(())
    }
}

/// A single cell in the terminal grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// Rendered unicode character.
    pub ch: char,
    /// Active SGR text attributes.
    pub attrs: CellAttrs,
    /// Foreground color.
    pub fg: Option<Color>,
    /// Background color.
    pub bg: Option<Color>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            attrs: CellAttrs::empty(),
            fg: None,
            bg: None,
        }
    }
}

/// Internal parser state for ANSI escape sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParserState {
    Ground,
    Escape,
    Csi,
    Osc,
}

/// An in-memory headless terminal with a fixed 2D grid.
#[derive(Debug)]
pub struct HeadlessTerminal {
    width: usize,
    height: usize,
    cursor_x: usize,
    cursor_y: usize,
    /// Row-major cell buffer holding exactly `width * height` cells.
    grid: Vec<Cell>,
    attrs: CellAttrs,
    fg: Option<Color>,
    bg: Option<Color>,
    state: ParserState,
    csi_params: Vec<u16>,
    current_param: u16,
    has_param: bool,
    is_private_csi: bool,
    pending_utf8: arrayvec::ArrayVec<u8, 4>,
}

impl Default for HeadlessTerminal {
    fn default() -> Self {
        Self::new(0, 0)
    }
}

impl HeadlessTerminal {
    /// Creates a new headless terminal of the given dimensions initialized with spaces.
    pub fn new(width: u16, height: u16) -> Self {
        let w = width as usize;
        let h = height as usize;
        let grid = vec![Cell::default(); w * h];
        Self {
            width: w,
            height: h,
            cursor_x: 0,
            cursor_y: 0,
            grid,
            attrs: CellAttrs::empty(),
            fg: None,
            bg: None,
            state: ParserState::Ground,
            csi_params: Vec::with_capacity(8),
            current_param: 0,
            has_param: false,
            is_private_csi: false,
            pending_utf8: arrayvec::ArrayVec::new(),
        }
    }

    /// Returns the terminal width in columns.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Returns the terminal height in rows.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Returns the current cursor position `(col, row)` (0-indexed).
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_x, self.cursor_y)
    }

    /// Sets the write cursor to 0-indexed `(col, row)`, clamped to terminal bounds.
    #[inline]
    pub fn set_cursor(&mut self, col: usize, row: usize) {
        if self.width > 0 {
            self.cursor_x = col.min(self.width - 1);
        }
        if self.height > 0 {
            self.cursor_y = row.min(self.height - 1);
        }
    }

    /// Clears the entire grid back to default empty cells and moves cursor to (0, 0).
    pub fn clear(&mut self) {
        self.grid.fill(Cell::default());
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.reset_styles();
        self.state = ParserState::Ground;
        self.csi_params.clear();
        self.current_param = 0;
        self.has_param = false;
        self.is_private_csi = false;
        self.pending_utf8.clear();
    }

    /// Resizes the terminal grid in-place if dimensions changed and resets all cells and parser state.
    pub fn resize_and_clear(&mut self, width: u16, height: u16) {
        let w = width as usize;
        let h = height as usize;
        if self.width != w || self.height != h {
            self.width = w;
            self.height = h;
            self.grid.clear();
            self.grid.resize(w * h, Cell::default());
        }
        self.clear();
    }

    /// Returns the flat index of `(x, y)`, or `None` if out of bounds.
    fn index_of(&self, x: usize, y: usize) -> Option<usize> {
        (x < self.width && y < self.height).then(|| y * self.width + x)
    }

    /// Returns the half-open flat range covering row `y`, or `None` if out of bounds.
    fn row_range(&self, y: usize) -> Option<std::ops::Range<usize>> {
        (y < self.height).then(|| {
            let start = y * self.width;
            start..start + self.width
        })
    }

    /// Returns a reference to the cell at `(x, y)` if within bounds.
    pub fn cell(&self, x: usize, y: usize) -> Option<&Cell> {
        self.index_of(x, y).and_then(|i| self.grid.get(i))
    }

    /// Returns a mutable reference to the cell at `(x, y)` if within bounds.
    pub fn cell_mut(&mut self, x: usize, y: usize) -> Option<&mut Cell> {
        self.index_of(x, y).and_then(|i| self.grid.get_mut(i))
    }

    /// Returns the whole screen as one row-major slice of `width * height` cells.
    pub fn cells(&self) -> &[Cell] {
        &self.grid
    }

    /// Consumes the terminal and returns the row-major cell buffer.
    pub fn into_cells(self) -> Vec<Cell> {
        self.grid
    }

    /// Returns the row of cells at `y` if within bounds.
    pub fn line_cells(&self, y: usize) -> Option<&[Cell]> {
        self.row_range(y).and_then(|r| self.grid.get(r))
    }

    /// Returns a mutable slice over the row of cells at `y` if within bounds.
    pub fn line_cells_mut(&mut self, y: usize) -> Option<&mut [Cell]> {
        self.row_range(y).and_then(|r| self.grid.get_mut(r))
    }

    /// Returns the text content of line `y`, trimmed of trailing whitespace.
    pub fn line_text(&self, y: usize) -> String {
        if let Some(row) = self.line_cells(y) {
            let s: String = row.iter().filter(|c| c.ch != '\0').map(|c| c.ch).collect();
            s.trim_end().to_string()
        } else {
            String::new()
        }
    }

    /// Returns the exact raw text of line `y`.
    pub fn line_text_raw(&self, y: usize) -> String {
        if let Some(row) = self.line_cells(y) {
            row.iter().filter(|c| c.ch != '\0').map(|c| c.ch).collect()
        } else {
            String::new()
        }
    }

    /// Returns the entire screen text with trailing whitespace trimmed from each line.
    pub fn screen_text(&self) -> String {
        (0..self.height)
            .map(|y| self.line_text(y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Returns the raw screen text with exact line widths.
    pub fn screen_text_raw(&self) -> String {
        (0..self.height)
            .map(|y| self.line_text_raw(y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Returns true if the cell at `(x, y)` has every attribute in `attrs` set.
    pub fn cell_has(&self, x: usize, y: usize, attrs: CellAttrs) -> bool {
        self.cell(x, y).is_some_and(|c| c.attrs.contains(attrs))
    }

    /// Returns true if the cell at `(x, y)` has the reverse attribute set.
    pub fn is_reverse(&self, x: usize, y: usize) -> bool {
        self.cell_has(x, y, CellAttrs::REVERSE)
    }

    /// Returns true if the cell at `(x, y)` has the bold attribute set.
    pub fn is_bold(&self, x: usize, y: usize) -> bool {
        self.cell_has(x, y, CellAttrs::BOLD)
    }

    /// Returns true if the cell at `(x, y)` has the dim attribute set.
    pub fn is_dim(&self, x: usize, y: usize) -> bool {
        self.cell_has(x, y, CellAttrs::DIM)
    }

    /// Returns true if the cell at `(x, y)` has the italic attribute set.
    pub fn is_italic(&self, x: usize, y: usize) -> bool {
        self.cell_has(x, y, CellAttrs::ITALIC)
    }

    /// Returns true if the cell at `(x, y)` has the underline attribute set.
    pub fn is_underline(&self, x: usize, y: usize) -> bool {
        self.cell_has(x, y, CellAttrs::UNDERLINE)
    }

    /// Returns true if the cell at `(x, y)` has the strikethrough attribute set.
    pub fn is_strike(&self, x: usize, y: usize) -> bool {
        self.cell_has(x, y, CellAttrs::STRIKE)
    }

    /// Returns true if any non-blank cell on line `y` has the reverse attribute.
    pub fn line_has_reverse(&self, y: usize) -> bool {
        self.line_cells(y).is_some_and(|row| {
            row.iter()
                .any(|c| c.attrs.contains(CellAttrs::REVERSE) && c.ch != ' ')
        })
    }

    /// Searches for `needle` in the terminal screen and returns its first `(x, y)` cell coordinates.
    pub fn find_text(&self, needle: &str) -> Option<(usize, usize)> {
        for y in 0..self.height {
            let Some(row) = self.line_cells(y) else {
                continue;
            };
            let mut visible = String::with_capacity(self.width);
            let mut byte_to_col = Vec::with_capacity(self.width);
            for (col, cell) in row.iter().enumerate() {
                if cell.ch != '\0' {
                    let start_byte = visible.len();
                    visible.push(cell.ch);
                    while byte_to_col.len() < visible.len() {
                        byte_to_col.push(col);
                    }
                    let _ = start_byte;
                }
            }
            if let Some(byte_idx) = visible.find(needle)
                && let Some(&col) = byte_to_col.get(byte_idx)
            {
                return Some((col, y));
            }
        }
        None
    }

    /// Asserts that line `y` contains `expected` text.
    pub fn assert_line_contains(&self, y: usize, expected: &str) {
        let text = self.line_text(y);
        assert!(
            text.contains(expected),
            "Line {y} expected to contain \"{expected}\", but got:\n\"{text}\""
        );
    }

    /// Formats the screen into a clean visual snapshot string.
    pub fn snapshot(&self) -> String {
        self.screen_text()
    }

    /// Builds a cell holding `ch` with the pen's current colors and attributes.
    fn pen_cell(&self, ch: char) -> Cell {
        Cell {
            ch,
            attrs: self.attrs,
            fg: self.fg,
            bg: self.bg,
        }
    }

    fn reset_styles(&mut self) {
        self.attrs = CellAttrs::empty();
        self.fg = None;
        self.bg = None;
    }

    fn apply_sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            self.reset_styles();
            return;
        }

        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => self.reset_styles(),
                1 => self.attrs.insert(CellAttrs::BOLD),
                2 => self.attrs.insert(CellAttrs::DIM),
                3 => self.attrs.insert(CellAttrs::ITALIC),
                4 => self.attrs.insert(CellAttrs::UNDERLINE),
                7 => self.attrs.insert(CellAttrs::REVERSE),
                9 => self.attrs.insert(CellAttrs::STRIKE),
                21 => self.attrs.remove(CellAttrs::BOLD),
                // SGR 22 is "normal intensity": it clears bold and dim together.
                22 => self.attrs.remove(CellAttrs::BOLD | CellAttrs::DIM),
                23 => self.attrs.remove(CellAttrs::ITALIC),
                24 => self.attrs.remove(CellAttrs::UNDERLINE),
                27 => self.attrs.remove(CellAttrs::REVERSE),
                29 => self.attrs.remove(CellAttrs::STRIKE),
                30 => self.fg = Some(Color::Black),
                31 => self.fg = Some(Color::Red),
                32 => self.fg = Some(Color::Green),
                33 => self.fg = Some(Color::Yellow),
                34 => self.fg = Some(Color::Blue),
                35 => self.fg = Some(Color::Magenta),
                36 => self.fg = Some(Color::Cyan),
                37 => self.fg = Some(Color::White),
                39 => self.fg = None,
                40 => self.bg = Some(Color::Black),
                41 => self.bg = Some(Color::Red),
                42 => self.bg = Some(Color::Green),
                43 => self.bg = Some(Color::Yellow),
                44 => self.bg = Some(Color::Blue),
                45 => self.bg = Some(Color::Magenta),
                46 => self.bg = Some(Color::Cyan),
                47 => self.bg = Some(Color::White),
                49 => self.bg = None,
                90 => self.fg = Some(Color::BrightBlack),
                91 => self.fg = Some(Color::BrightRed),
                92 => self.fg = Some(Color::BrightGreen),
                93 => self.fg = Some(Color::BrightYellow),
                94 => self.fg = Some(Color::BrightBlue),
                95 => self.fg = Some(Color::BrightMagenta),
                96 => self.fg = Some(Color::BrightCyan),
                97 => self.fg = Some(Color::BrightWhite),
                100 => self.bg = Some(Color::BrightBlack),
                101 => self.bg = Some(Color::BrightRed),
                102 => self.bg = Some(Color::BrightGreen),
                103 => self.bg = Some(Color::BrightYellow),
                104 => self.bg = Some(Color::BrightBlue),
                105 => self.bg = Some(Color::BrightMagenta),
                106 => self.bg = Some(Color::BrightCyan),
                107 => self.bg = Some(Color::BrightWhite),
                38 => {
                    if i + 2 < params.len() && params[i + 1] == 5 {
                        self.fg = Some(Color::Ansi256(params[i + 2] as u8));
                        i += 2;
                    } else if i + 4 < params.len() && params[i + 1] == 2 {
                        self.fg = Some(Color::Rgb(
                            params[i + 2] as u8,
                            params[i + 3] as u8,
                            params[i + 4] as u8,
                        ));
                        i += 4;
                    }
                }
                48 => {
                    if i + 2 < params.len() && params[i + 1] == 5 {
                        self.bg = Some(Color::Ansi256(params[i + 2] as u8));
                        i += 2;
                    } else if i + 4 < params.len() && params[i + 1] == 2 {
                        self.bg = Some(Color::Rgb(
                            params[i + 2] as u8,
                            params[i + 3] as u8,
                            params[i + 4] as u8,
                        ));
                        i += 4;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn put_char(&mut self, ch: char) {
        if ch == '\r' {
            self.cursor_x = 0;
            return;
        }
        if ch == '\n' {
            if self.height > 0 {
                self.cursor_y = (self.cursor_y + 1).min(self.height - 1);
            }
            self.cursor_x = 0;
            return;
        }
        if ch == '\t' {
            let next_tab = (self.cursor_x + 8) & !7;
            self.cursor_x = next_tab.min(self.width);
            return;
        }
        if ch == '\x08' {
            self.cursor_x = self.cursor_x.saturating_sub(1);
            return;
        }

        let (ch, char_width) = match UnicodeWidthChar::width(ch) {
            Some(0) => return,
            Some(w) => (ch, w),
            None => (char::REPLACEMENT_CHARACTER, 1),
        };

        if let Some(idx) = self.index_of(self.cursor_x, self.cursor_y) {
            // If overwriting the second half of a wide char, clear the orphaned leading half
            if self.grid[idx].ch == '\0' && self.cursor_x > 0 && idx > 0 {
                self.grid[idx - 1].ch = ' ';
            }
            if char_width > 1 && self.cursor_x + 1 >= self.width {
                // Wide character at last column cannot fit; pad with space
                self.grid[idx] = self.pen_cell(' ');
                if self.cursor_x + 1 < self.width
                    && idx + 1 < self.grid.len()
                    && self.grid[idx + 1].ch == '\0'
                {
                    self.grid[idx + 1].ch = ' ';
                }
                self.cursor_x = self.width;
                return;
            }

            self.grid[idx] = self.pen_cell(ch);

            // If wide character (width 2), fill subsequent cell with '\0' continuation marker
            if char_width > 1 && self.cursor_x + 1 < self.width {
                if idx + 1 < self.grid.len() {
                    if self.cursor_x + 2 < self.width
                        && idx + 2 < self.grid.len()
                        && self.grid[idx + 1].ch != '\0'
                        && self.grid[idx + 2].ch == '\0'
                    {
                        self.grid[idx + 2].ch = ' ';
                    }
                    self.grid[idx + 1] = self.pen_cell('\0');
                }
            } else if char_width == 1
                && self.cursor_x + 1 < self.width
                && idx + 1 < self.grid.len()
                && self.grid[idx + 1].ch == '\0'
            {
                // Overwrote first half of a wide character with a narrow character
                self.grid[idx + 1].ch = ' ';
            }
        }

        self.cursor_x = (self.cursor_x + char_width).min(self.width);
    }

    fn process_bytes(&mut self, data: &[u8]) {
        let mut i = 0;
        let len = data.len();

        while i < len {
            match self.state {
                ParserState::Ground => {
                    let b = data[i];
                    if b == 0x1b {
                        self.state = ParserState::Escape;
                        i += 1;
                    } else if (0x20..=0x7e).contains(&b) {
                        // Fast-path for contiguous printable ASCII runs (skips per-char UnicodeWidth lookup)
                        let base_cell = self.pen_cell(' ');
                        let row_start = self.cursor_y * self.width;
                        while i < len {
                            let c = data[i];
                            if !(0x20..=0x7e).contains(&c) {
                                break;
                            }
                            if self.cursor_x < self.width && self.cursor_y < self.height {
                                let idx = row_start + self.cursor_x;
                                if idx < self.grid.len() {
                                    if self.grid[idx].ch == '\0' && self.cursor_x > 0 && idx > 0 {
                                        self.grid[idx - 1].ch = ' ';
                                    }
                                    if self.cursor_x + 1 < self.width
                                        && idx + 1 < self.grid.len()
                                        && self.grid[idx + 1].ch == '\0'
                                    {
                                        self.grid[idx + 1].ch = ' ';
                                    }
                                    let mut cell = base_cell;
                                    cell.ch = c as char;
                                    self.grid[idx] = cell;
                                }
                                self.cursor_x += 1;
                            }
                            i += 1;
                        }
                    } else if b < 0x80 {
                        self.put_char(b as char);
                        i += 1;
                    } else {
                        // Multibyte UTF-8 decoding
                        let ch_len = match b {
                            0b1100_0000..=0b1101_1111 => 2,
                            0b1110_0000..=0b1110_1111 => 3,
                            0b1111_0000..=0b1111_0111 => 4,
                            _ => 1,
                        };

                        if i + ch_len <= len {
                            if ch_len > 1 {
                                if let Ok(s) = std::str::from_utf8(&data[i..i + ch_len]) {
                                    if let Some(ch) = s.chars().next() {
                                        self.put_char(ch);
                                    }
                                    i += ch_len;
                                } else {
                                    self.put_char(char::REPLACEMENT_CHARACTER);
                                    i += 1;
                                }
                            } else {
                                self.put_char(char::REPLACEMENT_CHARACTER);
                                i += 1;
                            }
                        } else {
                            // Incomplete UTF-8 sequence at end of buffer: save for next write
                            let _ = self.pending_utf8.try_extend_from_slice(&data[i..]);
                            break;
                        }
                    }
                }
                ParserState::Escape => {
                    let b = data[i];
                    match b {
                        b'[' => {
                            self.state = ParserState::Csi;
                            self.csi_params.clear();
                            self.current_param = 0;
                            self.has_param = false;
                            self.is_private_csi = false;
                            i += 1;
                        }
                        b']' => {
                            self.state = ParserState::Osc;
                            i += 1;
                        }
                        0x1b => {
                            // Consecutive escape
                            i += 1;
                        }
                        _ => {
                            // 2-byte escape sequence completed
                            self.state = ParserState::Ground;
                            i += 1;
                        }
                    }
                }
                ParserState::Csi => {
                    let b = data[i];
                    match b {
                        b'0'..=b'9' => {
                            self.current_param = self
                                .current_param
                                .saturating_mul(10)
                                .saturating_add(u16::from(b - b'0'));
                            self.has_param = true;
                            i += 1;
                        }
                        b';' => {
                            self.csi_params.push(if self.has_param {
                                self.current_param
                            } else {
                                0
                            });
                            self.current_param = 0;
                            self.has_param = false;
                            i += 1;
                        }
                        b'?' => {
                            self.is_private_csi = true;
                            i += 1;
                        }
                        b'H' | b'f' => {
                            // Cursor Position: \x1b[{row};{col}H
                            if self.has_param {
                                self.csi_params.push(self.current_param);
                            }
                            let row = self.csi_params.first().copied().unwrap_or(1);
                            let col = self.csi_params.get(1).copied().unwrap_or(1);
                            if self.height > 0 {
                                self.cursor_y =
                                    (row.saturating_sub(1) as usize).min(self.height - 1);
                            }
                            if self.width > 0 {
                                self.cursor_x =
                                    (col.saturating_sub(1) as usize).min(self.width - 1);
                            }
                            self.state = ParserState::Ground;
                            i += 1;
                        }
                        b'A' => {
                            // Cursor Up: \x1b[{n}A
                            if self.has_param {
                                self.csi_params.push(self.current_param);
                            }
                            let n = self.csi_params.first().copied().unwrap_or(1).max(1) as usize;
                            self.cursor_y = self.cursor_y.saturating_sub(n);
                            self.state = ParserState::Ground;
                            i += 1;
                        }
                        b'B' => {
                            // Cursor Down: \x1b[{n}B
                            if self.has_param {
                                self.csi_params.push(self.current_param);
                            }
                            let n = self.csi_params.first().copied().unwrap_or(1).max(1) as usize;
                            if self.height > 0 {
                                self.cursor_y = (self.cursor_y + n).min(self.height - 1);
                            }
                            self.state = ParserState::Ground;
                            i += 1;
                        }
                        b'C' => {
                            // Cursor Forward: \x1b[{n}C
                            if self.has_param {
                                self.csi_params.push(self.current_param);
                            }
                            let n = self.csi_params.first().copied().unwrap_or(1).max(1) as usize;
                            if self.width > 0 {
                                self.cursor_x = (self.cursor_x + n).min(self.width - 1);
                            }
                            self.state = ParserState::Ground;
                            i += 1;
                        }
                        b'D' => {
                            // Cursor Back: \x1b[{n}D
                            if self.has_param {
                                self.csi_params.push(self.current_param);
                            }
                            let n = self.csi_params.first().copied().unwrap_or(1).max(1) as usize;
                            self.cursor_x = self.cursor_x.saturating_sub(n);
                            self.state = ParserState::Ground;
                            i += 1;
                        }
                        b'J' => {
                            // Erase in Display: \x1b[{n}J
                            if self.has_param {
                                self.csi_params.push(self.current_param);
                            }
                            let code = self.csi_params.first().copied().unwrap_or(0);
                            if code == 2 {
                                self.grid.fill(Cell::default());
                            }
                            self.state = ParserState::Ground;
                            i += 1;
                        }
                        b'K' => {
                            // Erase in Line: \x1b[{n}K
                            if self.has_param {
                                self.csi_params.push(self.current_param);
                            }
                            let code = self.csi_params.first().copied().unwrap_or(0);
                            let cursor_x = self.cursor_x;
                            let width = self.width;
                            if let Some(row) = self.line_cells_mut(self.cursor_y) {
                                let span = match code {
                                    // Erase from cursor to end of line
                                    0 => Some(cursor_x.min(width)..width),
                                    // Erase from start to cursor (inclusive)
                                    1 => Some(0..(cursor_x + 1).min(width)),
                                    // Erase entire line
                                    2 => Some(0..width),
                                    _ => None,
                                };
                                if let Some(span) = span {
                                    row[span].fill(Cell::default());
                                }
                            }
                            self.state = ParserState::Ground;
                            i += 1;
                        }
                        b'm' => {
                            // SGR Style
                            if self.has_param {
                                self.csi_params.push(self.current_param);
                            }
                            let params = std::mem::take(&mut self.csi_params);
                            self.apply_sgr(&params);
                            self.csi_params = params;
                            self.state = ParserState::Ground;
                            i += 1;
                        }
                        0x40..=0x7e => {
                            // Final CSI character
                            self.state = ParserState::Ground;
                            i += 1;
                        }
                        _ => {
                            // Parameter or intermediate character, skip
                            i += 1;
                        }
                    }
                }
                ParserState::Osc => {
                    let b = data[i];
                    if b == 0x07 || (b == b'\\' && i > 0 && data[i - 1] == 0x1b) {
                        self.state = ParserState::Ground;
                    }
                    i += 1;
                }
            }
        }
    }
}

impl Write for HeadlessTerminal {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.pending_utf8.is_empty() {
            self.process_bytes(buf);
            return Ok(buf.len());
        }

        // Stitch split UTF-8 codepoint on the stack without heap allocation.
        let prev_len = self.pending_utf8.len();
        let mut stitched = arrayvec::ArrayVec::<u8, 8>::new();
        let _ = stitched.try_extend_from_slice(&self.pending_utf8);
        self.pending_utf8.clear();

        let take = buf.len().min(4);
        let _ = stitched.try_extend_from_slice(&buf[..take]);

        let first_byte = stitched[0];
        let expected_len = match first_byte {
            0b1100_0000..=0b1101_1111 => 2,
            0b1110_0000..=0b1110_1111 => 3,
            0b1111_0000..=0b1111_0111 => 4,
            _ => 1,
        };

        if stitched.len() >= expected_len {
            if let Ok(s) = std::str::from_utf8(&stitched[..expected_len]) {
                if let Some(ch) = s.chars().next() {
                    self.put_char(ch);
                }
                let consumed_from_buf = expected_len.saturating_sub(prev_len);
                self.process_bytes(&buf[consumed_from_buf..]);
            } else {
                // Pending UTF-8 prefix was invalid; emit replacement character and process full buf
                self.put_char(char::REPLACEMENT_CHARACTER);
                self.process_bytes(buf);
            }
        } else {
            let _ = self.pending_utf8.try_extend_from_slice(&stitched);
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_headless_terminal_plain_text() {
        let mut term = HeadlessTerminal::new(20, 5);
        write!(term, "Hello, World!").unwrap();
        assert_eq!(term.line_text(0), "Hello, World!");
        assert_eq!(term.cursor(), (13, 0));
    }

    #[test]
    fn test_headless_terminal_cursor_positioning_and_sgr() {
        let mut term = HeadlessTerminal::new(20, 5);
        // Move to row 2 col 5 (1-based: row 2, col 5 -> (4, 1) in 0-based)
        write!(term, "\x1b[2;5H\x1b[1m\x1b[7mTest\x1b[0m").unwrap();
        assert_eq!(term.cursor(), (8, 1));
        assert_eq!(term.line_text(1), "    Test");
        assert!(term.is_bold(4, 1));
        assert!(term.is_reverse(4, 1));
        assert!(!term.is_bold(8, 1));
        assert!(!term.is_reverse(8, 1));
    }

    #[test]
    fn test_headless_terminal_line_clearing() {
        let mut term = HeadlessTerminal::new(20, 5);
        write!(term, "Full line content here").unwrap();
        assert_eq!(term.line_text(0), "Full line content he"); // Truncated at 20

        // Clear entire line
        write!(term, "\x1b[1;1H\x1b[2KCleared").unwrap();
        assert_eq!(term.line_text(0), "Cleared");
    }

    #[test]
    fn test_headless_terminal_utf8_handling() {
        let mut term = HeadlessTerminal::new(30, 5);
        write!(term, "🦀 Rust TUI — done").unwrap();
        assert_eq!(term.line_text(0), "🦀 Rust TUI — done");
    }

    #[test]
    fn test_headless_terminal_split_utf8_byte_by_byte() {
        let mut term = HeadlessTerminal::new(30, 5);
        let input = "🦀 Rust TUI — done".as_bytes();
        for &b in input {
            term.write_all(&[b]).unwrap();
        }
        assert_eq!(term.line_text(0), "🦀 Rust TUI — done");
    }

    #[test]
    fn test_headless_terminal_comprehensive() {
        let mut term = HeadlessTerminal::new(30, 10);
        assert_eq!(term.width(), 30);
        assert_eq!(term.height(), 10);
        assert_eq!(term.cursor(), (0, 0));

        // Test tabs, backspace, \r, \n
        write!(term, "abc\tdef\x08X\rZ\nNext").unwrap();
        assert_eq!(term.line_text(0), "Zbc     deX");
        assert_eq!(term.line_text(1), "Next");

        // Cursor navigation: Up, Down, Forward, Back
        write!(term, "\x1b[2C").unwrap(); // Forward 2 -> col 6
        assert_eq!(term.cursor(), (6, 1));
        write!(term, "\x1b[1D").unwrap(); // Back 1 -> col 5
        assert_eq!(term.cursor(), (5, 1));
        write!(term, "\x1b[2B").unwrap(); // Down 2 -> row 3
        assert_eq!(term.cursor(), (5, 3));
        write!(term, "\x1b[1A").unwrap(); // Up 1 -> row 2
        assert_eq!(term.cursor(), (5, 2));

        // Clear in line: 0K (cursor to end), 1K (start to cursor)
        write!(term, "\x1b[1;1H1234567890\x1b[1;5H\x1b[0K").unwrap();
        assert_eq!(term.line_text(0), "1234");
        write!(term, "\x1b[1;1H1234567890\x1b[1;5H\x1b[1K").unwrap();
        assert_eq!(term.line_text(0), "     67890");

        // 2J (erase in display)
        write!(term, "\x1b[2J").unwrap();
        assert_eq!(term.line_text(0), "");

        // SGR colors: standard, bright, 256, RGB, resets
        write!(
            term,
            "\x1b[1;1H\x1b[31;42mRedOnGreen\x1b[39;49mDefault\x1b[91;102mBrightRedOnBrightGreen\x1b[0m"
        )
        .unwrap();
        assert_eq!(term.cell(0, 0).unwrap().fg, Some(Color::Red));
        assert_eq!(term.cell(0, 0).unwrap().bg, Some(Color::Green));

        write!(
            term,
            "\x1b[2;1H\x1b[38;5;123;48;5;234m256Color\x1b[38;2;10;20;30;48;2;40;50;60mRgbColor\x1b[0m"
        )
        .unwrap();
        assert_eq!(term.cell(0, 1).unwrap().fg, Some(Color::Ansi256(123)));
        assert_eq!(term.cell(0, 1).unwrap().bg, Some(Color::Ansi256(234)));
        assert_eq!(term.cell(8, 1).unwrap().fg, Some(Color::Rgb(10, 20, 30)));
        assert_eq!(term.cell(8, 1).unwrap().bg, Some(Color::Rgb(40, 50, 60)));

        // SGR 22 (bold off), 27 (reverse off)
        write!(
            term,
            "\x1b[3;1H\x1b[1mBold\x1b[22mNotBold\x1b[7mRev\x1b[27mNotRev"
        )
        .unwrap();
        assert!(!term.is_bold(10, 2));
        assert!(!term.is_reverse(16, 2));
        assert!(term.line_has_reverse(2));

        // Wide unicode character handling
        write!(term, "\x1b[4;1H你好世界").unwrap();
        assert_eq!(term.line_text(3), "你好世界");
        assert_eq!(term.cell(0, 3).unwrap().ch, '你');
        assert_eq!(term.cell(1, 3).unwrap().ch, '\0');

        // Chunked multi-byte UTF-8 split across write calls
        let utf8_bytes = "🚀 rocket".as_bytes();
        let _ = term.write(&utf8_bytes[..2]).unwrap();
        let _ = term.write(&utf8_bytes[2..]).unwrap();
        assert!(term.line_text(3).contains("🚀 rocket"));

        // OSC sequences (e.g. window title: \x1b]0;Title\x07 or \x1b]0;Title\x1b\)
        write!(term, "\x1b]0;Title\x07\x1b]0;Title2\x1b\\Visible").unwrap();
        assert!(term.line_text(3).contains("Visible"));

        // Consecutive escapes and 2-byte escape
        write!(term, "\x1b\x1b[5;1H\x1bMAtRow5").unwrap();
        assert_eq!(term.line_text(4), "AtRow5");

        // Utility inspection methods
        assert!(term.line_cells(0).is_some());
        assert!(!term.line_text_raw(0).is_empty());
        assert!(!term.screen_text().is_empty());
        assert!(!term.screen_text_raw().is_empty());
        assert_eq!(term.snapshot(), term.screen_text());
        assert_eq!(term.find_text("AtRow5"), Some((0, 4)));
        assert_eq!(term.find_text("nonexistent_string_404"), None);
        term.assert_line_contains(4, "AtRow5");

        // Cell mutation and flat-buffer accessors
        if let Some(c) = term.cell_mut(0, 4) {
            c.ch = 'X';
        }
        assert_eq!(term.cell(0, 4).unwrap().ch, 'X');
        let (w, h) = (term.width(), term.height());
        assert_eq!(term.cells().len(), w * h);
        assert_eq!(term.line_cells(4).unwrap().len(), w);
        assert_eq!(term.line_cells_mut(4).unwrap()[0].ch, 'X');
        assert!(term.line_cells(h).is_none());
        assert!(term.line_cells_mut(h).is_none());

        // clear()
        term.clear();
        assert_eq!(term.cursor(), (0, 0));
        assert_eq!(term.line_text(0), "");

        // into_cells()
        let cells = term.into_cells();
        assert_eq!(cells.len(), w * h);
    }

    #[test]
    fn test_headless_terminal_sgr_attributes() {
        let mut term = HeadlessTerminal::new(40, 5);
        // Test SGR 2 (dim), SGR 3 (italic), SGR 4 (underline), SGR 9 (strike)
        write!(
            term,
            "\x1b[1;1H\x1b[2mDim\x1b[22m\x1b[3mItalic\x1b[23m\x1b[4mUnder\x1b[24m\x1b[9mStrike\x1b[29m"
        )
        .unwrap();

        assert!(term.is_dim(0, 0));
        assert!(!term.is_dim(3, 0));

        assert!(term.is_italic(3, 0));
        assert!(!term.is_italic(9, 0));

        assert!(term.is_underline(9, 0));
        assert!(!term.is_underline(14, 0));

        assert!(term.is_strike(14, 0));
        assert!(!term.is_strike(20, 0));

        // Test SGR 0 resets all attributes
        write!(term, "\x1b[2;1H\x1b[1;2;3;4;7;9mAllOn\x1b[0mAllOff").unwrap();
        assert!(term.is_bold(0, 1));
        assert!(term.is_dim(0, 1));
        assert!(term.is_italic(0, 1));
        assert!(term.is_underline(0, 1));
        assert!(term.is_reverse(0, 1));
        assert!(term.is_strike(0, 1));

        assert!(!term.is_bold(5, 1));
        assert!(!term.is_dim(5, 1));
        assert!(!term.is_italic(5, 1));
        assert!(!term.is_underline(5, 1));
        assert!(!term.is_reverse(5, 1));
        assert!(!term.is_strike(5, 1));
    }

    #[test]
    fn test_color_from_ansi16_and_downsample() {
        use crate::term_cap::ColorProfile;

        for code in 0..=15 {
            let c = Color::from_ansi16(code);
            // Verify each ANSI16 code downsamples to itself
            assert_eq!(c.downsample(ColorProfile::Ansi16), c);
            assert_eq!(c.downsample(ColorProfile::Ansi256), c);
            assert_eq!(c.downsample(ColorProfile::TrueColor), c);
        }
        assert_eq!(Color::from_ansi16(99), Color::BrightWhite);

        let rgb = Color::Rgb(200, 100, 50);
        assert_eq!(rgb.downsample(ColorProfile::TrueColor), rgb);
        assert_eq!(rgb.downsample(ColorProfile::Monochrome), rgb);
        assert!(matches!(
            rgb.downsample(ColorProfile::Ansi256),
            Color::Ansi256(_)
        ));
        assert!(!matches!(
            rgb.downsample(ColorProfile::Ansi16),
            Color::Rgb(_, _, _) | Color::Ansi256(_)
        ));

        let c256 = Color::Ansi256(196);
        assert_eq!(c256.downsample(ColorProfile::Ansi256), c256);
        assert!(!matches!(
            c256.downsample(ColorProfile::Ansi16),
            Color::Ansi256(_)
        ));
    }
}
