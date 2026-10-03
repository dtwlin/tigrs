// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Integration coverage for whole-section worktree diffs.
//!
//! These back the synthetic "Untracked/Unstaged/Staged changes" rows of the main
//! view. A real repository is required because the tracked sections are produced
//! by `git diff` / `git diff --cached`.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;
use tigrs_core::cancel::CancellationToken;
use tigrs_git::{GitEngine, StatusSection};

fn git(path: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .current_dir(path)
        .status()
        .expect("failed to run git")
        .success();
    assert!(ok, "git {args:?} failed");
}

/// Creates a repository with one committed file, then leaves a staged change, an
/// unstaged change, and two untracked files in the worktree.
fn dirty_repo(path: &Path) {
    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.email", "section@example.com"]);
    git(path, &["config", "user.name", "Section Tester"]);

    fs::write(path.join("tracked.txt"), "one\ntwo\nthree\n").expect("write");
    fs::write(path.join("other.txt"), "alpha\n").expect("write");
    git(path, &["add", "tracked.txt", "other.txt"]);
    git(path, &["commit", "-m", "initial"]);

    // Staged modification.
    fs::write(path.join("tracked.txt"), "one\nTWO\nthree\n").expect("write");
    git(path, &["add", "tracked.txt"]);

    // Unstaged modification to a different file.
    fs::write(path.join("other.txt"), "ALPHA\n").expect("write");

    // Untracked files.
    fs::write(path.join("new_a.txt"), "fresh a\n").expect("write");
    fs::write(path.join("new_b.txt"), "fresh b\n").expect("write");
}

#[test]
fn test_section_diffs_cover_every_file_in_the_section() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path();
    dirty_repo(path);

    let engine = GitEngine::open(Some(path)).expect("open repo");
    let (_src, token) = CancellationToken::new();
    let report = engine.load_status(&token).expect("status");

    let staged = engine
        .compute_status_section_diff(StatusSection::Staged, &report.staged)
        .expect("staged diff");
    assert_eq!(&*staged.title, "Staged changes");
    assert_eq!(staged.files.len(), 1);
    assert_eq!(staged.files[0].path, "tracked.txt");

    let unstaged = engine
        .compute_status_section_diff(StatusSection::Unstaged, &report.unstaged)
        .expect("unstaged diff");
    assert_eq!(&*unstaged.title, "Unstaged changes");
    assert_eq!(unstaged.files.len(), 1);
    assert_eq!(unstaged.files[0].path, "other.txt");

    // The untracked section has no `git diff` equivalent: each file is rendered
    // as a synthetic all-additions diff, and all of them must be present.
    let untracked = engine
        .compute_status_section_diff(StatusSection::Untracked, &report.untracked)
        .expect("untracked diff");
    assert_eq!(&*untracked.title, "Untracked changes");
    assert_eq!(untracked.files.len(), 2);
    let mut paths: Vec<&str> = untracked.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, vec!["new_a.txt", "new_b.txt"]);
    assert_eq!(untracked.stats.insertions, 2);
}

#[test]
fn test_section_diff_is_empty_when_section_is_empty() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path();
    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.email", "section@example.com"]);
    git(path, &["config", "user.name", "Section Tester"]);
    fs::write(path.join("a.txt"), "a\n").expect("write");
    git(path, &["add", "a.txt"]);
    git(path, &["commit", "-m", "initial"]);

    let engine = GitEngine::open(Some(path)).expect("open repo");

    let staged = engine
        .compute_status_section_diff(StatusSection::Staged, &[])
        .expect("staged diff");
    assert!(staged.files.is_empty());
    assert_eq!(staged.stats.files_changed, 0);

    let untracked = engine
        .compute_status_section_diff(StatusSection::Untracked, &[])
        .expect("untracked diff");
    assert!(untracked.files.is_empty());
}

#[test]
#[cfg(unix)]
fn test_type_changed_regular_file_to_symlink_status_and_diff() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path();
    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.email", "section@example.com"]);
    git(path, &["config", "user.name", "Section Tester"]);

    fs::write(path.join("target.txt"), "real content\n").expect("write target");
    fs::write(path.join("entry.txt"), "regular file content\n").expect("write entry");
    git(path, &["add", "target.txt", "entry.txt"]);
    git(path, &["commit", "-m", "initial regular files"]);

    // Replace regular file `entry.txt` with a Unix symlink pointing to `target.txt`
    fs::remove_file(path.join("entry.txt")).expect("remove regular file");
    std::os::unix::fs::symlink("target.txt", path.join("entry.txt")).expect("create symlink");

    let engine = GitEngine::open(Some(path)).expect("open repo");
    let (_src, token) = CancellationToken::new();
    let report = engine.load_status(&token).expect("load_status");

    assert_eq!(report.unstaged.len(), 1);
    assert_eq!(report.unstaged[0].path, "entry.txt");
    assert_eq!(report.unstaged[0].status_code, 'T');

    let item_diff = engine
        .compute_status_item_diff(&report.unstaged[0])
        .expect("compute_status_item_diff on TypeChanged item");
    // Git emits a 2-part diff for TypeChanged (100644 deletion + 120000 symlink creation)
    assert_eq!(item_diff.files.len(), 2);
    assert!(item_diff.files.iter().all(|f| f.path == "entry.txt"));

    let section_diff = engine
        .compute_status_section_diff(StatusSection::Unstaged, &report.unstaged)
        .expect("compute_status_section_diff on TypeChanged section");
    assert_eq!(section_diff.files.len(), 2);
    assert!(section_diff.files.iter().all(|f| f.path == "entry.txt"));
}
