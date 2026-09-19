// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Suite 3: Terminal Rendering Cell-by-Cell Golden Parity & Differential Damage-Tracking Invariant Tests.
//!
//! Verifies:
//! 1. Algebraic equivalence of incremental damage-tracking updates:
//!    `ApplyAnsi(Frame_1) + ApplyIncrementalAnsi(Frame_2) == ApplyFullRepaint(Frame_2)`
//!    across all `width * height` `Cell` structs (`ch`, `fg`, `bg`, `attrs`).
//! 2. Byte-volume reduction of damage tracking during single-row cursor motion.
//! 3. Wide Unicode character (CJK / Emoji) 2-cell width alignment and `\0` continuation cell invariants.
//! 4. Color profile downsampling invariants (`TrueColor`, `Ansi256`, `Ansi16`, `Monochrome`).

use std::io::Write;
use std::sync::Arc;
use tigrs_git::diff::{
    CommitDiff, DiffHunk, DiffLineKind, DiffSummaryStats, FileChangeStatus, FileDiff, HunkLine,
};
use tigrs_git::types::{CommitSummary, ParentIds};
use tigrs_ui::app::AppState;
use tigrs_ui::app::actions::execute_action;
use tigrs_ui::app::layout::ViewKind;
use tigrs_ui::app::render::render_active;
use tigrs_ui::headless::{CellAttrs, Color, HeadlessTerminal};
use tigrs_ui::keymap::Action;
use tigrs_ui::term_cap::ColorProfile;
use tigrs_ui::view::{DiffView, MainView};

fn make_oid(idx: u32) -> gix::ObjectId {
    let mut bytes = [0u8; 20];
    bytes[0..4].copy_from_slice(&idx.to_be_bytes());
    bytes[16..20].copy_from_slice(&idx.to_le_bytes());
    gix::ObjectId::from_bytes_or_panic(&bytes)
}

fn build_unicode_commits() -> Vec<CommitSummary> {
    let mut p100 = ParentIds::new();
    p100.push(make_oid(99));

    let mut p99 = ParentIds::new();
    p99.push(make_oid(98));

    vec![
        CommitSummary {
            id: make_oid(100),
            parents: p100,
            author_name: Arc::from("Linus Torvalds <torvalds@linux-foundation.org>"),
            author_time_secs: 1_700_000_500,
            summary: Box::from("mm/page_alloc: 修复内核内存分配器性能瓶颈 🚀"),
        },
        CommitSummary {
            id: make_oid(99),
            parents: p99,
            author_name: Arc::from("Alice 张三 <alice@kernel.org>"),
            author_time_secs: 1_700_000_400,
            summary: Box::from("fs/vfs: lock-free openat/fstatat directory scanner 🦀"),
        },
        CommitSummary {
            id: make_oid(98),
            parents: ParentIds::new(),
            author_name: Arc::from("Bob 李四 <bob@kernel.org>"),
            author_time_secs: 1_700_000_300,
            summary: Box::from("Initial commit: kernel bootstrap"),
        },
    ]
}

fn build_sample_diff() -> CommitDiff {
    CommitDiff {
        commit_id: make_oid(100),
        parent_ids: vec![make_oid(99)],
        author_name: Arc::from("Linus Torvalds"),
        author_email: Arc::from("torvalds@linux-foundation.org"),
        author_date: "2026-09-17 12:00:00 +0000".to_string(),
        committer_name: Arc::from("Linus Torvalds"),
        committer_email: Arc::from("torvalds@linux-foundation.org"),
        committer_date: "2026-09-17 12:00:00 +0000".to_string(),
        title: Arc::from("mm/page_alloc: 修复内核内存分配器性能瓶颈 🚀"),
        body: Some("Signed-off-by: Linus Torvalds <torvalds@linux-foundation.org>\n".to_string()),
        stats: DiffSummaryStats {
            files_changed: 1,
            insertions: 2,
            deletions: 1,
        },
        files: vec![FileDiff {
            path: "mm/page_alloc.c".to_string(),
            status: FileChangeStatus::Modified,
            old_id: Some(make_oid(10)),
            new_id: Some(make_oid(11)),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_644),
            is_binary: false,
            additions: 2,
            deletions: 1,
            hunks: vec![DiffHunk {
                old_start: 10,
                old_len: 4,
                new_start: 10,
                new_len: 5,
                func_context: Some("struct page *alloc_pages(gfp_t gfp)".to_string()),
                lines: vec![
                    HunkLine {
                        kind: DiffLineKind::Context,
                        content: "    unsigned int order = 0;".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Remove,
                        content: "    return __alloc_pages_slowpath(gfp, order);".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Add,
                        content: "    // Fast path: 零拷贝分配 🚀".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Add,
                        content: "    return __alloc_pages_fast(gfp, order);".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Context,
                        content: "}".to_string(),
                        no_newline_at_eof: false,
                    },
                ],
            }],
        }],
    }
}

#[test]
fn test_differential_damage_tracking_algebraic_equivalence_main_and_diff() {
    let width = 100u16;
    let height = 30u16;

    let mut app = AppState::default();
    let mut main_view = MainView::new("main".to_string());
    main_view.append_commits(build_unicode_commits());
    app.views.main_view = Some(main_view);
    app.views.diff_view = Some(DiffView::new(build_sample_diff()));
    app.views.view_stack.push(ViewKind::Main);

    let mut term_incremental = HeadlessTerminal::new(width, height);

    // Frame 1: Initial full render into `term_incremental`
    let mut frame1_bytes = Vec::new();
    render_active(&app, &mut frame1_bytes, width, height).unwrap();
    term_incremental.write_all(&frame1_bytes).unwrap();
    let full_frame_size = frame1_bytes.len();

    // Verify initial cursor row (row 1, below title bar at row 0) has REVERSE video attribute
    assert!(
        term_incremental.line_has_reverse(1),
        "Selected commit row 1 must have reverse video attribute"
    );

    // Frame 2: Move cursor down (`Action::MoveDown`) -> incremental damage-tracking render
    execute_action(&mut app, &Action::MoveDown, height as usize);
    let mut frame2_inc_bytes = Vec::new();
    render_active(&app, &mut frame2_inc_bytes, width, height).unwrap();
    term_incremental.write_all(&frame2_inc_bytes).unwrap();

    // Damage tracking must emit significantly fewer bytes than a full screen repaint
    assert!(
        frame2_inc_bytes.len() < full_frame_size / 2,
        "Damage tracking emitted {} bytes vs full frame {} bytes; expected < 50%",
        frame2_inc_bytes.len(),
        full_frame_size
    );

    // Now perform a cold full repaint of Frame 2 into `term_cold_full`
    app.renderer.borrow_mut().invalidate();
    let mut term_cold_full = HeadlessTerminal::new(width, height);
    let mut frame2_cold_bytes = Vec::new();
    render_active(&app, &mut frame2_cold_bytes, width, height).unwrap();
    term_cold_full.write_all(&frame2_cold_bytes).unwrap();

    // ALGEBRAIC EQUIVALENCE INVARIANT:
    // Every single cell (ch, fg, bg, attrs) in `term_incremental` MUST equal `term_cold_full`!
    for y in 0..height as usize {
        let inc_row = term_incremental.line_cells(y).unwrap();
        let full_row = term_cold_full.line_cells(y).unwrap();
        assert_eq!(
            inc_row, full_row,
            "Cell mismatch on row {y} between incremental damage render and cold full repaint"
        );
    }
}

#[test]
fn test_wide_unicode_cjk_and_emoji_cell_continuation_invariants() {
    let width = 100u16;
    let height = 25u16;

    let mut app = AppState::default();
    app.views.diff_view = Some(DiffView::new(build_sample_diff()));
    app.views.view_stack.push(ViewKind::Diff);

    let mut term = HeadlessTerminal::new(width, height);
    let mut buf = Vec::new();
    render_active(&app, &mut buf, width, height).unwrap();
    term.write_all(&buf).unwrap();

    // Find the line containing CJK characters "零拷贝分配"
    let (x_pos, y_pos) = term
        .find_text("零拷贝")
        .expect("Diff view must render CJK comment line");
    let row_cells = term.line_cells(y_pos).unwrap();

    // '零' is at x_pos; being a 2-column wide CJK character, x_pos + 1 MUST be the '\0' continuation cell
    assert_eq!(row_cells[x_pos].ch, '零');
    assert_eq!(
        row_cells[x_pos + 1].ch,
        '\0',
        "Wide CJK character '零' must be followed by '\\0' continuation cell at x+1"
    );
    assert_eq!(
        row_cells[x_pos + 2].ch,
        '拷',
        "Next wide CJK character '拷' must start at x+2"
    );
    assert_eq!(row_cells[x_pos + 3].ch, '\0');
}

#[test]
fn test_color_profile_downsampling_cell_invariants() {
    let width = 100u16;
    let height = 25u16;

    let mut app = AppState::default();
    app.views.diff_view = Some(DiffView::new(build_sample_diff()));
    app.views.view_stack.push(ViewKind::Diff);

    // 1. Monochrome profile: NO cell may have `fg: Some(_)` or `bg: Some(_)`
    app.caps.color_profile = ColorProfile::Monochrome;
    app.renderer.borrow_mut().invalidate();
    let mut term_mono = HeadlessTerminal::new(width, height);
    let mut buf_mono = Vec::new();
    render_active(&app, &mut buf_mono, width, height).unwrap();
    term_mono.write_all(&buf_mono).unwrap();

    let mut found_reverse = false;
    for y in 0..height as usize {
        for cell in term_mono.line_cells(y).unwrap() {
            assert!(
                cell.fg.is_none() && cell.bg.is_none(),
                "Monochrome profile cell must have fg=None and bg=None, got: {cell:?}"
            );
            if cell.attrs.contains(CellAttrs::REVERSE) {
                found_reverse = true;
            }
        }
    }
    assert!(
        found_reverse,
        "Monochrome profile must preserve CellAttrs::REVERSE on cursor/title rows"
    );

    // 2. Ansi256 profile: NO cell may contain `Color::Rgb`
    app.caps.color_profile = ColorProfile::Ansi256;
    app.renderer.borrow_mut().invalidate();
    let mut term_256 = HeadlessTerminal::new(width, height);
    let mut buf_256 = Vec::new();
    render_active(&app, &mut buf_256, width, height).unwrap();
    term_256.write_all(&buf_256).unwrap();

    for y in 0..height as usize {
        for cell in term_256.line_cells(y).unwrap() {
            assert!(
                !matches!(cell.fg, Some(Color::Rgb(_, _, _))),
                "Ansi256 cell fg must not be Rgb: {cell:?}"
            );
            assert!(
                !matches!(cell.bg, Some(Color::Rgb(_, _, _))),
                "Ansi256 cell bg must not be Rgb: {cell:?}"
            );
        }
    }

    // 3. Ansi16 profile: NO cell may contain `Color::Rgb` or `Color::Ansi256`
    app.caps.color_profile = ColorProfile::Ansi16;
    app.renderer.borrow_mut().invalidate();
    let mut term_16 = HeadlessTerminal::new(width, height);
    let mut buf_16 = Vec::new();
    render_active(&app, &mut buf_16, width, height).unwrap();
    term_16.write_all(&buf_16).unwrap();

    for y in 0..height as usize {
        for cell in term_16.line_cells(y).unwrap() {
            assert!(
                !matches!(cell.fg, Some(Color::Rgb(_, _, _) | Color::Ansi256(_))),
                "Ansi16 cell fg must be 16-color palette: {cell:?}"
            );
            assert!(
                !matches!(cell.bg, Some(Color::Rgb(_, _, _) | Color::Ansi256(_))),
                "Ansi16 cell bg must be 16-color palette: {cell:?}"
            );
        }
    }
}

#[test]
fn test_split_view_vertical_and_horizontal_damage_equivalence() {
    let width = 140u16;
    let height = 35u16;

    let mut app = AppState::default();
    let mut main_view = MainView::new("main".to_string());
    main_view.append_commits(build_unicode_commits());
    app.views.main_view = Some(main_view);
    app.views.diff_view = Some(DiffView::new(build_sample_diff()));
    app.views.view_stack.push(ViewKind::Main);
    app.views.view_stack.push(ViewKind::Diff);

    // Enable vertical split layout (`vertical_split = true`)
    app.options.vertical_split = true;

    let mut term_inc = HeadlessTerminal::new(width, height);
    let mut initial_bytes = Vec::new();
    render_active(&app, &mut initial_bytes, width, height).unwrap();
    term_inc.write_all(&initial_bytes).unwrap();

    // Perform a sequence of split-view actions and verify incremental == cold full repaint after EACH step
    let actions = [
        Action::MoveDown,
        Action::MoveDown,
        Action::ScrollRight,
        Action::ViewNext, // Switch focus to Main pane
        Action::MoveDown,
        Action::ViewNext, // Switch focus back to Diff pane
        Action::MoveUp,
    ];

    for (step_idx, act) in actions.iter().enumerate() {
        execute_action(&mut app, act, height as usize);

        let mut inc_bytes = Vec::new();
        render_active(&app, &mut inc_bytes, width, height).unwrap();
        term_inc.write_all(&inc_bytes).unwrap();

        // Force a cold full repaint on a fresh HeadlessTerminal
        app.renderer.borrow_mut().invalidate();
        let mut term_cold = HeadlessTerminal::new(width, height);
        let mut cold_bytes = Vec::new();
        render_active(&app, &mut cold_bytes, width, height).unwrap();
        term_cold.write_all(&cold_bytes).unwrap();

        for y in 0..height as usize {
            assert_eq!(
                term_inc.line_cells(y).unwrap(),
                term_cold.line_cells(y).unwrap(),
                "Split-view cell mismatch at step {step_idx} ({act:?}), row {y}"
            );
        }
    }
}

#[test]
fn test_all_14_themes_across_4_color_profiles_and_high_contrast_cursor_legibility() {
    use tigrs_core::UiThemeId;
    use tigrs_ui::ui_theme::{ThemeCategory, UiPalette};

    let width = 100u16;
    let height = 20u16;

    let mut app = AppState::default();
    let mut main_view = MainView::new("main".to_string());
    main_view.append_commits(build_unicode_commits());
    app.views.main_view = Some(main_view);
    app.views.view_stack.push(ViewKind::Main);

    for &theme_id in UiThemeId::ALL {
        app.options.ui_theme = theme_id;
        let palette = UiPalette::for_theme(theme_id);

        for profile in [
            ColorProfile::TrueColor,
            ColorProfile::Ansi256,
            ColorProfile::Ansi16,
            ColorProfile::Monochrome,
        ] {
            app.caps.color_profile = profile;
            app.renderer.borrow_mut().invalidate();
            let mut term = HeadlessTerminal::new(width, height);
            let mut bytes = Vec::new();
            render_active(&app, &mut bytes, width, height).unwrap();
            term.write_all(&bytes).unwrap();

            // In TrueColor HighContrastDark / HighContrastLight, verify cursor row (row 1)
            // enforces >= 7.0 WCAG AAA contrast ratio on all non-space text cells.
            if profile == ColorProfile::TrueColor
                && matches!(
                    palette.category,
                    ThemeCategory::HighContrastDark | ThemeCategory::HighContrastLight
                )
            {
                let cursor_cells = term.line_cells(1).unwrap();
                for (col, cell) in cursor_cells.iter().enumerate() {
                    if !cell.ch.is_whitespace()
                        && cell.ch != '\0'
                        && let (Some(fg), Some(bg)) = (cell.fg, cell.bg)
                    {
                        let ratio = UiPalette::contrast_ratio(fg, bg);
                        assert!(
                            ratio >= 7.0,
                            "Theme {:?} cursor row col {} ({:?}) fg={:?} bg={:?} contrast {:.2} < 7.0",
                            theme_id,
                            col,
                            cell.ch,
                            fg,
                            bg,
                            ratio
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn test_all_views_narrow_terminal_widths_no_panic() {
    let mut app = AppState::default();
    let mut main_view = MainView::new("main".to_string());
    main_view.append_commits(build_unicode_commits());
    app.views.main_view = Some(main_view);
    app.views.diff_view = Some(DiffView::new(build_sample_diff()));

    for view_kind in [ViewKind::Main, ViewKind::Diff, ViewKind::Help] {
        app.views.view_stack.clear();
        app.views.view_stack.push(view_kind);
        for width in [1u16, 5, 12, 20, 40] {
            app.renderer.borrow_mut().invalidate();
            let mut term = HeadlessTerminal::new(width, 10);
            let mut bytes = Vec::new();
            render_active(&app, &mut bytes, width, 10).unwrap();
            term.write_all(&bytes).unwrap();
        }
    }
}
