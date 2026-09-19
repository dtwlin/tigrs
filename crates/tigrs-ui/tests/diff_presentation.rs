// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Integration tests for Diff Presentation, Full-File View, Side-by-Side Dual Pane,
//! and Formatter Pipeline.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::fs::File;
use std::io::Write;
use std::process::Command;
use std::sync::Arc;
use tempfile::TempDir;
use tigrs_git::{
    CommitDiff, DiffHunk, DiffLineKind, DiffSummaryStats, FileChangeStatus, FileDiff, GitEngine,
    HunkLine, ObjectId,
};
use tigrs_ui::diff::theme::DiffTheme;
use tigrs_ui::diff::{DiffLineType, build_diff_document, run_external_formatter};
use tigrs_ui::{
    AppState, Color, ColorProfile, DIFF_CONTEXT_FULL, DiffIndicator, DiffLayout, DiffPresentation,
    DiffView, HeadlessTerminal, TerminalCapabilities, ViewKind, ViewOptions,
    handle_event_with_dimensions,
};

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn make_oid(b: u8) -> ObjectId {
    ObjectId::from_bytes_or_panic(&[b; 20])
}

fn screen_text(term: &HeadlessTerminal, height: usize) -> String {
    let mut out = String::new();
    for y in 0..height {
        out.push_str(&term.line_text(y));
        out.push('\n');
    }
    out
}

fn sample_diff() -> CommitDiff {
    let hunk1 = DiffHunk {
        old_start: 1,
        old_len: 5,
        new_start: 1,
        new_len: 5,
        func_context: Some("fn main()".to_string()),
        lines: vec![
            HunkLine {
                kind: DiffLineKind::Context,
                content: "fn main() {".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Remove,
                content: "    let message = \"Hello old\";".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Add,
                content: "    let message = \"Hello new\";".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Context,
                content: "    println!(\"{}\", message);".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Context,
                content: "}".to_string(),
                no_newline_at_eof: false,
            },
        ],
    };

    let hunk2 = DiffHunk {
        old_start: 10,
        old_len: 4,
        new_start: 10,
        new_len: 5,
        func_context: Some("fn helper()".to_string()),
        lines: vec![
            HunkLine {
                kind: DiffLineKind::Context,
                content: "fn helper() {".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Remove,
                content: "    first_deleted();".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Remove,
                content: "    second_deleted();".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Add,
                content: "    first_added();".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Add,
                content: "    inserted_middle();".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Add,
                content: "    second_added();".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Context,
                content: "}".to_string(),
                no_newline_at_eof: false,
            },
        ],
    };

    CommitDiff {
        commit_id: make_oid(0x2a),
        parent_ids: vec![make_oid(0x1a)],
        author_name: Arc::from("Tigrs Developer"),
        author_email: Arc::from("dev@example.com"),
        author_date: "Mon Sep 14 12:00:00 2026 +0000".to_string(),
        committer_name: Arc::from("Tigrs Developer"),
        committer_email: Arc::from("dev@example.com"),
        committer_date: "Mon Sep 14 12:00:00 2026 +0000".to_string(),
        title: Arc::from("Refactor greeting and helper routines"),
        body: Some("Detailed diff testing commit".to_string()),
        files: vec![FileDiff {
            status: FileChangeStatus::Modified,
            path: "src/main.rs".to_string(),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_644),
            is_binary: false,
            hunks: vec![hunk1, hunk2],
            old_id: Some(make_oid(0x11)),
            new_id: Some(make_oid(0x22)),
            additions: 4,
            deletions: 3,
        }],
        stats: DiffSummaryStats {
            files_changed: 1,
            insertions: 4,
            deletions: 3,
        },
    }
}

// ---------------------------------------------------------------------------
// 1. Native Fancy Presentation (F1)
// ---------------------------------------------------------------------------

#[test]
fn test_diff_presentation_fancy_rendering() {
    let diff = sample_diff();
    let opts = ViewOptions {
        diff_presentation: DiffPresentation::Fancy,
        ..Default::default()
    };

    let caps = TerminalCapabilities::detect();
    let doc = build_diff_document(&diff, &opts, &caps, None);

    // Verify document contains FileHeader banner and fancy HunkHeader row types
    let has_banner = doc.rows.iter().any(|r| {
        r.row_type == DiffLineType::FileHeader
            && r.left
                .as_ref()
                .is_some_and(|c| c.text.contains("modified: src/main.rs"))
    });
    assert!(
        has_banner,
        "Document must include file banner row with 'modified: src/main.rs'"
    );

    let has_fancy_header = doc.rows.iter().any(|r| {
        r.row_type == DiffLineType::HunkHeader
            && r.left
                .as_ref()
                .is_some_and(|c| c.text.contains("@ src/main.rs:2 @ fn main()"))
    });
    assert!(
        has_fancy_header,
        "Document must include fancy hunk header with @ path:line @"
    );

    // Render into HeadlessTerminal to check painted text and attributes
    let mut term = HeadlessTerminal::new(100, 30);
    let view = DiffView::new_with_options(diff, &opts, None);
    view.render_with_options(&mut term, 100, 30, &opts).unwrap();

    let text = screen_text(&term, 30);
    // Banner should render "modified: src/main.rs"
    assert!(
        text.contains("modified: src/main.rs"),
        "Screen should display modified file banner"
    );
    // Fancy hunk header should render "@ src/main.rs:2 @ fn main()"
    assert!(
        text.contains("@ src/main.rs:2 @ fn main()"),
        "Screen should display fancy hunk header with line number and function context"
    );
}

// ---------------------------------------------------------------------------
// 2. Diff Indicator Tri-State and Monochrome Forced Sign Override (D-2)
// ---------------------------------------------------------------------------

#[test]
fn test_diff_indicator_and_monochrome_forced_signs() {
    let diff = sample_diff();

    // In color profile, fancy presentation with diff_indicator = Auto strips signs
    let color_caps = TerminalCapabilities {
        color_profile: ColorProfile::TrueColor,
        supports_kitty_keyboard: false,
        supports_unicode_box: true,
        supports_synchronized_output: false,
    };
    let mut opts = ViewOptions {
        diff_presentation: DiffPresentation::Fancy,
        diff_indicator: DiffIndicator::Auto,
        ..Default::default()
    };

    let theme = DiffTheme::for_presentation(opts.diff_presentation)
        .for_capabilities(&color_caps, opts.diff_indicator);
    assert!(
        theme.strip_signs,
        "In color mode, fancy auto indicator should strip signs"
    );

    // Render into HeadlessTerminal with color options
    let mut term_color = HeadlessTerminal::new(100, 30);
    let view_color = DiffView::new_with_options(diff.clone(), &opts, None);
    view_color
        .render_with_capabilities(&mut term_color, 100, 30, &opts, &color_caps)
        .unwrap();
    let text_color = screen_text(&term_color, 30);

    // Under fancy with sign stripping, the line is rendered without a leading "+"
    assert!(
        text_color.contains("    let message = \"Hello new\";"),
        "Content text should be rendered"
    );
    assert!(
        !text_color.contains("+    let message = \"Hello new\";"),
        "Leading '+' sign must be stripped in color mode"
    );

    // In Monochrome profile, Invariant D-2 forces diff-indicator = true even under fancy
    let mono_caps = TerminalCapabilities {
        color_profile: ColorProfile::Monochrome,
        supports_kitty_keyboard: false,
        supports_unicode_box: true,
        supports_synchronized_output: false,
    };
    let theme_mono = DiffTheme::for_presentation(opts.diff_presentation)
        .for_capabilities(&mono_caps, opts.diff_indicator);
    assert!(
        !theme_mono.strip_signs,
        "Under Monochrome, strip_signs MUST be forced false to maintain accessibility"
    );

    // Render into HeadlessTerminal with monochrome capabilities
    let mut term_mono = HeadlessTerminal::new(100, 30);
    let view_mono = DiffView::new_with_options(diff, &opts, None);
    view_mono
        .render_with_capabilities(&mut term_mono, 100, 30, &opts, &mono_caps)
        .unwrap();
    let text_mono = screen_text(&term_mono, 30);

    // Under Monochrome, sign is forced to appear
    assert!(
        text_mono.contains("+    let message = \"Hello new\";"),
        "Leading plus sign must be present under Monochrome"
    );

    // Explicit DiffIndicator::Yes always retains signs (strip_signs = false)
    opts.diff_indicator = DiffIndicator::Yes;
    let theme_yes = DiffTheme::for_presentation(opts.diff_presentation)
        .for_capabilities(&color_caps, opts.diff_indicator);
    assert!(!theme_yes.strip_signs);

    // Explicit DiffIndicator::No always strips signs (strip_signs = true)
    opts.diff_indicator = DiffIndicator::No;
    let theme_no = DiffTheme::for_presentation(opts.diff_presentation)
        .for_capabilities(&color_caps, opts.diff_indicator);
    assert!(theme_no.strip_signs);
}

// ---------------------------------------------------------------------------
// 3. Side-by-Side Dual-Pane Layout & Similarity Alignment
// ---------------------------------------------------------------------------

#[test]
fn test_side_by_side_layout_and_similarity_alignment() {
    let diff = sample_diff();
    let opts = ViewOptions {
        diff_layout: DiffLayout::SideBySide,
        ..Default::default()
    };

    let caps = TerminalCapabilities::detect();
    let doc = build_diff_document(&diff, &opts, &caps, None);

    // In side-by-side mode, row.right is populated for diff content rows
    let paired_rows: Vec<_> = doc
        .rows
        .iter()
        .filter(|r| {
            r.right.is_some()
                && r.left.as_ref().is_some_and(|l| !l.text.is_empty())
                && r.right.as_ref().is_some_and(|rg| !rg.text.is_empty())
        })
        .collect();
    assert!(
        !paired_rows.is_empty(),
        "Side-by-side layout must produce paired left/right rows"
    );

    // Verify unequal added/deleted run alignment in hunk 2:
    // 2 deletions and 3 additions should be aligned with blank filler where needed
    let blank_filler_rows = doc.rows.iter().filter(|r| {
        r.right.is_some()
            && ((r.left.is_none() && r.right.is_some())
                || (r.left.is_some() && r.right.is_none())
                || (r.left.as_ref().is_some_and(|l| l.text.is_empty())
                    && r.right.as_ref().is_some_and(|rg| !rg.text.is_empty()))
                || (r.left.as_ref().is_some_and(|l| !l.text.is_empty())
                    && r.right.as_ref().is_some_and(|rg| rg.text.is_empty())))
    });
    assert!(
        blank_filler_rows.count() > 0,
        "Alignment must emit blank filler for unequal added/deleted counts"
    );

    // Render to HeadlessTerminal at 120 width
    let mut term = HeadlessTerminal::new(120, 30);
    let view = DiffView::new_with_options(diff, &opts, None);
    view.render_with_options(&mut term, 120, 30, &opts).unwrap();

    let text = screen_text(&term, 30);
    // The vertical divider bar '│' should separate the two panes
    assert!(
        text.contains('│'),
        "Side-by-side layout should render a vertical divider bar"
    );

    // Verify left and right contents are present
    assert!(text.contains("Hello old"));
    assert!(text.contains("Hello new"));
}

#[test]
fn test_side_by_side_narrow_terminal_fallback() {
    let diff = sample_diff();
    let opts = ViewOptions {
        diff_layout: DiffLayout::SideBySide,
        side_by_side_min_width: 80,
        ..Default::default()
    };

    // Terminal width 70 (< 80 min width): should fall back to unified rendering
    let mut term_narrow = HeadlessTerminal::new(70, 30);
    let view = DiffView::new_with_options(diff, &opts, None);
    view.render_with_options(&mut term_narrow, 70, 30, &opts)
        .unwrap();

    let narrow_text = screen_text(&term_narrow, 30);
    assert!(
        !narrow_text.contains('│'),
        "Narrow terminal should fall back to unified layout without divider bar"
    );
}

// ---------------------------------------------------------------------------
// 4. Horizontal Scrolling in Diff View
// ---------------------------------------------------------------------------

#[test]
fn test_diff_view_horizontal_scrolling() {
    let diff = sample_diff();
    let mut view = DiffView::new(diff);
    assert_eq!(view.col_offset(), 0);

    view.scroll_right(10);
    assert_eq!(view.col_offset(), 10);

    view.scroll_right(5);
    assert_eq!(view.col_offset(), 15);

    view.scroll_left(8);
    assert_eq!(view.col_offset(), 7);

    view.scroll_first_col();
    assert_eq!(view.col_offset(), 0);
}

// ---------------------------------------------------------------------------
// 5. Full-File Context & Blob Splicing with Staging Safety
// ---------------------------------------------------------------------------

#[test]
fn test_full_file_context_and_staging_safety() {
    // Setup a real git repo with a 15-line file
    let temp = TempDir::new().unwrap();
    let p = temp.path();

    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["init"])
        .current_dir(p)
        .output()
        .unwrap();
    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "user.name", "Test User"])
        .current_dir(p)
        .output()
        .unwrap();
    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(p)
        .output()
        .unwrap();

    let mut lines = Vec::new();
    for i in 1..=15 {
        lines.push(format!("line {i}"));
    }
    File::create(p.join("file.txt"))
        .unwrap()
        .write_all(lines.join("\n").as_bytes())
        .unwrap();

    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "file.txt"])
        .current_dir(p)
        .output()
        .unwrap();
    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "initial"])
        .current_dir(p)
        .output()
        .unwrap();

    // Modify line 8
    lines[7] = "line 8 modified".to_string();
    File::create(p.join("file.txt"))
        .unwrap()
        .write_all(lines.join("\n").as_bytes())
        .unwrap();

    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "file.txt"])
        .current_dir(p)
        .output()
        .unwrap();
    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "second"])
        .current_dir(p)
        .output()
        .unwrap();

    let commit2_str = String::from_utf8(
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["rev-parse", "HEAD"])
            .current_dir(p)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let engine = GitEngine::open(Some(p)).unwrap();
    let commit2_oid = ObjectId::from_hex(commit2_str.as_bytes()).unwrap();
    let diff = engine.compute_commit_diff(commit2_oid).unwrap();

    // Build standard document (diff_context = 3)
    let standard_opts = ViewOptions::default();
    let caps = TerminalCapabilities::detect();
    let standard_doc = build_diff_document(&diff, &standard_opts, &caps, None);

    // Build full-file document (diff_context = DIFF_CONTEXT_FULL)
    let full_opts = ViewOptions {
        diff_context: DIFF_CONTEXT_FULL,
        ..Default::default()
    };

    let repo = engine.repository();
    let provider = |oid: ObjectId| -> Option<Arc<[Arc<str>]>> {
        tigrs_git::tree::read_blob_raw_lines(repo, oid).ok()
    };

    let full_doc = build_diff_document(&diff, &full_opts, &caps, Some(&provider));

    // Full file doc must have significantly more lines than standard 3-context doc
    assert!(
        full_doc.rows.len() > standard_doc.rows.len(),
        "Full-file diff document must include spliced context lines"
    );

    // Invariant D-5 / Staging Safety:
    // Context lines spliced outside canonical hunks MUST have anchor: None
    let unanchored_context = full_doc
        .rows
        .iter()
        .filter(|r| r.row_type == DiffLineType::DiffContext && r.anchor.is_none())
        .count();
    assert!(
        unanchored_context > 0,
        "Spliced full-file context rows must have anchor: None to guarantee staging safety"
    );
}

// ---------------------------------------------------------------------------
// 6. External Diff Formatter Escape Hatch (F4)
// ---------------------------------------------------------------------------

#[test]
fn test_external_diff_formatter_and_cap() {
    let diff = sample_diff();
    let caps = TerminalCapabilities::detect();

    // 1. Successful execution of an echo-based external formatter
    let cmd = "echo '+formatted addition'";
    let doc = run_external_formatter(&diff, cmd, 80, &caps).expect("formatter should succeed");
    assert!(!doc.rows.is_empty(), "Formatter document must not be empty");
    assert_eq!(doc.rows[0].row_type, DiffLineType::DiffHeader);
    assert!(
        doc.rows[0]
            .left
            .as_ref()
            .unwrap()
            .text
            .contains("formatted addition")
    );

    // 2. Output cap enforcement: command generating > 32MB gets capped
    let infinite_cmd = "head -c 35000000 /dev/zero | tr '\\0' 'A'";
    let res = run_external_formatter(&diff, infinite_cmd, 80, &caps);
    // Should safely return error without crashing or hanging
    assert!(
        res.is_err(),
        "Output exceeding 32MB should be capped and return error"
    );

    // 3. Timeout guard: command that sleeps longer than 5s fails gracefully
    let timeout_cmd = "sleep 10";
    let start = std::time::Instant::now();
    let res_timeout = run_external_formatter(&diff, timeout_cmd, 80, &caps);
    let elapsed = start.elapsed();
    assert!(
        res_timeout.is_err(),
        "External command timing out should return error"
    );
    assert!(
        elapsed.as_secs() >= 4 && elapsed.as_secs() <= 7,
        "Timeout guard should trigger around 5 seconds, elapsed: {elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. Keymap Dispatches and Upstream Tig Parity in AppState
// ---------------------------------------------------------------------------

#[test]
fn test_app_state_diff_layout_and_context_keybindings() {
    let diff = sample_diff();
    let mut app = AppState::default();
    app.push_view(ViewKind::Diff);
    app.views.diff_view = Some(DiffView::new(diff));

    // Initial state
    assert_eq!(app.options.diff_layout, DiffLayout::Unified);
    assert_eq!(app.options.diff_context, 3);

    // 'v' toggles diff-layout to SideBySide
    handle_event_with_dimensions(&key(KeyCode::Char('v')), &mut app, 120, 30);
    assert_eq!(app.options.diff_layout, DiffLayout::SideBySide);
    assert!(
        app.status_message
            .as_ref()
            .unwrap()
            .contains("side-by-side")
    );

    // 'v' again toggles back to Unified
    handle_event_with_dimensions(&key(KeyCode::Char('v')), &mut app, 120, 30);
    assert_eq!(app.options.diff_layout, DiffLayout::Unified);
    assert!(app.status_message.as_ref().unwrap().contains("unified"));

    // ']' increases diff-context (upstream Tig parity)
    handle_event_with_dimensions(&key(KeyCode::Char(']')), &mut app, 120, 30);
    assert_eq!(app.options.diff_context, 4);

    // '[' decreases diff-context (upstream Tig parity)
    handle_event_with_dimensions(&key(KeyCode::Char('[')), &mut app, 120, 30);
    assert_eq!(app.options.diff_context, 3);

    // Initially cursor is at 0 (commit header)
    let initial_cursor = app.views.diff_view.as_ref().unwrap().cursor_index();
    assert_eq!(initial_cursor, 0);

    // ')' navigates to first hunk header
    handle_event_with_dimensions(&key(KeyCode::Char(')')), &mut app, 120, 30);
    let hunk1_cursor = app.views.diff_view.as_ref().unwrap().cursor_index();
    assert!(hunk1_cursor > initial_cursor);

    // ')' again navigates to second hunk header
    handle_event_with_dimensions(&key(KeyCode::Char(')')), &mut app, 120, 30);
    let hunk2_cursor = app.views.diff_view.as_ref().unwrap().cursor_index();
    assert!(hunk2_cursor > hunk1_cursor);

    // '(' navigates back to first hunk header
    handle_event_with_dimensions(&key(KeyCode::Char('(')), &mut app, 120, 30);
    let back_cursor = app.views.diff_view.as_ref().unwrap().cursor_index();
    assert_eq!(back_cursor, hunk1_cursor);

    // Horizontal scrolling keys
    handle_event_with_dimensions(&key(KeyCode::Right), &mut app, 120, 30);
    assert_eq!(app.views.diff_view.as_ref().unwrap().col_offset(), 4);

    handle_event_with_dimensions(&key(KeyCode::Right), &mut app, 120, 30);
    assert_eq!(app.views.diff_view.as_ref().unwrap().col_offset(), 8);

    handle_event_with_dimensions(&key(KeyCode::Left), &mut app, 120, 30);
    assert_eq!(app.views.diff_view.as_ref().unwrap().col_offset(), 4);

    handle_event_with_dimensions(&key(KeyCode::Char('|')), &mut app, 120, 30);
    assert_eq!(app.views.diff_view.as_ref().unwrap().col_offset(), 0);
}

#[test]
fn test_diff_view_commit_title_overflow() {
    let diff = sample_diff(); // Title: "Refactor greeting and helper routines"
    let opts = ViewOptions {
        commit_title_overflow: Some(10),
        ..Default::default()
    };
    let view = DiffView::new_with_options(diff, &opts, None);
    let mut term = HeadlessTerminal::new(100, 30);
    let caps = TerminalCapabilities {
        color_profile: ColorProfile::TrueColor,
        supports_kitty_keyboard: false,
        supports_unicode_box: true,
        supports_synchronized_output: false,
    };
    view.render_with_capabilities(&mut term, 100, 30, &opts, &caps)
        .unwrap();

    // The body title line is indented by 4 spaces: "    Refactor greeting and helper routines"
    let (_, title_y) = term
        .find_text("    Refactor")
        .expect("find indented body title line");

    // First 10 chars of title ("Refactor g", columns 4..14) should be White
    let normal_cell = term.cell(4, title_y).expect("normal title cell");
    assert_eq!(normal_cell.fg, Some(Color::White));

    // Overflow chars past column 10 (starting at column 4 + 10 = 14: "reeting...") should be Red
    let overflow_cell = term.cell(14, title_y).expect("overflow cell");
    assert_eq!(overflow_cell.fg, Some(Color::Red));
}

#[test]
fn test_commit_message_syntax_highlighting_and_trailers() {
    let mut diff = sample_diff();
    diff.parent_ids = vec![make_oid(1), make_oid(2)];
    diff.author_name = Arc::from("Alice");
    diff.author_email = Arc::from("alice@example.com");
    diff.committer_name = Arc::from("Bob");
    diff.committer_email = Arc::from("bob@example.com");
    diff.title = Arc::from("Implement feature XYZ");
    diff.body = Some(
        "    This is the first paragraph.\n    More details about the feature.\n\n    Signed-off-by: Alice <alice@example.com>\n    Reviewed-by: Charlie <charlie@example.com>\n    Co-authored-by: Dave <dave@example.com>\n    Fixes: #42"
            .to_string(),
    );

    let opts = ViewOptions::default();
    let mut view = DiffView::new_with_options(diff, &opts, None);
    // Move cursor to normal body line so headers and trailers are painted with their base colors
    view.set_cursor(8, 30);
    let mut term = HeadlessTerminal::new(100, 30);
    let caps = TerminalCapabilities {
        color_profile: ColorProfile::TrueColor,
        supports_kitty_keyboard: false,
        supports_unicode_box: true,
        supports_synchronized_output: false,
    };
    view.render_with_capabilities(&mut term, 100, 30, &opts, &caps)
        .unwrap();

    // 1. Commit ID should be Green
    let (commit_x, commit_y) = term.find_text("commit ").expect("find commit header");
    assert_eq!(
        term.cell(commit_x, commit_y).unwrap().fg,
        Some(Color::Green)
    );

    // 2. Merge header should be Blue (upstream parity!)
    let (merge_x, merge_y) = term.find_text("Merge:").expect("find Merge header");
    assert_eq!(term.cell(merge_x, merge_y).unwrap().fg, Some(Color::Blue));

    // 3. Author header should be Cyan
    let (author_x, author_y) = term.find_text("Author:").expect("find Author header");
    assert_eq!(term.cell(author_x, author_y).unwrap().fg, Some(Color::Cyan));

    // 4. Committer header should be Magenta (upstream parity!)
    let (commit_hdr_x, commit_hdr_y) = term.find_text("Commit:").expect("find Commit header");
    assert_eq!(
        term.cell(commit_hdr_x, commit_hdr_y).unwrap().fg,
        Some(Color::Magenta)
    );

    // 5. AuthorDate & CommitDate should be Yellow
    let (ad_x, ad_y) = term
        .find_text("AuthorDate:")
        .expect("find AuthorDate header");
    assert_eq!(term.cell(ad_x, ad_y).unwrap().fg, Some(Color::Yellow));

    let (cd_x, cd_y) = term
        .find_text("CommitDate:")
        .expect("find CommitDate header");
    assert_eq!(term.cell(cd_x, cd_y).unwrap().fg, Some(Color::Yellow));

    // 6. Normal commit body line should have default foreground (None)
    let (body_x, body_y) = term
        .find_text("This is the first")
        .expect("find normal body");
    assert_eq!(term.cell(body_x, body_y).unwrap().fg, None);

    // 7. Git trailers in body must be Yellow (upstream parity!)
    let (sob_x, sob_y) = term
        .find_text("Signed-off-by:")
        .expect("find Signed-off-by");
    assert_eq!(term.cell(sob_x, sob_y).unwrap().fg, Some(Color::Yellow));

    let (rev_x, rev_y) = term.find_text("Reviewed-by:").expect("find Reviewed-by");
    assert_eq!(term.cell(rev_x, rev_y).unwrap().fg, Some(Color::Yellow));

    let (coa_x, coa_y) = term
        .find_text("Co-authored-by:")
        .expect("find Co-authored-by");
    assert_eq!(term.cell(coa_x, coa_y).unwrap().fg, Some(Color::Yellow));

    let (fix_x, fix_y) = term.find_text("Fixes:").expect("find Fixes");
    assert_eq!(term.cell(fix_x, fix_y).unwrap().fg, Some(Color::Yellow));

    // 8. Extended commit trailers (Tested: with indented continuation, hyphenated RFC-822 token
    //    Vendor-Bug-Id:, (cherry picked from commit ...), trailing-block promoted token) and
    //    non-trailer prose (Step-1:, UTF-8:) must be rendered with identical cell colors in
    //    BOTH DiffView and LogView.
    let extended_body = [
        "    Step-1: configure the build environment",
        "    UTF-8: supported out of the box",
        "",
        "    Tested: unit and integration suites",
        "      - verified on Linux x86_64",
        "    Vendor-Bug-Id: 98765",
        "    CustomTag: promoted-in-trailer-block",
        "    (cherry picked from commit 0123456789abcdef0123456789abcdef01234567)",
    ]
    .join("\n");

    let mut diff2 = sample_diff();
    diff2.title = Arc::from("feat: extended trailer coverage");
    diff2.body = Some(extended_body);
    let mut log_view = tigrs_ui::view::LogView::new("main");
    log_view.append_commit_diff(&diff2, None);
    let mut log_term = HeadlessTerminal::new(100, 35);
    log_view.render(&mut log_term, 100, 35).unwrap();

    let mut diff_view2 = DiffView::new_with_options(diff2, &opts, None);
    // Place cursor on the commit header row (0) so body/trailer rows are not cursor-inverted
    diff_view2.set_cursor(0, 35);
    let mut diff_term = HeadlessTerminal::new(100, 35);
    diff_view2
        .render_with_capabilities(&mut diff_term, 100, 35, &opts, &caps)
        .unwrap();

    for (label, t) in [("DiffView", &diff_term), ("LogView", &log_term)] {
        // Non-trailer prose with digits in key segment must stay default foreground (None)
        for non_trailer in ["Step-1:", "UTF-8:"] {
            let (nx, ny) = t
                .find_text(non_trailer)
                .unwrap_or_else(|| panic!("{label}: missing {non_trailer}"));
            assert_eq!(
                t.cell(nx, ny).unwrap().fg,
                None,
                "{label}: {non_trailer} in body prose must NOT be highlighted as a trailer"
            );
        }

        // Trailers and indented continuation lines must be Yellow
        for trailer_snippet in [
            "Tested:",
            "- verified on Linux x86_64",
            "Vendor-Bug-Id:",
            "CustomTag:",
            "(cherry picked from commit",
        ] {
            let (tx, ty) = t
                .find_text(trailer_snippet)
                .unwrap_or_else(|| panic!("{label}: missing {trailer_snippet}"));
            let fg = t.cell(tx, ty).unwrap().fg;
            assert!(
                matches!(fg, Some(Color::Yellow | Color::Ansi256(11))),
                "{label}: expected Yellow foreground on trailer snippet {trailer_snippet:?}, got {fg:?}"
            );
        }
    }
}

#[test]
fn test_commit_message_custom_line_color_rules() {
    let mut diff = sample_diff();
    diff.body = Some("    Bug: 12345\n    Signed-off-by: Alice\n    Plain text".to_string());

    let mut opts = ViewOptions::default();
    opts.colors.insert(
        "\"Bug: \"".to_string(),
        tigrs_core::ColorSpec::String("red".to_string()),
    );
    // Custom trailer color override:
    opts.colors.insert(
        "trailer".to_string(),
        tigrs_core::ColorSpec::String("cyan".to_string()),
    );

    let view = DiffView::new_with_options(diff, &opts, None);
    let mut term = HeadlessTerminal::new(100, 30);
    let caps = TerminalCapabilities {
        color_profile: ColorProfile::TrueColor,
        supports_kitty_keyboard: false,
        supports_unicode_box: true,
        supports_synchronized_output: false,
    };
    view.render_with_capabilities(&mut term, 100, 30, &opts, &caps)
        .unwrap();

    // Bug: line should match custom prefix rule and be Red
    let (bug_x, bug_y) = term.find_text("Bug:").expect("find Bug line");
    assert_eq!(term.cell(bug_x, bug_y).unwrap().fg, Some(Color::Red));

    // Trailer should respect overridden trailer color (Cyan)
    let (sob_x, sob_y) = term
        .find_text("Signed-off-by:")
        .expect("find Signed-off-by");
    assert_eq!(term.cell(sob_x, sob_y).unwrap().fg, Some(Color::Cyan));
}

#[test]
fn test_diff_layout_switching_modes_and_aliases() {
    let diff = sample_diff();
    let mut app = AppState::default();
    app.push_view(ViewKind::Diff);
    app.views.diff_view = Some(DiffView::new(diff));

    // Initial default: Unified / single
    assert_eq!(app.options.diff_layout, DiffLayout::Unified);

    // 1. :set diff-layout = side-to-side
    let cmd = tigrs_ui::prompt::ParsedCommand::parse(":set diff-layout = side-to-side");
    tigrs_ui::app::commands::execute_parsed_command(&mut app, cmd, 30);
    assert_eq!(app.options.diff_layout, DiffLayout::SideBySide);
    assert!(
        app.status_message
            .as_ref()
            .unwrap()
            .contains("side-to-side")
    );

    // 2. :set layout = single
    let cmd = tigrs_ui::prompt::ParsedCommand::parse(":set layout = single");
    tigrs_ui::app::commands::execute_parsed_command(&mut app, cmd, 30);
    assert_eq!(app.options.diff_layout, DiffLayout::Unified);
    assert!(app.status_message.as_ref().unwrap().contains("single"));

    // 3. :set diff-mode = side_by_side
    let cmd = tigrs_ui::prompt::ParsedCommand::parse(":set diff-mode = side_by_side");
    tigrs_ui::app::commands::execute_parsed_command(&mut app, cmd, 30);
    assert_eq!(app.options.diff_layout, DiffLayout::SideBySide);
    assert!(
        app.status_message
            .as_ref()
            .unwrap()
            .contains("side-by-side")
    );

    // 3b. :set side-to-side-min-width = 96
    let cmd = tigrs_ui::prompt::ParsedCommand::parse(":set side-to-side-min-width = 96");
    tigrs_ui::app::commands::execute_parsed_command(&mut app, cmd, 30);
    assert_eq!(app.options.side_by_side_min_width, 96);
    assert_eq!(
        app.status_message.as_deref(),
        Some(":set side-to-side-min-width = 96")
    );

    // 4. :toggle single
    let cmd = tigrs_ui::prompt::ParsedCommand::parse(":toggle single");
    tigrs_ui::app::commands::execute_parsed_command(&mut app, cmd, 30);
    assert_eq!(app.options.diff_layout, DiffLayout::Unified);

    // 5. :toggle side-to-side
    let cmd = tigrs_ui::prompt::ParsedCommand::parse(":toggle side-to-side");
    tigrs_ui::app::commands::execute_parsed_command(&mut app, cmd, 30);
    assert_eq!(app.options.diff_layout, DiffLayout::SideBySide);

    // 6. :toggle layout
    let cmd = tigrs_ui::prompt::ParsedCommand::parse(":toggle layout");
    tigrs_ui::app::commands::execute_parsed_command(&mut app, cmd, 30);
    assert_eq!(app.options.diff_layout, DiffLayout::Unified);

    // 7. Verify TOML config parsing
    let cfg = tigrs_core::Config::parse_toml(
        "[view]\ndiff_layout = \"side-to-side\"\nside_by_side_min_width = 92\n",
    )
    .unwrap();
    // The typed config canonicalizes the `side-to-side` alias to `SideBySide`.
    assert_eq!(cfg.view.diff_layout, DiffLayout::SideBySide);
    assert_eq!(cfg.view.side_by_side_min_width, 92);

    let mut opts = ViewOptions::default();
    opts.apply_config(&cfg);
    assert_eq!(opts.diff_layout, DiffLayout::SideBySide);
    assert_eq!(opts.side_by_side_min_width, 92);

    let cfg2 = tigrs_core::Config::parse_toml("[view]\ndiff_layout = \"single\"\n").unwrap();
    // The typed config canonicalizes the `single` alias to `Unified`.
    assert_eq!(cfg2.view.diff_layout, DiffLayout::Unified);

    opts.apply_config(&cfg2);
    assert_eq!(opts.diff_layout, DiffLayout::Unified);

    // 8. Verify 'v' key in Stage scope
    let engine = tigrs_ui::KeymapEngine::default();
    let lookup = engine.lookup(tigrs_ui::KeymapScope::Stage, &[tigrs_ui::Key::from('v')]);
    assert_eq!(
        lookup,
        tigrs_ui::KeymapLookupResult::Match(tigrs_ui::Action::ToggleOption(
            tigrs_ui::OptionId::DiffLayout
        ))
    );
}

// ---------------------------------------------------------------------------
// 10. Production UX Polish: Paired-Row Atomic Staging, Semantic Cursor
//     Preservation, Narrow Fallback Notice, and Truncation Indicator
// ---------------------------------------------------------------------------

#[test]
fn test_side_by_side_paired_row_staging() {
    let diff = sample_diff();
    let opts = ViewOptions {
        diff_layout: DiffLayout::SideBySide,
        ..Default::default()
    };
    let mut view = DiffView::new_with_options(diff, &opts, None);

    // Find the paired row containing both old ("Hello old") and new ("Hello new")
    let paired_row_idx = view
        .document()
        .rows
        .iter()
        .position(|r| {
            r.left
                .as_ref()
                .is_some_and(|c| c.text.contains("Hello old"))
                && r.right
                    .as_ref()
                    .is_some_and(|c| c.text.contains("Hello new"))
        })
        .expect("Should find paired modification row in side-by-side document");

    view.set_cursor(paired_row_idx, 30);

    // Verify selected_lines returns BOTH line indices (deletion + addition)
    let (file, hunk, line_indices) = view
        .selected_lines()
        .expect("Paired row should resolve selected_lines");
    assert_eq!(file.path, "src/main.rs");
    assert_eq!(
        line_indices.len(),
        2,
        "Paired side-by-side row must select both old and new line indices atomically"
    );

    // Synthesize patch from both line indices and verify both - and + lines are included
    let patch = tigrs_git::stage::synthesize_lines_patch(&file.path, hunk, &line_indices)
        .expect("Should synthesize atomic multi-line patch");
    assert!(
        patch.contains("-    let message = \"Hello old\";"),
        "Patch must include deleted line"
    );
    assert!(
        patch.contains("+    let message = \"Hello new\";"),
        "Patch must include added line"
    );
}

#[test]
fn test_diff_layout_cursor_anchor_preservation() {
    let diff = sample_diff();
    let mut opts = ViewOptions {
        diff_layout: DiffLayout::Unified,
        ..Default::default()
    };
    let mut view = DiffView::new_with_options(diff, &opts, None);

    // Position cursor on the added line ("Hello new") in unified mode
    let unified_add_idx = view
        .document()
        .rows
        .iter()
        .position(|r| {
            r.left
                .as_ref()
                .is_some_and(|c| c.text.contains("Hello new"))
        })
        .expect("Should find added line in unified document");

    view.set_cursor(unified_add_idx, 10);
    let (_, _, orig_line_idx) = view.selected_line().unwrap();

    // Switch to SideBySide layout and refresh
    opts.diff_layout = DiffLayout::SideBySide;
    view.refresh(&opts, None);

    // Cursor should map directly to the paired row containing "Hello new"
    let sbs_row = &view.document().rows[view.cursor_index()];
    assert!(
        sbs_row
            .right
            .as_ref()
            .is_some_and(|c| c.text.contains("Hello new")),
        "Cursor after switching to SideBySide should land on the row containing 'Hello new'"
    );
    let (_, _, sbs_indices) = view.selected_lines().unwrap();
    assert!(
        sbs_indices.contains(&orig_line_idx),
        "Selected line indices in SideBySide must include the original line index"
    );

    // Switch back to Unified layout and refresh
    opts.diff_layout = DiffLayout::Unified;
    view.refresh(&opts, None);

    let final_row = &view.document().rows[view.cursor_index()];
    assert!(
        final_row
            .left
            .as_ref()
            .is_some_and(|c| c.text.contains("Hello") || c.text.contains("println")),
        "Cursor after switching back to Unified must remain anchored in the modified hunk"
    );
}

#[test]
fn test_side_by_side_narrow_fallback_preserves_all_lines_and_status() {
    let diff = sample_diff();
    let opts = ViewOptions {
        diff_layout: DiffLayout::SideBySide,
        side_by_side_min_width: 80,
        ..Default::default()
    };
    let view = DiffView::new_with_options(diff, &opts, None);

    // Render into a narrow 78-column terminal (< 80 min width) with height 50 to show all hunks
    let mut term = HeadlessTerminal::new(78, 50);
    view.render_with_options(&mut term, 78, 50, &opts).unwrap();

    let text = screen_text(&term, 50);

    // Pure addition line ("    inserted_middle();") must NOT be blanked out or dropped
    assert!(
        text.contains("inserted_middle()"),
        "Narrow fallback must render unified document and preserve pure addition lines, got:\n{text}"
    );
    // Both old and new lines of the replacement should be visible in unified fallback
    assert!(
        text.contains("Hello old") && text.contains("Hello new"),
        "Narrow fallback must show both deleted and added lines stacked"
    );
    // Status bar must include responsive fallback notice
    let status_row = text.lines().last().unwrap_or("");
    assert!(
        status_row.contains("[unified layout: width 78 < 80 min]"),
        "Status bar must display responsive narrow fallback notice, got: {status_row}"
    );
}

#[test]
fn test_side_by_side_truncation_indicator() {
    let mut diff = sample_diff();
    // Add a very long line to trigger horizontal truncation in side-by-side pane
    diff.files[0].hunks[0].lines.push(HunkLine {
        kind: DiffLineKind::Add,
        content: "    let very_long_variable_name_that_exceeds_pane_width = \"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ\";".to_string(),
        no_newline_at_eof: false,
    });

    let opts = ViewOptions {
        diff_layout: DiffLayout::SideBySide,
        side_by_side_min_width: 80,
        ..Default::default()
    };
    let view = DiffView::new_with_options(diff, &opts, None);

    let mut term = HeadlessTerminal::new(90, 30);
    view.render_with_options(&mut term, 90, 30, &opts).unwrap();

    let text = screen_text(&term, 30);
    // Find the line containing "very_long_variable_name" and check that it ends with '$'
    let truncated_line = text
        .lines()
        .find(|l| l.contains("very_long_variable_name"))
        .expect("Should find long line in rendered side-by-side output");
    assert!(
        truncated_line.trim_end().ends_with('$'),
        "Truncated line in side-by-side pane must display '$' indicator at right edge, got: {truncated_line}"
    );
}

#[test]
fn test_diff_syntax_highlighting_composite_rendering() {
    let diff = sample_diff();
    let mut opts = ViewOptions {
        diff_layout: DiffLayout::Unified,
        syntax_highlighting: true,
        syntax_theme: "Dracula".to_string(),
        ..Default::default()
    };
    let view = DiffView::new_with_options(diff.clone(), &opts, None);

    // Verify that RowCell.syntax_spans are populated for Rust code lines
    let add_row = view
        .document()
        .rows
        .iter()
        .find(|r| {
            r.left
                .as_ref()
                .is_some_and(|c| c.text.contains("Hello new"))
        })
        .expect("Should find added line in diff document");
    let cell = add_row.left.as_ref().unwrap();
    assert!(
        !cell.syntax_spans.is_empty(),
        "Syntax spans must be populated for Rust source code in diff view"
    );

    // Render Unified diff view with syntax highlighting ON and TrueColor capabilities
    let caps = TerminalCapabilities::default();
    let mut term_on = HeadlessTerminal::new(100, 30);
    view.render_with_capabilities(&mut term_on, 100, 30, &opts, &caps)
        .unwrap();

    // Find the row on the terminal grid where "Hello new" was painted
    let grid_row_idx = (0..30)
        .find(|&y| term_on.line_text_raw(y).contains("Hello new"))
        .expect("Should render 'Hello new' on terminal screen");

    // Verify background tinting is preserved on added code cells (Rgb(20, 44, 28))
    let expected_add_bg = Some(Color::Rgb(20, 44, 28));
    let has_tinted_bg = (0..100).any(|x| {
        let c = term_on.cell(x, grid_row_idx).unwrap();
        c.bg == expected_add_bg || c.bg == Some(Color::Rgb(45, 94, 64))
    });
    assert!(
        has_tinted_bg,
        "Added code line with syntax highlighting must retain subtle green diff background tint"
    );

    // Verify foreground token colors differ across tokens (syntax tokens colored by Dracula theme)
    let fgs: std::collections::HashSet<_> = (6..60)
        .filter_map(|x| {
            let c = term_on.cell(x, grid_row_idx).unwrap();
            if c.ch.is_whitespace() { None } else { c.fg }
        })
        .collect();
    assert!(
        fgs.len() >= 2,
        "Syntax highlighting must produce multiple distinct foreground token colors, got: {fgs:?}"
    );

    // Render Side-by-Side diff view with syntax highlighting ON
    opts.diff_layout = DiffLayout::SideBySide;
    let sbs_view = DiffView::new_with_options(diff, &opts, None);
    let mut term_sbs = HeadlessTerminal::new(120, 30);
    sbs_view
        .render_with_capabilities(&mut term_sbs, 120, 30, &opts, &caps)
        .unwrap();
    let sbs_row_idx = (0..30)
        .find(|&y| term_sbs.line_text_raw(y).contains("Hello new"))
        .expect("Should render 'Hello new' in side-by-side pane");
    let has_del_bg = (0..60).any(|x| {
        let c = term_sbs.cell(x, sbs_row_idx).unwrap();
        c.bg == Some(Color::Rgb(54, 24, 30)) || c.bg == Some(Color::Rgb(110, 48, 59))
    });
    let has_add_bg = (60..120).any(|x| {
        let c = term_sbs.cell(x, sbs_row_idx).unwrap();
        c.bg == Some(Color::Rgb(20, 44, 28)) || c.bg == Some(Color::Rgb(45, 94, 64))
    });
    assert!(
        has_del_bg && has_add_bg,
        "Side-by-side panes must render red deletion background tint on left and green addition background tint on right"
    );
}

#[test]
fn test_syntax_highlighting_toggle_and_theme_switching() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use tigrs_ui::view::BlobView;
    use tigrs_ui::{AppState, Flow, ViewKind, handle_event};

    let diff = sample_diff();
    let blob = tigrs_git::BlobContent {
        oid: make_oid(1),
        path: "src/lib.rs".to_string(),
        size: 42,
        is_binary: false,
        lines: vec![
            "pub fn compute(x: i32) -> i32 {".to_string(),
            "    x * 2".to_string(),
            "}".to_string(),
        ]
        .into(),
    };

    let mut app = AppState::default();
    app.push_view(ViewKind::Diff);
    app.views.diff_view = Some(DiffView::new_with_options(
        diff,
        &ViewOptions::default(),
        None,
    ));
    app.views.blob_view = Some(BlobView::new_with_options(
        make_oid(1),
        blob,
        &ViewOptions::default(),
    ));

    assert!(
        app.options.syntax_highlighting,
        "Syntax highlighting defaults to enabled"
    );

    // 1. Press 'S' in DiffView -> toggles syntax highlighting OFF
    let key_s = Event::Key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE));
    assert_eq!(handle_event(&key_s, &mut app, 24), Flow::Continue);
    assert!(!app.options.syntax_highlighting);
    assert_eq!(
        app.status_message.as_deref(),
        Some(":set syntax-highlighting = no")
    );

    // Press 'S' again -> toggles syntax highlighting back ON
    assert_eq!(handle_event(&key_s, &mut app, 24), Flow::Continue);
    assert!(app.options.syntax_highlighting);
    assert_eq!(
        app.status_message.as_deref(),
        Some(":set syntax-highlighting = yes")
    );

    // 2. Press 'T' in DiffView -> cycles syntax theme
    let prev_theme = app.options.syntax_theme.clone();
    let key_t = Event::Key(KeyEvent::new(KeyCode::Char('T'), KeyModifiers::NONE));
    assert_eq!(handle_event(&key_t, &mut app, 24), Flow::Continue);
    assert_ne!(app.options.syntax_theme, prev_theme);
    assert!(
        app.status_message
            .as_deref()
            .unwrap()
            .starts_with(":set syntax-theme = ")
    );

    // 3. Switch to BlobView and verify 'S' and 'T' work there too
    app.push_view(ViewKind::Blob);
    assert_eq!(handle_event(&key_s, &mut app, 24), Flow::Continue);
    assert!(!app.options.syntax_highlighting);
    assert_eq!(
        app.views
            .blob_view
            .as_ref()
            .unwrap()
            .highlighted_lines_len(),
        0,
        "Disabling syntax highlighting clears highlighted lines in BlobView"
    );

    assert_eq!(handle_event(&key_s, &mut app, 24), Flow::Continue);
    assert!(app.options.syntax_highlighting);
    assert!(
        app.views
            .blob_view
            .as_ref()
            .unwrap()
            .highlighted_lines_len()
            > 0,
        "Re-enabling syntax highlighting rebuilds highlighted lines in BlobView"
    );
}

#[test]
fn test_config_and_set_command_syntax_theme() {
    use tigrs_core::config::Config;
    use tigrs_ui::prompt::ParsedCommand;
    use tigrs_ui::{AppState, Flow, execute_parsed_command};

    // 1. Verify TOML config parsing of syntax_highlighting and syntax_theme
    let toml_content = r#"
        [view]
        syntax_highlighting = false
        syntax_theme = "Nord"
    "#;
    let cfg = Config::parse_toml(toml_content).unwrap();
    assert!(!cfg.view.syntax_highlighting);
    assert_eq!(cfg.view.syntax_theme, "Nord");

    let mut opts = ViewOptions::default();
    opts.apply_config(&cfg);
    assert!(!opts.syntax_highlighting);
    assert_eq!(opts.syntax_theme, "Nord");

    // 2. Verify interactive prompt :set commands
    let mut app = AppState::default();

    // :set syntax-theme ? -> lists available themes
    let flow = execute_parsed_command(&mut app, ParsedCommand::parse(":set syntax-theme ?"), 24);
    assert_eq!(flow, Flow::Continue);
    let msg = app.status_message.as_deref().unwrap();
    assert!(
        msg.starts_with("Available syntax themes:")
            && msg.contains("Dracula")
            && msg.contains("Nord"),
        "Theme query must list available themes including Dracula and Nord, got: {msg}"
    );

    // :set syntax-theme = monokai -> resolves alias to Monokai Extended
    execute_parsed_command(
        &mut app,
        ParsedCommand::parse(":set syntax-theme = monokai"),
        24,
    );
    assert_eq!(app.options.syntax_theme, "Monokai Extended");
    assert_eq!(
        app.status_message.as_deref(),
        Some(":set syntax-theme = Monokai Extended")
    );

    // :set syntax-theme = unknown-xyz -> friendly error message
    execute_parsed_command(
        &mut app,
        ParsedCommand::parse(":set syntax-theme = unknown-xyz"),
        24,
    );
    assert!(
        app.status_message
            .as_deref()
            .unwrap()
            .contains("Unknown syntax theme: 'unknown-xyz'")
    );

    // :set syntax-highlighting = off -> sets false
    execute_parsed_command(
        &mut app,
        ParsedCommand::parse(":set syntax-highlighting = off"),
        24,
    );
    assert!(!app.options.syntax_highlighting);
    assert_eq!(
        app.status_message.as_deref(),
        Some(":set syntax-highlighting = no")
    );
}

#[test]
fn test_side_by_side_line_numbers_and_word_diff_emphasis_compositing() {
    let diff = sample_diff();
    let mut opts = ViewOptions {
        diff_layout: DiffLayout::SideBySide,
        line_number: true,
        syntax_highlighting: true,
        syntax_theme: "Dracula".to_string(),
        word_diff: true,
        diff_presentation: DiffPresentation::Fancy,
        ..Default::default()
    };

    let mut view = DiffView::new_with_options(diff, &opts, None);
    let caps = TerminalCapabilities::default();
    let mut term = HeadlessTerminal::new(120, 30);

    // Move cursor to the modified line row and render
    let mod_idx = view
        .document()
        .rows
        .iter()
        .position(|r| {
            r.left
                .as_ref()
                .is_some_and(|c| c.text.contains("Hello old"))
        })
        .unwrap();
    view.set_cursor(mod_idx, 30);
    view.render_with_capabilities(&mut term, 120, 30, &opts, &caps)
        .unwrap();

    // Move cursor off the modified line row and render again to test non-cursor gutter and emphasis compositing
    view.set_cursor(0, 30);
    let mut term2 = HeadlessTerminal::new(120, 30);
    view.render_with_capabilities(&mut term2, 120, 30, &opts, &caps)
        .unwrap();

    let row_y = (0..30)
        .find(|&y| term2.line_text_raw(y).contains("Hello new"))
        .expect("Modified line should be rendered");

    // Verify bright green word-diff emphasis background tint (Rgb(45, 94, 64)) is applied to "new"
    let has_emph_add_bg = (60..120).any(|x| {
        let c = term2.cell(x, row_y).unwrap();
        c.bg == Some(Color::Rgb(45, 94, 64))
    });
    assert!(
        has_emph_add_bg,
        "Word-diff emphasis inside syntax-highlighted addition must render bright green emphasis background tint"
    );

    // Verify bright red word-diff emphasis background tint (Rgb(110, 48, 59)) is applied to "old"
    let has_emph_del_bg = (0..60).any(|x| {
        let c = term2.cell(x, row_y).unwrap();
        c.bg == Some(Color::Rgb(110, 48, 59))
    });
    assert!(
        has_emph_del_bg,
        "Word-diff emphasis inside syntax-highlighted deletion must render bright red emphasis background tint"
    );

    // Also test Unified mode with syntax_highlighting = false and word_diff = true
    opts.diff_layout = DiffLayout::Unified;
    opts.syntax_highlighting = false;
    let view_no_syn = DiffView::new_with_options(sample_diff(), &opts, None);
    let mut term3 = HeadlessTerminal::new(100, 30);
    view_no_syn
        .render_with_capabilities(&mut term3, 100, 30, &opts, &caps)
        .unwrap();
}

#[test]
fn test_prompt_set_and_toggle_commands_comprehensive_edge_cases() {
    use tigrs_ui::prompt::ParsedCommand;
    use tigrs_ui::{AppState, execute_parsed_command};

    let mut app = AppState::default();

    // :set mouse = yes / no / invalid
    execute_parsed_command(&mut app, ParsedCommand::parse(":set mouse = yes"), 24);
    assert!(app.options.mouse);
    execute_parsed_command(&mut app, ParsedCommand::parse(":set mouse = no"), 24);
    assert!(!app.options.mouse);
    execute_parsed_command(&mut app, ParsedCommand::parse(":set mouse = invalid"), 24);
    assert!(
        app.status_message
            .as_deref()
            .unwrap()
            .contains("Invalid value for 'mouse'")
    );

    // :set side-by-side = yes / no and :set unified = yes / no
    execute_parsed_command(
        &mut app,
        ParsedCommand::parse(":set side-by-side = yes"),
        24,
    );
    assert_eq!(app.options.diff_layout, DiffLayout::SideBySide);
    execute_parsed_command(&mut app, ParsedCommand::parse(":set side-by-side = no"), 24);
    assert_eq!(app.options.diff_layout, DiffLayout::Unified);
    execute_parsed_command(&mut app, ParsedCommand::parse(":set unified = no"), 24);
    assert_eq!(app.options.diff_layout, DiffLayout::SideBySide);
    execute_parsed_command(&mut app, ParsedCommand::parse(":set unified = yes"), 24);
    assert_eq!(app.options.diff_layout, DiffLayout::Unified);

    // :set diff-layout = invalid
    execute_parsed_command(
        &mut app,
        ParsedCommand::parse(":set diff-layout = invalid_mode"),
        24,
    );
    assert!(
        app.status_message
            .as_deref()
            .unwrap()
            .contains("Invalid value for 'diff-layout'")
    );

    // :set side-by-side-min-width = invalid
    execute_parsed_command(
        &mut app,
        ParsedCommand::parse(":set side-by-side-min-width = not_a_num"),
        24,
    );
    assert!(
        app.status_message
            .as_deref()
            .unwrap()
            .contains("expected positive integer")
    );

    // :set syntax-highlighting = invalid
    execute_parsed_command(
        &mut app,
        ParsedCommand::parse(":set syntax-highlighting = maybe"),
        24,
    );
    assert!(
        app.status_message
            .as_deref()
            .unwrap()
            .contains("Invalid value for 'syntax-highlighting'")
    );

    // :toggle commands
    execute_parsed_command(
        &mut app,
        ParsedCommand::parse(":toggle syntax-highlighting"),
        24,
    );
    execute_parsed_command(&mut app, ParsedCommand::parse(":toggle syntax-theme"), 24);
    execute_parsed_command(&mut app, ParsedCommand::parse(":toggle diff-layout"), 24);
    execute_parsed_command(&mut app, ParsedCommand::parse(":toggle line-number"), 24);
    execute_parsed_command(&mut app, ParsedCommand::parse(":toggle mouse"), 24);
    execute_parsed_command(&mut app, ParsedCommand::parse(":toggle unknown-opt"), 24);
    assert!(
        app.status_message
            .as_deref()
            .unwrap()
            .contains("Unknown option")
    );

    // :goto invalid revision
    execute_parsed_command(&mut app, ParsedCommand::parse(":deadbeefcafebabe"), 24);
    assert!(
        app.status_message
            .as_deref()
            .unwrap()
            .contains("Cannot resolve revision")
    );
}

#[test]
fn test_external_formatter_error_handling_and_edge_cases() {
    let diff = sample_diff();
    let caps = TerminalCapabilities {
        color_profile: ColorProfile::TrueColor,
        supports_kitty_keyboard: false,
        supports_unicode_box: true,
        supports_synchronized_output: false,
    };

    // 1. Empty command returns error
    let res_empty = tigrs_ui::diff::run_external_formatter(&diff, "   ", 80, &caps);
    assert!(res_empty.is_err());

    // 2. Non-zero exit code returns error
    let res_fail = tigrs_ui::diff::run_external_formatter(&diff, "exit 1", 80, &caps);
    assert!(res_fail.is_err());

    // 3. Valid command (`cat`) returns DiffDocument
    let doc = tigrs_ui::diff::run_external_formatter(&diff, "cat", 80, &caps).unwrap();
    assert!(!doc.rows.is_empty());
}

#[test]
fn test_e2e_real_git_repo_commit_messages_in_diff_and_log_views() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();

    let run_git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(path)
            .status()
            .expect("git command");
        assert!(status.success(), "git {args:?} failed");
    };

    run_git(&["init", "-b", "main"]);
    run_git(&["config", "user.name", "Alice Developer"]);
    run_git(&["config", "user.email", "alice@example.com"]);

    let full_commit_msg = "\
net: socket: make receive ring buffer capacity configurable

Previously the ring buffer used a fixed array of 4096 bytes,
which could overflow under high packet burst loads.

SKIP_CI: Flaky integration suite on arm64 runner
Manual verification complete
Tested: on local testbed https://ci.example.com/runs/10492
with benchmark profile https://git.example.com/perf/profiles/42
Issue-Id: 100200
Issue-Id: 100201
Change-Id: I1234567890abcdef1234567890abcdef12345678
Signed-off-by: Alice Developer <alice@example.com>";

    std::fs::write(path.join("ringbuf.c"), "int ring_buf_size = 4096;\n").unwrap();
    run_git(&["add", "ringbuf.c"]);
    run_git(&["commit", "--cleanup=verbatim", "-m", full_commit_msg]);

    // Load real CommitDiff through GitEngine (no synthetic struct mocking)
    let engine = tigrs_git::GitEngine::open(Some(path)).expect("open repo");
    let head_id = engine.head_commit_id().expect("resolve HEAD");
    let diff = engine.compute_commit_diff(head_id).expect("compute diff");

    let expected_lines = [
        "net: socket: make receive ring buffer capacity configurable",
        "Previously the ring buffer used a fixed array of 4096 bytes,",
        "which could overflow under high packet burst loads.",
        "SKIP_CI: Flaky integration suite on arm64 runner",
        "Manual verification complete",
        "Tested: on local testbed https://ci.example.com/runs/10492",
        "with benchmark profile https://git.example.com/perf/profiles/42",
        "Issue-Id: 100200",
        "Issue-Id: 100201",
        "Change-Id: I1234567890abcdef1234567890abcdef12345678",
        "Signed-off-by: Alice Developer <alice@example.com>",
    ];

    let caps = TerminalCapabilities {
        color_profile: ColorProfile::TrueColor,
        supports_kitty_keyboard: false,
        supports_unicode_box: true,
        supports_synchronized_output: false,
    };

    // 1. Verify DiffView across Plain, Fancy, and SideBySide presentations
    for (presentation, layout) in [
        (DiffPresentation::Classic, DiffLayout::Unified),
        (DiffPresentation::Fancy, DiffLayout::Unified),
        (DiffPresentation::Fancy, DiffLayout::SideBySide),
    ] {
        let opts = ViewOptions {
            diff_presentation: presentation,
            diff_layout: layout,
            ..Default::default()
        };

        let mut diff_view = DiffView::new_with_options(diff.clone(), &opts, None);
        diff_view.set_cursor(22, 40);
        let mut term = HeadlessTerminal::new(120, 40);
        diff_view
            .render_with_capabilities(&mut term, 120, 40, &opts, &caps)
            .expect("render DiffView");

        let screen = term.screen_text();
        for expected in &expected_lines {
            assert!(
                screen.contains(expected),
                "DiffView ({presentation:?}, {layout:?}) missing commit message line: '{expected}'\nScreen:\n{screen}"
            );
        }

        // Verify trailer coloring (Yellow) on real parsed commit trailers
        let (sob_x, sob_y) = term
            .find_text("Signed-off-by:")
            .expect("find Signed-off-by in DiffView");
        assert_eq!(term.cell(sob_x, sob_y).unwrap().fg, Some(Color::Yellow));
    }

    // 2. Verify LogView renders the complete commit message and diffstat
    let mut log_view = tigrs_ui::view::log_view::LogView::new("main");
    log_view.append_commit_diff(&diff, None);
    let mut log_term = HeadlessTerminal::new(120, 40);
    let opts = ViewOptions::default();
    tigrs_ui::view::View::render(&log_view, &mut log_term, 120, 40, &opts).expect("render LogView");

    let log_screen = log_term.screen_text();
    for expected in &expected_lines {
        assert!(
            log_screen.contains(expected),
            "LogView missing commit message line: '{expected}'\nScreen:\n{log_screen}"
        );
    }
}

#[test]
fn test_line_number_in_diff_view() {
    let diff = sample_diff();
    let caps = TerminalCapabilities {
        color_profile: ColorProfile::TrueColor,
        supports_kitty_keyboard: true,
        supports_unicode_box: true,
        supports_synchronized_output: true,
    };

    // 1. Verify `line_number = true` only renders source file line numbers on diff hunk lines,
    // not on commit headers or MainView rows.
    let opts = ViewOptions {
        line_number: true,
        ..Default::default()
    };
    let diff_view = DiffView::new_with_options(diff.clone(), &opts, None);
    let mut term = HeadlessTerminal::new(100, 35);
    diff_view
        .render_with_capabilities(&mut term, 100, 35, &opts, &caps)
        .expect("render diff view with line numbers");

    assert!(
        term.line_text(1).starts_with("commit "),
        "Expected header row 1 to not have a synthetic row number, got: {}",
        term.line_text(1)
    );
    let hunk_row_y = (0..35)
        .find(|&y| {
            let l = term.line_text_raw(y);
            l.starts_with("   1 ") || l.starts_with("  10 ") || l.starts_with("  11 ")
        })
        .expect("Expected source code diff hunk lines to start with source file line numbers");

    // Verify the line-number gutter cell uses terminal default foreground (fg == None)
    // with SGR DIM attribute so it remains legible on both dark and light terminal themes.
    let digit_cell = (0..4)
        .filter_map(|x| term.cell(x, hunk_row_y))
        .find(|c| c.ch.is_ascii_digit())
        .expect("Expected digit cell in line number gutter");
    assert_eq!(
        digit_cell.fg, None,
        "Line number gutter should preserve terminal default foreground color (fg == None)"
    );
    assert!(
        digit_cell
            .attrs
            .contains(tigrs_ui::headless::CellAttrs::DIM),
        "Line number gutter should carry CellAttrs::DIM for theme-adaptive contrast"
    );

    // 2. Verify `config.toml` `line_number = true` (or `number = true` / `nu = true`) applies to ViewOptions
    let cfg = tigrs_core::Config::parse_toml(
        r"
        [view]
        number = true
        ",
    )
    .expect("parse config");
    let mut applied_opts = ViewOptions::default();
    assert!(!applied_opts.line_number);
    applied_opts.apply_config(&cfg);
    assert!(applied_opts.line_number);

    // 3. Verify `:set number` / `:set nu` / `:set nonumber` / `:set nonu` in DiffView
    let mut app = AppState::default();
    app.push_view(ViewKind::Diff);
    app.views.diff_view = Some(DiffView::new(diff));
    assert!(!app.options.line_number);

    for ch in ":set nu".chars() {
        handle_event_with_dimensions(&key(KeyCode::Char(ch)), &mut app, 100, 25);
    }
    handle_event_with_dimensions(&key(KeyCode::Enter), &mut app, 100, 25);
    assert!(app.options.line_number);

    for ch in ":set nonu".chars() {
        handle_event_with_dimensions(&key(KeyCode::Char(ch)), &mut app, 100, 25);
    }
    handle_event_with_dimensions(&key(KeyCode::Enter), &mut app, 100, 25);
    assert!(!app.options.line_number);
}

#[test]
fn test_gerrit_style_side_by_side_diff_row_and_word_highlighting_without_signs() {
    let diff = sample_diff();
    let caps = TerminalCapabilities {
        color_profile: ColorProfile::TrueColor,
        supports_kitty_keyboard: true,
        supports_unicode_box: true,
        supports_synchronized_output: true,
    };

    // Default Classic presentation + SideBySide layout (with word_diff not manually toggled)
    let opts = ViewOptions {
        diff_layout: DiffLayout::SideBySide,
        line_number: true,
        ..Default::default()
    };
    let view = DiffView::new_with_options(diff, &opts, None);
    let mut term = HeadlessTerminal::new(120, 35);
    view.render_with_capabilities(&mut term, 120, 35, &opts, &caps)
        .expect("render Gerrit-style side-by-side diff");

    // 1. Find the paired modification row ("Hello old" on left, "Hello new" on right)
    let paired_y = (0..35)
        .find(|&y| {
            let line = term.line_text_raw(y);
            line.contains("Hello old") && line.contains("Hello new")
        })
        .expect("Should find paired modification row in side-by-side diff");

    let row_str = term.line_text_raw(paired_y);
    let parts: Vec<&str> = row_str.split('│').collect();
    assert_eq!(
        parts.len(),
        2,
        "Row must have left and right panes separated by │"
    );
    // Neither pane should contain a leading '-' or '+' diff marker after the 5-column line number gutter
    let left_after_gutter = &parts[0][5..];
    let right_after_gutter = &parts[1][5..];
    assert!(
        !left_after_gutter.starts_with('-'),
        "Left side-by-side pane must not use '-' sign marker, got: '{left_after_gutter}'"
    );
    assert!(
        !right_after_gutter.starts_with('+'),
        "Right side-by-side pane must not use '+' sign marker, got: '{right_after_gutter}'"
    );

    // 2. Verify two-tier Gerrit row + intra-line word background colors on left and right panes
    let sep_x = (0..120)
        .find(|&x| term.cell(x, paired_y).unwrap().ch == '│')
        .unwrap();

    // Left pane must have both base deletion row bg Rgb(54, 24, 30) AND intra-line emphasis bg Rgb(110, 48, 59)
    let left_has_base_bg =
        (0..sep_x).any(|x| term.cell(x, paired_y).unwrap().bg == Some(Color::Rgb(54, 24, 30)));
    let left_has_emph_bg =
        (0..sep_x).any(|x| term.cell(x, paired_y).unwrap().bg == Some(Color::Rgb(110, 48, 59)));
    assert!(
        left_has_base_bg && left_has_emph_bg,
        "Left pane must highlight whole row with base red bg and edited word with stronger red bg"
    );

    // Right pane must have both base addition row bg Rgb(20, 44, 28) AND intra-line emphasis bg Rgb(45, 94, 64)
    let right_has_base_bg = ((sep_x + 1)..120)
        .any(|x| term.cell(x, paired_y).unwrap().bg == Some(Color::Rgb(20, 44, 28)));
    let right_has_emph_bg = ((sep_x + 1)..120)
        .any(|x| term.cell(x, paired_y).unwrap().bg == Some(Color::Rgb(45, 94, 64)));
    assert!(
        right_has_base_bg && right_has_emph_bg,
        "Right pane must highlight whole row with base green bg and edited word with stronger green bg"
    );

    // 3. Verify unpaired insertion row shades the empty left filler half-row with filler background Rgb(30, 32, 42)
    let inserted_y = (0..35)
        .find(|&y| term.line_text_raw(y).contains("inserted_middle()"))
        .expect("Should find unpaired insertion row");
    let left_filler_shaded =
        (0..sep_x).all(|x| term.cell(x, inserted_y).unwrap().bg == Some(Color::Rgb(30, 32, 42)));
    assert!(
        left_filler_shaded,
        "Unpaired insertion row must shade empty opposite left pane with muted filler background"
    );
}

#[test]
fn test_diff_view_syntax_highlighting_preserved_at_max_context() {
    use std::sync::Arc;

    // Build a 350-line Rust file where the modified hunk is at line 280 (well beyond the 200-line
    // MAX_DIFF_HIGHLIGHT_TOTAL_LINES threshold).
    let blob_lines: Vec<Arc<str>> = (1..=350)
        .map(|i| {
            if i == 280 {
                Arc::from("    let updated_value: usize = compute_new_score(280);")
            } else {
                Arc::from(format!("    let item_{i}: usize = {i};").as_str())
            }
        })
        .collect();
    let blob_arc: Arc<[Arc<str>]> = Arc::from(blob_lines);

    let new_blob_oid = gix::ObjectId::from_bytes_or_panic(&[0x42; 20]);
    let old_blob_oid = gix::ObjectId::from_bytes_or_panic(&[0x41; 20]);

    let diff = CommitDiff {
        commit_id: gix::ObjectId::from_bytes_or_panic(&[0x99; 20]),
        parent_ids: vec![old_blob_oid],
        author_name: "Alice Developer".into(),
        author_email: "alice@example.com".into(),
        author_date: "2026-09-22".to_string(),
        committer_name: "Alice Developer".into(),
        committer_email: "alice@example.com".into(),
        committer_date: "2026-09-22".to_string(),
        title: "Update score calculation past line 200".into(),
        body: None,
        stats: DiffSummaryStats {
            files_changed: 1,
            insertions: 1,
            deletions: 1,
        },
        files: vec![FileDiff {
            path: "src/engine.rs".to_string(),
            old_id: Some(old_blob_oid),
            new_id: Some(new_blob_oid),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_644),
            status: FileChangeStatus::Modified,
            is_binary: false,
            additions: 1,
            deletions: 1,
            hunks: vec![DiffHunk {
                old_start: 279,
                old_len: 3,
                new_start: 279,
                new_len: 3,
                func_context: Some("fn process_items()".to_string()),
                lines: vec![
                    HunkLine {
                        kind: DiffLineKind::Context,
                        content: "    let item_279: usize = 279;".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Remove,
                        content: "    let item_280: usize = 280;".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Add,
                        content: "    let updated_value: usize = compute_new_score(280);"
                            .to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Context,
                        content: "    let item_281: usize = 281;".to_string(),
                        no_newline_at_eof: false,
                    },
                ],
            }],
        }],
    };

    let provider = |oid: tigrs_git::ObjectId| -> Option<Arc<[Arc<str>]>> {
        if oid == new_blob_oid {
            Some(Arc::clone(&blob_arc))
        } else {
            None
        }
    };

    // 1. Unified layout at max context (DIFF_CONTEXT_FULL)
    let mut full_unified_opts = ViewOptions {
        syntax_highlighting: true,
        diff_context: ViewOptions::DIFF_CONTEXT_FULL,
        diff_layout: DiffLayout::Unified,
        ..ViewOptions::default()
    };
    let unified_view =
        DiffView::new_with_options(diff.clone(), &full_unified_opts, Some(&provider));
    let unified_doc = unified_view.document();

    // Every code row (context, deletion, addition) across all 350 lines must have non-empty syntax_spans
    let code_rows: Vec<_> = unified_doc
        .rows
        .iter()
        .filter(|r| {
            matches!(
                r.row_type,
                DiffLineType::DiffContext | DiffLineType::DiffAdd | DiffLineType::DiffDel
            )
        })
        .collect();
    assert_eq!(
        code_rows.len(),
        351,
        "Full context unified diff of 350-line file with 1 del + 1 add must have 351 code rows"
    );
    for (idx, row) in code_rows.iter().enumerate() {
        let cell = row
            .left
            .as_ref()
            .expect("Unified code row must have left cell");
        assert!(
            !cell.syntax_spans.is_empty(),
            "Unified row {idx} ('{}') lost syntax highlighting at max context",
            cell.text
        );
    }

    // 2. Side-by-Side layout at max context (DIFF_CONTEXT_FULL)
    full_unified_opts.diff_layout = DiffLayout::SideBySide;
    let sbs_view = DiffView::new_with_options(diff, &full_unified_opts, Some(&provider));
    let sbs_doc = sbs_view.document();
    let sbs_code_rows: Vec<_> = sbs_doc
        .rows
        .iter()
        .filter(|r| {
            matches!(
                r.row_type,
                DiffLineType::DiffContext | DiffLineType::DiffAdd | DiffLineType::DiffDel
            )
        })
        .collect();
    assert_eq!(
        sbs_code_rows.len(),
        350,
        "Full context side-by-side diff of 350-line file with 1 paired modification must have 350 code rows"
    );
    for (idx, row) in sbs_code_rows.iter().enumerate() {
        if let Some(ref left) = row.left {
            assert!(
                !left.syntax_spans.is_empty(),
                "Side-by-side left cell at row {idx} ('{}') lost syntax highlighting at max context",
                left.text
            );
        }
        if let Some(ref right) = row.right {
            assert!(
                !right.syntax_spans.is_empty(),
                "Side-by-side right cell at row {idx} ('{}') lost syntax highlighting at max context",
                right.text
            );
        }
    }
}

#[test]
fn test_side_by_side_large_line_numbers_column_alignment() {
    // Regression test for side-by-side right-pane column misalignment when a diff touches
    // files with >4-digit line numbers (e.g., 6-digit line numbers 411_707..523_006) alongside
    // files with smaller line numbers (e.g., 3-digit line numbers 408..415), and mixes
    // Context (Some, Some), unpaired Added (None, Some), unpaired Deleted (Some, None),
    // and Paired (Some, Some) modification rows.
    let diff = CommitDiff {
        commit_id: make_oid(0x88),
        parent_ids: vec![make_oid(0x87)],
        author_name: Arc::from("Alice Developer"),
        author_email: Arc::from("alice@example.com"),
        author_date: "Mon Sep 14 12:00:00 2026 +0000".to_string(),
        committer_name: Arc::from("Alice Developer"),
        committer_email: Arc::from("alice@example.com"),
        committer_date: "Mon Sep 14 12:00:00 2026 +0000".to_string(),
        title: Arc::from("drm: update register table entries"),
        body: None,
        stats: DiffSummaryStats {
            files_changed: 2,
            insertions: 6,
            deletions: 2,
        },
        files: vec![
            FileDiff {
                status: FileChangeStatus::Modified,
                path: "drivers/gpu/drm/register_table.c".to_string(),
                old_mode: Some(0o100_644),
                new_mode: Some(0o100_644),
                old_id: Some(make_oid(0x81)),
                new_id: Some(make_oid(0x82)),
                is_binary: false,
                additions: 5,
                deletions: 2,
                hunks: vec![DiffHunk {
                    old_start: 411_707,
                    old_len: 6,
                    new_start: 411_707,
                    new_len: 9,
                    func_context: Some("static const struct reg_entry table[] = {".to_string()),
                    lines: vec![
                        // Context row (both left and right have 6-digit line numbers: 411707)
                        HunkLine {
                            kind: DiffLineKind::Context,
                            content: "    REG_ENTRY(0x1000, 0x00),".to_string(),
                            no_newline_at_eof: false,
                        },
                        // Paired modification row (left: 411708, right: 411708)
                        HunkLine {
                            kind: DiffLineKind::Remove,
                            content: "    REG_ENTRY(0x1004, 0x01),".to_string(),
                            no_newline_at_eof: false,
                        },
                        HunkLine {
                            kind: DiffLineKind::Add,
                            content: "    REG_ENTRY(0x1004, 0x02),".to_string(),
                            no_newline_at_eof: false,
                        },
                        // Unpaired additions (left is None, right has 6-digit line numbers: 411709..411711)
                        HunkLine {
                            kind: DiffLineKind::Add,
                            content: "    REG_ENTRY(0x1008, 0x03),".to_string(),
                            no_newline_at_eof: false,
                        },
                        HunkLine {
                            kind: DiffLineKind::Add,
                            content: "    REG_ENTRY(0x100C, 0x04),".to_string(),
                            no_newline_at_eof: false,
                        },
                        HunkLine {
                            kind: DiffLineKind::Add,
                            content: "    REG_ENTRY(0x1010, 0x05),".to_string(),
                            no_newline_at_eof: false,
                        },
                        // Context row after additions
                        HunkLine {
                            kind: DiffLineKind::Context,
                            content: "    REG_ENTRY(0x1014, 0x06),".to_string(),
                            no_newline_at_eof: false,
                        },
                        // Unpaired deletion (left has 6-digit line number, right is None)
                        HunkLine {
                            kind: DiffLineKind::Remove,
                            content: "    REG_ENTRY(0x1018, 0xFF),".to_string(),
                            no_newline_at_eof: false,
                        },
                        // Trailing context row
                        HunkLine {
                            kind: DiffLineKind::Context,
                            content: "    REG_ENTRY(0x101C, 0x07),".to_string(),
                            no_newline_at_eof: false,
                        },
                    ],
                }],
            },
            FileDiff {
                status: FileChangeStatus::Modified,
                path: "drivers/gpu/drm/device_init.c".to_string(),
                old_mode: Some(0o100_644),
                new_mode: Some(0o100_644),
                old_id: Some(make_oid(0x83)),
                new_id: Some(make_oid(0x84)),
                is_binary: false,
                additions: 1,
                deletions: 0,
                hunks: vec![DiffHunk {
                    old_start: 408,
                    old_len: 2,
                    new_start: 408,
                    new_len: 3,
                    func_context: Some("int drm_device_init(void)".to_string()),
                    lines: vec![
                        HunkLine {
                            kind: DiffLineKind::Context,
                            content: "    REG_ENTRY(0x2000, 0x10),".to_string(),
                            no_newline_at_eof: false,
                        },
                        HunkLine {
                            kind: DiffLineKind::Add,
                            content: "    REG_ENTRY(0x2004, 0x11),".to_string(),
                            no_newline_at_eof: false,
                        },
                        HunkLine {
                            kind: DiffLineKind::Context,
                            content: "    REG_ENTRY(0x2008, 0x12),".to_string(),
                            no_newline_at_eof: false,
                        },
                    ],
                }],
            },
        ],
    };

    let opts = ViewOptions {
        diff_layout: DiffLayout::SideBySide,
        line_number: true,
        side_by_side_min_width: 80,
        ..Default::default()
    };

    let view = DiffView::new_with_options(diff, &opts, None);
    assert_eq!(
        view.document().max_lineno_digits,
        6,
        "DiffDocument must record max_lineno_digits = 6 for 411707..411715"
    );

    let width: usize = 120;
    let height: usize = 45;
    let mut term = HeadlessTerminal::new(width as u16, height as u16);
    let caps = TerminalCapabilities {
        color_profile: ColorProfile::TrueColor,
        supports_kitty_keyboard: false,
        supports_unicode_box: true,
        supports_synchronized_output: false,
    };
    view.render_with_capabilities(&mut term, width as u16, height as u16, &opts, &caps)
        .unwrap();

    // Collect every rendered side-by-side row that contains a center separator '│'
    let mut separator_cols = Vec::new();
    let mut right_code_cols = Vec::new();

    for y in 0..(height - 1) {
        let line: String = (0..width).map(|x| term.cell(x, y).unwrap().ch).collect();
        if let Some((sep_x, _)) = (0..width)
            .map(|x| (x, term.cell(x, y).unwrap().ch))
            .find(|&(_, ch)| ch == '│')
        {
            separator_cols.push((y, sep_x, line.clone()));

            // If this row has code on the right side ("REG_ENTRY("), find the column where
            // "    REG_ENTRY(" starts in the right pane (after sep_x)
            let right_part: String = (sep_x + 1..width)
                .map(|x| term.cell(x, y).unwrap().ch)
                .collect();
            if let Some(rel_idx) = right_part.find("    REG_ENTRY(") {
                let abs_col = sep_x + 1 + rel_idx;
                right_code_cols.push((y, abs_col, line));
            }
        }
    }

    assert!(
        separator_cols.len() >= 10,
        "Expected at least 10 side-by-side rows with center separator '│', got {}",
        separator_cols.len()
    );
    assert!(
        right_code_cols.len() >= 9,
        "Expected at least 9 right-pane code rows containing '    REG_ENTRY(', got {}",
        right_code_cols.len()
    );

    let first_sep_col = separator_cols[0].1;
    for (y, col, line) in &separator_cols {
        assert_eq!(
            *col, first_sep_col,
            "Center separator '│' misaligned at row {y}: expected column {first_sep_col}, got {col} in line:\n{line}"
        );
    }

    let first_code_col = right_code_cols[0].1;
    for (y, col, line) in &right_code_cols {
        assert_eq!(
            *col, first_code_col,
            "Right-pane code start column misaligned at row {y}: expected column {first_code_col}, got {col} in line:\n{line}"
        );
    }
}

// ---------------------------------------------------------------------------
// 10. Tier 1 Diff & Code Review UX: File Folding, Soft Line Wrapping,
//     Moved-Block Highlighting, and Hunk-by-Hunk Context Expansion
// ---------------------------------------------------------------------------

#[test]
fn test_tier1_file_folding_and_diffstat_jump() {
    let diff = sample_diff();
    let mut app = AppState::default();
    app.push_view(ViewKind::Diff);
    app.views.diff_view = Some(DiffView::new(diff));

    let full_row_count = app.views.diff_view.as_ref().unwrap().line_count();

    // 1. Fold all files with 'zM'
    handle_event_with_dimensions(&key(KeyCode::Char('z')), &mut app, 120, 30);
    handle_event_with_dimensions(&key(KeyCode::Char('M')), &mut app, 120, 30);
    let folded_row_count = app.views.diff_view.as_ref().unwrap().line_count();
    assert!(
        folded_row_count < full_row_count,
        "Folding all files (zM) should reduce visible row count ({folded_row_count} < {full_row_count})"
    );
    assert_eq!(
        app.views.diff_view.as_ref().unwrap().folded_file_count(),
        1,
        "Expected 1 folded file after zM"
    );

    // Check that the FileHeader row shows the folded summary badge
    let file_header_row = app
        .views
        .diff_view
        .as_ref()
        .unwrap()
        .document()
        .rows
        .iter()
        .find(|r| r.row_type == DiffLineType::FileHeader)
        .expect("FileHeader row must remain visible when folded");
    let header_text = &file_header_row.left.as_ref().unwrap().text;
    assert!(
        header_text.contains("▸") && header_text.contains("[folded:"),
        "Folded file header must include '▸' and '[folded: ...]' badge, got: {header_text}"
    );

    // 2. Move cursor to the StatFile row in diffstat and press Enter:
    //    Should unfold the file and jump cursor directly to its FileHeader row!
    let stat_idx = app
        .views
        .diff_view
        .as_ref()
        .unwrap()
        .document()
        .rows
        .iter()
        .position(|r| r.row_type == DiffLineType::StatFile)
        .expect("Diffstat StatFile row must exist");
    app.views
        .diff_view
        .as_mut()
        .unwrap()
        .set_cursor(stat_idx, 30);

    handle_event_with_dimensions(&key(KeyCode::Enter), &mut app, 120, 30);
    assert_eq!(
        app.views.diff_view.as_ref().unwrap().folded_file_count(),
        0,
        "Pressing Enter on a folded file's diffstat row must unfold that file"
    );
    let cursor_after_jump = app.views.diff_view.as_ref().unwrap().cursor_index();
    let jumped_row_type =
        app.views.diff_view.as_ref().unwrap().document().rows[cursor_after_jump].row_type;
    assert_eq!(
        jumped_row_type,
        DiffLineType::FileHeader,
        "Pressing Enter on diffstat row must jump cursor to FileHeader"
    );

    // 3. Toggle fold on the current file with 'za'
    handle_event_with_dimensions(&key(KeyCode::Char('z')), &mut app, 120, 30);
    handle_event_with_dimensions(&key(KeyCode::Char('a')), &mut app, 120, 30);
    assert_eq!(
        app.views.diff_view.as_ref().unwrap().folded_file_count(),
        1,
        "Pressing za on FileHeader should fold the file"
    );

    // 4. Unfold all files with 'zR'
    handle_event_with_dimensions(&key(KeyCode::Char('z')), &mut app, 120, 30);
    handle_event_with_dimensions(&key(KeyCode::Char('R')), &mut app, 120, 30);
    assert_eq!(
        app.views.diff_view.as_ref().unwrap().folded_file_count(),
        0,
        "Pressing zR should unfold all files"
    );
    assert_eq!(
        app.views.diff_view.as_ref().unwrap().line_count(),
        full_row_count
    );
}

#[test]
fn test_tier1_soft_line_wrapping_and_horizontal_overflow_markers() {
    let oid = make_oid(0x44);
    let long_line = "let very_long_identifier_name = compute_extremely_detailed_configuration_value_with_arguments(alpha, beta, gamma, delta);";
    let diff = CommitDiff {
        commit_id: oid,
        parent_ids: vec![make_oid(0x33)],
        author_name: Arc::from("Alice Developer"),
        author_email: Arc::from("alice@example.com"),
        author_date: "Mon Sep 14 12:00:00 2026 +0000".to_string(),
        committer_name: Arc::from("Alice Developer"),
        committer_email: Arc::from("alice@example.com"),
        committer_date: "Mon Sep 14 12:00:00 2026 +0000".to_string(),
        title: Arc::from("Test soft line wrapping and horizontal scroll markers"),
        body: None,
        files: vec![FileDiff {
            status: FileChangeStatus::Modified,
            path: "src/long.rs".to_string(),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_644),
            is_binary: false,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_len: 0,
                new_start: 1,
                new_len: 1,
                func_context: None,
                lines: vec![HunkLine {
                    kind: DiffLineKind::Add,
                    content: long_line.to_string(),
                    no_newline_at_eof: false,
                }],
            }],
            old_id: Some(oid),
            new_id: Some(oid),
            additions: 1,
            deletions: 0,
        }],
        stats: DiffSummaryStats {
            files_changed: 1,
            insertions: 1,
            deletions: 0,
        },
    };

    let caps = TerminalCapabilities {
        color_profile: ColorProfile::TrueColor,
        supports_kitty_keyboard: false,
        supports_unicode_box: true,
        supports_synchronized_output: false,
    };

    // 1. Test horizontal scroll left-overflow marker '‹' when wrap_lines = false and col_offset > 0
    let mut opts = ViewOptions {
        wrap_lines: false,
        ..Default::default()
    };
    let mut view = DiffView::new_with_options(diff, &opts, None);
    let add_row_idx = view
        .document()
        .rows
        .iter()
        .position(|r| r.row_type == DiffLineType::DiffAdd)
        .unwrap();
    view.set_cursor(add_row_idx, 20);
    view.scroll_right(8);

    let mut term = HeadlessTerminal::new(60, 25);
    view.render_with_capabilities(&mut term, 60, 25, &opts, &caps)
        .unwrap();
    let rendered_text: String = (0..24)
        .map(|y| {
            (0..60)
                .map(|x| term.cell(x, y).unwrap().ch)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered_text.contains('‹'),
        "Horizontally panned diff (col_offset > 0) must display '‹' left-overflow marker, got:\n{rendered_text}"
    );

    // 2. Test soft line wrapping when wrap_lines = true (continuation marker '↪' and tail text visible)
    opts.wrap_lines = true;
    view.scroll_first_col();
    let mut term_wrap = HeadlessTerminal::new(60, 25);
    view.render_with_capabilities(&mut term_wrap, 60, 25, &opts, &caps)
        .unwrap();
    let wrapped_text: String = (0..24)
        .map(|y| {
            (0..60)
                .map(|x| term_wrap.cell(x, y).unwrap().ch)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        wrapped_text.contains('↪'),
        "Wrapped diff line must render continuation marker '↪' in gutter, got:\n{wrapped_text}"
    );
    assert!(
        wrapped_text.contains("↪configuration_value_with_arguments")
            && wrapped_text.contains("↪a);"),
        "Wrapped diff line must render the overflowing tail across continuation rows, got:\n{wrapped_text}"
    );
}

#[test]
fn test_tier1_moved_block_detection_and_highlighting() {
    let oid = make_oid(0x55);
    let moved_lines = [
        "fn validate_request_signature(payload: &[u8], key: &[u8]) -> bool {",
        "    let digest = compute_hmac_sha256_digest(payload, key);",
        "    constant_time_compare_slices(&digest, expected_signature)",
        "}",
    ];

    let diff = CommitDiff {
        commit_id: oid,
        parent_ids: vec![make_oid(0x54)],
        author_name: Arc::from("Alice Developer"),
        author_email: Arc::from("alice@example.com"),
        author_date: "Mon Sep 14 12:00:00 2026 +0000".to_string(),
        committer_name: Arc::from("Alice Developer"),
        committer_email: Arc::from("alice@example.com"),
        committer_date: "Mon Sep 14 12:00:00 2026 +0000".to_string(),
        title: Arc::from("Move signature validation helper into dedicated module"),
        body: None,
        files: vec![
            FileDiff {
                status: FileChangeStatus::Modified,
                path: "src/server.rs".to_string(),
                old_mode: Some(0o100_644),
                new_mode: Some(0o100_644),
                is_binary: false,
                hunks: vec![DiffHunk {
                    old_start: 10,
                    old_len: 4,
                    new_start: 10,
                    new_len: 0,
                    func_context: None,
                    lines: moved_lines
                        .iter()
                        .map(|l| HunkLine {
                            kind: DiffLineKind::Remove,
                            content: (*l).to_string(),
                            no_newline_at_eof: false,
                        })
                        .collect(),
                }],
                old_id: Some(oid),
                new_id: Some(oid),
                additions: 0,
                deletions: 4,
            },
            FileDiff {
                status: FileChangeStatus::Modified,
                path: "src/auth.rs".to_string(),
                old_mode: Some(0o100_644),
                new_mode: Some(0o100_644),
                is_binary: false,
                hunks: vec![DiffHunk {
                    old_start: 1,
                    old_len: 0,
                    new_start: 1,
                    new_len: 4,
                    func_context: None,
                    lines: moved_lines
                        .iter()
                        .map(|l| HunkLine {
                            kind: DiffLineKind::Add,
                            content: (*l).to_string(),
                            no_newline_at_eof: false,
                        })
                        .collect(),
                }],
                old_id: Some(oid),
                new_id: Some(oid),
                additions: 4,
                deletions: 0,
            },
        ],
        stats: DiffSummaryStats {
            files_changed: 2,
            insertions: 4,
            deletions: 4,
        },
    };

    let mut opts = ViewOptions {
        color_moved: true,
        ..Default::default()
    };
    let view = DiffView::new_with_options(diff.clone(), &opts, None);

    let moved_del_count = view
        .document()
        .rows
        .iter()
        .filter(|r| {
            r.row_type == DiffLineType::DiffDel && r.left.as_ref().is_some_and(|c| c.is_moved)
        })
        .count();
    let moved_add_count = view
        .document()
        .rows
        .iter()
        .filter(|r| {
            r.row_type == DiffLineType::DiffAdd && r.left.as_ref().is_some_and(|c| c.is_moved)
        })
        .count();
    assert_eq!(
        moved_del_count, 4,
        "All 4 deleted lines in moved block must be marked is_moved"
    );
    assert_eq!(
        moved_add_count, 4,
        "All 4 added lines in moved block must be marked is_moved"
    );

    // When color_moved is disabled, zero rows should be marked is_moved
    opts.color_moved = false;
    let view_disabled = DiffView::new_with_options(diff, &opts, None);
    let any_moved = view_disabled.document().rows.iter().any(|r| {
        r.left.as_ref().is_some_and(|c| c.is_moved) || r.right.as_ref().is_some_and(|c| c.is_moved)
    });
    assert!(
        !any_moved,
        "When color_moved = false, no cells may be marked is_moved"
    );
}

#[test]
fn test_tier1_interactive_hunk_by_hunk_context_expansion() {
    let diff = sample_diff();
    let mut lines_vec: Vec<Arc<str>> = vec![
        Arc::from("fn main() {"),
        Arc::from("    let message = \"Hello new\";"),
        Arc::from("    println!(\"{}\", message);"),
        Arc::from("}"),
    ];
    // Lines 5..=9 (gap between hunk 1 and hunk 2)
    for i in 5..=9 {
        lines_vec.push(Arc::from(format!("// gap context line {i}")));
    }
    // Lines 10..=14 (hunk 2 new lines)
    lines_vec.extend([
        Arc::from("fn helper() {"),
        Arc::from("    first_added();"),
        Arc::from("    inserted_middle();"),
        Arc::from("    second_added();"),
        Arc::from("}"),
    ]);
    // Lines 15..=30 (trailing lines after hunk 2)
    for i in 15..=30 {
        lines_vec.push(Arc::from(format!("fn trailing_line_{i}() {{}}")));
    }
    let full_lines: Arc<[Arc<str>]> = lines_vec.into();

    let opts = ViewOptions {
        diff_context: 1,
        ..Default::default()
    };
    let provider = |_oid: ObjectId| Some(Arc::clone(&full_lines));
    let mut view = DiffView::new_with_options(diff, &opts, Some(&provider));

    let base_rows = view.line_count();

    // Move cursor to the second hunk (which starts at line 10 and has surrounding blob context)
    view.next_hunk(30);
    view.next_hunk(30);
    let hunk2_idx = view.cursor_index();
    assert_eq!(
        view.document().rows[hunk2_idx].row_type,
        DiffLineType::HunkHeader
    );

    // Expand context around ONLY this hunk by +10 lines via `expand_current_hunk_context` (or pressing Enter on @@)
    let msg = view
        .expand_current_hunk_context(10, &opts, Some(&provider), 30)
        .expect("Expanding hunk context should succeed");
    assert!(
        msg.contains("+10"),
        "Status message should mention +10 extra context: {msg}"
    );

    let expanded_rows = view.line_count();
    assert!(
        expanded_rows > base_rows,
        "Expanding hunk 2 context by +10 must splice additional context rows ({expanded_rows} > {base_rows})"
    );

    // Verify that the HunkHeader row now displays the `[+10 ctx]` badge
    let hunk_headers: Vec<&str> = view
        .document()
        .rows
        .iter()
        .filter(|r| r.row_type == DiffLineType::HunkHeader)
        .filter_map(|r| r.left.as_ref().map(|c| c.text.as_ref()))
        .collect();
    assert!(
        hunk_headers.iter().any(|h| h.contains("[+10 ctx]")),
        "Expanded hunk header must display '[+10 ctx]' badge, got headers: {hunk_headers:?}"
    );

    // Shrink context around this hunk by -10 lines back to default
    let shrink_msg = view
        .shrink_current_hunk_context(10, &opts, Some(&provider), 30)
        .expect("Shrinking hunk context should succeed");
    assert!(
        shrink_msg.contains("default"),
        "Status message should confirm reset to default: {shrink_msg}"
    );
    assert_eq!(view.line_count(), base_rows);
}

#[test]
fn test_continuous_diff_context_toggle_never_freezes_on_huge_files() {
    let huge_blob_oid = ObjectId::from_bytes_or_panic(&[0x77; 20]);
    let small_blob_oid = ObjectId::from_bytes_or_panic(&[0x88; 20]);

    // 100,000-line Rust file (exceeds diff_context_full_max_lines = 50,000 and MAX_HIGHLIGHT_LINES = 2,000)
    let huge_lines: Arc<[Arc<str>]> = (1..=100_000)
        .map(|i| {
            if i == 50_000 {
                Arc::from("    let updated_metric: u64 = compute_fast_metric(50_000);")
            } else {
                Arc::from(format!("    let line_{i}: u64 = {i};").as_str())
            }
        })
        .collect::<Vec<_>>()
        .into();

    // 250-line Rust file (within diff_context_full_max_lines and MAX_HIGHLIGHT_LINES)
    let small_lines: Arc<[Arc<str>]> = (1..=250)
        .map(|i| {
            if i == 125 {
                Arc::from("    let updated_helper: usize = compute_helper(125);")
            } else {
                Arc::from(format!("    let helper_{i}: usize = {i};").as_str())
            }
        })
        .collect::<Vec<_>>()
        .into();

    let diff = CommitDiff {
        commit_id: ObjectId::from_bytes_or_panic(&[0x66; 20]),
        parent_ids: vec![ObjectId::from_bytes_or_panic(&[0x65; 20])],
        author_name: "Alice Developer".into(),
        author_email: "alice@example.com".into(),
        author_date: "2026-09-22".to_string(),
        committer_name: "Alice Developer".into(),
        committer_email: "alice@example.com".into(),
        committer_date: "2026-09-22".to_string(),
        title: "Touch both a 100k-line file and a 250-line file".into(),
        body: None,
        stats: DiffSummaryStats {
            files_changed: 2,
            insertions: 2,
            deletions: 2,
        },
        files: vec![
            FileDiff {
                path: "src/huge_generated.rs".to_string(),
                old_id: Some(ObjectId::from_bytes_or_panic(&[0x76; 20])),
                new_id: Some(huge_blob_oid),
                old_mode: Some(0o100_644),
                new_mode: Some(0o100_644),
                status: FileChangeStatus::Modified,
                is_binary: false,
                additions: 1,
                deletions: 1,
                hunks: vec![DiffHunk {
                    old_start: 49_999,
                    old_len: 3,
                    new_start: 49_999,
                    new_len: 3,
                    func_context: Some("fn huge_table()".to_string()),
                    lines: vec![
                        HunkLine {
                            kind: DiffLineKind::Context,
                            content: "    let line_49999: u64 = 49999;".to_string(),
                            no_newline_at_eof: false,
                        },
                        HunkLine {
                            kind: DiffLineKind::Remove,
                            content: "    let line_50000: u64 = 50000;".to_string(),
                            no_newline_at_eof: false,
                        },
                        HunkLine {
                            kind: DiffLineKind::Add,
                            content: "    let updated_metric: u64 = compute_fast_metric(50_000);"
                                .to_string(),
                            no_newline_at_eof: false,
                        },
                        HunkLine {
                            kind: DiffLineKind::Context,
                            content: "    let line_50001: u64 = 50001;".to_string(),
                            no_newline_at_eof: false,
                        },
                    ],
                }],
            },
            FileDiff {
                path: "src/small_helper.rs".to_string(),
                old_id: Some(ObjectId::from_bytes_or_panic(&[0x87; 20])),
                new_id: Some(small_blob_oid),
                old_mode: Some(0o100_644),
                new_mode: Some(0o100_644),
                status: FileChangeStatus::Modified,
                is_binary: false,
                additions: 1,
                deletions: 1,
                hunks: vec![DiffHunk {
                    old_start: 124,
                    old_len: 3,
                    new_start: 124,
                    new_len: 3,
                    func_context: Some("fn small_helper()".to_string()),
                    lines: vec![
                        HunkLine {
                            kind: DiffLineKind::Context,
                            content: "    let helper_124: usize = 124;".to_string(),
                            no_newline_at_eof: false,
                        },
                        HunkLine {
                            kind: DiffLineKind::Remove,
                            content: "    let helper_125: usize = 125;".to_string(),
                            no_newline_at_eof: false,
                        },
                        HunkLine {
                            kind: DiffLineKind::Add,
                            content: "    let updated_helper: usize = compute_helper(125);"
                                .to_string(),
                            no_newline_at_eof: false,
                        },
                        HunkLine {
                            kind: DiffLineKind::Context,
                            content: "    let helper_126: usize = 126;".to_string(),
                            no_newline_at_eof: false,
                        },
                    ],
                }],
            },
        ],
    };

    let provider = |oid: ObjectId| -> Option<Arc<[Arc<str>]>> {
        if oid == huge_blob_oid {
            Some(Arc::clone(&huge_lines))
        } else if oid == small_blob_oid {
            Some(Arc::clone(&small_lines))
        } else {
            None
        }
    };

    let start = std::time::Instant::now();

    // 1. Verify full-context build enforces diff_context_full_max_lines on the 100,000-line file
    // while still expanding the 250-line file in full and highlighting it.
    let full_opts = ViewOptions {
        diff_context: ViewOptions::DIFF_CONTEXT_FULL,
        syntax_highlighting: true,
        ..Default::default()
    };
    let full_view = DiffView::new_with_options(diff.clone(), &full_opts, Some(&provider));
    assert!(
        full_view.line_count() < 1_000,
        "100k-line file must be bounded by diff_context_full_max_lines instead of splicing 100k rows, got {}",
        full_view.line_count()
    );

    // 2. Simulate holding `]` 30 times and `[` 30 times in AppState
    let mut app = AppState::default();
    app.push_view(ViewKind::Diff);
    app.views.diff_view = Some(full_view);
    app.options.diff_context = 3;
    app.refresh_diff_view();

    for _ in 0..30 {
        handle_event_with_dimensions(&key(KeyCode::Char(']')), &mut app, 120, 30);
    }
    assert_eq!(app.options.diff_context, ViewOptions::DIFF_CONTEXT_FULL);

    // Pressing `]` again when already at DIFF_CONTEXT_FULL must be a true O(1) no-op
    let doc_before_noop = Arc::clone(app.views.diff_view.as_ref().unwrap().document_arc());
    handle_event_with_dimensions(&key(KeyCode::Char(']')), &mut app, 120, 30);
    let doc_after_noop = Arc::clone(app.views.diff_view.as_ref().unwrap().document_arc());
    assert!(
        Arc::ptr_eq(&doc_before_noop, &doc_after_noop),
        "Pressing ] when already at DIFF_CONTEXT_FULL must not rebuild DiffDocument"
    );

    for _ in 0..30 {
        handle_event_with_dimensions(&key(KeyCode::Char('[')), &mut app, 120, 30);
    }
    assert_eq!(app.options.diff_context, 0);

    // Pressing `[` again when already at 0 must be a true O(1) no-op
    let doc_at_zero = Arc::clone(app.views.diff_view.as_ref().unwrap().document_arc());
    handle_event_with_dimensions(&key(KeyCode::Char('[')), &mut app, 120, 30);
    assert!(
        Arc::ptr_eq(
            &doc_at_zero,
            app.views.diff_view.as_ref().unwrap().document_arc()
        ),
        "Pressing [ when already at 0 must not rebuild DiffDocument"
    );

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 5_000,
        "Continuous 60-keystroke ] / [ stress burst took too long: {elapsed:?}"
    );
}

#[test]
fn test_banner_presentation_p1_to_p11_and_yank_fidelity() {
    use tigrs_core::DiffHintsMode;
    use tigrs_ui::Flow;
    use tigrs_ui::diff::layout::{
        elide_middle_path, format_magnitude_sparkline, format_rename_brace_diff,
    };

    // 1. Verify P1: Out-of-box default is DiffPresentation::Banner
    let opts = ViewOptions::default();
    assert_eq!(opts.diff_presentation, DiffPresentation::Banner);
    assert!(opts.diff_sticky_header);
    assert_eq!(opts.diff_hints, DiffHintsMode::Auto);
    assert!(opts.diff_collapse_generated);

    // 2. Verify P4 helper functions: smart path elision & rename brace-diffing
    let elided = elide_middle_path(
        "drivers/gpu/drm/amd/display/dc/dml2/dml21/src/dml2_core/dml2_core_dcn4.c",
        38,
        false,
    );
    assert!(
        elided.contains('…') && elided.ends_with("dml2_core_dcn4.c"),
        "Expected middle-elided path preserving first segment and filename, got: {elided}"
    );
    let brace_diff = format_rename_brace_diff(
        "sound/soc/amd/acp/acp-sdw-mach.c",
        "sound/soc/amd/acp/acp-sdw-sof-mach.c",
        false,
    );
    assert_eq!(
        brace_diff,
        "sound/soc/amd/acp/acp-sdw-{mach \u{2192} sof-mach}.c"
    );

    // 3. Verify P8 sparkline rendering (UTF-8 and ASCII fallback)
    assert_eq!(format_magnitude_sparkline(1, 0, false), "▏\u{2581}▏");
    assert_eq!(format_magnitude_sparkline(250, 50, false), "▏\u{2587}▏");
    assert_eq!(
        format_magnitude_sparkline(301, 50, false),
        "▏\u{2588}\u{2589}▏"
    );
    assert_eq!(format_magnitude_sparkline(1, 0, true), "[.  ]");
    assert_eq!(format_magnitude_sparkline(250, 50, true), "[###]");

    // 4. Build a multi-file CommitDiff exercising P1, P3, P4, P6, P7, P9, P10, P11
    tigrs_git::mark_path_linguist_generated("api/schema.pb.go", true);
    let diff = CommitDiff {
        commit_id: make_oid(0x42),
        parent_ids: vec![make_oid(0x41)],
        author_name: Arc::from("Alice Developer"),
        author_email: Arc::from("alice@example.com"),
        author_date: "Mon Sep 14 12:00:00 2026 +0000".to_string(),
        committer_name: Arc::from("Alice Developer"),
        committer_email: Arc::from("alice@example.com"),
        committer_date: "Mon Sep 14 12:00:00 2026 +0000".to_string(),
        title: Arc::from("sound: soc: amd: update machine driver"),
        body: None,
        files: vec![
            // File 0: Renamed C file with function context (P1, P3, P4, P6, P7, P8, P11)
            FileDiff {
                status: FileChangeStatus::Renamed {
                    source_path: "sound/soc/amd/acp/acp-sdw-mach.c".to_string(),
                    similarity_pct: 97,
                },
                path: "sound/soc/amd/acp/acp-sdw-sof-mach.c".to_string(),
                old_mode: Some(0o100_644),
                new_mode: Some(0o100_644),
                is_binary: false,
                old_id: Some(make_oid(0x11)),
                new_id: Some(make_oid(0x22)),
                additions: 1,
                deletions: 0,
                hunks: vec![DiffHunk {
                    old_start: 285,
                    old_len: 17,
                    new_start: 285,
                    new_len: 18,
                    func_context: Some(
                        "static int sof_card_dai_links_create(struct snd_soc_card *card)"
                            .to_string(),
                    ),
                    lines: {
                        let mut hlines = vec![
                            HunkLine {
                                kind: DiffLineKind::Context,
                                content: "    int ret = 0;".to_string(),
                                no_newline_at_eof: false,
                            },
                            HunkLine {
                                kind: DiffLineKind::Add,
                                content: "    card->owner = THIS_MODULE;".to_string(),
                                no_newline_at_eof: false,
                            },
                        ];
                        for i in 0..16 {
                            hlines.push(HunkLine {
                                kind: DiffLineKind::Context,
                                content: format!("    snd_soc_card_step({i});"),
                                no_newline_at_eof: false,
                            });
                        }
                        hlines
                    },
                }],
            },
            // File 1: Mode-only invisible change (P10)
            FileDiff {
                status: FileChangeStatus::Modified,
                path: "scripts/deploy.sh".to_string(),
                old_mode: Some(0o100_644),
                new_mode: Some(0o100_755),
                is_binary: false,
                old_id: Some(make_oid(0x33)),
                new_id: Some(make_oid(0x33)),
                additions: 0,
                deletions: 0,
                hunks: vec![],
            },
            // File 2: Whitespace-only invisible change (P10)
            FileDiff {
                status: FileChangeStatus::Modified,
                path: "src/whitespace.rs".to_string(),
                old_mode: Some(0o100_644),
                new_mode: Some(0o100_644),
                is_binary: false,
                old_id: Some(make_oid(0x44)),
                new_id: Some(make_oid(0x55)),
                additions: 1,
                deletions: 1,
                hunks: vec![DiffHunk {
                    old_start: 1,
                    old_len: 1,
                    new_start: 1,
                    new_len: 1,
                    func_context: None,
                    lines: vec![
                        HunkLine {
                            kind: DiffLineKind::Remove,
                            content: "let x = 1;".to_string(),
                            no_newline_at_eof: false,
                        },
                        HunkLine {
                            kind: DiffLineKind::Add,
                            content: "    let x = 1;".to_string(),
                            no_newline_at_eof: false,
                        },
                    ],
                }],
            },
            // File 3: linguist-generated file (P9)
            FileDiff {
                status: FileChangeStatus::Modified,
                path: "api/schema.pb.go".to_string(),
                old_mode: Some(0o100_644),
                new_mode: Some(0o100_644),
                is_binary: false,
                old_id: Some(make_oid(0x66)),
                new_id: Some(make_oid(0x77)),
                additions: 2,
                deletions: 0,
                hunks: vec![DiffHunk {
                    old_start: 10,
                    old_len: 1,
                    new_start: 10,
                    new_len: 3,
                    func_context: None,
                    lines: vec![
                        HunkLine {
                            kind: DiffLineKind::Add,
                            content: "// generated line 1".to_string(),
                            no_newline_at_eof: false,
                        },
                        HunkLine {
                            kind: DiffLineKind::Add,
                            content: "// generated line 2".to_string(),
                            no_newline_at_eof: false,
                        },
                    ],
                }],
            },
        ],
        stats: DiffSummaryStats {
            files_changed: 4,
            insertions: 4,
            deletions: 1,
        },
    };

    let caps = TerminalCapabilities::default();
    let mut view = DiffView::new_with_options(diff, &opts, None);

    // Verify P9: File 3 (idx 3, "api/schema.pb.go") was auto-folded because of linguist-generated,
    // while non-attributed files (0, 1, 2) are NOT folded.
    let file3_banner_idx = view.document().file_indices[3];
    let gen_banner_text = &view.document().rows[file3_banner_idx]
        .left
        .as_ref()
        .unwrap()
        .text;
    assert!(
        gen_banner_text.starts_with("▸") && gen_banner_text.contains("generated (.gitattributes)"),
        "Generated file banner must auto-collapse with ▸ glyph and 'generated (.gitattributes)' badge, got: {gen_banner_text}"
    );
    assert!(
        !view.document().rows.iter().enumerate().any(|(idx, r)| {
            view.document().line_to_file.get(idx).copied().flatten() == Some(3)
                && r.row_type != DiffLineType::FileHeader
                && r.row_type != DiffLineType::StatFile
        }),
        "Auto-collapsed linguist-generated file must hide its hunks until expanded"
    );

    // Verify P1, P3, P4, P6, P7, P8, P10 in document rows
    let file0_row_idx = view.document().file_indices[0];
    let file0_banner = &view.document().rows[file0_row_idx]
        .left
        .as_ref()
        .unwrap()
        .text;
    assert!(
        file0_banner.contains("1/4")
            && file0_banner.contains("R97")
            && file0_banner.contains("acp-sdw-{mach \u{2192} sof-mach}.c")
            && file0_banner.contains("+1 -0"),
        "File 0 banner missing expected P1/P4/P6/P8 elements: {file0_banner}"
    );

    let hunk0_row_idx = view
        .document()
        .rows
        .iter()
        .enumerate()
        .find(|&(idx, r)| {
            r.row_type == DiffLineType::HunkHeader
                && view.document().line_to_file.get(idx).copied().flatten() == Some(0)
        })
        .unwrap()
        .0;
    let file0_hunk = &view.document().rows[hunk0_row_idx]
        .left
        .as_ref()
        .unwrap()
        .text;
    assert!(
        file0_hunk.contains("⋯ 284 unchanged lines ⋯   + expand  ] all")
            && file0_hunk.contains("┃ C ▸ sof_card_dai_links_create()"),
        "File 0 hunk separator missing expected P3/P7 elements: {file0_hunk}"
    );

    let file1_row_idx = view.document().file_indices[1];
    let file1_banner = &view.document().rows[file1_row_idx]
        .left
        .as_ref()
        .unwrap()
        .text;
    assert!(
        file1_banner.contains("mode 100644 \u{2192} 100755 \u{00b7} no content change"),
        "File 1 banner missing P10 mode-only badge: {file1_banner}"
    );

    let file2_row_idx = view.document().file_indices[2];
    let file2_banner = &view.document().rows[file2_row_idx]
        .left
        .as_ref()
        .unwrap()
        .text;
    assert!(
        file2_banner.contains("whitespace only"),
        "File 2 banner missing P10 whitespace-only badge: {file2_banner}"
    );

    // Verify P11: Hint chips appear on wide terminal (140 cols >= 100) and hide on narrow terminal (85 cols < 100)
    view.set_cursor(file0_row_idx, 30);
    let mut wide_term = HeadlessTerminal::new(140, 30);
    view.render_with_capabilities(&mut wide_term, 140, 30, &opts, &caps)
        .unwrap();
    let wide_screen = wide_term.screen_text();
    assert!(
        wide_screen.contains("za fold  e edit  i info  y copy"),
        "Wide terminal (140 cols) with diff_hints=Auto must render action hint chips:\n{wide_screen}"
    );
    assert!(
        wide_term.line_text(0).contains("File 1 of 4"),
        "Title bar (P6) must include 'File 1 of 4' breadcrumb, got: {}",
        wide_term.line_text(0)
    );

    let mut narrow_term = HeadlessTerminal::new(85, 30);
    view.render_with_capabilities(&mut narrow_term, 85, 30, &opts, &caps)
        .unwrap();
    let narrow_screen = narrow_term.screen_text();
    assert!(
        !narrow_screen.contains("za fold  e edit  i info  y copy"),
        "Narrow terminal (85 cols < 100) with diff_hints=Auto must hide action hint chips:\n{narrow_screen}"
    );

    // Verify Copy Fidelity (`y` / yank_text_at_cursor):
    // Positioning cursor on File 0's decorative banner row must yank the real git header!
    view.set_cursor(file0_row_idx, 30);
    let yanked_header = view.yank_text_at_cursor().expect("yank file banner");
    assert!(
        yanked_header.starts_with(
            "diff --git a/sound/soc/amd/acp/acp-sdw-mach.c b/sound/soc/amd/acp/acp-sdw-sof-mach.c"
        ) && yanked_header.contains("similarity index 97%")
            && yanked_header.contains("rename from sound/soc/amd/acp/acp-sdw-mach.c"),
        "Yanking a Banner FileHeader must return the real underlying git diff header, got:\n{yanked_header}"
    );

    // Positioning cursor on File 0's decorative hunk separator must yank the real @@ header!
    view.set_cursor(hunk0_row_idx, 30);
    let yanked_hunk = view.yank_text_at_cursor().expect("yank hunk separator");
    assert_eq!(
        yanked_hunk,
        "@@ -285,17 +285,18 @@ static int sof_card_dai_links_create(struct snd_soc_card *card)"
    );

    // Verify P2 Adaptive Sticky Header:
    // Scroll past File 0's banner (`scroll_offset > file0_row_idx`) and move cursor below scroll_offset
    view.scroll_line_down(file0_row_idx + 1, 15);
    view.set_cursor(hunk0_row_idx + 2, 15);
    let mut sticky_term = HeadlessTerminal::new(120, 15);
    view.render_with_capabilities(&mut sticky_term, 120, 15, &opts, &caps)
        .unwrap();
    let row2_text = sticky_term.line_text(1);
    assert!(
        row2_text.starts_with("📌")
            && row2_text.contains("1/4")
            && row2_text.contains("┃  C ▸ sof_card_dai_links_create()"),
        "Adaptive sticky header must appear on ANSI row 2 (0-based row 1) when scrolled past file banner, got: {row2_text}"
    );

    // Verify P5 On-Demand File Details Popover (`i` to open, `Esc` to close)
    let mut app = AppState::default();
    app.push_view(ViewKind::Diff);
    app.views.diff_view = Some(view);

    let key_i = Event::Key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert_eq!(
        handle_event_with_dimensions(&key_i, &mut app, 120, 30),
        Flow::Continue
    );
    assert!(
        app.views
            .diff_view
            .as_ref()
            .unwrap()
            .is_showing_file_details(),
        "Pressing 'i' in DiffView must open the File Details popover"
    );

    let mut popover_term = HeadlessTerminal::new(120, 30);
    app.views
        .diff_view
        .as_ref()
        .unwrap()
        .render_with_capabilities(&mut popover_term, 120, 30, &opts, &caps)
        .unwrap();
    let popover_screen = popover_term.screen_text();
    assert!(
        popover_screen.contains("┌ File Details")
            && popover_screen.contains("Renamed from sound/soc/amd/acp/acp-sdw-mach.c (97%)")
            && popover_screen.contains("index 1111111..2222222"),
        "File Details popover must render bordered metadata card with raw git header and status:\n{popover_screen}"
    );

    // Pressing Esc closes the File Details popover FIRST without closing DiffView
    let key_esc = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(
        handle_event_with_dimensions(&key_esc, &mut app, 120, 30),
        Flow::Continue
    );
    assert!(
        !app.views
            .diff_view
            .as_ref()
            .unwrap()
            .is_showing_file_details(),
        "Pressing Esc when File Details popover is open must close the popover first"
    );
    assert_eq!(
        app.active_view(),
        Some(ViewKind::Diff),
        "Pressing Esc to close File Details popover must keep DiffView open"
    );
}

#[test]
fn test_e2e_gitattributes_linguist_generated_and_sticky_popover_yank_boundaries() {
    use std::fmt::Write as _;
    use tigrs_core::LineGraphics;
    use tigrs_ui::Flow;

    // 1. Create a real Git repository on disk with .gitattributes setting and unsetting linguist-generated
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    let run_git = |args: &[&str]| {
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(path)
            .status()
            .expect("git command");
        assert!(status.success(), "git {args:?} failed");
    };

    run_git(&["init", "-b", "main"]);
    run_git(&["config", "user.name", "Alice Developer"]);
    run_git(&["config", "user.email", "alice@example.com"]);

    std::fs::create_dir_all(path.join("api")).unwrap();
    std::fs::create_dir_all(path.join("dist")).unwrap();
    std::fs::create_dir_all(path.join("src")).unwrap();

    std::fs::write(
        path.join(".gitattributes"),
        "*.pb.go linguist-generated\ndist/bundle.js linguist-generated=true\napi/handwritten.pb.go -linguist-generated\n",
    )
    .unwrap();
    std::fs::write(path.join("api/service.pb.go"), "// v1 generated\n").unwrap();
    std::fs::write(path.join("dist/bundle.js"), "/* v1 bundle */\n").unwrap();
    std::fs::write(path.join("api/handwritten.pb.go"), "// v1 manual\n").unwrap();
    let mut main_src = String::from("int init_driver(void) {\n");
    for i in 0..20 {
        let _ = writeln!(main_src, "    step_call({i});");
    }
    main_src.push_str("    return 0;\n}\n");
    std::fs::write(path.join("src/driver.c"), &main_src).unwrap();

    run_git(&["add", "."]);
    run_git(&["commit", "-m", "Initial commit with .gitattributes"]);

    // Modify all four files in the second commit
    std::fs::write(
        path.join("api/service.pb.go"),
        "// v2 generated\n// extra line\n",
    )
    .unwrap();
    std::fs::write(
        path.join("dist/bundle.js"),
        "/* v2 bundle */\nconsole.log(1);\n",
    )
    .unwrap();
    std::fs::write(
        path.join("api/handwritten.pb.go"),
        "// v2 manual edit\nfunc Manual() {}\n",
    )
    .unwrap();
    let mut main_src_v2 = String::from("int init_driver(void) {\n    int status = 1;\n");
    for i in 0..20 {
        let _ = writeln!(main_src_v2, "    step_call({i});");
    }
    main_src_v2.push_str("    return status;\n}\n");
    std::fs::write(path.join("src/driver.c"), &main_src_v2).unwrap();

    run_git(&["add", "."]);
    run_git(&["commit", "-m", "Update generated and manual files"]);

    // Compute commit diff through real GitEngine (exercising DriverResolver on .gitattributes)
    let engine = GitEngine::open(Some(path)).expect("open repo");
    let head_id = engine.head_commit_id().expect("resolve HEAD");
    let diff = engine.compute_commit_diff(head_id).expect("compute diff");

    assert!(tigrs_git::is_path_marked_linguist_generated(
        "api/service.pb.go"
    ));
    assert!(tigrs_git::is_path_marked_linguist_generated(
        "dist/bundle.js"
    ));
    assert!(
        !tigrs_git::is_path_marked_linguist_generated("api/handwritten.pb.go"),
        "Negated '-linguist-generated' in .gitattributes must unset generated status"
    );
    assert!(!tigrs_git::is_path_marked_linguist_generated(
        "src/driver.c"
    ));

    let mut opts = ViewOptions::default();
    let view = DiffView::new_with_options(diff.clone(), &opts, None);
    assert_eq!(
        view.folded_file_count(),
        2,
        "Only the 2 linguist-generated files (api/service.pb.go and dist/bundle.js) must auto-fold"
    );

    // When diff_collapse_generated = false, 0 files auto-fold on open
    opts.diff_collapse_generated = false;
    let view_unfolded = DiffView::new_with_options(diff, &opts, None);
    assert_eq!(view_unfolded.folded_file_count(), 0);
    opts.diff_collapse_generated = true;

    // 2. Verify Sticky Header boundary conditions on `src/driver.c`
    let mut view = view_unfolded;
    let driver_idx = view
        .diff()
        .files
        .iter()
        .position(|f| f.path == "src/driver.c")
        .unwrap();
    let driver_banner_row = view.document().file_indices[driver_idx];
    let caps = TerminalCapabilities::default();

    // Scroll past driver_banner_row and place cursor below scroll_offset
    view.scroll_line_down(driver_banner_row + 1, 15);
    view.set_cursor(driver_banner_row + 4, 15);

    // (a) Short viewport (< 10 visible rows, e.g. height = 10 -> visible_height = 8): sticky header suppressed
    let mut short_term = HeadlessTerminal::new(120, 10);
    view.render_with_capabilities(&mut short_term, 120, 10, &opts, &caps)
        .unwrap();
    assert!(
        !short_term.line_text(1).starts_with("📌"),
        "Sticky header must be suppressed when visible_height < 10, got: {}",
        short_term.line_text(1)
    );

    // (b) cursor == scroll_offset: sticky header suppressed so top cursor row is never occluded
    let current_scroll = view.scroll_offset();
    view.set_cursor(current_scroll, 15);
    let mut cursor_top_term = HeadlessTerminal::new(120, 15);
    view.render_with_capabilities(&mut cursor_top_term, 120, 15, &opts, &caps)
        .unwrap();
    assert!(
        !cursor_top_term.line_text(1).starts_with("📌"),
        "Sticky header must not occlude cursor when cursor == scroll_offset"
    );

    // (c) ASCII line-graphics fallback: renders `[pinned]` and `|` separator
    view.set_cursor(current_scroll + 3, 15);
    opts.line_graphics = LineGraphics::Ascii;
    let mut ascii_term = HeadlessTerminal::new(120, 15);
    view.render_with_capabilities(&mut ascii_term, 120, 15, &opts, &caps)
        .unwrap();
    let ascii_row2 = ascii_term.line_text(1);
    assert!(
        ascii_row2.starts_with("[pinned]") && ascii_row2.contains('|'),
        "ASCII line-graphics must render '[pinned]' and '|' in sticky header, got: {ascii_row2}"
    );
    opts.line_graphics = LineGraphics::Utf8;

    // 3. Verify `y` (YankDiffText) when File Details popover is open vs on code lines
    let mut app = AppState::default();
    app.push_view(ViewKind::Diff);
    app.views.diff_view = Some(view);

    // Press `i` to open File Details popover, then press `y` -> yanks full multi-line file header
    handle_event_with_dimensions(&key(KeyCode::Char('i')), &mut app, 120, 20);
    assert!(
        app.views
            .diff_view
            .as_ref()
            .unwrap()
            .is_showing_file_details()
    );
    let popover_yank = app
        .views
        .diff_view
        .as_ref()
        .unwrap()
        .yank_text_at_cursor()
        .unwrap();
    assert!(
        popover_yank.contains("diff --git a/src/driver.c b/src/driver.c")
            && popover_yank.contains("--- a/src/driver.c")
            && popover_yank.contains("+++ b/src/driver.c"),
        "yank_diff_text_at_cursor while File Details popover is open must return the full multi-line file header, got: {popover_yank:?}"
    );
    assert_eq!(
        handle_event_with_dimensions(&key(KeyCode::Char('y')), &mut app, 120, 20),
        Flow::Continue
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("Copied: diff --git a/src/driver.c b/src/driver.c")
    );

    // Pressing `y` while File Details popover is open copies the full header and keeps the popover open
    assert!(
        app.views
            .diff_view
            .as_ref()
            .unwrap()
            .is_showing_file_details()
    );

    // Press `i` (or Esc) -> dismisses the File Details popover
    handle_event_with_dimensions(&key(KeyCode::Char('i')), &mut app, 120, 20);
    assert!(
        !app.views
            .diff_view
            .as_ref()
            .unwrap()
            .is_showing_file_details()
    );

    // Press `y` on a code line -> copies code line and sets "Copied: ..." status message
    handle_event_with_dimensions(&key(KeyCode::Char('y')), &mut app, 120, 20);
    assert!(
        app.status_message
            .as_deref()
            .unwrap_or("")
            .starts_with("Copied: "),
        "Pressing 'y' on a code line must copy the line content, got: {:?}",
        app.status_message
    );
}

#[test]
fn test_multi_file_cpp_macro_syntax_highlighting_and_blank_line_bg() {
    // Preceding files totaling > 100 lines (which previously left < 100 lines of budget
    // under the old 200-line cap).
    let mut header_lines = Vec::new();
    for i in 0..120 {
        header_lines.push(HunkLine {
            kind: DiffLineKind::Add,
            content: format!("constexpr uint8_t kTestVector{i} = 0x{:02x};", i & 0xff),
            no_newline_at_eof: false,
        });
    }

    // Main C++ test file with multiple macro blocks and blank lines (~180 lines).
    let mut cpp_lines = vec![
        HunkLine {
            kind: DiffLineKind::Add,
            content: "#include <cstdint>".to_string(),
            no_newline_at_eof: false,
        },
        HunkLine {
            kind: DiffLineKind::Add,
            content: String::new(),
            no_newline_at_eof: false,
        },
        HunkLine {
            kind: DiffLineKind::Add,
            content: "DEFINE_TEST_GROUP(crypto_tests, nullptr, nullptr);".to_string(),
            no_newline_at_eof: false,
        },
    ];
    for idx in 0..15 {
        cpp_lines.extend([
            HunkLine {
                kind: DiffLineKind::Add,
                content: String::new(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Add,
                content: format!("DEFINE_TEST_CASE(crypto_tests, test_case_{idx}) {{"),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Add,
                content: "  VerifyFlow([&]() {".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Add,
                content: "    const uint32_t status = RunCheck();".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Add,
                content: String::new(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Add,
                content: "    EXPECT_EQ(status, 0, \"check failed\");".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Add,
                content: "  });".to_string(),
                no_newline_at_eof: false,
            },
            HunkLine {
                kind: DiffLineKind::Add,
                content: "}".to_string(),
                no_newline_at_eof: false,
            },
        ]);
    }

    let cpp_line_count = u32::try_from(cpp_lines.len()).unwrap();
    let diff = CommitDiff {
        commit_id: make_oid(0x11),
        parent_ids: vec![],
        author_name: "Alice Developer".into(),
        author_email: "alice@example.com".into(),
        author_date: "2026-03-01 12:00:00 +0000".into(),
        committer_name: "Alice Developer".into(),
        committer_email: "alice@example.com".into(),
        committer_date: "2026-03-01 12:00:00 +0000".into(),
        title: "test: add C++ macro unit tests".into(),
        body: None,
        stats: DiffSummaryStats {
            files_changed: 2,
            insertions: 120 + usize::try_from(cpp_line_count).unwrap(),
            deletions: 0,
        },
        files: vec![
            FileDiff {
                path: "src/vectors.h".into(),
                status: FileChangeStatus::Added,
                additions: 120,
                deletions: 0,
                is_binary: false,
                old_id: None,
                new_id: None,
                old_mode: None,
                new_mode: Some(0o100_644),
                hunks: vec![DiffHunk {
                    old_start: 0,
                    old_len: 0,
                    new_start: 1,
                    new_len: 120,
                    func_context: None,
                    lines: header_lines,
                }],
            },
            FileDiff {
                path: "src/main.cc".into(),
                status: FileChangeStatus::Added,
                additions: usize::try_from(cpp_line_count).unwrap(),
                deletions: 0,
                is_binary: false,
                old_id: None,
                new_id: None,
                old_mode: None,
                new_mode: Some(0o100_644),
                hunks: vec![DiffHunk {
                    old_start: 0,
                    old_len: 0,
                    new_start: 1,
                    new_len: cpp_line_count,
                    func_context: None,
                    lines: cpp_lines,
                }],
            },
        ],
    };

    let opts = ViewOptions {
        syntax_highlighting: true,
        ..Default::default()
    };
    let view = DiffView::new_with_options(diff, &opts, None);

    // Every single row of `src/main.cc` (including the final macro block well past line 200
    // cumulative, and blank lines inside the hunk) must have non-empty `syntax_spans` so both
    // syntax token coloring and `syntax_diff_bg` background tinting remain continuous.
    let main_cc_rows: Vec<_> = view
        .document()
        .rows
        .iter()
        .enumerate()
        .filter(|&(idx, r)| {
            view.document().line_to_file.get(idx).copied().flatten() == Some(1)
                && matches!(r.row_type, DiffLineType::DiffAdd)
        })
        .collect();
    assert_eq!(main_cc_rows.len(), usize::try_from(cpp_line_count).unwrap());
    for (row_idx, row) in &main_cc_rows {
        let cell = row.left.as_ref().expect("Added row has left cell");
        assert!(
            !cell.syntax_spans.is_empty(),
            "Row {row_idx} ({:?}) in src/main.cc lost syntax highlighting!",
            cell.text
        );
    }

    // Verify at the painted terminal cell layer (`HeadlessTerminal`) that blank `+` rows inside
    // `src/main.cc` receive the exact same `syntax_diff_bg` (`cell.bg`) as adjacent non-blank `+`
    // code rows in BOTH `DiffLayout::Unified` and `DiffLayout::SideBySide`.
    let caps = TerminalCapabilities {
        color_profile: ColorProfile::TrueColor,
        supports_kitty_keyboard: false,
        supports_unicode_box: true,
        supports_synchronized_output: false,
    };
    let first_main_cc_row = main_cc_rows[0].0;
    for layout in [
        tigrs_core::DiffLayout::Unified,
        tigrs_core::DiffLayout::SideBySide,
    ] {
        let layout_opts = ViewOptions {
            syntax_highlighting: true,
            diff_layout: layout,
            ..Default::default()
        };
        let mut layout_view = DiffView::new_with_options(view.diff().clone(), &layout_opts, None);
        // Position viewport inside `src/main.cc` and place cursor at the first row so rows below
        // are painted without cursor selection styling.
        layout_view.set_cursor(first_main_cc_row + 15, 25);
        layout_view.set_cursor(first_main_cc_row, 25);

        let mut term = HeadlessTerminal::new(120, 25);
        layout_view
            .render_with_capabilities(&mut term, 120, 25, &layout_opts, &caps)
            .unwrap();

        let (_, code_y) = term
            .find_text("DEFINE_TEST_GROUP(crypto_tests")
            .unwrap_or_else(|| panic!("find DEFINE_TEST_GROUP in {layout:?}"));
        // The row immediately above `DEFINE_TEST_GROUP` is the blank `+` line (`cpp_lines[1]`).
        let blank_y = code_y - 1;
        let sample_col = match layout {
            tigrs_core::DiffLayout::Unified => 12,
            tigrs_core::DiffLayout::SideBySide => 75,
        };
        let code_bg = term.cell(sample_col, code_y).unwrap().bg;
        let blank_bg = term.cell(sample_col, blank_y).unwrap().bg;
        assert!(
            code_bg.is_some(),
            "Expected non-None syntax_diff_bg on code row in {layout:?}"
        );
        assert_eq!(
            blank_bg, code_bg,
            "Blank added row in {layout:?} must have identical background tint to code row (no black stripe)"
        );
    }

    // Verify `FileSyntaxHighlighter` `old_hl` vs `new_hl` isolation: an unclosed `/*` block
    // comment on a deleted (`-`) line must NOT cause subsequent added (`+`) code lines to be
    // tokenized as comments.
    let mut iso_diff = sample_diff();
    iso_diff.files[0].path = "src/isolated.rs".to_string();
    iso_diff.files[0].hunks[0].lines = vec![
        HunkLine {
            kind: DiffLineKind::Remove,
            content: "/* unclosed block comment on deleted line".to_string(),
            no_newline_at_eof: false,
        },
        HunkLine {
            kind: DiffLineKind::Add,
            content: "pub fn verify_token(input: u32) -> bool { input == 42 }".to_string(),
            no_newline_at_eof: false,
        },
    ];
    let iso_view = DiffView::new_with_options(iso_diff, &opts, None);
    let add_row = iso_view
        .document()
        .rows
        .iter()
        .find(|r| matches!(r.row_type, DiffLineType::DiffAdd))
        .expect("find DiffAdd row");
    let add_cell = add_row.left.as_ref().expect("add left cell");
    // A comment line has a single uniform foreground color across the entire line, whereas a
    // properly tokenized Rust function signature has multiple distinct syntax token colors.
    let distinct_fgs: std::collections::HashSet<_> =
        add_cell.syntax_spans.iter().map(|s| s.fg).collect();
    assert!(
        distinct_fgs.len() >= 2,
        "Added Rust code line after deleted unclosed /* comment must retain multi-token syntax highlighting, got spans: {:?}",
        add_cell.syntax_spans
    );
}
