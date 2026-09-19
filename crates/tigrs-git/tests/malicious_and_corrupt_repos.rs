// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Integration test suite for hostile repositories, corrupt Git objects,
//! and soak/stress stability.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use tigrs_core::cancel::CancellationToken;
use tigrs_core::error::TigError;
use tigrs_git::GitEngine;
use tigrs_git::path_security::{verify_relative_path, verify_worktree_path_safety};
use tigrs_git::stage::{discard_untracked_file, stage_file};
use tigrs_git::types::CommitSummary;

/// Helper initializing a fresh git repository in a temp directory.
fn init_git_repo(dir: &Path) {
    let output = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["init", "--initial-branch=main"])
        .current_dir(dir)
        .output()
        .expect("git init");
    assert!(output.status.success());

    // Configure user identity
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "user.name", "Test User"])
        .current_dir(dir)
        .output();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(dir)
        .output();
}

/// Helper to collect commits synchronously using `stream_commits`.
fn collect_commits(engine: &GitEngine, limit: usize) -> Vec<CommitSummary> {
    let (_, token) = CancellationToken::new();
    let iter = engine
        .stream_commits(None, Some(limit), token)
        .expect("stream_commits");
    let mut out = Vec::new();
    for batch in iter {
        out.extend(batch.expect("commit batch"));
        if out.len() >= limit {
            break;
        }
    }
    out
}

#[test]
fn test_malicious_symlink_traversal_attacks() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    init_git_repo(root);

    // 1. Outside absolute symlink
    let evil_abs = root.join("evil_abs");
    std::os::unix::fs::symlink("/etc/passwd", &evil_abs).unwrap();

    // 2. Relative escape symlink
    let evil_rel = root.join("evil_rel");
    std::os::unix::fs::symlink("../../../../../../../../etc/shadow", &evil_rel).unwrap();

    // 3. Circular symlink
    let loop_a = root.join("loop_a");
    let loop_b = root.join("loop_b");
    std::os::unix::fs::symlink("loop_b", &loop_a).unwrap();
    std::os::unix::fs::symlink("loop_a", &loop_b).unwrap();

    // verify_relative_path checks
    assert!(verify_relative_path("/etc/passwd").is_err());
    assert!(verify_relative_path("../etc/passwd").is_err());
    assert!(verify_relative_path("foo/../../etc/passwd").is_err());
    assert!(verify_relative_path("foo\0bar").is_err());

    // verify_worktree_path_safety checks
    let res_abs = verify_worktree_path_safety(root, "evil_abs");
    assert!(
        matches!(res_abs, Err(TigError::Security(_))),
        "Expected Security error for absolute symlink: {res_abs:?}"
    );

    let res_rel = verify_worktree_path_safety(root, "evil_rel");
    assert!(
        matches!(res_rel, Err(TigError::Security(_))),
        "Expected Security error for escaping symlink: {res_rel:?}"
    );

    // Staging / untracked file operations refuse to escape root
    assert!(stage_file(root, "../outside", None).is_err());
    assert!(stage_file(root, "/etc/passwd", None).is_err());
    assert!(discard_untracked_file(root, "evil_rel").is_err());
    assert!(discard_untracked_file(root, "evil_abs").is_err());
    assert!(discard_untracked_file(root, "../outside").is_err());
}

#[test]
fn test_corrupt_git_objects_graceful_handling() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    init_git_repo(root);

    // Commit 1 valid file
    fs::write(root.join("hello.txt"), "hello world\n").unwrap();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "hello.txt"])
        .current_dir(root)
        .output();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "initial commit"])
        .current_dir(root)
        .output();

    // Create an intentionally corrupted loose object in .git/objects/
    let objects_dir = root.join(".git/objects");
    let dummy_dir = objects_dir.join("aa");
    fs::create_dir_all(&dummy_dir).unwrap();
    // Write 16 bytes of garbage
    fs::write(
        dummy_dir.join("00000000000000000000000000000000000000"),
        b"garbage corrupted content\x00\xff",
    )
    .unwrap();

    // Open engine
    let engine = GitEngine::open(Some(root)).unwrap();

    // Revwalk should succeed on valid commit without crashing on corrupt loose objects
    let summaries = collect_commits(&engine, 10);
    assert_eq!(summaries.len(), 1);

    // Reading a nonexistent or corrupt OID should gracefully return Err, NEVER panic
    let corrupt_oid = gix::ObjectId::from_hex(b"aa00000000000000000000000000000000000000").unwrap();
    let blob_res = engine.read_blob(corrupt_oid, "corrupt.txt");
    assert!(
        blob_res.is_err(),
        "Reading corrupt object should return Err"
    );
}

#[test]
fn test_hostile_metadata_terminal_injection_sanitization() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    init_git_repo(root);

    // Commit with hostile terminal hijack sequences in subject and author name
    fs::write(root.join("test.txt"), "content\n").unwrap();
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "test.txt"])
        .current_dir(root)
        .output();

    let hostile_title = "Safe title \x1b]0;TitleHijack\x07\x1b[2J\x1b[H\rOverwritten!";
    let hostile_author = "Attacker \x1b[31mRed\x1b[0m\x08\x08";

    let output = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            &format!("user.name={hostile_author}"),
            "commit",
            "-m",
            hostile_title,
        ])
        .current_dir(root)
        .output()
        .expect("git commit with hostile metadata");
    assert!(output.status.success());

    let engine = GitEngine::open(Some(root)).unwrap();
    let (_, token) = CancellationToken::new();

    // Revwalk verification
    let summaries = collect_commits(&engine, 10);
    assert_eq!(summaries.len(), 1);

    let summary = &summaries[0];
    // Verify subject is sanitized (no OSC \x1b] or CSI \x1b[ or carriage return \r)
    assert!(
        !summary.summary.contains("\x1b]"),
        "OSC injection not sanitized in summary: {:?}",
        summary.summary
    );
    assert!(
        !summary.summary.contains("\x1b["),
        "CSI injection not sanitized in summary: {:?}",
        summary.summary
    );
    assert!(
        !summary.summary.contains('\r'),
        "Carriage return not sanitized in summary: {:?}",
        summary.summary
    );

    // Diff view verification
    let diff = engine.compute_commit_diff(summary.id).unwrap();
    assert!(
        !diff.title.contains("\x1b]"),
        "Commit title not sanitized: {:?}",
        diff.title
    );
    assert!(
        !diff.author_name.contains("\x1b["),
        "Author name not sanitized: {:?}",
        diff.author_name
    );

    // Status view verification
    let status_report = engine.load_status(&token).unwrap();
    for item in status_report
        .staged
        .iter()
        .chain(&status_report.unstaged)
        .chain(&status_report.untracked)
        .chain(&status_report.unmerged)
    {
        assert!(
            !item.path.contains('\x1b'),
            "Status item path contains escape sequence: {:?}",
            item.path
        );
    }
}

#[test]
fn test_soak_stress_commit_chain_and_rapid_traversal() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    init_git_repo(root);

    // Create 25 rapid sequential commits modifying and adding files
    for i in 1..=25 {
        let file_name = format!("file_{}.txt", i % 5);
        fs::write(root.join(&file_name), format!("iteration {i}\n")).unwrap();
        let _ = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["add", &file_name])
            .current_dir(root)
            .output();
        let _ = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", &format!("commit number {i}")])
            .current_dir(root)
            .output();
    }

    let engine = GitEngine::open(Some(root)).unwrap();

    // 1. Full revwalk
    let summaries = collect_commits(&engine, 100);
    assert_eq!(summaries.len(), 25);

    // 2. Tree navigation across commits
    for summary in summaries.iter().take(10) {
        let tree_listing = engine.read_tree(summary.id, "");
        assert!(tree_listing.is_ok());
        let tree = tree_listing.unwrap();
        assert!(!tree.entries.is_empty());
    }

    // 3. Diff computation across commits
    for summary in summaries.iter().take(10) {
        let diff = engine.compute_commit_diff(summary.id);
        assert!(diff.is_ok());
    }

    // 4. In-process Blame computation
    let head_summary = &summaries[0];
    let blame = engine.blame_file(head_summary.id, "file_1.txt");
    assert!(blame.is_ok());
}
