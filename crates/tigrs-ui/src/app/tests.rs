// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

use super::*;
use crate::keymap::{Action, KeySequence, KeymapScope, RunFlags};
use crate::view::GrepMatch;
use crossterm::event::{KeyEvent, MouseEvent, MouseEventKind};
use gix::ObjectId;
use std::sync::Arc;
use tigrs_git::{
    CommitDiff, DiffHunk, DiffLineKind, DiffSummaryStats, FileChangeStatus, FileDiff, HunkLine,
    StatusSection,
};

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn ctrl_key(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

fn mouse_scroll_down_at(column: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

fn mouse_scroll_up_at(column: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

fn mouse_scroll_down() -> Event {
    mouse_scroll_down_at(0, 0)
}

fn mouse_scroll_up() -> Event {
    mouse_scroll_up_at(0, 0)
}

fn view_with(n: usize) -> MainView {
    let mut v = MainView::new("main".to_string());
    v.append_commits(
        (0..n)
            .map(|i| CommitSummary {
                id: ObjectId::empty_tree(gix::hash::Kind::Sha1),
                parents: tigrs_git::ParentIds::new(),
                author_name: Arc::from("A"),
                author_time_secs: 0,
                summary: Box::from(format!("c{i}")),
            })
            .collect(),
    );
    v
}

fn test_diff() -> CommitDiff {
    let sha1 = ObjectId::empty_tree(gix::hash::Kind::Sha1);
    CommitDiff {
        commit_id: sha1,
        parent_ids: vec![],
        author_name: Arc::from("Alice"),
        author_email: Arc::from("alice@example.com"),
        author_date: "Mon Sep 1 12:00:00 2026 +0000".to_string(),
        committer_name: Arc::from("Alice"),
        committer_email: Arc::from("alice@example.com"),
        committer_date: "Mon Sep 1 12:00:00 2026 +0000".to_string(),
        title: Arc::from("Add important feature"),
        body: Some("Detailed notes on the feature.".to_string()),
        files: vec![FileDiff {
            path: "src/main.rs".to_string(),
            status: FileChangeStatus::Modified,
            old_id: Some(sha1),
            new_id: Some(sha1),
            old_mode: Some(0o100_644),
            new_mode: Some(0o100_644),
            is_binary: false,

            additions: 1,
            deletions: 0,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_len: 1,
                new_start: 1,
                new_len: 2,
                func_context: None,
                lines: vec![
                    HunkLine {
                        kind: DiffLineKind::Context,
                        content: "fn main() {".to_string(),
                        no_newline_at_eof: false,
                    },
                    HunkLine {
                        kind: DiffLineKind::Add,
                        content: "    println!();".to_string(),
                        no_newline_at_eof: false,
                    },
                ],
            }],
        }],
        stats: DiffSummaryStats {
            files_changed: 1,
            insertions: 1,
            deletions: 0,
        },
    }
}

fn app_with(n: usize) -> AppState {
    AppState {
        views: ViewManager {
            main_view: Some(view_with(n)),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn sample_status_report() -> tigrs_git::StatusReport {
    tigrs_git::StatusReport {
        staged: vec![tigrs_git::StatusItem::new(
            'M',
            tigrs_git::StatusSection::Staged,
            "src/main.rs",
            None,
        )],
        unstaged: vec![tigrs_git::StatusItem::new(
            'D',
            tigrs_git::StatusSection::Unstaged,
            "README.md",
            None,
        )],
        untracked: Vec::new(),
        unmerged: Vec::new(),
        branch: "main".to_string(),
        head_commit: None,
    }
}

#[test]
fn test_quit_keys() {
    let mut app = app_with(3);
    assert_eq!(
        handle_event(&key(KeyCode::Char('q')), &mut app, 10),
        Flow::Quit
    );
    assert_eq!(handle_event(&key(KeyCode::Esc), &mut app, 10), Flow::Quit);
    assert_eq!(handle_event(&ctrl_key('c'), &mut app, 10), Flow::Quit);
}

#[test]
fn test_navigation_keys_move_cursor() {
    let mut app = app_with(50);
    handle_event(&key(KeyCode::Char('j')), &mut app, 10);
    handle_event(&key(KeyCode::Char('j')), &mut app, 10);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 2);

    handle_event(&key(KeyCode::Char('k')), &mut app, 10);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 1);

    handle_event(&key(KeyCode::End), &mut app, 10);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 49);

    handle_event(&key(KeyCode::Home), &mut app, 10);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);
}

#[test]
fn test_diff_view_stacked_navigation_and_pop() {
    let mut app = app_with(10);
    app.views.diff_view = Some(DiffView::new(test_diff()));

    // In DiffView, 'j' advances diff cursor
    handle_event(&key(KeyCode::Char('j')), &mut app, 10);
    assert_eq!(app.views.diff_view.as_ref().unwrap().cursor_index(), 1);

    // 'q' in stacked diff view returns to MainView without quitting
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 10);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.diff_view.is_none());
    assert!(app.views.main_view.is_some());

    // Now in MainView, 'q' quits
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 10);
    assert_eq!(flow, Flow::Quit);
}

#[test]
fn test_standalone_diff_view_quit() {
    let mut app = AppState {
        views: ViewManager {
            diff_view: Some(DiffView::new(test_diff())),
            ..Default::default()
        },
        ..Default::default()
    };

    // 'q' in standalone diff view exits directly
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 10);
    assert_eq!(flow, Flow::Quit);
}

#[test]
fn test_status_view_stacked_navigation_and_pop() {
    let mut app = app_with(10);
    app.views.status_view = Some(StatusView::new(sample_status_report()));

    // In StatusView, initial cursor is at 1 (first item: src/main.rs)
    assert_eq!(app.views.status_view.as_ref().unwrap().cursor_index(), 1);

    // 'j' moves cursor down, skipping empty row
    handle_event(&key(KeyCode::Char('j')), &mut app, 10);
    assert_eq!(app.views.status_view.as_ref().unwrap().cursor_index(), 3);

    // 's' in stacked StatusView toggles back to MainView
    let flow = handle_event(&key(KeyCode::Char('s')), &mut app, 10);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.status_view.is_none());
    assert!(app.views.main_view.is_some());

    // Re-open status view
    app.views.status_view = Some(StatusView::new(sample_status_report()));

    // 'q' in stacked StatusView pops back to MainView
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 10);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.status_view.is_none());
    assert!(app.views.main_view.is_some());
}

#[test]
fn test_standalone_status_view_quit() {
    let mut app = AppState {
        views: ViewManager {
            status_view: Some(StatusView::new(sample_status_report())),
            ..Default::default()
        },
        ..Default::default()
    };

    // 'q' in standalone status view exits directly
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 10);
    assert_eq!(flow, Flow::Quit);
}

#[test]
fn test_status_view_to_diff_view_and_pop_back() {
    let mut app = AppState {
        views: ViewManager {
            status_view: Some(StatusView::new(sample_status_report())),
            ..Default::default()
        },
        ..Default::default()
    };

    // Manually push a DiffView as if 'd' / Enter was pressed
    app.views.diff_view = Some(DiffView::new(test_diff()));

    // 'q' in DiffView returns to StatusView without quitting
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 10);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.diff_view.is_none());
    assert!(app.views.status_view.is_some());

    // Now in StatusView, 'q' quits
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 10);
    assert_eq!(flow, Flow::Quit);
}

#[test]
fn test_unhandled_key_is_ignored() {
    let mut app = app_with(5);
    assert_eq!(
        handle_event(&key(KeyCode::Char('z')), &mut app, 10),
        Flow::Continue
    );
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);
}

#[test]
fn test_key_release_is_ignored() {
    let mut app = app_with(5);
    let release = Event::Key(KeyEvent::new_with_kind(
        KeyCode::Char('j'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    ));
    handle_event(&release, &mut app, 10);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);
}

#[test]
fn test_status_view_file_staging_with_engine() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&["init"]);
    run(&["config", "user.name", "Tester"]);
    run(&["config", "user.email", "test@test.com"]);
    std::fs::write(p.join("foo.txt"), "hello\n").unwrap();
    run(&["add", "foo.txt"]);
    run(&["commit", "-m", "initial"]);

    std::fs::write(p.join("foo.txt"), "hello modified\n").unwrap();

    let engine = GitEngine::open(Some(p)).unwrap();
    let (_src, token) = CancellationToken::new();
    let report = engine.load_status(&token).unwrap();
    assert_eq!(report.unstaged.len(), 1);
    assert_eq!(report.staged.len(), 0);

    let mut app = AppState {
        views: ViewManager {
            status_view: Some(StatusView::new(report)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };

    // Cursor is on item 1 (foo.txt, unstaged)
    assert!(!app.is_read_only());
    // 1. When Read-Only mode is enabled, pressing 'u' is blocked and shows READ_ONLY_WARNING_MSG
    app.options.read_only = true;
    app.sync_read_only_state();
    handle_event(&key(KeyCode::Char('u')), &mut app, 10);
    assert_eq!(
        app.status_message.as_deref(),
        Some(crate::app::READ_ONLY_WARNING_MSG)
    );
    let unchanged = app.views.status_view.as_ref().unwrap().report();
    assert_eq!(unchanged.staged.len(), 0);
    assert_eq!(unchanged.unstaged.len(), 1);

    // 2. Unlock Update Mode (`:set read-only = false`) and press 'u' to stage
    app.options.read_only = false;
    app.sync_read_only_state();
    handle_event(&key(KeyCode::Char('u')), &mut app, 10);
    let updated = app.views.status_view.as_ref().unwrap().report();
    assert_eq!(updated.staged.len(), 1);
    assert_eq!(updated.unstaged.len(), 0);

    // Press 'u' again to unstage
    handle_event(&key(KeyCode::Char('u')), &mut app, 10);
    let updated2 = app.views.status_view.as_ref().unwrap().report();
    assert_eq!(updated2.staged.len(), 0);
    assert_eq!(updated2.unstaged.len(), 1);

    // Press '!' to discard changes
    handle_event(&key(KeyCode::Char('!')), &mut app, 10);
    let updated3 = app.views.status_view.as_ref().unwrap().report();
    assert!(updated3.is_empty());
}

#[test]
fn test_diff_view_hunk_and_line_staging_with_engine() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&["init"]);
    run(&["config", "user.name", "Tester"]);
    run(&["config", "user.email", "test@test.com"]);
    std::fs::write(
        p.join("multi.txt"),
        "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )
    .unwrap();
    run(&["add", "multi.txt"]);
    run(&["commit", "-m", "initial"]);

    std::fs::write(
        p.join("multi.txt"),
        "line1_mod\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10_mod\n",
    )
    .unwrap();

    let engine = GitEngine::open(Some(p)).unwrap();
    let (_src, token) = CancellationToken::new();
    let report = engine.load_status(&token).unwrap();
    assert_eq!(report.unstaged.len(), 1);

    let mut app = AppState {
        views: ViewManager {
            status_view: Some(StatusView::new(report)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };

    // 1. Press Enter to open DiffView for multi.txt
    handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert!(app.views.diff_view.is_some());

    // 2. Jump to first hunk
    handle_event(&key(KeyCode::Char(')')), &mut app, 24);
    assert!(
        app.views
            .diff_view
            .as_ref()
            .unwrap()
            .selected_hunk()
            .is_some()
    );

    // 3a. When Read-Only mode is enabled, pressing 'u' is blocked and shows READ_ONLY_WARNING_MSG
    app.options.read_only = true;
    app.sync_read_only_state();
    handle_event(&key(KeyCode::Char('u')), &mut app, 24);
    assert_eq!(
        app.status_message.as_deref(),
        Some(crate::app::READ_ONLY_WARNING_MSG)
    );

    // 3b. Unlock Update Mode and press 'u' to stage hunk 1
    app.options.read_only = false;
    app.sync_read_only_state();
    handle_event(&key(KeyCode::Char('u')), &mut app, 24);

    // Check git diff --cached output
    let diff_cached = std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["diff", "--cached"])
        .current_dir(p)
        .output()
        .unwrap();
    let diff_str = String::from_utf8_lossy(&diff_cached.stdout);
    assert!(diff_str.contains("+line1_mod"));
    assert!(!diff_str.contains("+line10_mod"));

    // 4. In remaining unstaged diff, jump to remaining hunk (line10_mod)
    handle_event(&key(KeyCode::Char(')')), &mut app, 24);

    // Advance cursor until we land on the addition line (+line10_mod)
    for _ in 0..10 {
        if let Some((_, hunk, l_idx)) = app.views.diff_view.as_ref().unwrap().selected_line()
            && hunk.lines[l_idx].kind == DiffLineKind::Add
        {
            break;
        }
        handle_event(&key(KeyCode::Char('j')), &mut app, 24);
    }
    let sel = app.views.diff_view.as_ref().unwrap().selected_line();
    assert!(sel.is_some());
    assert_eq!(sel.unwrap().1.lines[sel.unwrap().2].kind, DiffLineKind::Add);

    // 5. Press '1' to stage single line
    handle_event(&key(KeyCode::Char('1')), &mut app, 24);

    let diff_cached2 = std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["diff", "--cached"])
        .current_dir(p)
        .output()
        .unwrap();
    let diff_str2 = String::from_utf8_lossy(&diff_cached2.stdout);
    assert!(diff_str2.contains("+line1_mod"));
    assert!(diff_str2.contains("+line10_mod"));
}

#[test]
fn test_tree_and_blob_view_stacked_navigation_with_engine() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&["init"]);
    run(&["config", "user.name", "Tester"]);
    run(&["config", "user.email", "test@test.com"]);
    std::fs::create_dir_all(p.join("src")).unwrap();
    std::fs::write(p.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(p.join("README.md"), "# Title\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-m", "init"]);

    let engine = GitEngine::open(Some(p)).unwrap();
    let mut app = AppState {
        views: ViewManager {
            main_view: Some(MainView::new("main".to_string())),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };

    // 1. Press 't' to open TreeView
    let flow = handle_event(&key(KeyCode::Char('t')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.tree_view.is_some());
    assert_eq!(app.views.tree_view.as_ref().unwrap().current_path(), "");

    // 2. Cursor is on first item: directory "src". Press Enter to descend.
    let flow = handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.views.tree_view.as_ref().unwrap().current_path(), "src");

    // Inside "src", rows are: [ParentDir, Entry(main.rs)]
    // Cursor is on 0 (ParentDir). Move down to main.rs
    handle_event(&key(KeyCode::Char('j')), &mut app, 24);
    assert_eq!(app.views.tree_view.as_ref().unwrap().cursor(), 1);

    // 3. Press Enter on main.rs to open BlobView
    let flow = handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.blob_view.is_some());
    assert_eq!(app.views.blob_view.as_ref().unwrap().path(), "src/main.rs");
    assert_eq!(app.views.blob_view.as_ref().unwrap().line_count(), 1);

    // 4. Press 'q' in BlobView to pop back to TreeView ("src")
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.blob_view.is_none());
    assert!(app.views.tree_view.is_some());
    assert_eq!(app.views.tree_view.as_ref().unwrap().current_path(), "src");

    // 5. Press 'q' in TreeView ("src") to ascend to parent ("")
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.tree_view.is_some());
    assert_eq!(app.views.tree_view.as_ref().unwrap().current_path(), "");

    // 6. Press 'q' at root TreeView to pop back to MainView
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.tree_view.is_none());
    assert!(app.views.main_view.is_some());

    // 7. Press 'q' in MainView to quit
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Quit);
}

#[test]
fn test_standalone_blame_view_quit() {
    let oid = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
    let view = BlameView::from_result(tigrs_git::BlameResult {
        commit_id: oid,
        path: "test.txt".to_string(),
        is_binary: false,
        lines: Vec::new(),
    });
    let mut app = AppState {
        views: ViewManager {
            blame_view: Some(view),
            ..Default::default()
        },
        ..Default::default()
    };

    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Quit);
}

#[test]
fn test_blame_view_stacked_navigation_with_engine() {
    use std::fs::File;
    use std::io::Write;
    use std::process::Command;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let p = dir.path();

    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(p)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success(), "Command failed: git {args:?}");
    };

    run(&["init"]);
    run(&["config", "user.name", "Blame Author"]);
    run(&["config", "user.email", "blame@example.com"]);

    // Commit 1: Initial
    let file_path = p.join("test.txt");
    let mut f = File::create(&file_path).unwrap();
    writeln!(f, "Line 1 - Initial").unwrap();
    writeln!(f, "Line 2 - Initial").unwrap();
    drop(f);

    run(&["add", "test.txt"]);
    run(&["commit", "-m", "Commit 1"]);

    let c1_hex = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["rev-parse", "HEAD"])
        .current_dir(p)
        .output()
        .unwrap();
    let c1_oid =
        ObjectId::from_hex(String::from_utf8_lossy(&c1_hex.stdout).trim().as_bytes()).unwrap();

    // Commit 2: Modify Line 2, add Line 3
    let mut f = File::create(&file_path).unwrap();
    writeln!(f, "Line 1 - Initial").unwrap();
    writeln!(f, "Line 2 - Modified").unwrap();
    writeln!(f, "Line 3 - New").unwrap();
    drop(f);

    run(&["add", "test.txt"]);
    run(&["commit", "-m", "Commit 2"]);

    let c2_hex = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["rev-parse", "HEAD"])
        .current_dir(p)
        .output()
        .unwrap();
    let c2_oid =
        ObjectId::from_hex(String::from_utf8_lossy(&c2_hex.stdout).trim().as_bytes()).unwrap();

    let engine = GitEngine::open(Some(p)).unwrap();
    let mut app = AppState {
        views: ViewManager {
            main_view: Some(MainView::new("main".to_string())),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };

    // 1. Open TreeView at root
    handle_event(&key(KeyCode::Char('t')), &mut app, 24);
    assert!(app.views.tree_view.is_some());

    // 2. Press 'b' on test.txt
    let flow = handle_event(&key(KeyCode::Char('b')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.blame_view.is_some());
    let bv = app.views.blame_view.as_ref().unwrap();
    assert_eq!(bv.path(), "test.txt");
    assert_eq!(bv.line_count(), 3);
    assert_eq!(bv.commit_oid(), c2_oid);

    // Line 0 is from Commit 1
    assert_eq!(bv.selected_line().unwrap().commit_id, c1_oid);

    // 3. Move cursor down to Line 1 (from Commit 2)
    handle_event(&key(KeyCode::Char('j')), &mut app, 24);
    let bv = app.views.blame_view.as_ref().unwrap();
    assert_eq!(bv.cursor(), 1);
    assert_eq!(bv.selected_line().unwrap().commit_id, c2_oid);

    // 4. Press Enter: opens DiffView for Commit 2!
    let flow = handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.diff_view.is_some());
    assert_eq!(app.views.diff_view.as_ref().unwrap().commit_id(), c2_oid);

    // 5. Press 'q' in DiffView: pops back to BlameView!
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.diff_view.is_none());
    assert!(app.views.blame_view.is_some());

    // 6. Press ',' in BlameView on line from Commit 2: blames parent commit (Commit 1)!
    let flow = handle_event(&key(KeyCode::Char(',')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.views.blame_view.as_ref().unwrap().commit_oid(), c1_oid);
    assert!(app.views.blame_view.as_ref().unwrap().has_history());

    // 7. Press Backspace in BlameView: restores history back to Commit 2!
    let flow = handle_event(&key(KeyCode::Backspace), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.views.blame_view.as_ref().unwrap().commit_oid(), c2_oid);

    // 8. Press 'q' in BlameView: pops back to TreeView!
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.blame_view.is_none());
    assert!(app.views.tree_view.is_some());

    // 9. Press 'q' in TreeView: pops back to MainView!
    let flow = handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.views.tree_view.is_none());
    assert!(app.views.main_view.is_some());
}

#[test]
fn test_app_multi_key_sequence_handling() {
    let mut app = app_with(50);
    // Bind vim-style 'gg' -> MoveFirstLine, 'G' -> MoveLastLine
    let generic = KeymapScope::Generic;
    app.keymap
        .bind(generic, KeySequence::parse("g").unwrap(), Action::None);
    app.keymap.bind(
        generic,
        KeySequence::parse("gg").unwrap(),
        Action::MoveFirstLine,
    );
    app.keymap.bind(
        generic,
        KeySequence::parse("G").unwrap(),
        Action::MoveLastLine,
    );
    app.keymap.bind(
        KeymapScope::Main,
        KeySequence::parse("G").unwrap(),
        Action::MoveLastLine,
    );

    // Move to row 10
    app.views.main_view.as_mut().unwrap().move_down(10, 20);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 10);

    // First 'g' is a prefix: pending keys should buffer 'g'
    let flow = handle_event(&key(KeyCode::Char('g')), &mut app, 20);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.pending_keys.len(), 1);
    assert_eq!(app.pending_keys_str(), "g-");
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 10);

    // Second 'g' completes 'gg': executes MoveFirstLine and clears pending
    let flow = handle_event(&key(KeyCode::Char('g')), &mut app, 20);
    assert_eq!(flow, Flow::Continue);
    assert!(app.pending_keys.is_empty());
    assert_eq!(app.pending_keys_str(), "");
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);

    // Test Esc cancels pending multi-key sequence without closing view
    app.views.main_view.as_mut().unwrap().move_down(5, 20);
    handle_event(&key(KeyCode::Char('g')), &mut app, 20);
    assert_eq!(app.pending_keys_str(), "g-");

    let flow = handle_event(&key(KeyCode::Esc), &mut app, 20);
    assert_eq!(flow, Flow::Continue);
    assert!(app.pending_keys.is_empty());
    assert_eq!(app.pending_keys_str(), "");
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 5);
    assert!(app.views.main_view.is_some());

    // Test G executes MoveLastLine immediately
    let flow = handle_event(&key(KeyCode::Char('G')), &mut app, 20);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 49);

    // Test invalid continuation drops buffer gracefully
    handle_event(&key(KeyCode::Char('g')), &mut app, 20);
    assert_eq!(app.pending_keys_str(), "g-");
    handle_event(&key(KeyCode::Char('z')), &mut app, 20);
    assert!(app.pending_keys.is_empty());
}

#[test]
fn test_app_prompt_line_jump() {
    let mut app = app_with(50);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);

    // 1. Press ':' to open Command prompt
    let flow = handle_event(&key(KeyCode::Char(':')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_some());
    assert_eq!(app.prompt.as_ref().unwrap().kind, PromptKind::Command);

    // 2. Type "42"
    handle_event(&key(KeyCode::Char('4')), &mut app, 24);
    handle_event(&key(KeyCode::Char('2')), &mut app, 24);
    assert_eq!(app.prompt.as_ref().unwrap().buffer, "42");

    // 3. Press Enter to submit
    let flow = handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_none());
    // Cursor jumps to 42 - 1 = 41
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 41);
}

#[test]
fn test_app_prompt_quit_and_cancel() {
    let mut app = app_with(10);

    // Test cancel with Esc
    handle_event(&key(KeyCode::Char(':')), &mut app, 24);
    assert!(app.prompt.is_some());
    handle_event(&key(KeyCode::Char('1')), &mut app, 24);
    let flow = handle_event(&key(KeyCode::Esc), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_none());
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);

    // Test :q quits
    handle_event(&key(KeyCode::Char(':')), &mut app, 24);
    handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    let flow = handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Quit);
}

#[test]
fn test_app_prompt_shell_command() {
    let mut app = app_with(10);
    assert!(!app.is_read_only());

    // 0. When Read-Only mode is enabled, `:!echo hello_tig` is blocked and shows READ_ONLY_WARNING_MSG
    app.options.read_only = true;
    handle_event(&key(KeyCode::Char(':')), &mut app, 24);
    for c in "!echo hello_tig".chars() {
        handle_event(&key(KeyCode::Char(c)), &mut app, 24);
    }
    let flow = handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.pending_handover.is_none());
    assert_eq!(
        app.status_message.as_deref(),
        Some(crate::app::READ_ONLY_WARNING_MSG)
    );

    // Unlock via `:set read-only = false`
    handle_event(&key(KeyCode::Char(':')), &mut app, 24);
    for c in "set read-only = false".chars() {
        handle_event(&key(KeyCode::Char(c)), &mut app, 24);
    }
    handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert!(!app.is_read_only());

    // 1. Open prompt and type "!echo hello_tig" (foreground command queues TTY handover)
    handle_event(&key(KeyCode::Char(':')), &mut app, 24);
    for c in "!echo hello_tig".chars() {
        handle_event(&key(KeyCode::Char(c)), &mut app, 24);
    }
    let flow = handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_none());
    assert!(matches!(
        app.pending_handover.take(),
        Some((ref command, _)) if command == "echo hello_tig"
    ));

    // 2. Open prompt and type "+echo hello_tig" (echo command captures output in status bar)
    handle_event(&key(KeyCode::Char(':')), &mut app, 24);
    for c in "+echo hello_tig".chars() {
        handle_event(&key(KeyCode::Char(c)), &mut app, 24);
    }
    let flow = handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_none());
    assert_eq!(app.status_message.as_deref(), Some("hello_tig"));
}

#[test]
fn test_app_prompt_search_forward_and_backward() {
    let mut app = app_with(20);

    // 1. Press '/' to open SearchForward prompt
    let flow = handle_event(&key(KeyCode::Char('/')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_some());
    assert_eq!(app.prompt.as_ref().unwrap().kind, PromptKind::SearchForward);

    // Search for "c7"
    for c in "c7".chars() {
        handle_event(&key(KeyCode::Char(c)), &mut app, 24);
    }
    let flow = handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_none());
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 7);

    // 2. Press '?' to open SearchBackward prompt
    let flow = handle_event(&key(KeyCode::Char('?')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_some());
    assert_eq!(
        app.prompt.as_ref().unwrap().kind,
        PromptKind::SearchBackward
    );

    // Search backward for "c3"
    for c in "c3".chars() {
        handle_event(&key(KeyCode::Char(c)), &mut app, 24);
    }
    let flow = handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_none());
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 3);
}

#[test]
fn test_app_interactive_macro_and_confirmation() {
    let mut app = app_with(10);
    assert!(!app.is_read_only());

    // 0. When Read-Only mode is enabled, Action::Run is blocked with READ_ONLY_WARNING_MSG
    app.options.read_only = true;
    let run_cmd = Arc::new(RunCommand {
        flags: RunFlags::ECHO,
        command: "echo %(prompt Enter greeting)".to_string(),
    });
    let flow = execute_action(&mut app, &Action::Run(Arc::clone(&run_cmd)), 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_none());
    assert_eq!(
        app.status_message.as_deref(),
        Some(crate::app::READ_ONLY_WARNING_MSG)
    );

    // Unlock Update Mode
    app.options.read_only = false;

    // 1. Action::Run with %(prompt <msg>) opens interactive prompt
    let flow = execute_action(&mut app, &Action::Run(run_cmd), 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_some());
    assert_eq!(
        app.prompt.as_ref().unwrap().prompt_prefix(),
        "Enter greeting: "
    );

    for c in "hello_world".chars() {
        handle_event(&key(KeyCode::Char(c)), &mut app, 24);
    }
    let flow = handle_event(&key(KeyCode::Enter), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_none());
    assert_eq!(app.status_message.as_deref(), Some("hello_world"));

    // 2. Action::Run with confirm = true
    let run_cmd_confirm = Arc::new(RunCommand {
        flags: RunFlags::CONFIRM | RunFlags::ECHO,
        command: "echo confirmed_msg".to_string(),
    });
    let flow = execute_action(&mut app, &Action::Run(run_cmd_confirm), 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_some());
    assert_eq!(
        app.prompt.as_ref().unwrap().prompt_prefix(),
        "Run: echo confirmed_msg? [y/N]"
    );

    // Confirm with 'y'
    let flow = handle_event(&key(KeyCode::Char('y')), &mut app, 24);
    assert_eq!(flow, Flow::Continue);
    assert!(app.prompt.is_none());
    assert_eq!(app.status_message.as_deref(), Some("confirmed_msg"));

    // Reject confirmation with 'n'
    let run_cmd_reject = Arc::new(RunCommand {
        flags: RunFlags::CONFIRM | RunFlags::ECHO,
        command: "echo rejected_msg".to_string(),
    });
    execute_action(&mut app, &Action::Run(run_cmd_reject), 24);
    assert!(app.prompt.is_some());
    handle_event(&key(KeyCode::Char('n')), &mut app, 24);
    assert!(app.prompt.is_none());
    assert_ne!(app.status_message.as_deref(), Some("rejected_msg"));
}

#[test]
fn test_app_prompt_history_draft_restoration() {
    let mut app = app_with(10);

    // Submit first command
    handle_event(&key(KeyCode::Char(':')), &mut app, 24);
    for c in "first_cmd".chars() {
        handle_event(&key(KeyCode::Char(c)), &mut app, 24);
    }
    handle_event(&key(KeyCode::Enter), &mut app, 24);

    // Open prompt again, type a draft "my_draft"
    handle_event(&key(KeyCode::Char(':')), &mut app, 24);
    for c in "my_draft".chars() {
        handle_event(&key(KeyCode::Char(c)), &mut app, 24);
    }
    assert_eq!(app.prompt.as_ref().unwrap().buffer, "my_draft");

    // Press Up: loads previous command "first_cmd"
    handle_event(&key(KeyCode::Up), &mut app, 24);
    assert_eq!(app.prompt.as_ref().unwrap().buffer, "first_cmd");

    // Press Down: restores draft "my_draft"
    handle_event(&key(KeyCode::Down), &mut app, 24);
    assert_eq!(app.prompt.as_ref().unwrap().buffer, "my_draft");
}

#[test]
fn test_app_search_repeat_and_wrap() {
    let mut app = app_with(10);

    // 1. Repeat search with 'n' before any search query is set
    handle_event(&key(KeyCode::Char('n')), &mut app, 24);
    assert_eq!(
        app.status_message.as_deref(),
        Some("No previous search query")
    );

    // 2. Search for "c7"
    execute_search(&mut app, "c7", SearchDirection::Forward, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 7);
    assert_eq!(app.status_message, None);

    // 3. Press 'n' (FindNext) when only 1 match exists: wraps around to top
    handle_event(&key(KeyCode::Char('n')), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 7);
    assert_eq!(app.status_message.as_deref(), Some("Search wrapped to top"));

    // 4. Press 'N' (FindPrev): searches backward, wraps around to bottom
    handle_event(&key(KeyCode::Char('N')), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 7);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Search wrapped to bottom")
    );

    // 5. Search for "c" (matches every commit)
    app.views.main_view.as_mut().unwrap().set_cursor(0, 24);
    execute_search(&mut app, "c", SearchDirection::Forward, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 1);

    handle_event(&key(KeyCode::Char('n')), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 2);

    handle_event(&key(KeyCode::Char('n')), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 3);

    // Reverse with 'N'
    handle_event(&key(KeyCode::Char('N')), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 2);
}

#[test]
fn test_app_search_not_found() {
    let mut app = app_with(10);
    execute_search(&mut app, "nonexistent", SearchDirection::Forward, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Pattern not found: 'nonexistent'")
    );
}

#[test]
fn test_app_search_in_diff_and_tree_view() {
    // Test search in DiffView
    let diff = test_diff();
    let mut app = AppState {
        views: ViewManager {
            diff_view: Some(DiffView::new(diff)),
            ..Default::default()
        },
        ..Default::default()
    };
    app.push_view(ViewKind::Diff);

    // In test_diff, file path is "src/main.rs"
    execute_search(&mut app, "main.rs", SearchDirection::Forward, 24);
    assert!(app.views.diff_view.as_ref().unwrap().cursor_index() > 0);

    // Test search in TreeView
    let listing = tigrs_git::TreeListing {
        commit_oid: ObjectId::empty_tree(gix::hash::Kind::Sha1),
        path: String::new(),
        parent_path: None,
        entries: vec![
            tigrs_git::TreeEntry {
                oid: ObjectId::empty_tree(gix::hash::Kind::Sha1),
                name: "Cargo.toml".to_string(),
                path: "Cargo.toml".to_string(),
                kind: tigrs_git::TreeEntryKind::Blob,
                mode: 0o100_644,
                size: Some(100),
            },
            tigrs_git::TreeEntry {
                oid: ObjectId::empty_tree(gix::hash::Kind::Sha1),
                name: "src".to_string(),
                path: "src".to_string(),
                kind: tigrs_git::TreeEntryKind::Tree,
                mode: 0o040_000,
                size: None,
            },
        ],
    };
    let mut app_tree = AppState {
        views: ViewManager {
            tree_view: Some(TreeView::new(listing)),
            ..Default::default()
        },
        ..Default::default()
    };
    app_tree.push_view(ViewKind::Tree);

    execute_search(&mut app_tree, "src", SearchDirection::Forward, 24);
    assert_eq!(app_tree.views.tree_view.as_ref().unwrap().cursor(), 1);
}

#[test]
fn test_app_option_menu_workflow() {
    let mut app = app_with(5);

    // 1. Press 'o' in Main view -> opens context-aware on 3:History & Graph (item 0 = Revision Graph, item 4 = Date Format)
    handle_event(&key(KeyCode::Char('o')), &mut app, 24);
    assert!(app.prompt.is_some());
    if let Some(PromptState {
        kind: PromptKind::OptionMenu(menu),
        ..
    }) = &app.prompt
    {
        assert_eq!(menu.category, Some(crate::options::OptionCategory::History));
    } else {
        panic!("Expected OptionMenu prompt");
    }

    // 2. Use inline '/' filter to jump directly to Date Format, press Enter to exit filter typing, then Enter to cycle in-place
    handle_event(&key(KeyCode::Char('/')), &mut app, 24);
    for ch in "date".chars() {
        handle_event(&key(KeyCode::Char(ch)), &mut app, 24);
    }
    handle_event(&key(KeyCode::Enter), &mut app, 24); // exit filter input mode
    handle_event(&key(KeyCode::Enter), &mut app, 24); // cycle Date Format in-place (menu stays open!)
    assert!(app.prompt.is_some());
    assert_eq!(app.options.date_format, crate::options::DateFormat::Short);
    assert_eq!(app.status_message.as_deref(), Some(":set date = short"));

    // 3. First Esc clears the active '/' filter; second Esc closes the menu
    handle_event(&key(KeyCode::Esc), &mut app, 24);
    assert!(app.prompt.is_some());
    handle_event(&key(KeyCode::Esc), &mut app, 24);
    assert!(app.prompt.is_none());

    // 4. Press 'o' and then press direct hotkey 'W' to toggle ignore-space in-place, then 'q' to close
    handle_event(&key(KeyCode::Char('o')), &mut app, 24);
    handle_event(&key(KeyCode::Char('W')), &mut app, 24);
    assert!(app.prompt.is_some());
    assert_eq!(app.options.ignore_space, crate::options::IgnoreSpace::All);
    assert_eq!(
        app.status_message.as_deref(),
        Some(":set ignore-space = all")
    );
    handle_event(&key(KeyCode::Char('q')), &mut app, 24);
    assert!(app.prompt.is_none());

    // 5. Press 'o' and then press hotkey '|' to toggle vertical-split in-place, then 'o' to close
    handle_event(&key(KeyCode::Char('o')), &mut app, 24);
    handle_event(&key(KeyCode::Char('|')), &mut app, 24);
    assert!(app.prompt.is_some());
    assert!(app.options.vertical_split);
    assert_eq!(
        app.status_message.as_deref(),
        Some(":set vertical-split = yes")
    );
    handle_event(&key(KeyCode::Char('o')), &mut app, 24);
    assert!(app.prompt.is_none());
}

#[test]
fn test_app_direct_toggle_keys() {
    let mut app = app_with(5);

    // Direct key '#' or '.' toggles line-number
    handle_event(&key(KeyCode::Char('#')), &mut app, 24);
    assert!(app.options.line_number);
    assert_eq!(
        app.status_message.as_deref(),
        Some(":set line-number = yes")
    );

    handle_event(&key(KeyCode::Char('.')), &mut app, 24);
    assert!(!app.options.line_number);
    assert_eq!(app.status_message.as_deref(), Some(":set line-number = no"));

    // Direct key 'D' cycles date
    handle_event(&key(KeyCode::Char('D')), &mut app, 24);
    assert_eq!(app.options.date_format, crate::options::DateFormat::Short);

    // Direct key 'A' cycles author
    handle_event(&key(KeyCode::Char('A')), &mut app, 24);
    assert_eq!(
        app.options.author_format,
        crate::options::AuthorFormat::Abbreviated
    );

    // Direct key 'X' toggles id
    handle_event(&key(KeyCode::Char('X')), &mut app, 24);
    assert!(app.options.commit_id);

    // Direct key 'w' toggles word-diff (default is true)
    handle_event(&key(KeyCode::Char('w')), &mut app, 24);
    assert!(!app.options.word_diff);
    assert_eq!(app.status_message.as_deref(), Some(":set word-diff = no"));
}

#[test]
fn test_app_toggle_via_prompt_command() {
    let mut app = app_with(5);

    // Run :toggle line-number
    let parsed = ParsedCommand::parse("toggle line-number");
    execute_parsed_command(&mut app, parsed, 24);
    assert!(app.options.line_number);
    assert_eq!(
        app.status_message.as_deref(),
        Some(":set line-number = yes")
    );

    // Run :toggle date
    let parsed = ParsedCommand::parse(":toggle date");
    execute_parsed_command(&mut app, parsed, 24);
    assert_eq!(app.options.date_format, crate::options::DateFormat::Short);
}

#[test]
fn test_view_rendering_with_options() {
    let mut app = app_with(3);
    let mut buf = Vec::new();

    // 1. Default render: no line number
    render_active(&app, &mut buf, 80, 24).unwrap();
    let rendered_default = String::from_utf8_lossy(&buf);
    assert!(!rendered_default.contains("   1 "));

    // 2. Enable line numbers: MainView does NOT render row numbers (only source code / diff views do)
    app.options.line_number = true;
    buf.clear();
    render_active(&app, &mut buf, 80, 24).unwrap();
    let rendered_lineno = String::from_utf8_lossy(&buf);
    assert!(!rendered_lineno.contains("   1 "));

    // 3. Enable commit_id: renders short SHA
    app.options.commit_id = true;
    buf.clear();
    render_active(&app, &mut buf, 80, 24).unwrap();
    let rendered_id = String::from_utf8_lossy(&buf);
    let short_sha = app.views.main_view.as_ref().unwrap().commits()[0].short_id();
    assert!(rendered_id.contains(&short_sha));
}

#[test]
fn test_split_view_vertical_rendering() {
    let mut app = app_with(5);
    app.push_view(ViewKind::Main);
    app.views.diff_view = Some(DiffView::new(test_diff()));
    app.push_view(ViewKind::Diff);
    app.options.vertical_split = true;

    let mut term = crate::headless::HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term, 80, 24).unwrap();

    let left_title = term.line_text_raw(0);
    assert!(left_title.contains("[main]"));
    assert!(left_title.contains('│'));
    assert!(left_title.contains("[diff]"));
}

#[test]
fn test_split_view_horizontal_rendering() {
    let mut app = app_with(5);
    app.push_view(ViewKind::Main);
    app.views.diff_view = Some(DiffView::new(test_diff()));
    app.push_view(ViewKind::Diff);
    app.options.vertical_split = false;

    let mut term = crate::headless::HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term, 80, 24).unwrap();

    term.assert_line_contains(0, "[main]");
    term.assert_line_contains(12, "[diff]");
}

#[test]
fn test_split_view_tab_focus_switching() {
    let mut app = app_with(5);
    app.push_view(ViewKind::Main);
    app.views.diff_view = Some(DiffView::new(test_diff()));
    app.push_view(ViewKind::Diff);
    app.options.vertical_split = true;

    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    let (primary, secondary) = app.split_views().unwrap();
    assert_eq!(primary, ViewKind::Main);
    assert_eq!(secondary, ViewKind::Diff);

    handle_event(&key(KeyCode::Tab), &mut app, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    let (primary2, secondary2) = app.split_views().unwrap();
    assert_eq!(primary2, ViewKind::Main);
    assert_eq!(secondary2, ViewKind::Diff);

    handle_event(&key(KeyCode::Tab), &mut app, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
}

#[test]
fn test_split_view_maximize_toggle() {
    let mut app = app_with(5);
    app.push_view(ViewKind::Main);
    app.views.diff_view = Some(DiffView::new(test_diff()));
    app.push_view(ViewKind::Diff);

    assert!(!app.views.maximized);

    handle_event(&key(KeyCode::Char('O')), &mut app, 24);
    assert!(app.views.maximized);
    assert_eq!(app.status_message.as_deref(), Some("View maximized"));

    let mut term = crate::headless::HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term, 80, 24).unwrap();
    term.assert_line_contains(0, "[diff]");

    handle_event(&key(KeyCode::Char('O')), &mut app, 24);
    assert!(!app.views.maximized);
    assert_eq!(app.status_message.as_deref(), Some("View split restored"));

    term.clear();
    render_active(&app, &mut term, 80, 24).unwrap();
    term.assert_line_contains(0, "[main]");
    term.assert_line_contains(12, "[diff]");
}

#[test]
fn test_split_view_cursor_diff_sync() {
    let tmp = tempfile::TempDir::new().unwrap();
    let p = tmp.path();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&["init"]);
    run(&["config", "user.name", "Tester"]);
    run(&["config", "user.email", "test@test.com"]);

    std::fs::write(p.join("c1.txt"), "commit 1\n").unwrap();
    run(&["add", "c1.txt"]);
    run(&["commit", "-m", "first commit"]);

    std::fs::write(p.join("c2.txt"), "commit 2\n").unwrap();
    run(&["add", "c2.txt"]);
    run(&["commit", "-m", "second commit"]);

    let engine = GitEngine::open(Some(p)).unwrap();
    let head_id = engine.head_commit_id().unwrap();

    let mut app = AppState {
        views: ViewManager {
            main_view: Some(MainView::new("main".to_string())),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };

    if let (Some(main), Some(eng)) = (&mut app.views.main_view, &app.engine) {
        let (_src, token) = CancellationToken::new();
        if let Ok(iter) = eng.stream_commits(Some(head_id), Some(10), token) {
            for batch in iter.flatten() {
                main.append_commits(batch);
            }
        }
        main.set_finished();
    }

    let commits = app.views.main_view.as_ref().unwrap().commits().to_vec();
    assert_eq!(commits.len(), 2);
    let c2_id = commits[0].id;
    let c1_id = commits[1].id;

    app.push_view(ViewKind::Main);
    // Open DiffView for c2
    let diff_data = app
        .engine
        .as_ref()
        .unwrap()
        .compute_commit_diff(c2_id)
        .unwrap();
    app.views.diff_view = Some(DiffView::new(diff_data));
    app.push_view(ViewKind::Diff);

    // Tab focus to MainView
    handle_event(&key(KeyCode::Tab), &mut app, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Main));
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(c2_id)
    );

    // Move cursor down in MainView -> moves to c1
    handle_event(&key(KeyCode::Down), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 1);
    // Split diff should have automatically synchronized to c1!
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(c1_id)
    );

    // Move cursor up in MainView -> moves back to c2
    handle_event(&key(KeyCode::Up), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);
    // Split diff should have automatically synchronized back to c2!
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(c2_id)
    );
}

#[test]
fn test_split_view_status_staging_sync() {
    let tmp = tempfile::TempDir::new().unwrap();
    let p = tmp.path();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&["init"]);
    run(&["config", "user.name", "Tester"]);
    run(&["config", "user.email", "test@test.com"]);

    std::fs::write(p.join("file_a.txt"), "hello a\n").unwrap();
    std::fs::write(p.join("file_b.txt"), "hello b\n").unwrap();
    run(&["add", "file_a.txt", "file_b.txt"]);
    run(&["commit", "-m", "initial"]);

    std::fs::write(p.join("file_a.txt"), "hello a modified\n").unwrap();
    std::fs::write(p.join("file_b.txt"), "hello b modified\n").unwrap();

    let engine = GitEngine::open(Some(p)).unwrap();
    let (_src, token) = CancellationToken::new();
    let report = engine.load_status(&token).unwrap();
    assert_eq!(report.unstaged.len(), 2);

    let mut app = AppState {
        views: ViewManager {
            status_view: Some(StatusView::new(report)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };

    let initial_item = app
        .views
        .status_view
        .as_ref()
        .unwrap()
        .selected_item()
        .unwrap()
        .clone();
    let initial_diff = app
        .engine
        .as_ref()
        .unwrap()
        .compute_status_item_diff(&initial_item)
        .unwrap();
    app.views.diff_view = Some(DiffView::new_status(initial_diff, initial_item.clone()));

    app.push_view(ViewKind::Status);
    app.push_view(ViewKind::Diff);

    // Tab focus to StatusView
    handle_event(&key(KeyCode::Tab), &mut app, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Status));

    // Move down to next file in StatusView
    handle_event(&key(KeyCode::Down), &mut app, 24);
    let next_item = app
        .views
        .status_view
        .as_ref()
        .unwrap()
        .selected_item()
        .unwrap()
        .clone();
    assert_ne!(initial_item.path, next_item.path);

    // Split diff should have automatically synchronized to next_item!
    let diff_item = app
        .views
        .diff_view
        .as_ref()
        .and_then(DiffView::status_item)
        .unwrap();
    assert_eq!(diff_item.path, next_item.path);
}

#[test]
fn test_prompt_command_line_number_and_quit() {
    let mut app = AppState {
        views: ViewManager {
            help_view: Some(HelpView::new()),
            ..Default::default()
        },
        ..Default::default()
    };
    app.push_view(ViewKind::Help);

    // Jump to line 5
    let flow = execute_parsed_command(&mut app, ParsedCommand::LineNumber(5), 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.views.help_view.as_ref().unwrap().cursor(), 4);

    // Execute quit command :q
    let quit_cmd = ParsedCommand::parse(":q");
    let flow_quit = execute_parsed_command(&mut app, quit_cmd, 24);
    assert_eq!(flow_quit, Flow::Quit);
}

#[test]
fn test_search_in_refs_and_help_view() {
    let mut app = AppState {
        views: ViewManager {
            help_view: Some(HelpView::new()),
            ..Default::default()
        },
        ..Default::default()
    };
    app.push_view(ViewKind::Help);

    // Search for "view-diff"
    execute_search(&mut app, "view-diff", SearchDirection::Forward, 24);
    let cursor_help = app.views.help_view.as_ref().unwrap().cursor();
    assert!(cursor_help > 0);

    // Search in RefsView
    let oid = ObjectId::from_hex(b"0123456789abcdef0123456789abcdef01234567").unwrap();
    let refs = vec![
        tigrs_git::RefEntry {
            name: "main".to_string(),
            full_name: "refs/heads/main".to_string(),
            kind: tigrs_git::RefKind::LocalBranch,
            commit_id: oid,
            summary: "Initial commit".to_string(),
            author_name: "Alice".to_string(),
            author_time_secs: 1_700_000_000,
        },
        tigrs_git::RefEntry {
            name: "release-candidate".to_string(),
            full_name: "refs/tags/release-candidate".to_string(),
            kind: tigrs_git::RefKind::Tag,
            commit_id: oid,
            summary: "RC1".to_string(),
            author_name: "Bob".to_string(),
            author_time_secs: 1_700_001_000,
        },
    ];
    let mut app_refs = AppState {
        views: ViewManager {
            refs_view: Some(RefsView::new(refs)),
            ..Default::default()
        },
        ..Default::default()
    };
    app_refs.push_view(ViewKind::Refs);

    execute_search(&mut app_refs, "release", SearchDirection::Forward, 24);
    assert_eq!(app_refs.views.refs_view.as_ref().unwrap().cursor(), 1);
}

#[test]
fn test_search_in_all_remaining_views() {
    let oid = ObjectId::from_hex(b"0123456789abcdef0123456789abcdef01234567").unwrap();

    // 1. BlobView
    let blob = tigrs_git::BlobContent {
        oid,
        path: "file.rs".to_string(),
        size: 20,
        lines: vec![
            "first line".to_string(),
            "target needle in blob".to_string(),
        ]
        .into(),
        is_binary: false,
    };
    let mut app_blob = AppState {
        views: ViewManager {
            blob_view: Some(BlobView::new(oid, blob)),
            ..Default::default()
        },
        ..Default::default()
    };
    app_blob.push_view(ViewKind::Blob);
    execute_search(&mut app_blob, "needle", SearchDirection::Forward, 24);
    assert_eq!(app_blob.views.blob_view.as_ref().unwrap().cursor(), 1);

    // 2. BlameView
    let blame_res = tigrs_git::BlameResult {
        commit_id: oid,
        path: "test.rs".to_string(),
        is_binary: false,
        lines: vec![
            tigrs_git::BlameLine {
                line_number: 1,
                commit_id: oid,
                short_commit_id: Arc::from("11111111"),
                author: Arc::from("Alice"),
                author_date: Arc::from("2026-09-10"),
                summary: Arc::from("First"),
                content: "hello".to_string(),
                is_hunk_start: true,
                parent_commit_id: None,
                source_path: None,
                source_line_number: 1,
            },
            tigrs_git::BlameLine {
                line_number: 2,
                commit_id: oid,
                short_commit_id: Arc::from("22222222"),
                author: Arc::from("BobTarget"),
                author_date: Arc::from("2026-09-12"),
                summary: Arc::from("Second"),
                content: "world".to_string(),
                is_hunk_start: true,
                parent_commit_id: None,
                source_path: None,
                source_line_number: 2,
            },
        ],
    };
    let mut app_blame = AppState {
        views: ViewManager {
            blame_view: Some(BlameView::from_result(blame_res)),
            ..Default::default()
        },
        ..Default::default()
    };
    app_blame.push_view(ViewKind::Blame);
    execute_search(&mut app_blame, "BobTarget", SearchDirection::Forward, 24);
    assert_eq!(app_blame.views.blame_view.as_ref().unwrap().cursor(), 1);

    // 3. StatusView
    let status_report = tigrs_git::StatusReport {
        staged: vec![
            tigrs_git::StatusItem::new('M', tigrs_git::StatusSection::Staged, "file_a.txt", None),
            tigrs_git::StatusItem::new(
                'A',
                tigrs_git::StatusSection::Staged,
                "special_target.txt",
                None,
            ),
        ],
        ..Default::default()
    };
    let mut app_status = AppState {
        views: ViewManager {
            status_view: Some(StatusView::new(status_report)),
            ..Default::default()
        },
        ..Default::default()
    };
    app_status.push_view(ViewKind::Status);
    execute_search(
        &mut app_status,
        "special_target",
        SearchDirection::Forward,
        24,
    );
    assert!(
        app_status
            .views
            .status_view
            .as_ref()
            .unwrap()
            .cursor_index()
            > 0
    );

    // 4. StashView
    let stashes = vec![
        tigrs_git::StashEntry {
            index: 0,
            commit_id: oid,
            summary: "first stash".to_string(),
            time_secs: 100,
        },
        tigrs_git::StashEntry {
            index: 1,
            commit_id: oid,
            summary: "my stash needle".to_string(),
            time_secs: 200,
        },
    ];
    let mut app_stash = AppState {
        views: ViewManager {
            stash_view: Some(StashView::new(stashes)),
            ..Default::default()
        },
        ..Default::default()
    };
    app_stash.push_view(ViewKind::Stash);
    execute_search(&mut app_stash, "needle", SearchDirection::Forward, 24);
    assert_eq!(app_stash.views.stash_view.as_ref().unwrap().cursor(), 1);

    // 5. GrepView
    let grep_matches = vec![
        GrepMatch {
            path: "src/a.rs".to_string(),
            line_num: 1,
            content: "nothing".to_string(),
        },
        GrepMatch {
            path: "src/b.rs".to_string(),
            line_num: 10,
            content: "found keyword here".to_string(),
        },
    ];
    let mut app_grep = AppState {
        views: ViewManager {
            grep_view: Some(GrepView::new("keyword".to_string(), grep_matches)),
            ..Default::default()
        },
        ..Default::default()
    };
    app_grep.push_view(ViewKind::Grep);
    execute_search(&mut app_grep, "found keyword", SearchDirection::Forward, 24);
    assert_eq!(app_grep.views.grep_view.as_ref().unwrap().cursor(), 1);

    // 6. ReflogView
    let reflog_entries = vec![
        tigrs_git::ReflogEntry {
            index: 0,
            old_id: oid,
            new_id: oid,
            committer_name: "Tester".to_string(),
            time_secs: 100,
            message: "commit 1".to_string(),
        },
        tigrs_git::ReflogEntry {
            index: 1,
            old_id: oid,
            new_id: oid,
            committer_name: "Tester".to_string(),
            time_secs: 200,
            message: "rebase target entry".to_string(),
        },
    ];
    let mut app_reflog = AppState {
        views: ViewManager {
            reflog_view: Some(ReflogView::new("HEAD".to_string(), reflog_entries)),
            ..Default::default()
        },
        ..Default::default()
    };
    app_reflog.push_view(ViewKind::Reflog);
    execute_search(
        &mut app_reflog,
        "rebase target",
        SearchDirection::Forward,
        24,
    );
    assert_eq!(app_reflog.views.reflog_view.as_ref().unwrap().cursor(), 1);

    // 7. PagerView
    let mut app_pager = AppState {
        views: ViewManager {
            pager_view: Some(PagerView::new(
                "log".to_string(),
                vec!["line 0".to_string(), "target line in pager".to_string()],
            )),
            ..Default::default()
        },
        ..Default::default()
    };
    app_pager.push_view(ViewKind::Pager);
    execute_search(&mut app_pager, "target line", SearchDirection::Forward, 24);
    assert_eq!(app_pager.views.pager_view.as_ref().unwrap().cursor(), 1);
}

#[test]
fn test_split_view_small_terminal_fallback() {
    let mut app = AppState {
        views: ViewManager {
            help_view: Some(HelpView::new()),
            pager_view: Some(PagerView::new(
                "log".to_string(),
                vec!["line 1".to_string()],
            )),
            ..Default::default()
        },
        ..Default::default()
    };
    app.options.vertical_split = true;
    app.push_view(ViewKind::Help);
    app.push_view(ViewKind::Pager);

    let mut buf = Vec::new();
    // Terminal smaller than min vertical split threshold (width < 30)
    let res = render_active_direct(&app, &mut buf, 20, 5);
    assert!(res.is_ok());
    let output = String::from_utf8_lossy(&buf);
    // Vertical split border '│' should NOT be present because it fell back to single view
    assert!(!output.contains('│'));
}

#[test]
fn test_diff_view_parent_navigation() {
    let tmp = tempfile::TempDir::new().unwrap();
    let p = tmp.path();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&["init"]);
    run(&["config", "user.name", "Tester"]);
    run(&["config", "user.email", "test@test.com"]);

    std::fs::write(p.join("file.txt"), "v1\n").unwrap();
    run(&["add", "file.txt"]);
    run(&["commit", "-m", "first"]);

    std::fs::write(p.join("file.txt"), "v2\n").unwrap();
    run(&["commit", "-a", "-m", "second"]);

    let engine = GitEngine::open(Some(p)).unwrap();
    let head_id = engine.head_commit_id().unwrap();
    let (_src, token) = CancellationToken::new();

    let mut main = MainView::new("main".to_string());
    if let Ok(iter) = engine.stream_commits(Some(head_id), Some(10), token) {
        for batch in iter.flatten() {
            main.append_commits(batch);
        }
    }
    main.set_finished();

    let commits = main.commits().to_vec();
    assert_eq!(commits.len(), 2);
    let c2_id = commits[0].id;
    let c1_id = commits[1].id;

    let diff_data = engine.compute_commit_diff(c2_id).unwrap();
    let mut app = AppState {
        views: ViewManager {
            main_view: Some(main),
            diff_view: Some(DiffView::new(diff_data)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Main);
    app.push_view(ViewKind::Diff);

    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(c2_id)
    );

    // Press ',' (Action::Parent) to go to previous/parent commit
    handle_event(&key(KeyCode::Char(',')), &mut app, 24);

    // DiffView should now show the parent commit (c1)!
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(c1_id)
    );
}

/// Builds a repo with `count` commits plus a Main->Diff stack focused on the diff.
fn diff_over_log_app(count: usize) -> (tempfile::TempDir, AppState, Vec<tigrs_git::ObjectId>) {
    let tmp = tempfile::TempDir::new().unwrap();
    let p = tmp.path();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&["init"]);
    run(&["config", "user.name", "Tester"]);
    run(&["config", "user.email", "test@test.com"]);

    for i in 0..count {
        std::fs::write(p.join("file.txt"), format!("v{i}\n")).unwrap();
        run(&["add", "file.txt"]);
        run(&["commit", "-m", &format!("commit {i}")]);
    }

    let engine = GitEngine::open(Some(p)).unwrap();
    let head_id = engine.head_commit_id().unwrap();
    let (_src, token) = CancellationToken::new();

    let mut main = MainView::new("main".to_string());
    if let Ok(iter) = engine.stream_commits(Some(head_id), Some(100), token) {
        for batch in iter.flatten() {
            main.append_commits(batch);
        }
    }
    main.set_finished();

    let ids: Vec<_> = main.commits().iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), count);

    let diff_data = engine.compute_commit_diff(ids[0]).unwrap();
    let mut app = AppState {
        views: ViewManager {
            main_view: Some(main),
            diff_view: Some(DiffView::new(diff_data)),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };
    app.push_view(ViewKind::Main);
    app.push_view(ViewKind::Diff);

    (tmp, app, ids)
}

/// Regression: in a commit diff view the arrow keys must scroll the diff
/// body, not jump commits. They were bound to `next`/`previous`, so `<Down>`
/// walked the log *upwards* and `<Up>` appeared dead because the diff was
/// already pinned to its first line.
#[test]
fn test_diff_view_arrow_keys_scroll_diff_not_commits() {
    let (_tmp, mut app, ids) = diff_over_log_app(3);

    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);
    assert_eq!(app.views.diff_view.as_ref().unwrap().cursor_index(), 0);

    // <Down> moves the diff cursor down and leaves the commit selection alone.
    handle_event(&key(KeyCode::Down), &mut app, 24);
    assert_eq!(
        app.views.diff_view.as_ref().unwrap().cursor_index(),
        1,
        "<Down> must advance the diff cursor"
    );
    assert_eq!(
        app.views.main_view.as_ref().unwrap().cursor_index(),
        0,
        "<Down> must not jump to another commit"
    );
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(ids[0]),
        "<Down> must keep showing the same commit"
    );

    // <Up> is the exact inverse and actually moves.
    handle_event(&key(KeyCode::Up), &mut app, 24);
    assert_eq!(
        app.views.diff_view.as_ref().unwrap().cursor_index(),
        0,
        "<Up> must move the diff cursor back"
    );
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(ids[0])
    );
}

/// `next`/`previous` steer the parent log view and are exact mirrors:
/// `J`/`<Ctrl-N>` step to the older commit, `K`/`<Ctrl-P>` back to the newer.
#[test]
fn test_next_previous_step_parent_view_symmetrically() {
    let (_tmp, mut app, ids) = diff_over_log_app(3);

    assert_eq!(app.parent_view(), Some(ViewKind::Main));

    handle_event(&key(KeyCode::Char('J')), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 1);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(ids[1])
    );

    handle_event(&key(KeyCode::Char('J')), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 2);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(ids[2])
    );

    handle_event(&key(KeyCode::Char('K')), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 1);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(ids[1])
    );

    handle_event(&key(KeyCode::Char('K')), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(ids[0])
    );

    // Clamped at the top: `previous` is a no-op rather than wrapping.
    handle_event(&key(KeyCode::Char('K')), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(ids[0])
    );

    // Ctrl-N / Ctrl-P are aliases with identical directions.
    handle_event(&ctrl_key('n'), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 1);
    handle_event(&ctrl_key('p'), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);
}

/// With no parent (a root view), `next`/`previous` degrade to moving the
/// focused view's own cursor, matching upstream Tig.
#[test]
fn test_next_previous_without_parent_moves_own_cursor() {
    let mut app = app_with(10);
    app.push_view(ViewKind::Main);

    assert_eq!(app.parent_view(), None);

    handle_event(&key(KeyCode::Char('J')), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 1);

    handle_event(&key(KeyCode::Char('K')), &mut app, 24);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);
}

#[test]
fn test_active_view_visible_height_calculation() {
    let mut app = app_with(50);
    // Single view: terminal 80x24 -> visible height is 24 - 2 = 22
    assert_eq!(app.active_view_visible_height(80, 24), 22);

    // Add DiffView and push both to stack for horizontal split
    app.push_view(ViewKind::Main);
    app.views.diff_view = Some(DiffView::new(test_diff()));
    app.push_view(ViewKind::Diff);

    // Terminal 80x24: top pane = 12 (visible 10), bottom pane = 12 (visible 10)
    // Active view is Diff (bottom/secondary) -> 10
    assert_eq!(app.active_view_visible_height(80, 24), 10);

    // Tab focus to Main (top/primary) -> 10
    handle_event(&key(KeyCode::Tab), &mut app, 10);
    assert_eq!(app.active_view(), Some(ViewKind::Main));
    assert_eq!(app.active_view_visible_height(80, 24), 10);

    // Odd terminal height: 25 -> top_h = 12 (visible 10), bottom_h = 13 (visible 11)
    assert_eq!(app.active_view_visible_height(80, 25), 10);
    // Tab back to Diff (bottom)
    handle_event(&key(KeyCode::Tab), &mut app, 10);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert_eq!(app.active_view_visible_height(80, 25), 11);

    // Vertical split: full vertical height minus header/status
    app.options.vertical_split = true;
    assert_eq!(app.active_view_visible_height(80, 24), 22);
    app.options.vertical_split = false;

    // Maximized view: full height minus header/status
    app.views.maximized = true;
    assert_eq!(app.active_view_visible_height(80, 24), 22);
    app.views.maximized = false;

    // Small terminal below split threshold: fallback to single view height
    assert_eq!(app.active_view_visible_height(15, 5), 3);
}

#[test]
fn test_split_view_cursor_scrolls_down_at_pane_boundary() {
    let mut app = app_with(50);
    app.push_view(ViewKind::Main);
    app.views.diff_view = Some(DiffView::new(test_diff()));
    app.push_view(ViewKind::Diff);

    // Switch focus to MainView in top pane
    handle_event_with_dimensions(&key(KeyCode::Tab), &mut app, 80, 24);
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    // In 80x24 terminal with horizontal split, top pane has 10 visible content rows.
    let main = app.views.main_view.as_ref().unwrap();
    assert_eq!(main.cursor_index(), 0);
    assert_eq!(main.scroll_offset(), 0);

    // Move down 9 times to reach the bottom visible row of the pane (index 9)
    for _ in 0..9 {
        handle_event_with_dimensions(&key(KeyCode::Down), &mut app, 80, 24);
    }
    let main = app.views.main_view.as_ref().unwrap();
    assert_eq!(main.cursor_index(), 9);
    assert_eq!(main.scroll_offset(), 0);

    // Stepping past the bottom row (10th move) MUST roll the view down (scroll_offset = 1)
    handle_event_with_dimensions(&key(KeyCode::Down), &mut app, 80, 24);
    let main = app.views.main_view.as_ref().unwrap();
    assert_eq!(main.cursor_index(), 10);
    assert_eq!(
        main.scroll_offset(),
        1,
        "View must roll down when cursor steps beyond visible pane height"
    );

    // Move down 5 more times
    for _ in 0..5 {
        handle_event_with_dimensions(&key(KeyCode::Down), &mut app, 80, 24);
    }
    let main = app.views.main_view.as_ref().unwrap();
    assert_eq!(main.cursor_index(), 15);
    assert_eq!(main.scroll_offset(), 6);

    // Move back up to the top
    for _ in 0..15 {
        handle_event_with_dimensions(&key(KeyCode::Up), &mut app, 80, 24);
    }
    let main = app.views.main_view.as_ref().unwrap();
    assert_eq!(main.cursor_index(), 0);
    assert_eq!(main.scroll_offset(), 0);
}

#[test]
fn test_scroll_actions_ctrl_e_and_ctrl_y() {
    let mut app = app_with(50);
    let visible = 10;
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 0);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);

    // Ctrl-E (ScrollLineDown): scrolls content down by 1 line
    handle_event(&ctrl_key('e'), &mut app, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 1);
    // Cursor pulled along if it fell out of the viewport
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 1);

    // Scroll down 4 more lines
    for _ in 0..4 {
        handle_event(&ctrl_key('e'), &mut app, visible);
    }
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 5);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 5);

    // Ctrl-Y (ScrollLineUp): scrolls content up by 1 line
    handle_event(&ctrl_key('y'), &mut app, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 4);

    // Move cursor to bottom of viewport (offset 4, visible 10 -> cursor can be up to 13)
    app.views
        .main_view
        .as_mut()
        .unwrap()
        .set_cursor(13, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 4);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 13);

    // ScrollLineUp pulls cursor up when it would exceed the new viewport
    handle_event(&ctrl_key('y'), &mut app, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 3);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 12);
}

#[test]
fn test_mouse_wheel_scrolling() {
    let mut app = app_with(50);
    let visible = 10;
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 0);

    // Mouse wheel scroll down: scrolls by 3 lines
    handle_event(&mouse_scroll_down(), &mut app, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 3);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 3);

    // Mouse wheel scroll down again
    handle_event(&mouse_scroll_down(), &mut app, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 6);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 6);

    // Mouse wheel scroll up: scrolls back by 3 lines
    handle_event(&mouse_scroll_up(), &mut app, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 3);

    // Mouse wheel scroll up to top
    handle_event(&mouse_scroll_up(), &mut app, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 0);
}

#[test]
fn test_page_down_and_page_up_scrolling() {
    let mut app = app_with(50);
    let visible = 10;
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 0);

    // PageDown (MovePageDown): moves down by visible - 2 = 8 lines
    handle_event(&key(KeyCode::PageDown), &mut app, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 8);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 0);

    // Second PageDown moves down to 16, rolling the view down (scroll_offset = 7)
    handle_event(&key(KeyCode::PageDown), &mut app, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 16);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 7);

    // PageUp (MovePageUp): moves back up to 8
    handle_event(&key(KeyCode::PageUp), &mut app, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 8);

    // Second PageUp moves back to 0 (scroll_offset = 0)
    handle_event(&key(KeyCode::PageUp), &mut app, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 0);

    // Action::ScrollPageDown scrolls viewport down by step (8)
    execute_action(&mut app, &Action::ScrollPageDown, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 8);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 8);

    // Action::ScrollPageUp scrolls viewport back up
    execute_action(&mut app, &Action::ScrollPageUp, visible);
    assert_eq!(app.views.main_view.as_ref().unwrap().scroll_offset(), 0);
}

/// Builds a diff with `lines` context lines so the diff pane has content to scroll.
fn tall_test_diff(lines: usize) -> CommitDiff {
    let mut diff = test_diff();
    diff.files[0].hunks[0].lines = (0..lines)
        .map(|i| HunkLine {
            kind: DiffLineKind::Context,
            content: format!("line {i}"),
            no_newline_at_eof: false,
        })
        .collect();
    diff
}

/// Builds a Main+Diff split app, focused on the Diff pane.
fn split_app(vertical: bool) -> AppState {
    let mut app = app_with(50);
    app.push_view(ViewKind::Main);
    app.views.diff_view = Some(DiffView::new(tall_test_diff(400)));
    app.push_view(ViewKind::Diff);
    app.options.vertical_split = vertical;
    app
}

fn main_offset(app: &AppState) -> usize {
    app.views.main_view.as_ref().unwrap().scroll_offset()
}

fn diff_offset(app: &AppState) -> usize {
    app.views.diff_view.as_ref().unwrap().scroll_offset()
}

#[test]
fn test_view_layout_vertical_split_geometry() {
    let app = split_app(true);

    let layout = app.view_layout(100, 24).expect("layout");
    let ViewLayout::Split {
        primary,
        secondary,
        vertical,
    } = layout
    else {
        panic!("expected split layout, got {layout:?}");
    };

    assert!(vertical);
    // One column is reserved for the separator between the panes.
    assert_eq!(
        primary,
        PaneLayout {
            kind: ViewKind::Main,
            x: 0,
            y: 0,
            width: 49,
            height: 24,
        }
    );
    assert_eq!(
        secondary,
        PaneLayout {
            kind: ViewKind::Diff,
            x: 50,
            y: 0,
            width: 50,
            height: 24,
        }
    );
    // Both panes span the full terminal height, so both show 22 content rows.
    assert_eq!(primary.visible_height(), 22);
    assert_eq!(secondary.visible_height(), 22);
    assert_eq!(
        primary.width + 1 + secondary.width,
        100,
        "panes plus separator must exactly fill the terminal width"
    );
}

#[test]
fn test_view_layout_horizontal_split_geometry() {
    let app = split_app(false);

    let layout = app.view_layout(80, 24).expect("layout");
    let ViewLayout::Split {
        primary,
        secondary,
        vertical,
    } = layout
    else {
        panic!("expected split layout, got {layout:?}");
    };
    assert!(!vertical);
    assert_eq!(primary.y, 0);
    assert_eq!(primary.height, 12);
    assert_eq!(secondary.y, 12);
    assert_eq!(secondary.height, 12);
    assert_eq!(primary.width, 80);
    assert_eq!(secondary.width, 80);

    // Odd heights give the extra row to the bottom pane.
    let layout = app.view_layout(80, 25).expect("layout");
    let ViewLayout::Split {
        primary, secondary, ..
    } = layout
    else {
        panic!("expected split layout");
    };
    assert_eq!(primary.height, 12);
    assert_eq!(secondary.y, 12);
    assert_eq!(secondary.height, 13);
    assert_eq!(primary.visible_height(), 10);
    assert_eq!(secondary.visible_height(), 11);
}

#[test]
fn test_view_layout_single_maximized_and_below_threshold() {
    let mut app = app_with(50);
    assert_eq!(
        app.view_layout(80, 24),
        Some(ViewLayout::Single(PaneLayout {
            kind: ViewKind::Main,
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        }))
    );

    app.push_view(ViewKind::Main);
    app.views.diff_view = Some(DiffView::new(test_diff()));
    app.push_view(ViewKind::Diff);

    // A maximized view never splits.
    app.views.maximized = true;
    assert!(matches!(
        app.view_layout(80, 24),
        Some(ViewLayout::Single(_))
    ));
    app.views.maximized = false;

    // Terminals below the split thresholds fall back to a single pane.
    assert!(matches!(
        app.view_layout(19, 24),
        Some(ViewLayout::Single(_))
    ));
    assert!(matches!(
        app.view_layout(80, 5),
        Some(ViewLayout::Single(_))
    ));
    app.options.vertical_split = true;
    assert!(matches!(
        app.view_layout(29, 24),
        Some(ViewLayout::Single(_))
    ));
    assert!(matches!(
        app.view_layout(80, 3),
        Some(ViewLayout::Single(_))
    ));

    // No active view means no layout at all.
    let empty = AppState::default();
    assert_eq!(empty.view_layout(80, 24), None);
}

#[test]
fn test_view_layout_pane_hit_testing() {
    let app = split_app(true);
    let layout = app.view_layout(100, 24).expect("layout");

    assert_eq!(
        layout.pane_at(0, 0).map(|p| p.kind),
        Some(ViewKind::Main),
        "top-left cell belongs to the left pane"
    );
    assert_eq!(layout.pane_at(48, 23).map(|p| p.kind), Some(ViewKind::Main));
    assert_eq!(
        layout.pane_at(49, 12),
        None,
        "separator column belongs to no pane"
    );
    assert_eq!(layout.pane_at(50, 0).map(|p| p.kind), Some(ViewKind::Diff));
    assert_eq!(layout.pane_at(99, 23).map(|p| p.kind), Some(ViewKind::Diff));
    assert_eq!(layout.pane_at(100, 0), None, "column past the right edge");
    assert_eq!(layout.pane_at(0, 24), None, "row past the bottom edge");

    assert_eq!(layout.pane_for(ViewKind::Main).map(|p| p.x), Some(0));
    assert_eq!(layout.pane_for(ViewKind::Diff).map(|p| p.x), Some(50));
    assert_eq!(layout.pane_for(ViewKind::Status), None);

    // Horizontal split hit testing splits on rows instead of columns.
    let app = split_app(false);
    let layout = app.view_layout(80, 24).expect("layout");
    assert_eq!(layout.pane_at(40, 11).map(|p| p.kind), Some(ViewKind::Main));
    assert_eq!(layout.pane_at(40, 12).map(|p| p.kind), Some(ViewKind::Diff));
}

#[test]
fn test_mouse_wheel_scrolls_pane_under_pointer_in_vertical_split() {
    let mut app = split_app(true);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));

    // Pointer over the unfocused left pane: the left pane must roll down,
    // the focused right pane must stay put, and focus must not move.
    handle_event_with_dimensions(&mouse_scroll_down_at(10, 12), &mut app, 100, 24);
    assert_eq!(main_offset(&app), 3, "pane under the pointer must scroll");
    assert_eq!(diff_offset(&app), 0, "focused pane must not scroll");
    assert_eq!(
        app.active_view(),
        Some(ViewKind::Diff),
        "scrolling must not steal focus"
    );

    handle_event_with_dimensions(&mouse_scroll_down_at(10, 12), &mut app, 100, 24);
    assert_eq!(main_offset(&app), 6);
    assert_eq!(diff_offset(&app), 0);

    handle_event_with_dimensions(&mouse_scroll_up_at(10, 12), &mut app, 100, 24);
    assert_eq!(main_offset(&app), 3);

    // Pointer over the focused right pane: only the diff scrolls.
    handle_event_with_dimensions(&mouse_scroll_down_at(70, 12), &mut app, 100, 24);
    assert_eq!(diff_offset(&app), 3);
    assert_eq!(main_offset(&app), 3);
}

#[test]
fn test_mouse_wheel_scrolls_pane_under_pointer_in_horizontal_split() {
    let mut app = split_app(false);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));

    // Row 3 is inside the top (Main) pane of an 80x24 terminal.
    handle_event_with_dimensions(&mouse_scroll_down_at(10, 3), &mut app, 80, 24);
    assert_eq!(main_offset(&app), 3);
    assert_eq!(diff_offset(&app), 0);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));

    // Row 20 is inside the bottom (Diff) pane.
    handle_event_with_dimensions(&mouse_scroll_down_at(10, 20), &mut app, 80, 24);
    assert_eq!(main_offset(&app), 3);
    assert_eq!(diff_offset(&app), 3);
}

#[test]
fn test_mouse_wheel_clamps_using_the_pointed_pane_height() {
    // At 80x25 the top pane shows 10 rows and the bottom pane 11, so the
    // clamp proves the pointed-at pane's own height is used.
    let mut app = split_app(false);
    for _ in 0..40 {
        handle_event_with_dimensions(&mouse_scroll_down_at(10, 3), &mut app, 80, 25);
    }
    assert_eq!(
        main_offset(&app),
        40,
        "main has 50 commits and a 10-row pane, so it clamps at 40"
    );
}

#[test]
fn test_mouse_wheel_over_separator_falls_back_to_focused_pane() {
    let mut app = split_app(true);
    // Column 49 is the separator in a 100-column vertical split.
    handle_event_with_dimensions(&mouse_scroll_down_at(49, 12), &mut app, 100, 24);
    assert_eq!(diff_offset(&app), 3, "separator routes to the focused pane");
    assert_eq!(main_offset(&app), 0);
}

#[test]
fn test_mouse_wheel_without_dimensions_uses_focused_pane() {
    // `handle_event` has no geometry, so it must keep the legacy behaviour
    // of scrolling whichever pane holds focus.
    let mut app = split_app(true);
    handle_event(&mouse_scroll_down_at(10, 12), &mut app, 22);
    assert_eq!(diff_offset(&app), 3);
    assert_eq!(main_offset(&app), 0);
}

fn status_item(path: &str, section: StatusSection, code: char) -> tigrs_git::StatusItem {
    tigrs_git::StatusItem::new(code, section, path, None)
}

fn dirty_report() -> tigrs_git::StatusReport {
    tigrs_git::StatusReport {
        staged: vec![status_item("s.txt", StatusSection::Staged, 'M')],
        unstaged: vec![status_item("u.txt", StatusSection::Unstaged, 'M')],
        untracked: vec![status_item("n.txt", StatusSection::Untracked, '?')],
        unmerged: Vec::new(),
        ..Default::default()
    }
}

#[test]
fn test_changes_rows_follow_tig_ordering() {
    let options = ViewOptions::default();
    let rows = changes_rows_from_status(&dirty_report(), &options);

    assert_eq!(
        rows,
        vec![
            ChangesRow::new(ChangesKind::Untracked, 1),
            ChangesRow::new(ChangesKind::Unstaged, 1),
            ChangesRow::new(ChangesKind::Staged, 1),
        ]
    );
}

#[test]
fn test_changes_rows_omit_empty_sections() {
    let mut report = dirty_report();
    report.unstaged.clear();
    report.untracked.clear();

    let rows = changes_rows_from_status(&report, &ViewOptions::default());
    assert_eq!(rows, vec![ChangesRow::new(ChangesKind::Staged, 1)]);
}

#[test]
fn test_unmerged_paths_fold_into_unstaged() {
    // Upstream tig has no separate conflicts row; unmerged paths are counted
    // as part of the unstaged section.
    let mut report = dirty_report();
    report
        .unmerged
        .push(status_item("c.txt", StatusSection::Unmerged, 'U'));

    let rows = changes_rows_from_status(&report, &ViewOptions::default());
    assert!(rows.contains(&ChangesRow::new(ChangesKind::Unstaged, 2)));
}

#[test]
fn test_show_changes_and_show_untracked_gate_rows() {
    let report = dirty_report();

    let mut options = ViewOptions {
        show_untracked: false,
        ..Default::default()
    };
    let rows = changes_rows_from_status(&report, &options);
    assert_eq!(
        rows,
        vec![
            ChangesRow::new(ChangesKind::Unstaged, 1),
            ChangesRow::new(ChangesKind::Staged, 1),
        ]
    );

    // `show-changes` off hides everything, including untracked.
    options.show_untracked = true;
    options.show_changes = false;
    assert!(changes_rows_from_status(&report, &options).is_empty());
}

#[test]
fn test_toggling_show_changes_updates_main_view_rows() {
    let mut app = AppState {
        views: ViewManager {
            main_view: Some(MainView::new("main".to_string())),
            ..Default::default()
        },
        changes_report: Some(dirty_report()),
        ..Default::default()
    };

    assert!(app.apply_changes_rows());
    assert_eq!(app.views.main_view.as_ref().unwrap().changes_len(), 3);

    // Toggling must take effect immediately, without another worktree scan.
    app.toggle_option("show-untracked");
    assert_eq!(app.views.main_view.as_ref().unwrap().changes_len(), 2);

    app.toggle_option("show-changes");
    assert_eq!(app.views.main_view.as_ref().unwrap().changes_len(), 0);

    app.toggle_option("show-changes");
    app.toggle_option("show-untracked");
    assert_eq!(app.views.main_view.as_ref().unwrap().changes_len(), 3);
}

#[test]
fn test_dispatch_nav_motion_comprehensive() {
    let mut app = AppState {
        views: ViewManager {
            main_view: Some(view_with(50)),
            view_stack: vec![ViewKind::Main],
            ..Default::default()
        },
        ..Default::default()
    };

    let h = 10;
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);

    // Down
    dispatch_nav_motion(&mut app, NavMotion::Down(3), h);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 3);

    // Up
    dispatch_nav_motion(&mut app, NavMotion::Up(1), h);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 2);

    let page_step = h.saturating_sub(2).max(1);

    // PageDown
    dispatch_nav_motion(&mut app, NavMotion::PageDown, h);
    assert_eq!(
        app.views.main_view.as_ref().unwrap().cursor_index(),
        2 + page_step
    );

    // PageUp
    dispatch_nav_motion(&mut app, NavMotion::PageUp, h);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 2);

    // HalfPageDown
    dispatch_nav_motion(&mut app, NavMotion::HalfPageDown, h);
    assert_eq!(
        app.views.main_view.as_ref().unwrap().cursor_index(),
        2 + h / 2
    );

    // HalfPageUp
    dispatch_nav_motion(&mut app, NavMotion::HalfPageUp, h);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 2);

    // LastLine
    dispatch_nav_motion(&mut app, NavMotion::LastLine, h);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 49);

    // FirstLine
    dispatch_nav_motion(&mut app, NavMotion::FirstLine, h);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 0);

    // SetCursor
    dispatch_nav_motion(&mut app, NavMotion::SetCursor(25), h);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 25);

    // ScrollDown & ScrollUp
    dispatch_nav_motion(&mut app, NavMotion::ScrollDown(5), h);
    assert!(app.views.main_view.as_ref().unwrap().scroll_offset() > 0);
    dispatch_nav_motion(&mut app, NavMotion::ScrollUp(5), h);
}

#[test]
fn test_render_active_synchronized_output() {
    let mut app = AppState {
        views: ViewManager {
            main_view: Some(MainView::new("main".to_string())),
            ..Default::default()
        },
        ..Default::default()
    };

    // Without synchronized output
    let mut buf_plain = Vec::new();
    render_active(&app, &mut buf_plain, 80, 24).unwrap();
    assert!(!buf_plain.starts_with(b"\x1b[?2026h"));
    assert!(!buf_plain.ends_with(b"\x1b[?2026l"));

    // With synchronized output enabled
    app.caps.supports_synchronized_output = true;
    app.invalidate_screen();
    let mut buf_sync = Vec::new();
    render_active(&app, &mut buf_sync, 80, 24).unwrap();
    assert!(buf_sync.starts_with(b"\x1b[?2026h"));
    assert!(buf_sync.ends_with(b"\x1b[?2026l"));

    // Unchanged frame with zero damage should not emit empty DEC 2026 wrappers
    let mut buf_unchanged = Vec::new();
    render_active(&app, &mut buf_unchanged, 80, 24).unwrap();
    assert!(buf_unchanged.is_empty());
}

#[test]
fn test_split_view_async_debounced_diff_sync() {
    let tmp = tempfile::TempDir::new().unwrap();
    let p = tmp.path();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&["init"]);
    run(&["config", "user.name", "Tester"]);
    run(&["config", "user.email", "test@test.com"]);

    std::fs::write(p.join("c1.txt"), "commit 1\n").unwrap();
    run(&["add", "c1.txt"]);
    run(&["commit", "-m", "first commit"]);

    std::fs::write(p.join("c2.txt"), "commit 2\n").unwrap();
    run(&["add", "c2.txt"]);
    run(&["commit", "-m", "second commit"]);

    let engine = GitEngine::open(Some(p)).unwrap();
    let head_id = engine.head_commit_id().unwrap();

    let mut app = AppState {
        views: ViewManager {
            main_view: Some(MainView::new("main".to_string())),
            ..Default::default()
        },
        engine: Some(engine.clone()),
        ..Default::default()
    };

    let (diff_tx, diff_rx) = bounded::<DiffWorkerResponse>(16);
    app.diff_task.tx = Some(diff_tx);

    if let (Some(main), Some(eng)) = (&mut app.views.main_view, &app.engine) {
        let (_src, token) = CancellationToken::new();
        if let Ok(iter) = eng.stream_commits(Some(head_id), Some(10), token) {
            for batch in iter.flatten() {
                main.append_commits(batch);
            }
        }
        main.set_finished();
    }

    let commits = app.views.main_view.as_ref().unwrap().commits().to_vec();
    assert_eq!(commits.len(), 2);
    let c2_id = commits[0].id;
    let c1_id = commits[1].id;

    // Open diff view in split mode for c2 (head)
    let initial_diff = engine.compute_commit_diff(c2_id).unwrap();
    app.views.diff_view = Some(DiffView::new(initial_diff));
    app.push_view(ViewKind::Main);
    app.push_view(ViewKind::Diff);

    // Move cursor down to c1
    app.views.main_view.as_mut().unwrap().move_down(1, 24);
    sync_split_diff_if_open(&mut app);

    // Diff for c1 is not yet in LRU cache, so pending_diff_target is scheduled
    assert!(app.pending_diff_target.is_some());
    let (pending_oid, _deadline, req_id) = app.pending_diff_target.clone().unwrap();
    assert_eq!(pending_oid, c1_id);
    assert_eq!(req_id, app.diff_task.active_request_id);

    // Simulate worker thread completing the diff
    let diff_result = engine.compute_commit_diff(c1_id).map(std::sync::Arc::new);
    app.diff_task
        .tx
        .as_ref()
        .unwrap()
        .send(DiffWorkerResponse {
            target: DiffRequestTarget::Commit(c1_id),
            generation: engine.generation(),
            request_id: req_id,
            is_prefetch: false,
            render_key: Some(crate::diff::DiffRenderKey::from_options(
                &app.options,
                app.caps.color_profile,
            )),
            result: diff_result,
            precomputed_view: None,
        })
        .unwrap();

    // Process response
    let resp = diff_rx.recv().unwrap();
    assert_eq!(resp.request_id, app.diff_task.active_request_id);
    app.views.diff_view = Some(DiffView::new(resp.result.unwrap()));
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(c1_id)
    );

    // Now move cursor back up to c2: LRU cache hits immediately!
    app.views.main_view.as_mut().unwrap().move_up(1, 24);
    sync_split_diff_if_open(&mut app);
    // Instant update from cache, no pending target!
    assert_eq!(
        app.views.diff_view.as_ref().map(DiffView::commit_id),
        Some(c2_id)
    );
    assert!(app.pending_diff_target.is_none());
}

#[test]
fn test_changes_diff_cache_and_cumulative_highlight_budget() {
    let mut diff = tigrs_git::CommitDiff {
        commit_id: tigrs_git::ObjectId::from_bytes_or_panic(&[0; 20]),
        parent_ids: Vec::new(),
        author_name: "Tester".into(),
        author_email: "test@example.com".into(),
        author_date: "0".to_string(),
        committer_name: "Tester".into(),
        committer_email: "test@example.com".into(),
        committer_date: "0".to_string(),
        title: "Unstaged changes".into(),
        body: None,
        stats: tigrs_git::DiffSummaryStats::default(),
        files: Vec::new(),
    };
    for f_idx in 0..10 {
        let mut lines = Vec::new();
        for l_idx in 0..300 {
            lines.push(tigrs_git::HunkLine {
                kind: tigrs_git::DiffLineKind::Add,
                content: format!("let var_{f_idx}_{l_idx} = {l_idx};"),
                no_newline_at_eof: false,
            });
        }
        diff.files.push(tigrs_git::FileDiff {
            path: format!("src/file_{f_idx}.rs"),
            status: tigrs_git::FileChangeStatus::Modified,
            additions: 300,
            deletions: 0,
            is_binary: false,
            old_id: None,
            new_id: None,
            old_mode: None,
            new_mode: None,
            hunks: vec![tigrs_git::DiffHunk {
                old_start: 1,
                old_len: 0,
                new_start: 1,
                new_len: 300,
                func_context: None,
                lines,
            }],
        });
    }

    let lean_opts = ViewOptions {
        syntax_highlighting: true,
        memory_profile: tigrs_core::MemoryProfile::Lean,
        ..Default::default()
    };
    let view = DiffView::new_with_options(diff.clone(), &lean_opts, None);
    let highlighted_rows = view
        .document()
        .rows
        .iter()
        .filter(|r| r.left.as_ref().is_some_and(|c| !c.syntax_spans.is_empty()))
        .count();
    assert_eq!(
        highlighted_rows,
        crate::highlight::MAX_DIFF_HIGHLIGHT_TOTAL_LINES,
        "Lean profile syntax highlight budget across files must be capped at MAX_DIFF_HIGHLIGHT_TOTAL_LINES"
    );

    let greedy_opts = ViewOptions {
        syntax_highlighting: true,
        memory_profile: tigrs_core::MemoryProfile::Greedy,
        ..Default::default()
    };
    let greedy_view = DiffView::new_with_options(diff, &greedy_opts, None);
    let greedy_highlighted = greedy_view
        .document()
        .rows
        .iter()
        .filter(|r| r.left.as_ref().is_some_and(|c| !c.syntax_spans.is_empty()))
        .count();
    assert_eq!(
        greedy_highlighted,
        crate::highlight::MAX_DIFF_HIGHLIGHT_TOTAL_LINES,
        "Greedy profile must also cap cumulative syntax highlighting at MAX_DIFF_HIGHLIGHT_TOTAL_LINES so large uncommitted diffs do not stall"
    );
}

/// After a child process (or `SIGTSTP`/`SIGCONT`) re-enters the alternate
/// screen the terminal is blank, so the very next frame has to repaint
/// everything even though the view itself did not change.
#[test]
fn test_invalidate_screen_forces_full_repaint() {
    let mut app = AppState {
        views: ViewManager {
            pager_view: Some(PagerView::new(
                "pager".to_string(),
                vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
            )),
            ..Default::default()
        },
        ..AppState::default()
    };
    app.ensure_view_stack();

    let mut first = Vec::new();
    render_active(&app, &mut first, 40, 8).unwrap();
    assert!(!first.is_empty(), "first frame must paint");

    // Nothing changed and the screen still holds the frame: damage tracking
    // legitimately collapses this to (nearly) nothing.
    let mut second = Vec::new();
    render_active(&app, &mut second, 40, 8).unwrap();
    assert!(
        !String::from_utf8_lossy(&second).contains("alpha"),
        "unchanged content should not be repainted"
    );

    // The screen was wiped behind our back: every row must come back.
    app.invalidate_screen();
    let mut third = Vec::new();
    render_active(&app, &mut third, 40, 8).unwrap();
    let text = String::from_utf8_lossy(&third);
    assert!(
        text.contains("alpha") && text.contains("gamma"),
        "invalidated screen must be fully repainted, got: {text:?}"
    );
}

#[test]
fn test_maximize_and_layout_actions_invalidate_screen() {
    let mut app = AppState {
        views: ViewManager {
            pager_view: Some(PagerView::new(
                "pager".to_string(),
                vec!["alpha".to_string(), "beta".to_string()],
            )),
            help_view: Some(HelpView::new()),
            ..Default::default()
        },
        ..AppState::default()
    };
    app.views.view_stack = vec![ViewKind::Pager, ViewKind::Help];
    let mut out = Vec::new();
    render_active(&app, &mut out, 40, 8).unwrap();
    assert!(app.renderer.borrow().has_valid_back_grid());

    // Maximize action must invalidate screen
    execute_action(&mut app, &Action::Maximize, 8);
    assert!(!app.renderer.borrow().has_valid_back_grid());

    // Re-render and test ToggleVerticalSplit
    render_active(&app, &mut out, 40, 8).unwrap();
    assert!(app.renderer.borrow().has_valid_back_grid());
    execute_action(
        &mut app,
        &Action::ToggleOption(crate::options::OptionId::VerticalSplit),
        8,
    );
    assert!(!app.renderer.borrow().has_valid_back_grid());

    // Re-render and test ViewNext
    render_active(&app, &mut out, 40, 8).unwrap();
    assert!(app.renderer.borrow().has_valid_back_grid());
    execute_action(&mut app, &Action::ViewNext, 8);
    assert!(!app.renderer.borrow().has_valid_back_grid());
}

#[test]
fn test_speculative_diff_prefetch_lifecycle() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo_path = tmp.path();
    std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["init", "-b", "main"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    std::fs::write(repo_path.join("file.txt"), "hello\n").unwrap();
    std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "."])
        .current_dir(repo_path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "first",
        ])
        .current_dir(repo_path)
        .output()
        .unwrap();

    let engine = GitEngine::open(Some(repo_path)).unwrap();
    let head_id = engine.head_commit_id().unwrap();

    let mut app = AppState {
        views: ViewManager {
            main_view: Some(MainView::new("main".to_string())),
            ..Default::default()
        },
        engine: Some(engine.clone()),
        ..Default::default()
    };

    // Populate commit in main view
    if let (Some(main), Some(eng)) = (&mut app.views.main_view, &app.engine) {
        let (_src, token) = CancellationToken::new();
        if let Ok(iter) = eng.stream_commits(Some(head_id), Some(10), token) {
            for batch in iter.flatten() {
                main.append_commits(batch);
            }
        }
        main.set_finished();
    }

    // In single-pane mode, schedule speculative prefetch on idle
    schedule_speculative_diff_prefetch_if_idle(&mut app);
    assert!(app.pending_speculative_prefetch.is_some());
    let (oid, _deadline) = app.pending_speculative_prefetch.unwrap();
    assert_eq!(oid, head_id);

    // Pre-compute the diff into engine's LRU cache
    let _ = engine.compute_commit_diff_cached(head_id).unwrap();

    // Now schedule again: since it's already in the cache, no prefetch should be scheduled!
    schedule_speculative_diff_prefetch_if_idle(&mut app);
    assert!(app.pending_speculative_prefetch.is_none());

    // Opening the diff view clears any pending prefetch
    app.pending_speculative_prefetch = Some((head_id, Instant::now()));
    open_diff_for_main_selection(&mut app);
    assert!(app.pending_speculative_prefetch.is_none());
    assert!(app.views.diff_view.is_some());
}

#[test]
fn test_render_active_debug_frame_stats() {
    let app = AppState {
        views: ViewManager {
            main_view: Some(MainView::new("main".to_string())),
            ..Default::default()
        },
        debug_frame_stats: true,
        ..Default::default()
    };
    let mut buf = Vec::new();
    render_active(&app, &mut buf, 80, 24).unwrap();
    assert!(!buf.is_empty());
}

#[test]
fn test_app_prompt_and_command_execution_comprehensive() {
    let mut app = app_with(10);

    // Helper to submit a command string via prompt
    let run_cmd = |cmd: &str, state: &mut AppState| {
        handle_event(&key(KeyCode::Char(':')), state, 24);
        for c in cmd.chars() {
            handle_event(&key(KeyCode::Char(c)), state, 24);
        }
        handle_event(&key(KeyCode::Enter), state, 24)
    };

    // 1. Empty command
    assert_eq!(run_cmd("", &mut app), Flow::Continue);

    // 2. Echo command
    assert_eq!(run_cmd("echo test_echo_output", &mut app), Flow::Continue);
    assert_eq!(app.status_message.as_deref(), Some("test_echo_output"));

    // 3. Unknown command
    assert_eq!(
        run_cmd("unknown_command_with space", &mut app),
        Flow::Continue
    );
    assert!(
        app.status_message
            .as_ref()
            .unwrap()
            .contains("Unknown command")
    );

    // 4. Line number jump: :5 jumps to 1-based line 5 (0-indexed row 4)
    assert_eq!(run_cmd("5", &mut app), Flow::Continue);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor_index(), 4);

    // 5. Toggle options via prompt
    assert_eq!(
        run_cmd("toggle status-show-untracked-dirs", &mut app),
        Flow::Continue
    );
    assert!(app.status_message.as_ref().unwrap().contains("untracked"));

    assert_eq!(run_cmd("toggle file-filter", &mut app), Flow::Continue);
    assert!(app.status_message.as_ref().unwrap().contains("file-filter"));

    // 6. Default is Update Mode (`!app.is_read_only()`). Enabling Read-Only mode (`:read-only`)
    //    blocks Action::Edit with READ_ONLY_WARNING_MSG; `:update-mode` unlocks it again.
    assert!(!app.is_read_only());
    assert_eq!(run_cmd("read-only", &mut app), Flow::Continue);
    assert!(app.is_read_only());
    let flow = execute_action(&mut app, &Action::Edit, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(
        app.status_message.as_deref(),
        Some(crate::app::READ_ONLY_WARNING_MSG)
    );
    assert_eq!(run_cmd("update-mode", &mut app), Flow::Continue);
    assert!(!app.is_read_only());
    let flow = execute_action(&mut app, &Action::Edit, 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.status_message.as_deref(), Some("Nothing to edit"));

    // 7. Background command execution
    let bg_run = Arc::new(RunCommand {
        flags: RunFlags::SILENT,
        command: "echo background_done".to_string(),
    });
    let flow = execute_action(&mut app, &Action::Run(bg_run), 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Command started in background")
    );

    // 8. Interactive macro with different prompt label formats:
    // - No label: %(prompt)
    let macro_no_label = Arc::new(RunCommand {
        flags: RunFlags::ECHO,
        command: "echo %(prompt)".to_string(),
    });
    execute_action(&mut app, &Action::Run(macro_no_label), 24);
    assert!(app.prompt.is_some());
    assert_eq!(app.prompt.as_ref().unwrap().prompt_prefix(), ": ");
    handle_event(&key(KeyCode::Esc), &mut app, 24);

    // - Label ending with colon: %(prompt Input:)
    let macro_colon = Arc::new(RunCommand {
        flags: RunFlags::ECHO,
        command: "echo %(prompt Input:)".to_string(),
    });
    execute_action(&mut app, &Action::Run(macro_colon), 24);
    assert!(app.prompt.is_some());
    assert_eq!(app.prompt.as_ref().unwrap().prompt_prefix(), "Input: ");
    handle_event(&key(KeyCode::Esc), &mut app, 24);
}

#[test]
fn test_app_actions_and_commands_exhaustive() {
    use crate::options::OptionId;

    let mut app = app_with(10);
    app.options.read_only = false;
    app.push_view(ViewKind::Main);
    let h = 24;

    // 1. Toggle actions
    let toggle_actions = [
        Action::ToggleOption(OptionId::FileSize),
        Action::ToggleOption(OptionId::IgnoreSpace),
        Action::ToggleOption(OptionId::WordDiff),
        Action::ToggleOption(OptionId::CommitOrder),
        Action::ToggleOption(OptionId::CommitTitleRefs),
        Action::ToggleOption(OptionId::ShowChanges),
        Action::ToggleOption(OptionId::Id),
        Action::ToggleOption(OptionId::FileFilter),
        Action::ToggleOption(OptionId::RevFilter),
        Action::ToggleOption(OptionId::CommitTitleOverflow),
        Action::ToggleOption(OptionId::StatusShowUntrackedDirs),
        Action::ToggleOption(OptionId::VerticalSplit),
        Action::ToggleDiffContext(1),
        Action::ToggleDiffContext(-1),
        Action::ToggleOption(OptionId::DiffLayout),
        Action::ToggleOption(OptionId::DiffPresentation),
        Action::ToggleOption(OptionId::Date),
        Action::ToggleOption(OptionId::Author),
        Action::ToggleOption(OptionId::Committer),
        Action::ToggleOption(OptionId::LineGraphics),
        Action::ToggleOption(OptionId::CommitTitleGraph),
        Action::ToggleOption(OptionId::FileName),
        Action::ToggleOption(OptionId::LineNumber),
    ];
    for action in toggle_actions {
        assert_eq!(execute_action(&mut app, &action, h), Flow::Continue);
        assert!(app.status_message.is_some());
    }

    // 2. Navigation actions
    let nav_actions = [
        Action::MoveDown,
        Action::MoveUp,
        Action::MovePageDown,
        Action::MovePageUp,
        Action::MoveHalfPageDown,
        Action::MoveHalfPageUp,
        Action::MoveFirstLine,
        Action::MoveLastLine,
        Action::ScrollLineDown,
        Action::ScrollLineUp,
        Action::ScrollPageDown,
        Action::ScrollPageUp,
        Action::NextHunk,
        Action::PrevHunk,
        Action::NextFile,
        Action::PrevFile,
        Action::FindNext,
        Action::FindPrev,
        Action::Options,
        Action::Prompt,
        Action::Search,
        Action::SearchBack,
        Action::Maximize,
        Action::ViewNext,
        Action::Parent,
        Action::Back,
    ];
    for action in nav_actions {
        assert_eq!(execute_action(&mut app, &action, h), Flow::Continue);
    }

    // 3. Quit
    assert_eq!(execute_action(&mut app, &Action::Quit, h), Flow::Quit);

    // 4. View actions
    let view_actions = [
        Action::OpenView(ViewKind::Help),
        Action::OpenView(ViewKind::Refs),
        Action::OpenView(ViewKind::Stash),
        Action::OpenView(ViewKind::Reflog),
        Action::OpenView(ViewKind::Grep),
        Action::OpenView(ViewKind::Pager),
        Action::ViewClose,
    ];
    for action in view_actions {
        assert_eq!(execute_action(&mut app, &action, h), Flow::Continue);
    }

    // 5. execute_parsed_command
    assert_eq!(
        execute_parsed_command(
            &mut app,
            ParsedCommand::Search {
                query: "c1".to_string(),
                backward: false,
            },
            h
        ),
        Flow::Continue
    );
    assert_eq!(
        execute_parsed_command(
            &mut app,
            ParsedCommand::Search {
                query: "c1".to_string(),
                backward: true,
            },
            h
        ),
        Flow::Continue
    );
    assert_eq!(
        execute_parsed_command(
            &mut app,
            ParsedCommand::Goto("nonexistent_commit".to_string()),
            h
        ),
        Flow::Continue
    );
    assert_eq!(
        execute_parsed_command(&mut app, ParsedCommand::Grep("test".to_string()), h),
        Flow::Continue
    );
    assert_eq!(
        execute_parsed_command(
            &mut app,
            ParsedCommand::Shell {
                command: "echo test".to_string(),
                foreground: true,
            },
            h
        ),
        Flow::Continue
    );
    assert!(app.pending_handover.is_some());
    app.pending_handover = None;

    assert_eq!(
        execute_parsed_command(
            &mut app,
            ParsedCommand::Shell {
                command: "echo test_bg".to_string(),
                foreground: false,
            },
            h
        ),
        Flow::Continue
    );
    assert_eq!(app.status_message.as_deref(), Some("test_bg"));
    assert_eq!(
        execute_parsed_command(
            &mut app,
            ParsedCommand::Builtin(Action::ToggleOption(OptionId::LineNumber)),
            h
        ),
        Flow::Continue
    );

    // 6. execute_run_command
    let internal_cmd = RunCommand {
        flags: RunFlags::INTERNAL,
        command: "echo internal_test".to_string(),
    };
    assert_eq!(
        execute_run_command(&mut app, &internal_cmd.command, &internal_cmd),
        Flow::Continue
    );

    let unsafe_cmd = RunCommand {
        flags: RunFlags::default(),
        command: "echo \x00bad".to_string(),
    };
    assert_eq!(
        execute_run_command(&mut app, &unsafe_cmd.command, &unsafe_cmd),
        Flow::Continue
    );
    assert!(
        app.status_message
            .as_ref()
            .unwrap()
            .contains("Invalid command")
    );

    let echo_cmd = RunCommand {
        flags: RunFlags::ECHO,
        command: "echo echo_output".to_string(),
    };
    assert_eq!(
        execute_run_command(&mut app, &echo_cmd.command, &echo_cmd),
        Flow::Continue
    );
    assert_eq!(app.status_message.as_deref(), Some("echo_output"));

    let fail_cmd = RunCommand {
        flags: RunFlags::ECHO,
        command: "false".to_string(),
    };
    assert_eq!(
        execute_run_command(&mut app, &fail_cmd.command, &fail_cmd),
        Flow::Continue
    );
    assert!(app.status_message.as_ref().unwrap().contains("failed"));
}

#[test]
fn test_view_diff_full_window_and_enter_split_parity() {
    let (_tmp, mut app, _ids) = diff_over_log_app(2);
    // diff_over_log_app initializes with [Main, Diff]. Reset to just Main view.
    app.views.view_stack = vec![ViewKind::Main];
    app.views.maximized = false;
    let h = 24;
    let w = 80;

    // 1. Press Enter in Main view: opens Diff in SPLIT mode
    let flow = execute_action(&mut app, &Action::Enter, h);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(!app.views.maximized);
    match app.view_layout(w, h as u16).expect("view layout") {
        ViewLayout::Split {
            primary,
            secondary,
            vertical,
        } => {
            assert_eq!(primary.kind, ViewKind::Main);
            assert_eq!(secondary.kind, ViewKind::Diff);
            assert!(!vertical);
            assert_eq!(primary.height, 12);
            assert_eq!(secondary.height, 12);
        }
        ViewLayout::Single(_) => panic!("expected Split layout on Enter"),
    }

    // 2. Press 'd' (Action::OpenView(ViewKind::Diff)) while in Split Diff view: maximizes to full-window
    let flow = execute_action(&mut app, &Action::OpenView(ViewKind::Diff), h);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.maximized);
    match app.view_layout(w, h as u16).expect("view layout") {
        ViewLayout::Single(pane) => {
            assert_eq!(pane.kind, ViewKind::Diff);
            assert_eq!(pane.width, w);
            assert_eq!(pane.height, h as u16);
        }
        ViewLayout::Split { .. } => panic!("expected Single layout on ViewDiff"),
    }

    // 3. Redundant 'd' (Action::OpenView(ViewKind::Diff)) when already in full-window Diff view: silent no-op
    app.status_message = None;
    let flow = execute_action(&mut app, &Action::OpenView(ViewKind::Diff), h);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.maximized);
    assert!(app.status_message.is_none());

    // 4. Press 'm' (Action::OpenView(ViewKind::Main)): switches to Main view and maximizes it
    let flow = execute_action(&mut app, &Action::OpenView(ViewKind::Main), h);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Main));
    assert!(app.views.maximized);
    match app.view_layout(w, h as u16).expect("view layout") {
        ViewLayout::Single(pane) => {
            assert_eq!(pane.kind, ViewKind::Main);
            assert_eq!(pane.width, w);
            assert_eq!(pane.height, h as u16);
        }
        ViewLayout::Split { .. } => panic!("expected Single layout on ViewMain"),
    }

    // 5. Press 'd' (Action::OpenView(ViewKind::Diff)) directly from Main view: opens Diff in full-window
    let flow = execute_action(&mut app, &Action::OpenView(ViewKind::Diff), h);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.maximized);
    match app.view_layout(w, h as u16).expect("view layout") {
        ViewLayout::Single(pane) => {
            assert_eq!(pane.kind, ViewKind::Diff);
            assert_eq!(pane.width, w);
            assert_eq!(pane.height, h as u16);
        }
        ViewLayout::Split { .. } => panic!("expected Single layout on ViewDiff from Main"),
    }

    // 6. Pop view (close Diff view): returns to Main view and resets maximized
    let popped = app.pop_active_view();
    assert_eq!(popped, Some(ViewKind::Diff));
    assert_eq!(app.active_view(), Some(ViewKind::Main));
    assert_eq!(app.views.view_stack.len(), 1);
    assert!(!app.views.maximized);
    match app.view_layout(w, h as u16).expect("view layout") {
        ViewLayout::Single(pane) => {
            assert_eq!(pane.kind, ViewKind::Main);
            assert_eq!(pane.width, w);
            assert_eq!(pane.height, h as u16);
        }
        ViewLayout::Split { .. } => panic!("expected Single layout for single view on stack"),
    }
}

#[test]
fn test_all_14_canonical_views_parity() {
    let (_tmp, mut app, _ids) = diff_over_log_app(3);
    let h = 24;

    // Reset stack to Main view only
    app.views.view_stack = vec![ViewKind::Main];
    app.views.maximized = false;

    // 1. 'l' (Action::OpenView(ViewKind::Log)) & 'm' (Action::OpenView(ViewKind::Main))
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Log), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Log));
    assert!(app.views.maximized);

    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Main), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    // Test lazy initialization of MainView if None
    app.views.main_view = None;
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Main), h),
        Flow::Continue
    );
    assert!(app.views.main_view.is_some());
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    // 2. 'd' (Action::OpenView(ViewKind::Diff))
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Diff), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    assert!(app.views.maximized);

    // 3. 's' / 'S' (Action::OpenView(ViewKind::Status))
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Status), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Status));
    // Toggle back
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Status), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Diff));

    // 4. 'c' (Action::ViewStage)
    // From normal commit diff (not status diff), 'c' reports no stage content
    app.status_message = None;
    assert_eq!(
        execute_action(&mut app, &Action::ViewStage, h),
        Flow::Continue
    );
    assert!(
        app.status_message
            .as_ref()
            .unwrap()
            .contains("No stage content")
    );

    // Switch to Status view and test 'c' opens stage diff if item exists
    execute_action(&mut app, &Action::OpenView(ViewKind::Status), h);
    assert_eq!(app.active_view(), Some(ViewKind::Status));
    let status_item =
        tigrs_git::StatusItem::new('M', tigrs_git::StatusSection::Unstaged, "file.txt", None);
    let dummy_diff = app
        .engine
        .as_ref()
        .unwrap()
        .compute_status_item_diff(&status_item)
        .unwrap();
    app.views.diff_view = Some(app.create_status_diff_view(dummy_diff, status_item));
    // Pop status so we are in Diff view
    app.pop_active_view();
    assert_eq!(app.active_view(), Some(ViewKind::Diff));
    // Now 'c' in Diff view with status diff maximizes it
    app.views.maximized = false;
    assert_eq!(
        execute_action(&mut app, &Action::ViewStage, h),
        Flow::Continue
    );
    assert!(app.views.maximized);

    // 5. 't' (Action::OpenView(ViewKind::Tree))
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Tree), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Tree));
    assert!(app.views.tree_view.is_some());

    // 6. 'f' (Action::OpenView(ViewKind::Blob))
    // From Tree view with file selected
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Blob), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Blob));
    assert!(app.views.blob_view.is_some());

    // 7. 'b' (Action::OpenView(ViewKind::Blame))
    // From Blob view
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Blame), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Blame));
    assert!(app.views.blame_view.is_some());

    // Test ViewTree from Blame view
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Tree), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Tree));

    // 8. 'r' (Action::OpenView(ViewKind::Refs))
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Refs), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Refs));
    assert!(app.views.refs_view.is_some());
    // Toggle back
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Refs), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Tree));

    // 9. 'y' (Action::OpenView(ViewKind::Stash))
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Stash), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Stash));
    assert!(app.views.stash_view.is_some());
    // Toggle back
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Stash), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Tree));

    // 10. 'L' (Action::OpenView(ViewKind::Reflog))
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Reflog), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Reflog));
    assert!(app.views.reflog_view.is_some());
    // Toggle back
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Reflog), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Tree));

    // 11. 'g' (Action::OpenView(ViewKind::Grep))
    // Initial grep: prompts :grep
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Grep), h),
        Flow::Continue
    );
    assert!(app.prompt.is_some());
    app.prompt = None;
    // Grep with existing grep_view
    app.views.grep_view = Some(GrepView::new("v0".to_string(), vec![]));
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Grep), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Grep));
    // When in grep_view, pressing 'g' re-prompts
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Grep), h),
        Flow::Continue
    );
    assert!(app.prompt.is_some());
    app.prompt = None;

    // 12. 'p' (Action::OpenView(ViewKind::Pager))
    app.views.pager_view = None;
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Pager), h),
        Flow::Continue
    );
    assert_eq!(app.status_message.as_deref(), Some("No pager content"));
    app.views.pager_view = Some(PagerView::new(
        "Pager".to_string(),
        vec!["hello pager".to_string()],
    ));
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Pager), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Pager));
    // Toggle back
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Pager), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Grep));

    // 13. 'h' (Action::OpenView(ViewKind::Help))
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Help), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Help));
    assert!(app.views.help_view.is_some());
    // Toggle back
    assert_eq!(
        execute_action(&mut app, &Action::OpenView(ViewKind::Help), h),
        Flow::Continue
    );
    assert_eq!(app.active_view(), Some(ViewKind::Grep));

    // 14. Verify HelpView documents all 14 canonical view switching items
    let help = HelpView::new();
    let mut help_rendered = Vec::new();
    help.render(&mut help_rendered, 80, 50).unwrap();
    let help_text = String::from_utf8_lossy(&help_rendered);
    assert!(help_text.contains("view-main"));
    assert!(help_text.contains("view-diff"));
    assert!(help_text.contains("view-log"));
    assert!(help_text.contains("view-reflog"));
    assert!(help_text.contains("view-tree"));
    assert!(help_text.contains("view-blob"));
    assert!(help_text.contains("view-blame"));
    assert!(help_text.contains("view-refs"));
    assert!(help_text.contains("view-status"));
    assert!(help_text.contains("view-stage"));
    assert!(help_text.contains("view-stash"));
    assert!(help_text.contains("view-grep"));
    assert!(help_text.contains("view-pager"));
    assert!(help_text.contains("view-help"));
}

#[test]
fn test_all_toggle_actions_and_scroll_actions_coverage() {
    use crate::options::OptionId;
    let (_tmp, mut app, _ids) = diff_over_log_app(3);

    let toggle_actions = [
        Action::ToggleOption(OptionId::LineNumber),
        Action::ToggleOption(OptionId::Date),
        Action::ToggleOption(OptionId::Author),
        Action::ToggleOption(OptionId::Committer),
        Action::ToggleOption(OptionId::LineGraphics),
        Action::ToggleOption(OptionId::CommitTitleGraph),
        Action::ToggleOption(OptionId::FileName),
        Action::ToggleOption(OptionId::FileSize),
        Action::ToggleOption(OptionId::IgnoreSpace),
        Action::ToggleOption(OptionId::WordDiff),
        Action::ToggleOption(OptionId::CommitOrder),
        Action::ToggleOption(OptionId::CommitTitleRefs),
        Action::ToggleOption(OptionId::ShowChanges),
        Action::ToggleOption(OptionId::Id),
        Action::ToggleOption(OptionId::FileFilter),
        Action::ToggleOption(OptionId::RevFilter),
        Action::ToggleOption(OptionId::CommitTitleOverflow),
        Action::ToggleOption(OptionId::StatusShowUntrackedDirs),
        Action::ToggleOption(OptionId::VerticalSplit),
        Action::ToggleOption(OptionId::Mouse),
        Action::ToggleDiffContext(1),
        Action::ToggleDiffContext(-1),
        Action::ToggleOption(OptionId::DiffLayout),
        Action::ToggleOption(OptionId::DiffPresentation),
        Action::ToggleOption(OptionId::SyntaxHighlighting),
        Action::ToggleOption(OptionId::SyntaxTheme),
    ];

    for action in &toggle_actions {
        assert_eq!(execute_action(&mut app, action, 24), Flow::Continue);
        assert!(app.status_message.is_some());
    }

    // Hunk / File navigation actions in DiffView
    assert_eq!(
        execute_action(&mut app, &Action::NextHunk, 24),
        Flow::Continue
    );
    assert_eq!(
        execute_action(&mut app, &Action::PrevHunk, 24),
        Flow::Continue
    );
    assert_eq!(
        execute_action(&mut app, &Action::NextFile, 24),
        Flow::Continue
    );
    assert_eq!(
        execute_action(&mut app, &Action::PrevFile, 24),
        Flow::Continue
    );

    // Scroll actions
    let scroll_actions = [
        Action::ScrollLineDown,
        Action::ScrollLineUp,
        Action::ScrollPageDown,
        Action::ScrollPageUp,
        Action::ScrollLeft,
        Action::ScrollRight,
        Action::ScrollFirstCol,
    ];
    for action in &scroll_actions {
        assert_eq!(execute_action(&mut app, action, 24), Flow::Continue);
    }

    // Maximize, ViewNext, Options, Prompt, Search, SearchBack, FindNext, FindPrev
    assert_eq!(
        execute_action(&mut app, &Action::Maximize, 24),
        Flow::Continue
    );
    assert!(app.views.maximized);
    assert_eq!(
        execute_action(&mut app, &Action::Maximize, 24),
        Flow::Continue
    );
    assert!(!app.views.maximized);

    assert_eq!(
        execute_action(&mut app, &Action::ViewNext, 24),
        Flow::Continue
    );
    assert_eq!(
        execute_action(&mut app, &Action::Options, 24),
        Flow::Continue
    );
    assert!(app.prompt.is_some());
    assert_eq!(
        execute_action(&mut app, &Action::Prompt, 24),
        Flow::Continue
    );
    assert_eq!(
        execute_action(&mut app, &Action::Search, 24),
        Flow::Continue
    );
    assert_eq!(
        execute_action(&mut app, &Action::SearchBack, 24),
        Flow::Continue
    );
    assert_eq!(
        execute_action(&mut app, &Action::FindNext, 24),
        Flow::Continue
    );
    assert_eq!(
        execute_action(&mut app, &Action::FindPrev, 24),
        Flow::Continue
    );
}

#[test]
fn test_view_ref_view_mut_and_clear_view_all_13_kinds() {
    use super::dispatch::{
        NavMotion, scroll_view_down, scroll_view_first_col, scroll_view_left, scroll_view_right,
        scroll_view_up,
    };

    let (_tmp, mut app, ids) = diff_over_log_app(3);
    let head = ids[0];
    let engine = app.engine.as_ref().unwrap().clone();
    let diff_data = engine.compute_commit_diff(head).unwrap();
    let listing = engine.read_tree(head, "").unwrap();
    let blob = tigrs_git::BlobContent {
        oid: head,
        path: "file.txt".to_string(),
        size: 10,
        is_binary: false,
        lines: vec!["line 1".to_string(), "line 2".to_string()].into(),
    };
    let blame_res = engine.blame_file(head, "file.txt").unwrap();
    let (_src, token) = tigrs_core::cancel::CancellationToken::new();
    let status_report = engine.load_status(&token).unwrap();
    let refs_list = engine.list_refs().unwrap_or_default();
    let stashes = engine.list_stashes().unwrap_or_default();
    let reflog_entries = engine.read_reflog("HEAD").unwrap_or_default();

    let mut log_view = LogView::new("main".to_string());
    log_view.append_commit_diff(&diff_data, None);

    app.views.main_view = Some(MainView::new("main".to_string()));
    app.views.diff_view = Some(DiffView::new(diff_data));
    app.views.status_view = Some(StatusView::new(status_report));
    app.views.tree_view = Some(TreeView::new(listing));
    app.views.blob_view = Some(BlobView::new(head, blob));
    app.views.blame_view = Some(BlameView::from_result(blame_res));
    app.views.help_view = Some(HelpView::new());
    app.views.refs_view = Some(RefsView::new(refs_list));
    app.views.stash_view = Some(StashView::new(stashes));
    app.views.grep_view = Some(GrepView::new(
        "line".to_string(),
        vec![crate::view::GrepMatch {
            path: "file.txt".to_string(),
            line_num: 1,
            content: "line 1".to_string(),
        }],
    ));
    app.views.reflog_view = Some(ReflogView::new("HEAD".to_string(), reflog_entries));
    app.views.log_view = Some(log_view);
    app.views.pager_view = Some(PagerView::new(
        "pager".to_string(),
        vec!["pager line 1".to_string(), "pager line 2".to_string()],
    ));

    let all_kinds = [
        ViewKind::Main,
        ViewKind::Diff,
        ViewKind::Status,
        ViewKind::Tree,
        ViewKind::Blob,
        ViewKind::Blame,
        ViewKind::Help,
        ViewKind::Refs,
        ViewKind::Stash,
        ViewKind::Grep,
        ViewKind::Reflog,
        ViewKind::Log,
        ViewKind::Pager,
    ];

    let motions = [
        NavMotion::Down(1),
        NavMotion::Up(1),
        NavMotion::PageDown,
        NavMotion::PageUp,
        NavMotion::HalfPageDown,
        NavMotion::HalfPageUp,
        NavMotion::FirstLine,
        NavMotion::LastLine,
        NavMotion::ScrollDown(1),
        NavMotion::ScrollUp(1),
        NavMotion::SetCursor(0),
    ];

    for &kind in &all_kinds {
        // 1. ViewRef inspection & rendering
        let vref = app.view_ref(kind).expect("view_ref should exist");
        assert_eq!(vref.kind(), kind);
        let _ = vref.cursor();
        let _ = vref.scroll_offset();
        let _ = vref.line_count();
        let _ = vref.selected_commit_id();
        let mut buf = Vec::new();
        vref.render(&mut buf, 80, 24, &app.options).unwrap();

        // 2. ViewMut motions
        if let Some(vmut) = app.view_mut(kind) {
            for &motion in &motions {
                dispatch_motion(vmut, motion, 24);
            }
        }

        // 3. Scroll helpers
        scroll_view_right(&mut app, kind, 4);
        scroll_view_left(&mut app, kind, 2);
        scroll_view_first_col(&mut app, kind);
        scroll_view_down(&mut app, kind, 1, 24);
        scroll_view_up(&mut app, kind, 1, 24);

        // 4. clear_view
        app.clear_view(kind);
        assert!(app.view_ref(kind).is_none());
    }
}

#[test]
fn test_view_transitions_from_secondary_views_and_commands_coverage() {
    let tmp = tempfile::TempDir::new().unwrap();
    let p = tmp.path();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&["init"]);
    run(&["config", "user.name", "Tester"]);
    run(&["config", "user.email", "test@test.com"]);
    std::fs::create_dir_all(p.join("sub")).unwrap();
    std::fs::write(p.join("sub/file.txt"), "line 1\nline 2\n").unwrap();
    std::fs::write(p.join("root.txt"), "root 1\nroot 2\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-m", "commit 1"]);

    std::fs::write(p.join("sub/file.txt"), "line 1\nline 2 modified\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-m", "commit 2"]);

    // Create a stash entry
    std::fs::write(p.join("root.txt"), "root 1 modified\n").unwrap();
    run(&["stash", "push", "-m", "test stash"]);

    // Leave unstaged modification
    std::fs::write(p.join("root.txt"), "root unstaged\n").unwrap();

    let engine = GitEngine::open(Some(p)).unwrap();
    let head = engine.head_commit_id().unwrap();
    let diff_data = engine.compute_commit_diff(head).unwrap();
    let listing = engine.read_tree(head, "").unwrap();
    let blob = engine.read_blob_at_commit_path(head, "root.txt").unwrap();
    let blame_res = engine.blame_file(head, "root.txt").unwrap();
    let (_src, token) = tigrs_core::cancel::CancellationToken::new();
    let status_report = engine.load_status(&token).unwrap();
    let refs_list = engine.list_refs().unwrap_or_default();
    let stashes = engine.list_stashes().unwrap_or_default();
    let reflog_entries = engine.read_reflog("HEAD").unwrap_or_default();

    let mut log_view = LogView::new("main".to_string());
    log_view.append_commit_diff(&diff_data, None);

    let mut main_view = MainView::new("main".to_string());
    let (_src2, token2) = tigrs_core::cancel::CancellationToken::new();
    for batch in engine
        .stream_commits(Some(head), Some(10), token2)
        .unwrap()
        .flatten()
    {
        main_view.append_commits(batch);
    }
    main_view.set_finished();

    let mut app = AppState {
        views: ViewManager {
            main_view: Some(main_view),
            diff_view: Some(DiffView::new(diff_data)),
            status_view: Some(StatusView::new(status_report)),
            tree_view: Some(TreeView::new(listing)),
            blob_view: Some(BlobView::new(head, blob)),
            blame_view: Some(BlameView::from_result(blame_res)),
            help_view: Some(HelpView::new()),
            refs_view: Some(RefsView::new(refs_list)),
            stash_view: Some(StashView::new(stashes)),
            grep_view: Some(GrepView::new(
                "line".to_string(),
                vec![crate::view::GrepMatch {
                    path: "sub/file.txt".to_string(),
                    line_num: 1,
                    content: "line 1".to_string(),
                }],
            )),
            reflog_view: Some(ReflogView::new("HEAD".to_string(), reflog_entries)),
            log_view: Some(log_view),
            ..Default::default()
        },
        engine: Some(engine),
        ..Default::default()
    };

    let secondary_views = [
        ViewKind::Tree,
        ViewKind::Blob,
        ViewKind::Blame,
        ViewKind::Refs,
        ViewKind::Stash,
        ViewKind::Reflog,
        ViewKind::Grep,
        ViewKind::Log,
        ViewKind::Status,
        ViewKind::Main,
        ViewKind::Diff,
    ];

    let actions_to_test = [
        Action::OpenView(ViewKind::Log),
        Action::OpenView(ViewKind::Diff),
        Action::ViewStage,
        Action::OpenView(ViewKind::Tree),
        Action::OpenView(ViewKind::Blob),
        Action::OpenView(ViewKind::Blame),
        Action::Enter,
        Action::Parent,
        Action::Back,
    ];

    for &view_kind in &secondary_views {
        for action in &actions_to_test {
            app.views.view_stack = vec![ViewKind::Main, view_kind];
            let flow = execute_action(&mut app, action, 24);
            assert_eq!(flow, Flow::Continue);
        }
    }

    // Test TreeView subdirectory navigation with Enter, Parent, Back, and ViewClose
    let sub_listing = app.engine.as_ref().unwrap().read_tree(head, "sub").unwrap();
    app.views.tree_view = Some(TreeView::new(sub_listing.clone()));
    app.views.view_stack = vec![ViewKind::Main, ViewKind::Tree];
    // Enter on ParentDir row (cursor 0)
    execute_action(&mut app, &Action::Enter, 24);
    // Re-open sub
    app.views.tree_view = Some(TreeView::new(sub_listing.clone()));
    app.views.view_stack = vec![ViewKind::Main, ViewKind::Tree];
    execute_action(&mut app, &Action::Parent, 24);
    app.views.tree_view = Some(TreeView::new(sub_listing.clone()));
    app.views.view_stack = vec![ViewKind::Main, ViewKind::Tree];
    execute_action(&mut app, &Action::Back, 24);
    app.views.tree_view = Some(TreeView::new(sub_listing));
    app.views.view_stack = vec![ViewKind::Main, ViewKind::Tree];
    execute_action(&mut app, &Action::ViewClose, 24);

    // Test ParsedCommand::Goto with valid commit OID and short prefix
    app.views.view_stack = vec![ViewKind::Main];
    let short_sha = &head.to_string()[..7];
    app.status_message = None;
    execute_parsed_command(&mut app, ParsedCommand::Goto(head.to_string()), 24);
    assert!(app.status_message.is_none());
    execute_parsed_command(&mut app, ParsedCommand::Goto(short_sha.to_string()), 24);
    assert!(app.status_message.is_none());

    // Test ParsedCommand::Set with show-changes option
    execute_parsed_command(
        &mut app,
        ParsedCommand::parse(":set show-changes = yes"),
        24,
    );
    assert!(app.options.show_changes);
    assert!(
        app.status_message
            .as_deref()
            .unwrap_or("")
            .contains(":set show-changes = yes")
    );

    execute_parsed_command(&mut app, ParsedCommand::parse(":toggle unknown-option"), 24);
    assert!(
        app.status_message
            .as_deref()
            .unwrap_or("")
            .contains("Unknown option")
    );

    // Test search in LogView and MainView with ChangesRows
    app.views.view_stack = vec![ViewKind::Log];
    execute_search(&mut app, "commit", SearchDirection::Forward, 24);
    execute_search(&mut app, "commit", SearchDirection::Backward, 24);

    app.views.view_stack = vec![ViewKind::Main];
    app.options.show_changes = true;
    app.apply_changes_rows();
    execute_search(&mut app, "Unstaged", SearchDirection::Forward, 24);
    execute_search(&mut app, "commit", SearchDirection::Backward, 24);
}

#[test]
fn test_half_page_navigation_in_tree_blob_and_blame_views() {
    let (_tmp, mut app, ids) = diff_over_log_app(2);
    let commit_id = ids[0];

    let lines: Vec<String> = (1..=30).map(|i| format!("line {i}")).collect();
    let blob = tigrs_git::BlobContent {
        oid: commit_id,
        path: "test.rs".to_string(),
        size: 300,
        is_binary: false,
        lines: lines.into(),
    };

    // 1. BlobView with 30 lines
    app.views.blob_view = Some(BlobView::new(commit_id, blob.clone()));
    app.views.view_stack = vec![ViewKind::Blob];
    if let Some(v) = app.view_mut(ViewKind::Blob) {
        dispatch_motion(v, NavMotion::HalfPageDown, 20);
    }
    assert_eq!(
        app.views.blob_view.as_ref().unwrap().cursor(),
        10,
        "HalfPageDown in BlobView with height=20 must advance 10 rows"
    );
    if let Some(v) = app.view_mut(ViewKind::Blob) {
        dispatch_motion(v, NavMotion::HalfPageUp, 20);
    }
    assert_eq!(
        app.views.blob_view.as_ref().unwrap().cursor(),
        0,
        "HalfPageUp in BlobView must return to row 0"
    );

    // 2. BlameView with 30 lines
    app.views.blame_view = Some(BlameView::from_blob(commit_id, blob));
    app.views.view_stack = vec![ViewKind::Blame];
    if let Some(v) = app.view_mut(ViewKind::Blame) {
        dispatch_motion(v, NavMotion::HalfPageDown, 20);
    }
    assert_eq!(
        app.views.blame_view.as_ref().unwrap().cursor(),
        10,
        "HalfPageDown in BlameView with height=20 must advance 10 rows"
    );
    if let Some(v) = app.view_mut(ViewKind::Blame) {
        dispatch_motion(v, NavMotion::HalfPageUp, 20);
    }
    assert_eq!(
        app.views.blame_view.as_ref().unwrap().cursor(),
        0,
        "HalfPageUp in BlameView must return to row 0"
    );

    // 3. TreeView with 30 entries
    let entries: Vec<tigrs_git::TreeEntry> = (1..=30)
        .map(|i| tigrs_git::TreeEntry {
            name: format!("file_{i:02}.rs"),
            path: format!("file_{i:02}.rs"),
            kind: tigrs_git::TreeEntryKind::Blob,
            oid: commit_id,
            mode: 0o100_644,
            size: Some(128),
        })
        .collect();
    let listing = tigrs_git::TreeListing {
        commit_oid: commit_id,
        path: String::new(),
        parent_path: None,
        entries,
    };
    app.views.tree_view = Some(TreeView::new(listing));
    app.views.view_stack = vec![ViewKind::Tree];
    if let Some(v) = app.view_mut(ViewKind::Tree) {
        dispatch_motion(v, NavMotion::HalfPageDown, 20);
    }
    assert_eq!(
        app.views.tree_view.as_ref().unwrap().cursor(),
        10,
        "HalfPageDown in TreeView with height=20 must advance 10 rows"
    );
}

#[test]
fn test_option_effect_selective_refresh_and_active_macro_context() {
    use crate::options::OptionEffect;

    let (_tmp, mut app, ids) = diff_over_log_app(2);

    // Verify OptionEffect classification
    let (_, eff_lineno) = app
        .options
        .toggle_by_name_with_effect("line-number")
        .unwrap();
    assert_eq!(eff_lineno, OptionEffect::None);

    let (_, eff_changes) = app
        .options
        .toggle_by_name_with_effect("show-changes")
        .unwrap();
    assert_eq!(eff_changes, OptionEffect::RefreshChanges);

    let (_, eff_syntax) = app
        .options
        .toggle_by_name_with_effect("syntax-highlighting")
        .unwrap();
    assert_eq!(eff_syntax, OptionEffect::RefreshSyntax);

    let (_, eff_layout) = app
        .options
        .set_by_name("diff-layout", "side-by-side")
        .unwrap();
    assert_eq!(eff_layout, OptionEffect::RefreshDiff);

    // Verify macro_context prioritizes active view (e.g. StatusView) over background diff_view
    let status_report = tigrs_git::StatusReport {
        branch: "main".to_string(),
        head_commit: Some(ids[0]),
        staged: vec![],
        unstaged: vec![tigrs_git::StatusItem::new(
            'M',
            tigrs_git::StatusSection::Unstaged,
            "priority_status_file.rs",
            None,
        )],
        untracked: vec![],
        unmerged: vec![],
    };
    app.views.status_view = Some(StatusView::new(status_report));
    // Status is top of stack while Diff is also open
    app.views.view_stack = vec![ViewKind::Diff, ViewKind::Status];
    assert_eq!(app.active_view(), Some(ViewKind::Status));

    let ctx = app.macro_context();
    assert_eq!(
        ctx.file.as_deref(),
        Some("priority_status_file.rs"),
        "macro_context must reflect active StatusView file rather than background DiffView"
    );
    assert_eq!(ctx.commit, Some(ids[0].to_string()));
}

#[test]
fn test_view_ref_polymorphic_dispatch() {
    let help = HelpView::new();
    let mut vm = ViewManager {
        help_view: Some(help),
        ..Default::default()
    };
    let vref = vm.view_ref(ViewKind::Help).unwrap();
    assert_eq!(vref.kind(), ViewKind::Help);
    assert!(vref.line_count() > 0);

    let vmut = vm.view_mut(ViewKind::Help).unwrap();
    assert_eq!(vmut.kind(), ViewKind::Help);
}

#[test]
fn test_view_manager_and_app_state_split_borrowing() {
    let mut app = app_with(10);
    app.views.view_stack = vec![ViewKind::Main];

    let AppState {
        ref mut views,
        ref mut options,
        ref mut status_message,
        ..
    } = app;
    assert!(views.has_view(ViewKind::Main));
    assert_eq!(views.active_view(), Some(ViewKind::Main));

    // Mutate views and context concurrently without borrow conflict
    views.push_view(ViewKind::Help);
    views.help_view = Some(HelpView::new());
    options.line_number = true;
    *status_message = Some("Testing split borrow".to_string());

    assert_eq!(views.active_view(), Some(ViewKind::Help));
    let popped = views.pop_active_view();
    assert_eq!(popped, Some(ViewKind::Help));
    assert!(views.help_view.is_none());
}

#[test]
fn test_phase2_terminal_renderer_double_buffer_swap_and_invalidate() {
    let (_tmp, app, _ids) = diff_over_log_app(2);
    let mut out = Vec::new();
    render_active(&app, &mut out, 80, 24).unwrap();
    assert!(!out.is_empty());

    // After split rendering, back_grid should be valid
    {
        let renderer = app.renderer.borrow();
        assert!(renderer.has_valid_back_grid());
    }

    // Repainting an identical split frame should emit zero diff bytes
    out.clear();
    render_active(&app, &mut out, 80, 24).unwrap();
    assert!(
        out.is_empty(),
        "Unchanged double-buffered split frame should emit 0 bytes, got {} bytes",
        out.len()
    );

    // Invalidating screen should force full repaint
    app.invalidate_screen();
    {
        let renderer = app.renderer.borrow();
        assert!(!renderer.has_valid_back_grid());
    }
    out.clear();
    render_active(&app, &mut out, 80, 24).unwrap();
    assert!(!out.is_empty(), "Invalidated frame must emit full repaint");
}

#[test]
fn test_phase2_active_task_cancels_supersedes_previous_tokens() {
    let mut cancels = ActiveTaskCancels::default();

    let status_tok1 = cancels.reset_status();
    assert!(!status_tok1.is_cancelled());
    let status_tok2 = cancels.reset_status();
    assert!(
        status_tok1.is_cancelled(),
        "New status reset must cancel previous token"
    );
    assert!(!status_tok2.is_cancelled());

    let prefetch_tok1 = cancels.reset_prefetch();
    assert!(!prefetch_tok1.is_cancelled());
    let prefetch_tok2 = cancels.reset_prefetch();
    assert!(prefetch_tok1.is_cancelled());
    assert!(!prefetch_tok2.is_cancelled());
    cancels.cancel_prefetch();
    assert!(prefetch_tok2.is_cancelled());
}

#[test]
fn test_phase2_options_registry_and_option_id_exhaustive_consistency() {
    use crate::options::{OPTIONS_REGISTRY, OptionId};

    let mut opts = ViewOptions::default();
    for desc in OPTIONS_REGISTRY {
        let mut copy_a = opts.clone();
        let mut copy_b = opts.clone();

        let (msg_id, eff_id) = copy_a.toggle_by_id(desc.id);
        let (msg_name, eff_name) = copy_b
            .toggle_by_name_with_effect(desc.canonical_name)
            .expect("Canonical name in OPTIONS_REGISTRY must resolve");

        assert_eq!(msg_id, msg_name, "Mismatch for {:?}", desc.id);
        assert_eq!(eff_id, eff_name, "Effect mismatch for {:?}", desc.id);
        assert_eq!(
            copy_a, copy_b,
            "State mismatch after toggling {:?}",
            desc.id
        );

        let action = Action::ToggleOption(desc.id);
        assert_eq!(action.name(), desc.id.action_name());

        opts = copy_a;
    }

    // Verify Action::parse maps toggle commands directly to ToggleOption
    assert_eq!(
        Action::parse("toggle-lineno").unwrap(),
        Action::ToggleOption(OptionId::LineNumber)
    );
    assert_eq!(
        Action::parse("toggle-word-diff").unwrap(),
        Action::ToggleOption(OptionId::WordDiff)
    );
    assert_eq!(
        Action::parse("toggle-syntax-theme").unwrap(),
        Action::ToggleOption(OptionId::SyntaxTheme)
    );
}

#[test]
fn test_phase3_embedded_view_manager_disjoint_borrow() {
    let mut app = app_with(8);
    app.views.view_stack = vec![ViewKind::Main];

    let AppState {
        ref mut views,
        ref mut options,
        ref mut status_message,
        ..
    } = app;

    assert!(views.has_view(ViewKind::Main));
    assert_eq!(views.active_view(), Some(ViewKind::Main));

    views.help_view = Some(HelpView::new());
    views.push_view(ViewKind::Help);
    options.file_size = true;
    *status_message = Some("Embedded ViewManager disjoint borrow test".to_string());

    assert_eq!(app.active_view(), Some(ViewKind::Help));
    assert!(app.options.file_size);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Embedded ViewManager disjoint borrow test")
    );
}

#[test]
fn stackable_and_fallback_view_sets_are_disjoint() {
    use super::{FALLBACK_VIEWS, STACKABLE_VIEWS};
    for s in STACKABLE_VIEWS {
        assert!(
            !FALLBACK_VIEWS.contains(&s),
            "ViewKind::{s:?} must not appear in both STACKABLE_VIEWS and FALLBACK_VIEWS"
        );
    }
}

#[test]
fn test_every_registry_option_round_trips_through_set_by_name() {
    use crate::options::{OPTIONS_REGISTRY, OptionId};
    let mut opts = ViewOptions::default();
    for desc in OPTIONS_REGISTRY {
        let res = opts.set_by_name(desc.canonical_name, "toggle");
        assert!(
            res.is_ok(),
            "ViewOptions::set_by_name({:?}, \"toggle\") failed: {:?}",
            desc.canonical_name,
            res
        );
        let explicit_val = match desc.id {
            OptionId::Date => "relative",
            OptionId::Author => "full",
            OptionId::LineGraphics => "utf-8",
            OptionId::CommitTitleGraph => "v2",
            OptionId::IgnoreSpace => "all",
            OptionId::CommitOrder => "topo",
            OptionId::VerticalSplit => "vertical",
            OptionId::DiffPresentation => "fancy",
            OptionId::DiffIndicator => "auto",
            OptionId::DiffLayout => "side-by-side",
            OptionId::WordDiffPairing => "similarity",
            OptionId::SyntaxTheme => "github-dark",
            OptionId::UiTheme => "catppuccin-mocha",
            OptionId::CommitTitleOverflow => "50",
            _ => "yes",
        };
        let res2 = opts.set_by_name(desc.canonical_name, explicit_val);
        assert!(
            res2.is_ok(),
            "ViewOptions::set_by_name({:?}, {:?}) failed: {:?}",
            desc.canonical_name,
            explicit_val,
            res2
        );
        // Also verify OptionId::descriptor(desc.id) matches
        assert_eq!(desc.id.descriptor().canonical_name, desc.canonical_name);
    }
}

#[test]
fn test_log_view_open_at_deep_index_uses_placeholders() {
    let (_tmp, mut app, ids) = diff_over_log_app(15);
    let (log_tx, _log_rx) = bounded(16);
    app.log_task.tx = Some(log_tx);
    app.views.log_view = None;

    // Move cursor to commit index 10 in MainView
    if let Some(ref mut main) = app.views.main_view {
        main.set_cursor(10, 24);
    }
    app.views.view_stack = vec![ViewKind::Main];

    let _ = crate::app::actions::view_switch::handle_view_log(&mut app, 24);
    let log = app
        .views
        .log_view
        .as_ref()
        .expect("log_view must be created");

    // Verify selected commit in LogView matches target commit at index 10
    assert_eq!(log.selected_commit_id(), Some(ids[10]));

    let mut rendered = Vec::new();
    render_active(&app, &mut rendered, 100, 40).unwrap();
    let text = String::from_utf8_lossy(&rendered);
    assert!(
        text.contains(&ids[10].to_string()[..7]),
        "Log view should jump to and display target commit at index 10"
    );
}

#[test]
fn test_blame_back_preserves_history_when_cancelled() {
    use tigrs_git::BlameResult;
    let oid2 = ObjectId::from_bytes_or_panic(&[2; 20]);
    let mut blame = BlameView::from_result(BlameResult {
        commit_id: oid2,
        path: "src/lib.rs".to_string(),
        is_binary: false,
        lines: vec![],
    });
    blame.push_history();

    let (_tmp, mut app, _ids) = diff_over_log_app(2);
    let (blame_tx, _blame_rx) = bounded(16);
    app.blame_task.tx = Some(blame_tx);
    app.views.blame_view = Some(blame);
    app.views.view_stack = vec![ViewKind::Blame];

    // Trigger handle_back — should spawn async worker and NOT pop history yet
    let _ = crate::app::actions::navigation::handle_back(&mut app, 24);
    assert!(
        app.views
            .blame_view
            .as_ref()
            .unwrap()
            .peek_history()
            .is_some(),
        "Blame history must remain intact (peeked, not popped) while async blame is in flight"
    );

    // Cancel active blame task before worker finishes
    app.cancel_for_view(ViewKind::Blame);
    assert!(
        app.views
            .blame_view
            .as_ref()
            .unwrap()
            .peek_history()
            .is_some(),
        "Blame history must still be intact after cancellation"
    );
}

#[test]
fn test_closing_log_view_cancels_stream() {
    let mut app = app_with(5);
    let (_, token, _) = app.log_task.begin_request();
    assert!(!token.is_cancelled());

    app.views.log_view = Some(crate::view::log_view::LogView::new("main".to_string()));
    app.push_view(ViewKind::Log);
    assert_eq!(app.active_view(), Some(ViewKind::Log));

    let popped = app.pop_active_view();
    assert_eq!(popped, Some(ViewKind::Log));
    assert!(
        token.is_cancelled(),
        "Popping LogView must cancel its active background streaming token"
    );
}

#[test]
fn test_phase5_async_task_slot_lifecycle() {
    let mut slot: AsyncTaskSlot<u32> = AsyncTaskSlot::default();
    assert!(!slot.has_sender());

    let (tx, _rx) = bounded::<u32>(4);
    slot.tx = Some(tx);
    assert!(slot.has_sender());

    let (req1, tok1, sender1) = slot.begin_request();
    assert_eq!(req1, 1);
    assert!(slot.is_current(1));
    assert!(!tok1.is_cancelled());
    assert!(sender1.is_some());

    let (req2, tok2, _) = slot.begin_request();
    assert_eq!(req2, 2);
    assert!(!slot.is_current(1));
    assert!(slot.is_current(2));
    assert!(
        tok1.is_cancelled(),
        "Superseding request must cancel previous token"
    );
    assert!(!tok2.is_cancelled());

    slot.cancel_in_flight();
    assert!(tok2.is_cancelled());
}

#[test]
fn test_phase5_never_channel_does_not_spin_in_select() {
    let rx = crossbeam_channel::never::<Vec<CommitSummary>>();
    let default_hit = crossbeam_channel::select! {
        recv(rx) -> _ => false,
        default => true,
    };
    assert!(default_hit);

    let (tx, disconnected_rx) = bounded::<Vec<CommitSummary>>(1);
    drop(tx);
    let mut active_rx = disconnected_rx;
    let mut iterations = 0;
    for _ in 0..5 {
        crossbeam_channel::select! {
            recv(active_rx) -> msg => {
                if msg.is_err() {
                    active_rx = crossbeam_channel::never();
                } else {
                    panic!("disconnected channel cannot yield Ok");
                }
            }
            default => {
                iterations += 1;
            }
        }
    }
    assert_eq!(
        iterations, 4,
        "After first iteration disconnects, remaining iterations must fall through to default without spinning"
    );
}

#[test]
fn test_options_panel_interactive_save_and_source_workflow() {
    let key_event = |code: KeyCode| Event::Key(KeyEvent::from(code));
    let dir = tempfile::tempdir().unwrap();
    let target_toml = dir.path().join("my_saved_config.toml");

    let mut app = AppState::default();
    app.views.main_view = Some(MainView::new("main".to_string()));
    app.push_view(ViewKind::Main);

    // 1. Open Options & Config Panel via 'o'
    let _ = handle_event_with_dimensions(&key_event(KeyCode::Char('o')), &mut app, 100, 30);
    assert!(matches!(
        app.prompt,
        Some(PromptState {
            kind: PromptKind::OptionMenu(_),
            ..
        })
    ));

    // 2. Press '.' hotkey to toggle line-number in-place (panel stays open!)
    assert!(!app.options.line_number);
    let _ = handle_event_with_dimensions(&key_event(KeyCode::Char('.')), &mut app, 100, 30);
    assert!(app.options.line_number);
    assert!(
        app.prompt.is_some(),
        "Options panel must remain open after hotkey/Space toggle"
    );

    // 3. Press '3' for History & Graph tab, then 'D' to select and cycle Date Format forward, then '[' backward and ']' forward
    let _ = handle_event_with_dimensions(&key_event(KeyCode::Char('3')), &mut app, 100, 30);
    let _ = handle_event_with_dimensions(&key_event(KeyCode::Char('D')), &mut app, 100, 30);
    assert_eq!(app.options.date_format, crate::options::DateFormat::Short);
    let _ = handle_event_with_dimensions(&key_event(KeyCode::Char('[')), &mut app, 100, 30);
    assert_eq!(
        app.options.date_format,
        crate::options::DateFormat::Relative
    );
    let _ = handle_event_with_dimensions(&key_event(KeyCode::Char(']')), &mut app, 100, 30);
    assert_eq!(app.options.date_format, crate::options::DateFormat::Short);

    // 4. Render the active screen with the Options Panel open and verify panel contents
    let mut render_buf = Vec::new();
    render_active(&app, &mut render_buf, 100, 30).unwrap();
    let rendered_str = String::from_utf8_lossy(&render_buf);
    assert!(rendered_str.contains("Options & Config"));
    assert!(rendered_str.contains("Commit Date"));

    // 5. Press 'P' (Save-As), clear default path with Ctrl+U, type custom path, and press Enter
    let _ = handle_event_with_dimensions(&key_event(KeyCode::Char('P')), &mut app, 100, 30);
    let ctrl_u = Event::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    let _ = handle_event_with_dimensions(&ctrl_u, &mut app, 100, 30);
    for ch in target_toml.to_string_lossy().chars() {
        let _ = handle_event_with_dimensions(&key_event(KeyCode::Char(ch)), &mut app, 100, 30);
    }
    let _ = handle_event_with_dimensions(&key_event(KeyCode::Enter), &mut app, 100, 30);
    assert!(
        target_toml.exists(),
        "Custom TOML file should be created on disk"
    );

    // 6. Close panel with 'q', reset options with ':set line-number = no', then ':source <path>'
    let _ = handle_event_with_dimensions(&key_event(KeyCode::Char('q')), &mut app, 100, 30);
    assert!(app.prompt.is_none());

    app.options.line_number = false;
    app.options.date_format = crate::options::DateFormat::Relative;

    let cmd = ParsedCommand::parse(&format!("source {}", target_toml.display()));
    let _ = execute_parsed_command(&mut app, cmd, 24);
    assert!(
        app.options.line_number,
        ":source must restore saved line_number = true"
    );
    assert_eq!(
        app.options.date_format,
        crate::options::DateFormat::Short,
        ":source must restore saved date = short"
    );
}

#[test]
fn test_options_panel_navigation_and_toggle_damage_tracking_byte_budget() {
    let key_event = |code: KeyCode| Event::Key(KeyEvent::from(code));
    let mut app = AppState::default();
    app.views.main_view = Some(MainView::new("main".to_string()));
    app.push_view(ViewKind::Main);

    // Prime initial frame into app.renderer.back_grid
    let mut buf = Vec::new();
    render_active(&app, &mut buf, 120, 40).unwrap();

    // Open Options Panel ('o') and render frame
    let _ = handle_event_with_dimensions(&key_event(KeyCode::Char('o')), &mut app, 120, 40);
    buf.clear();
    render_active(&app, &mut buf, 120, 40).unwrap();
    assert!(String::from_utf8_lossy(&buf).contains("Options & Config"));

    // Navigate Down ('j') inside Options Panel: only the 2 changed highlight rows + hint row
    // should be emitted via ScreenGrid damage tracking (< 1,500 bytes, not full 11 KB drawer).
    let _ = handle_event_with_dimensions(&key_event(KeyCode::Char('j')), &mut app, 120, 40);
    assert!(
        app.renderer.borrow().has_valid_back_grid,
        "Options Panel navigation must not invalidate back_grid"
    );
    buf.clear();
    render_active(&app, &mut buf, 120, 40).unwrap();
    assert!(
        buf.len() < 1500,
        "Options panel 'j' navigation emitted {} bytes (expected < 1,500 bytes with ScreenGrid overlay compositing)",
        buf.len()
    );

    // Toggle option with Space: must not clear back_grid
    let _ = handle_event_with_dimensions(&key_event(KeyCode::Char(' ')), &mut app, 120, 40);
    assert!(
        app.renderer.borrow().has_valid_back_grid,
        "Toggling an option in Options Panel must not trigger app.invalidate_screen()"
    );
}

#[test]
fn test_diff_document_cache_instant_relayout_and_toggle_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&["init"]);
    run(&["config", "user.name", "Alice Developer"]);
    run(&["config", "user.email", "alice@example.com"]);
    std::fs::write(p.join("c1.txt"), "commit 1\n").unwrap();
    run(&["add", "c1.txt"]);
    run(&["commit", "-m", "first commit"]);

    let engine = GitEngine::open(Some(p)).unwrap();
    let c1_id = engine.head_commit_id().unwrap();
    let mut app = AppState::with_engine_and_config(Some(engine.clone()), None);
    let diff = std::sync::Arc::new(engine.compute_commit_diff(c1_id).unwrap());

    // First build populates DiffDocumentCache for Unified layout
    let view_unified = app.create_diff_view(std::sync::Arc::clone(&diff));
    let unified_doc_ptr = std::sync::Arc::as_ptr(view_unified.document_arc());
    assert_eq!(app.diff_document_cache.borrow().len(), 1);

    // Toggle to SideBySide: populates second entry in DiffDocumentCache
    app.options.diff_layout = crate::options::DiffLayout::SideBySide;
    let view_sbs = app.create_diff_view(std::sync::Arc::clone(&diff));
    let sbs_doc_ptr = std::sync::Arc::as_ptr(view_sbs.document_arc());
    assert_ne!(unified_doc_ptr, sbs_doc_ptr);
    assert_eq!(app.diff_document_cache.borrow().len(), 2);

    // Toggle back to Unified: must be an O(1) Arc pointer hit to the exact same DiffDocument!
    app.options.diff_layout = crate::options::DiffLayout::Unified;
    let view_unified_again = app.create_diff_view(std::sync::Arc::clone(&diff));
    assert_eq!(
        std::sync::Arc::as_ptr(view_unified_again.document_arc()),
        unified_doc_ptr,
        "Round-trip option toggle must reuse cached Arc<DiffDocument> without re-running Syntect"
    );
}

#[test]
fn test_status_split_diff_cache_and_submillisecond_command_polling() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&["init"]);
    run(&["config", "user.name", "Alice Developer"]);
    run(&["config", "user.email", "alice@example.com"]);
    std::fs::write(p.join("file_0.txt"), "initial line\n").unwrap();
    run(&["add", "file_0.txt"]);
    run(&["commit", "-m", "initial commit"]);
    std::fs::write(p.join("file_0.txt"), "modified line 1\nline 2\n").unwrap();

    let engine = GitEngine::open(Some(p)).unwrap();
    let mut app = AppState::with_engine_and_config(Some(engine.clone()), None);
    let (_src, token) = CancellationToken::new();
    let report = engine.load_status(&token).unwrap();
    app.views.status_view = Some(crate::view::status_view::StatusView::new(report));
    let head_diff = engine
        .compute_commit_diff(engine.head_commit_id().unwrap())
        .unwrap();
    app.views.diff_view = Some(app.create_diff_view(head_diff));
    app.push_view(ViewKind::Status);
    app.views.maximized = false;

    // Select the unstaged file row in StatusView
    for _ in 0..5 {
        if app
            .views
            .status_view
            .as_ref()
            .and_then(|sv| sv.selected_item())
            .is_some()
        {
            break;
        }
        if let Some(sv) = app.views.status_view.as_mut() {
            sv.move_down(1, 24);
        }
    }
    let selected = app
        .views
        .status_view
        .as_ref()
        .and_then(|sv| sv.selected_item())
        .expect("Should select a StatusItem")
        .clone();

    // First sync populates status_item_diff_cache
    crate::app::sync_split_status_diff_if_open(&mut app);
    let generation = app.engine.as_ref().unwrap().generation();
    let render_key = app.current_diff_render_key();
    assert!(
        app.status_item_diff_cache
            .contains_key(&(selected.clone(), generation, render_key)),
        "sync_split_status_diff_if_open must cache StatusItem diff by (StatusItem, generation, render_key)"
    );
    let first_doc_ptr =
        std::sync::Arc::as_ptr(app.views.diff_view.as_ref().unwrap().document_arc());

    // Second sync on same item & generation must reuse cached DiffView without spawning git diff
    crate::app::sync_split_status_diff_if_open(&mut app);
    let second_doc_ptr =
        std::sync::Arc::as_ptr(app.views.diff_view.as_ref().unwrap().document_arc());
    assert_eq!(first_doc_ptr, second_doc_ptr);
}

#[test]
fn test_read_only_mode_default_badge_warnings_and_unlock() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&["init"]);
    run(&["config", "user.name", "Alice Developer"]);
    run(&["config", "user.email", "alice@example.com"]);
    std::fs::write(p.join("file_0.txt"), "initial line\n").unwrap();
    run(&["add", "file_0.txt"]);
    run(&["commit", "-m", "initial commit"]);

    let engine = GitEngine::open(Some(p)).unwrap();
    assert!(!engine.is_read_only());

    let mut app = AppState::with_engine_and_config(Some(engine.clone()), None);
    assert!(!app.is_read_only());
    app.views.main_view = Some(crate::view::MainView::new("main".to_string()));
    app.push_view(ViewKind::Main);

    // 0. By default (Update Mode), title bar does NOT display [RO] badge
    let mut term0 = crate::headless::HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term0, 80, 24).unwrap();
    assert!(
        !term0.line_text(0).contains("[RO]"),
        "Title bar must not contain [RO] in default update mode"
    );

    // 1. Enabling Read-Only mode via `:set read-only = true` displays [RO] badge and syncs GitEngine
    execute_parsed_command(&mut app, ParsedCommand::parse(":set read-only = true"), 24);
    assert!(app.is_read_only());
    assert!(app.engine.as_ref().unwrap().is_read_only());
    let mut term = crate::headless::HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term, 80, 24).unwrap();
    term.assert_line_contains(0, "[RO]");

    // 2. Saving config inside the repository worktree is blocked in Read-Only mode
    let inside_repo_cfg = p.join("inside_repo_config.toml");
    app.save_config_to_toml(Some(&inside_repo_cfg.to_string_lossy()), true);
    assert_eq!(
        app.status_message.as_deref(),
        Some(crate::app::READ_ONLY_WARNING_MSG)
    );
    assert!(
        !inside_repo_cfg.exists(),
        "save_config_to_toml must not write inside the repository in read-only mode"
    );

    // 3. Unlock via `:set read-only = false` removes [RO] badge and syncs GitEngine
    execute_parsed_command(&mut app, ParsedCommand::parse(":set read-only = false"), 24);
    assert!(!app.is_read_only());
    assert!(!app.engine.as_ref().unwrap().is_read_only());

    let mut term2 = crate::headless::HeadlessTerminal::new(80, 24);
    render_active(&app, &mut term2, 80, 24).unwrap();
    assert!(
        !term2.line_text(0).contains("[RO]"),
        "Title bar must not contain [RO] after unlocking update mode"
    );

    // 4. Re-lock via `:read-only` / `:ro` and unlock via `:update-mode` / `:rw` / `:set noro`
    execute_parsed_command(&mut app, ParsedCommand::parse(":ro"), 24);
    assert!(app.is_read_only());
    assert!(app.engine.as_ref().unwrap().is_read_only());

    execute_parsed_command(&mut app, ParsedCommand::parse(":rw"), 24);
    assert!(!app.is_read_only());
    assert!(!app.engine.as_ref().unwrap().is_read_only());

    execute_parsed_command(&mut app, ParsedCommand::parse(":set ro"), 24);
    assert!(app.is_read_only());

    execute_parsed_command(&mut app, ParsedCommand::parse(":set noro"), 24);
    assert!(!app.is_read_only());
}

#[test]
fn test_render_active_main_view_spotlight_selected_row_full_width_and_lazy_total_commits() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(p)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&["init", "-b", "main"]);
    run(&["config", "user.name", "Alice Developer"]);
    run(&["config", "user.email", "alice@example.com"]);
    std::fs::write(p.join("a.txt"), "one\n").unwrap();
    run(&["add", "a.txt"]);
    run(&["commit", "-m", "feat: first commit"]);
    std::fs::write(p.join("a.txt"), "two\n").unwrap();
    run(&["add", "a.txt"]);
    run(&["commit", "-m", "fix: second commit"]);
    run(&["-c", "pack.writeReverseIndex=true", "repack", "-ad", "-q"]);

    let engine = GitEngine::open(Some(p)).unwrap();
    assert_eq!(engine.fast_commit_count_estimate(), Some(2));

    // Start in StatusView with `main_view = None`, then switch to MainView via `Action::ViewMain`:
    // `handle_view_main` must initialize `total_commits` from `engine.fast_commit_count_estimate()`.
    let mut app = AppState::with_engine_and_config(Some(engine), None);
    app.views.status_view = Some(crate::view::StatusView::new(
        tigrs_git::StatusReport::default(),
    ));
    app.push_view(ViewKind::Status);
    assert!(app.views.main_view.is_none());

    let _ = execute_action(&mut app, &Action::OpenView(ViewKind::Main), 24);
    assert_eq!(app.active_view(), Some(ViewKind::Main));
    let mv = app.views.main_view.as_mut().expect("MainView created");
    assert_eq!(
        mv.total_commits(),
        Some(2),
        "Lazily opened MainView must initialize total_commits from fast_commit_count_estimate()"
    );

    // Populate commits and render through `render_active` (which runs `MainView::render_with_options`
    // and `apply_ui_palette_to_terminal`) with the default `MainSpotlight::Author`.
    mv.set_current_user("Alice Developer", "alice@example.com");
    let c0 = CommitSummary {
        id: ObjectId::from_bytes_or_panic(&[1_u8; 20]),
        parents: tigrs_git::ParentIds::new(),
        author_name: Arc::from("Alice Developer"),
        author_time_secs: 1_700_000_000,
        summary: Box::from("feat: highlighted commit #42"),
    };
    let c1 = CommitSummary {
        id: ObjectId::from_bytes_or_panic(&[2_u8; 20]),
        parents: tigrs_git::ParentIds::new(),
        author_name: Arc::from("Bob Reviewer"),
        author_time_secs: 1_700_000_000,
        summary: Box::from("fix: earlier commit"),
    };
    mv.append_commits(vec![c0, c1]);
    mv.set_finished();

    assert_eq!(
        app.options.main_spotlight,
        tigrs_core::MainSpotlight::Author
    );

    // 1. Default theme: selected row (row 1) must keep REVERSE across all 80 columns.
    let mut term_default = crate::headless::HeadlessTerminal::new(80, 10);
    render_active(&app, &mut term_default, 80, 10).unwrap();
    for x in 0..80 {
        let cell = term_default.cell(x, 1).unwrap();
        assert!(
            cell.attrs.contains(crate::headless::CellAttrs::REVERSE),
            "render_active (Default theme) lost REVERSE on selected row at col {x}"
        );
    }

    // 2. Dracula theme: selected row (row 1) must keep cursor_row.bg across all 80 columns.
    app.options.ui_theme = tigrs_core::UiThemeId::Dracula;
    let expected_bg = app.options.ui_palette().cursor_row.bg;
    assert!(expected_bg.is_some());
    let mut term_dracula = crate::headless::HeadlessTerminal::new(80, 10);
    render_active(&app, &mut term_dracula, 80, 10).unwrap();
    for x in 0..80 {
        let cell = term_dracula.cell(x, 1).unwrap();
        assert_eq!(
            cell.bg, expected_bg,
            "render_active (Dracula theme) lost cursor_row.bg on selected row at col {x}"
        );
    }
}
