// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Git repository discovery and format detection.

use crate::types::RepoInfo;
use std::path::Path;
use tigrs_core::error::{Result, TigError};

/// Refuses *all* configuration sources for `gix`'s security-sensitive lookups.
///
/// `gix` defaults to `gix::config::section::is_trusted`, which accepts
/// `.git/config` (and `.git/config.worktree`, plus anything they `include`)
/// whenever the repository directory is owned by the current user. That
/// ownership check says nothing about provenance: a repository cloned or
/// unpacked from a hostile source is owned by whoever cloned it.
///
/// `gix` consults this predicate exactly at the points where configuration can
/// name an external program or a network endpoint:
/// `filter.<driver>.clean`/`.smudge`/`.process`, `diff.<driver>.command`/
/// `.textconv`, `merge.<driver>.command`, `core.sshCommand`, `ssh.variant`,
/// `credential.helper`, remote URLs, `core.worktree`,
/// `gitoxide.core.shallowFile`, and `core.useReplaceRefs`. Returning `false`
/// unconditionally leaves `gix_filter::Pipeline` and the `gix_diff`/`gix_merge`
/// driver tables empty, which is the in-process equivalent of the `-c`
/// overrides, `GIT_WORK_TREE` / `core.worktree` pinning, and `GIT_ATTR_SOURCE`
/// that [`crate::path_security::safe_git_command`] and
/// [`crate::path_security::apply_untrusted_repo_env`] inject into every Git
/// subprocess so Tier-1 (`gix`) and Tier-2 (`git` CLI) always resolve the exact
/// same canonical worktree root.
///
/// Two distinct zero-click attacks are closed by this, both triggered merely by
/// rendering the status or diff view of a non-reftable repository:
///
/// 1. A worktree `.gitattributes` saying `* filter=evil` combined with a
///    repository-local `[filter "evil"] clean = <command>`.
/// 2. A worktree `.gitattributes` binding every path to a driver the *user*
///    legitimately defined in `~/.gitconfig`, which lets the repository choose
///    which of the user's external programs is fed attacker-controlled bytes
///    and an attacker-chosen `%f` path.
///
/// The cost is that content filters are never applied in-process, so e.g.
/// Git-LFS pointer files are compared as stored rather than as smudged. That
/// matches what the sandboxed Tier-2 CLI already does, and `tigrs` is a
/// read-only viewer that performs no network or merge operations, so nothing
/// else depends on these keys.
fn reject_all_config_sources(_meta: &gix::config::file::Metadata) -> bool {
    false
}

/// Builds the trust mapping used for every repository `tigrs` opens in-process.
///
/// The per-trust-level defaults are preserved (so a repository owned by another
/// user stays on `gix`'s reduced-permission profile) and only the
/// security-sensitive configuration filter is tightened.
fn hardened_trust_map() -> gix::sec::trust::Mapping<gix::open::Options> {
    let map = gix::sec::trust::Mapping::<gix::open::Options>::default();
    gix::sec::trust::Mapping {
        full: map.full.filter_config_section(reject_all_config_sources),
        reduced: map.reduced.filter_config_section(reject_all_config_sources),
    }
}

/// Discovers and opens a Git repository starting from `path` (or current working directory).
///
/// Traverses parent directories upwards until a `.git` directory or gitfile is found.
/// Inspects the repository structure to detect if `--ref-format=reftable` is in use.
///
/// The repository is opened with a hardened `gix` trust mapping (see
/// `reject_all_config_sources`), and `work_dir` is canonicalized and pinned via
/// [`crate::path_security::safe_git_command`] (`GIT_WORK_TREE` and `-c core.worktree`),
/// ensuring that neither repository configuration nor `.gitattributes` can introduce
/// external commands or worktree path differentials between `gix` and the `git` CLI.
pub fn discover_repository(path: Option<&Path>) -> Result<(gix::Repository, RepoInfo)> {
    let start_path = match path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };

    let repo: gix::Repository = gix::ThreadSafeRepository::discover_opts(
        &start_path,
        gix::discover::upwards::Options::default(),
        hardened_trust_map(),
    )
    .map_err(|_err| TigError::RepoNotFound(start_path.clone()))?
    .into();

    let git_dir = repo.git_dir().to_path_buf();
    let common_dir = repo.common_dir().to_path_buf();
    let work_dir = repo
        .workdir()
        .map(|wd| wd.canonicalize().unwrap_or_else(|_| wd.to_path_buf()));
    let is_bare = repo.is_bare();

    verify_untrusted_core_worktree(&git_dir, &common_dir, work_dir.as_deref())?;

    if let Some(ref wd) = work_dir {
        let _ = crate::path_security::safe_git_command(wd);
    }

    let is_reftable = detect_reftable(&git_dir, &common_dir);

    let info = RepoInfo {
        git_dir,
        common_dir,
        work_dir,
        is_reftable,
        is_bare,
    };

    Ok((repo, info))
}

/// Validates that `.git/config`, `config.worktree`, and any recursively included config files
/// (`[include]` / `[includeIf]`) do not specify a `core.worktree` pointing outside the
/// discovered `work_dir`, preventing differential worktree resolution between in-process
/// `gix` (`reject_all_config_sources`) and external `git` CLI subprocesses.
fn verify_untrusted_core_worktree(
    git_dir: &Path,
    common_dir: &Path,
    work_dir: Option<&Path>,
) -> Result<()> {
    for resolved in crate::path_security::collect_core_worktree_targets(git_dir, common_dir) {
        let Some(repo_root) = work_dir else {
            return Err(TigError::Security(format!(
                "Bare repository config specifies forbidden core.worktree '{}'",
                resolved.display()
            )));
        };
        let configured_tree = resolved.canonicalize().unwrap_or(resolved);
        if !configured_tree.starts_with(repo_root) {
            return Err(TigError::Security(format!(
                "Repository config core.worktree '{}' escapes worktree '{}'",
                configured_tree.display(),
                repo_root.display()
            )));
        }
    }
    Ok(())
}

/// Detects whether the repository uses the reftable format.
///
/// Checks:
/// 1. Presence of `.git/reftable/` or `common_dir/reftable/` directory.
/// 2. Or `.git/HEAD` containing `ref: refs/heads/.invalid`.
fn detect_reftable(git_dir: &Path, common_dir: &Path) -> bool {
    if git_dir.join("reftable").is_dir() || common_dir.join("reftable").is_dir() {
        return true;
    }

    let head_file = git_dir.join("HEAD");
    if let Ok(content) = std::fs::read_to_string(&head_file)
        && content.contains(".invalid")
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_config_source_is_trusted_for_command_executing_keys() {
        use gix::config::Source;
        // Every source `gix` knows about. `filter.<drv>.clean`, `diff.<drv>.textconv`,
        // `core.sshCommand` and friends must not be honoured from any of them,
        // because a malicious `.gitattributes` gets to pick which driver runs.
        for source in [
            Source::GitInstallation,
            Source::System,
            Source::Git,
            Source::User,
            Source::Local,
            Source::Worktree,
            Source::Env,
            Source::Cli,
            Source::Api,
            Source::EnvOverride,
        ] {
            let meta = gix::config::file::Metadata {
                path: None,
                source,
                level: 0,
                trust: gix::sec::Trust::Full,
            };
            assert!(
                !reject_all_config_sources(&meta),
                "source {source:?} must not be trusted for command-executing config"
            );
        }
    }

    #[test]
    fn test_detect_reftable_false_on_empty_dir() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!detect_reftable(temp.path(), temp.path()));
    }

    #[test]
    fn test_detect_reftable_true_with_dir() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("reftable")).unwrap();
        assert!(detect_reftable(temp.path(), temp.path()));
    }

    #[test]
    fn test_detect_reftable_true_with_common_dir_on_linked_worktree() {
        let git_dir = tempfile::tempdir().unwrap();
        let common_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(common_dir.path().join("reftable")).unwrap();
        assert!(detect_reftable(git_dir.path(), common_dir.path()));
    }

    #[test]
    fn test_detect_reftable_true_with_invalid_head() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("HEAD"), "ref: refs/heads/.invalid\n").unwrap();
        assert!(detect_reftable(temp.path(), temp.path()));
    }

    #[test]
    fn test_discover_repository_nonexistent_path() {
        let nonexistent = Path::new("/nonexistent/fake/directory/for/testing");
        let res = discover_repository(Some(nonexistent));
        assert!(matches!(res, Err(TigError::RepoNotFound(_))));
    }

    #[test]
    fn test_discover_repository_from_nested_subdir() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().canonicalize().unwrap();

        // Initialize git repo using gix or git
        let _ = gix::init(&repo_root).expect("init git repo");

        let nested = repo_root.join("a/b/c/d");
        std::fs::create_dir_all(&nested).unwrap();

        let (repo, info) = discover_repository(Some(&nested)).expect("discovery from subdir");
        assert!(!info.is_bare);
        assert!(!info.is_reftable);
        assert_eq!(info.work_dir.as_deref().unwrap(), repo_root.as_path());
        assert!(repo.workdir().is_some());
    }
}
