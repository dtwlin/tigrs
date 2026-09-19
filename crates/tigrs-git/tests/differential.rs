// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Differential test harness: the semantic correctness gate for the diff engine.
//!
//! Every assertion here compares `tigrs`'s rendered patch byte-for-byte against
//! the output of the installed `git` binary for the same commit. The diff engine
//! is the foundation that hunk staging will later *write* through, so a
//! divergence here is not cosmetic — it is a correctness bug waiting to corrupt
//! a working tree.
//!
//! Reference output uses `--full-index` so the comparison does not depend on
//! Git's repository-size-sensitive abbreviation heuristics, and `--no-commit-id`
//! so only the patch body is compared.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;
use tigrs_git::{IndexAbbrev, compute_commit_diff, format_patch};

/// Runs a git command in `dir`, asserting success, and returns stdout as bytes.
fn git_bytes(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("failed to run git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// Runs a git command in `dir`, asserting success, and returns stdout as a String.
fn git(dir: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git_bytes(dir, args))
        .trim_end_matches('\n')
        .to_string()
}

/// Runs a git command, ignoring failure (used for setup steps that may no-op).
fn git_ok(dir: &Path, args: &[&str]) {
    let _ = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .current_dir(dir)
        .output();
}

/// Reference patch text produced by the real `git` binary for `commit`.
///
/// Root commits need `--root`; every other commit is compared against its first
/// parent, which is the same basis `compute_commit_diff` uses.
fn git_reference_patch(dir: &Path, commit: &str) -> String {
    let parents = git(dir, &["rev-list", "--parents", "-n", "1", commit]);
    let fields: Vec<&str> = parents.split_whitespace().collect();

    let base_args = [
        "-c",
        "core.quotePath=true",
        "diff-tree",
        "-p",
        "--no-commit-id",
        "--full-index",
        "--no-color",
        "-M",
        "--no-ext-diff",
        "--no-textconv",
    ];

    let mut args: Vec<&str> = base_args.to_vec();
    if fields.len() <= 1 {
        args.push("--root");
        args.push(commit);
    } else {
        args.push(fields[1]);
        args.push(commit);
    }

    String::from_utf8_lossy(&git_bytes(dir, &args)).to_string()
}

/// Patch text produced by tigrs' in-process diff engine for `commit`.
fn tigrs_patch(dir: &Path, commit: &str) -> String {
    let hex = git(dir, &["rev-parse", commit]);
    let oid = gix::ObjectId::from_hex(hex.as_bytes()).expect("valid oid");
    let repo = gix::open(dir).expect("open repo");
    let diff = compute_commit_diff(&repo, oid).expect("compute diff");
    format_patch(&diff, IndexAbbrev::Full)
}

/// Escapes bytes that would otherwise be invisible in a terminal report.
///
/// Carriage returns and tabs are both semantically significant in patch output
/// and both vanish when printed raw, so they are rendered explicitly.
fn visible(s: &str) -> String {
    s.replace('\r', "<CR>").replace('\t', "<TAB>")
}

/// Renders a readable side-by-side report for a mismatching commit.
///
/// Splits on `\n` only; `str::lines()` would strip trailing `\r` and hide
/// exactly the class of divergence the CRLF fixtures exist to catch.
fn mismatch_report(label: &str, commit: &str, expected: &str, actual: &str) -> String {
    let exp: Vec<&str> = expected.split('\n').collect();
    let act: Vec<&str> = actual.split('\n').collect();
    let mut s = format!("\n=== DIVERGENCE [{label}] commit {commit} ===\n");
    let max = exp.len().max(act.len());
    for i in 0..max {
        let e = exp.get(i).copied().unwrap_or("<EOF>");
        let a = act.get(i).copied().unwrap_or("<EOF>");
        if e == a {
            let _ = writeln!(s, "  {}", visible(e));
        } else {
            let _ = writeln!(s, "git  | {}", visible(e));
            let _ = writeln!(s, "tigrs| {}", visible(a));
        }
    }
    s
}

/// Asserts tigrs and git agree on every commit reachable from HEAD.
fn assert_all_commits_match(dir: &Path, label: &str) {
    let revs = git(dir, &["rev-list", "--reverse", "HEAD"]);
    let mut checked = 0;
    for commit in revs.lines().filter(|l| !l.is_empty()) {
        let expected = git_reference_patch(dir, commit);
        let actual = tigrs_patch(dir, commit);
        assert_eq!(
            expected,
            actual,
            "{}",
            mismatch_report(label, commit, &expected, &actual)
        );
        checked += 1;
    }
    assert!(checked > 0, "{label}: no commits were checked");
}

/// Initializes a repository with deterministic identity and diff settings.
fn init_repo(dir: &Path) {
    git_ok(dir, &["init", "-b", "main"]);
    git_ok(dir, &["config", "user.name", "Tigrs Tester"]);
    git_ok(dir, &["config", "user.email", "tester@example.com"]);
    git_ok(dir, &["config", "commit.gpgsign", "false"]);
    git_ok(dir, &["config", "core.autocrlf", "false"]);
}

/// Stages everything and commits with the given message.
fn commit_all(dir: &Path, msg: &str) {
    git_ok(dir, &["add", "-A"]);
    git_ok(dir, &["commit", "-m", msg]);
}

#[test]
fn test_differential_basic_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    std::fs::write(d.join("a.txt"), "line1\nline2\nline3\n").unwrap();
    commit_all(d, "add a");

    std::fs::write(d.join("a.txt"), "line1\nCHANGED\nline3\n").unwrap();
    commit_all(d, "modify a");

    std::fs::write(d.join("b.txt"), "b\n").unwrap();
    commit_all(d, "add b");

    std::fs::remove_file(d.join("a.txt")).unwrap();
    commit_all(d, "delete a");

    assert_all_commits_match(d, "basic-lifecycle");
}

#[test]
fn test_differential_single_line_files() {
    // Exercises the `@@ -1 +1 @@` form, where Git omits the unit line count.
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    std::fs::write(d.join("one.txt"), "only\n").unwrap();
    commit_all(d, "add single line");

    std::fs::write(d.join("one.txt"), "changed\n").unwrap();
    commit_all(d, "change single line");

    assert_all_commits_match(d, "single-line");
}

#[test]
fn test_differential_missing_trailing_newline() {
    // The `\ No newline at end of file` marker, in every position it can occur.
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    std::fs::write(d.join("n.txt"), "no trailing newline").unwrap();
    commit_all(d, "add without newline");

    std::fs::write(d.join("n.txt"), "no trailing newline now changed").unwrap();
    commit_all(d, "modify still without newline");

    std::fs::write(d.join("n.txt"), "now it has one\n").unwrap();
    commit_all(d, "add trailing newline");

    std::fs::write(d.join("n.txt"), "and removed again").unwrap();
    commit_all(d, "remove trailing newline");

    std::fs::write(d.join("multi.txt"), "a\nb\nc").unwrap();
    commit_all(d, "multiline without trailing newline");

    std::fs::write(d.join("multi.txt"), "a\nB\nc").unwrap();
    commit_all(d, "edit middle, still no trailing newline");

    assert_all_commits_match(d, "no-trailing-newline");
}

#[test]
fn test_differential_empty_files() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    std::fs::write(d.join("empty.txt"), "").unwrap();
    commit_all(d, "add empty file");

    std::fs::write(d.join("empty.txt"), "content\n").unwrap();
    commit_all(d, "fill empty file");

    std::fs::write(d.join("empty.txt"), "").unwrap();
    commit_all(d, "empty it again");

    assert_all_commits_match(d, "empty-files");
}

#[test]
fn test_differential_mode_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    let script = d.join("s.sh");
    std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
    commit_all(d, "add script");

    // Pure mode change: content identical, so Git emits no index line at all.
    git_ok(d, &["update-index", "--chmod=+x", "s.sh"]);
    git_ok(d, &["commit", "-m", "chmod +x"]);

    // Mode change combined with a content edit.
    std::fs::write(&script, "#!/bin/sh\necho bye\n").unwrap();
    git_ok(d, &["add", "-A"]);
    git_ok(d, &["update-index", "--chmod=-x", "s.sh"]);
    git_ok(d, &["commit", "-m", "chmod -x and edit"]);

    assert_all_commits_match(d, "mode-changes");
}

#[test]
fn test_differential_renames() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    let body: String = (0..50).fold(String::new(), |mut acc, i| {
        let _ = writeln!(acc, "line {i}");
        acc
    });
    std::fs::write(d.join("orig.txt"), &body).unwrap();
    commit_all(d, "add original");

    // Pure rename: 100% similarity.
    git_ok(d, &["mv", "orig.txt", "renamed.txt"]);
    commit_all(d, "pure rename");

    // Rename with edits: partial similarity percentage.
    git_ok(d, &["mv", "renamed.txt", "renamed2.txt"]);
    let edited = body.replace("line 7\n", "line SEVEN\n");
    std::fs::write(d.join("renamed2.txt"), &edited).unwrap();
    commit_all(d, "rename with edit");

    assert_all_commits_match(d, "renames");
}

#[test]
fn test_differential_binary_files() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    // NUL bytes in the first 8000 bytes are what make Git call a blob binary.
    let bin: Vec<u8> = (0u16..512).flat_map(|i| [0u8, (i % 251) as u8]).collect();
    std::fs::write(d.join("blob.bin"), &bin).unwrap();
    commit_all(d, "add binary");

    let mut bin2 = bin.clone();
    bin2.extend_from_slice(&[0, 1, 2, 3, 0, 9]);
    std::fs::write(d.join("blob.bin"), &bin2).unwrap();
    commit_all(d, "modify binary");

    std::fs::remove_file(d.join("blob.bin")).unwrap();
    commit_all(d, "delete binary");

    assert_all_commits_match(d, "binary");
}

#[test]
fn test_differential_symlinks_and_typechange() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    std::fs::write(d.join("target.txt"), "target content\n").unwrap();
    commit_all(d, "add target");

    std::os::unix::fs::symlink("target.txt", d.join("link")).unwrap();
    commit_all(d, "add symlink");

    // Symlink replaced by a regular file: an entry-type change.
    std::fs::remove_file(d.join("link")).unwrap();
    std::fs::write(d.join("link"), "now a real file\n").unwrap();
    commit_all(d, "symlink becomes file");

    assert_all_commits_match(d, "symlinks");
}

#[test]
fn test_differential_unusual_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    std::fs::create_dir_all(d.join("deep/nested/dir")).unwrap();
    std::fs::write(d.join("deep/nested/dir/file.txt"), "deep\n").unwrap();
    std::fs::write(d.join("with space.txt"), "spaced\n").unwrap();
    std::fs::write(d.join("üñïçødé.txt"), "unicode name\n").unwrap();
    std::fs::write(d.join("dash-and_under.txt"), "punct\n").unwrap();
    commit_all(d, "add unusual paths");

    std::fs::write(d.join("with space.txt"), "spaced changed\n").unwrap();
    std::fs::write(d.join("üñïçødé.txt"), "unicode changed\n").unwrap();
    commit_all(d, "modify unusual paths");

    assert_all_commits_match(d, "unusual-paths");
}

#[test]
fn test_differential_crlf_and_control_chars() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    std::fs::write(d.join("crlf.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
    commit_all(d, "add crlf");

    std::fs::write(d.join("crlf.txt"), "one\r\nTWO\r\nthree\r\n").unwrap();
    commit_all(d, "modify crlf");

    // A lone CR and a form feed are valid inside text blobs.
    std::fs::write(d.join("ctrl.txt"), "alpha\x0cbeta\ngamma\rdelta\n").unwrap();
    commit_all(d, "add control chars");

    assert_all_commits_match(d, "crlf-control");
}

#[test]
fn test_differential_line_ending_conversion_only() {
    // A pure LF -> CRLF conversion changes every line and nothing else. This is
    // the sharpest test of line tokenization: a tokenizer that folds `\r\n` and
    // `\n` into the same token sees no change at all and emits an empty patch,
    // while Git reports every line as replaced.
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    std::fs::write(d.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    commit_all(d, "lf endings");

    std::fs::write(d.join("f.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
    commit_all(d, "convert to crlf");

    std::fs::write(d.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    commit_all(d, "convert back to lf");

    // Mixed endings within one file.
    std::fs::write(d.join("f.txt"), "one\r\ntwo\nthree\r\n").unwrap();
    commit_all(d, "mixed endings");

    assert_all_commits_match(d, "line-ending-conversion");
}

#[test]
fn test_differential_multiple_hunks_and_context_merging() {
    // Verifies hunk splitting and the 3-line context window, including the case
    // where two edits are close enough that Git merges them into one hunk.
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    let original: String = (0..60).fold(String::new(), |mut acc, i| {
        let _ = writeln!(acc, "line {i}");
        acc
    });
    std::fs::write(d.join("big.txt"), &original).unwrap();
    commit_all(d, "add big file");

    // Far-apart edits produce separate hunks.
    let far = original
        .replace("line 5\n", "line FIVE\n")
        .replace("line 50\n", "line FIFTY\n");
    std::fs::write(d.join("big.txt"), &far).unwrap();
    commit_all(d, "two distant edits");

    // Edits 4 lines apart sit exactly at the context-merge boundary.
    let near = far
        .replace("line 20\n", "line TWENTY\n")
        .replace("line 24\n", "line TWENTYFOUR\n");
    std::fs::write(d.join("big.txt"), &near).unwrap();
    commit_all(d, "two near edits");

    // Edits at the very start and very end exercise hunk clamping.
    let edges = near
        .replace("line 0\n", "line ZERO\n")
        .replace("line 59\n", "line FIFTYNINE\n");
    std::fs::write(d.join("big.txt"), &edges).unwrap();
    commit_all(d, "edge edits");

    assert_all_commits_match(d, "multi-hunk");
}

#[test]
fn test_differential_merge_commit_uses_first_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    std::fs::write(d.join("base.txt"), "base\n").unwrap();
    commit_all(d, "base");

    git_ok(d, &["checkout", "-b", "feature"]);
    std::fs::write(d.join("feature.txt"), "feature\n").unwrap();
    commit_all(d, "feature work");

    git_ok(d, &["checkout", "main"]);
    std::fs::write(d.join("main.txt"), "main\n").unwrap();
    commit_all(d, "main work");

    git_ok(d, &["merge", "--no-ff", "feature", "-m", "merge feature"]);

    assert_all_commits_match(d, "merge-first-parent");
}

#[test]
fn test_differential_many_files_single_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    for i in 0..40 {
        std::fs::write(d.join(format!("f{i:03}.txt")), format!("content {i}\n")).unwrap();
    }
    commit_all(d, "add many files");

    // Touch a subset so file ordering in the patch is exercised.
    for i in (0..40).step_by(3) {
        std::fs::write(d.join(format!("f{i:03}.txt")), format!("changed {i}\n")).unwrap();
    }
    commit_all(d, "modify subset");

    assert_all_commits_match(d, "many-files");
}

#[test]
fn test_differential_gitattributes_userdiff_drivers() {
    // The hunk header's function context comes from the language driver named
    // by the `diff` attribute, not from Git's generic matcher. Getting this
    // wrong is invisible in a repository without `.gitattributes` and wrong on
    // every hunk in one that has it.
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    std::fs::write(
        d.join(".gitattributes"),
        "*.rs diff=rust\n*.py diff=python\n*.c diff=cpp\n",
    )
    .unwrap();

    // In each file the line directly above the change is *not* what the
    // language driver picks, so a wrong matcher produces a wrong header rather
    // than no header.
    std::fs::write(
        d.join("lib.rs"),
        "impl Thread {\n    fn translate_object(&self) {\n        let a = 1;\n        let b = 2;\n        let c = 3;\n        let d = 4;\n    }\n}\n",
    )
    .unwrap();
    std::fs::write(
        d.join("app.py"),
        "class Handler:\n    def handle(self):\n        a = 1\n        b = 2\n        c = 3\n        d = 4\n",
    )
    .unwrap();
    std::fs::write(
        d.join("main.c"),
        "static int helper(int x)\n{\n\tint a = 1;\nout:\n\tint b = 2;\n\tint c = 3;\n\treturn x;\n}\n",
    )
    .unwrap();
    commit_all(d, "add sources");

    std::fs::write(
        d.join("lib.rs"),
        "impl Thread {\n    fn translate_object(&self) {\n        let a = 1;\n        let b = 22;\n        let c = 3;\n        let d = 4;\n    }\n}\n",
    )
    .unwrap();
    std::fs::write(
        d.join("app.py"),
        "class Handler:\n    def handle(self):\n        a = 1\n        b = 22\n        c = 3\n        d = 4\n",
    )
    .unwrap();
    std::fs::write(
        d.join("main.c"),
        "static int helper(int x)\n{\n\tint a = 1;\nout:\n\tint b = 22;\n\tint c = 3;\n\treturn x;\n}\n",
    )
    .unwrap();
    commit_all(d, "edit sources");

    assert_all_commits_match(d, "userdiff-drivers");
}

#[test]
fn test_differential_configured_xfuncname_overrides_builtin() {
    // `diff.<name>.xfuncname` from the repository config takes precedence over
    // a builtin driver of the same name.
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);
    git_ok(d, &["config", "diff.custom.xfuncname", "^SECTION .*$"]);

    std::fs::write(d.join(".gitattributes"), "*.txt diff=custom\n").unwrap();
    std::fs::write(
        d.join("doc.txt"),
        "SECTION one\nalpha\nbravo\ncharlie\ndelta\necho\nfoxtrot\n",
    )
    .unwrap();
    commit_all(d, "add doc");

    std::fs::write(
        d.join("doc.txt"),
        "SECTION one\nalpha\nbravo\nCHARLIE\ndelta\necho\nfoxtrot\n",
    )
    .unwrap();
    commit_all(d, "edit doc");

    assert_all_commits_match(d, "configured-xfuncname");
}

#[test]
fn test_differential_commit_messages_vs_git_log_format_b() {
    // Differential check that `compute_commit_diff` preserves 100% of the commit
    // message (subject, body paragraphs, continuation lines, and all trailer blocks)
    // when compared against `git log -1 --format=%B`.
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    let messages = [
        // 1. Multi-paragraph message with mixed continuation lines and trailers
        "net: socket: make receive ring buffer capacity configurable\n\n\
Previously the ring buffer used a fixed array of 4096 bytes,\n\
which could overflow under high packet burst loads.\n\n\
SKIP_CI: Flaky integration suite on arm64 runner\n\
Manual verification complete\n\
Tested: on local testbed https://ci.example.com/runs/10492\n\
with benchmark profile https://git.example.com/perf/profiles/42\n\
Issue-Id: 100200\n\
Issue-Id: 100201\n\
Change-Id: I1234567890abcdef1234567890abcdef12345678\n\
Signed-off-by: Alice Developer <alice@example.com>",
        // 2. Key=Value and multi-line indented list in trailer block
        "driver: memory: align controller register mappings with hardware spec\n\n\
Update the controller mock addresses to match the v2 hardware specification.\n\n\
SKIP_LINT=Temporary workaround for upstream header macro\n\
Issue-Id: 200300\n\
Tested: 1. cargo test --package tigrs-git\n\
        2. cargo test --package tigrs-ui\n\
Reviewed-by: Bob Reviewer <bob@example.com>\n\
Change-Id: Iabcdef1234567890abcdef1234567890abcdef12\n\
Signed-off-by: Carol Engineer <carol@example.com>",
        // 3. Commit whose body consists solely of trailers (no prose paragraph)
        "chore: bump crate version\n\n\
Issue-Id: 300400\n\
Change-Id: I1111111111111111111111111111111111111111\n\
Signed-off-by: Release Bot <bot@example.com>",
        // 4. Commit with multi-line first paragraph (no blank line after line 1)
        "first line of subject without blank line\n\
second line immediately below subject\n\
third line in first paragraph\n\n\
Trailer-Key: trailer value",
        // 5. Commit with colon-headed section in middle of body
        "metrics: support independent histogram threshold bins\n\n\
Fallback Mechanism:\n\
If independent bins are not configured for the target aggregation level,\n\
the collector automatically falls back to the default partition bins.\n\n\
Tested: verified on staging cluster\n\
RelNotes: Support independent histogram threshold bins\n\
Issue-Id: 400500\n\
Change-Id: I2222222222222222222222222222222222222222\n\
Signed-off-by: Dave Maintainer <dave@example.com>",
        // 6. Single-line commit message (subject only)
        "single line subject only",
    ];

    let mut commit_ids = Vec::new();
    for (idx, msg) in messages.iter().enumerate() {
        std::fs::write(d.join("file.txt"), format!("step {idx}\n")).unwrap();
        git_ok(d, &["add", "file.txt"]);
        git_ok(d, &["commit", "--cleanup=verbatim", "-m", msg]);
        let rev = git(d, &["rev-parse", "HEAD"]);
        let oid = gix::ObjectId::from_hex(rev.trim().as_bytes()).unwrap();
        commit_ids.push(oid);
    }

    let engine = tigrs_git::GitEngine::open(Some(d)).expect("open test repo");
    for (idx, oid) in commit_ids.into_iter().enumerate() {
        let diff = engine.compute_commit_diff(oid).expect("compute diff");
        let reconstructed = match &diff.body {
            Some(body) => format!("{}\n\n{}", diff.title, body),
            None => diff.title.to_string(),
        };

        let expected_raw = git(d, &["log", "-1", "--format=%B", &oid.to_hex().to_string()]);
        let expected_lines: Vec<&str> = expected_raw.trim_end().lines().collect();
        let actual_lines: Vec<&str> = reconstructed.trim_end().lines().collect();

        // In case 4 (no blank line after line 1), `reconstructed` inserts a blank line
        // between `title` and `body`; all non-empty lines must match 1-to-1 in order.
        let expected_non_empty: Vec<&str> = expected_lines
            .iter()
            .copied()
            .filter(|l| !l.trim().is_empty())
            .collect();
        let actual_non_empty: Vec<&str> = actual_lines
            .iter()
            .copied()
            .filter(|l| !l.trim().is_empty())
            .collect();

        assert_eq!(
            actual_non_empty, expected_non_empty,
            "Commit #{idx} ({oid}) message lines diverged from `git log -1 --format=%B`"
        );
    }
}

#[test]
fn test_differential_submodule_gitlink_160000_tree_and_diff() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init_repo(d);

    std::fs::write(d.join("README.md"), "# Root Repo\n").unwrap();
    git_ok(d, &["add", "README.md"]);
    let sub_oid_1 = "1111111111111111111111111111111111111111";
    let sub_oid_2 = "2222222222222222222222222222222222222222";
    git_ok(
        d,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{sub_oid_1},vendor/submod"),
        ],
    );
    git_ok(d, &["commit", "-m", "Add submodule gitlink"]);

    git_ok(
        d,
        &[
            "update-index",
            "--cacheinfo",
            &format!("160000,{sub_oid_2},vendor/submod"),
        ],
    );
    git_ok(d, &["commit", "-m", "Bump submodule gitlink"]);

    let engine = tigrs_git::GitEngine::open(Some(d)).expect("open repo");
    let head = engine.head_commit_id().expect("head");

    // 1. Tree listing at vendor/ must report EntryKind::Commit (mode 160000)
    let tree = engine.read_tree(head, "vendor").expect("read vendor tree");
    assert_eq!(tree.entries.len(), 1);
    assert_eq!(tree.entries[0].name, "submod");
    assert_eq!(tree.entries[0].kind, tigrs_git::TreeEntryKind::Commit);
    assert_eq!(tree.entries[0].mode, 0o160_000);
    assert_eq!(tree.entries[0].oid.to_hex().to_string(), sub_oid_2);

    // 2. Commit diff on the submodule bump commit must emit `-Subproject commit <oid1>` / `+Subproject commit <oid2>`
    let diff = engine
        .compute_commit_diff(head)
        .expect("diff submodule bump");
    assert_eq!(diff.files.len(), 1);
    assert_eq!(diff.files[0].path, "vendor/submod");
    assert_eq!(diff.files[0].hunks.len(), 1);
    let lines = &diff.files[0].hunks[0].lines;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].content, format!("Subproject commit {sub_oid_1}"));
    assert_eq!(lines[1].content, format!("Subproject commit {sub_oid_2}"));
}
