// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Integration coverage for repositories using the reftable ref backend.
//!
//! `gix` cannot read `HEAD` from reftable storage, so every code path that
//! resolves references has to route through the engine's Git CLI fallback.
//! These tests drive the same sequence the `tigrs` binary uses, which is what
//! distinguishes them from unit tests that pass an already-resolved commit id.

use std::path::Path;
use std::process::Command;
use tempfile::tempdir;
use tigrs_core::cancel::CancellationToken;
use tigrs_git::{CommitSummary, GitEngine};

/// Initializes a reftable repository, returning `false` when the local `git`
/// is too old to support the backend (the test then skips).
fn init_reftable_repo(path: &Path) -> bool {
    let Ok(out) = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["init", "-b", "main", "--ref-format=reftable"])
        .current_dir(path)
        .output()
    else {
        return false; // git binary not available
    };

    if !out.status.success() {
        eprintln!("git does not support --ref-format=reftable on this system, skipping test");
        return false;
    }

    for args in [
        ["config", "user.email", "reftable-test@example.com"],
        ["config", "user.name", "Reftable Tester"],
    ] {
        let ok = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(path)
            .status()
            .expect("git config failed")
            .success();
        assert!(ok, "git {args:?} failed");
    }

    true
}

fn commit(path: &Path, message: &str) {
    let ok = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "--allow-empty", "-m", message])
        .current_dir(path)
        .status()
        .expect("git commit failed")
        .success();
    assert!(ok, "git commit failed");
}

fn collect_commits<I>(stream: I) -> Vec<CommitSummary>
where
    I: Iterator<Item = tigrs_core::error::Result<Vec<CommitSummary>>>,
{
    stream
        .flat_map(|batch| batch.expect("revwalk batch failed"))
        .collect()
}

#[test]
fn test_reftable_repo_dynamic_fixture() {
    let dir = tempdir().expect("failed to create tempdir");
    let path = dir.path();
    if !init_reftable_repo(path) {
        return;
    }
    commit(path, "initial reftable test commit");

    let engine = GitEngine::open(Some(path)).expect("failed to open reftable repo");
    assert!(
        engine.info().is_reftable,
        "repo should be detected as reftable"
    );

    let head_id = engine
        .head_commit_id()
        .expect("reftable head commit resolution failed");
    let branch = engine
        .current_branch()
        .expect("reftable branch resolution failed");
    assert_eq!(branch, "main", "branch name should be main");

    let (_src, token) = CancellationToken::new();
    let stream = engine
        .stream_commits(Some(head_id), Some(50), token)
        .expect("revwalk failed");
    let batches: Vec<_> = stream.collect();
    assert!(!batches.is_empty(), "should walk at least one batch");
    let first_batch = batches[0].as_ref().unwrap();
    assert!(!first_batch.is_empty());
    assert_eq!(
        first_batch[0].summary.as_ref(),
        "initial reftable test commit"
    );
    assert_eq!(first_batch[0].author_name.as_ref(), "Reftable Tester");
}

/// Regression: launching `tigrs` with no arguments reported "0 commits loaded"
/// on reftable repositories because the implicit `HEAD` tip was resolved by
/// `gix`, which cannot instantiate `HEAD` on that backend.
#[test]
fn test_reftable_default_log_walks_head_without_arguments() {
    let dir = tempdir().expect("failed to create tempdir");
    let path = dir.path();
    if !init_reftable_repo(path) {
        return;
    }
    commit(path, "first reftable commit");
    commit(path, "second reftable commit");

    let engine = GitEngine::open(Some(path)).expect("failed to open reftable repo");
    assert!(engine.info().is_reftable);

    // Exactly the sequence the binary performs for a bare `tigrs` invocation.
    let spec = engine
        .parse_rev_args(&[])
        .expect("parsing empty arguments failed");
    assert!(
        spec.included.is_empty(),
        "an argument-less spec carries no explicit tips"
    );

    let (_src, token) = CancellationToken::new();
    let commits = collect_commits(
        engine
            .stream_commits_spec(spec, Some(50), token)
            .expect("default revwalk failed"),
    );

    assert_eq!(commits.len(), 2, "the full history must be walked");
    assert_eq!(commits[0].summary.as_ref(), "second reftable commit");
    assert_eq!(commits[1].summary.as_ref(), "first reftable commit");
}

/// A reftable repository without commits must open and render an empty log,
/// exactly like one using the files backend, rather than failing to start.
#[test]
fn test_reftable_repo_without_commits_opens_with_empty_log() {
    let dir = tempdir().expect("failed to create tempdir");
    let path = dir.path();
    if !init_reftable_repo(path) {
        return;
    }

    let engine = GitEngine::open(Some(path)).expect("an unborn reftable repo must still open");
    assert_eq!(
        engine.head_commit_id_opt().expect("HEAD lookup failed"),
        None,
        "an unborn branch has no commit"
    );
    assert!(
        engine.head_commit_id().is_err(),
        "callers demanding a commit still get an error"
    );
    assert_eq!(
        engine.current_branch().expect("branch lookup failed"),
        "main"
    );

    let spec = engine
        .resolve_spec_tips(engine.parse_rev_args(&[]).expect("parse args"))
        .expect("tip resolution must succeed on an unborn branch");
    assert!(spec.included.is_empty(), "no tips to walk");

    let (_src, token) = CancellationToken::new();
    let commits = collect_commits(
        engine
            .stream_commits_spec(spec, Some(50), token)
            .expect("walking an empty history must not fail"),
    );
    assert!(commits.is_empty());
}
