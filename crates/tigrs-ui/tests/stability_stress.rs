// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Suite 5: Long-Running Chaos Monkey UI Stress Suite.
//!
//! Verifies:
//! 1. Deterministic Chaos Monkey UI Fuzzing (`test_chaos_monkey_ui_fuzz_3500_consecutive_actions_and_viewport_resizes`):
//!    Executes 3,500 consecutive pseudo-random navigation actions, view transitions (`Main`, `Diff`,
//!    `Status`, `Tree`, `Blob`, `Blame`, `Refs`, `Stash`, `Reflog`, `Help`), split-view layout toggles,
//!    option toggles, colon commands, and pathological viewport resizes (`1x1`, `3x2`, `15x4`, `240x80`).
//!    Proves zero panics, zero integer underflows/overflows, and valid terminal frame rendering.

use std::io::Write;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use tigrs_core::cancel::CancellationToken;
use tigrs_core::config::Config;
use tigrs_git::GitEngine;
use tigrs_git::revwalk::stream_commit_chunks_with_spec;
use tigrs_ui::app::actions::execute_action;
use tigrs_ui::app::commands::execute_parsed_command;
use tigrs_ui::app::layout::ViewKind;
use tigrs_ui::app::render::render_active;
use tigrs_ui::app::{AppState, Flow};
use tigrs_ui::headless::HeadlessTerminal;
use tigrs_ui::keymap::Action;
use tigrs_ui::prompt::ParsedCommand;
use tigrs_ui::view::{
    BlameView, BlobView, DiffView, HelpView, MainView, ReflogView, RefsView, StashView, StatusView,
    TreeView,
};

struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0xBAD_5EED } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn choose<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
        let idx = (self.next_u64() as usize) % slice.len();
        &slice[idx]
    }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Chaos Monkey")
        .env("GIT_AUTHOR_EMAIL", "chaos@linux.org")
        .env("GIT_COMMITTER_NAME", "Chaos Monkey")
        .env("GIT_COMMITTER_EMAIL", "chaos@linux.org")
        .status()
        .expect("git command failed");
    assert!(status.success(), "git {args:?} failed");
}

fn create_chaos_repository() -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    let p = dir.path();
    git(p, &["init", "-b", "main"]);
    git(p, &["config", "user.name", "Chaos Monkey"]);
    git(p, &["config", "user.email", "chaos@linux.org"]);

    for i in 1..=12 {
        std::fs::create_dir_all(p.join("src/module")).unwrap();
        std::fs::write(
            p.join("src/module/engine.rs"),
            format!("// Revision {i}\npub fn compute(x: usize) -> usize {{\n    x + {i}\n}}\n"),
        )
        .unwrap();
        std::fs::write(
            p.join("README.md"),
            format!("# Chaos Test Repo\nCommit iteration {i}\n"),
        )
        .unwrap();
        git(p, &["add", "."]);
        git(
            p,
            &[
                "commit",
                "-m",
                &format!("Commit #{i}: update engine and readme"),
            ],
        );
    }

    // Create a branch and merge commit
    git(p, &["checkout", "-b", "feature-chaos", "HEAD~3"]);
    std::fs::write(p.join("feature.txt"), "feature branch content\n").unwrap();
    git(p, &["add", "feature.txt"]);
    git(p, &["commit", "-m", "Feature branch commit"]);
    git(p, &["checkout", "main"]);
    git(
        p,
        &[
            "merge",
            "--no-ff",
            "feature-chaos",
            "-m",
            "Merge feature-chaos into main",
        ],
    );

    // Create tags and a stash
    git(p, &["tag", "v1.0.0", "HEAD~5"]);
    git(p, &["tag", "v2.0.0", "HEAD"]);
    std::fs::write(p.join("README.md"), "Stashable modification\n").unwrap();
    git(p, &["stash", "push", "-m", "chaos stash entry"]);

    // Leave working tree with staged, unstaged, and untracked files
    std::fs::write(p.join("src/module/engine.rs"), "// Staged edit\n").unwrap();
    git(p, &["add", "src/module/engine.rs"]);
    std::fs::write(p.join("README.md"), "// Unstaged edit\n").unwrap();
    std::fs::write(p.join("untracked_chaos.log"), "untracked log lines\n").unwrap();

    dir
}

#[test]
fn test_chaos_monkey_ui_fuzz_3500_consecutive_actions_and_viewport_resizes() {
    let repo_dir = create_chaos_repository();
    let engine = GitEngine::open(Some(repo_dir.path())).expect("open repo");

    // Populate initial commits
    let spec = engine.parse_rev_args(&["HEAD".to_string()]).unwrap();
    let (_src, token) = CancellationToken::new();
    let mut stream = stream_commit_chunks_with_spec(engine.repository(), spec, 100, token).unwrap();
    let mut commits = Vec::new();
    for batch in stream.by_ref() {
        commits.extend(batch.unwrap());
    }
    drop(stream);
    assert!(!commits.is_empty());

    let head_id = commits[0].id;
    let diff = engine.compute_commit_diff(head_id).expect("compute diff");
    let status_snapshot = engine
        .load_status(&CancellationToken::none())
        .expect("load status");
    let refs_list = engine.list_refs().expect("list refs");
    let stashes = engine.list_stashes().unwrap_or_default();
    let reflogs = engine.read_reflog("HEAD").unwrap_or_default();
    let tree_listing = engine.read_tree(head_id, "").expect("read tree");
    let blame_res = engine
        .blame_file(head_id, "src/module/engine.rs")
        .expect("blame file");
    let blob_content = engine
        .read_blob_at_commit_path(head_id, "README.md")
        .expect("read blob");

    let cfg = Config::default();
    let mut app = AppState::with_engine_and_config(Some(engine), Some(&cfg));

    let mut main_view = MainView::new("main".to_string());
    main_view.append_commits(commits);
    app.views.main_view = Some(main_view);
    app.views.diff_view = Some(DiffView::new(diff));
    app.views.status_view = Some(StatusView::new(status_snapshot));
    app.views.refs_view = Some(RefsView::new(refs_list));
    app.views.stash_view = Some(StashView::new(stashes));
    app.views.reflog_view = Some(ReflogView::new("HEAD".to_string(), reflogs));
    app.views.tree_view = Some(TreeView::new(tree_listing));
    app.views.blame_view = Some(BlameView::from_result(blame_res));
    app.views.blob_view = Some(BlobView::new(head_id, blob_content));
    app.views.help_view = Some(HelpView::new());
    app.views.view_stack.push(ViewKind::Main);

    let actions = [
        Action::MoveDown,
        Action::MoveUp,
        Action::MovePageDown,
        Action::MovePageUp,
        Action::MoveHalfPageDown,
        Action::MoveHalfPageUp,
        Action::MoveFirstLine,
        Action::MoveLastLine,
        Action::ScrollLeft,
        Action::ScrollRight,
        Action::OpenView(ViewKind::Main),
        Action::OpenView(ViewKind::Diff),
        Action::OpenView(ViewKind::Status),
        Action::OpenView(ViewKind::Tree),
        Action::OpenView(ViewKind::Blob),
        Action::OpenView(ViewKind::Blame),
        Action::OpenView(ViewKind::Refs),
        Action::OpenView(ViewKind::Stash),
        Action::OpenView(ViewKind::Reflog),
        Action::OpenView(ViewKind::Help),
        Action::ViewNext,
        Action::Maximize,
        Action::ViewClose,
        Action::FindNext,
        Action::FindPrev,
    ];

    let viewports: &[(u16, u16)] = &[
        (1, 1),
        (3, 2),
        (15, 4),
        (40, 10),
        (80, 24),
        (120, 40),
        (240, 80),
    ];

    let toggle_options = [
        "line-number",
        "date",
        "author",
        "committer",
        "line-graphics",
        "commit-title-graph",
        "show-changes",
        "vertical-split",
        "diff-presentation",
        "diff-layout",
        "diff-indicator",
        "word-diff",
    ];

    let colon_commands = [
        ":1",
        ":5",
        ":999999",
        ":/Commit",
        ":?Merge",
        ":goto HEAD~1",
        ":set diff-context = 2",
        ":set diff-context = full",
        ":set tab-size = 4",
        ":echo chaos_monkey_ping",
    ];

    let mut rng = XorShift64::new(0xDEAD_BEEF_CAFE_BABE);
    let mut current_vp = (80u16, 24u16);
    let mut term = HeadlessTerminal::new(current_vp.0, current_vp.1);

    for step in 0..3_500 {
        let roll = (rng.next_u64() % 100) as u8;

        if roll < 65 {
            // Execute random keymap action
            let action = rng.choose(&actions);
            let flow = execute_action(&mut app, action, current_vp.1 as usize);
            if flow == Flow::Quit || app.views.view_stack.is_empty() {
                app.views.view_stack.push(ViewKind::Main);
            }
        } else if roll < 80 {
            // Toggle random option
            let opt = rng.choose(&toggle_options);
            app.toggle_option(opt);
        } else if roll < 92 {
            // Execute colon command
            let cmd_str = rng.choose(&colon_commands);
            let parsed = ParsedCommand::parse(cmd_str);
            let flow = execute_parsed_command(&mut app, parsed, current_vp.1 as usize);
            if flow == Flow::Quit || app.views.view_stack.is_empty() {
                app.views.view_stack.push(ViewKind::Main);
            }
        } else {
            // Resize viewport to random dimension (including pathological 1x1 or 3x2)
            current_vp = *rng.choose(viewports);
            term = HeadlessTerminal::new(current_vp.0, current_vp.1);
        }

        // Render and verify invariant every 25 steps or on pathological small viewports
        if step % 25 == 0 || current_vp.0 <= 15 || current_vp.1 <= 4 {
            let mut frame_buf = Vec::with_capacity(4096);
            render_active(&app, &mut frame_buf, current_vp.0, current_vp.1)
                .expect("render_active must never fail");
            term.write_all(&frame_buf)
                .expect("HeadlessTerminal ANSI parse must never fail");

            // Ensure active view cursor is within sane bounds
            if let Some(kind) = app.active_view()
                && let Some(view) = app.view_ref(kind)
            {
                let count = view.line_count();
                let cur = view.cursor();
                assert!(
                    count == 0 || cur < count,
                    "Cursor out of bounds at step {step} in {kind:?}: cur={cur}, count={count}"
                );
            }
        }
    }
}
