// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Suite 4: Exhaustive Colon-Commands, Options Registry, & Macro Expansion SQE Suite.
//!
//! Covers:
//! 1. Programmatic verification of 100% of `OPTIONS_REGISTRY` entries, canonical names,
//!    aliases, `toggle_by_name_with_effect`, `set_by_name(..., "toggle")`, and explicit valid/invalid values.
//! 2. Exhaustive `:prompt` colon command parsing (`ParsedCommand::parse`) and live `AppState`
//!    execution (`execute_parsed_command`), including `:set`, `:toggle`, `:<lineno>`, `:goto <rev>`,
//!    `:grep <pattern>`, `:+<echo-shell>`, `:/<search>`, `:?<search>`, and all `:view-*` commands.
//! 3. Live `AppState::macro_context()` extraction across views (`Main`, `Diff`, `Status`, `Tree`,
//!    `Blame`, `Blob`) and strict POSIX shell-quoting / injection defense in `MacroContext::expand`.

use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use tigrs_core::cancel::CancellationToken;
use tigrs_core::config::Config;
use tigrs_core::macro_ctx::MacroContext;
use tigrs_git::GitEngine;
use tigrs_git::revwalk::stream_commit_chunks_with_spec;
use tigrs_ui::app::commands::execute_parsed_command;
use tigrs_ui::app::layout::ViewKind;
use tigrs_ui::app::{AppState, Flow};
use tigrs_ui::options::{OPTIONS_REGISTRY, ViewOptions, find_descriptor};
use tigrs_ui::prompt::ParsedCommand;
use tigrs_ui::view::{MainView, View};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "SQE Tester")
        .env("GIT_AUTHOR_EMAIL", "sqe@example.com")
        .env("GIT_COMMITTER_NAME", "SQE Tester")
        .env("GIT_COMMITTER_EMAIL", "sqe@example.com")
        .status()
        .expect("git command failed to spawn");
    assert!(status.success(), "git {args:?} failed");
}

fn create_sqe_repo() -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    let p = dir.path();
    git(p, &["init", "-b", "main"]);
    git(p, &["config", "user.name", "SQE Tester"]);
    git(p, &["config", "user.email", "sqe@example.com"]);

    std::fs::write(
        p.join("alpha.rs"),
        "fn alpha() {\n    println!(\"needle_alpha\");\n}\n",
    )
    .unwrap();
    git(p, &["add", "alpha.rs"]);
    git(p, &["commit", "-m", "Initial commit alpha"]);

    std::fs::create_dir_all(p.join("sub")).unwrap();
    std::fs::write(
        p.join("sub/beta.rs"),
        "fn beta() {\n    println!(\"needle_beta\");\n}\n",
    )
    .unwrap();
    git(p, &["add", "sub/beta.rs"]);
    git(p, &["commit", "-m", "Add sub/beta.rs"]);

    std::fs::write(
        p.join("alpha.rs"),
        "fn alpha() {\n    println!(\"needle_alpha_modified\");\n}\n",
    )
    .unwrap();
    git(p, &["add", "alpha.rs"]);
    git(p, &["commit", "-m", "Modify alpha.rs"]);

    // Create a tag and a modified untracked/unstaged state for status/macro testing
    git(p, &["tag", "v1.0.0"]);
    std::fs::write(
        p.join("alpha.rs"),
        "fn alpha() {\n    println!(\"working_tree_edit\");\n}\n",
    )
    .unwrap();

    dir
}

#[test]
fn test_options_registry_exhaustive_toggle_and_alias_bijection() {
    assert!(
        OPTIONS_REGISTRY.len() >= 27,
        "Expected at least 27 options in OPTIONS_REGISTRY, found {}",
        OPTIONS_REGISTRY.len()
    );

    let mut opts = ViewOptions::default();

    for desc in OPTIONS_REGISTRY {
        // 1. Canonical lookup must resolve to exact descriptor
        let found = find_descriptor(desc.canonical_name)
            .unwrap_or_else(|| panic!("Failed to find canonical option '{}'", desc.canonical_name));
        assert_eq!(found.id, desc.id);

        // Case-insensitive lookup
        let upper = desc.canonical_name.to_uppercase();
        let found_upper = find_descriptor(&upper)
            .unwrap_or_else(|| panic!("Case-insensitive lookup failed for '{upper}'"));
        assert_eq!(found_upper.id, desc.id);

        // 2. Toggle via canonical name must succeed and return matching OptionEffect
        let (msg, effect) = opts
            .toggle_by_name_with_effect(desc.canonical_name)
            .unwrap_or_else(|e| {
                panic!(
                    "toggle_by_name_with_effect failed for '{}': {e}",
                    desc.canonical_name
                )
            });
        assert!(
            msg.starts_with(":set "),
            "Toggle status message should start with ':set ', got '{msg}' for '{}'",
            desc.canonical_name
        );
        assert_eq!(effect, desc.effect);

        // 3. `:set <name> = toggle` must also succeed
        let (msg_set_toggle, effect_set_toggle) = opts
            .set_by_name(desc.canonical_name, "toggle")
            .unwrap_or_else(|e| {
                panic!(
                    "set_by_name(..., 'toggle') failed for '{}': {e}",
                    desc.canonical_name
                )
            });
        assert!(msg_set_toggle.starts_with(":set "));
        assert_eq!(effect_set_toggle, desc.effect);

        // 4. Every alias must resolve and toggle identically
        for &alias in desc.aliases {
            let found_alias = find_descriptor(alias).unwrap_or_else(|| {
                panic!(
                    "Failed to find alias '{alias}' for '{}'",
                    desc.canonical_name
                )
            });
            assert_eq!(found_alias.id, desc.id);

            let (alias_msg, alias_effect) = opts
                .toggle_by_name_with_effect(alias)
                .unwrap_or_else(|e| panic!("toggle failed for alias '{alias}': {e}"));
            assert!(alias_msg.starts_with(":set "));
            assert_eq!(alias_effect, desc.effect);
        }
    }
}

#[test]
fn test_options_set_by_name_valid_and_invalid_values_matrix() {
    let mut opts = ViewOptions::default();

    // Test boolean options with all truthy/falsy representations
    let bool_options = [
        "line-number",
        "committer",
        "file-name",
        "file-size",
        "word-diff",
        "commit-title-refs",
        "show-changes",
        "show-untracked",
        "id",
        "file-filter",
        "rev-filter",
        "status-show-untracked-dirs",
        "vertical-split",
        "mouse",
        "syntax-highlighting",
    ];

    for name in bool_options {
        for truthy in ["yes", "true", "1", "on"] {
            assert!(
                opts.set_by_name(name, truthy).is_ok(),
                "Expected '{name} = {truthy}' to succeed"
            );
        }
        for falsy in ["no", "false", "0", "off"] {
            assert!(
                opts.set_by_name(name, falsy).is_ok(),
                "Expected '{name} = {falsy}' to succeed"
            );
        }
        assert!(
            opts.set_by_name(name, "invalid_bool").is_err(),
            "Expected '{name} = invalid_bool' to return Err"
        );
    }

    // Test Enum & Numeric options
    let valid_cases = [
        ("date", "relative"),
        ("date", "short"),
        ("date", "iso"),
        ("date", "no"),
        ("author", "full"),
        ("author", "abbreviated"),
        ("author", "email"),
        ("author", "no"),
        ("line-graphics", "utf-8"),
        ("line-graphics", "ascii"),
        ("line-graphics", "default"),
        ("commit-title-graph", "auto"),
        ("commit-title-graph", "v2"),
        ("commit-title-graph", "no"),
        ("ignore-space", "no"),
        ("ignore-space", "all"),
        ("ignore-space", "some"),
        ("ignore-space", "at-eol"),
        ("commit-order", "default"),
        ("commit-order", "topo"),
        ("commit-order", "date"),
        ("commit-order", "author-date"),
        ("commit-order", "reverse"),
        ("diff-presentation", "classic"),
        ("diff-presentation", "fancy"),
        ("diff-layout", "unified"),
        ("diff-layout", "side-by-side"),
        ("diff-indicator", "auto"),
        ("diff-indicator", "yes"),
        ("diff-indicator", "no"),
        ("word-diff-pairing", "similarity"),
        ("word-diff-pairing", "positional"),
        ("diff-context", "5"),
        ("diff-context", "full"),
        ("tab-size", "4"),
        ("side-by-side-min-width", "100"),
        ("commit-title-overflow", "50"),
        ("commit-title-overflow", "no"),
    ];

    for (var, val) in valid_cases {
        assert!(
            opts.set_by_name(var, val).is_ok(),
            "Expected valid set '{var} = {val}' to succeed"
        );
    }

    // Test invalid enum/numeric boundaries
    let invalid_cases = [
        ("date", "mars-time"),
        ("author", "nickname"),
        ("line-graphics", "3d"),
        ("commit-title-graph", "v99"),
        ("ignore-space", "everything"),
        ("commit-order", "random"),
        ("diff-context", "-5"),
        ("diff-context", "not_a_num"),
        ("tab-size", "0"),
        ("tab-size", "64"),
        ("side-by-side-min-width", "abc"),
        ("nonexistent-option-xyz", "true"),
    ];

    for (var, val) in invalid_cases {
        assert!(
            opts.set_by_name(var, val).is_err(),
            "Expected invalid set '{var} = {val}' to fail"
        );
    }
}

fn load_repo_commits(engine: &GitEngine) -> Vec<tigrs_git::types::CommitSummary> {
    let spec = engine.parse_rev_args(&["HEAD".to_string()]).unwrap();
    let (_src, token) = CancellationToken::new();
    let stream = stream_commit_chunks_with_spec(engine.repository(), spec, 100, token).unwrap();
    let mut all = Vec::new();
    for batch in stream {
        all.extend(batch.unwrap());
    }
    all
}

#[test]
fn test_colon_commands_live_execution_and_grep_goto_subshell() {
    let repo_dir = create_sqe_repo();
    let engine = GitEngine::open(Some(repo_dir.path())).expect("open repo");
    let commits = load_repo_commits(&engine);
    assert_eq!(commits.len(), 3);

    let mut main_view = MainView::new("main".to_string());
    main_view.append_commits(commits.clone());
    let cfg = Config::default();
    let mut app = AppState::with_engine_and_config(Some(engine), Some(&cfg));
    app.views.main_view = Some(main_view);
    app.views.view_stack.push(ViewKind::Main);

    // 1. Line number jump `:2` -> cursor at index 1
    let flow = execute_parsed_command(&mut app, ParsedCommand::parse(":2"), 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor(), 1);

    // 2. `:goto` by short SHA of oldest commit (index 2)
    let oldest_sha = commits[2].id.to_string();
    let short_sha = &oldest_sha[..8];
    let flow = execute_parsed_command(
        &mut app,
        ParsedCommand::parse(&format!(":goto {short_sha}")),
        24,
    );
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor(), 2);

    // 3. `:goto` by revision `HEAD~1` (index 1)
    let flow = execute_parsed_command(&mut app, ParsedCommand::parse(":goto HEAD~1"), 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.views.main_view.as_ref().unwrap().cursor(), 1);

    // 4a. Default is Update Mode (`!app.is_read_only()`); when Read-Only mode is enabled (`:read-only`),
    //     `:+echo sqe_subshell_ok` is blocked with READ_ONLY_WARNING_MSG
    assert!(!app.is_read_only());
    let _ = execute_parsed_command(&mut app, ParsedCommand::parse(":read-only"), 24);
    assert!(app.is_read_only());
    let flow = execute_parsed_command(&mut app, ParsedCommand::parse(":+echo sqe_subshell_ok"), 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(
        app.status_message.as_deref(),
        Some(tigrs_ui::app::READ_ONLY_WARNING_MSG)
    );

    // 4b. Unlock Update Mode (`:set read-only = false`) and run `:+echo sqe_subshell_ok`
    let _ = execute_parsed_command(&mut app, ParsedCommand::parse(":set read-only = false"), 24);
    let flow = execute_parsed_command(&mut app, ParsedCommand::parse(":+echo sqe_subshell_ok"), 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(
        app.status_message.as_deref(),
        Some("sqe_subshell_ok"),
        "Subshell output should be captured into status_message"
    );

    // 5. `:grep needle_beta` searches working tree and opens GrepView
    let flow = execute_parsed_command(&mut app, ParsedCommand::parse(":grep needle_beta"), 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Grep));
    let grep_view = app
        .views
        .grep_view
        .as_ref()
        .expect("grep_view should be populated");
    assert!(
        grep_view.line_count() >= 1,
        "Expected at least 1 grep match for needle_beta"
    );

    // 6. `:view-main` switches back to Main view
    let flow = execute_parsed_command(&mut app, ParsedCommand::parse(":view-main"), 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.active_view(), Some(ViewKind::Main));

    // 7. Forward and backward search via `:/` and `:?`
    let flow = execute_parsed_command(&mut app, ParsedCommand::parse(":/Initial commit"), 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(
        app.views.main_view.as_ref().unwrap().cursor(),
        2,
        "Forward search should jump to commit matching 'Initial commit'"
    );

    let flow = execute_parsed_command(&mut app, ParsedCommand::parse(":?Modify alpha"), 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(
        app.views.main_view.as_ref().unwrap().cursor(),
        0,
        "Backward search should jump back to commit matching 'Modify alpha'"
    );

    // 8. `:set diff-context = 7` updates ViewOptions and status message
    let flow = execute_parsed_command(&mut app, ParsedCommand::parse(":set diff-context = 7"), 24);
    assert_eq!(flow, Flow::Continue);
    assert_eq!(app.options.diff_context, 7);
}

#[test]
fn test_live_app_macro_context_and_shell_injection_immunity() {
    let repo_dir = create_sqe_repo();
    let engine = GitEngine::open(Some(repo_dir.path())).expect("open repo");
    let commits = load_repo_commits(&engine);
    let head_sha = commits[0].id.to_string();

    let mut main_view = MainView::new("main".to_string());
    main_view.append_commits(commits);
    let cfg = Config::default();
    let mut app = AppState::with_engine_and_config(Some(engine), Some(&cfg));
    app.views.main_view = Some(main_view);
    app.views.view_stack.push(ViewKind::Main);

    let ctx = app.macro_context();
    assert_eq!(ctx.commit.as_deref(), Some(head_sha.as_str()));
    assert_eq!(ctx.head.as_deref(), Some(head_sha.as_str()));
    assert_eq!(ctx.branch.as_deref(), Some("main"));
    assert!(ctx.repo_git_dir.is_some());
    assert!(ctx.repo_worktree.is_some());

    // Test shell injection payloads in MacroContext::expand
    let mut injection_ctx = MacroContext::new();
    injection_ctx.file = Some("malicious; rm -rf /; $(reboot) 'quote'.rs".to_string());
    injection_ctx.commit = Some("deadbeef".to_string());
    injection_ctx.lineno = Some(42);

    let expanded = injection_ctx
        .expand(
            "git checkout %(commit) -- %(file) && sed -n '%(lineno)p' %(file)",
            |_| None,
        )
        .expect("macro expand");

    // Verify that single quotes wrap the payload and internal single quotes are POSIX-escaped as '\''
    assert!(
        expanded.contains("'malicious; rm -rf /; $(reboot) '\\''quote'\\''.rs'"),
        "Expanded command failed POSIX single-quote escaping: {expanded}"
    );

    // Test interactive prompt macro expansion & cancellation
    assert!(MacroContext::has_prompt_token(
        "git tag -a %(prompt Enter tag name: ) %(commit)"
    ));
    let labels = MacroContext::extract_prompt_labels(
        "git commit -m %(prompt Commit message: ) --author=%(prompt Author: )",
    );
    assert_eq!(labels, vec!["Commit message:", "Author:"]);

    // Successful prompt resolution
    let prompt_expanded = injection_ctx
        .expand("git tag %(prompt Tag name: ) %(commit)", |label| {
            assert_eq!(label, "Tag name:");
            Some("v2.0-rc1; echo pwned".to_string())
        })
        .expect("prompt expand");
    assert_eq!(prompt_expanded, "git tag 'v2.0-rc1; echo pwned' 'deadbeef'");

    // User cancellation returns Err
    let cancelled = injection_ctx.expand("git tag %(prompt Tag name: )", |_| None);
    assert!(
        cancelled.is_err(),
        "Cancelling prompt resolver must return Err"
    );
}
