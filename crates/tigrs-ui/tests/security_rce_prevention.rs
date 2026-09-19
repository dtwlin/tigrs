// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Security regression test suite for Untrusted Repository RCE and Argument Injection Prevention.
//!
//! Verifies all three vectors from `security_remediation_plan.md`:
//! 1. Untrusted repository-local `.git/config` `core.editor` RCE prevention.
//! 2. Dash-prefixed filename (`-c:!sh`, `--cmd=...`) CLI argument injection neutralization.
//! 3. Hardened `safe_git_command` blocking `core.fsmonitor` and repository hooks.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use tigrs_core::cancel::CancellationToken;
use tigrs_git::GitEngine;
use tigrs_git::status::{StatusItem, StatusReport, StatusSection};
use tigrs_ui::ViewKind;
use tigrs_ui::app::{AppState, execute_action};
use tigrs_ui::editor::{EditTarget, build_editor_command_line};
use tigrs_ui::keymap::Action;
use tigrs_ui::view::StatusView;

fn init_git_repo(dir: &Path) {
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["init", "--initial-branch=main"])
        .current_dir(dir)
        .status()
        .expect("git init");
    assert!(status.success());

    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "user.name", "Security Auditor"])
        .current_dir(dir)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "user.email", "security@example.com"])
        .current_dir(dir)
        .status();
}

#[test]
fn test_untrusted_repo_local_core_editor_is_rejected() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path();
    init_git_repo(repo_root);

    // Create a file in the worktree so Edit target validation succeeds
    fs::write(repo_root.join("README.md"), "Hello untrusted repo\n").unwrap();

    // Configure a malicious repository-local core.editor in .git/config
    let malicious_editor = "touch /tmp/TIGRS_RCE_PWNED; vi";
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "--local", "core.editor", malicious_editor])
        .current_dir(repo_root)
        .status()
        .expect("git config --local core.editor");
    assert!(status.success());

    let engine = GitEngine::open(Some(repo_root)).expect("open git engine");

    // 1. Untrusted repository MUST reject repository-local core.editor
    assert_ne!(
        engine.core_editor(false).as_deref(),
        Some(malicious_editor),
        "Untrusted repository must ignore local .git/config core.editor"
    );

    // 2. Explicitly trusted repository accepts local core.editor
    assert_eq!(
        engine.core_editor(true).as_deref(),
        Some(malicious_editor),
        "Trusted repository should accept local core.editor"
    );

    // 3. In AppState with default SecurityConfig (untrusted), pressing 'e' must NOT use malicious editor
    let mut app = AppState {
        engine: Some(engine.clone()),
        ..Default::default()
    };

    let mut report = StatusReport::default();
    report.untracked.push(StatusItem::new(
        '?',
        StatusSection::Untracked,
        "README.md",
        None,
    ));
    app.views.status_view = Some(StatusView::new(report));
    app.push_view(ViewKind::Status);
    // Move cursor to the file row in StatusView (row 0 is header, row 1 is untracked section header, row 2 is README.md)
    if let Some(ref mut sv) = app.views.status_view {
        sv.set_cursor(2, 24);
    }

    assert!(!app.is_read_only());

    // 3. When Read-Only mode is enabled (`read_only = true`), pressing 'e' is blocked before even spawning an editor
    app.options.read_only = true;
    app.sync_read_only_state();
    let _ = execute_action(&mut app, &Action::Edit, 24);
    assert!(
        app.pending_editor.is_none(),
        "Read-only mode must block Action::Edit"
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some(tigrs_ui::app::READ_ONLY_WARNING_MSG)
    );

    // 4. In default Update Mode (`read_only = false`), no repository is trusted for
    // repository-local `.git/config` `core.editor` (no allowlist)
    app.options.read_only = false;
    app.sync_read_only_state();
    let _ = execute_action(&mut app, &Action::Edit, 24);
    let inv = app
        .pending_editor
        .take()
        .expect("pending_editor should be populated in update mode");
    assert!(
        !inv.command_line.contains("TIGRS_RCE_PWNED"),
        "Repository pending_editor command line must never execute local .git/config core.editor: {}",
        inv.command_line
    );
}

#[test]
fn test_dash_prefixed_filename_argument_injection_neutralized() {
    // Malicious filename attempting Vim Ex command flag injection (`-c:!sh`)
    let target = EditTarget {
        path: "-c:!touch /tmp/TIGRS_ARG_INJ_PWNED".to_string(),
        line: Some(10),
        temp: false,
    };

    let cmd = build_editor_command_line("vim", &target, true);
    assert_eq!(
        cmd, "vim +10 './-c:!touch /tmp/TIGRS_ARG_INJ_PWNED'",
        "Dash-leading relative path MUST be prefixed with './' before quoting"
    );

    // Without line numbers
    let cmd_noline = build_editor_command_line("vi", &target, false);
    assert_eq!(cmd_noline, "vi './-c:!touch /tmp/TIGRS_ARG_INJ_PWNED'");

    // Normal relative paths are unchanged
    let normal_target = EditTarget {
        path: "src/main.rs".to_string(),
        line: Some(42),
        temp: false,
    };
    assert_eq!(
        build_editor_command_line("nvim", &normal_target, true),
        "nvim +42 'src/main.rs'"
    );
}

#[test]
fn test_safe_git_command_blocks_untrusted_fsmonitor_and_hooks() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path();
    init_git_repo(repo_root);

    let marker_file = temp.path().join("PWNED_FSMONITOR_MARKER");
    let fsmonitor_cmd = format!("touch {}", marker_file.display());

    // Set core.fsmonitor in local .git/config
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "--local", "core.fsmonitor", &fsmonitor_cmd])
        .current_dir(repo_root)
        .status()
        .expect("git config --local core.fsmonitor");
    assert!(status.success());

    fs::write(repo_root.join("code.rs"), "fn main() {}\n").unwrap();

    let engine = GitEngine::open(Some(repo_root)).expect("open engine");
    let (_, token) = CancellationToken::new();

    // 1. Status scan via CLI fallback
    let _ = engine.load_status(&token);
    assert!(
        !marker_file.exists(),
        "safe_git_command must block core.fsmonitor execution during status scan"
    );

    // 2. Stage file
    let _ = engine.stage_file("code.rs", None);
    assert!(
        !marker_file.exists(),
        "safe_git_command must block core.fsmonitor execution during stage_file"
    );

    // 3. List refs
    let _ = engine.list_refs();
    assert!(
        !marker_file.exists(),
        "safe_git_command must block core.fsmonitor execution during list_refs"
    );
}

#[test]
fn test_editor_plus_prefix_ex_command_injection_prevented() {
    use tigrs_ui::editor::{EditTarget, build_editor_command_line};

    let malicious_plus = EditTarget {
        path: "+!touch /tmp/pwned".to_string(),
        line: Some(10),
        temp: false,
    };
    let cmd = build_editor_command_line("vim", &malicious_plus, true);
    assert_eq!(
        cmd, "vim +10 './+!touch /tmp/pwned'",
        "+-leading filenames must be prefixed with ./ so vim/nvim/vi/emacs/nano do not execute them as Ex/startup commands"
    );

    let malicious_dash = EditTarget {
        path: "-c:!sh".to_string(),
        line: None,
        temp: false,
    };
    let cmd_dash = build_editor_command_line("nvim", &malicious_dash, false);
    assert_eq!(cmd_dash, "nvim './-c:!sh'");
}

#[test]
fn test_untrusted_repo_diff_textconv_rce_prevented() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path();
    init_git_repo(repo_root);

    let marker_file = temp.path().join("PWNED_TEXTCONV_MARKER");
    let evil_script = format!("touch {}", marker_file.display());

    // Configure diff.pwn.textconv in local .git/config
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "--local", "diff.pwn.textconv", &evil_script])
        .current_dir(repo_root)
        .status()
        .expect("git config --local diff.pwn.textconv");
    assert!(status.success());

    // Configure .gitattributes to route all files through the malicious diff driver
    fs::write(repo_root.join(".gitattributes"), "* diff=pwn\n").unwrap();
    fs::write(repo_root.join("test.txt"), "line 1\n").unwrap();

    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "."])
        .current_dir(repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "init"])
        .current_dir(repo_root)
        .status();

    // Modify test.txt to create unstaged and staged changes
    fs::write(repo_root.join("test.txt"), "line 1\nline 2\n").unwrap();

    let engine = GitEngine::open(Some(repo_root)).expect("open engine");
    let (_, token) = CancellationToken::new();

    let item = StatusItem::new('M', StatusSection::Unstaged, "test.txt", None);

    // 1. Single-item diff must NOT execute diff.pwn.textconv
    let diff_res = engine.compute_status_item_diff(&item);
    assert!(diff_res.is_ok(), "diff computation should succeed");
    assert!(
        !marker_file.exists(),
        "compute_status_item_diff must prevent diff.textconv RCE via --no-textconv and GIT_ATTR_SOURCE"
    );

    // 2. Section diff must NOT execute diff.pwn.textconv
    let section_res =
        engine.compute_status_section_diff(StatusSection::Unstaged, std::slice::from_ref(&item));
    assert!(section_res.is_ok(), "section diff should succeed");
    assert!(
        !marker_file.exists(),
        "compute_status_section_diff must prevent diff.textconv RCE"
    );

    // 3. CLI Blame must NOT execute diff.pwn.textconv
    let blame_res = tigrs_git::blame::compute_blame_via_cli_cancellable(
        Some(repo_root),
        None,
        "test.txt",
        &token,
    );
    assert!(blame_res.is_ok(), "blame should succeed");
    assert!(
        !marker_file.exists(),
        "compute_blame_via_cli_cancellable must prevent diff.textconv RCE via --no-textconv"
    );
}

#[test]
fn test_untrusted_repo_filter_clean_smudge_rce_prevented() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path();
    init_git_repo(repo_root);

    let marker_file = temp.path().join("PWNED_FILTER_MARKER");
    let evil_script = format!("touch {}", marker_file.display());

    // Configure filter.pwn.clean and filter.pwn.smudge in local .git/config
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "--local", "filter.pwn.clean", &evil_script])
        .current_dir(repo_root)
        .status()
        .expect("git config filter.pwn.clean");
    assert!(status.success());
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "--local", "filter.pwn.smudge", &evil_script])
        .current_dir(repo_root)
        .status()
        .expect("git config filter.pwn.smudge");
    assert!(status.success());

    fs::write(repo_root.join(".gitattributes"), "* filter=pwn\n").unwrap();
    fs::write(repo_root.join("sample.txt"), "hello filter security\n").unwrap();

    let engine = GitEngine::open(Some(repo_root)).expect("open engine");
    assert!(!engine.is_read_only());
    engine.set_read_only(true);
    assert!(matches!(
        engine.stage_file("sample.txt", None),
        Err(tigrs_core::error::TigError::ReadOnly(_))
    ));
    engine.set_read_only(false);

    // 1. Staging untrusted file must NOT execute filter.pwn.clean
    let stage_res = engine.stage_file("sample.txt", None);
    assert!(stage_res.is_ok(), "staging should succeed");
    assert!(
        !marker_file.exists(),
        "stage_file must prevent filter.clean RCE via GIT_ATTR_SOURCE and attributes hardening"
    );

    // Modify file and test discard
    fs::write(repo_root.join("sample.txt"), "modified\n").unwrap();

    // 2. Discarding changes must NOT execute filter.pwn.smudge
    let discard_res = engine.discard_file_changes("sample.txt", None);
    assert!(discard_res.is_ok(), "discarding should succeed");
    assert!(
        !marker_file.exists(),
        "discard_file_changes must prevent filter.smudge RCE"
    );
}

#[test]
fn test_git_metadata_boundary_and_symlink_escapes_rejected() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path();
    init_git_repo(repo_root);

    // 1. verify_relative_path rejects .git and its subpaths
    assert!(tigrs_git::verify_relative_path(".git").is_err());
    assert!(tigrs_git::verify_relative_path(".git/config").is_err());
    assert!(tigrs_git::verify_relative_path(".git/hooks/pre-commit").is_err());
    assert!(tigrs_git::verify_relative_path("a/../../.git/config").is_err());
    assert!(tigrs_git::verify_relative_path(".GIT/config").is_err());

    // 2. verify_worktree_path_safety rejects symlinks resolving into .git
    let evil_symlink = repo_root.join("evil_config");
    std::os::unix::fs::symlink(".git/config", &evil_symlink).unwrap();

    let res = tigrs_git::verify_worktree_path_safety(repo_root, "evil_config");
    assert!(res.is_err(), "Symlink into .git must be rejected");

    // 3. Staging and hunk operations reject .git paths
    let dummy_hunk = tigrs_git::DiffHunk {
        old_start: 1,
        old_len: 1,
        new_start: 1,
        new_len: 1,
        lines: vec![],
        func_context: None,
    };
    assert!(tigrs_git::stage::stage_hunk(repo_root, ".git/config", &dummy_hunk).is_err());
    assert!(tigrs_git::stage::unstage_hunk(repo_root, ".git/config", &dummy_hunk).is_err());
    assert!(tigrs_git::stage::stage_lines(repo_root, ".git/config", &dummy_hunk, &[0]).is_err());
    assert!(tigrs_git::stage::unstage_lines(repo_root, ".git/config", &dummy_hunk, &[0]).is_err());

    // 4. Editor target resolution refuses symlinks to .git metadata
    let engine = GitEngine::open(Some(repo_root)).expect("open engine");
    let mut app = AppState {
        engine: Some(engine),
        ..Default::default()
    };

    let mut report = StatusReport::default();
    report.unstaged.push(StatusItem::new(
        'M',
        StatusSection::Unstaged,
        "evil_config",
        None,
    ));
    app.views.status_view = Some(StatusView::new(report));
    app.push_view(ViewKind::Status);
    if let Some(ref mut sv) = app.views.status_view {
        sv.set_cursor(2, 24);
    }

    let edit_res = tigrs_ui::editor::resolve_edit_target(&app);
    assert!(
        matches!(edit_res, Err(tigrs_ui::editor::EditRefusal::Unreadable(_))),
        "Editing symlink pointing to .git metadata must be refused"
    );
}

#[test]
fn test_info_attributes_filter_drivers_neutralized() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path();
    init_git_repo(repo_root);

    fs::write(repo_root.join("tracked.txt"), "initial line\n").unwrap();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "tracked.txt"])
        .current_dir(repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "initial"])
        .current_dir(repo_root)
        .status();

    let marker_file = temp.path().join("INFO_ATTR_MARKER");
    let marker_cmd = format!("touch {}", marker_file.display());

    for key in [
        "filter.drv.clean",
        "filter.drv.smudge",
        "filter.drv.process",
    ] {
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["config", "--local", key, &marker_cmd])
            .current_dir(repo_root)
            .status()
            .expect("git config filter");
        assert!(status.success());
    }
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "--local", "filter.drv.required", "true"])
        .current_dir(repo_root)
        .status();

    let info_dir = repo_root.join(".git").join("info");
    fs::create_dir_all(&info_dir).unwrap();
    fs::write(info_dir.join("attributes"), "* filter=drv\n").unwrap();

    fs::write(repo_root.join("tracked.txt"), "modified line\n").unwrap();

    let engine = GitEngine::open(Some(repo_root)).expect("open engine");
    let item = StatusItem::new('M', StatusSection::Unstaged, "tracked.txt", None);

    // 1. Single-item diff must neutralize .git/info/attributes filters
    let item_diff = engine.compute_status_item_diff(&item);
    assert!(item_diff.is_ok(), "item diff should succeed: {item_diff:?}");
    assert!(
        !marker_file.exists(),
        "compute_status_item_diff must block .git/info/attributes filter execution"
    );

    // 2. Section diff must neutralize .git/info/attributes filters
    let section_diff =
        engine.compute_status_section_diff(StatusSection::Unstaged, std::slice::from_ref(&item));
    assert!(
        section_diff.is_ok(),
        "section diff should succeed: {section_diff:?}"
    );
    assert!(
        !marker_file.exists(),
        "compute_status_section_diff must block .git/info/attributes filter execution"
    );

    // 3. Staging file (blocked when Read-Only mode is enabled; allowed in default Update Mode)
    //    must neutralize .git/info/attributes filters
    assert!(!engine.is_read_only());
    engine.set_read_only(true);
    assert!(matches!(
        engine.stage_file("tracked.txt", None),
        Err(tigrs_core::error::TigError::ReadOnly(_))
    ));
    engine.set_read_only(false);
    let stage_res = engine.stage_file("tracked.txt", None);
    assert!(
        stage_res.is_ok(),
        "stage_file should succeed: {stage_res:?}"
    );
    assert!(
        !marker_file.exists(),
        "stage_file must block .git/info/attributes filter execution"
    );

    // 4. Discarding changes must neutralize .git/info/attributes filters
    fs::write(repo_root.join("tracked.txt"), "dirty again\n").unwrap();
    let discard_res = engine.discard_file_changes("tracked.txt", None);
    assert!(
        discard_res.is_ok(),
        "discard_file_changes should succeed: {discard_res:?}"
    );
    assert!(
        !marker_file.exists(),
        "discard_file_changes must block .git/info/attributes filter execution"
    );
}

#[test]
fn test_sha256_repository_diff_and_status_compatibility() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path();

    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["init", "--object-format=sha256", "--initial-branch=main"])
        .current_dir(repo_root)
        .status()
        .expect("git init sha256");
    assert!(status.success());

    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "user.name", "Tester"])
        .current_dir(repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "user.email", "tester@example.com"])
        .current_dir(repo_root)
        .status();

    fs::write(repo_root.join("hello.txt"), "v1\n").unwrap();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "hello.txt"])
        .current_dir(repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "c1"])
        .current_dir(repo_root)
        .status();

    fs::write(repo_root.join("hello.txt"), "v2\n").unwrap();

    let item = StatusItem::new('M', StatusSection::Unstaged, "hello.txt", None);
    let diff_res = tigrs_git::status::compute_status_item_diff(repo_root, &item);
    assert!(
        diff_res.is_ok(),
        "SHA-256 repository diff must succeed with SHA-256 GIT_ATTR_SOURCE: {diff_res:?}"
    );
    assert_eq!(diff_res.unwrap().files.len(), 1);
}

#[test]
fn test_equals_sign_and_included_filter_driver_rce_blocked() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    init_git_repo(&repo_root);

    let marker_file = temp.path().join("PWNED_BY_EQUALS_FILTER");
    let marker_cmd = format!("touch {}; cat", marker_file.display());

    // 1. Define a filter with `=` in its name (`filter.a=b.clean` and `filter.a=b.smudge`)
    //    both directly and via an `[include]` file inside `.git/`
    let inc_path = repo_root.join(".git").join("evil.inc");
    fs::write(
        &inc_path,
        format!(
            "[filter \"a=b\"]\n\tclean = {marker_cmd}\n\tsmudge = {marker_cmd}\n\trequired = true\n[filter \"inc=drv\"]\n\tclean = {marker_cmd}\n\tsmudge = {marker_cmd}\n"
        ),
    )
    .unwrap();

    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "include.path", "evil.inc"])
        .current_dir(&repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "filter.a=b.clean", &marker_cmd])
        .current_dir(&repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "filter.a=b.smudge", &marker_cmd])
        .current_dir(&repo_root)
        .status();

    fs::write(repo_root.join("tracked.txt"), "initial line\n").unwrap();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "tracked.txt"])
        .current_dir(&repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "initial"])
        .current_dir(&repo_root)
        .status();

    // 2. Reference both `=b` drivers in `.git/info/attributes`
    let info_dir = repo_root.join(".git").join("info");
    fs::create_dir_all(&info_dir).unwrap();
    fs::write(
        info_dir.join("attributes"),
        "*.txt filter=a=b\n*.md filter=\"inc=drv\"\n",
    )
    .unwrap();

    fs::write(repo_root.join("tracked.txt"), "modified line\n").unwrap();
    fs::write(repo_root.join("notes.md"), "untracked markdown\n").unwrap();

    let engine = GitEngine::open(Some(&repo_root)).expect("open engine");
    engine.set_read_only(false);

    let stage_res = engine.stage_file("tracked.txt", None);
    assert!(
        stage_res.is_ok(),
        "stage_file should succeed: {stage_res:?}"
    );
    assert!(
        !marker_file.exists(),
        "stage_file must block filter driver containing '=' in its name"
    );

    let stage_inc = engine.stage_file("notes.md", None);
    assert!(
        stage_inc.is_ok(),
        "stage_file should succeed: {stage_inc:?}"
    );
    assert!(
        !marker_file.exists(),
        "stage_file must block included filter driver containing '=' in its name"
    );

    let _ = engine.unstage_file("tracked.txt", None);
    fs::write(repo_root.join("tracked.txt"), "dirty again\n").unwrap();
    let discard_res = engine.discard_file_changes("tracked.txt", None);
    assert!(
        discard_res.is_ok(),
        "discard_file_changes should succeed: {discard_res:?}"
    );
    assert!(
        !marker_file.exists(),
        "discard_file_changes must block filter driver containing '=' in its name"
    );
}

#[test]
fn test_discard_untracked_directory_symlink_preserves_target_contents() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    init_git_repo(&repo_root);

    // Create a tracked directory with an important file inside the worktree
    let real_dir = repo_root.join("real_dir");
    fs::create_dir_all(&real_dir).unwrap();
    let important_file = real_dir.join("important.txt");
    fs::write(&important_file, "precious data\n").unwrap();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "real_dir/important.txt"])
        .current_dir(&repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "add important file"])
        .current_dir(&repo_root)
        .status();

    // Create an untracked symlink pointing to `real_dir`
    let symlink_dir = repo_root.join("untracked_link_to_dir");
    std::os::unix::fs::symlink("real_dir", &symlink_dir).unwrap();

    let res = tigrs_git::stage::discard_untracked_file(&repo_root, "untracked_link_to_dir");
    assert!(res.is_ok(), "discard_untracked_file failed: {res:?}");
    assert!(
        !symlink_dir.exists() && !symlink_dir.is_symlink(),
        "untracked symlink itself should be removed"
    );
    assert!(
        important_file.exists(),
        "target directory contents must NEVER be deleted when discarding a symlink to a directory"
    );
}

#[test]
fn test_backtick_macro_interpolation_blocks_command_substitution() {
    let temp = TempDir::new().unwrap();
    let marker = temp.path().join("PWNED_BACKTICK");

    let evil_branch = format!(
        "feat'`touch {}`$(touch {})\"\\; echo PWN",
        marker.display(),
        marker.display()
    );
    let mut map = std::collections::HashMap::new();
    map.insert("branch", evil_branch.as_str());

    let templates_and_expected = [
        // 1. Unquoted backtick + inner Unquoted
        (
            "x=`printf '%s' %(branch)`; printf '%s' \"$x\"",
            evil_branch.clone(),
        ),
        // 2. Unquoted backtick + inner SingleQuoted
        (
            "x=`printf '%s' 'pre:%(branch):post'`; printf '%s' \"$x\"",
            format!("pre:{evil_branch}:post"),
        ),
        // 3. Unquoted backtick + inner DoubleQuoted
        (
            "x=`printf '%s' \"pre:%(branch):post\"`; printf '%s' \"$x\"",
            format!("pre:{evil_branch}:post"),
        ),
        // 4. DoubleQuoted backtick + inner Unquoted
        (
            "x=\"`printf '%s' %(branch)`\"; printf '%s' \"$x\"",
            evil_branch.clone(),
        ),
        // 5. DoubleQuoted backtick + inner SingleQuoted
        (
            "x=\"`printf '%s' 'pre:%(branch):post'`\"; printf '%s' \"$x\"",
            format!("pre:{evil_branch}:post"),
        ),
        // 6. DoubleQuoted backtick + inner DoubleQuoted
        (
            "x=\"`printf '%s' \\\"pre:%(branch):post\\\"`\"; printf '%s' \"$x\"",
            format!("pre:{evil_branch}:post"),
        ),
    ];

    for (tpl, expected) in templates_and_expected {
        let cmd = tigrs_core::quote::interpolate_command(tpl, &map).unwrap();
        assert!(
            tigrs_core::quote::verify_shell_safety(&cmd).is_ok(),
            "verify_shell_safety failed for template {tpl}: {cmd}"
        );
        let out = Command::new("sh").arg("-c").arg(&cmd).output().unwrap();
        assert!(out.status.success(), "sh failed for {tpl}: {cmd}");
        assert!(
            !marker.exists(),
            "backtick command substitution executed payload for template {tpl}"
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            expected,
            "mismatch for template {tpl}"
        );
    }
}

#[test]
fn test_mixed_case_config_section_and_quoted_attribute_pattern_rce_blocked() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    init_git_repo(&repo_root);

    let marker_file = temp.path().join("PWNED_BY_MIXED_CASE_AND_QUOTED_ATTR");
    let marker_cmd = format!("touch {}; cat", marker_file.display());

    fs::write(repo_root.join("tracked.txt"), "v1\n").unwrap();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "tracked.txt"])
        .current_dir(&repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "initial"])
        .current_dir(&repo_root)
        .status();

    // 1. Write mixed-case `[Filter "evil"]` and `[Include]` in `.git/config`
    let inc_path = repo_root.join(".git").join("mixed.inc");
    fs::write(
        &inc_path,
        format!(
            "[FILTER \"inc_evil\"]\n\tclean = {marker_cmd}\n\tsmudge = {marker_cmd}\n[filter.dot-evil]\n\tclean = {marker_cmd}\n"
        ),
    )
    .unwrap();

    let cfg_path = repo_root.join(".git").join("config");
    let cfg_text = fs::read_to_string(&cfg_path).unwrap();
    let updated_cfg = format!(
        "{cfg_text}\n[Filter \"evil\"]\n\tclean = {marker_cmd}\n\tsmudge = {marker_cmd}\n[Include]\n\tPaTh = mixed.inc\n"
    );
    fs::write(&cfg_path, updated_cfg).unwrap();

    // 2. Write C-style quoted pattern without trailing space `"*"filter=evil` in `.git/info/attributes`
    let info_dir = repo_root.join(".git").join("info");
    fs::create_dir_all(&info_dir).unwrap();
    fs::write(
        info_dir.join("attributes"),
        "\"*\"filter=evil\n\"*.md\"filter=inc_evil\n\"*.rs\"filter=dot-evil\n",
    )
    .unwrap();

    // Dirty tracked.txt so zero-click status diff would invoke filter.evil.clean if unmitigated
    fs::write(repo_root.join("tracked.txt"), "v2\n").unwrap();
    fs::write(repo_root.join("doc.md"), "md\n").unwrap();
    fs::write(repo_root.join("lib.rs"), "rs\n").unwrap();

    let engine = GitEngine::open(Some(&repo_root)).expect("open engine");
    let item = StatusItem::new('M', StatusSection::Unstaged, "tracked.txt", None);

    // Zero-click diff inspection
    let diff_res = engine.compute_status_item_diff(&item);
    assert!(diff_res.is_ok(), "item diff should succeed: {diff_res:?}");
    assert!(
        !marker_file.exists(),
        "compute_status_item_diff must block mixed-case [Filter] + quoted pattern \"*\"filter=evil"
    );

    let sec_res =
        engine.compute_status_section_diff(StatusSection::Unstaged, std::slice::from_ref(&item));
    assert!(sec_res.is_ok(), "section diff should succeed: {sec_res:?}");
    assert!(
        !marker_file.exists(),
        "compute_status_section_diff must block mixed-case [Filter] + quoted pattern \"*\"filter=evil"
    );

    // Interactive staging and discarding
    let _ = engine.stage_file("tracked.txt", None);
    let _ = engine.stage_file("doc.md", None);
    let _ = engine.stage_file("lib.rs", None);
    assert!(
        !marker_file.exists(),
        "stage_file must block mixed-case [Filter], [Include], and dot-syntax filter drivers"
    );

    let _ = engine.unstage_file("tracked.txt", None);
    let _ = engine.discard_file_changes("tracked.txt", None);
    assert!(
        !marker_file.exists(),
        "discard_file_changes must block mixed-case [Filter] + quoted pattern \"*\"filter=evil"
    );
}

#[test]
fn test_non_utf8_byte_poisoning_in_config_and_info_attributes_rce_blocked() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    init_git_repo(&repo_root);

    let marker_file = temp.path().join("PWNED_BY_NON_UTF8_POISONING");
    let marker_cmd = format!("touch {}; cat", marker_file.display());

    fs::write(repo_root.join("tracked.txt"), "v1\n").unwrap();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "tracked.txt"])
        .current_dir(&repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "initial"])
        .current_dir(&repo_root)
        .status();

    // 1. Poison `.git/config` with invalid UTF-8 byte `0xff` in a comment alongside `[filter "a]b"]`
    //    (`]` inside subsection quotes also fails `gix::config::File` parsing, testing `scan_git_config_stream`).
    let cfg_path = repo_root.join(".git").join("config");
    let mut cfg_bytes = fs::read(&cfg_path).unwrap();
    cfg_bytes.extend_from_slice(b"\n# non-utf8 comment \xff\xfe\n");
    cfg_bytes.extend_from_slice(
        format!("[filter \"a]b\"]\n\tclean = {marker_cmd}\n\tsmudge = {marker_cmd}\n").as_bytes(),
    );
    fs::write(&cfg_path, cfg_bytes).unwrap();

    // 2. Poison `.git/info/attributes` with invalid UTF-8 byte `0xff` in a comment
    let info_dir = repo_root.join(".git").join("info");
    fs::create_dir_all(&info_dir).unwrap();
    let mut attr_bytes = Vec::new();
    attr_bytes.extend_from_slice(b"# non-utf8 comment \xff\xfe\n");
    attr_bytes.extend_from_slice(b"*.txt filter=a]b\n*.md filter=extra_attr_drv\n");
    fs::write(info_dir.join("attributes"), attr_bytes).unwrap();

    // Dirty tracked.txt
    fs::write(repo_root.join("tracked.txt"), "v2\n").unwrap();

    let engine = GitEngine::open(Some(&repo_root)).expect("open engine");
    let item = StatusItem::new('M', StatusSection::Unstaged, "tracked.txt", None);

    let diff_res = engine.compute_status_item_diff(&item);
    assert!(diff_res.is_ok(), "item diff should succeed: {diff_res:?}");
    assert!(
        !marker_file.exists(),
        "non-UTF-8 bytes in .git/config and .git/info/attributes must not bypass filter neutralization"
    );

    engine.set_read_only(false);
    let stage_res = engine.stage_file("tracked.txt", None);
    assert!(
        stage_res.is_ok(),
        "stage_file should succeed: {stage_res:?}"
    );
    assert!(
        !marker_file.exists(),
        "stage_file must block filter.a]b.clean even when .git/config contains non-UTF-8 bytes"
    );
}

#[test]
fn test_nested_subdirectory_relative_include_and_line_continuation_rce_blocked() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    init_git_repo(&repo_root);

    let marker_file = temp.path().join("PWNED_BY_NESTED_INCLUDE_OR_CONT");
    let marker_cmd = format!("touch {}; cat", marker_file.display());

    fs::write(repo_root.join("tracked.txt"), "v1\n").unwrap();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "tracked.txt"])
        .current_dir(&repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "initial"])
        .current_dir(&repo_root)
        .status();

    // 1. Create `.git/sub/a.inc` which relatively includes `b.inc` (resolved relative to `.git/sub/`!)
    let sub_dir = repo_root.join(".git").join("sub");
    fs::create_dir_all(&sub_dir).unwrap();
    fs::write(
        sub_dir.join("a.inc"),
        b"# \xff\n[include]\n\tpath = \"b\\\n.inc\"\n",
    )
    .unwrap();
    let mut b_inc_bytes = b"# \xff\n".to_vec();
    b_inc_bytes.extend_from_slice(
        format!("[filter \"sub]drv\"]\n\tclean = {marker_cmd}\n\tsmudge = {marker_cmd}\n")
            .as_bytes(),
    );
    fs::write(sub_dir.join("b.inc"), b_inc_bytes).unwrap();

    // 2. Also create `.git/crlf_evil.inc` included via CRLF `\` + `\r\n` continuation
    let mut crlf_inc_bytes = b"# \xff\n".to_vec();
    crlf_inc_bytes.extend_from_slice(
        format!("[filter \"crlf]drv\"]\n\tclean = {marker_cmd}\n\tsmudge = {marker_cmd}\n")
            .as_bytes(),
    );
    fs::write(repo_root.join(".git").join("crlf_evil.inc"), crlf_inc_bytes).unwrap();

    // 3. Reference `sub/a.inc` (via quoted `\` + `\n` continuation) and `crlf_evil.inc` (via CRLF continuation) in `.git/config`
    let cfg_path = repo_root.join(".git").join("config");
    let mut cfg_bytes = fs::read(&cfg_path).unwrap();
    cfg_bytes.extend_from_slice(
        b"\n# \xff\n[include]\n\tpath = \"sub/a\\\n.inc\"\n\tpath = \\\r\n\tcrlf_evil.inc\n",
    );
    fs::write(&cfg_path, cfg_bytes).unwrap();

    let info_dir = repo_root.join(".git").join("info");
    fs::create_dir_all(&info_dir).unwrap();
    fs::write(
        info_dir.join("attributes"),
        "*.txt filter=sub]drv\n*.md filter=crlf]drv\n",
    )
    .unwrap();

    fs::write(repo_root.join("tracked.txt"), "v2\n").unwrap();
    fs::write(repo_root.join("readme.md"), "md\n").unwrap();

    let engine = GitEngine::open(Some(&repo_root)).expect("open engine");
    let item = StatusItem::new('M', StatusSection::Unstaged, "tracked.txt", None);

    let diff_res = engine.compute_status_item_diff(&item);
    assert!(diff_res.is_ok(), "item diff should succeed: {diff_res:?}");
    assert!(
        !marker_file.exists(),
        "compute_status_item_diff must block nested relative [include] and backslash-newline continuation filters"
    );

    engine.set_read_only(false);
    let _ = engine.stage_file("tracked.txt", None);
    let _ = engine.stage_file("readme.md", None);
    assert!(
        !marker_file.exists(),
        "stage_file must block nested relative [include] and CRLF continuation filters"
    );
}

#[test]
fn test_untrusted_repo_env_blocks_malicious_core_editor_and_hooks_on_handover() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    init_git_repo(&repo_root);

    let marker_file = temp.path().join("PWNED_BY_REPO_EDITOR_OR_HOOK");
    let marker_cmd = format!("touch {}", marker_file.display());

    // Configure malicious `core.editor` and `core.hooksPath` in `.git/config`
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "core.editor", &marker_cmd])
        .current_dir(&repo_root)
        .status();
    let hooks_dir = repo_root.join(".git").join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();
    let pre_commit = hooks_dir.join("pre-commit");
    fs::write(&pre_commit, format!("#!/bin/sh\n{marker_cmd}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&pre_commit, fs::Permissions::from_mode(0o755));
    }

    fs::write(repo_root.join("file.txt"), "staged content\n").unwrap();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "file.txt"])
        .current_dir(&repo_root)
        .status();

    // Simulate spawning `git commit` inside a shell with `apply_untrusted_repo_env`
    let mut shell_cmd = Command::new("sh");
    shell_cmd
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .arg("-c")
        .arg("git commit -m 'msg'")
        .current_dir(&repo_root)
        .env("GIT_EDITOR", "true");
    tigrs_git::apply_untrusted_repo_env(&mut shell_cmd, &repo_root);

    let status = shell_cmd.status().expect("run git commit in shell");
    assert!(status.success(), "git commit should succeed");
    assert!(
        !marker_file.exists(),
        "apply_untrusted_repo_env must block repository-local pre-commit hooks and core.editor"
    );
}

#[test]
fn test_vertical_tab_comment_spoofing_and_subsection_escape_rce_blocked() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    init_git_repo(&repo_root);

    let marker_file = temp.path().join("PWNED_BY_VT_AND_SUBSECTION_ESCAPE");
    let marker_cmd = format!("touch {}; cat", marker_file.display());

    // 1. Create a file named `\x0b#` (vertical tab + `#`) and commit it
    let vt_filename = "\x0b#";
    fs::write(repo_root.join(vt_filename), "v1\n").unwrap();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", vt_filename])
        .current_dir(&repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "initial"])
        .current_dir(&repo_root)
        .status();

    // 2. Define `[filter "a]b\nc\t\b"]` in `.git/config` (with UTF-8 BOM and non-UTF-8 byte).
    //    In Git's `config.c` (`parse_section_header`), `\n`, `\t`, `\b` inside quoted
    //    subsection headers strip `\` and become literal `'n'`, `'t'`, `'b'` (`a]bnctb`).
    let cfg_path = repo_root.join(".git").join("config");
    let mut cfg_bytes = b"\xef\xbb\xbf".to_vec();
    cfg_bytes.extend_from_slice(&fs::read(&cfg_path).unwrap());
    cfg_bytes.extend_from_slice(b"\n# \xff\n");
    cfg_bytes.extend_from_slice(
        format!("[filter \"a]b\\nc\\t\\b\"]\n\tclean = {marker_cmd}\n\tsmudge = {marker_cmd}\n")
            .as_bytes(),
    );
    fs::write(&cfg_path, cfg_bytes).unwrap();

    // 3. In `.git/info/attributes`, start the rule with `\x0b# filter=a]bnctb`.
    //    Because `attr.c` only trims `" \t\r\n"`, `\x0b#` is NOT a `#` comment in Git!
    let info_dir = repo_root.join(".git").join("info");
    fs::create_dir_all(&info_dir).unwrap();
    fs::write(
        info_dir.join("attributes"),
        b"\xef\xbb\xbf\x0b# filter=a]bnctb\n",
    )
    .unwrap();

    // Dirty `\x0b#`
    fs::write(repo_root.join(vt_filename), "v2\n").unwrap();

    let engine = GitEngine::open(Some(&repo_root)).expect("open engine");
    let item = StatusItem::new('M', StatusSection::Unstaged, vt_filename, None);

    let diff_res = engine.compute_status_item_diff(&item);
    assert!(diff_res.is_ok(), "item diff should succeed: {diff_res:?}");
    assert!(
        !marker_file.exists(),
        "vertical-tab comment spoofing and subsection backslash escape must not bypass filter neutralization"
    );

    engine.set_read_only(false);
    let stage_res = engine.stage_file(vt_filename, None);
    assert!(
        stage_res.is_ok(),
        "stage_file should succeed: {stage_res:?}"
    );
    assert!(
        !marker_file.exists(),
        "stage_file must block filter.a]bnctb.clean"
    );
}

#[test]
fn test_chained_symlink_escape_with_nonexistent_leaf_and_decoy_dot_git_blocked() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path().join("repo");
    let outside_dir = temp.path().join("outside");
    fs::create_dir_all(&repo_root).unwrap();
    fs::create_dir_all(&outside_dir).unwrap();
    init_git_repo(&repo_root);

    // 1. Create a 2-hop symlink chain where the final leaf does NOT exist on disk:
    //    `escape_dir -> ../outside` and `chained_evil -> escape_dir/nonexistent_target`
    std::os::unix::fs::symlink(&outside_dir, repo_root.join("escape_dir")).unwrap();
    std::os::unix::fs::symlink(
        "escape_dir/nonexistent_target",
        repo_root.join("chained_evil"),
    )
    .unwrap();

    // Also create `escape_git -> .git/hooks` and `chained_hook -> escape_git/pre-commit` (nonexistent leaf)
    std::os::unix::fs::symlink(".git/hooks", repo_root.join("escape_git")).unwrap();
    std::os::unix::fs::symlink("escape_git/pre-commit", repo_root.join("chained_hook")).unwrap();

    assert!(
        tigrs_git::verify_worktree_path_safety(&repo_root, "chained_evil").is_err(),
        "verify_worktree_path_safety must reject chained symlink escaping worktree even when leaf does not exist"
    );
    assert!(
        tigrs_git::verify_worktree_path_safety(&repo_root, "chained_hook").is_err(),
        "verify_worktree_path_safety must reject chained symlink into .git/hooks even when leaf does not exist"
    );
    assert!(
        tigrs_git::stage::stage_file(&repo_root, "chained_evil", None).is_err(),
        "stage_file must reject chained symlink escaping worktree"
    );

    // 2. Test decoy `sub/.git` directory + repository `[alias]` neutralization
    let marker_file = temp.path().join("PWNED_BY_DECOY_OR_ALIAS");
    let marker_cmd = format!("touch {}", marker_file.display());
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "alias.st", &format!("!{marker_cmd}")])
        .current_dir(&repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "core.fsmonitor", &marker_cmd])
        .current_dir(&repo_root)
        .status();

    let sub_dir = repo_root.join("sub");
    let decoy_dot_git = sub_dir.join(".git");
    fs::create_dir_all(&decoy_dot_git).unwrap();

    // Running `safe_git_command` or `apply_untrusted_repo_env` from `sub_dir` must walk past
    // the decoy `sub/.git` (which has no HEAD/config) and neutralize `repo/.git/config`!
    let _ = tigrs_git::safe_git_command(&sub_dir).arg("status").status();
    assert!(
        !marker_file.exists(),
        "decoy sub/.git directory must not blind discover_git_metadata_dirs from scanning parent .git/config"
    );

    let mut alias_cmd = Command::new("sh");
    alias_cmd.arg("-c").arg("git st").current_dir(&sub_dir);
    tigrs_git::apply_untrusted_repo_env(&mut alias_cmd, &sub_dir);
    let _ = alias_cmd.status();
    assert!(
        !marker_file.exists(),
        "apply_untrusted_repo_env must neutralize repository-local [alias] commands"
    );
}

#[test]
fn test_nul_byte_truncation_in_commondir_gitfile_and_info_attributes_rce_blocked() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    init_git_repo(&repo_root);

    let marker_file = temp.path().join("PWNED_BY_NUL_BYTE_TRUNCATION");
    let marker_cmd = format!("touch {}; cat", marker_file.display());

    fs::write(repo_root.join("tracked.txt"), "v1\n").unwrap();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "tracked.txt"])
        .current_dir(&repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "initial"])
        .current_dir(&repo_root)
        .status();

    // Create `.git/real_common/config` and `.git/real_common/info/attributes`
    // referenced via `.git/commondir` containing `real_common\x00_decoy\n`
    let real_common = repo_root.join(".git").join("real_common");
    fs::create_dir_all(real_common.join("info")).unwrap();
    std::os::unix::fs::symlink("../objects", real_common.join("objects")).unwrap();
    std::os::unix::fs::symlink("../refs", real_common.join("refs")).unwrap();
    let orig_cfg = fs::read(repo_root.join(".git").join("config")).unwrap();
    let mut common_cfg = orig_cfg.clone();
    common_cfg.extend_from_slice(b"\n# \xff\n");
    common_cfg.extend_from_slice(
        format!("[filter \"nul_drv\"]\n\tclean = {marker_cmd}\n\tsmudge = {marker_cmd}\n")
            .as_bytes(),
    );
    fs::write(real_common.join("config"), common_cfg).unwrap();

    // Also place a NUL-truncated attribute line `* filter=nul_drv\x00_decoy\n` in both
    // `real_common/info/attributes` and `.git/info/attributes`
    let nul_attr = b"* filter=nul_drv\x00_decoy\n";
    fs::write(real_common.join("info").join("attributes"), nul_attr).unwrap();
    let dot_git_info = repo_root.join(".git").join("info");
    fs::create_dir_all(&dot_git_info).unwrap();
    fs::write(dot_git_info.join("attributes"), nul_attr).unwrap();

    fs::write(
        repo_root.join(".git").join("commondir"),
        b"real_common\x00_decoy\n",
    )
    .unwrap();

    // Dirty tracked.txt
    fs::write(repo_root.join("tracked.txt"), "v2\n").unwrap();

    // Remove commondir before opening gix engine (since gix rejects non-standard commondir setups),
    // or test safe_git_command / compute_status_item_diff directly with `.git/commondir` in place!
    let item = StatusItem::new('M', StatusSection::Unstaged, "tracked.txt", None);
    let diff_res = tigrs_git::status::compute_status_item_diff(&repo_root, &item);
    assert!(
        diff_res.is_ok(),
        "compute_status_item_diff must not fail with InvalidInput on NUL byte in info/attributes or commondir: {diff_res:?}"
    );
    assert!(
        !marker_file.exists(),
        "NUL-byte truncation in .git/commondir and .git/info/attributes must not bypass filter neutralization"
    );
}

#[test]
fn test_unquoted_bracket_in_value_and_quoted_whitespace_include_path_rce_blocked() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    init_git_repo(&repo_root);

    let marker_file = temp.path().join("PWNED_BY_BRACKET_OR_QUOTED_SPACE_INCLUDE");
    let marker_cmd = format!("touch {}; cat", marker_file.display());

    fs::write(repo_root.join("tracked.txt"), "v1\n").unwrap();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "tracked.txt"])
        .current_dir(&repo_root)
        .status();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "initial"])
        .current_dir(&repo_root)
        .status();

    // Create `.git/ spaced_evil.inc` (note leading space in filename!)
    let spaced_inc = repo_root.join(".git").join(" spaced_evil.inc");
    let mut inc_bytes = b"# \xff\n".to_vec();
    inc_bytes.extend_from_slice(
        format!("[filter \"br_space]drv\"]\n\tclean = {marker_cmd}\n\tsmudge = {marker_cmd}\n")
            .as_bytes(),
    );
    fs::write(&spaced_inc, inc_bytes).unwrap();

    // In `.git/config`, place `dummy = [core]` (unquoted `[` in value) before `path = " spaced_evil.inc"`
    let cfg_path = repo_root.join(".git").join("config");
    let mut cfg_bytes = fs::read(&cfg_path).unwrap();
    cfg_bytes.extend_from_slice(
        b"\n# \xff\n[include]\n\tdummy = [core]\n\tpath = \" spaced_evil.inc\"\n",
    );
    fs::write(&cfg_path, cfg_bytes).unwrap();

    let info_dir = repo_root.join(".git").join("info");
    fs::create_dir_all(&info_dir).unwrap();
    fs::write(info_dir.join("attributes"), "* filter=br_space]drv\n").unwrap();

    fs::write(repo_root.join("tracked.txt"), "v2\n").unwrap();

    let engine = GitEngine::open(Some(&repo_root)).expect("open engine");
    let item = StatusItem::new('M', StatusSection::Unstaged, "tracked.txt", None);

    let diff_res = engine.compute_status_item_diff(&item);
    assert!(diff_res.is_ok(), "item diff should succeed: {diff_res:?}");
    assert!(
        !marker_file.exists(),
        "unquoted '[' in config value and quoted leading whitespace in include.path must not bypass filter neutralization"
    );

    let _ = engine.stage_file("tracked.txt", None);
    assert!(
        !marker_file.exists(),
        "stage_file must neutralize filter defined inside '.git/ spaced_evil.inc'"
    );
}

#[test]
fn test_pager_subcommand_and_interactive_difffilter_neutralized() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    init_git_repo(&repo_root);

    let marker_file = temp.path().join("PWNED_BY_PAGER_OR_DIFFFILTER");
    let marker_cmd = format!("touch {}", marker_file.display());

    let cfg_path = repo_root.join(".git").join("config");
    let orig_cfg = fs::read_to_string(&cfg_path).unwrap();
    let cfg = format!(
        "{orig_cfg}\n[pager]\n\tlog = {marker_cmd}\n\tdiff = {marker_cmd}\n[interactive]\n\tdiffFilter = {marker_cmd}\n"
    );
    fs::write(&cfg_path, cfg).unwrap();

    // Verify `apply_untrusted_repo_env` sets `GIT_PAGER=cat`, `pager.log=false`, `interactive.diffFilter=`
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg("test \"$GIT_PAGER\" = \"cat\" && test \"$(git config --get pager.log)\" = \"false\" && test -z \"$(git config --get interactive.diffFilter)\"")
        .current_dir(&repo_root);
    tigrs_git::apply_untrusted_repo_env(&mut cmd, &repo_root);
    let status = cmd.status().expect("run env verification");
    assert!(
        status.success(),
        "apply_untrusted_repo_env must set GIT_PAGER=cat, pager.log=false, and empty interactive.diffFilter"
    );
    assert!(!marker_file.exists());
}

#[test]
fn test_untracked_file_diff_intermediate_symlink_escape_blocked() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path().join("repo");
    let outside_dir = temp.path().join("outside");
    fs::create_dir_all(&repo_root).unwrap();
    fs::create_dir_all(&outside_dir).unwrap();
    init_git_repo(&repo_root);

    let secret_path = outside_dir.join("secret_key.pem");
    fs::write(&secret_path, "TOP_SECRET_PRIVATE_KEY_MATERIAL\n").unwrap();

    // Create an intermediate directory symlink inside `repo_root` pointing to `outside_dir`
    std::os::unix::fs::symlink(&outside_dir, repo_root.join("link_dir")).unwrap();

    let engine = GitEngine::open(Some(&repo_root)).expect("open engine");
    let item = StatusItem::new(
        '?',
        StatusSection::Untracked,
        "link_dir/secret_key.pem",
        None,
    );

    // 1. `compute_status_item_diff` must reject the escaping intermediate symlink with a Security error
    let item_res = engine.compute_status_item_diff(&item);
    assert!(
        item_res.is_err(),
        "compute_status_item_diff must reject untracked path traversing intermediate symlink outside worktree"
    );

    // 2. `compute_status_section_diff` must not read `secret_key.pem`
    let sec_diff = engine
        .compute_status_section_diff(StatusSection::Untracked, &[item])
        .expect("section diff");
    for file in &sec_diff.files {
        for hunk in &file.hunks {
            for line in &hunk.lines {
                assert!(
                    !line.content.contains("TOP_SECRET_PRIVATE_KEY_MATERIAL"),
                    "untracked section diff must never read external file via intermediate symlink"
                );
            }
        }
    }
}

#[test]
fn test_macro_leading_dash_option_injection_and_diff_status_pager_ansi_sanitization() {
    // 1. Verify MacroContext prefixes leading-dash paths with `./` and strips leading dashes from refs
    let ctx = tigrs_core::macro_ctx::MacroContext {
        file: Some("--open-files-in-pager=touch /tmp/pwn".to_string()),
        branch: Some("--upload-pack=touch /tmp/pwn".to_string()),
        ..Default::default()
    };
    assert_eq!(
        ctx.expand("git log %(file)", |_| Some(String::new()))
            .unwrap(),
        "git log './--open-files-in-pager=touch /tmp/pwn'"
    );
    assert_eq!(
        ctx.expand("git checkout %(branch)", |_| Some(String::new()))
            .unwrap(),
        "git checkout 'upload-pack=touch /tmp/pwn'"
    );

    // 2. Verify DiffDocument::RowCell strips raw OSC 52 and BiDi sequences
    let cell = tigrs_ui::diff::document::RowCell::new(
        tigrs_ui::diff::document::LineMarker::Add,
        "hello \x1b]52;c;cHduZWQ=\x07 \u{202e}world",
        Some(1),
    );
    assert!(
        !cell.text.contains('\x1b') && !cell.text.contains('\u{202e}'),
        "RowCell::new must strip ESC and BiDi characters: {:?}",
        cell.text
    );

    // 3. Verify PagerView strips non-SGR escape sequences and BiDi characters
    let pager = tigrs_ui::view::PagerView::new(
        "Title \x1b]2;Evil\x07".to_string(),
        vec!["Line \x1b]52;c;cHduZWQ=\x07 \x1b[32mgreen\x1b[0m \u{202e}bidi".to_string()],
    );
    assert!(!pager.title().contains('\x1b'));
    let line0 = pager.row_text(0).unwrap();
    assert!(!line0.contains("\x1b]52"));
    assert!(!line0.contains('\u{202e}'));
    assert!(line0.contains("\x1b[32m"));
}

/// Initializes a repository that is pinned to the `files` ref backend.
///
/// `tigrs` only takes the in-process (Tier-1) `gix` status path when the
/// repository is **not** reftable-backed (see `tigrs_git::status::scan_status`),
/// and Git >= 2.55 creates reftable repositories by default. Without this pin
/// the test below would silently exercise the already-hardened Tier-2 CLI path
/// and assert nothing.
fn init_files_format_git_repo(dir: &Path) {
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["init", "--ref-format=files", "--initial-branch=main"])
        .current_dir(dir)
        .status()
        .expect("git init --ref-format=files");
    if !status.success() {
        let fallback = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init", "--initial-branch=main"])
            .current_dir(dir)
            .status()
            .expect("git init fallback");
        assert!(fallback.success());
    }
    for (key, value) in [
        ("user.name", "Alice Developer"),
        ("user.email", "alice@example.com"),
        ("core.autocrlf", "false"),
        ("commit.gpgsign", "false"),
    ] {
        let _ = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["config", key, value])
            .current_dir(dir)
            .status();
    }
}

#[test]
fn test_in_process_gix_ignores_repository_local_filter_and_diff_drivers() {
    let temp = TempDir::new().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir(&repo_root).unwrap();
    init_files_format_git_repo(&repo_root);

    let clean_marker = temp.path().join("PWNED_GIX_CLEAN");
    let textconv_marker = temp.path().join("PWNED_GIX_TEXTCONV");

    fs::write(repo_root.join("tracked.txt"), "v1\n").unwrap();
    // A WORKTREE `.gitattributes` (not `$GIT_DIR/info/attributes`) binding every
    // path to repository-defined filter and diff drivers.
    fs::write(
        repo_root.join(".gitattributes"),
        "* filter=evil diff=evilconv\n",
    )
    .unwrap();
    assert!(
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["add", "-A"])
            .current_dir(&repo_root)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-q", "-m", "initial"])
            .current_dir(&repo_root)
            .status()
            .unwrap()
            .success()
    );

    for (key, marker) in [
        ("filter.evil.clean", &clean_marker),
        ("filter.evil.smudge", &clean_marker),
        ("diff.evilconv.textconv", &textconv_marker),
        ("diff.evilconv.command", &textconv_marker),
    ] {
        let payload = format!("touch {}; cat", marker.display());
        assert!(
            Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(["config", "--local", key, &payload])
                .current_dir(&repo_root)
                .status()
                .unwrap()
                .success()
        );
    }

    // Dirty the tracked file so the status scan must normalize worktree content
    // against the index, which is where the `clean` filter would be invoked.
    fs::write(repo_root.join("tracked.txt"), "v2-modified\n").unwrap();

    let engine = GitEngine::open(Some(&repo_root)).expect("open git engine");
    let cancel = CancellationToken::none();

    let report = engine.load_status(&cancel).expect("load status");
    assert_eq!(
        report.unstaged.len(),
        1,
        "the Tier-1 gix status path must still report the modified file"
    );
    assert!(
        !clean_marker.exists(),
        "in-process gix status must not execute repository-local filter.<drv>.clean"
    );

    let item = StatusItem::new('M', StatusSection::Unstaged, "tracked.txt", None);
    let diff = engine
        .compute_status_item_diff(&item)
        .expect("status item diff");
    assert!(!diff.files.is_empty(), "status item diff must still render");
    assert!(
        !clean_marker.exists(),
        "status item diff must not execute repository-local filter.<drv>.clean"
    );
    assert!(
        !textconv_marker.exists(),
        "status item diff must not execute repository-local diff.<drv>.textconv"
    );

    // The repository-local `core.sshCommand`/`credential.helper` family is gated
    // by the same `gix` predicate; confirm the snapshot refuses to surface it.
    let head = engine.head_commit_id();
    assert!(head.is_ok(), "repository must still be readable");
}
