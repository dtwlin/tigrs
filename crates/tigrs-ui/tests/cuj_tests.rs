// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Critical User Journey (CUJ) Key-Input Integration Tests.
//!
//! Simulates end-to-end user workflows purely at the terminal key-input level
//! (`crossterm::event::Event::Key`) against real temporary Git repositories,
//! rendering every frame into an in-memory `HeadlessTerminal` buffer.

use std::fs;
use std::path::Path;
use std::process::Command;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tempfile::TempDir;
use tigrs_core::CancellationToken;
use tigrs_git::GitEngine;
use tigrs_ui::{
    AppState, Flow, HeadlessTerminal, MainView, ViewKind, handle_event_with_dimensions,
    render_active,
};

/// End-to-end CUJ test harness that drives `AppState` through keystroke sequences
/// and inspects rendered `HeadlessTerminal` output at every step.
pub struct CujSession {
    /// Active TUI application state under test.
    pub app: AppState,
    /// In-memory 2D terminal emulator capturing rendered frames.
    pub term: HeadlessTerminal,
    /// Terminal viewport width in columns.
    pub width: u16,
    /// Terminal viewport height in rows.
    pub height: u16,
    /// Last control-flow result (`Flow::Continue` or `Flow::Quit`).
    pub last_flow: Flow,
}

impl CujSession {
    /// Launches a headless `tigrs` session against a real Git repository on disk,
    /// mirroring the startup sequence of `tigrs-cli` (`MainView` populated with
    /// commits, refs, and working-tree status changes).
    pub fn launch(repo_path: &Path, width: u16, height: u16) -> Self {
        let engine = GitEngine::open(Some(repo_path)).expect("failed to open GitEngine");
        let branch = engine
            .current_branch()
            .unwrap_or_else(|_| "HEAD".to_string());

        let mut main = MainView::new(branch);
        let (_cancel_src, token) = CancellationToken::new();
        if let Ok(iter) = engine.stream_commits(None, Some(100), token) {
            for batch in iter.flatten() {
                main.append_commits(batch);
            }
        }
        main.set_finished();

        if let Ok(refs) = engine.list_refs() {
            main.set_ref_badges(&refs);
        }

        let (_status_cancel, status_token) = CancellationToken::new();
        let changes_report = engine.load_status(&status_token).ok();

        let mut app = AppState {
            views: tigrs_ui::app::ViewManager {
                main_view: Some(main),
                view_stack: vec![ViewKind::Main],
                maximized: true,
                ..Default::default()
            },
            engine: Some(engine),
            changes_report,
            last_terminal_size: Some((width, height)),
            ..Default::default()
        };
        app.apply_changes_rows();

        let mut term = HeadlessTerminal::new(width, height);
        render_active(&app, &mut term, width, height).expect("initial render failed");

        Self {
            app,
            term,
            width,
            height,
            last_flow: Flow::Continue,
        }
    }

    /// Sends a space-separated sequence of keys or special tokens (e.g. `"j Enter O v q"`).
    ///
    /// Tokens can be:
    /// - Named keys: `Enter`, `Esc`, `Tab`, `Backspace`, `Space`, `Up`, `Down`, `Left`, `Right`,
    ///   `PageUp`, `PageDown`, `Home`, `End`
    /// - Bracketed keys inside strings: `<Enter>`, `<Esc>`, `<Tab>`, `<Backspace>`, `<Space>`, `<C-d>`, `<C-u>`
    /// - Single or multi-key characters
    pub fn press(&mut self, script: &str) -> Flow {
        for token in parse_key_script(script) {
            self.last_flow = handle_event_with_dimensions(
                &Event::Key(token),
                &mut self.app,
                self.width,
                self.height,
            );
            render_active(&self.app, &mut self.term, self.width, self.height)
                .expect("render failed after key event");
            if self.last_flow == Flow::Quit {
                break;
            }
        }
        self.last_flow
    }

    /// Simulates typing a command or search prompt string character-by-character
    /// (including spaces) and pressing `Enter` (e.g. `":set diff-layout = side-by-side"` or `"/v2"`).
    pub fn prompt(&mut self, text: &str) -> Flow {
        for c in text.chars() {
            self.last_flow = handle_event_with_dimensions(
                &Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)),
                &mut self.app,
                self.width,
                self.height,
            );
        }
        self.last_flow = handle_event_with_dimensions(
            &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            &mut self.app,
            self.width,
            self.height,
        );
        render_active(&self.app, &mut self.term, self.width, self.height)
            .expect("render failed after prompt");
        self.last_flow
    }

    /// Simulates a terminal window resize event (`Event::Resize`) and re-renders at the new dimensions.
    pub fn resize(&mut self, width: u16, height: u16) -> Flow {
        self.width = width;
        self.height = height;
        self.term = HeadlessTerminal::new(width, height);
        self.app.invalidate_screen();
        self.last_flow = handle_event_with_dimensions(
            &Event::Resize(width, height),
            &mut self.app,
            width,
            height,
        );
        render_active(&self.app, &mut self.term, width, height)
            .expect("render failed after resize");
        self.last_flow
    }

    /// Returns the full rendered screen text (rows joined by `\n`).
    pub fn screen_text(&self) -> String {
        self.term.screen_text()
    }

    /// Returns the currently focused view kind on top of the view stack.
    pub fn active_view(&self) -> Option<ViewKind> {
        self.app.active_view()
    }

    /// Asserts that the rendered terminal screen contains `expected`.
    #[track_caller]
    pub fn assert_screen_contains(&self, expected: &str) {
        let screen = self.screen_text();
        assert!(
            screen.contains(expected),
            "Expected screen to contain {expected:?}, but screen was:\n{screen}"
        );
    }

    /// Asserts that the rendered terminal screen does NOT contain `unexpected`.
    #[track_caller]
    pub fn assert_screen_not_contains(&self, unexpected: &str) {
        let screen = self.screen_text();
        assert!(
            !screen.contains(unexpected),
            "Expected screen NOT to contain {unexpected:?}, but screen was:\n{screen}"
        );
    }
}

/// Parses a key script string into a sequence of `KeyEvent`s.
fn parse_key_script(script: &str) -> Vec<KeyEvent> {
    let mut events = Vec::new();
    for word in script.split_whitespace() {
        if let Some(ev) = match_named_key(word) {
            events.push(ev);
            continue;
        }
        // Parse mixed character + `<Tag>` sequences within a token (e.g. `/Makefile<Enter>`)
        let mut chars = word.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '<' {
                let mut tag = String::new();
                while let Some(&next_c) = chars.peek() {
                    chars.next();
                    if next_c == '>' {
                        break;
                    }
                    tag.push(next_c);
                }
                if let Some(ev) = match_named_key(&tag) {
                    events.push(ev);
                }
            } else {
                events.push(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            }
        }
    }
    events
}

fn match_named_key(name: &str) -> Option<KeyEvent> {
    match name {
        "Enter" | "Return" | "CR" => Some(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        "Esc" | "Escape" => Some(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        "Tab" => Some(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        "Backspace" | "BS" => Some(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
        "Space" => Some(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
        "Up" => Some(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
        "Down" => Some(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        "Left" => Some(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
        "Right" => Some(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
        "PageUp" | "PgUp" => Some(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
        "PageDown" | "PgDn" => Some(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)),
        "Home" => Some(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)),
        "End" => Some(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
        "C-d" | "Ctrl-d" => Some(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
        "C-u" | "Ctrl-u" => Some(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
        "C-f" | "Ctrl-f" => Some(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL)),
        "C-b" | "Ctrl-b" => Some(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)),
        "C-n" | "Ctrl-n" => Some(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)),
        "C-p" | "Ctrl-p" => Some(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
        _ => None,
    }
}

/// Helper to initialize a realistic multi-commit repository for CUJ tests.
fn create_cuj_test_repo() -> TempDir {
    let dir = TempDir::new().expect("create tempdir");
    let root = dir.path();

    let run_git = |args: &[&str]| {
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Alice Dev")
            .env("GIT_AUTHOR_EMAIL", "alice@example.com")
            .env("GIT_COMMITTER_NAME", "Alice Dev")
            .env("GIT_COMMITTER_EMAIL", "alice@example.com")
            .status()
            .expect("git command failed");
        assert!(status.success(), "git {args:?} failed");
    };

    run_git(&["init", "-b", "main"]);
    run_git(&["config", "user.name", "Alice Dev"]);
    run_git(&["config", "user.email", "alice@example.com"]);

    // Commit 1: Initial project structure
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Makefile"),
        "CC = gcc\nCFLAGS = -O2\nall:\n\t$(CC) $(CFLAGS) src/main.c -o app\n",
    )
    .unwrap();
    fs::write(
        root.join("src/main.c"),
        "#include <stdio.h>\n\nint main(void) {\n    printf(\"Hello v1\\n\");\n    return 0;\n}\n",
    )
    .unwrap();
    run_git(&["add", "."]);
    run_git(&["commit", "-m", "Initial project import"]);

    // Commit 2: Feature update
    fs::write(
        root.join("src/main.c"),
        "#include <stdio.h>\n\nint main(void) {\n    printf(\"Hello v2 with features\\n\");\n    return 0;\n}\n",
    )
    .unwrap();
    run_git(&["add", "src/main.c"]);
    run_git(&["commit", "-m", "Add v2 greeting feature"]);

    // Working tree modifications for staging/status CUJ
    fs::write(
        root.join("Makefile"),
        "CC = clang\nCFLAGS = -O3 -Wall\nall:\n\t$(CC) $(CFLAGS) src/main.c -o app\n",
    )
    .unwrap();
    fs::write(root.join("README.md"), "# Sample Project\n").unwrap();

    dir
}

#[test]
fn cuj_1_history_navigation_split_diff_and_search() {
    let repo = create_cuj_test_repo();
    let mut session = CujSession::launch(repo.path(), 100, 28);

    // 1. Verify initial MainView displays commits and uncommitted changes rows
    assert_eq!(session.active_view(), Some(ViewKind::Main));
    session.assert_screen_contains("Add v2 greeting feature");
    session.assert_screen_contains("Initial project import");

    // 2. Navigate down to the commit row ("Add v2 greeting feature") and press Enter to open split DiffView
    // Note: row 0 & 1 are Unstaged/Untracked changes; pressing 'j' twice lands on HEAD commit.
    session.press("j j Enter");
    assert_eq!(session.active_view(), Some(ViewKind::Diff));
    assert!(
        !session.app.views.maximized,
        "Enter from MainView should open split view"
    );

    // Press 'O' (Maximize) so the full diff hunk is visible on screen
    session.press("O");
    assert!(
        session.app.views.maximized,
        "'O' should maximize the diff view"
    );
    session.assert_screen_contains("Hello v2 with features");

    // 3. Search inside DiffView for 'v2' via '/v2' prompt
    session.prompt("/v2");
    assert_eq!(
        session.app.active_search.as_ref().map(|s| s.pattern.raw()),
        Some("v2")
    );

    // 4. Toggle Side-by-Side DiffView via 'v' shortcut key (Action::ToggleDiffLayout)
    session.press("v");
    session.assert_screen_contains("Hello v1");
    session.assert_screen_contains("Hello v2 with features");

    // And toggle back via prompt ':set diff-layout = unified'
    session.prompt(":set diff-layout = unified");
    assert_eq!(
        session.app.options.diff_layout,
        tigrs_ui::DiffLayout::Unified
    );

    // 5. Close DiffView with 'q' to return to MainView, then 'q' to exit app
    let flow = session.press("q");
    assert_eq!(flow, Flow::Continue);
    assert_eq!(session.active_view(), Some(ViewKind::Main));

    let exit_flow = session.press("q");
    assert_eq!(exit_flow, Flow::Quit);
}

#[test]
fn cuj_2_status_view_interactive_staging_and_unstaging() {
    let repo = create_cuj_test_repo();
    let mut session = CujSession::launch(repo.path(), 100, 28);

    // 1. User presses 's' to open StatusView
    session.press("s");
    assert_eq!(session.active_view(), Some(ViewKind::Status));
    session.assert_screen_contains("Changes not staged for commit");
    session.assert_screen_contains("Makefile");
    session.assert_screen_contains("Untracked files");
    session.assert_screen_contains("README.md");

    // 2. Navigate with 'j' until 'Makefile' under "Changes not staged for commit" is selected, then press 'u' to stage it
    for _ in 0..10 {
        if session
            .app
            .views
            .status_view
            .as_ref()
            .and_then(|s| s.selected_item())
            .map(|item| item.path.as_str())
            == Some("Makefile")
        {
            break;
        }
        session.press("j");
    }
    // When Read-Only mode is enabled (`:read-only`), pressing 'u' displays the read-only status bar warning
    session.prompt(":read-only");
    session.press("u");
    session.assert_screen_contains("Read-only mode: repository modifications are disabled");

    // Unlock via command prompt (`:set read-only = false`) and press 'u' to stage
    session.prompt(":set read-only = false");
    session.press("u");

    // 3. Verify 'Makefile' is now staged in the real Git index
    let (_src, token) = CancellationToken::new();
    let status = session
        .app
        .engine
        .as_ref()
        .unwrap()
        .load_status(&token)
        .unwrap();
    assert!(
        status.staged.iter().any(|item| item.path == "Makefile"),
        "Makefile should be staged in the Git index after pressing 'u'"
    );

    // 4. Jump to top of StatusView with 'Home' and navigate to 'Makefile' in Staged section, then press 'u' to unstage
    session.press("Home");
    for _ in 0..10 {
        if session
            .app
            .views
            .status_view
            .as_ref()
            .and_then(|s| s.selected_item())
            .map(|item| item.path.as_str())
            == Some("Makefile")
        {
            break;
        }
        session.press("j");
    }
    session.press("u");

    let status_after = session
        .app
        .engine
        .as_ref()
        .unwrap()
        .load_status(&token)
        .unwrap();
    assert!(
        status_after.staged.is_empty(),
        "Staged list should be empty after unstaging Makefile with 'u'"
    );
    assert!(
        status_after
            .unstaged
            .iter()
            .any(|item| item.path == "Makefile"),
        "Makefile should return to unstaged after pressing 'u' again"
    );
}

#[test]
fn cuj_3_code_archeology_tree_blob_blame_and_parent_navigation() {
    use tigrs_ui::TreeRow;

    let repo = create_cuj_test_repo();
    let mut session = CujSession::launch(repo.path(), 100, 28);

    // 1. User presses 't' to open TreeView at HEAD
    session.press("t");
    assert_eq!(session.active_view(), Some(ViewKind::Tree));
    session.assert_screen_contains("src");
    session.assert_screen_contains("Makefile");

    // 2. Navigate with 'j' to 'src' directory and press Enter
    for _ in 0..10 {
        let is_src = matches!(
            session.app.views.tree_view.as_ref().and_then(|t| t.selected_row()),
            Some(TreeRow::Entry(e)) if e.name == "src"
        );
        if is_src {
            break;
        }
        session.press("j");
    }
    session.press("Enter");
    session.assert_screen_contains("main.c");

    // 3. Navigate with 'j' to 'main.c' and press Enter to open BlobView
    for _ in 0..10 {
        let is_main_c = matches!(
            session.app.views.tree_view.as_ref().and_then(|t| t.selected_row()),
            Some(TreeRow::Entry(e)) if e.name == "main.c"
        );
        if is_main_c {
            break;
        }
        session.press("j");
    }
    session.press("Enter");
    assert_eq!(session.active_view(), Some(ViewKind::Blob));
    session.assert_screen_contains("printf(\"Hello v2 with features\\n\");");

    // 4. Press 'b' to open BlameView from BlobView
    session.press("b");
    assert_eq!(session.active_view(), Some(ViewKind::Blame));
    session.assert_screen_contains("Alice Dev");

    // 5. Move cursor down to line 4 (modified in commit 2) and press ',' (ParentBlame)
    // to inspect the line's state in the parent commit
    session.press("j j j ,");
    session.assert_screen_contains("Hello v1");

    // 6. Press '<Backspace>' (or ':back<Enter>') to pop blame history back to v2
    session.press(":back<Enter>");
    session.assert_screen_contains("Hello v2 with features");
}

#[test]
fn cuj_4_log_refs_stash_and_options_menu() {
    let repo = create_cuj_test_repo();
    let root = repo.path();

    // Add a tag and a stash entry so RefsView and StashView have rich content
    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(root)
        .args(["tag", "v2.0.0"])
        .status()
        .unwrap();
    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(root)
        .args(["stash", "push", "-m", "WIP: experimental clang flags"])
        .status()
        .unwrap();

    let mut session = CujSession::launch(root, 100, 28);

    // 1. Press 'l' to switch to LogView (rich commit log with diffstat summary)
    session.press("l");
    assert_eq!(session.active_view(), Some(ViewKind::Log));
    session.assert_screen_contains("Add v2 greeting feature");
    session.assert_screen_contains("src/main.c");

    // 2. Press 'r' to open RefsView and inspect branches & tags
    session.press("r");
    assert_eq!(session.active_view(), Some(ViewKind::Refs));
    session.assert_screen_contains("main");
    session.assert_screen_contains("v2.0.0");

    // 3. Press 'y' to open StashView and verify the stashed WIP entry is listed
    session.press("y");
    assert_eq!(session.active_view(), Some(ViewKind::Stash));
    session.assert_screen_contains("WIP: experimental clang flags");

    // 4. Press 'o' to open the interactive Options menu prompt and 'Esc' to close it
    let initial_lineno = session.app.options.line_number;
    session.press("o");
    assert!(
        session.app.prompt.is_some(),
        "Pressing 'o' should open the interactive options menu prompt"
    );
    session.press("Esc");
    assert!(
        session.app.prompt.is_none(),
        "Pressing 'Esc' should dismiss the options menu prompt"
    );

    // Press '#' shortcut key to toggle line numbers
    session.press("#");
    assert_ne!(
        session.app.options.line_number, initial_lineno,
        "Pressing '#' should toggle line_number"
    );
}

#[test]
fn cuj_5_split_pane_parent_stepping_and_focus_cycling_stress_walk() {
    let repo = create_cuj_test_repo();
    let mut session = CujSession::launch(repo.path(), 110, 32);

    // 1. Move down to commit 2 ("Add v2 greeting feature") and open split DiffView
    session.press("j j Enter");
    assert_eq!(session.active_view(), Some(ViewKind::Diff));
    assert!(!session.app.views.maximized);

    let commit_v2_id = session.app.views.diff_view.as_ref().unwrap().commit_id();

    // 2. While focused inside DiffView, press 'J' (Action::Next) to step parent MainView down to Commit 1
    session.press("J");
    let commit_v1_id = session.app.views.diff_view.as_ref().unwrap().commit_id();
    assert_ne!(
        commit_v2_id, commit_v1_id,
        "Pressing 'J' inside child DiffView should advance parent MainView cursor and sync DiffView"
    );

    // 3. Press 'K' (Action::Previous) to step back up to Commit 2
    session.press("K");
    assert_eq!(
        session.app.views.diff_view.as_ref().unwrap().commit_id(),
        commit_v2_id,
        "Pressing 'K' should step back to Commit 2"
    );

    // 4. Press 'Tab' (Action::ViewNext) to cycle focus between MainView and DiffView in split mode
    session.press("Tab");
    assert_eq!(
        session.active_view(),
        Some(ViewKind::Main),
        "'Tab' should switch focus back to MainView while keeping split open"
    );
    session.press("Tab");
    assert_eq!(
        session.active_view(),
        Some(ViewKind::Diff),
        "Second 'Tab' should return focus to DiffView"
    );

    // 5. Toggle vertical vs horizontal split layout via prompt
    session.prompt(":toggle vertical-split");
    session.assert_screen_contains("Add v2 greeting feature");
    session.prompt(":toggle vertical-split");
    session.assert_screen_contains("Add v2 greeting feature");
}

#[test]
fn cuj_6_granular_hunk_and_single_line_staging_and_reverting_walk() {
    let repo = create_cuj_test_repo();
    let root = repo.path();

    // Create a multi-hunk file modification in src/main.c
    fs::write(
        root.join("src/main.c"),
        "#include <stdio.h>\n#include <stdlib.h>\n\nint main(void) {\n    printf(\"Hello v2 with features\\n\");\n    printf(\"Extra line 1\\n\");\n    printf(\"Extra line 2\\n\");\n    return 0;\n}\n",
    )
    .unwrap();

    let mut session = CujSession::launch(root, 100, 30);

    // 1. Open StatusView ('s'), select 'src/main.c', and press Enter to view working-tree diff
    session.press("s");
    for _ in 0..10 {
        if session
            .app
            .views
            .status_view
            .as_ref()
            .and_then(|s| s.selected_item())
            .map(|item| item.path.as_str())
            == Some("src/main.c")
        {
            break;
        }
        session.press("j");
    }
    session.press("Enter O");
    assert_eq!(session.active_view(), Some(ViewKind::Diff));
    session.assert_screen_contains("+    printf(\"Extra line 1\\n\");");

    // 2. Jump to first hunk with '@' and move cursor onto an added line ('+    printf("Extra line 1\n");')
    session.press("@");
    for _ in 0..15 {
        if let Some((_, hunk, idx)) = session
            .app
            .views
            .diff_view
            .as_ref()
            .and_then(|d| d.selected_line())
            && hunk
                .lines
                .get(idx)
                .is_some_and(|l| l.content.contains("Extra line 1"))
        {
            break;
        }
        session.press("j");
    }

    // 3. When Read-Only mode is enabled (`:read-only`), pressing '1' is blocked and displays the read-only warning;
    //    unlock via `:update-mode` and press '1' to stage ONLY that single line
    session.prompt(":read-only");
    session.press("1");
    session.assert_screen_contains("Read-only mode: repository modifications are disabled");
    session.prompt(":update-mode");
    session.press("1");

    // Verify partial staging: src/main.c should appear in BOTH staged AND unstaged lists!
    let (_src, token) = CancellationToken::new();
    let status = session
        .app
        .engine
        .as_ref()
        .unwrap()
        .load_status(&token)
        .unwrap();
    assert!(
        status.staged.iter().any(|item| item.path == "src/main.c"),
        "src/main.c should have staged changes after single-line stage ('1')"
    );
    assert!(
        status.unstaged.iter().any(|item| item.path == "src/main.c"),
        "src/main.c should still have remaining unstaged changes after single-line stage ('1')"
    );

    // 4. Stage remaining hunk lines with 'u' (Action::StatusUpdate)
    session.press("@ u");
    let status_full = session
        .app
        .engine
        .as_ref()
        .unwrap()
        .load_status(&token)
        .unwrap();
    assert!(
        !status_full
            .unstaged
            .iter()
            .any(|item| item.path == "src/main.c"),
        "All changes in src/main.c should be staged after pressing 'u' on remaining hunk"
    );
}

#[test]
fn cuj_7_deep_view_stack_crisscross_and_unwind_stress_walk() {
    let repo = create_cuj_test_repo();
    let mut session = CujSession::launch(repo.path(), 100, 28);

    // Build a deep view stack across 8 distinct view transitions:
    // Main -> Log ('l') -> Diff ('d') -> Tree ('t') -> Blob ('Enter' on file) -> Blame ('b') -> Refs ('r') -> Reflog ('L') -> Help ('h')
    assert_eq!(session.active_view(), Some(ViewKind::Main));

    session.press("l");
    assert_eq!(session.active_view(), Some(ViewKind::Log));

    session.press("d");
    assert_eq!(session.active_view(), Some(ViewKind::Diff));

    session.press("t");
    assert_eq!(session.active_view(), Some(ViewKind::Tree));

    // Navigate to 'Makefile' in TreeView and press Enter to open BlobView
    for _ in 0..10 {
        let is_makefile = matches!(
            session.app.views.tree_view.as_ref().and_then(|t| t.selected_row()),
            Some(tigrs_ui::TreeRow::Entry(e)) if e.name == "Makefile"
        );
        if is_makefile {
            break;
        }
        session.press("j");
    }
    session.press("Enter");
    assert_eq!(session.active_view(), Some(ViewKind::Blob));

    session.press("b");
    assert_eq!(session.active_view(), Some(ViewKind::Blame));

    session.press("r");
    assert_eq!(session.active_view(), Some(ViewKind::Refs));

    session.press("L");
    assert_eq!(session.active_view(), Some(ViewKind::Reflog));

    session.press("h");
    assert_eq!(session.active_view(), Some(ViewKind::Help));
    session.assert_screen_contains("View Switching");

    // Now unwind step-by-step with 'q' (ViewClose) and verify LIFO stack order
    session.press("q");
    assert_eq!(session.active_view(), Some(ViewKind::Reflog));

    session.press("q");
    assert_eq!(session.active_view(), Some(ViewKind::Refs));

    session.press("q");
    assert_eq!(session.active_view(), Some(ViewKind::Blame));

    session.press("q");
    assert_eq!(session.active_view(), Some(ViewKind::Blob));

    session.press("q");
    assert_eq!(session.active_view(), Some(ViewKind::Tree));

    session.press("q");
    assert_eq!(session.active_view(), Some(ViewKind::Diff));

    session.press("q");
    assert_eq!(session.active_view(), Some(ViewKind::Log));

    session.press("q");
    assert_eq!(session.active_view(), Some(ViewKind::Main));

    // Final 'Q' (Action::Quit) exits immediately from any state
    let exit_flow = session.press("Q");
    assert_eq!(exit_flow, Flow::Quit);
}

#[test]
fn cuj_8_grep_search_to_blob_blame_and_editor_invocation_walk() {
    let repo = create_cuj_test_repo();
    let mut session = CujSession::launch(repo.path(), 100, 28);

    // 1. User presses 'g' to open the ':grep ' prompt and searches for "printf"
    session.press("g");
    assert!(session.app.prompt.is_some(), "'g' should open grep prompt");
    session.prompt("printf");

    // 2. Verify we land in GrepView with matches displayed
    assert_eq!(session.active_view(), Some(ViewKind::Grep));
    session.assert_screen_contains("src/main.c");
    session.assert_screen_contains("printf");

    // 3. Press 'Enter' on the grep match to open BlobView positioned at that exact line
    session.press("Enter");
    assert_eq!(session.active_view(), Some(ViewKind::Blob));
    session.assert_screen_contains("printf(\"Hello v2 with features\\n\");");

    // 4. When Read-Only mode is enabled (`:read-only`), pressing 'e' (Action::Edit) is blocked and displays the warning;
    //    unlock via `:update-mode` and press 'e' to invoke $EDITOR at the current cursor line
    session.prompt(":read-only");
    session.press("e");
    session.assert_screen_contains("Read-only mode: repository modifications are disabled");
    session.prompt(":update-mode");
    session.press("e");
    let editor_req = session
        .app
        .pending_editor
        .as_ref()
        .expect("Pressing 'e' in BlobView should populate pending_editor handover");
    assert!(
        editor_req.target.path.ends_with("src/main.c"),
        "Editor target path should point to src/main.c, got {:?}",
        editor_req.target.path
    );
    assert_eq!(
        editor_req.target.line,
        Some(4),
        "Editor target line should match the grep match line (line 4)"
    );
}

#[test]
fn cuj_9_multi_parent_commit_history_and_deep_blame_stack_walk() {
    let repo = create_cuj_test_repo();
    let root = repo.path();

    // Add Commit 3 so we have a 3-commit chain (v1 -> v2 -> v3) on src/main.c
    fs::write(
        root.join("src/main.c"),
        "#include <stdio.h>\n\nint main(void) {\n    printf(\"Hello v3 ultra edition\\n\");\n    return 0;\n}\n",
    )
    .unwrap();
    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(root)
        .args(["add", "src/main.c"])
        .status()
        .unwrap();
    Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(root)
        .args(["commit", "-m", "Upgrade greeting to v3"])
        .env("GIT_AUTHOR_NAME", "Alice Dev")
        .env("GIT_AUTHOR_EMAIL", "alice@example.com")
        .env("GIT_COMMITTER_NAME", "Alice Dev")
        .env("GIT_COMMITTER_EMAIL", "alice@example.com")
        .status()
        .unwrap();

    let mut session = CujSession::launch(root, 100, 28);

    // 1. Open TreeView ('t') -> 'src' -> 'main.c' -> BlameView ('b')
    session.press("t");
    for _ in 0..10 {
        if matches!(
            session.app.views.tree_view.as_ref().and_then(|t| t.selected_row()),
            Some(tigrs_ui::TreeRow::Entry(e)) if e.name == "src"
        ) {
            break;
        }
        session.press("j");
    }
    session.press("Enter");
    for _ in 0..10 {
        if matches!(
            session.app.views.tree_view.as_ref().and_then(|t| t.selected_row()),
            Some(tigrs_ui::TreeRow::Entry(e)) if e.name == "main.c"
        ) {
            break;
        }
        session.press("j");
    }
    session.press("b");
    assert_eq!(session.active_view(), Some(ViewKind::Blame));
    session.assert_screen_contains("Hello v3 ultra edition");

    // 2. Move cursor to line 4 and press ',' (ParentBlame) -> should step from v3 to v2
    session.press("j j j ,");
    session.assert_screen_contains("Hello v2 with features");

    // 3. Press ',' (ParentBlame) again -> should step from v2 to v1
    session.press(",");
    session.assert_screen_contains("Hello v1");

    // 4. Press 'Backspace' (Action::Back) twice -> should walk forward v1 -> v2 -> v3 in LIFO order!
    session.press("Backspace");
    session.assert_screen_contains("Hello v2 with features");

    session.press("Backspace");
    session.assert_screen_contains("Hello v3 ultra edition");
}

#[test]
fn cuj_10_rapid_toggle_and_viewport_resize_stress_walk() {
    let repo = create_cuj_test_repo();
    let mut session = CujSession::launch(repo.path(), 120, 40);

    // 1. Open split DiffView on HEAD commit
    session.press("j j Enter");
    assert_eq!(session.active_view(), Some(ViewKind::Diff));

    // 2. Rapidly exercise display option toggles via single-key shortcuts:
    // '#' (lineno), 'D' (date), 'A' (author), '~' (graphics), 'w' (word-diff),
    // 'W' (ignore-space), 'X' (id), '$' (title-overflow), 'v' (layout), 'S' (syntax), 'T' (theme)
    session.press("# D A ~ w W X $ v S T");
    assert_eq!(
        session.app.options.diff_layout,
        tigrs_ui::DiffLayout::SideBySide
    );

    // 3. Stress-test dynamic terminal resizing down to ultra-compact (34x10) and ultrawide (180x50)
    // interleaved with viewport scrolling motions
    session.resize(34, 10);
    session.press("PageDown PageUp C-d C-u Home End Left Right |");

    session.resize(180, 50);
    session.press("v # D A ~ w W");
    session.assert_screen_contains("Add v2 greeting feature");

    // 4. Return to MainView and stress-scroll across commits
    session.press("q Home End j k");
    assert_eq!(session.active_view(), Some(ViewKind::Main));
}
