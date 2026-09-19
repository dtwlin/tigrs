// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Suite 2: Revwalk, Pathspec Streaming Yield/Cancellation, Complex DAG Topologies,
//! and `GraphOidTable` (Commit-Graph mmap vs Synthetic Fallback) Parity & Stability Tests.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use tigrs_core::cancel::CancellationToken;
use tigrs_git::graph_table::GraphOidTable;
use tigrs_git::{GitEngine, RevwalkSpec, stream_commit_chunks_with_spec};

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| panic!("failed to execute git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed with {status}");
}

fn init_test_repo(dir: &Path) {
    run_git(dir, &["init", "-b", "main"]);
    run_git(dir, &["config", "user.name", "SQE Architect"]);
    run_git(dir, &["config", "user.email", "sqe@linux.org"]);
}

#[test]
fn test_sparse_pathspec_partial_batch_yield_and_cancellation() {
    use std::fmt::Write as _;
    let temp = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_test_repo(repo_dir);

    // Use `git fast-import` to generate 280 commits in < 10ms:
    // Commit #1 modifies `sparse_target.rs`.
    // Commits #2..=280 modify `churn.txt`.
    let mut fast_import = String::with_capacity(64 * 1024);
    let body1 = "pub fn target() {}\n";
    let msg1 = "Commit 1: sparse target";
    let _ = write!(
        fast_import,
        "blob\nmark :1\ndata {}\n{}\ncommit refs/heads/main\nmark :2\nauthor SQE <sqe@linux.org> 1700000001 +0000\ncommitter SQE <sqe@linux.org> 1700000001 +0000\ndata {}\n{}\nM 100644 :1 sparse_target.rs\n",
        body1.len(),
        body1,
        msg1.len(),
        msg1
    );

    let mut prev_commit_mark = 2usize;
    let mut next_mark = 3usize;
    for i in 2..=280 {
        let blob_mark = next_mark;
        let commit_mark = next_mark + 1;
        next_mark += 2;

        let body = format!("churn line {i}\n");
        let msg = format!("Churn commit {i}");
        let ts = 1_700_000_000 + i;
        let _ = write!(
            fast_import,
            "blob\nmark :{blob_mark}\ndata {}\n{}\ncommit refs/heads/main\nmark :{commit_mark}\nauthor SQE <sqe@linux.org> {ts} +0000\ncommitter SQE <sqe@linux.org> {ts} +0000\ndata {}\n{}\nfrom :{prev_commit_mark}\nM 100644 :{blob_mark} churn.txt\n",
            body.len(),
            body,
            msg.len(),
            msg
        );
        prev_commit_mark = commit_mark;
    }

    let mut child = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["fast-import", "--quiet"])
        .current_dir(repo_dir)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn git fast-import");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(fast_import.as_bytes())
        .unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());

    let engine = GitEngine::open(Some(repo_dir)).expect("open repo");
    let spec = engine
        .parse_rev_args(&[
            "HEAD".to_string(),
            "--".to_string(),
            "sparse_target.rs".to_string(),
        ])
        .expect("parse_rev_args");

    assert_eq!(spec.pathspecs, vec!["sparse_target.rs".to_string()]);
    assert!(!spec.is_full_history());

    // Request chunk_size = 50, even though only 1 commit matches across 280 commits.
    // The iterator MUST yield the partial batch promptly after scanning 256 commits
    // and finishing the walk without stalling.
    let (_src, token) = CancellationToken::new();
    let mut stream =
        stream_commit_chunks_with_spec(engine.repository(), spec.clone(), 50, token).unwrap();

    let first_batch = stream
        .next()
        .expect("stream must yield at least one batch")
        .expect("batch must not error");
    assert_eq!(
        first_batch.len(),
        1,
        "Partial batch must contain exactly the 1 matching sparse commit"
    );
    assert_eq!(first_batch[0].summary.as_ref(), "Commit 1: sparse target");
    assert!(
        stream.next().is_none(),
        "Stream must terminate cleanly after yielding sparse matches"
    );

    // Test immediate cancellation mid-walk
    let (cancel_src, cancel_token) = CancellationToken::new();
    cancel_src.cancel();
    let mut cancelled_stream =
        stream_commit_chunks_with_spec(engine.repository(), spec, 50, cancel_token).unwrap();
    let res = cancelled_stream.next();
    assert!(
        res.is_none() || matches!(res, Some(Err(_))),
        "Cancelled stream must abort immediately"
    );
}

#[test]
fn test_revwalk_range_exclusions_and_multi_pathspecs() {
    let temp = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_test_repo(repo_dir);

    fs::create_dir_all(repo_dir.join("src/core")).unwrap();
    fs::create_dir_all(repo_dir.join("docs")).unwrap();

    // Base commit on main
    fs::write(repo_dir.join("src/core/lib.rs"), "// base\n").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "Base commit"]);

    // Create feature branch
    run_git(repo_dir, &["checkout", "-b", "feature"]);
    fs::write(repo_dir.join("src/core/lib.rs"), "// feature core change\n").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "Feature core commit"]);

    fs::write(repo_dir.join("docs/README.md"), "# Feature Docs\n").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "Feature docs commit"]);

    // Switch back to main and add a main-only commit
    run_git(repo_dir, &["checkout", "main"]);
    fs::write(repo_dir.join("main_only.txt"), "main only\n").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "Main only commit"]);

    let engine = GitEngine::open(Some(repo_dir)).expect("open repo");

    // 1. Test range `main..feature` (should yield the 2 feature commits, excluding Base and Main only)
    let range_spec = engine
        .parse_rev_args(&["main..feature".to_string()])
        .expect("parse range");
    assert_eq!(range_spec.included.len(), 1);
    assert_eq!(range_spec.excluded.len(), 1);

    let (_src, token) = CancellationToken::new();
    let mut commits = Vec::new();
    for chunk in engine
        .stream_commits_spec(range_spec.clone(), Some(10), token)
        .unwrap()
    {
        commits.extend(chunk.unwrap());
    }
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].summary.as_ref(), "Feature docs commit");
    assert_eq!(commits[1].summary.as_ref(), "Feature core commit");

    // 2. Test range + directory pathspec `main..feature -- src/core`
    let path_range_spec = engine
        .parse_rev_args(&[
            "main..feature".to_string(),
            "--".to_string(),
            "src/core".to_string(),
        ])
        .expect("parse range with pathspec");
    let (_src2, token2) = CancellationToken::new();
    let mut filtered_commits = Vec::new();
    for chunk in engine
        .stream_commits_spec(path_range_spec, Some(10), token2)
        .unwrap()
    {
        filtered_commits.extend(chunk.unwrap());
    }
    assert_eq!(filtered_commits.len(), 1);
    assert_eq!(filtered_commits[0].summary.as_ref(), "Feature core commit");
}

#[test]
fn test_complex_dag_octopus_merge_and_commit_graph_mmap_parity() {
    let temp = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_test_repo(repo_dir);

    // Root commit
    fs::write(repo_dir.join("root.txt"), "root\n").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "Root commit"]);

    // Create branch_b and branch_c
    run_git(repo_dir, &["checkout", "-b", "branch_b"]);
    fs::write(repo_dir.join("b.txt"), "branch b\n").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "Branch B commit"]);

    run_git(repo_dir, &["checkout", "main"]);
    run_git(repo_dir, &["checkout", "-b", "branch_c"]);
    fs::write(repo_dir.join("c.txt"), "branch c\n").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "Branch C commit"]);

    // Switch to main, add a commit, and perform a 3-parent Octopus Merge (`git merge branch_b branch_c`)
    run_git(repo_dir, &["checkout", "main"]);
    fs::write(repo_dir.join("main.txt"), "main\n").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "Main diverged commit"]);

    run_git(
        repo_dir,
        &[
            "merge",
            "branch_b",
            "branch_c",
            "-m",
            "Octopus merge commit",
        ],
    );

    let engine = GitEngine::open(Some(repo_dir)).expect("open repo");
    let spec = engine
        .resolve_spec_tips(RevwalkSpec::default())
        .expect("resolve HEAD");
    let (_src, token) = CancellationToken::new();

    let mut all_commits = Vec::new();
    for chunk in engine.stream_commits_spec(spec, Some(10), token).unwrap() {
        all_commits.extend(chunk.unwrap());
    }

    assert_eq!(all_commits.len(), 5);
    let octopus = &all_commits[0];
    assert_eq!(octopus.summary.as_ref(), "Octopus merge commit");
    assert_eq!(
        octopus.parents.len(),
        3,
        "Octopus merge must preserve all 3 parent ObjectIds"
    );

    // 1. Test GraphOidTable before commit-graph exists
    let common_dir = repo_dir.join(".git");
    let synth_table = GraphOidTable::open(&common_dir);
    assert!(
        !synth_table.is_commitgraph(),
        "Must report no commit-graph before commit-graph write"
    );
    assert!(synth_table.is_empty());

    // 2. Write real `.git/objects/info/commit-graph` and test zero-copy mmap mode
    run_git(repo_dir, &["config", "core.commitGraph", "true"]);
    run_git(repo_dir, &["commit-graph", "write", "--reachable"]);
    let mmap_table = GraphOidTable::open(&common_dir);
    assert!(
        mmap_table.is_commitgraph(),
        "Must open in zero-copy CommitGraph mmap mode after git commit-graph write"
    );
    assert_eq!(mmap_table.len(), 5);
}

#[test]
fn test_criss_cross_merge_and_symmetric_difference_revwalk() {
    let temp = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_test_repo(repo_dir);

    // Base commit
    fs::write(repo_dir.join("base.txt"), "base\n").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "Base"]);

    // Create branch_a and branch_b
    run_git(repo_dir, &["checkout", "-b", "branch_a"]);
    fs::write(repo_dir.join("a1.txt"), "a1\n").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "A1"]);

    run_git(repo_dir, &["checkout", "main"]);
    run_git(repo_dir, &["checkout", "-b", "branch_b"]);
    fs::write(repo_dir.join("b1.txt"), "b1\n").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "B1"]);

    // Criss-cross merge:
    // branch_a merges branch_b (commit M_A), while branch_b merges A1 (commit M_B)
    run_git(repo_dir, &["checkout", "branch_a"]);
    run_git(repo_dir, &["merge", "branch_b", "-m", "Merge B into A"]);

    run_git(repo_dir, &["checkout", "branch_b"]);
    run_git(repo_dir, &["merge", "branch_a~1", "-m", "Merge A1 into B"]);

    let engine = GitEngine::open(Some(repo_dir)).expect("open repo");

    // Symmetric difference `branch_a...branch_b` includes both tips
    let sym_spec = engine
        .parse_rev_args(&["branch_a...branch_b".to_string()])
        .expect("parse symmetric diff");
    assert_eq!(sym_spec.included.len(), 2);

    let (_src, token) = CancellationToken::new();
    let mut commits = Vec::new();
    for chunk in engine
        .stream_commits_spec(sym_spec, Some(10), token)
        .unwrap()
    {
        commits.extend(chunk.unwrap());
    }
    // Should include Merge B into A, Merge A1 into B, A1, B1, Base (5 commits total)
    assert_eq!(commits.len(), 5);
}
