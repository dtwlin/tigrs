// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Terminal screen rendering, split compositing, and damage tracking.

use super::AppState;
use super::layout::{PaneLayout, ViewKind, ViewLayout};
use crate::headless::CellAttrs;
use crate::term_cap::ColorProfile;
use arrayvec::ArrayVec;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tigrs_core::error::Result;

static GLOBAL_DEBUG_FRAME_STATS: AtomicBool = AtomicBool::new(false);

/// Enables or disables global frame statistics logging to stderr (`--debug-frame-stats`).
pub fn set_debug_frame_stats(enabled: bool) {
    GLOBAL_DEBUG_FRAME_STATS.store(enabled, Ordering::Relaxed);
}

pub(crate) struct CountingWriter<W> {
    inner: W,
    bytes_written: usize,
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes_written += n;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Renders the currently active view of `app` to `writer`.
pub fn render_active<W: Write>(
    app: &AppState,
    writer: &mut W,
    width: u16,
    height: u16,
) -> Result<()> {
    let t0 = Instant::now();
    let log_stats = app.debug_frame_stats
        || GLOBAL_DEBUG_FRAME_STATS.load(Ordering::Relaxed)
        || std::env::var_os("TIGRS_FRAME_STATS").is_some();
    let mut counting = CountingWriter {
        inner: writer,
        bytes_written: 0,
    };

    // Color-profile reduction and DEC 2026 synchronized output framing happen
    // inside `render_active_direct` so unchanged frames emit 0 bytes.
    let res = render_active_direct(app, &mut counting, width, height);

    if log_stats && res.is_ok() {
        let dur = t0.elapsed();
        let bytes = counting.bytes_written;
        eprintln!("[tigrs frame] render: {dur:?}, bytes: {bytes}, size: {width}x{height}");
    }

    res
}

/// Stateful terminal renderer maintaining double-buffered `ScreenGrid`s and reusable
/// scratch `HeadlessTerminal` surfaces to eliminate per-frame heap allocations and deep cloning.
#[derive(Debug)]
pub struct TerminalRenderer {
    front_grid: ScreenGrid,
    back_grid: ScreenGrid,
    scratch_primary: crate::headless::HeadlessTerminal,
    scratch_secondary: crate::headless::HeadlessTerminal,
    scratch_overlay: crate::headless::HeadlessTerminal,
    pub(crate) has_valid_back_grid: bool,
    cursor_visible: bool,
    frame_spool: Vec<u8>,
    overlay_spool: Vec<u8>,
}

impl Default for TerminalRenderer {
    fn default() -> Self {
        Self {
            front_grid: ScreenGrid::default(),
            back_grid: ScreenGrid::default(),
            scratch_primary: crate::headless::HeadlessTerminal::default(),
            scratch_secondary: crate::headless::HeadlessTerminal::default(),
            scratch_overlay: crate::headless::HeadlessTerminal::default(),
            has_valid_back_grid: false,
            cursor_visible: false,
            frame_spool: Vec::with_capacity(64 * 1024),
            overlay_spool: Vec::with_capacity(4 * 1024),
        }
    }
}

impl TerminalRenderer {
    /// Creates a new `TerminalRenderer`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Invalidates the damage-tracking back buffer so the next frame performs a full repaint.
    pub fn invalidate(&mut self) {
        self.has_valid_back_grid = false;
        self.cursor_visible = false;
    }

    /// Returns whether a valid back grid is currently cached.
    #[must_use]
    pub fn has_valid_back_grid(&self) -> bool {
        self.has_valid_back_grid
    }
}

/// A single contiguous 2D grid of terminal cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenGrid {
    width: usize,
    height: usize,
    cells: Vec<crate::headless::Cell>,
}

impl Default for ScreenGrid {
    fn default() -> Self {
        Self::new(0, 0)
    }
}

impl ScreenGrid {
    /// Creates a new `ScreenGrid` initialized with default cells.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![crate::headless::Cell::default(); width * height],
        }
    }

    /// Resizes and clears the grid in place, reusing existing heap capacity.
    pub fn resize_and_clear(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        let total = width * height;
        self.cells.clear();
        self.cells.resize(total, crate::headless::Cell::default());
    }

    /// Copies cells from a `HeadlessTerminal` into this `ScreenGrid` without allocating.
    pub fn copy_from_headless(&mut self, term: &crate::headless::HeadlessTerminal) {
        let w = term.width();
        let h = term.height();
        self.resize_and_clear(w, h);
        if w == 0 || h == 0 {
            return;
        }
        // Both buffers are row-major `w * h`, so this is a single memcpy.
        let src = term.cells();
        let len = self.cells.len().min(src.len());
        self.cells[..len].copy_from_slice(&src[..len]);
    }

    /// Maps every cell color into the gamut supported by `profile`, in place.
    ///
    /// This replaces re-parsing the emitted byte stream: the grid is the single
    /// source of truth for color, so the reduction is applied once per frame to
    /// structured data instead of once per escape sequence to text.
    ///
    /// `TrueColor` is a no-op. `Monochrome` drops color entirely, leaving only
    /// text attributes, which matches the behavior of the byte-level downgrade.
    pub fn downsample(&mut self, profile: ColorProfile) {
        match profile {
            ColorProfile::TrueColor => {}
            ColorProfile::Monochrome => {
                for cell in &mut self.cells {
                    cell.fg = None;
                    cell.bg = None;
                }
            }
            ColorProfile::Ansi256 | ColorProfile::Ansi16 => {
                for cell in &mut self.cells {
                    cell.fg = cell.fg.map(|c| c.downsample(profile));
                    cell.bg = cell.bg.map(|c| c.downsample(profile));
                }
            }
        }
    }

    /// Creates a `ScreenGrid` from existing rows.
    pub fn from_rows<R: AsRef<[crate::headless::Cell]>>(rows: &[R]) -> Self {
        let height = rows.len();
        let width = rows.first().map_or(0, |r| r.as_ref().len());
        let mut cells = Vec::with_capacity(width * height);
        for row in rows {
            cells.extend_from_slice(row.as_ref());
        }
        Self {
            width,
            height,
            cells,
        }
    }

    /// Returns terminal width in columns.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Returns terminal height in rows.
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Returns a slice of cells for row `y` if within bounds.
    #[must_use]
    pub fn row(&self, y: usize) -> Option<&[crate::headless::Cell]> {
        if y < self.height {
            let start = y * self.width;
            let end = start + self.width;
            self.cells.get(start..end)
        } else {
            None
        }
    }

    /// Returns a mutable slice of cells for row `y` if within bounds.
    pub fn row_mut(&mut self, y: usize) -> Option<&mut [crate::headless::Cell]> {
        if y < self.height {
            let start = y * self.width;
            let end = start + self.width;
            self.cells.get_mut(start..end)
        } else {
            None
        }
    }

    /// Returns a reference to the cell at `(x, y)` if within bounds.
    #[must_use]
    pub fn cell(&self, x: usize, y: usize) -> Option<&crate::headless::Cell> {
        if x < self.width && y < self.height {
            self.cells.get(y * self.width + x)
        } else {
            None
        }
    }

    /// Returns a mutable reference to the cell at `(x, y)` if within bounds.
    pub fn cell_mut(&mut self, x: usize, y: usize) -> Option<&mut crate::headless::Cell> {
        if x < self.width && y < self.height {
            let idx = y * self.width + x;
            self.cells.get_mut(idx)
        } else {
            None
        }
    }

    /// Converts into a nested vector of rows.
    #[cfg(test)]
    #[must_use]
    pub fn into_vec(self) -> Vec<Vec<crate::headless::Cell>> {
        let mut result = Vec::with_capacity(self.height);
        let stride = self.width.max(1);
        for row in self.cells.chunks_exact(stride) {
            result.push(row.to_vec());
        }
        result
    }
}

impl From<Vec<Vec<crate::headless::Cell>>> for ScreenGrid {
    fn from(rows: Vec<Vec<crate::headless::Cell>>) -> Self {
        Self::from_rows(&rows)
    }
}

impl From<&[Vec<crate::headless::Cell>]> for ScreenGrid {
    fn from(rows: &[Vec<crate::headless::Cell>]) -> Self {
        Self::from_rows(rows)
    }
}

/// SGR parameter accumulator for a single pen transition.
///
/// The worst case is a full reset followed by every attribute and both 24-bit
/// colors: `0;1;2;3;4;7;9;38;2;r;g;b;48;2;r;g;b` — 18 parameters. 24 leaves
/// headroom for future attributes while staying entirely on the stack.
type SgrParams = ArrayVec<u16, 24>;

/// Active terminal styling state tracking SGR attributes and colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PenState {
    pub attrs: CellAttrs,
    pub fg: Option<crate::headless::Color>,
    pub bg: Option<crate::headless::Color>,
}

impl PenState {
    #[must_use]
    pub fn from_cell(cell: &crate::headless::Cell) -> Self {
        Self {
            attrs: cell.attrs,
            fg: cell.fg,
            bg: cell.bg,
        }
    }

    /// Appends the SGR parameters needed for `color` to `params`.
    fn append_color_params(params: &mut SgrParams, color: crate::headless::Color, is_bg: bool) {
        use crate::headless::Color;
        let base = if is_bg { 40 } else { 30 };
        let bright_base = if is_bg { 100 } else { 90 };
        match color {
            Color::Black => params.push(base),
            Color::Red => params.push(base + 1),
            Color::Green => params.push(base + 2),
            Color::Yellow => params.push(base + 3),
            Color::Blue => params.push(base + 4),
            Color::Magenta => params.push(base + 5),
            Color::Cyan => params.push(base + 6),
            Color::White => params.push(base + 7),
            Color::BrightBlack => params.push(bright_base),
            Color::BrightRed => params.push(bright_base + 1),
            Color::BrightGreen => params.push(bright_base + 2),
            Color::BrightYellow => params.push(bright_base + 3),
            Color::BrightBlue => params.push(bright_base + 4),
            Color::BrightMagenta => params.push(bright_base + 5),
            Color::BrightCyan => params.push(bright_base + 6),
            Color::BrightWhite => params.push(bright_base + 7),
            Color::Ansi256(c) => {
                let p = if is_bg { 48 } else { 38 };
                params.push(p);
                params.push(5);
                params.push(u16::from(c));
            }
            Color::Rgb(r, g, b) => {
                let p = if is_bg { 48 } else { 38 };
                params.push(p);
                params.push(2);
                params.push(u16::from(r));
                params.push(u16::from(g));
                params.push(u16::from(b));
            }
        }
    }

    /// Appends SGR escape sequence(s) to `spool` to transition from `self` to `target`.
    pub fn transition_to_spool(&mut self, target: &PenState, spool: &mut Vec<u8>) {
        if self == target {
            return;
        }

        // There is no universal "turn this one attribute off" that is safe
        // across terminals, so clearing anything requires a full SGR reset.
        let turned_off = self.attrs.difference(target.attrs) != CellAttrs::empty()
            || (self.fg.is_some() && target.fg.is_none())
            || (self.bg.is_some() && target.bg.is_none());

        let mut params = SgrParams::new();

        // After a reset the entire target state must be re-emitted; otherwise
        // only what is newly enabled or changed needs to go out.
        let attrs_to_emit = if turned_off {
            params.push(0);
            target.attrs
        } else {
            target.attrs.difference(self.attrs)
        };
        for (flag, param) in CellAttrs::SGR_PARAMS {
            if attrs_to_emit.contains(flag) {
                params.push(param);
            }
        }
        if (turned_off || self.fg != target.fg)
            && let Some(fg) = target.fg
        {
            Self::append_color_params(&mut params, fg, false);
        }
        if (turned_off || self.bg != target.bg)
            && let Some(bg) = target.bg
        {
            Self::append_color_params(&mut params, bg, true);
        }

        if !params.is_empty() {
            spool.extend_from_slice(b"\x1b[");
            let mut itoa_buf = itoa::Buffer::new();
            for (idx, p) in params.iter().enumerate() {
                if idx > 0 {
                    spool.push(b';');
                }
                spool.extend_from_slice(itoa_buf.format(*p).as_bytes());
            }
            spool.push(b'm');
        }

        *self = *target;
    }

    /// Emits SGR escape sequence(s) to transition from `self` to `target`.
    #[cfg(test)]
    pub fn transition_to<W: Write>(&mut self, target: &PenState, writer: &mut W) -> Result<()> {
        if self == target {
            return Ok(());
        }
        let mut spool = Vec::with_capacity(32);
        self.transition_to_spool(target, &mut spool);
        if !spool.is_empty() {
            writer.write_all(&spool)?;
        }
        Ok(())
    }
}

/// Appends a single row of cells with differential SGR styling to `spool`.
pub(crate) fn write_row_to_spool(
    row: &[crate::headless::Cell],
    term_row: usize,
    spool: &mut Vec<u8>,
) {
    let mut num_buf = itoa::Buffer::new();
    spool.extend_from_slice(b"\x1b[");
    spool.extend_from_slice(num_buf.format(term_row).as_bytes());
    spool.extend_from_slice(b";1H");

    let default_cell = crate::headless::Cell::default();
    let effective_end = row
        .iter()
        .rposition(|c| *c != default_cell && c.ch != '\0')
        .map_or(0, |idx| {
            if idx + 1 < row.len() && row[idx + 1].ch == '\0' {
                idx + 2
            } else {
                idx + 1
            }
        });

    if effective_end == 0 {
        spool.extend_from_slice(b"\x1b[0m\x1b[K");
        return;
    }

    let mut pen = PenState::default();
    let mut ch_buf = [0u8; 4];

    for cell in &row[..effective_end] {
        if cell.ch == '\0' {
            continue;
        }

        let cell_pen = PenState::from_cell(cell);
        pen.transition_to_spool(&cell_pen, spool);

        if cell.ch.is_ascii() {
            spool.push(cell.ch as u8);
        } else {
            let s = cell.ch.encode_utf8(&mut ch_buf);
            spool.extend_from_slice(s.as_bytes());
        }
    }

    spool.extend_from_slice(b"\x1b[0m");
    if effective_end < row.len() {
        spool.extend_from_slice(b"\x1b[K");
    }
}

/// Renders a single row of cells with differential SGR styling.
#[cfg(test)]
pub(crate) fn write_row_to<W: Write>(
    row: &[crate::headless::Cell],
    term_row: usize,
    writer: &mut W,
) -> Result<()> {
    let mut spool = Vec::with_capacity(row.len() * 2 + 32);
    write_row_to_spool(row, term_row, &mut spool);
    writer.write_all(&spool)?;
    Ok(())
}

/// Appends damaged rows from a contiguous `ScreenGrid` to `spool`.
pub(crate) fn write_screen_grid_to_spool(
    grid: &ScreenGrid,
    previous_grid: Option<&ScreenGrid>,
    spool: &mut Vec<u8>,
) {
    for row_idx in 0..grid.height() {
        let Some(row_cells) = grid.row(row_idx) else {
            continue;
        };
        if let Some(prev) = previous_grid
            && prev.row(row_idx) == Some(row_cells)
        {
            continue;
        }

        write_row_to_spool(row_cells, row_idx + 1, spool);
    }
}

/// Writes damaged rows from a contiguous `ScreenGrid` to `writer` in a single vectored write.
#[cfg(test)]
pub(crate) fn write_screen_grid_to<W: Write>(
    grid: &ScreenGrid,
    previous_grid: Option<&ScreenGrid>,
    writer: &mut W,
) -> Result<()> {
    let mut spool = Vec::with_capacity(grid.width() * grid.height() + 256);
    write_screen_grid_to_spool(grid, previous_grid, &mut spool);
    if !spool.is_empty() {
        writer.write_all(&spool)?;
    }
    Ok(())
}

/// Presents the composed `front_grid`: reduces it to the terminal's color
/// gamut, appends only the rows that differ from the previous frame into `renderer.frame_spool`,
/// and promotes `front_grid` to the back buffer.
fn present_frame_to_spool(renderer: &mut TerminalRenderer, profile: ColorProfile) {
    renderer.front_grid.downsample(profile);
    let prev = renderer.has_valid_back_grid.then_some(&renderer.back_grid);
    write_screen_grid_to_spool(&renderer.front_grid, prev, &mut renderer.frame_spool);
    std::mem::swap(&mut renderer.front_grid, &mut renderer.back_grid);
    renderer.has_valid_back_grid = true;
}

pub(crate) fn render_view_kind<W: Write>(
    app: &AppState,
    kind: ViewKind,
    writer: &mut W,
    width: u16,
    height: u16,
) -> Result<()> {
    if let Some(view) = app.view_ref(kind) {
        view.render(writer, width, height, &app.options)?;
        if app.options.read_only && width >= 40 && height >= 1 {
            let col = width - 5;
            write!(writer, "\x1b[1;{col}H\x1b[7;1;36m[RO] \x1b[0m")?;
        }
    }
    Ok(())
}

/// Dims a pane's title bar to signal that the pane does not hold input focus.
pub(crate) fn demote_pane_title(term: &mut crate::headless::HeadlessTerminal) {
    if let Some(row) = term.line_cells_mut(0) {
        for cell in row.iter_mut() {
            cell.attrs.remove(CellAttrs::REVERSE | CellAttrs::BOLD);
            cell.fg = Some(crate::headless::Color::BrightBlack);
        }
    }
}

/// Maps a standard 16-color ANSI token used by canonical views into the active `UiPalette`'s
/// semantic foreground color, while preserving 24-bit `Color::Rgb` and `Color::Ansi256` spans.
fn map_semantic_fg(
    color: crate::headless::Color,
    palette: &crate::ui_theme::UiPalette,
) -> Option<crate::headless::Color> {
    use crate::headless::Color;
    match color {
        Color::Yellow | Color::BrightYellow => Some(palette.commit_id_fg),
        Color::Green => Some(palette.author_fg),
        Color::BrightGreen => Some(palette.ref_branch_fg),
        Color::Cyan | Color::BrightCyan => Some(palette.date_fg),
        Color::Blue | Color::BrightBlue => Some(palette.ref_head_fg),
        Color::Magenta | Color::BrightMagenta => Some(palette.ref_tag_fg),
        Color::Red | Color::BrightRed => Some(palette.ref_remote_fg),
        Color::BrightBlack => palette.muted_style.fg.or(Some(color)),
        Color::White | Color::BrightWhite | Color::Black => palette.text_fg.or(Some(color)),
        Color::Ansi256(_) | Color::Rgb(..) => Some(color),
    }
}

/// Applies the active `UiPalette` to a rendered `HeadlessTerminal` pane (`scratch_primary` or `scratch_secondary`),
/// styling the title bar, status bar, selected cursor rows, canvas background, and semantic view columns.
pub(crate) fn apply_ui_palette_to_terminal(
    term: &mut crate::headless::HeadlessTerminal,
    options: &crate::options::ViewOptions,
    is_active_pane: bool,
) {
    if options.ui_theme == tigrs_core::config::UiThemeId::Default && options.colors.is_empty() {
        return;
    }

    let palette = options.ui_palette();
    let h = term.height();
    if h == 0 {
        return;
    }

    // 1. Row 0: Pane title bar (`title_bar_active` vs `title_bar_inactive`)
    let title_style = if is_active_pane {
        palette.title_bar_active
    } else {
        palette.title_bar_inactive
    };
    if let Some(row0) = term.line_cells_mut(0) {
        for cell in row0.iter_mut() {
            cell.fg = title_style.fg;
            cell.bg = title_style.bg;
            cell.attrs = title_style.attrs;
        }
    }

    // 2. Last row `h - 1` (if `h >= 2` and the view drew a status bar row)
    let has_status_bar = if h >= 2
        && let Some(last_row) = term.line_cells(h - 1)
    {
        last_row.first().is_some_and(|c| {
            c.attrs.contains(CellAttrs::REVERSE)
                || matches!(c.bg, Some(crate::headless::Color::Blue))
        })
    } else {
        false
    };
    let content_end = if has_status_bar { h - 1 } else { h };

    if has_status_bar && let Some(status_row) = term.line_cells_mut(h - 1) {
        for cell in status_row.iter_mut() {
            cell.fg = palette.status_bar.fg;
            cell.bg = palette.status_bar.bg;
            cell.attrs = palette.status_bar.attrs;
        }
    }

    // 3. Content rows (`1 .. content_end`)
    for y in 1..content_end {
        let Some(row) = term.line_cells_mut(y) else {
            continue;
        };
        let rev_count = row
            .iter()
            .filter(|c| c.attrs.contains(CellAttrs::REVERSE))
            .count();
        let is_cursor_row = rev_count > row.len() / 3;

        for cell in row.iter_mut() {
            if is_cursor_row && cell.attrs.contains(CellAttrs::REVERSE) {
                if !palette.cursor_row.attrs.contains(CellAttrs::REVERSE) {
                    cell.attrs.remove(CellAttrs::REVERSE);
                }
                cell.attrs |= palette.cursor_row.attrs;
                cell.bg = palette.cursor_row.bg;
                cell.fg = match cell.fg {
                    None
                    | Some(
                        crate::headless::Color::White
                        | crate::headless::Color::BrightWhite
                        | crate::headless::Color::Black,
                    ) => palette.cursor_row.fg,
                    Some(fg) => map_semantic_fg(fg, &palette)
                        .map(|c| palette.resolve_row_fg(c, true))
                        .or(palette.cursor_row.fg),
                };
            } else {
                if cell.bg.is_none() && palette.canvas_bg.is_some() {
                    cell.bg = palette.canvas_bg;
                }
                if let Some(fg) = cell.fg {
                    cell.fg = map_semantic_fg(fg, &palette);
                } else if cell.attrs.contains(CellAttrs::DIM) {
                    cell.fg = palette.muted_style.fg.or(palette.text_fg);
                } else if palette.text_fg.is_some() {
                    cell.fg = palette.text_fg;
                }
            }
        }
    }
}

static DEFAULT_VIEW_OPTIONS: std::sync::LazyLock<crate::options::ViewOptions> =
    std::sync::LazyLock::new(crate::options::ViewOptions::default);

/// Appends the global prompt or status message over the bottom terminal rows into `spool`,
/// returning `(first_touched_row_0based, optional_cursor_col_1based)`.
pub(crate) fn render_overlay_line_to_spool(
    app: &AppState,
    spool: &mut Vec<u8>,
    width: u16,
    height: u16,
) -> Option<(usize, Option<usize>)> {
    if height == 0 || width == 0 {
        return None;
    }
    let bottom_bar_sgr = if app.options.ui_theme == tigrs_core::config::UiThemeId::Default
        && app.options.colors.is_empty()
    {
        "\x1b[7m".to_string()
    } else {
        let palette = app.options.ui_palette();
        crate::ui_theme::UiPalette::sgr_for_style(palette.status_bar)
    };

    if let Some(ref prompt) = app.prompt {
        let mut first_row_1based = height as usize;
        if let crate::prompt::PromptKind::OptionMenu(ref menu) = prompt.kind
            && height >= 8
            && width >= 30
        {
            let max_panel_rows = ((height as usize) * 3 / 5).clamp(6, 18);
            let panel_lines = menu.render_panel_lines(
                &app.options,
                &app.saved_options_baseline,
                &DEFAULT_VIEW_OPTIONS,
                app.config_file_path.as_deref(),
                width as usize,
                max_panel_rows,
            );
            let start_row = (height as usize).saturating_sub(panel_lines.len()) + 1;
            for (i, line) in panel_lines.iter().enumerate() {
                let row_1based = start_row + i;
                if row_1based >= 1 && row_1based <= height as usize {
                    first_row_1based = first_row_1based.min(row_1based);
                    let _ = write!(spool, "\x1b[{row_1based};1H{line}");
                }
            }
            return Some((first_row_1based.saturating_sub(1), None));
        }
        let (prompt_text, cursor_x) = prompt.render_line(width as usize);
        let cursor_col_1based = cursor_x.min(width as usize) + 1;
        let _ = write!(
            spool,
            "\x1b[{height};1H{bottom_bar_sgr}{prompt_text}\x1b[0m\x1b[{height};{cursor_col_1based}H"
        );
        Some((first_row_1based.saturating_sub(1), Some(cursor_col_1based)))
    } else if let Some(ref msg) = app.status_message {
        const SPACES: &[u8; 64] =
            b"                                                                ";
        let clean = tigrs_core::ansi::strip_control_chars(msg);
        let msg_w = tigrs_core::ansi::visible_width(&clean);
        let mut pad_len = (width as usize).saturating_sub(msg_w);
        let _ = write!(spool, "\x1b[{height};1H{bottom_bar_sgr}{clean}");
        while pad_len > 0 {
            let chunk = pad_len.min(SPACES.len());
            spool.extend_from_slice(&SPACES[..chunk]);
            pad_len -= chunk;
        }
        spool.extend_from_slice(b"\x1b[0m");
        Some(((height as usize).saturating_sub(1), None))
    } else {
        None
    }
}

/// Composites any active Options Panel drawer, command/search prompt, or status message
/// directly into `renderer.front_grid` prior to `present_frame_to_spool` so that
/// `ScreenGrid` damage tracking diffs overlay rows cell-by-cell.
fn composite_overlay_into_front_grid(
    app: &AppState,
    renderer: &mut TerminalRenderer,
    width: u16,
    height: u16,
) -> Option<usize> {
    renderer.overlay_spool.clear();
    let (first_row_0based, cursor_col_1based) =
        render_overlay_line_to_spool(app, &mut renderer.overlay_spool, width, height)?;

    let overlay_h = (height as usize).saturating_sub(first_row_0based);
    if overlay_h > 0 && !renderer.overlay_spool.is_empty() {
        renderer.scratch_overlay.resize_and_clear(width, height);
        let _ = renderer.scratch_overlay.write_all(&renderer.overlay_spool);
        for row_y in first_row_0based..(height as usize) {
            if let (Some(src_row), Some(dst_row)) = (
                renderer.scratch_overlay.line_cells(row_y),
                renderer.front_grid.row_mut(row_y),
            ) {
                let len = dst_row.len().min(src_row.len());
                dst_row[..len].copy_from_slice(&src_row[..len]);
            }
        }
    }

    cursor_col_1based
}

/// Draws the global prompt or status message over the bottom terminal row.
pub(crate) fn render_overlay_line<W: Write>(
    app: &AppState,
    writer: &mut W,
    width: u16,
    height: u16,
) -> Result<()> {
    let mut spool = Vec::with_capacity(256);
    let _ = render_overlay_line_to_spool(app, &mut spool, width, height);
    if !spool.is_empty() {
        writer.write_all(&spool)?;
    }
    Ok(())
}

/// Renders both panes of a split layout into the rectangles described by `primary`
/// and `secondary`, compositing them into a single frame and emitting 1 write call.
#[allow(clippy::needless_range_loop, clippy::too_many_arguments)]
pub(crate) fn render_split_direct<W: Write>(
    app: &AppState,
    renderer: &mut TerminalRenderer,
    writer: &mut W,
    width: u16,
    height: u16,
    primary: &PaneLayout,
    secondary: &PaneLayout,
    vertical: bool,
) -> Result<()> {
    let active = app.active_view();

    renderer
        .scratch_primary
        .resize_and_clear(primary.width, primary.height);
    renderer
        .scratch_secondary
        .resize_and_clear(secondary.width, secondary.height);

    render_view_kind(
        app,
        primary.kind,
        &mut renderer.scratch_primary,
        primary.width,
        primary.height,
    )?;
    render_view_kind(
        app,
        secondary.kind,
        &mut renderer.scratch_secondary,
        secondary.width,
        secondary.height,
    )?;

    let primary_active = active == Some(primary.kind);
    let secondary_active = active == Some(secondary.kind);
    if !primary_active {
        demote_pane_title(&mut renderer.scratch_primary);
    }
    if !secondary_active {
        demote_pane_title(&mut renderer.scratch_secondary);
    }
    apply_ui_palette_to_terminal(&mut renderer.scratch_primary, &app.options, primary_active);
    apply_ui_palette_to_terminal(
        &mut renderer.scratch_secondary,
        &app.options,
        secondary_active,
    );

    let w = width as usize;
    let h = height as usize;
    renderer.front_grid.resize_and_clear(w, h);

    if vertical {
        let sep_char = match app.options.line_graphics {
            crate::options::LineGraphics::Ascii => '|',
            crate::options::LineGraphics::Utf8 => '│',
        };
        let palette = app.options.ui_palette();
        let sep_cell = crate::headless::Cell {
            ch: sep_char,
            fg: Some(palette.split_separator_fg),
            bg: palette.canvas_bg,
            attrs: CellAttrs::empty(),
        };

        let left_w = primary.width as usize;
        let right_w = secondary.width as usize;
        for y in 0..h {
            if let Some(row) = renderer.front_grid.row_mut(y) {
                for x in 0..left_w {
                    if let Some(c) = renderer.scratch_primary.cell(x, y) {
                        row[x] = *c;
                    }
                }
                if left_w < row.len() {
                    row[left_w] = sep_cell;
                }
                for x in 0..right_w {
                    let target_x = left_w + 1 + x;
                    if target_x < row.len()
                        && let Some(c) = renderer.scratch_secondary.cell(x, y)
                    {
                        row[target_x] = *c;
                    }
                }
            }
        }
    } else {
        for y in 0..primary.height as usize {
            if let Some(row) = renderer.front_grid.row_mut(y) {
                for x in 0..w {
                    if let Some(c) = renderer.scratch_primary.cell(x, y) {
                        row[x] = *c;
                    }
                }
            }
        }
        let offset = primary.height as usize;
        for y in 0..secondary.height as usize {
            if let Some(row) = renderer.front_grid.row_mut(offset + y) {
                for x in 0..w {
                    if let Some(c) = renderer.scratch_secondary.cell(x, y) {
                        row[x] = *c;
                    }
                }
            }
        }
    }

    let cursor_col = composite_overlay_into_front_grid(app, renderer, width, height);
    flush_rendered_frame(app, renderer, writer, height, cursor_col)
}

fn flush_rendered_frame<W: Write>(
    app: &AppState,
    renderer: &mut TerminalRenderer,
    writer: &mut W,
    height: u16,
    cursor_col: Option<usize>,
) -> Result<()> {
    let sync = app.caps.supports_synchronized_output;
    renderer.frame_spool.clear();
    if sync {
        renderer.frame_spool.extend_from_slice(b"\x1b[?2026h");
    }
    let prefix_len = renderer.frame_spool.len();
    present_frame_to_spool(renderer, app.caps.color_profile);
    if let Some(col_1based) = cursor_col {
        let _ = write!(renderer.frame_spool, "\x1b[{height};{col_1based}H");
        if !renderer.cursor_visible {
            renderer.frame_spool.extend_from_slice(b"\x1b[?25h");
            renderer.cursor_visible = true;
        }
    } else if renderer.cursor_visible {
        renderer.frame_spool.extend_from_slice(b"\x1b[?25l");
        renderer.cursor_visible = false;
    }
    if renderer.frame_spool.len() > prefix_len {
        if sync {
            renderer.frame_spool.extend_from_slice(b"\x1b[?2026l");
        }
        writer.write_all(&renderer.frame_spool)?;
    } else {
        renderer.frame_spool.clear();
    }
    Ok(())
}

pub(crate) fn render_active_direct<W: Write>(
    app: &AppState,
    writer: &mut W,
    width: u16,
    height: u16,
) -> Result<()> {
    let mut renderer_guard = app.renderer.borrow_mut();
    let renderer = &mut *renderer_guard;
    if app
        .screen_dirty
        .swap(false, std::sync::atomic::Ordering::Relaxed)
    {
        renderer.invalidate();
    }

    match app.view_layout(width, height) {
        Some(ViewLayout::Split {
            primary,
            secondary,
            vertical,
        }) => render_split_direct(
            app, renderer, writer, width, height, &primary, &secondary, vertical,
        ),
        Some(ViewLayout::Single(pane)) => {
            renderer
                .scratch_primary
                .resize_and_clear(pane.width, pane.height);
            render_view_kind(
                app,
                pane.kind,
                &mut renderer.scratch_primary,
                pane.width,
                pane.height,
            )?;
            apply_ui_palette_to_terminal(&mut renderer.scratch_primary, &app.options, true);
            renderer
                .front_grid
                .copy_from_headless(&renderer.scratch_primary);
            let cursor_col = composite_overlay_into_front_grid(app, renderer, width, height);
            flush_rendered_frame(app, renderer, writer, height, cursor_col)
        }
        None => {
            renderer.invalidate();
            render_overlay_line(app, writer, width, height)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::{Cell, Color};

    #[test]
    fn test_write_row_to() {
        let mut term = crate::headless::HeadlessTerminal::new(20, 3);
        write!(term, "\x1b[2;3H\x1b[1;32mHello Direct\x1b[K").unwrap();
        assert_eq!(term.line_text(1), "  Hello Direct");
        assert_eq!(term.cell(2, 1).unwrap().fg, Some(Color::Green));
        assert!(term.cell(2, 1).unwrap().attrs.contains(CellAttrs::BOLD));

        let mut out = Vec::new();
        write_row_to(term.line_cells(1).unwrap(), 2, &mut out).unwrap();
        assert!(!out.is_empty());
    }

    #[test]
    fn test_screen_grid_basic_operations() {
        let mut grid = ScreenGrid::new(10, 5);
        assert_eq!(grid.width(), 10);
        assert_eq!(grid.height(), 5);

        // Default cells
        assert_eq!(grid.cell(0, 0).unwrap().ch, ' ');
        assert!(grid.cell(0, 0).unwrap().attrs.is_empty());

        // Mutation via cell_mut
        if let Some(c) = grid.cell_mut(3, 2) {
            c.ch = 'Z';
            c.attrs |= CellAttrs::BOLD;
            c.fg = Some(Color::Cyan);
        }
        assert_eq!(grid.cell(3, 2).unwrap().ch, 'Z');
        assert!(grid.cell(3, 2).unwrap().attrs.contains(CellAttrs::BOLD));
        assert_eq!(grid.cell(3, 2).unwrap().fg, Some(Color::Cyan));

        // Row slicing
        let row2 = grid.row(2).unwrap();
        assert_eq!(row2.len(), 10);
        assert_eq!(row2[3].ch, 'Z');

        // Out of bounds
        assert!(grid.cell(10, 0).is_none());
        assert!(grid.cell(0, 5).is_none());
        assert!(grid.row(5).is_none());

        // from_rows & into_vec round-trip
        let nested = vec![
            vec![
                Cell {
                    ch: 'a',
                    ..Default::default()
                },
                Cell {
                    ch: 'b',
                    ..Default::default()
                },
            ],
            vec![
                Cell {
                    ch: 'c',
                    ..Default::default()
                },
                Cell {
                    ch: 'd',
                    ..Default::default()
                },
            ],
        ];
        let from = ScreenGrid::from_rows(&nested);
        assert_eq!(from.width(), 2);
        assert_eq!(from.height(), 2);
        assert_eq!(from.into_vec(), nested);
    }

    #[test]
    fn test_pen_state_differential_transitions() {
        let mut pen = PenState::default();

        // 1. Default -> Bold
        let mut buf = Vec::new();
        let target_bold = PenState {
            attrs: CellAttrs::BOLD,
            ..Default::default()
        };
        pen.transition_to(&target_bold, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "\x1b[1m");
        assert_eq!(pen, target_bold);

        // 2. Bold -> Bold (identical: 0 bytes)
        let mut buf = Vec::new();
        pen.transition_to(&target_bold, &mut buf).unwrap();
        assert!(buf.is_empty());

        // 3. Bold -> Bold + Fg(Red) (differential: emit only fg, no reset)
        let mut buf = Vec::new();
        let target_bold_red = PenState {
            attrs: CellAttrs::BOLD,
            fg: Some(Color::Red),
            ..Default::default()
        };
        pen.transition_to(&target_bold_red, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "\x1b[31m");
        assert_eq!(pen, target_bold_red);

        // 4. Bold + Fg(Red) -> Bold + Fg(Green) (differential color change, no reset)
        let mut buf = Vec::new();
        let target_bold_green = PenState {
            attrs: CellAttrs::BOLD,
            fg: Some(Color::Green),
            ..Default::default()
        };
        pen.transition_to(&target_bold_green, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "\x1b[32m");
        assert_eq!(pen, target_bold_green);

        // 5. Bold + Fg(Green) -> Default (turning off bold/fg: emit reset \x1b[0m)
        let mut buf = Vec::new();
        let target_default = PenState::default();
        pen.transition_to(&target_default, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "\x1b[0m");
        assert_eq!(pen, target_default);

        // 6. Default -> Complex style (batched parameters in single sequence)
        let mut buf = Vec::new();
        let target_complex = PenState {
            attrs: CellAttrs::BOLD | CellAttrs::ITALIC,
            fg: Some(Color::Ansi256(42)),
            bg: Some(Color::Rgb(10, 20, 30)),
        };
        pen.transition_to(&target_complex, &mut buf).unwrap();
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "\x1b[1;3;38;5;42;48;2;10;20;30m"
        );
        assert_eq!(pen, target_complex);
    }

    #[test]
    fn test_write_screen_grid_to_damage_tracking() {
        let mut grid1 = ScreenGrid::new(4, 2);
        grid1.cell_mut(0, 0).unwrap().ch = 'H';
        grid1.cell_mut(1, 0).unwrap().ch = 'i';
        grid1.cell_mut(0, 1).unwrap().ch = 'O';
        grid1.cell_mut(1, 1).unwrap().ch = 'k';

        // Frame 1: Full paint
        let mut buf1 = Vec::new();
        write_screen_grid_to(&grid1, None, &mut buf1).unwrap();
        let text1 = String::from_utf8(buf1).unwrap();
        assert!(text1.contains("\x1b[1;1H"));
        assert!(text1.contains("\x1b[2;1H"));
        assert!(text1.contains("Hi"));
        assert!(text1.contains("Ok"));

        // Frame 2: Identical grid -> 0 bytes emitted
        let mut buf2 = Vec::new();
        write_screen_grid_to(&grid1, Some(&grid1), &mut buf2).unwrap();
        assert!(buf2.is_empty(), "identical frame must emit 0 bytes");

        // Frame 3: Only row 2 modified -> row 1 skipped
        let mut grid2 = grid1.clone();
        grid2.cell_mut(0, 1).unwrap().ch = 'N';
        grid2.cell_mut(1, 1).unwrap().ch = 'o';

        let mut buf3 = Vec::new();
        write_screen_grid_to(&grid2, Some(&grid1), &mut buf3).unwrap();
        let text3 = String::from_utf8(buf3).unwrap();
        assert!(!text3.contains("\x1b[1;1H"), "row 1 must not be repainted");
        assert!(text3.contains("\x1b[2;1H"), "row 2 must be repainted");
        assert!(text3.contains("No"));
    }

    #[test]
    fn test_screen_grid_downsample_per_profile() {
        let make = || {
            let mut g = ScreenGrid::new(2, 1);
            if let Some(c) = g.cell_mut(0, 0) {
                c.fg = Some(Color::Rgb(0xd7, 0x00, 0x00));
                c.bg = Some(Color::Ansi256(240));
                c.attrs |= CellAttrs::BOLD;
            }
            g
        };

        // TrueColor leaves the grid untouched.
        let mut truecolor = make();
        truecolor.downsample(ColorProfile::TrueColor);
        assert_eq!(truecolor, make());

        // Ansi256 folds 24-bit RGB into the 256-color cube.
        let mut indexed = make();
        indexed.downsample(ColorProfile::Ansi256);
        let cell = indexed.cell(0, 0).unwrap();
        assert!(matches!(cell.fg, Some(Color::Ansi256(_))));
        assert_eq!(cell.bg, Some(Color::Ansi256(240)));

        // Ansi16 folds both RGB and 256-color into the base palette.
        let mut basic = make();
        basic.downsample(ColorProfile::Ansi16);
        let cell = basic.cell(0, 0).unwrap();
        assert!(!matches!(cell.fg, Some(Color::Rgb(..) | Color::Ansi256(_))));
        assert!(!matches!(cell.bg, Some(Color::Rgb(..) | Color::Ansi256(_))));

        // Monochrome drops color but preserves text attributes.
        let mut mono = make();
        mono.downsample(ColorProfile::Monochrome);
        let cell = mono.cell(0, 0).unwrap();
        assert_eq!(cell.fg, None);
        assert_eq!(cell.bg, None);
        assert!(cell.attrs.contains(CellAttrs::BOLD));
    }

    #[test]
    fn test_write_row_to_spool_empty_row_fast_path_and_headless_parity() {
        let mut grid1 = ScreenGrid::new(20, 2);
        for (i, ch) in "Hello colored row".chars().enumerate() {
            let c = grid1.cell_mut(i, 0).unwrap();
            c.ch = ch;
            c.fg = Some(Color::Green);
            c.attrs = CellAttrs::BOLD;
        }
        for (i, ch) in "Second line text".chars().enumerate() {
            grid1.cell_mut(i, 1).unwrap().ch = ch;
        }

        let mut term = crate::headless::HeadlessTerminal::new(20, 2);
        let mut buf1 = Vec::new();
        write_screen_grid_to(&grid1, None, &mut buf1).unwrap();
        term.write_all(&buf1).unwrap();
        assert!(term.line_text(0).starts_with("Hello colored row"));
        assert!(term.line_text(1).starts_with("Second line text"));

        // 1. Transitioning row 0 to all default spaces must emit the fast-path
        //    `\x1b[1;1H\x1b[0m\x1b[K` sequence and clear row 0 in `HeadlessTerminal`.
        let mut grid2 = grid1.clone();
        for x in 0..20 {
            *grid2.cell_mut(x, 0).unwrap() = Cell::default();
        }
        let mut buf2 = Vec::new();
        write_screen_grid_to(&grid2, Some(&grid1), &mut buf2).unwrap();
        let text2 = String::from_utf8(buf2.clone()).unwrap();
        assert_eq!(
            text2, "\x1b[1;1H\x1b[0m\x1b[K",
            "Clearing a non-empty row to default spaces must use the fast-path EL sequence"
        );
        term.write_all(&buf2).unwrap();
        assert_eq!(term.line_text(0).trim(), "");
        assert!(term.line_text(1).starts_with("Second line text"));

        // 2. Modifying a single middle cell in row 1 repaints from column 1 and produces
        //    exact cell-for-cell parity with a fresh full-screen render.
        let mut grid3 = grid2.clone();
        let mid = grid3.cell_mut(7, 1).unwrap();
        mid.ch = 'X';
        mid.fg = Some(Color::Yellow);

        let mut buf3 = Vec::new();
        write_screen_grid_to(&grid3, Some(&grid2), &mut buf3).unwrap();
        term.write_all(&buf3).unwrap();

        let mut fresh_term = crate::headless::HeadlessTerminal::new(20, 2);
        let mut fresh_buf = Vec::new();
        write_screen_grid_to(&grid3, None, &mut fresh_buf).unwrap();
        fresh_term.write_all(&fresh_buf).unwrap();

        for y in 0..2_usize {
            for x in 0..20_usize {
                assert_eq!(
                    term.cell(x, y),
                    fresh_term.cell(x, y),
                    "Incremental damage render diverged from full render at ({x}, {y})"
                );
            }
        }
    }
}
