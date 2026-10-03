// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Headless TUI Snapshot Suite.
//!
//! Provides comprehensive snapshot and cell-attribute testing for all interactive views
//! (Main, Diff, Status, Tree, Blob, Blame) and view-stack transitions using the in-memory
//! `HeadlessTerminal` emulator.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::fs::File;
use std::io::Write;
use std::process::Command;
use std::sync::Arc;
use tempfile::TempDir;
use tigrs_git::{
    BlameLine, BlameResult, BlobContent, CommitDiff, CommitSummary, DiffHunk, DiffLineKind,
    DiffSummaryStats, FileChangeStatus, FileDiff, GitEngine, HunkLine, ObjectId, ParentIds,
    RefEntry, RefKind, ReflogEntry, StashEntry, StatusItem, StatusReport, StatusSection, TreeEntry,
    TreeEntryKind, TreeListing,
};
use tigrs_ui::{
    AppState, BlameView, BlobView, CellAttrs, ChangesKind, ChangesRow, Color, DiffView, Flow,
    GrepMatch, GrepView, HeadlessTerminal, HelpView, LineGraphics, MainView, PagerView, PromptKind,
    ReflogView, RefsView, StashView, StatusView, TreeView, ViewKind, ViewOptions,
    handle_event_with_dimensions, render_active,
};

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn make_oid(hex_byte: u8) -> ObjectId {
    let bytes = [hex_byte; 20];
    ObjectId::from_bytes_or_panic(&bytes)
}

fn sample_commits(count: usize) -> Vec<CommitSummary> {
    (0..count)
        .map(|i| {
            let id = make_oid(i as u8 + 1);
            CommitSummary {
                id,
                parents: ParentIds::new(),
                author_name: Arc::from(format!("Author {}", i + 1)),
                author_time_secs: 1_700_000_000 + (i as i64 * 86400),
                summary: Box::from(format!("Commit subject message number {}", i + 1)),
            }
        })
        .collect()
}

fn sample_commit_diff() -> CommitDiff {
    CommitDiff {
        commit_id: make_oid(0x1a),
        parent_ids: vec![],
        author_name: Arc::from("Linus Torvalds"),
        author_email: Arc::from("torvalds@kernel.org"),
        author_date: "Sun Sep 13 00:00:00 2026 +0000".to_string(),
        committer_name: Arc::from("Linus Torvalds"),
        committer_email: Arc::from("torvalds@kernel.org"),
        committer_date: "Sun Sep 13 00:00:00 2026 +0000".to_string(),
        title: Arc::from("Initial release of Linux"),
        body: Some(
            "Detailed description of initial release.\nSupports 386 AT hardware.".to_string(),
        ),
        files: vec![FileDiff {
            path: "Makefile".to_string(),
            status: FileChangeStatus::Modified,
            old_id: Some(make_oid(0x10)),
            new_id: Some(make_oid(0x11)),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_644),
            is_binary: false,
            additions: 2,
            deletions: 1,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_len: 3,
                new_start: 1,
                new_len: 4,
                func_context: Some("subsystem configuration".to_string()),
                lines: vec![
                    HunkLine {
                        kind: DiffLineKind::Context,
                        content: "# Makefile for Linux".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Remove,
                        content: "VERSION = 0.01".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Add,
                        content: "VERSION = 1.0.0".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Add,
                        content: "EXTRAVERSION = -rc1".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Context,
                        content: "CC = gcc".to_string(),
                        no_newline_at_eof: false,
                    },
                ],
            }],
        }],
        stats: DiffSummaryStats {
            files_changed: 1,
            insertions: 2,
            deletions: 1,
        },
    }
}

fn sample_status_report() -> StatusReport {
    StatusReport {
        staged: vec![StatusItem::new(
            'M',
            StatusSection::Staged,
            "src/main.rs",
            None,
        )],
        unstaged: vec![StatusItem::new(
            'D',
            StatusSection::Unstaged,
            "old.txt",
            None,
        )],
        untracked: vec![StatusItem::new(
            '?',
            StatusSection::Untracked,
            "new_file.rs",
            None,
        )],
        unmerged: Vec::new(),
        branch: "main".to_string(),
        head_commit: Some(make_oid(1)),
    }
}

// ----------------------------------------------------------------------------
// 1. Main View Snapshots
// ----------------------------------------------------------------------------

#[test]
fn test_snapshot_main_view_standard_80x24() {
    let mut main = MainView::new("main".to_string());
    main.append_commits(sample_commits(5));
    main.set_finished();

    let mut term = HeadlessTerminal::new(80, 24);
    main.render(&mut term, 80, 24).expect("render");

    // Line 0: Header
    term.assert_line_contains(0, "[main] main - 5 commits loaded");
    assert!(term.is_bold(1, 0));

    // Line 1: First commit (selected, reverse video)
    term.assert_line_contains(1, "Author 1");
    term.assert_line_contains(1, "Commit subject message number 1");
    assert!(term.line_has_reverse(1));

    // Line 2: Second commit (unselected, normal video)
    term.assert_line_contains(2, "Author 2");
    assert!(!term.line_has_reverse(2));

    // Line 23: Status line
    term.assert_line_contains(23, "[main] line 1 of 5 (20%)");
    assert!(term.line_has_reverse(23));
}

#[test]
fn test_snapshot_main_view_wide_120x40() {
    let mut main = MainView::new("release/2.0".to_string());
    main.append_commits(sample_commits(10));
    main.set_finished();

    let mut term = HeadlessTerminal::new(120, 40);
    main.render(&mut term, 120, 40).expect("render");

    term.assert_line_contains(0, "[main] release/2.0 - 10 commits loaded");
    term.assert_line_contains(1, "Author 1");
    term.assert_line_contains(39, "[release/2.0] line 1 of 10 (10%)");
}

#[test]
fn test_snapshot_main_view_loading_and_empty() {
    // 1. Loading state (is_loading = true by default)
    let loading_main = MainView::new("main".to_string());
    let mut term = HeadlessTerminal::new(80, 24);
    loading_main.render(&mut term, 80, 24).expect("render");

    term.assert_line_contains(0, "(loading...)");
    term.assert_line_contains(1, "Loading commits...");

    // 2. Empty state
    let mut empty_main = MainView::new("main".to_string());
    empty_main.set_finished();
    term.clear();
    empty_main.render(&mut term, 80, 24).expect("render");

    term.assert_line_contains(0, "0 commits loaded");
    term.assert_line_contains(1, "No commits found.");
}

// ----------------------------------------------------------------------------
// 2. Diff View Snapshots
// ----------------------------------------------------------------------------

#[test]
fn test_snapshot_diff_view_commit_standard_80x24() {
    let diff = sample_commit_diff();
    let view = DiffView::new(diff);

    let mut term = HeadlessTerminal::new(80, 24);
    view.render(&mut term, 80, 24).expect("render");

    // Header bar (row 0): bold + reverse
    term.assert_line_contains(0, "[diff] 1a1a1a1 - Initial release of Linux");
    assert!(term.is_bold(0, 0));
    assert!(term.is_reverse(0, 0));

    // Commit metadata lines
    term.assert_line_contains(1, "commit 1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a");
    term.assert_line_contains(2, "Author:     Linus Torvalds <torvalds@kernel.org>");
    term.assert_line_contains(3, "AuthorDate: Sun Sep 13 00:00:00 2026 +0000");
    term.assert_line_contains(4, "Commit:     Linus Torvalds <torvalds@kernel.org>");
    term.assert_line_contains(5, "CommitDate: Sun Sep 13 00:00:00 2026 +0000");
    term.assert_line_contains(7, "Initial release of Linux");

    // Diffstat
    term.assert_line_contains(14, "1 file changed, 2 insertions(+), 1 deletion(-)");

    // Single-line file banner & human hunk separator (default Banner presentation)
    term.assert_line_contains(16, "─ 1/1 ─ M ─ Makefile ──── +2 -1 ▏▁▏ ─");
    term.assert_line_contains(17, "── lines 1..4 ──  ┃ Make ▸ subsystem configuration");
    term.assert_line_contains(18, "# Makefile for Linux");
    term.assert_line_contains(19, "-VERSION = 0.01");

    // Status bar (row 23)
    term.assert_line_contains(23, "[diff] line 1 of 22 (4%)");
    assert!(term.is_reverse(0, 23));
}

#[test]
fn test_snapshot_diff_view_selected_hunk_and_line() {
    let diff = sample_commit_diff();
    let mut view = DiffView::new(diff);

    let mut term = HeadlessTerminal::new(80, 25);

    // Jump to hunk header
    view.next_hunk(25);
    term.clear();
    view.render(&mut term, 80, 25).expect("render");

    // The hunk header line should be selected with reverse attribute
    let hunk_text = "── lines 1..4 ──";
    let (hunk_x, hunk_y) = term.find_text(hunk_text).expect("find hunk header");
    assert!(term.is_reverse(hunk_x, hunk_y));
    // Copy fidelity ('y') on the hunk separator still returns the real @@ header
    assert_eq!(
        view.yank_text_at_cursor().as_deref(),
        Some("@@ -1,3 +1,4 @@ subsystem configuration")
    );

    // Move cursor down 3 lines to '+VERSION = 1.0.0'
    view.move_down(3, 25);
    term.clear();
    view.render(&mut term, 80, 25).expect("render");

    let add_text = "+VERSION = 1.0.0";
    let (add_x, add_y) = term.find_text(add_text).expect("find addition line");
    assert!(term.is_reverse(add_x, add_y));
}

#[test]
fn test_snapshot_diff_view_status_staging_hints() {
    let diff = sample_commit_diff();

    // 1. Staged item: hints unstage
    let staged_item = StatusItem::new('M', StatusSection::Staged, "Makefile", None);
    let staged_view = DiffView::new_status(diff.clone(), staged_item);
    let mut term = HeadlessTerminal::new(80, 24);
    staged_view.render(&mut term, 80, 24).expect("render");

    term.assert_line_contains(23, "[status-diff]");
    term.assert_line_contains(23, "u: unstage hunk, 1: unstage line");

    // 2. Unstaged item: hints stage
    let unstaged_item = StatusItem::new('M', StatusSection::Unstaged, "Makefile", None);
    let unstaged_view = DiffView::new_status(diff.clone(), unstaged_item);
    term.clear();
    unstaged_view.render(&mut term, 80, 24).expect("render");

    term.assert_line_contains(23, "[status-diff]");
    term.assert_line_contains(23, "u: stage hunk, 1: stage line");

    // 3. Untracked item: hints stage hunk
    let untracked_item = StatusItem::new('?', StatusSection::Untracked, "Makefile", None);
    let untracked_view = DiffView::new_status(diff, untracked_item);
    term.clear();
    untracked_view.render(&mut term, 80, 24).expect("render");

    term.assert_line_contains(23, "[status-diff]");
    term.assert_line_contains(23, "u: stage hunk");
}

// ----------------------------------------------------------------------------
// 3. Status View Snapshots
// ----------------------------------------------------------------------------

#[test]
fn test_snapshot_status_view_dirty_repo() {
    let report = sample_status_report();
    let view = StatusView::new(report);

    let mut term = HeadlessTerminal::new(80, 24);
    view.render(&mut term, 80, 24).expect("render");

    // Header
    term.assert_line_contains(0, "[status] main - 3 changes");

    // Sections
    term.assert_line_contains(1, "Changes to be committed:");
    term.assert_line_contains(2, "modified");
    term.assert_line_contains(2, "src/main.rs");
    term.assert_line_contains(4, "Changes not staged for commit:");
    term.assert_line_contains(5, "deleted");
    term.assert_line_contains(5, "old.txt");
    term.assert_line_contains(7, "Untracked files:");
    term.assert_line_contains(8, "untracked");
    term.assert_line_contains(8, "new_file.rs");

    // Cursor on row 2 (src/main.rs) has reverse video
    assert!(term.line_has_reverse(2));

    // Status bar
    term.assert_line_contains(23, "[status] line 2 of 8");
    term.assert_line_contains(23, "src/main.rs");
}

#[test]
fn test_snapshot_status_view_clean_repo() {
    let report = StatusReport::default();
    let view = StatusView::new(report);

    let mut term = HeadlessTerminal::new(80, 24);
    view.render(&mut term, 80, 24).expect("render");

    term.assert_line_contains(0, "[status] HEAD - 0 changes");
    term.assert_line_contains(1, "nothing to commit, working tree clean");
    term.assert_line_contains(23, "[status] line 1 of 1 (100%)");
}

// ----------------------------------------------------------------------------
// 4. Tree View Snapshots
// ----------------------------------------------------------------------------

#[test]
fn test_snapshot_tree_view_root() {
    let listing = TreeListing {
        commit_oid: make_oid(0x2b),
        path: String::new(),
        parent_path: None,
        entries: vec![
            TreeEntry {
                name: "src".to_string(),
                path: "src".to_string(),
                kind: TreeEntryKind::Tree,
                oid: make_oid(0x01),
                mode: 0o040_000,
                size: None,
            },
            TreeEntry {
                name: "tests".to_string(),
                path: "tests".to_string(),
                kind: TreeEntryKind::Tree,
                oid: make_oid(0x02),
                mode: 0o040_000,
                size: None,
            },
            TreeEntry {
                name: "Cargo.toml".to_string(),
                path: "Cargo.toml".to_string(),
                kind: TreeEntryKind::Blob,
                oid: make_oid(0x03),
                mode: 0o100_644,
                size: Some(1250),
            },
            TreeEntry {
                name: "README.md".to_string(),
                path: "README.md".to_string(),
                kind: TreeEntryKind::Blob,
                oid: make_oid(0x04),
                mode: 0o100_644,
                size: Some(3400),
            },
        ],
    };

    let view = TreeView::new(listing);
    let mut term = HeadlessTerminal::new(80, 24);
    view.render(&mut term, 80, 24).expect("render");

    // Header
    term.assert_line_contains(0, "[tree] 2b2b2b2:/ - 4 entries");

    // Entries (directories with trailing '/')
    term.assert_line_contains(1, "src/");
    term.assert_line_contains(2, "tests/");
    term.assert_line_contains(3, "Cargo.toml");
    term.assert_line_contains(4, "README.md");

    // First entry has reverse selection
    assert!(term.line_has_reverse(1));

    // Status bar
    term.assert_line_contains(23, "[tree] line 1 of 4 (25%) - directory");
}

#[test]
fn test_snapshot_tree_view_subdirectory() {
    let listing = TreeListing {
        commit_oid: make_oid(0x2b),
        path: "src".to_string(),
        parent_path: Some(String::new()),
        entries: vec![
            TreeEntry {
                name: "main.rs".to_string(),
                path: "src/main.rs".to_string(),
                kind: TreeEntryKind::Blob,
                oid: make_oid(0x11),
                mode: 0o100_644,
                size: Some(420),
            },
            TreeEntry {
                name: "lib.rs".to_string(),
                path: "src/lib.rs".to_string(),
                kind: TreeEntryKind::Blob,
                oid: make_oid(0x12),
                mode: 0o100_644,
                size: Some(890),
            },
        ],
    };

    let view = TreeView::new(listing);
    let mut term = HeadlessTerminal::new(80, 24);
    view.render(&mut term, 80, 24).expect("render");

    // Header indicates subdirectory
    term.assert_line_contains(0, "[tree] 2b2b2b2:/src - 2 entries");

    // Parent directory row '..'
    term.assert_line_contains(1, "..");
    term.assert_line_contains(2, "main.rs");
    term.assert_line_contains(3, "lib.rs");
}

// ----------------------------------------------------------------------------
// 5. Blob View Snapshots
// ----------------------------------------------------------------------------

#[test]
fn test_snapshot_blob_view_highlighted_code() {
    let code_lines = vec![
        "fn main() {".to_string(),
        "    let message = \"Hello, World!\";".to_string(),
        "    println!(\"{}\", message);".to_string(),
        "}".to_string(),
    ];
    let blob = BlobContent {
        oid: make_oid(0x3c),
        path: "src/main.rs".to_string(),
        size: 80,
        is_binary: false,
        lines: code_lines.into(),
    };
    let view = BlobView::new(make_oid(0x3c), blob);

    let mut term = HeadlessTerminal::new(80, 24);
    view.render(&mut term, 80, 24).expect("render");

    // Header
    term.assert_line_contains(0, "[blob] 3c3c3c3:src/main.rs - 4 lines");

    // Code lines with gutter
    term.assert_line_contains(1, "1 > fn main() {");
    term.assert_line_contains(2, "2 │     let message = \"Hello, World!\";");
    term.assert_line_contains(3, "3 │     println!(\"{}\", message);");
    term.assert_line_contains(4, "4 │ }");

    // First line has cursor reverse selection
    assert!(term.line_has_reverse(1));

    // Status bar
    term.assert_line_contains(23, "[blob] line 1 of 4 (25%) - src/main.rs [q: back]");
}

#[test]
fn test_snapshot_blob_view_binary_file() {
    let blob = BlobContent {
        oid: make_oid(0x3c),
        path: "target/app.bin".to_string(),
        size: 1024,
        is_binary: true,
        lines: tigrs_core::LineBuffer::empty(),
    };
    let view = BlobView::new(make_oid(0x3c), blob);

    let mut term = HeadlessTerminal::new(80, 24);
    view.render(&mut term, 80, 24).expect("render");

    term.assert_line_contains(0, "[blob] 3c3c3c3:target/app.bin (binary, 1.0K)");
    term.assert_line_contains(1, "[Binary file: 1024 bytes - cannot display]");
    term.assert_line_contains(23, "[blob] binary (1.0K) - target/app.bin [q: back]");
}

// ----------------------------------------------------------------------------
// 6. Blame View Snapshots
// ----------------------------------------------------------------------------

#[test]
fn test_snapshot_blame_view_standard() {
    let commit1 = make_oid(0x11);
    let commit2 = make_oid(0x22);

    let res = BlameResult {
        commit_id: commit2,
        path: "src/lib.rs".to_string(),
        is_binary: false,
        lines: vec![
            BlameLine {
                line_number: 1,
                commit_id: commit1,
                short_commit_id: Arc::from("11111111"),
                author: Arc::from("Alice"),
                author_date: Arc::from("2023-11-14"),
                summary: Arc::from("Initial scaffold"),
                content: "pub fn add(a: i32, b: i32) -> i32 {".to_string(),
                is_hunk_start: true,
                parent_commit_id: None,
                source_path: None,
                source_line_number: 1,
            },
            BlameLine {
                line_number: 2,
                commit_id: commit2,
                short_commit_id: Arc::from("22222222"),
                author: Arc::from("Bob"),
                author_date: Arc::from("2023-11-15"),
                summary: Arc::from("Add implementation"),
                content: "    a + b".to_string(),
                is_hunk_start: true,
                parent_commit_id: Some(commit1),
                source_path: None,
                source_line_number: 2,
            },
            BlameLine {
                line_number: 3,
                commit_id: commit1,
                short_commit_id: Arc::from("11111111"),
                author: Arc::from("Alice"),
                author_date: Arc::from("2023-11-14"),
                summary: Arc::from("Initial scaffold"),
                content: "}".to_string(),
                is_hunk_start: false,
                parent_commit_id: None,
                source_path: None,
                source_line_number: 3,
            },
        ],
    };

    let view = BlameView::from_result(res);
    let mut term = HeadlessTerminal::new(80, 24);
    view.render(&mut term, 80, 24).expect("render");

    // Header
    term.assert_line_contains(0, "[blame] 2222222:src/lib.rs - 3 lines");

    // Annotations: 8 hex hash, author, date, gutter, code
    term.assert_line_contains(
        1,
        "11111111 Alice        2023-11-14   1 > pub fn add(a: i32, b: i32) -> i32 {",
    );
    term.assert_line_contains(2, "22222222 Bob          2023-11-15   2 │     a + b");
    term.assert_line_contains(3, "11111111 Alice        2023-11-14   3 │ }");

    // First line has cursor reverse selection
    assert!(term.line_has_reverse(1));

    // Status bar with commit summary
    term.assert_line_contains(
        23,
        "[blame] 1/3 (33%) - \"Initial scaffold\" [Enter: diff, ,: parent, q: back]",
    );
}

#[test]
fn test_snapshot_blame_view_binary_file() {
    let res = BlameResult {
        commit_id: make_oid(0x55),
        path: "firmware.bin".to_string(),
        is_binary: true,
        lines: Vec::new(),
    };

    let view = BlameView::from_result(res);
    let mut term = HeadlessTerminal::new(80, 24);
    view.render(&mut term, 80, 24).expect("render");

    term.assert_line_contains(0, "[blame] 5555555:firmware.bin (binary)");
    term.assert_line_contains(1, "[Binary file: cannot display blame annotations]");
    term.assert_line_contains(23, "[blame] binary - firmware.bin [q: back]");
}

// ----------------------------------------------------------------------------
// 7. Full View-Stack Navigation & Snapshot Transitions
// ----------------------------------------------------------------------------

#[test]
fn test_snapshot_full_view_stack_transitions() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path();

    // Initialize git repository
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

    // Commit 1: add src/main.rs
    std::fs::create_dir_all(p.join("src")).unwrap();
    let file_path = p.join("src/main.rs");
    let mut f = File::create(&file_path).unwrap();
    writeln!(f, "fn main() {{\n    println!(\"v1\");\n}}").unwrap();
    drop(f);

    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "src/main.rs"])
        .current_dir(p)
        .output()
        .unwrap();
    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "Commit 1: initial main"])
        .current_dir(p)
        .output()
        .unwrap();

    // Commit 2: update src/main.rs
    let mut f = File::create(&file_path).unwrap();
    writeln!(f, "fn main() {{\n    println!(\"v2\");\n}}").unwrap();
    drop(f);

    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-am", "Commit 2: update main"])
        .current_dir(p)
        .output()
        .unwrap();

    let engine = GitEngine::open(Some(p)).unwrap();
    let head_id = engine.head_commit_id().unwrap();

    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some(MainView::new("main".to_string())),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    if let Some(ref mut main) = app.views.main_view
        && let Some(ref eng) = app.engine
    {
        let (_src, token) = tigrs_core::cancel::CancellationToken::new();
        if let Ok(iter) = eng.stream_commits(Some(head_id), Some(10), token) {
            for batch in iter.flatten() {
                main.append_commits(batch);
            }
        }
        main.set_finished();
    }
    app.push_view(ViewKind::Main);

    let mut term = HeadlessTerminal::new(80, 24);

    // Step 1: MainView snapshot
    render_active(&app, &mut term, 80, 24).unwrap();
    term.assert_line_contains(0, "[main] main - 2 commits loaded");
    term.assert_line_contains(1, "Commit 2: update main");

    // Step 2: Enter -> DiffView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    term.clear();
    render_active(&app, &mut term, 80, 24).unwrap();
    term.assert_line_contains(0, "[main]");
    term.assert_line_contains(12, "[diff]");
    term.assert_line_contains(12, "Commit 2: update main");

    // Step 3: 't' -> TreeView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('t')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Tree));
    term.clear();
    render_active(&app, &mut term, 80, 24).unwrap();
    term.assert_line_contains(0, "[tree]");
    term.assert_line_contains(1, "src/");

    // Step 4: Enter on 'src/' -> Subdirectory TreeView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    term.clear();
    render_active(&app, &mut term, 80, 24).unwrap();
    term.assert_line_contains(0, "[tree]");
    term.assert_line_contains(1, "..");
    term.assert_line_contains(2, "main.rs");

    // Step 5: Down to 'main.rs', then Enter -> BlobView (split with TreeView)
    tigrs_ui::handle_event(&key(KeyCode::Down), &mut app, 24);
    let flow = tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Blob));
    term.clear();
    app.invalidate_screen();
    render_active(&app, &mut term, 80, 24).unwrap();
    term.assert_line_contains(0, "[tree]");
    term.assert_line_contains(12, "[blob]");
    term.assert_line_contains(12, "src/main.rs");
    term.assert_line_contains(14, "println!(\"v2\")");

    // Step 6: 'b' -> BlameView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('b')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Blame));
    term.clear();
    render_active(&app, &mut term, 80, 24).unwrap();
    term.assert_line_contains(0, "[blame]");
    term.assert_line_contains(0, "src/main.rs");

    // Step 7: Move down to line 2 (println!("v2")), press ',' -> Parent BlameView
    tigrs_ui::handle_event(&key(KeyCode::Down), &mut app, 24);
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char(',')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Blame));
    term.clear();
    render_active(&app, &mut term, 80, 24).unwrap();
    term.assert_line_contains(2, "println!(\"v1\")");

    // Step 8: Backspace -> Pop blame history back to Commit 2 BlameView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Backspace), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    term.clear();
    render_active(&app, &mut term, 80, 24).unwrap();
    term.assert_line_contains(2, "println!(\"v2\")");

    // Step 9: 'q' -> Pops back to BlobView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Blob));

    // Step 10: 'q' -> Pops back to TreeView (still in src/ subdir)
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Tree));

    // Step 11: 'q' -> In TreeView (src/), ascends to parent root directory
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Tree));

    // Step 12: 'q' -> In TreeView (/), pops back to DiffView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));

    // Step 13: 'q' -> Pops back to MainView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    // Step 14: 'q' -> Quits application
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Quit);
}

// ----------------------------------------------------------------------------
// 8. Cell Attributes & Color Verification
// ----------------------------------------------------------------------------

#[test]
fn test_headless_cell_attributes_and_color_inspection() {
    let diff = sample_commit_diff();
    let view = DiffView::new(diff);

    let mut term = HeadlessTerminal::new(80, 25);
    view.render(&mut term, 80, 25).expect("render");

    // 1. Title bar cell attributes (row 0)
    let title_cell = term.cell(0, 0).expect("title cell");
    assert_eq!(title_cell.ch, '[');
    assert!(title_cell.attrs.contains(CellAttrs::BOLD));
    assert!(title_cell.attrs.contains(CellAttrs::REVERSE));

    // 2. Addition line color inspection (+VERSION = 1.0.0)
    let (add_x, add_y) = term
        .find_text("+VERSION = 1.0.0")
        .expect("find addition line");
    let add_cell = term.cell(add_x, add_y).expect("addition cell");
    assert_eq!(add_cell.fg, Some(Color::Green));

    // 3. Deletion line color inspection (-VERSION = 0.01)
    let (del_x, del_y) = term
        .find_text("-VERSION = 0.01")
        .expect("find deletion line");
    let del_cell = term.cell(del_x, del_y).expect("deletion cell");
    assert_eq!(del_cell.fg, Some(Color::Red));

    // 4. Commit metadata header colors
    let (author_x, author_y) = term.find_text("Author:").expect("find Author header");
    let author_cell = term.cell(author_x, author_y).expect("author cell");
    assert_eq!(author_cell.fg, Some(Color::Cyan));

    let (date_x, date_y) = term
        .find_text("AuthorDate:")
        .expect("find AuthorDate header");
    let date_cell = term.cell(date_x, date_y).expect("date cell");
    assert_eq!(date_cell.fg, Some(Color::Yellow));

    // 5. Commit message title (White + Bold)
    let (title_x, title_y) = term
        .find_text("    Initial release of Linux")
        .expect("find commit title in body");
    let title_msg_cell = term.cell(title_x + 4, title_y).expect("commit title cell");
    assert_eq!(title_msg_cell.fg, Some(Color::White));
    assert!(title_msg_cell.attrs.contains(CellAttrs::BOLD));

    // 6. Commit message body (Default context color, not bold)
    let (body_x, body_y) = term
        .find_text("Detailed description of initial release.")
        .expect("find commit body");
    let body_cell = term.cell(body_x, body_y).expect("commit body cell");
    assert_eq!(body_cell.fg, None);
    assert!(!body_cell.attrs.contains(CellAttrs::BOLD));
}

// ----------------------------------------------------------------------------
// 9. Viewport Resizing Invariants
// ----------------------------------------------------------------------------

#[test]
fn test_headless_viewport_resizing_compact_and_ultrawide() {
    let mut main = MainView::new("main".to_string());
    main.append_commits(sample_commits(20));

    // 1. Compact viewport (40x10)
    let mut compact_term = HeadlessTerminal::new(40, 10);
    main.render(&mut compact_term, 40, 10)
        .expect("compact render");
    assert_eq!(compact_term.line_text_raw(0).len(), 40);
    compact_term.assert_line_contains(0, "[main]");
    compact_term.assert_line_contains(9, "[main] line 1 of 20");

    // 2. Ultra-wide viewport (160x50)
    let mut wide_term = HeadlessTerminal::new(160, 50);
    main.render(&mut wide_term, 160, 50).expect("wide render");
    assert_eq!(wide_term.line_text_raw(0).len(), 160);
    wide_term.assert_line_contains(0, "[main] main - 20 commits loaded");
    wide_term.assert_line_contains(49, "[main] line 1 of 20 (5%)");
}

// ----------------------------------------------------------------------------
// 10. Graph DAG Layout and Box-Drawing Visual Tests
// ----------------------------------------------------------------------------

#[test]
#[allow(clippy::similar_names)]
fn test_snapshot_main_view_graph_box_drawing_utf8() {
    let mut main = MainView::new("main".to_string());

    let root_oid = make_oid(1);
    let branch_a_oid = make_oid(2);
    let branch_b_oid = make_oid(3);
    let merge_oid = make_oid(4);

    let c_root = CommitSummary {
        id: root_oid,
        parents: ParentIds::new(),
        author_name: Arc::from("Alice"),
        author_time_secs: 1_700_000_000,
        summary: Box::from("Initial root commit"),
    };
    let c_branch_a = CommitSummary {
        id: branch_a_oid,
        parents: ParentIds::from_slice(&[root_oid]),
        author_name: Arc::from("Bob"),
        author_time_secs: 1_700_000_100,
        summary: Box::from("Commit on branch A"),
    };
    let c_branch_b = CommitSummary {
        id: branch_b_oid,
        parents: ParentIds::from_slice(&[root_oid]),
        author_name: Arc::from("Charlie"),
        author_time_secs: 1_700_000_200,
        summary: Box::from("Commit on branch B"),
    };
    let c_merge = CommitSummary {
        id: merge_oid,
        parents: ParentIds::from_slice(&[branch_a_oid, branch_b_oid]),
        author_name: Arc::from("Alice"),
        author_time_secs: 1_700_000_300,
        summary: Box::from("Merge branch B into branch A"),
    };

    // Append in reverse chronological order (merge, branch_a, branch_b, root)
    main.append_commits(vec![c_merge, c_branch_a, c_branch_b, c_root]);
    main.set_finished();

    let mut term = HeadlessTerminal::new(80, 24);
    let opts = ViewOptions {
        line_graphics: LineGraphics::Utf8,
        ..Default::default()
    };
    main.render_with_options(&mut term, 80, 24, &opts)
        .expect("render");

    // Line 1: Merge commit (selected) -> "●──╮"
    term.assert_line_contains(1, "●──╮");
    term.assert_line_contains(1, "Merge branch B into branch A");

    // Line 2: Branch A -> " ∙ │"
    term.assert_line_contains(2, " ∙ │");
    term.assert_line_contains(2, "Commit on branch A");

    // Line 3: Branch B joining into root -> " │─╯"
    term.assert_line_contains(3, " │─╯");
    term.assert_line_contains(3, "Commit on branch B");

    // Line 4: Root commit -> " ◎"
    term.assert_line_contains(4, " ◎");
    term.assert_line_contains(4, "Initial root commit");
}

#[test]
#[allow(clippy::similar_names)]
fn test_snapshot_main_view_graph_ascii_fallback() {
    let mut main = MainView::new("main".to_string());

    let root_oid = make_oid(1);
    let branch_a_oid = make_oid(2);
    let branch_b_oid = make_oid(3);
    let merge_oid = make_oid(4);

    let c_root = CommitSummary {
        id: root_oid,
        parents: ParentIds::new(),
        author_name: Arc::from("Alice"),
        author_time_secs: 1_700_000_000,
        summary: Box::from("Initial root commit"),
    };
    let c_branch_a = CommitSummary {
        id: branch_a_oid,
        parents: ParentIds::from_slice(&[root_oid]),
        author_name: Arc::from("Bob"),
        author_time_secs: 1_700_000_100,
        summary: Box::from("Commit on branch A"),
    };
    let c_branch_b = CommitSummary {
        id: branch_b_oid,
        parents: ParentIds::from_slice(&[root_oid]),
        author_name: Arc::from("Charlie"),
        author_time_secs: 1_700_000_200,
        summary: Box::from("Commit on branch B"),
    };
    let c_merge = CommitSummary {
        id: merge_oid,
        parents: ParentIds::from_slice(&[branch_a_oid, branch_b_oid]),
        author_name: Arc::from("Alice"),
        author_time_secs: 1_700_000_300,
        summary: Box::from("Merge branch B into branch A"),
    };

    main.append_commits(vec![c_merge, c_branch_a, c_branch_b, c_root]);
    main.set_finished();

    let mut term = HeadlessTerminal::new(80, 24);
    let opts = ViewOptions {
        line_graphics: LineGraphics::Ascii,
        ..Default::default()
    };
    main.render_with_options(&mut term, 80, 24, &opts)
        .expect("render");

    // Line 1: Merge commit -> "M--."
    term.assert_line_contains(1, "M--.");
    term.assert_line_contains(1, "Merge branch B into branch A");

    // Line 2: Branch A -> " * |"
    term.assert_line_contains(2, " * |");

    // Line 3: Branch B joining into root -> " |-'"
    term.assert_line_contains(3, " |-'");

    // Line 4: Root commit -> " I"
    term.assert_line_contains(4, " I");
}

#[test]
fn test_snapshot_main_view_graph_runtime_toggle() {
    let mut main = MainView::new("main".to_string());
    let root_oid = make_oid(1);
    let child_oid = make_oid(2);

    let c_root = CommitSummary {
        id: root_oid,
        parents: ParentIds::new(),
        author_name: Arc::from("Alice"),
        author_time_secs: 1_700_000_000,
        summary: Box::from("Initial"),
    };
    let c_child = CommitSummary {
        id: child_oid,
        parents: ParentIds::from_slice(&[root_oid]),
        author_name: Arc::from("Alice"),
        author_time_secs: 1_700_000_100,
        summary: Box::from("Child"),
    };
    main.append_commits(vec![c_child, c_root]);
    main.set_finished();

    let mut term = HeadlessTerminal::new(80, 24);
    let mut opts = ViewOptions::default();

    // Default is UTF-8
    assert_eq!(opts.line_graphics, LineGraphics::Utf8);
    main.render_with_options(&mut term, 80, 24, &opts)
        .expect("render");
    term.assert_line_contains(1, " ∙");

    // Toggle to ASCII
    opts.toggle_line_graphics();
    assert_eq!(opts.line_graphics, LineGraphics::Ascii);
    term.clear();
    main.render_with_options(&mut term, 80, 24, &opts)
        .expect("render");
    term.assert_line_contains(1, " *");

    // Toggle back to UTF-8
    opts.toggle_line_graphics();
    assert_eq!(opts.line_graphics, LineGraphics::Utf8);
    term.clear();
    main.render_with_options(&mut term, 80, 24, &opts)
        .expect("render");
    term.assert_line_contains(1, " ∙");
}

#[test]
fn test_headless_split_view_scrolling_rolls_down_content() {
    let mut main = MainView::new("main".to_string());
    main.append_commits(sample_commits(25));
    main.set_finished();

    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some(main),
            diff_view: Some(DiffView::new(sample_commit_diff())),
            ..Default::default()
        },
        ..Default::default()
    };
    app.push_view(ViewKind::Main);
    app.push_view(ViewKind::Diff);

    // Focus MainView (top pane)
    handle_event_with_dimensions(&key(KeyCode::Tab), &mut app, 80, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    // Terminal 80x24: top pane occupies lines 0..12.
    // Line 0 is header, lines 1..=10 are commits, line 11 is status/border.
    let mut term = HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term, 80, 24).expect("render");

    // Line 1 should have commit 1, Line 10 should have commit 10
    term.assert_line_contains(1, "Commit subject message number 1");
    term.assert_line_contains(10, "Commit subject message number 10");

    // Move cursor down 9 times to row 9 (still fits within visible height 10, offset remains 0)
    for _ in 0..9 {
        handle_event_with_dimensions(&key(KeyCode::Down), &mut app, 80, 24);
    }
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 9);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 0);

    // 10th move: cursor steps to index 10.
    // The view MUST roll down: scroll_offset becomes 1!
    handle_event_with_dimensions(&key(KeyCode::Down), &mut app, 80, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 10);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 1);

    // Re-render and verify the buffer content actually rolled down!
    term.clear();
    render_active(&app, &mut term, 80, 24).expect("render");

    // Commit 1 rolled off top, so Line 1 now contains Commit 2!
    term.assert_line_contains(1, "Commit subject message number 2");
    // Line 10 now contains Commit 11!
    term.assert_line_contains(10, "Commit subject message number 11");
    // Commit 1 is no longer visible in the top pane
    assert!(!term.line_text_raw(1).contains("number 1 "));

    // Test mouse wheel scroll down by 3 lines (offset goes from 1 to 4)
    let mouse_down = Event::Mouse(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::ScrollDown,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    handle_event_with_dimensions(&mouse_down, &mut app, 80, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 4);

    term.clear();
    render_active(&app, &mut term, 80, 24).expect("render");
    // Line 1 now starts at commit index 4 (number 5)
    term.assert_line_contains(1, "Commit subject message number 5");
}

#[test]
fn test_headless_vertical_split_wheel_scrolls_pane_under_pointer() {
    let mut main = MainView::new("main".to_string());
    main.append_commits(sample_commits(25));
    main.set_finished();

    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some(main),
            diff_view: Some(DiffView::new(sample_commit_diff())),
            ..Default::default()
        },
        ..Default::default()
    };
    app.push_view(ViewKind::Main);
    app.push_view(ViewKind::Diff);
    app.options.vertical_split = true;

    // The Diff pane (right) holds focus; the Main pane (left) does not.
    assert_eq!(app.active_view(), Some(ViewKind::Diff));

    // 160 columns split into a 79-column left pane, a separator, and an 80-column right pane.
    let layout = app.view_layout(160, 24).expect("layout");
    let left = layout.pane_for(ViewKind::Main).expect("left pane");
    assert_eq!((left.x, left.width), (0, 79));

    let mut term = HeadlessTerminal::new(160, 24);
    render_active(&app, &mut term, 160, 24).expect("render");
    term.assert_line_contains(1, "Commit subject message number 1");
    assert_eq!(
        term.line_text_raw(5).chars().nth(79),
        Some('│'),
        "separator column must sit exactly where the layout says it does"
    );

    // Wheel over the unfocused left pane must roll the left pane, not the focused right pane.
    let wheel_over_left_pane = Event::Mouse(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::ScrollDown,
        column: 10,
        row: 12,
        modifiers: KeyModifiers::NONE,
    });
    handle_event_with_dimensions(&wheel_over_left_pane, &mut app, 160, 24);

    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 3);
    assert_eq!(app.views.diff_view.as_ref().unwrap().scroll_offset(), 0);
    assert_eq!(
        app.active_view(),
        Some(ViewKind::Diff),
        "wheel scrolling must not move focus"
    );

    term.clear();
    render_active(&app, &mut term, 160, 24).expect("render");
    term.assert_line_contains(1, "Commit subject message number 4");
    assert!(!term.line_text_raw(1).contains("number 1 "));
}

// ----------------------------------------------------------------------------
// 9. Uncommitted Changes Rows in the Main View
// ----------------------------------------------------------------------------

/// Builds a repository with one commit plus staged, unstaged, and untracked work.
///
/// Returns the `TempDir` (which must be kept alive for the duration of the test)
/// alongside an engine opened on it.
fn dirty_repo() -> (TempDir, GitEngine) {
    let dir = TempDir::new().unwrap();
    let p = dir.path();

    for args in [
        vec!["init"],
        vec!["config", "user.name", "Test User"],
        vec!["config", "user.email", "test@example.com"],
    ] {
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(&args)
            .current_dir(p)
            .output()
            .unwrap();
    }

    // Committed baseline.
    let tracked = p.join("tracked.txt");
    writeln!(File::create(&tracked).unwrap(), "v1").unwrap();
    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "tracked.txt"])
        .current_dir(p)
        .output()
        .unwrap();
    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "Commit 1: baseline"])
        .current_dir(p)
        .output()
        .unwrap();

    // Staged change.
    let staged = p.join("staged.txt");
    writeln!(File::create(&staged).unwrap(), "staged content").unwrap();
    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "staged.txt"])
        .current_dir(p)
        .output()
        .unwrap();

    // Unstaged change to the committed file.
    writeln!(File::create(&tracked).unwrap(), "v2").unwrap();

    // Untracked file.
    writeln!(File::create(p.join("untracked.txt")).unwrap(), "new").unwrap();

    let engine = GitEngine::open(Some(p)).unwrap();
    (dir, engine)
}

/// Builds an `AppState` on a dirty repository with the main view populated from
/// the real commit stream and the real status scan, exactly as the event loop does.
fn dirty_changes_app() -> (TempDir, AppState) {
    let (dir, engine) = dirty_repo();
    let head_id = engine.head_commit_id().unwrap();

    let mut main = MainView::new("main".to_string());
    let (_src, token) = tigrs_core::cancel::CancellationToken::new();
    if let Ok(iter) = engine.stream_commits(Some(head_id), Some(10), token) {
        for batch in iter.flatten() {
            main.append_commits(batch);
        }
    }
    main.set_finished();

    let (_status_src, status_token) = tigrs_core::cancel::CancellationToken::new();
    let report = engine.load_status(&status_token).expect("status scan");
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some(main),
            ..Default::default()
        },
        engine: Some(engine),
        changes_report: Some(report),
        ..Default::default()
    };
    app.push_view(ViewKind::Main);
    assert!(app.apply_changes_rows(), "dirty repo must produce rows");
    (dir, app)
}

#[test]
fn test_headless_main_view_renders_changes_rows_above_commits() {
    let mut main = MainView::new("main".to_string());
    main.append_commits(sample_commits(3));
    main.set_finished();
    main.set_changes(vec![
        ChangesRow::new(ChangesKind::Untracked, 2),
        ChangesRow::new(ChangesKind::Unstaged, 1),
        ChangesRow::new(ChangesKind::Staged, 4),
    ]);

    let options = ViewOptions {
        commit_id: true,
        ..Default::default()
    };

    let mut term = HeadlessTerminal::new(100, 24);
    main.render_with_options(&mut term, 100, 24, &options)
        .expect("render");

    // The header still counts only real commits (upstream Tig parity).
    term.assert_line_contains(0, "[main] main - 3 commits loaded");

    // Changes rows come first, in Tig's untracked -> unstaged -> staged order.
    for (line, title) in [
        (1, "Untracked changes"),
        (2, "Unstaged changes"),
        (3, "Staged changes"),
    ] {
        term.assert_line_contains(line, title);
        term.assert_line_contains(line, "0000000");
        term.assert_line_contains(line, "Not Committed Y");
    }

    // The first changes row is selected; the commits follow below.
    assert!(term.line_has_reverse(1));
    term.assert_line_contains(4, "Commit subject message number 1");

    // The status bar counts display rows, not just commits.
    term.assert_line_contains(23, "[main] line 1 of 6");
}

#[test]
fn test_headless_main_view_changes_rows_use_ascii_marker_when_configured() {
    let mut main = MainView::new("main".to_string());
    main.append_commits(sample_commits(1));
    main.set_finished();
    main.set_changes(vec![ChangesRow::new(ChangesKind::Unstaged, 1)]);

    let options = ViewOptions {
        line_graphics: LineGraphics::Ascii,
        ..Default::default()
    };

    let mut term = HeadlessTerminal::new(80, 24);
    main.render_with_options(&mut term, 80, 24, &options)
        .expect("render");

    let row = term.line_text_raw(1);
    assert!(
        row.contains(" o Unstaged changes"),
        "ASCII graphics must use 'o' for the changes marker, got: {row:?}"
    );
    assert!(
        !row.contains('○'),
        "ASCII graphics must not emit UTF-8 glyphs"
    );
}

#[test]
fn test_headless_changes_rows_reflect_real_status_scan() {
    let (_dir, app) = dirty_changes_app();

    let main = app.views.main_view.as_ref().unwrap();
    assert_eq!(
        main.changes()
            .iter()
            .map(|row| row.kind)
            .collect::<Vec<_>>(),
        vec![
            ChangesKind::Untracked,
            ChangesKind::Unstaged,
            ChangesKind::Staged
        ]
    );
    assert_eq!(main.row_count(), main.changes_len() + 1);

    // The cursor starts pinned to the top, so the first changes row is selected
    // and commit-only actions must fail safe.
    assert_eq!(main.cursor_index(), 0);
    assert!(main.selected_commit().is_none());

    let mut term = HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term, 80, 24).expect("render");
    term.assert_line_contains(1, "Untracked changes");
    term.assert_line_contains(4, "Commit 1: baseline");
}

/// Returns true if any rendered line of `diff` mentions `needle`.
fn diff_mentions(diff: &DiffView, needle: &str) -> bool {
    (0..diff.line_count()).any(|i| diff.line_text(i).is_some_and(|t| t.contains(needle)))
}

#[test]
fn test_headless_enter_on_changes_row_opens_section_diff() {
    let (_dir, mut app) = dirty_changes_app();
    let mut term = HeadlessTerminal::new(80, 24);

    // Row 0: untracked changes.
    let flow = tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    let diff = app.views.diff_view.as_ref().expect("diff view");
    assert_eq!(diff.title(), "Untracked changes");
    assert!(
        diff_mentions(diff, "untracked.txt"),
        "untracked section diff must cover the untracked file"
    );
    render_active(&app, &mut term, 80, 24).expect("render");
    term.assert_line_contains(12, "[diff]");
    term.assert_line_contains(12, "Untracked changes");

    // `,` walks the parent main view downwards, refreshing the split diff in place.
    tigrs_ui::handle_event(&key(KeyCode::Char(',')), &mut app, 24);
    assert_eq!(
        app.views.diff_view.as_ref().unwrap().title(),
        "Unstaged changes",
        "advancing to the next changes row must refresh the section diff"
    );

    tigrs_ui::handle_event(&key(KeyCode::Char(',')), &mut app, 24);
    let diff = app.views.diff_view.as_ref().unwrap();
    assert_eq!(diff.title(), "Staged changes");
    assert!(diff_mentions(diff, "staged.txt"));

    // Stepping past the last changes row lands on a real commit diff.
    tigrs_ui::handle_event(&key(KeyCode::Char(',')), &mut app, 24);
    assert_eq!(
        app.views.diff_view.as_ref().unwrap().title(),
        "Commit 1: baseline"
    );

    // `K` (previous) walks the parent main view back *up* towards the changes rows.
    tigrs_ui::handle_event(&key(KeyCode::Char('K')), &mut app, 24);
    assert_eq!(
        app.views.diff_view.as_ref().unwrap().title(),
        "Staged changes"
    );

    // `J` (next) walks back *down*, the exact mirror of `K`.
    tigrs_ui::handle_event(&key(KeyCode::Char('J')), &mut app, 24);
    assert_eq!(
        app.views.diff_view.as_ref().unwrap().title(),
        "Commit 1: baseline"
    );
}

#[test]
fn test_headless_toggling_show_changes_removes_rows_from_frame() {
    let (_dir, mut app) = dirty_changes_app();
    let mut term = HeadlessTerminal::new(80, 24);

    render_active(&app, &mut term, 80, 24).expect("render");
    term.assert_line_contains(1, "Untracked changes");
    term.assert_line_contains(23, "line 1 of 4");

    // `show-untracked` drops just the untracked row. Toggles leave a status
    // message behind, which takes over the status bar, so clear it before
    // asserting on the bar itself.
    app.toggle_option("show-untracked");
    app.status_message = None;
    assert_eq!(app.views.main_view.as_ref().unwrap().changes_len(), 2);
    term.clear();
    render_active(&app, &mut term, 80, 24).expect("render");
    term.assert_line_contains(1, "Unstaged changes");
    assert!(!term.line_text_raw(1).contains("Untracked changes"));
    term.assert_line_contains(23, "line 1 of 3");

    // `show-changes` drops the whole prefix, leaving the commit history alone.
    app.toggle_option("show-changes");
    app.status_message = None;
    assert_eq!(app.views.main_view.as_ref().unwrap().changes_len(), 0);
    term.clear();
    render_active(&app, &mut term, 80, 24).expect("render");
    term.assert_line_contains(1, "Commit 1: baseline");
    term.assert_line_contains(23, "line 1 of 1");

    // Toggling back restores them.
    app.toggle_option("show-changes");
    app.toggle_option("show-untracked");
    assert_eq!(app.views.main_view.as_ref().unwrap().changes_len(), 3);
}

#[test]
fn test_headless_changes_rows_preserve_commit_selection() {
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some({
                let mut main = MainView::new("main".to_string());
                main.append_commits(sample_commits(5));
                main.set_finished();
                main
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    app.push_view(ViewKind::Main);

    // Park the cursor on the third commit before any status scan completes.
    app.views.main_view.as_mut().unwrap().set_cursor(2, 22);
    let selected = app
        .views
        .main_view
        .as_ref()
        .unwrap()
        .selected_commit()
        .unwrap()
        .id;

    app.changes_report = Some(sample_status_report());
    assert!(app.apply_changes_rows());

    let main = app.views.main_view.as_ref().unwrap();
    assert_eq!(main.changes_len(), 3);
    assert_eq!(
        main.cursor_index(),
        5,
        "cursor must follow the commit across the inserted prefix"
    );
    assert_eq!(main.selected_commit().map(|c| c.id), Some(selected));
    assert_eq!(
        main.scroll_offset(),
        3,
        "the viewport shifts by the prefix delta so the screen does not jump"
    );

    let mut term = HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term, 80, 24).expect("render");
    // Screen line = 1 (header) + cursor - scroll_offset: the selected commit
    // stays exactly where it was before the changes rows appeared.
    assert!(term.line_has_reverse(3));
    term.assert_line_contains(3, "Commit subject message number 3");
}

#[test]
fn test_headless_grep_view_enter_opens_blob_view() {
    let (_dir, engine) = dirty_repo();
    let grep_matches = vec![GrepMatch {
        path: "tracked.txt".to_string(),
        line_num: 1,
        content: "v1".to_string(),
    }];
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            grep_view: Some(GrepView::new("v1".to_string(), grep_matches)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Grep);
    assert_eq!(app.active_view(), Some(ViewKind::Grep));

    let mut term = HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term, 80, 24).expect("render grep");
    term.assert_line_contains(1, "tracked.txt:1");

    // Press Enter on grep match -> opens BlobView in split mode
    let flow = tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Blob));

    let blob = app.views.blob_view.as_ref().expect("blob view open");
    // Line num 1 maps to 0-based cursor 0
    assert_eq!(blob.cursor(), 0);

    term.clear();
    app.invalidate_screen();
    render_active(&app, &mut term, 80, 24).unwrap();
    term.assert_line_contains(1, "tracked.txt:1");
    term.assert_line_contains(12, "[blob]");
    term.assert_line_contains(13, "v1");

    // Press 'q' to return to GrepView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Grep));
}

#[test]
fn test_headless_refs_view_enter_opens_diff_view() {
    let (_dir, engine) = dirty_repo();
    let head_id = engine.head_commit_id().unwrap();

    let refs = vec![RefEntry {
        full_name: "refs/heads/main".to_string(),
        name: "main".to_string(),
        kind: RefKind::LocalBranch,
        commit_id: head_id,
        summary: "Commit 1: baseline".to_string(),
        author_name: "Tester".to_string(),
        author_time_secs: 1_700_000_000,
    }];
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            refs_view: Some(RefsView::new(refs)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Refs);
    assert_eq!(app.active_view(), Some(ViewKind::Refs));

    let mut term = HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term, 80, 24).expect("render refs");
    term.assert_line_contains(1, "main");

    // Press Enter on ref -> opens DiffView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));

    let diff = app.views.diff_view.as_ref().expect("diff view open");
    assert!(diff.title().contains("Commit 1: baseline"));

    // Press 'q' returns to RefsView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Refs));
}

#[test]
fn test_headless_stash_view_enter_opens_diff_view() {
    let (_dir, engine) = dirty_repo();
    let head_id = engine.head_commit_id().unwrap();

    let stashes = vec![StashEntry {
        index: 0,
        commit_id: head_id,
        summary: "WIP on main: test stash".to_string(),
        time_secs: 1_700_000_000,
    }];
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            stash_view: Some(StashView::new(stashes)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Stash);
    assert_eq!(app.active_view(), Some(ViewKind::Stash));

    let mut term = HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term, 80, 24).expect("render stash");
    term.assert_line_contains(1, "stash@{0}");

    // Press Enter on stash -> opens DiffView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));

    // Press 'q' returns to StashView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Stash));
}

#[test]
fn test_headless_reflog_view_enter_opens_diff_view() {
    let (_dir, engine) = dirty_repo();
    let head_id = engine.head_commit_id().unwrap();

    let reflog = vec![ReflogEntry {
        index: 0,
        old_id: make_oid(0x1),
        new_id: head_id,
        committer_name: "Tester".to_string(),
        time_secs: 1_700_000_000,
        message: "commit: baseline".to_string(),
    }];
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            reflog_view: Some(ReflogView::new("HEAD".to_string(), reflog)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Reflog);
    assert_eq!(app.active_view(), Some(ViewKind::Reflog));

    let mut term = HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term, 80, 24).expect("render reflog");
    term.assert_line_contains(1, "baseline");

    // Press Enter on reflog -> opens DiffView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));

    // Press 'q' returns to ReflogView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Reflog));
}

/// Builds a repository with two commits touching the same file.
///
/// Returns the `TempDir` (which must outlive the test), an engine opened on it,
/// and the two commit ids, oldest first.
fn two_commit_repo() -> (TempDir, GitEngine, ObjectId, ObjectId) {
    let dir = TempDir::new().unwrap();
    let p = dir.path();

    for args in [
        vec!["init"],
        vec!["config", "user.name", "Test User"],
        vec!["config", "user.email", "test@example.com"],
    ] {
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(&args)
            .current_dir(p)
            .output()
            .unwrap();
    }

    let tracked = p.join("tracked.txt");
    for (content, message) in [("v1", "Commit 1: baseline"), ("v2", "Commit 2: update")] {
        writeln!(File::create(&tracked).unwrap(), "{content}").unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["add", "tracked.txt"])
            .current_dir(p)
            .output()
            .unwrap();
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", message])
            .current_dir(p)
            .output()
            .unwrap();
    }

    let engine = GitEngine::open(Some(p)).unwrap();
    let newer = engine.head_commit_id().unwrap();
    let older = engine.resolve_revision("HEAD~1").unwrap();
    (dir, engine, older, newer)
}

#[test]
fn test_headless_next_previous_in_refs_view_resyncs_diff() {
    let (_dir, engine, older, newer) = two_commit_repo();

    let refs = vec![
        RefEntry {
            full_name: "refs/heads/main".to_string(),
            name: "main".to_string(),
            kind: RefKind::LocalBranch,
            commit_id: newer,
            summary: "Commit 2: update".to_string(),
            author_name: "Tester".to_string(),
            author_time_secs: 1_700_000_001,
        },
        RefEntry {
            full_name: "refs/heads/older".to_string(),
            name: "older".to_string(),
            kind: RefKind::LocalBranch,
            commit_id: older,
            summary: "Commit 1: baseline".to_string(),
            author_name: "Tester".to_string(),
            author_time_secs: 1_700_000_000,
        },
    ];
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            refs_view: Some(RefsView::new(refs)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Refs);

    tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(newer)
    );

    // `next` steps the parent refs cursor down and pulls the diff along.
    tigrs_ui::handle_event(&key(KeyCode::Char('J')), &mut app, 24);
    assert_eq!(app.views.refs_view.as_ref().unwrap().cursor(), 1);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(older),
        "`next` must refresh the diff for the newly selected ref"
    );

    // `previous` is the exact mirror.
    tigrs_ui::handle_event(&key(KeyCode::Char('K')), &mut app, 24);
    assert_eq!(app.views.refs_view.as_ref().unwrap().cursor(), 0);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(newer),
        "`previous` must refresh the diff for the newly selected ref"
    );
}

#[test]
fn test_headless_next_previous_in_stash_view_resyncs_diff() {
    let (_dir, engine, older, newer) = two_commit_repo();

    let stashes = vec![
        StashEntry {
            index: 0,
            commit_id: newer,
            summary: "WIP on main: newer".to_string(),
            time_secs: 1_700_000_001,
        },
        StashEntry {
            index: 1,
            commit_id: older,
            summary: "WIP on main: older".to_string(),
            time_secs: 1_700_000_000,
        },
    ];
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            stash_view: Some(StashView::new(stashes)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Stash);

    tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(newer)
    );

    tigrs_ui::handle_event(&key(KeyCode::Char('J')), &mut app, 24);
    assert_eq!(app.views.stash_view.as_ref().unwrap().cursor(), 1);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(older)
    );

    tigrs_ui::handle_event(&key(KeyCode::Char('K')), &mut app, 24);
    assert_eq!(app.views.stash_view.as_ref().unwrap().cursor(), 0);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(newer)
    );
}

#[test]
fn test_headless_next_previous_in_reflog_view_resyncs_diff() {
    let (_dir, engine, older, newer) = two_commit_repo();

    let reflog = vec![
        ReflogEntry {
            index: 0,
            old_id: older,
            new_id: newer,
            committer_name: "Tester".to_string(),
            time_secs: 1_700_000_001,
            message: "commit: update".to_string(),
        },
        ReflogEntry {
            index: 1,
            old_id: make_oid(0x1),
            new_id: older,
            committer_name: "Tester".to_string(),
            time_secs: 1_700_000_000,
            message: "commit (initial): baseline".to_string(),
        },
    ];
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            reflog_view: Some(ReflogView::new("HEAD".to_string(), reflog)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Reflog);

    tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(newer)
    );

    tigrs_ui::handle_event(&key(KeyCode::Char('J')), &mut app, 24);
    assert_eq!(app.views.reflog_view.as_ref().unwrap().cursor(), 1);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(older)
    );

    tigrs_ui::handle_event(&key(KeyCode::Char('K')), &mut app, 24);
    assert_eq!(app.views.reflog_view.as_ref().unwrap().cursor(), 0);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(newer)
    );
}

#[test]
fn test_headless_next_previous_in_blame_view_resyncs_diff() {
    let (_dir, engine, older, newer) = two_commit_repo();

    let blame = BlameResult {
        commit_id: newer,
        path: "tracked.txt".to_string(),
        is_binary: false,
        lines: vec![
            BlameLine {
                line_number: 1,
                commit_id: newer,
                short_commit_id: Arc::from(&newer.to_string()[..8]),
                author: Arc::from("Tester"),
                author_date: Arc::from("2023-11-15"),
                summary: Arc::from("Commit 2: update"),
                content: "v2".to_string(),
                is_hunk_start: true,
                parent_commit_id: Some(older),
                source_path: None,
                source_line_number: 1,
            },
            BlameLine {
                line_number: 2,
                commit_id: older,
                short_commit_id: Arc::from(&older.to_string()[..8]),
                author: Arc::from("Tester"),
                author_date: Arc::from("2023-11-14"),
                summary: Arc::from("Commit 1: baseline"),
                content: "v1".to_string(),
                is_hunk_start: true,
                parent_commit_id: None,
                source_path: None,
                source_line_number: 2,
            },
        ],
    };
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            blame_view: Some(BlameView::from_result(blame)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Blame);

    tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(newer)
    );

    tigrs_ui::handle_event(&key(KeyCode::Char('J')), &mut app, 24);
    assert_eq!(app.views.blame_view.as_ref().unwrap().cursor(), 1);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(older)
    );

    tigrs_ui::handle_event(&key(KeyCode::Char('K')), &mut app, 24);
    assert_eq!(app.views.blame_view.as_ref().unwrap().cursor(), 0);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(newer)
    );
}

#[test]
fn test_headless_help_view_navigation() {
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            help_view: Some(HelpView::new()),
            ..Default::default()
        },
        ..Default::default()
    };
    app.push_view(ViewKind::Help);
    assert_eq!(app.active_view(), Some(ViewKind::Help));

    let mut term = HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term, 80, 24).expect("render help");
    term.assert_line_contains(1, "About tigrs 0.1.0");
    assert!(!term.line_text(1).contains("==="));
    term.assert_line_contains(5, "View Switching");
    assert!(!term.line_text(5).contains("==="));

    let initial_cursor = app.views.help_view.as_ref().unwrap().cursor();
    // Move down
    tigrs_ui::handle_event(&key(KeyCode::Char('j')), &mut app, 24);
    assert_eq!(
        app.views.help_view.as_ref().unwrap().cursor(),
        initial_cursor + 1
    );

    // Move up
    tigrs_ui::handle_event(&key(KeyCode::Char('k')), &mut app, 24);
    assert_eq!(
        app.views.help_view.as_ref().unwrap().cursor(),
        initial_cursor
    );

    // Page down
    tigrs_ui::handle_event(&key(KeyCode::PageDown), &mut app, 24);
    assert!(app.views.help_view.as_ref().unwrap().cursor() > initial_cursor);

    // Jump to first line with Home key
    tigrs_ui::handle_event(&key(KeyCode::Home), &mut app, 24);
    assert_eq!(app.views.help_view.as_ref().unwrap().cursor(), 0);
}

#[test]
fn test_headless_diff_view_hunk_and_file_navigation_and_maximize() {
    let multi_diff = CommitDiff {
        commit_id: make_oid(0x22),
        parent_ids: vec![],
        author_name: Arc::from("Author"),
        author_email: Arc::from("a@b.com"),
        author_date: "Sun Sep 13 00:00:00 2026 +0000".to_string(),
        committer_name: Arc::from("Author"),
        committer_email: Arc::from("a@b.com"),
        committer_date: "Sun Sep 13 00:00:00 2026 +0000".to_string(),
        title: Arc::from("Multi file commit"),
        body: None,
        files: vec![
            FileDiff {
                path: "file1.rs".to_string(),
                status: FileChangeStatus::Modified,
                old_id: Some(make_oid(0x1)),
                new_id: Some(make_oid(0x2)),
                old_mode: Some(0o100_644),
                new_mode: Some(0o100_644),
                is_binary: false,
                additions: 2,
                deletions: 0,
                hunks: vec![
                    DiffHunk {
                        old_start: 1,
                        old_len: 2,
                        new_start: 1,
                        new_len: 3,
                        func_context: None,
                        lines: vec![
                            HunkLine {
                                kind: DiffLineKind::Context,
                                content: "context 1".to_string(),
                                no_newline_at_eof: false,
                            },
                            HunkLine {
                                kind: DiffLineKind::Add,
                                content: "add 1".to_string(),
                                no_newline_at_eof: false,
                            },
                        ],
                    },
                    DiffHunk {
                        old_start: 10,
                        old_len: 2,
                        new_start: 11,
                        new_len: 3,
                        func_context: None,
                        lines: vec![
                            HunkLine {
                                kind: DiffLineKind::Context,
                                content: "context 2".to_string(),
                                no_newline_at_eof: false,
                            },
                            HunkLine {
                                kind: DiffLineKind::Add,
                                content: "add 2".to_string(),
                                no_newline_at_eof: false,
                            },
                        ],
                    },
                ],
            },
            FileDiff {
                path: "file2.rs".to_string(),
                status: FileChangeStatus::Added,
                old_id: None,
                new_id: Some(make_oid(0x3)),
                old_mode: None,
                new_mode: Some(0o100_644),
                is_binary: false,
                additions: 1,
                deletions: 0,
                hunks: vec![DiffHunk {
                    old_start: 0,
                    old_len: 0,
                    new_start: 1,
                    new_len: 1,
                    func_context: None,
                    lines: vec![HunkLine {
                        kind: DiffLineKind::Add,
                        content: "new file content".to_string(),
                        no_newline_at_eof: false,
                    }],
                }],
            },
        ],
        stats: DiffSummaryStats {
            files_changed: 2,
            insertions: 3,
            deletions: 0,
        },
    };

    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some({
                let mut main = MainView::new("main".to_string());
                main.append_commits(sample_commits(2));
                main
            }),
            diff_view: Some(DiffView::new(multi_diff)),
            ..Default::default()
        },
        ..Default::default()
    };
    app.push_view(ViewKind::Main);
    app.push_view(ViewKind::Diff);

    // Initially at cursor 0
    assert_eq!(app.views.diff_view.as_ref().unwrap().cursor_index(), 0);

    // NextHunk: ')'
    tigrs_ui::handle_event(&key(KeyCode::Char(')')), &mut app, 24);
    let cur_hunk1 = app.views.diff_view.as_ref().unwrap().cursor_index();
    assert!(
        cur_hunk1 > 0,
        "NextHunk ')' should jump to first hunk header"
    );

    // NextHunk again: jumps to second hunk (also test '@' works for NextHunk)
    tigrs_ui::handle_event(&key(KeyCode::Char('@')), &mut app, 24);
    let cur_hunk2 = app.views.diff_view.as_ref().unwrap().cursor_index();
    assert!(
        cur_hunk2 > cur_hunk1,
        "NextHunk '@' should jump to second hunk header"
    );

    // PrevHunk: '('
    tigrs_ui::handle_event(&key(KeyCode::Char('(')), &mut app, 24);
    let cur_back = app.views.diff_view.as_ref().unwrap().cursor_index();
    assert_eq!(
        cur_back, cur_hunk1,
        "PrevHunk '(' should return to first hunk header"
    );

    // NextFile: '}'
    tigrs_ui::handle_event(&key(KeyCode::Char('}')), &mut app, 24);
    let cur_file2 = app.views.diff_view.as_ref().unwrap().cursor_index();
    assert!(
        cur_file2 > cur_hunk2,
        "NextFile should jump to file2.rs header"
    );

    // PrevFile: '{'
    tigrs_ui::handle_event(&key(KeyCode::Char('{')), &mut app, 24);
    let cur_file1 = app.views.diff_view.as_ref().unwrap().cursor_index();
    assert!(
        cur_file1 < cur_file2,
        "PrevFile should jump back to file1.rs header"
    );

    // Action::ToggleDiffContext via ']' and '[' (upstream Tig parity)
    let initial_ctx = app.options.diff_context;
    tigrs_ui::handle_event(&key(KeyCode::Char(']')), &mut app, 24);
    assert_eq!(app.options.diff_context, initial_ctx + 1);
    tigrs_ui::handle_event(&key(KeyCode::Char('[')), &mut app, 24);
    assert_eq!(app.options.diff_context, initial_ctx);

    // Maximize toggle: 'O'
    assert!(!app.views.maximized);
    tigrs_ui::handle_event(&key(KeyCode::Char('O')), &mut app, 24);
    assert!(app.views.maximized);
    tigrs_ui::handle_event(&key(KeyCode::Char('O')), &mut app, 24);
    assert!(!app.views.maximized);
}

#[test]
fn test_headless_extreme_terminal_dimensions() {
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some({
                let mut main = MainView::new("main".to_string());
                main.append_commits(sample_commits(5));
                main
            }),
            diff_view: Some(DiffView::new(sample_commit_diff())),
            status_view: Some(StatusView::new(StatusReport::default())),
            help_view: Some(HelpView::new()),
            ..Default::default()
        },
        ..Default::default()
    };
    app.push_view(ViewKind::Main);

    let extreme_sizes = [(1, 1), (2, 2), (5, 3), (10, 4), (300, 100)];

    for (w, h) in extreme_sizes {
        let mut term = HeadlessTerminal::new(w, h);

        // Main view
        render_active(&app, &mut term, w, h).expect("render main at extreme size");

        // Diff view
        app.push_view(ViewKind::Diff);
        render_active(&app, &mut term, w, h).expect("render diff at extreme size");
        app.pop_active_view();

        // Status view
        app.push_view(ViewKind::Status);
        render_active(&app, &mut term, w, h).expect("render status at extreme size");
        app.pop_active_view();

        // Help view
        app.push_view(ViewKind::Help);
        render_active(&app, &mut term, w, h).expect("render help at extreme size");
        app.pop_active_view();
    }
}

#[test]
fn test_headless_multibyte_utf8_prompt_editing() {
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some(MainView::new("main".to_string())),
            ..Default::default()
        },
        ..Default::default()
    };
    app.push_view(ViewKind::Main);

    // 1. Open command prompt with ':'
    tigrs_ui::handle_event(&key(KeyCode::Char(':')), &mut app, 24);
    assert!(app.prompt.is_some());
    assert_eq!(app.prompt.as_ref().unwrap().kind, PromptKind::Command);

    // 2. Type multibyte Unicode / CJK characters: '🦀', '中', '文', '🚀'
    for ch in ['🦀', '中', '文', '🚀'] {
        tigrs_ui::handle_event(&key(KeyCode::Char(ch)), &mut app, 24);
    }
    assert_eq!(app.prompt.as_ref().unwrap().buffer, "🦀中文🚀");
    assert_eq!(app.prompt.as_ref().unwrap().char_count(), 4);
    assert_eq!(app.prompt.as_ref().unwrap().cursor, 4);

    // 3. Move left by one char, then backspace should delete '文'
    tigrs_ui::handle_event(&key(KeyCode::Left), &mut app, 24);
    assert_eq!(app.prompt.as_ref().unwrap().cursor, 3);

    tigrs_ui::handle_event(&key(KeyCode::Backspace), &mut app, 24);
    assert_eq!(app.prompt.as_ref().unwrap().buffer, "🦀中🚀");
    assert_eq!(app.prompt.as_ref().unwrap().cursor, 2);

    // 4. Delete at cursor: should delete '🚀'
    tigrs_ui::handle_event(&key(KeyCode::Delete), &mut app, 24);
    assert_eq!(app.prompt.as_ref().unwrap().buffer, "🦀中");
    assert_eq!(app.prompt.as_ref().unwrap().cursor, 2);

    // 5. Cancel with Esc
    tigrs_ui::handle_event(&key(KeyCode::Esc), &mut app, 24);
    assert!(app.prompt.is_none());
}

#[test]
fn test_headless_secondary_view_stack_transitions() {
    let oid = make_oid(0x22);
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some(MainView::new("main".to_string())),
            grep_view: Some(GrepView::new(
                "pattern".to_string(),
                vec![GrepMatch {
                    path: "test.rs".to_string(),
                    line_num: 1,
                    content: "match content".to_string(),
                }],
            )),
            stash_view: Some(StashView::new(vec![StashEntry {
                index: 0,
                commit_id: oid,
                summary: "WIP test".to_string(),
                time_secs: 1_700_000_000,
            }])),
            reflog_view: Some(ReflogView::new(
                "HEAD".to_string(),
                vec![ReflogEntry {
                    index: 0,
                    old_id: oid,
                    new_id: oid,
                    committer_name: "Tester".to_string(),
                    time_secs: 1_700_000_000,
                    message: "commit: message".to_string(),
                }],
            )),
            refs_view: Some(RefsView::new(vec![RefEntry {
                name: "main".to_string(),
                full_name: "refs/heads/main".to_string(),
                kind: RefKind::LocalBranch,
                commit_id: oid,
                summary: "commit".to_string(),
                author_name: "Author".to_string(),
                author_time_secs: 1_700_000_000,
            }])),
            ..Default::default()
        },
        ..Default::default()
    };
    app.push_view(ViewKind::Main);
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    // Push GrepView -> navigate -> press 'q' to pop
    app.push_view(ViewKind::Grep);
    assert_eq!(app.active_view(), Some(ViewKind::Grep));
    tigrs_ui::handle_event(&key(KeyCode::Char('j')), &mut app, 24);
    tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    // Push StashView -> navigate -> press 'q' to pop
    app.push_view(ViewKind::Stash);
    assert_eq!(app.active_view(), Some(ViewKind::Stash));
    tigrs_ui::handle_event(&key(KeyCode::Char('j')), &mut app, 24);
    tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    // Push ReflogView -> navigate -> press 'q' to pop
    app.push_view(ViewKind::Reflog);
    assert_eq!(app.active_view(), Some(ViewKind::Reflog));
    tigrs_ui::handle_event(&key(KeyCode::Char('j')), &mut app, 24);
    tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    // Push RefsView -> navigate -> press 'q' to pop
    app.push_view(ViewKind::Refs);
    assert_eq!(app.active_view(), Some(ViewKind::Refs));
    tigrs_ui::handle_event(&key(KeyCode::Char('j')), &mut app, 24);
    tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Main));
}

#[test]
fn test_headless_resize_event_flow() {
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some(MainView::new("main".to_string())),
            ..Default::default()
        },
        ..Default::default()
    };
    app.push_view(ViewKind::Main);

    let resize_ev = Event::Resize(120, 40);
    let flow = tigrs_ui::handle_event_with_dimensions(&resize_ev, &mut app, 120, 40);
    assert_eq!(flow, Flow::Continue);
}

#[test]
fn test_headless_edit_in_status_view() {
    let (_dir, engine) = dirty_repo();
    let (_src, token) = tigrs_core::cancel::CancellationToken::new();
    let report = engine.load_status(&token).expect("status report");

    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            status_view: Some(StatusView::new(report)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Status);

    let expected_path = app
        .views
        .status_view
        .as_ref()
        .unwrap()
        .selected_item()
        .unwrap()
        .path
        .clone();

    assert!(!app.options.read_only);
    // Press 'e' when Read-Only mode is enabled -> blocked with READ_ONLY_WARNING_MSG
    app.options.read_only = true;
    tigrs_ui::handle_event(&key(KeyCode::Char('e')), &mut app, 24);
    assert!(app.pending_editor.is_none());
    assert_eq!(
        app.status_message.as_deref(),
        Some(tigrs_ui::app::READ_ONLY_WARNING_MSG)
    );

    // Unlock Update Mode and press 'e' to edit
    app.options.read_only = false;
    tigrs_ui::handle_event(&key(KeyCode::Char('e')), &mut app, 24);

    let inv = app.pending_editor.expect("pending editor should be set");
    assert_eq!(inv.target.path, expected_path);
    assert_eq!(inv.target.line, None);
    assert!(!inv.used_line_number);
    assert!(inv.command_line.ends_with(&format!("'{expected_path}'")));
}

#[test]
fn test_headless_edit_in_diff_view() {
    let (_dir, engine, _older, newer) = two_commit_repo();
    let diff_data = engine.compute_commit_diff_cached(newer).expect("diff");
    let mut diff_view = DiffView::new((*diff_data).clone());
    // Move to first hunk
    diff_view.next_hunk(24);

    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            diff_view: Some(diff_view),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.options.read_only = false;
    app.push_view(ViewKind::Diff);

    // Press 'e' on the diff hunk
    tigrs_ui::handle_event(&key(KeyCode::Char('e')), &mut app, 24);

    let inv = app.pending_editor.expect("pending editor should be set");
    assert_eq!(inv.target.path, "tracked.txt");
    assert!(inv.target.line.is_some());
    assert!(inv.used_line_number);
    assert!(inv.command_line.contains("+1 'tracked.txt'"));
}

#[test]
fn test_headless_edit_in_blame_view() {
    let (_dir, engine, _older, newer) = two_commit_repo();
    let blame_res = engine.blame_file(newer, "tracked.txt").expect("blame");
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            blame_view: Some(BlameView::from_result(blame_res)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.options.read_only = false;
    app.push_view(ViewKind::Blame);

    // Press 'e' on blame view line 1
    tigrs_ui::handle_event(&key(KeyCode::Char('e')), &mut app, 24);

    let inv = app.pending_editor.expect("pending editor should be set");
    assert_eq!(inv.target.path, "tracked.txt");
    assert_eq!(inv.target.line, Some(1));
    assert!(inv.used_line_number);
    assert!(inv.command_line.contains("+1 'tracked.txt'"));
}

#[test]
fn test_headless_edit_refusals() {
    let (_dir, engine, _older, newer) = two_commit_repo();

    // 1. In Main view: default is Update Mode (`!app.options.read_only`);
    //    when Read-Only mode is enabled -> READ_ONLY_WARNING_MSG;
    //    when unlocked (`read_only = false`) -> "Nothing to edit"
    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some(MainView::new("main".to_string())),
            ..Default::default()
        },
        engine: Some(engine.clone()),
        ..Default::default()
    };
    app.push_view(ViewKind::Main);
    assert!(!app.options.read_only);
    app.options.read_only = true;
    tigrs_ui::handle_event(&key(KeyCode::Char('e')), &mut app, 24);
    assert_eq!(
        app.status_message.as_deref(),
        Some(tigrs_ui::app::READ_ONLY_WARNING_MSG)
    );
    app.options.read_only = false;
    tigrs_ui::handle_event(&key(KeyCode::Char('e')), &mut app, 24);
    assert_eq!(app.status_message.as_deref(), Some("Nothing to edit"));
    assert!(app.pending_editor.is_none());

    // 2. In Diff view on deleted file -> "File has been deleted."
    let commit_diff = tigrs_git::CommitDiff {
        commit_id: newer,
        parent_ids: vec![],
        author_name: Arc::from(""),
        author_email: Arc::from(""),
        author_date: String::new(),
        committer_name: Arc::from(""),
        committer_email: Arc::from(""),
        committer_date: String::new(),
        title: Arc::from("Delete commit"),
        body: None,
        files: vec![tigrs_git::FileDiff {
            path: "removed.txt".to_string(),
            status: tigrs_git::FileChangeStatus::Deleted,
            old_id: None,
            new_id: None,
            old_mode: None,
            new_mode: None,
            is_binary: false,
            additions: 0,
            deletions: 5,
            hunks: vec![],
        }],
        stats: tigrs_git::DiffSummaryStats::default(),
    };
    let mut diff_view = DiffView::new(commit_diff);
    diff_view.next_file(24);
    let mut diff_app = AppState {
        views: tigrs_ui::app::ViewManager {
            diff_view: Some(diff_view),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    diff_app.options.read_only = false;
    diff_app.push_view(ViewKind::Diff);
    tigrs_ui::handle_event(&key(KeyCode::Char('e')), &mut diff_app, 24);
    assert_eq!(
        diff_app.status_message.as_deref(),
        Some("File has been deleted.")
    );
    assert!(diff_app.pending_editor.is_none());
}

#[test]
fn test_headless_edit_line_number_disabled() {
    let (_dir, engine, _older, newer) = two_commit_repo();
    let diff_data = engine.compute_commit_diff_cached(newer).expect("diff");
    let mut diff_view = DiffView::new((*diff_data).clone());
    diff_view.next_hunk(24);

    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            diff_view: Some(diff_view),
            ..Default::default()
        },
        engine: Some(engine),
        editor_line_number_disabled: true,
        // Auto-disabled runtime toggle
        ..Default::default()
    };
    app.options.read_only = false;
    app.push_view(ViewKind::Diff);

    tigrs_ui::handle_event(&key(KeyCode::Char('e')), &mut app, 24);

    let inv = app.pending_editor.expect("pending editor should be set");
    assert_eq!(inv.target.path, "tracked.txt");
    assert!(!inv.used_line_number);
    // Command line must not contain +1
    assert!(!inv.command_line.contains("+1"));
    assert!(inv.command_line.ends_with("'tracked.txt'"));
}

#[test]
fn test_headless_d_key_opens_diff_view_across_views() {
    let (_dir, engine) = dirty_repo();
    let head_id = engine.head_commit_id().unwrap();

    let commit = CommitSummary {
        id: head_id,
        parents: ParentIds::new(),
        author_name: Arc::from("Tester"),
        author_time_secs: 1_700_000_000,
        summary: Box::from("Baseline commit"),
    };
    let mut main = MainView::new("main".to_string());
    main.append_commits(vec![commit]);
    main.set_finished();

    let (_src, token) = tigrs_core::cancel::CancellationToken::new();
    let report = engine.load_status(&token).expect("status report");

    let refs = vec![RefEntry {
        full_name: "refs/heads/main".to_string(),
        name: "main".to_string(),
        kind: RefKind::LocalBranch,
        commit_id: head_id,
        summary: "Commit 1: baseline".to_string(),
        author_name: "Tester".to_string(),
        author_time_secs: 1_700_000_000,
    }];
    let stashes = vec![StashEntry {
        index: 0,
        commit_id: head_id,
        summary: "WIP on main: test stash".to_string(),
        time_secs: 1_700_000_000,
    }];
    let reflogs = vec![ReflogEntry {
        index: 0,
        old_id: make_oid(0x1),
        new_id: head_id,
        committer_name: "Tester".to_string(),
        time_secs: 1_700_000_000,
        message: "commit: baseline".to_string(),
    }];

    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some(main),
            status_view: Some(StatusView::new(report)),
            refs_view: Some(RefsView::new(refs)),
            stash_view: Some(StashView::new(stashes)),
            reflog_view: Some(ReflogView::new("HEAD".to_string(), reflogs)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Main);
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    // 1. Press 'd' in MainView -> opens DiffView maximized
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('d')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.diff_view.is_some());
    assert!(app.views.maximized);

    let mut term = HeadlessTerminal::new(80, 24);
    app.invalidate_screen();
    render_active(&app, &mut term, 80, 24).expect("render diff full-window");
    term.assert_line_contains(0, "[diff]");

    // Redundant 'd' in full-window DiffView -> silent no-op
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('d')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.maximized);

    // Press 'q' -> pops DiffView back to MainView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Main));
    assert!(!app.views.maximized);

    // 2. Switch to StatusView, press 'd' -> opens DiffView for selected status item maximized
    app.push_view(ViewKind::Status);
    assert_eq!(app.active_view(), Some(ViewKind::Status));
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('d')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.diff_view.is_some());
    assert!(app.views.maximized);

    term.clear();
    app.invalidate_screen();
    render_active(&app, &mut term, 80, 24).expect("render diff status");
    term.assert_line_contains(0, "[diff]");

    // Press 'q' -> returns to StatusView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Status));
    assert!(!app.views.maximized);

    // 3. Switch to RefsView, press 'd' -> opens DiffView maximized
    app.push_view(ViewKind::Refs);
    assert_eq!(app.active_view(), Some(ViewKind::Refs));
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('d')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.diff_view.is_some());
    assert!(app.views.maximized);

    term.clear();
    app.invalidate_screen();
    render_active(&app, &mut term, 80, 24).expect("render diff refs");
    term.assert_line_contains(0, "[diff]");

    // Press 'q' -> returns to RefsView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Refs));
    assert!(!app.views.maximized);

    // 4. Switch to StashView, press 'd' -> opens DiffView maximized
    app.push_view(ViewKind::Stash);
    assert_eq!(app.active_view(), Some(ViewKind::Stash));
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('d')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.diff_view.is_some());
    assert!(app.views.maximized);

    term.clear();
    app.invalidate_screen();
    render_active(&app, &mut term, 80, 24).expect("render diff stash");
    term.assert_line_contains(0, "[diff]");

    // Press 'q' -> returns to StashView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Stash));
    assert!(!app.views.maximized);

    // 5. Switch to ReflogView, press 'd' -> opens DiffView maximized
    app.push_view(ViewKind::Reflog);
    assert_eq!(app.active_view(), Some(ViewKind::Reflog));
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('d')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.diff_view.is_some());
    assert!(app.views.maximized);

    term.clear();
    app.invalidate_screen();
    render_active(&app, &mut term, 80, 24).expect("render diff reflog");
    term.assert_line_contains(0, "[diff]");

    // Press 'q' -> returns to ReflogView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Reflog));
    assert!(!app.views.maximized);
}

#[test]
fn test_headless_diff_view_split_enter_and_maximize_d_lifecycle() {
    let (_dir, engine) = dirty_repo();
    let head_id = engine.head_commit_id().unwrap();

    let commit = CommitSummary {
        id: head_id,
        parents: ParentIds::new(),
        author_name: Arc::from("Tester"),
        author_time_secs: 1_700_000_000,
        summary: Box::from("Baseline commit"),
    };
    let mut main = MainView::new("main".to_string());
    main.append_commits(vec![commit]);
    main.set_finished();

    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some(main),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Main);
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    let mut term = HeadlessTerminal::new(80, 24);

    // Step 1: Initial state is full-window MainView
    render_active(&app, &mut term, 80, 24).expect("render main");
    term.assert_line_contains(0, "[main]");
    assert!(!term.line_text_raw(12).contains("[diff]"));

    // Step 2: Press Enter on commit -> opens DiffView in SPLIT mode
    let flow = tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(!app.views.maximized);

    term.clear();
    render_active(&app, &mut term, 80, 24).expect("render split");
    term.assert_line_contains(0, "[main]");
    term.assert_line_contains(12, "[diff]");

    // Step 3: Press 'd' -> maximizes DiffView to full-window
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('d')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.maximized);

    term.clear();
    render_active(&app, &mut term, 80, 24).expect("render full diff");
    term.assert_line_contains(0, "[diff]");
    assert!(!term.line_text_raw(0).contains("[main]"));
    assert!(!term.line_text_raw(12).contains("[diff]"));

    // Step 4: Redundant 'd' while already maximized -> silent no-op
    app.status_message = None;
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('d')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.maximized);
    assert!(app.status_message.is_none());

    // Step 5: Press 'O' (Action::Maximize) -> restores dual split view
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('O')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(!app.views.maximized);
    assert_eq!(app.status_message.as_deref(), Some("View split restored"));

    term.clear();
    render_active(&app, &mut term, 80, 24).expect("render split restored");
    term.assert_line_contains(0, "[main]");
    term.assert_line_contains(12, "[diff]");

    // Step 6: Press 'q' -> closes DiffView, returns to full-window MainView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Main));
    assert_eq!(app.views.view_stack.len(), 1);
    assert!(!app.views.maximized);

    term.clear();
    render_active(&app, &mut term, 80, 24).expect("render returned main");
    term.assert_line_contains(0, "[main]");
    assert!(!term.line_text_raw(12).contains("[diff]"));
}

#[test]
fn test_headless_14_canonical_view_keys_and_navigation() {
    let (_dir, engine) = dirty_repo();
    let head_id = engine.head_commit_id().unwrap();

    let commit = CommitSummary {
        id: head_id,
        parents: ParentIds::new(),
        author_name: Arc::from("Tester"),
        author_time_secs: 1_700_000_000,
        summary: Box::from("Baseline commit"),
    };
    let mut main = MainView::new("main".to_string());
    main.append_commits(vec![commit]);
    main.set_finished();

    let mut app = AppState {
        views: tigrs_ui::app::ViewManager {
            main_view: Some(main),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Main);
    let mut term = HeadlessTerminal::new(80, 24);

    // 1. 'l' (view-log) and 'm' (view-main)
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('l')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Log));

    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('m')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    // 2. 'd' (view-diff)
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('d')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.maximized);

    // 3. 's' / 'S' (view-status)
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('s')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Status));
    // Press 'S' toggles back
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('S')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));

    // 4. 'c' (view-stage)
    // Go to status view
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('s')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Status));
    // Press 'c' to open stage view for current item
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('c')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.maximized);

    // 5. 't' (view-tree)
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('t')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Tree));

    // 6. 'f' (view-blob)
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('f')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Blob));

    // 7. 'b' (view-blame)
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('b')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Blame));

    // 8. 'r' (view-refs)
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('r')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Refs));
    // Toggle back
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('r')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Blame));

    // 9. 'y' (view-stash)
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('y')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Stash));
    // Toggle back
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('y')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Blame));

    // 10. 'L' (view-reflog)
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('L')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Reflog));
    // Toggle back
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('L')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Blame));

    // 11. 'g' (view-grep)
    app.views.grep_view = Some(GrepView::new("search".to_string(), vec![]));
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('g')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Grep));
    // In grep view, 'g' triggers prompt
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('g')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_some());
    app.prompt = None;

    // 12. 'p' (view-pager)
    app.views.pager_view = Some(PagerView::new(
        "Log".to_string(),
        vec!["Pager output line 1".to_string()],
    ));
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('p')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Pager));
    // Toggle back
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('p')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Grep));

    // 13. 'h' (view-help)
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('h')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Help));

    term.clear();
    render_active(&app, &mut term, 80, 24).expect("render help");
    // Verify that rendered help view includes keybindings
    let mut found_stage = false;
    let mut found_reflog = false;
    for y in 0..24 {
        let line = term.line_text_raw(y);
        if line.contains("view-stage") {
            found_stage = true;
        }
        if line.contains("view-reflog") {
            found_reflog = true;
        }
    }
    assert!(found_stage, "view-stage must be rendered in HelpView");
    assert!(found_reflog, "view-reflog must be rendered in HelpView");

    // Toggle back from help
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('h')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Grep));

    // Return to main with 'm'
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('m')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Main));
}

#[test]
fn test_headless_log_view_shows_summary_changes_and_diff_enter() {
    let (_dir, mut app) = dirty_changes_app();
    let mut term = HeadlessTerminal::new(80, 24);

    // Initial view is Main
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    // Move to commit row (skip the 2 uncommitted changes rows)
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('j')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('j')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);

    // Press 'l' to open LogView (shows summary changes for commit)
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('l')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Log));
    assert!(app.views.maximized);
    assert!(app.views.log_view.is_some());

    // Render the LogView
    term.clear();
    render_active(&app, &mut term, 80, 24).expect("render log view");

    // Verify rendered content contains commit headers, diffstat file bars, and diffstat summary
    let mut found_commit_hdr = false;
    let mut found_author = false;
    let mut found_diffstat_bar = false;
    let mut found_diffstat_summary = false;

    for y in 0..24 {
        let line = term.line_text_raw(y);
        if line.contains("commit ") {
            found_commit_hdr = true;
        }
        if line.contains("Author:") {
            found_author = true;
        }
        if line.contains('|') && (line.contains('+') || line.contains('-') || line.contains("Bin"))
        {
            found_diffstat_bar = true;
        }
        if line.contains("file") && line.contains("changed") {
            found_diffstat_summary = true;
        }
    }

    assert!(found_commit_hdr, "LogView must render commit header line");
    assert!(found_author, "LogView must render Author line");
    assert!(
        found_diffstat_bar,
        "LogView must render diffstat file bar with '|'"
    );
    assert!(
        found_diffstat_summary,
        "LogView must render aggregate diffstat summary (X files changed)"
    );

    // Navigation in LogView: move down and up
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('j')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.views.log_view.as_ref().unwrap().cursor(), 1);

    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('k')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.views.log_view.as_ref().unwrap().cursor(), 0);

    // Press 'Enter' on the commit header in LogView to open full DiffView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.diff_view.is_some());

    // Press 'l' switches back to LogView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('l')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Log));

    // Press 'm' switches back to MainView
    let flow = tigrs_ui::handle_event(&key(KeyCode::Char('m')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Main));
}
