// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Tier-2 Git CLI fallback for reftable backend repositories.
//!
//! Git >= 2.45 supports `--ref-format=reftable`, where refs are stored in binary block
//! tables inside `.git/reftable/` and `.git/HEAD` is a placeholder symref `refs/heads/.invalid`.
//! Since pure-Rust `gix 0.83.0` does not yet parse reftable files, this module shells out to
//! canonical Git CLI plumbing exclusively for ref/HEAD resolution. Revwalks and object
//! accesses remain 100% in-process via `gix`.
//!
//! Process spawns are the single most expensive operation on the startup path
//! (~6.6 ms each on a warm cache), so HEAD and the branch name are resolved with
//! one combined invocation rather than one per value.

use gix::ObjectId;
use std::path::Path;
use tigrs_core::error::{Result, TigError};

/// HEAD state resolved from the Git CLI.
#[derive(Debug, Clone)]
pub struct HeadState {
    /// Commit the current `HEAD` resolves to, or `None` when the branch is
    /// unborn (a fresh repository with no commits yet).
    pub commit: Option<ObjectId>,
    /// Branch name, or the abbreviated hash when `HEAD` is detached.
    pub branch: String,
}

/// Resolves both the `HEAD` commit and its branch name in a single `git` invocation.
///
/// `git rev-parse HEAD --abbrev-ref HEAD` prints the full hash on the first line
/// and the symbolic name on the second, halving the process-spawn cost compared
/// with issuing the two queries separately.
///
/// An unborn `HEAD` is reported as `Ok` with `commit: None` rather than as an
/// error, so a freshly initialized repository opens and renders an empty log
/// exactly like one using the files ref backend.
pub fn resolve_head_state_via_cli(repo_dir: &Path) -> Result<HeadState> {
    let output = crate::path_security::safe_git_command(repo_dir)
        .args(["rev-parse", "HEAD", "--abbrev-ref", "HEAD"])
        .output()
        .map_err(|err| TigError::Git(format!("Failed to execute 'git rev-parse': {err}")))?;

    if !output.status.success() {
        // `git rev-parse HEAD` also fails when the repository simply has no
        // commits. A resolvable symref is git's own definition of that state,
        // and costs an extra spawn only on this cold path.
        if let Some(branch) = symbolic_head_via_cli(repo_dir) {
            return Ok(HeadState {
                commit: None,
                branch,
            });
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TigError::Git(format!(
            "git rev-parse failed: {}",
            stderr.trim()
        )));
    }

    let stdout = std::str::from_utf8(&output.stdout)
        .map_err(|err| TigError::Git(format!("Invalid UTF-8 from git rev-parse: {err}")))?;

    let mut lines = stdout.lines();

    let hex = lines
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| TigError::Git("git rev-parse returned no commit hash".to_string()))?;

    let commit = ObjectId::from_hex(hex.as_bytes())
        .map_err(|err| TigError::Git(format!("Failed to parse commit hash '{hex}': {err}")))?;

    // A detached HEAD makes `--abbrev-ref` echo "HEAD"; fall back to the short hash
    // so the status bar shows something meaningful.
    let branch = match lines.next().map(str::trim) {
        Some(name) if !name.is_empty() && name != "HEAD" => {
            tigrs_core::ansi::strip_control_chars(name).into_owned()
        }
        _ => hex.chars().take(7).collect(),
    };

    Ok(HeadState {
        commit: Some(commit),
        branch,
    })
}

/// Returns the branch `HEAD` symbolically points at, if it is a valid symref.
///
/// Succeeds for an unborn branch (the ref does not exist yet) and fails for a
/// detached `HEAD` or outside a repository.
fn symbolic_head_via_cli(repo_dir: &Path) -> Option<String> {
    let output = crate::path_security::safe_git_command(repo_dir)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let name = std::str::from_utf8(&output.stdout).ok()?.trim();
    (!name.is_empty()).then(|| tigrs_core::ansi::strip_control_chars(name).into_owned())
}

/// Resolves an arbitrary Git revision specification to an `ObjectId` via git CLI.
pub fn resolve_rev_via_cli(repo_dir: &Path, rev: &str) -> Result<ObjectId> {
    if rev.starts_with('-') {
        return Err(TigError::Security(format!(
            "Revision cannot start with dash: '{rev}'"
        )));
    }
    let output = crate::path_security::safe_git_command(repo_dir)
        .args(["rev-parse", "--verify", "--end-of-options", rev])
        .output()
        .map_err(|err| TigError::Git(format!("Failed to execute 'git rev-parse': {err}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TigError::Git(format!(
            "git rev-parse failed for '{rev}': {}",
            stderr.trim()
        )));
    }

    let stdout = std::str::from_utf8(&output.stdout)
        .map_err(|err| TigError::Git(format!("Invalid UTF-8 from git rev-parse: {err}")))?;

    let hex = stdout.trim();
    ObjectId::from_hex(hex.as_bytes())
        .map_err(|err| TigError::Git(format!("Failed to parse commit hash '{hex}': {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_repo(dir: &Path, extra_init_args: &[&str]) {
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(dir)
                .status()
                .expect("git invocation failed")
                .success();
            assert!(ok, "git {args:?} failed");
        };
        let mut init = vec!["init"];
        init.extend_from_slice(extra_init_args);
        run(&init);
        run(&["config", "user.name", "Tigrs Tester"]);
        run(&["config", "user.email", "tester@example.com"]);
        run(&["commit", "--allow-empty", "-m", "Initial commit"]);
    }

    #[test]
    fn test_resolve_head_state_on_branch() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path(), &[]);

        let state = resolve_head_state_via_cli(tmp.path()).expect("resolution failed");
        assert!(!state.branch.is_empty());
        assert_ne!(state.branch, "HEAD");
        let commit = state.commit.expect("HEAD must resolve to a commit");
        assert_eq!(commit.to_hex().to_string().len(), 40);
    }

    #[test]
    fn test_resolve_head_state_detached_falls_back_to_short_hash() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path(), &[]);

        let ok = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["checkout", "--detach"])
            .current_dir(tmp.path())
            .status()
            .expect("git checkout failed")
            .success();
        assert!(ok);

        let state = resolve_head_state_via_cli(tmp.path()).expect("resolution failed");
        // Detached HEAD must not surface the literal string "HEAD".
        assert_eq!(state.branch.len(), 7);
        let commit = state.commit.expect("HEAD must resolve to a commit");
        assert!(commit.to_hex().to_string().starts_with(&state.branch));
    }

    #[test]
    fn test_resolve_head_state_errors_outside_repo() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(resolve_head_state_via_cli(tmp.path()).is_err());
    }

    #[test]
    fn test_resolve_head_state_reports_unborn_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let ok = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init", "-b", "main"])
            .current_dir(tmp.path())
            .status()
            .expect("git init failed")
            .success();
        assert!(ok);

        // A repository without commits is not an error: the branch exists only
        // as a symref, so the commit is absent but the name is still known.
        let state = resolve_head_state_via_cli(tmp.path()).expect("unborn HEAD must resolve");
        assert_eq!(state.commit, None);
        assert_eq!(state.branch, "main");
    }
}
