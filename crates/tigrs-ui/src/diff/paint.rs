// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Rendering and painting logic for unified and side-by-side diff views.
//!
//! Handles terminal column geometry, two-pane layout, line number gutters,
//! sign stripping, intra-line word emphasis, and horizontal scrolling.

use super::document::{DiffDocument, DiffLineType, LineMarker, RowCell, RowPair};
use super::theme::{Attrs, DiffStyle, DiffTheme};
use crate::headless::Color;
use crate::options::{DiffLayout, LineGraphics, ViewOptions};
use crate::term_cap::ColorProfile;
use std::io::Write;

/// Emits ANSI SGR escape codes for a `DiffStyle`.
pub fn write_style<W: Write>(w: &mut W, style: &DiffStyle, is_cursor: bool) -> std::io::Result<()> {
    if is_cursor {
        write!(w, "\x1b[7m")?;
        return Ok(());
    }

    write!(w, "\x1b[0m")?;

    if let Some(fg) = style.fg {
        write_color(w, fg, false)?;
    }
    if let Some(bg) = style.bg {
        write_color(w, bg, true)?;
    }

    style.attrs.write_sgr(w)?;
    Ok(())
}

/// Emits color escape codes for foreground or background.
fn write_color<W: Write>(w: &mut W, color: Color, is_bg: bool) -> std::io::Result<()> {
    let offset = if is_bg { 10 } else { 0 };
    match color {
        Color::Black => write!(w, "\x1b[{}m", 30 + offset),
        Color::Red => write!(w, "\x1b[{}m", 31 + offset),
        Color::Green => write!(w, "\x1b[{}m", 32 + offset),
        Color::Yellow => write!(w, "\x1b[{}m", 33 + offset),
        Color::Blue => write!(w, "\x1b[{}m", 34 + offset),
        Color::Magenta => write!(w, "\x1b[{}m", 35 + offset),
        Color::Cyan => write!(w, "\x1b[{}m", 36 + offset),
        Color::White => write!(w, "\x1b[{}m", 37 + offset),
        Color::BrightBlack => write!(w, "\x1b[{}m", 90 + offset),
        Color::BrightRed => write!(w, "\x1b[{}m", 91 + offset),
        Color::BrightGreen => write!(w, "\x1b[{}m", 92 + offset),
        Color::BrightYellow => write!(w, "\x1b[{}m", 93 + offset),
        Color::BrightBlue => write!(w, "\x1b[{}m", 94 + offset),
        Color::BrightMagenta => write!(w, "\x1b[{}m", 95 + offset),
        Color::BrightCyan => write!(w, "\x1b[{}m", 96 + offset),
        Color::BrightWhite => write!(w, "\x1b[{}m", 97 + offset),
        Color::Ansi256(n) => write!(w, "\x1b[{};5;{}m", if is_bg { 48 } else { 38 }, n),
        Color::Rgb(r, g, b) => {
            write!(
                w,
                "\x1b[{};2;{};{};{}m",
                if is_bg { 48 } else { 38 },
                r,
                g,
                b
            )
        }
    }
}

/// Resolves the base `DiffStyle` for a given row type from the theme.
#[must_use]
pub fn style_for_type(theme: &DiffTheme, row_type: DiffLineType) -> DiffStyle {
    match row_type {
        DiffLineType::CommitHeader => theme.commit,
        DiffLineType::MergeHeader => theme.merge,
        DiffLineType::AuthorHeader => theme.author,
        DiffLineType::CommitterHeader => theme.committer,
        DiffLineType::DateHeader => theme.date,
        DiffLineType::MessageTitle => theme.message_title,
        DiffLineType::MessageBody => theme.message_body,
        DiffLineType::CommitTrailer => theme.trailer,
        DiffLineType::StatFile => theme.stat_file,
        DiffLineType::StatSummary => theme.stat_summary,
        DiffLineType::FileHeader
        | DiffLineType::FileMeta
        | DiffLineType::ModeChange
        | DiffLineType::StatusHint
        | DiffLineType::DiffHeader => theme.header,
        DiffLineType::FileOld | DiffLineType::BinaryNote => theme.file_old,
        DiffLineType::FileNew => theme.file_new,
        DiffLineType::HunkHeader => theme.hunk_header,
        DiffLineType::DiffContext => theme.context,
        DiffLineType::DiffAdd => theme.add,
        DiffLineType::DiffDel => theme.del,
        DiffLineType::Delimiter => theme.delimiter,
        DiffLineType::Empty => DiffStyle::default(),
    }
}

/// Viewport geometry and coordinates for painting a diff document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffViewport {
    /// Screen width in columns.
    pub width: u16,
    /// Screen height in rows.
    pub height: u16,
    /// Vertical scroll offset.
    pub scroll_offset: usize,
    /// Horizontal scroll offset.
    pub col_offset: usize,
    /// Cursor row index.
    pub cursor: usize,
}

struct UnifiedRowParams<'a> {
    row: &'a RowPair,
    theme: &'a DiffTheme,
    options: &'a ViewOptions,
    width: usize,
    col_offset: usize,
    is_cursor: bool,
    lineno_digits: usize,
    wrap_chunk: Option<(usize, usize, &'a [char], usize)>,
}

struct CellContentParams<'a> {
    base_style: &'a DiffStyle,
    emph_style: &'a DiffStyle,
    target_width: usize,
    col_offset: usize,
    is_cursor: bool,
    profile: ColorProfile,
    wrap_slice: Option<(usize, &'a [char], usize)>,
}

/// Splits `text` into display-width-bounded slices `(char_offset, chars, display_width)` for soft line wrapping.
fn split_wrap_slices(text: &str, max_width: usize) -> Vec<(usize, Vec<char>, usize)> {
    if text.is_empty() || max_width == 0 {
        return vec![(0, Vec::new(), 0)];
    }
    let mut slices = Vec::new();
    let mut cur_chars = Vec::new();
    let mut cur_width = 0usize;
    let mut chunk_start_idx = 0usize;

    for (char_idx, ch) in text.chars().enumerate() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_width + cw > max_width && !cur_chars.is_empty() {
            slices.push((chunk_start_idx, std::mem::take(&mut cur_chars), cur_width));
            chunk_start_idx = char_idx;
            cur_width = 0;
        }
        cur_chars.push(ch);
        cur_width += cw;
    }
    if !cur_chars.is_empty() || slices.is_empty() {
        slices.push((chunk_start_idx, cur_chars, cur_width));
    }
    slices
}

/// Counts the number of display-width-bounded soft-wrapped lines for `text` without heap allocation.
fn count_wrap_slices(text: &str, max_width: usize) -> usize {
    if text.is_empty() || max_width == 0 {
        return 1;
    }
    let mut count = 1usize;
    let mut cur_width = 0usize;
    let mut has_chars_in_chunk = false;

    for ch in text.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_width + cw > max_width && has_chars_in_chunk {
            count += 1;
            cur_width = 0;
        }
        has_chars_in_chunk = true;
        cur_width += cw;
    }
    count
}

fn wrapped_row_height(
    row: &RowPair,
    is_side_by_side: bool,
    theme: &DiffTheme,
    options: &ViewOptions,
    w: usize,
    lineno_digits: usize,
) -> usize {
    if !is_content_row(row.row_type) {
        return 1;
    }
    if is_side_by_side {
        let num_w = lineno_digits.max(4);
        let gutter_w = if options.line_number { num_w + 1 } else { 0 };
        let sep_w = 1;
        let avail = w.saturating_sub(gutter_w * 2 + sep_w);
        let left_w = avail / 2;
        let right_w = avail.saturating_sub(left_w);
        let sign_w = usize::from(
            theme.profile == ColorProfile::Monochrome
                || options.diff_indicator == crate::options::DiffIndicator::Yes,
        );
        let left_content_w = left_w.saturating_sub(sign_w).max(1);
        let right_content_w = right_w.saturating_sub(sign_w).max(1);
        let left_len = row
            .left
            .as_ref()
            .map_or(0, |c| count_wrap_slices(&c.text, left_content_w));
        let right_len = row
            .right
            .as_ref()
            .map_or(0, |c| count_wrap_slices(&c.text, right_content_w));
        left_len.max(right_len).max(1)
    } else {
        let cell_opt = row.left.as_ref().or(row.right.as_ref());
        let mut gutter_cols = 0usize;
        if options.line_number
            && let Some(num) = cell_opt.and_then(|c| c.lineno).map(|n| n as usize)
        {
            let num_w = lineno_digits
                .max(num.checked_ilog10().map_or(1, |d| d as usize + 1))
                .max(4);
            gutter_cols += num_w + 1;
        }
        if !theme.strip_signs && cell_opt.and_then(|c| c.marker.as_char()).is_some() {
            gutter_cols += 1;
        }
        let code_w = w.saturating_sub(gutter_cols).max(1);
        cell_opt
            .map_or(1, |c| count_wrap_slices(&c.text, code_w))
            .max(1)
    }
}

/// Paints the visible viewport of a `DiffDocument`.
pub fn paint_diff<W: Write>(
    writer: &mut W,
    doc: &DiffDocument,
    theme: &DiffTheme,
    options: &ViewOptions,
    viewport: &DiffViewport,
) -> std::io::Result<()> {
    if viewport.width == 0 || viewport.height == 0 {
        return Ok(());
    }

    let w = viewport.width as usize;
    let visible_height = (viewport.height as usize).saturating_sub(2);
    let lineno_digits = doc.max_lineno_digits.max(4);

    let is_side_by_side = options.diff_layout == DiffLayout::SideBySide
        && w >= options.side_by_side_min_width as usize;
    let soft_wrap = options.wrap_lines && viewport.col_offset == 0;

    if soft_wrap {
        let mut screen_row = 0usize;
        let mut line_idx = viewport.scroll_offset;
        if viewport.cursor > line_idx && viewport.cursor < doc.len() && visible_height > 0 {
            let cursor_h = wrapped_row_height(
                &doc.rows[viewport.cursor],
                is_side_by_side,
                theme,
                options,
                w,
                lineno_digits,
            )
            .min(visible_height);
            let mut rows_before_cursor = 0usize;
            let mut min_start = viewport.cursor;
            for idx in (viewport.scroll_offset..viewport.cursor).rev() {
                let h = wrapped_row_height(
                    &doc.rows[idx],
                    is_side_by_side,
                    theme,
                    options,
                    w,
                    lineno_digits,
                );
                if rows_before_cursor + h + cursor_h > visible_height {
                    break;
                }
                rows_before_cursor += h;
                min_start = idx;
            }
            line_idx = line_idx.max(min_start);
        }

        while screen_row < visible_height {
            if line_idx >= doc.len() {
                let term_row = screen_row + 2;
                write!(writer, "\x1b[{term_row};1H\x1b[2K\x1b[0m~")?;
                screen_row += 1;
                continue;
            }

            let row = &doc.rows[line_idx];
            let is_cursor = line_idx == viewport.cursor;

            if is_side_by_side && is_content_row(row.row_type) {
                let num_w = lineno_digits.max(4);
                let gutter_w = if options.line_number { num_w + 1 } else { 0 };
                let sep_w = 1;
                let avail = w.saturating_sub(gutter_w * 2 + sep_w);
                let left_w = avail / 2;
                let right_w = avail.saturating_sub(left_w);
                let sign_w = usize::from(
                    theme.profile == ColorProfile::Monochrome
                        || options.diff_indicator == crate::options::DiffIndicator::Yes,
                );
                let left_content_w = left_w.saturating_sub(sign_w).max(1);
                let right_content_w = right_w.saturating_sub(sign_w).max(1);

                let left_slices = row
                    .left
                    .as_ref()
                    .map(|c| split_wrap_slices(&c.text, left_content_w))
                    .unwrap_or_default();
                let right_slices = row
                    .right
                    .as_ref()
                    .map(|c| split_wrap_slices(&c.text, right_content_w))
                    .unwrap_or_default();
                let total_chunks = left_slices.len().max(right_slices.len()).max(1);

                for chunk_idx in 0..total_chunks {
                    if screen_row >= visible_height {
                        break;
                    }
                    let term_row = screen_row + 2;
                    write!(writer, "\x1b[{term_row};1H\x1b[2K")?;
                    let left_slice = left_slices
                        .get(chunk_idx)
                        .map(|(off, chs, dw)| (*off, chs.as_slice(), *dw));
                    let right_slice = right_slices
                        .get(chunk_idx)
                        .map(|(off, chs, dw)| (*off, chs.as_slice(), *dw));
                    paint_side_by_side_row(
                        writer,
                        row,
                        theme,
                        options,
                        w,
                        0,
                        is_cursor,
                        lineno_digits,
                        Some((chunk_idx, left_slice, right_slice)),
                    )?;
                    screen_row += 1;
                }
            } else if is_content_row(row.row_type) {
                let cell_opt = row.left.as_ref().or(row.right.as_ref());
                let mut gutter_cols = 0usize;
                if options.line_number
                    && let Some(num) = cell_opt.and_then(|c| c.lineno).map(|n| n as usize)
                {
                    let num_w = lineno_digits
                        .max(num.checked_ilog10().map_or(1, |d| d as usize + 1))
                        .max(4);
                    gutter_cols += num_w + 1;
                }
                if !theme.strip_signs && cell_opt.and_then(|c| c.marker.as_char()).is_some() {
                    gutter_cols += 1;
                }
                let code_w = w.saturating_sub(gutter_cols).max(1);
                let slices = cell_opt.map_or_else(
                    || vec![(0, Vec::new(), 0)],
                    |c| split_wrap_slices(&c.text, code_w),
                );

                for (chunk_idx, (char_off, chs, dw)) in slices.iter().enumerate() {
                    if screen_row >= visible_height {
                        break;
                    }
                    let term_row = screen_row + 2;
                    write!(writer, "\x1b[{term_row};1H\x1b[2K")?;
                    let params = UnifiedRowParams {
                        row,
                        theme,
                        options,
                        width: w,
                        col_offset: 0,
                        is_cursor,
                        lineno_digits,
                        wrap_chunk: Some((chunk_idx, *char_off, chs.as_slice(), *dw)),
                    };
                    paint_unified_row(writer, &params)?;
                    screen_row += 1;
                }
            } else {
                let term_row = screen_row + 2;
                write!(writer, "\x1b[{term_row};1H\x1b[2K")?;
                let params = UnifiedRowParams {
                    row,
                    theme,
                    options,
                    width: w,
                    col_offset: 0,
                    is_cursor,
                    lineno_digits,
                    wrap_chunk: None,
                };
                paint_unified_row(writer, &params)?;
                screen_row += 1;
            }

            line_idx += 1;
        }
    } else {
        for row_idx in 0..visible_height {
            let term_row = row_idx + 2;
            let line_idx = viewport.scroll_offset + row_idx;

            write!(writer, "\x1b[{term_row};1H\x1b[2K")?;

            if line_idx < doc.len() {
                let row = &doc.rows[line_idx];
                let is_cursor = line_idx == viewport.cursor;

                if is_side_by_side && is_content_row(row.row_type) {
                    paint_side_by_side_row(
                        writer,
                        row,
                        theme,
                        options,
                        w,
                        viewport.col_offset,
                        is_cursor,
                        lineno_digits,
                        None,
                    )?;
                } else {
                    let params = UnifiedRowParams {
                        row,
                        theme,
                        options,
                        width: w,
                        col_offset: viewport.col_offset,
                        is_cursor,
                        lineno_digits,
                        wrap_chunk: None,
                    };
                    paint_unified_row(writer, &params)?;
                }
            } else {
                write!(writer, "\x1b[0m~")?;
            }
        }
    }

    write!(writer, "\x1b[0m")?;
    Ok(())
}

/// Returns true if `row_type` should be displayed as dual-pane in side-by-side mode.
fn is_content_row(row_type: DiffLineType) -> bool {
    matches!(
        row_type,
        DiffLineType::DiffContext | DiffLineType::DiffAdd | DiffLineType::DiffDel
    )
}

/// Returns the base row background tint for added/deleted code lines (`Gerrit` style).
fn sbs_diff_bg(marker: LineMarker, is_light: bool, profile: ColorProfile) -> Option<Color> {
    if profile == ColorProfile::Monochrome {
        return None;
    }
    match (marker, is_light) {
        (LineMarker::Add, false) => Some(Color::Rgb(20, 44, 28).downsample(profile)),
        (LineMarker::Del, false) => Some(Color::Rgb(54, 24, 30).downsample(profile)),
        (LineMarker::Add, true) => Some(Color::Rgb(218, 251, 225).downsample(profile)),
        (LineMarker::Del, true) => Some(Color::Rgb(255, 224, 229).downsample(profile)),
        _ => None,
    }
}

/// Returns the base row background tint for moved-from (violet/plum) and moved-to (teal/cyan) blocks.
fn moved_diff_bg(marker: LineMarker, is_light: bool, profile: ColorProfile) -> Option<Color> {
    if profile == ColorProfile::Monochrome {
        return None;
    }
    match (marker, is_light) {
        (LineMarker::Del, false) => Some(Color::Rgb(48, 28, 66).downsample(profile)),
        (LineMarker::Del, true) => Some(Color::Rgb(237, 224, 255).downsample(profile)),
        (LineMarker::Add, false) => Some(Color::Rgb(18, 46, 62).downsample(profile)),
        (LineMarker::Add, true) => Some(Color::Rgb(216, 243, 255).downsample(profile)),
        _ => None,
    }
}

/// Returns the stronger intra-line word emphasis background tint (`Gerrit` style).
fn sbs_emph_bg(marker: LineMarker, is_light: bool, profile: ColorProfile) -> Option<Color> {
    if profile == ColorProfile::Monochrome {
        return None;
    }
    match (marker, is_light) {
        (LineMarker::Add, false) => Some(Color::Rgb(45, 94, 64).downsample(profile)),
        (LineMarker::Del, false) => Some(Color::Rgb(110, 48, 59).downsample(profile)),
        (LineMarker::Add, true) => Some(Color::Rgb(166, 227, 161).downsample(profile)),
        (LineMarker::Del, true) => Some(Color::Rgb(245, 169, 184).downsample(profile)),
        _ => None,
    }
}

/// Returns the stronger intra-line word emphasis background tint for moved code blocks.
fn moved_emph_bg(marker: LineMarker, is_light: bool, profile: ColorProfile) -> Option<Color> {
    if profile == ColorProfile::Monochrome {
        return None;
    }
    match (marker, is_light) {
        (LineMarker::Del, false) => Some(Color::Rgb(92, 48, 128).downsample(profile)),
        (LineMarker::Del, true) => Some(Color::Rgb(210, 180, 250).downsample(profile)),
        (LineMarker::Add, false) => Some(Color::Rgb(32, 86, 118).downsample(profile)),
        (LineMarker::Add, true) => Some(Color::Rgb(166, 220, 250).downsample(profile)),
        _ => None,
    }
}

/// Returns the muted filler background color for the empty opposite half-row of an unpaired
/// insertion or deletion in side-by-side mode (`Gerrit` style shaded placeholder).
fn sbs_filler_bg(is_light: bool, profile: ColorProfile) -> Option<Color> {
    if profile == ColorProfile::Monochrome {
        return None;
    }
    if is_light {
        Some(Color::Rgb(235, 237, 242).downsample(profile))
    } else {
        Some(Color::Rgb(30, 32, 42).downsample(profile))
    }
}

/// Returns subtle diff background tint for added/deleted code lines when syntax highlighting is active.
///
/// Only applied in [`ColorProfile::TrueColor`], because the 6x6x6 256-color cube has no sub-`0x5f`
/// dark tint and downsampling `Rgb(54, 24, 30)` to `Ansi256` maps to `Ansi256(52)` (`\x1b[48;5;52m`),
/// which collides directly with `del_emphasis` (`Ansi256(52)`).
fn syntax_diff_bg(marker: LineMarker, profile: ColorProfile) -> Option<Color> {
    if profile != ColorProfile::TrueColor {
        return None;
    }
    sbs_diff_bg(marker, false, profile)
}

/// Renders visible characters with composite `SyntaxSpan` foreground tokens and word-diff emphasis backgrounds.
///
/// Uses monotonic index cursors (`emph_idx`, `syntax_idx`) to achieve $O(1)$ amortized
/// span lookup per character instead of $O(E + S)$ linear scans.
#[allow(clippy::too_many_arguments)]
fn paint_styled_chars<W: Write>(
    writer: &mut W,
    visible_chars: &[char],
    col_offset: usize,
    cell: &RowCell,
    base_style: &DiffStyle,
    emph_style: &DiffStyle,
    options: &ViewOptions,
    profile: ColorProfile,
) -> std::io::Result<()> {
    let has_syntax = options.syntax_highlighting
        && !cell.syntax_spans.is_empty()
        && profile != ColorProfile::Monochrome;
    let has_emph = (options.word_diff || options.diff_layout == DiffLayout::SideBySide)
        && !cell.emphasis.is_empty();

    let mut current_style = *base_style;
    let mut emph_idx = 0usize;
    let mut syntax_idx = 0usize;
    let mut run_buf = String::with_capacity(visible_chars.len());

    for (ch_idx, ch) in visible_chars.iter().enumerate() {
        let orig_idx = (ch_idx + col_offset) as u32;
        let is_emph = if has_emph {
            while emph_idx < cell.emphasis.len() && cell.emphasis[emph_idx].1 <= orig_idx {
                emph_idx += 1;
            }
            emph_idx < cell.emphasis.len() && cell.emphasis[emph_idx].0 <= orig_idx
        } else {
            false
        };

        let bg = if is_emph {
            emph_style
                .bg
                .or_else(|| {
                    if has_syntax || options.diff_layout == DiffLayout::SideBySide {
                        sbs_emph_bg(cell.marker, options.ui_theme.is_light(), profile)
                    } else {
                        None
                    }
                })
                .or(base_style.bg)
        } else {
            base_style.bg
        };

        let (fg, attrs) = if has_syntax {
            while syntax_idx < cell.syntax_spans.len()
                && cell.syntax_spans[syntax_idx].end <= orig_idx
            {
                syntax_idx += 1;
            }
            let active_span = if syntax_idx < cell.syntax_spans.len()
                && cell.syntax_spans[syntax_idx].start <= orig_idx
            {
                Some(&cell.syntax_spans[syntax_idx])
            } else {
                None
            };
            if let Some(span) = active_span {
                let mut attrs = if is_emph {
                    emph_style.attrs
                } else {
                    base_style.attrs
                };
                if span.bold || is_emph {
                    attrs |= Attrs::BOLD;
                }
                if span.italic {
                    attrs |= Attrs::ITALIC;
                }
                (Some(span.fg.downsample(profile)), attrs)
            } else if is_emph {
                (emph_style.fg.or(base_style.fg), emph_style.attrs)
            } else {
                (base_style.fg, base_style.attrs)
            }
        } else if is_emph {
            (emph_style.fg.or(base_style.fg), emph_style.attrs)
        } else {
            (base_style.fg, base_style.attrs)
        };

        let char_style = DiffStyle { fg, bg, attrs };
        if char_style != current_style {
            if !run_buf.is_empty() {
                writer.write_all(run_buf.as_bytes())?;
                run_buf.clear();
            }
            write_style(writer, &char_style, false)?;
            current_style = char_style;
        }
        run_buf.push(*ch);
    }

    if !run_buf.is_empty() {
        writer.write_all(run_buf.as_bytes())?;
    }

    if current_style != *base_style {
        write_style(writer, base_style, false)?;
    }

    Ok(())
}

/// Paints a single unified row into `writer`.
fn paint_unified_row<W: Write>(
    writer: &mut W,
    params: &UnifiedRowParams<'_>,
) -> std::io::Result<()> {
    let mut base_style = style_for_type(params.theme, params.row.row_type);

    let Some(cell) = params.row.left.as_ref().or(params.row.right.as_ref()) else {
        if params.is_cursor && params.width > 0 {
            write!(writer, "{:width$}", "", width = params.width)?;
        }
        return Ok(());
    };

    if !params.theme.custom_rules.is_empty() {
        let raw = cell.text.as_ref();
        let trimmed = raw.trim_start();
        for rule in &params.theme.custom_rules {
            let rule_prefix = rule.prefix.as_str();
            let rule_trimmed = rule_prefix.trim_start();
            if raw
                .get(..rule_prefix.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(rule_prefix))
                || trimmed
                    .get(..rule_trimmed.len())
                    .is_some_and(|s| s.eq_ignore_ascii_case(rule_trimmed))
            {
                base_style = rule.style;
                break;
            }
        }
    }

    let is_light = params.options.ui_theme.is_light();
    let is_moved_line = params.options.color_moved
        && cell.is_moved
        && params.theme.profile != ColorProfile::Monochrome;

    // Syntax-highlighted or moved diff lines get a distinct background tint
    let has_syntax = params.options.syntax_highlighting
        && !cell.syntax_spans.is_empty()
        && params.theme.profile != ColorProfile::Monochrome;
    if is_moved_line && !params.is_cursor {
        base_style.bg = moved_diff_bg(cell.marker, is_light, params.theme.profile);
    } else if has_syntax && !params.is_cursor && base_style.bg.is_none() {
        base_style.bg = syntax_diff_bg(cell.marker, params.theme.profile);
    }

    write_style(writer, &base_style, params.is_cursor)?;

    let mut used_cols = 0;
    let is_continuation = params
        .wrap_chunk
        .is_some_and(|(chunk_idx, _, _, _)| chunk_idx > 0);
    let cont_marker = if params.options.line_graphics == LineGraphics::Ascii {
        '\\'
    } else {
        '↪'
    };

    // 1. Line Number Gutter (only for source code diff lines that carry a file line number)
    if params.options.line_number
        && let Some(num) = cell.lineno.map(|n| n as usize)
    {
        if params.is_cursor {
            write_style(writer, &base_style, true)?;
        } else {
            let mut gutter_style = params.theme.gutter;
            if is_moved_line {
                gutter_style.bg = base_style.bg;
            }
            write_style(writer, &gutter_style, false)?;
        }
        let num_w = params
            .lineno_digits
            .max(num.checked_ilog10().map_or(1, |d| d as usize + 1))
            .max(4);
        if is_continuation {
            write!(writer, "{cont_marker:>num_w$} ")?;
        } else {
            write!(writer, "{num:>num_w$} ")?;
        }
        used_cols += num_w + 1;
        if !params.is_cursor {
            write_style(writer, &base_style, false)?;
        }
    }

    // 2. Marker (+/-/' ')
    if !params.theme.strip_signs
        && let Some(m) = cell.marker.as_char()
    {
        let m_disp = if is_continuation {
            if params.options.line_number {
                ' '
            } else {
                cont_marker
            }
        } else {
            m
        };
        write!(writer, "{m_disp}")?;
        used_cols += 1;
    } else if is_continuation && !params.options.line_number {
        write!(writer, "{cont_marker}")?;
        used_cols += 1;
    }

    // 3. Line Content with Horizontal Scrolling, Soft Wrapping, Syntax Highlighting & Word-diff Emphasis
    let raw_text = if cell.is_empty_line
        && (params.row.row_type == DiffLineType::DiffAdd
            || params.row.row_type == DiffLineType::DiffDel)
    {
        if cell.text.is_empty() {
            " "
        } else {
            &cell.text
        }
    } else if params.row.row_type == DiffLineType::FileHeader
        && params.options.diff_presentation == crate::options::DiffPresentation::Banner
        && params.options.diff_hints == tigrs_core::DiffHintsMode::Auto
        && params.width < 100
    {
        cell.text
            .strip_suffix("  za fold  e edit  i info  y copy")
            .unwrap_or(&cell.text)
    } else {
        &cell.text
    };

    let max_avail = params.width.saturating_sub(used_cols);
    let show_left_scroll = params.col_offset > 0
        && params.wrap_chunk.is_none()
        && is_content_row(params.row.row_type)
        && !raw_text.is_empty()
        && max_avail >= 2;
    if show_left_scroll {
        let scroll_char = if params.options.line_graphics == LineGraphics::Ascii {
            '<'
        } else {
            '‹'
        };
        let mut ind_style = base_style;
        ind_style.attrs |= Attrs::DIM;
        write_style(writer, &ind_style, params.is_cursor)?;
        write!(writer, "{scroll_char}")?;
        write_style(writer, &base_style, params.is_cursor)?;
        used_cols += 1;
    }

    let content_avail = params.width.saturating_sub(used_cols);
    let (char_offset, visible_chars_vec, display_w) =
        if let Some((_, char_off, chs, dw)) = params.wrap_chunk {
            (char_off, chs.to_vec(), dw)
        } else {
            let mut v = Vec::new();
            let mut dw = 0usize;
            for ch in raw_text.chars().skip(params.col_offset) {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if dw + cw > content_avail {
                    break;
                }
                v.push(ch);
                dw += cw;
            }
            (params.col_offset, v, dw)
        };

    let has_emph = params.options.word_diff && !cell.emphasis.is_empty();
    if (has_syntax || has_emph) && !params.is_cursor {
        let mut emph_style = if params.row.row_type == DiffLineType::DiffAdd {
            params.theme.add_emphasis
        } else {
            params.theme.del_emphasis
        };
        if is_moved_line {
            emph_style.bg = moved_emph_bg(cell.marker, is_light, params.theme.profile);
        }
        paint_styled_chars(
            writer,
            &visible_chars_vec,
            char_offset,
            cell,
            &base_style,
            &emph_style,
            params.options,
            params.theme.profile,
        )?;
    } else if params.row.row_type == DiffLineType::MessageTitle
        && params.options.commit_title_overflow.is_some()
        && !params.is_cursor
    {
        let limit = params.options.commit_title_overflow.unwrap_or(50);
        // The commit message title is formatted as "    {title}", with 4 spaces indent.
        let overflow_threshold = 4 + limit;
        let mut currently_overflow = false;
        for (ch_idx, ch) in visible_chars_vec.iter().enumerate() {
            let orig_idx = ch_idx + char_offset;
            let is_overflow = orig_idx >= overflow_threshold;
            if is_overflow != currently_overflow {
                if is_overflow {
                    write_style(writer, &params.theme.overflow, false)?;
                } else {
                    write_style(writer, &base_style, false)?;
                }
                currently_overflow = is_overflow;
            }
            write!(writer, "{ch}")?;
        }
        if currently_overflow {
            write_style(writer, &base_style, false)?;
        }
    } else {
        for &ch in &visible_chars_vec {
            write!(writer, "{ch}")?;
        }
    }
    used_cols += display_w;

    // 4. Trailing pad on cursor line, moved block line, or syntax-tinted diff line
    if (params.is_cursor || ((has_syntax || is_moved_line) && base_style.bg.is_some()))
        && params.width > used_cols
    {
        write!(writer, "{:pad$}", "", pad = params.width - used_cols)?;
    }

    write!(writer, "\x1b[0m")?;
    Ok(())
}

type SbsWrapSlice<'a> = (
    usize,
    Option<(usize, &'a [char], usize)>,
    Option<(usize, &'a [char], usize)>,
);

/// Paints a single dual-pane side-by-side row into `writer` using Gerrit-style row background
/// highlighting (no `+` or `-` signs unless `ColorProfile::Monochrome` or `diff-indicator = yes`).
#[allow(clippy::too_many_arguments)]
fn paint_side_by_side_row<W: Write>(
    writer: &mut W,
    row: &RowPair,
    theme: &DiffTheme,
    options: &ViewOptions,
    width: usize,
    col_offset: usize,
    is_cursor: bool,
    lineno_digits: usize,
    wrap_slices: Option<SbsWrapSlice<'_>>,
) -> std::io::Result<()> {
    let num_w = lineno_digits.max(4);
    let gutter_w = if options.line_number { num_w + 1 } else { 0 };
    let sep_w = 1;
    let avail = width.saturating_sub(gutter_w * 2 + sep_w);
    let left_w = avail / 2;
    let right_w = avail.saturating_sub(left_w);
    let is_light = options.ui_theme.is_light();
    let is_adaptive_default =
        options.ui_theme == tigrs_core::UiThemeId::Default && options.colors.is_empty();
    // In side-by-side mode, Gerrit-style row background highlighting replaces `+` and `-` signs
    // unless the terminal is monochrome or the user explicitly forced `diff-indicator = yes`.
    let show_sbs_signs = theme.profile == ColorProfile::Monochrome
        || options.diff_indicator == crate::options::DiffIndicator::Yes;
    let is_continuation = wrap_slices.is_some_and(|(chunk_idx, _, _)| chunk_idx > 0);
    let cont_marker = if options.line_graphics == LineGraphics::Ascii {
        '\\'
    } else {
        '↪'
    };

    // --- LEFT PANE (Old / Removed) ---
    let left_cell = row.left.as_ref();
    let left_is_moved = options.color_moved
        && left_cell.is_some_and(|c| c.is_moved)
        && theme.profile != ColorProfile::Monochrome;
    let mut left_style = match left_cell.map(|c| c.marker) {
        Some(LineMarker::Del) => {
            let mut s = theme.del;
            if theme.profile != ColorProfile::Monochrome && !is_cursor {
                if left_is_moved {
                    s.bg = moved_diff_bg(LineMarker::Del, is_light, theme.profile);
                } else if s.bg.is_none() {
                    s.bg = sbs_diff_bg(LineMarker::Del, is_light, theme.profile);
                }
                if is_adaptive_default {
                    s.fg = None;
                }
            }
            s
        }
        Some(LineMarker::Context) => theme.context,
        _ => {
            let mut s = theme.context;
            if theme.profile != ColorProfile::Monochrome && !is_cursor {
                s.bg = sbs_filler_bg(is_light, theme.profile);
            }
            s
        }
    };
    if let Some(cell) = left_cell
        && options.syntax_highlighting
        && !cell.syntax_spans.is_empty()
        && theme.profile != ColorProfile::Monochrome
        && !is_cursor
        && left_style.bg.is_none()
    {
        left_style.bg = sbs_diff_bg(cell.marker, is_light, theme.profile);
    }

    let mut left_emph_style = theme.del_emphasis;
    if theme.profile != ColorProfile::Monochrome {
        if left_is_moved {
            left_emph_style.bg = moved_emph_bg(LineMarker::Del, is_light, theme.profile);
        } else if is_adaptive_default || left_emph_style.bg.is_none() {
            left_emph_style.bg = sbs_emph_bg(LineMarker::Del, is_light, theme.profile);
        }
        if is_adaptive_default {
            left_emph_style.fg = None;
        }
    }

    // Left line-number gutter (inherits row background highlight on modified/filler rows)
    if options.line_number {
        let mut left_gutter_style = theme.gutter;
        if left_style.bg.is_some() {
            left_gutter_style.bg = left_style.bg;
        }
        if is_cursor {
            write_style(writer, &left_style, true)?;
        } else {
            write_style(writer, &left_gutter_style, false)?;
        }
        if is_continuation {
            if wrap_slices.is_some_and(|(_, l_s, _)| l_s.is_some()) {
                write!(writer, "{cont_marker:>num_w$} ")?;
            } else {
                write!(writer, "{:gutter_w$}", "")?;
            }
        } else if let Some(num) = left_cell.and_then(|c| c.lineno) {
            write!(writer, "{num:>num_w$} ")?;
        } else {
            write!(writer, "{:gutter_w$}", "")?;
        }
    }

    // Left text (with optional sign only in monochrome or explicit `diff-indicator = yes`)
    write_style(writer, &left_style, is_cursor)?;
    let mut left_used = 0;
    if show_sbs_signs {
        let m_char = if is_continuation {
            ' '
        } else {
            left_cell.and_then(|c| c.marker.as_char()).unwrap_or(' ')
        };
        write!(writer, "{m_char}")?;
        left_used += 1;
    }

    let left_content_w = left_w.saturating_sub(left_used);
    if let Some(cell) = left_cell {
        let left_wrap_slice = wrap_slices.and_then(|(_, l_s, _)| l_s);
        if is_continuation && left_wrap_slice.is_none() {
            if left_content_w > 0 {
                write!(writer, "{:pad$}", "", pad = left_content_w)?;
            }
        } else {
            let params = CellContentParams {
                base_style: &left_style,
                emph_style: &left_emph_style,
                target_width: left_content_w,
                col_offset,
                is_cursor,
                profile: theme.profile,
                wrap_slice: left_wrap_slice,
            };
            paint_cell_content(writer, cell, options, &params)?;
        }
    } else if left_content_w > 0 {
        write!(writer, "{:pad$}", "", pad = left_content_w)?;
    }

    // --- SEPARATOR ---
    let sep_char = if options.line_graphics == LineGraphics::Ascii {
        '|'
    } else {
        '│'
    };
    if !is_cursor {
        write_style(writer, &theme.delimiter, false)?;
    }
    write!(writer, "{sep_char}")?;

    // --- RIGHT PANE (New / Added) ---
    let right_cell = row.right.as_ref();
    let right_is_moved = options.color_moved
        && right_cell.is_some_and(|c| c.is_moved)
        && theme.profile != ColorProfile::Monochrome;
    let mut right_style = match right_cell.map(|c| c.marker) {
        Some(LineMarker::Add) => {
            let mut s = theme.add;
            if theme.profile != ColorProfile::Monochrome && !is_cursor {
                if right_is_moved {
                    s.bg = moved_diff_bg(LineMarker::Add, is_light, theme.profile);
                } else if s.bg.is_none() {
                    s.bg = sbs_diff_bg(LineMarker::Add, is_light, theme.profile);
                }
                if is_adaptive_default {
                    s.fg = None;
                }
            }
            s
        }
        Some(LineMarker::Context) => theme.context,
        _ => {
            let mut s = theme.context;
            if theme.profile != ColorProfile::Monochrome && !is_cursor {
                s.bg = sbs_filler_bg(is_light, theme.profile);
            }
            s
        }
    };
    if let Some(cell) = right_cell
        && options.syntax_highlighting
        && !cell.syntax_spans.is_empty()
        && theme.profile != ColorProfile::Monochrome
        && !is_cursor
        && right_style.bg.is_none()
    {
        right_style.bg = sbs_diff_bg(cell.marker, is_light, theme.profile);
    }

    let mut right_emph_style = theme.add_emphasis;
    if theme.profile != ColorProfile::Monochrome {
        if right_is_moved {
            right_emph_style.bg = moved_emph_bg(LineMarker::Add, is_light, theme.profile);
        } else if is_adaptive_default || right_emph_style.bg.is_none() {
            right_emph_style.bg = sbs_emph_bg(LineMarker::Add, is_light, theme.profile);
        }
        if is_adaptive_default {
            right_emph_style.fg = None;
        }
    }

    // Right line-number gutter (inherits row background highlight on modified/filler rows)
    if options.line_number {
        let mut right_gutter_style = theme.gutter;
        if right_style.bg.is_some() {
            right_gutter_style.bg = right_style.bg;
        }
        if is_cursor {
            write_style(writer, &right_style, true)?;
        } else {
            write_style(writer, &right_gutter_style, false)?;
        }
        if is_continuation {
            if wrap_slices.is_some_and(|(_, _, r_s)| r_s.is_some()) {
                write!(writer, "{cont_marker:>num_w$} ")?;
            } else {
                write!(writer, "{:gutter_w$}", "")?;
            }
        } else if let Some(num) = right_cell.and_then(|c| c.lineno) {
            write!(writer, "{num:>num_w$} ")?;
        } else {
            write!(writer, "{:gutter_w$}", "")?;
        }
    }

    // Right text (with optional sign only in monochrome or explicit `diff-indicator = yes`)
    write_style(writer, &right_style, is_cursor)?;
    let mut right_used = 0;
    if show_sbs_signs {
        let m_char = if is_continuation {
            ' '
        } else {
            right_cell.and_then(|c| c.marker.as_char()).unwrap_or(' ')
        };
        write!(writer, "{m_char}")?;
        right_used += 1;
    }

    let right_content_w = right_w.saturating_sub(right_used);
    if let Some(cell) = right_cell {
        let right_wrap_slice = wrap_slices.and_then(|(_, _, r_s)| r_s);
        if is_continuation && right_wrap_slice.is_none() {
            if right_content_w > 0 {
                write!(writer, "{:pad$}", "", pad = right_content_w)?;
            }
        } else {
            let params = CellContentParams {
                base_style: &right_style,
                emph_style: &right_emph_style,
                target_width: right_content_w,
                col_offset,
                is_cursor,
                profile: theme.profile,
                wrap_slice: right_wrap_slice,
            };
            paint_cell_content(writer, cell, options, &params)?;
        }
    } else if right_content_w > 0 {
        write!(writer, "{:pad$}", "", pad = right_content_w)?;
    }

    write!(writer, "\x1b[0m")?;
    Ok(())
}

/// Helper to render a cell's text, applying syntax highlighting, word emphasis, scrolling, truncation indicator, and padding.
fn paint_cell_content<W: Write>(
    writer: &mut W,
    cell: &RowCell,
    options: &ViewOptions,
    params: &CellContentParams<'_>,
) -> std::io::Result<()> {
    let show_left_scroll = params.col_offset > 0
        && params.wrap_slice.is_none()
        && !cell.text.is_empty()
        && params.target_width >= 2;
    let avail_width = if show_left_scroll {
        params.target_width - 1
    } else {
        params.target_width
    };

    if show_left_scroll {
        let scroll_char = if options.line_graphics == LineGraphics::Ascii {
            '<'
        } else {
            '‹'
        };
        let mut ind_style = *params.base_style;
        ind_style.attrs |= Attrs::DIM;
        write_style(writer, &ind_style, params.is_cursor)?;
        write!(writer, "{scroll_char}")?;
        write_style(writer, params.base_style, params.is_cursor)?;
    }

    let (char_offset, visible_chars, display_w, is_truncated) =
        if let Some((off, chs, dw)) = params.wrap_slice {
            (off, chs.to_vec(), dw, false)
        } else {
            let mut v: Vec<char> = Vec::new();
            let mut dw = 0usize;
            let mut trunc = false;
            for ch in cell.text.chars().skip(params.col_offset) {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if dw + cw > avail_width {
                    trunc = true;
                    break;
                }
                v.push(ch);
                dw += cw;
            }
            if trunc && avail_width > 0 {
                while let Some(&last_ch) = v.last() {
                    let cw = unicode_width::UnicodeWidthChar::width(last_ch).unwrap_or(0);
                    if dw + 1 > avail_width {
                        v.pop();
                        dw = dw.saturating_sub(cw);
                    } else {
                        break;
                    }
                }
            }
            (params.col_offset, v, dw, trunc)
        };

    let has_syntax = options.syntax_highlighting
        && !cell.syntax_spans.is_empty()
        && params.profile != ColorProfile::Monochrome;
    let has_emph = (options.word_diff || options.diff_layout == DiffLayout::SideBySide)
        && !cell.emphasis.is_empty();

    if (has_syntax || has_emph) && !params.is_cursor {
        paint_styled_chars(
            writer,
            &visible_chars,
            char_offset,
            cell,
            params.base_style,
            params.emph_style,
            options,
            params.profile,
        )?;
    } else {
        for &ch in &visible_chars {
            write!(writer, "{ch}")?;
        }
    }

    if is_truncated {
        let mut trunc_style = *params.base_style;
        trunc_style.attrs |= Attrs::DIM;
        write_style(writer, &trunc_style, params.is_cursor)?;
        write!(writer, "$")?;
        let pad_after_trunc = avail_width.saturating_sub(display_w + 1);
        if pad_after_trunc > 0 {
            write_style(writer, params.base_style, params.is_cursor)?;
            write!(writer, "{:pad_after_trunc$}", "")?;
        }
    } else {
        let padding = avail_width.saturating_sub(display_w);
        if padding > 0 {
            write_style(writer, params.base_style, params.is_cursor)?;
            write!(writer, "{:padding$}", "")?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{DiffIndicator, DiffPresentation};
    use crate::term_cap::TerminalCapabilities;

    #[test]
    fn test_write_style_ansi_sgr_sequences() {
        let mut buf = Vec::new();

        // Cursor mode emits reverse video \x1b[7m
        let style = DiffStyle::fg(Color::Green);
        write_style(&mut buf, &style, true).unwrap();
        assert_eq!(buf, b"\x1b[7m");

        // Standard 16 color + bold
        buf.clear();
        let bold_red = DiffStyle {
            fg: Some(Color::Red),
            bg: Some(Color::Black),
            attrs: Attrs::BOLD,
        };
        write_style(&mut buf, &bold_red, false).unwrap();
        let s = String::from_utf8(buf.clone()).unwrap();
        assert!(s.contains("\x1b[31m"));
        assert!(s.contains("\x1b[40m"));
        assert!(s.contains("\x1b[1m"));

        // 256-color and TrueColor RGB
        buf.clear();
        let rgb_style = DiffStyle {
            fg: Some(Color::Rgb(10, 20, 30)),
            bg: Some(Color::Ansi256(240)),
            attrs: Attrs::empty(),
        };
        write_style(&mut buf, &rgb_style, false).unwrap();
        let s_rgb = String::from_utf8(buf).unwrap();
        assert!(s_rgb.contains("\x1b[38;2;10;20;30m"));
        assert!(s_rgb.contains("\x1b[48;5;240m"));
    }

    #[test]
    fn test_style_for_type_and_paint_diff_viewport() {
        let caps = TerminalCapabilities {
            color_profile: ColorProfile::TrueColor,
            supports_kitty_keyboard: true,
            supports_unicode_box: true,
            supports_synchronized_output: true,
        };
        let theme = DiffTheme::for_presentation(DiffPresentation::Classic)
            .for_capabilities(&caps, DiffIndicator::Yes);
        assert_eq!(style_for_type(&theme, DiffLineType::DiffAdd), theme.add);
        assert_eq!(style_for_type(&theme, DiffLineType::DiffDel), theme.del);
        assert_eq!(
            style_for_type(&theme, DiffLineType::HunkHeader),
            theme.hunk_header
        );

        let options = ViewOptions::default();
        let doc = DiffDocument::new(
            vec![RowPair::unified(
                RowCell::new(LineMarker::Add, "fn added() {}", Some(5)),
                None,
                DiffLineType::DiffAdd,
            )],
            vec![],
            vec![],
            vec![Some(0)],
            vec![None],
        );

        let viewport = DiffViewport {
            width: 80,
            height: 20,
            scroll_offset: 0,
            col_offset: 0,
            cursor: 0,
        };

        let mut out = Vec::new();
        paint_diff(&mut out, &doc, &theme, &options, &viewport).unwrap();
        let rendered = String::from_utf8_lossy(&out);
        assert!(rendered.contains("fn added() {}"));
    }
}
