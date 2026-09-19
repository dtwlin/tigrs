// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Path traversal security and worktree boundary enforcement.
//!
//! Enforces that all file and directory reads stay strictly within the repository
//! worktree boundary, rejecting directory traversal attempts (`..`), absolute paths,
//! and symlinks pointing outside the repository.

use std::path::{Component, Path, PathBuf};
use tigrs_core::error::{Result, TigError};

/// Verifies that a relative git or worktree path is well-formed and does not attempt
/// directory traversal outside its root.
///
/// Returns the normalized relative path string if valid.
///
/// # Errors
///
/// Returns [`TigError::Security`] if the path contains null bytes, starts with a root/prefix,
/// or contains `..` components that would navigate above the root.
pub fn verify_relative_path(path: &str) -> Result<String> {
    if path.contains('\0') {
        return Err(TigError::Security(
            "Path contains forbidden null byte".to_string(),
        ));
    }
    if path.is_empty() {
        return Ok(String::new());
    }
    if path.starts_with(":(")
        || path.starts_with(":/")
        || path.starts_with(":!")
        || path.starts_with(":^")
    {
        return Err(TigError::Security(format!(
            "Git pathspec magic prefixes are forbidden in repository paths: '{path}'"
        )));
    }

    let p = Path::new(path);
    for comp in p.components() {
        if matches!(comp, Component::Prefix(_) | Component::RootDir) {
            return Err(TigError::Security(format!(
                "Absolute paths are forbidden in repository queries: '{path}'"
            )));
        }
    }

    // Leverage gix::path::normalize to resolve lexical components and detect root escapes.
    let normalized = gix::path::normalize(std::borrow::Cow::Borrowed(p), Path::new(""))
        .ok_or_else(|| {
            TigError::Security(format!("Path traversal attempt escaping root: '{path}'"))
        })?;

    let parts: Vec<String> = normalized
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();

    // Prevent direct access to internal .git directory or metadata structures
    if parts.iter().any(|part| part.eq_ignore_ascii_case(".git")) {
        return Err(TigError::Security(format!(
            "Direct access to .git metadata directory is forbidden: '{path}'"
        )));
    }

    Ok(parts.join("/"))
}

fn verify_within_worktree_and_not_git(
    candidate: &Path,
    canonical_root: &Path,
    dot_git: &Path,
    rel_path: &str,
) -> Result<()> {
    if !candidate.starts_with(canonical_root) {
        return Err(TigError::Security(format!(
            "Symlink '{rel_path}' target escapes worktree (points outside worktree to '{}')",
            candidate.display()
        )));
    }
    if candidate.starts_with(dot_git)
        || candidate
            .components()
            .any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case(".git"))
    {
        return Err(TigError::Security(format!(
            "Symlink '{rel_path}' points into .git metadata directory: '{}'",
            candidate.display()
        )));
    }
    Ok(())
}

fn resolve_worktree_components(
    canonical_root: &Path,
    dot_git: &Path,
    base: &Path,
    rel: &Path,
    rel_path_display: &str,
    depth: usize,
    enforce_each_step: bool,
) -> Result<PathBuf> {
    use std::path::Component;
    const MAX_SYMLINK_DEPTH: usize = 40;

    if depth > MAX_SYMLINK_DEPTH {
        return Err(TigError::Security(format!(
            "Symlink depth limit exceeded while resolving '{rel_path_display}'"
        )));
    }

    let mut current = base.to_path_buf();
    let mut enforce = enforce_each_step && current.starts_with(canonical_root);
    for comp in rel.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                current.pop();
                if enforce {
                    verify_within_worktree_and_not_git(
                        &current,
                        canonical_root,
                        dot_git,
                        rel_path_display,
                    )?;
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                current = PathBuf::from("/");
                enforce = false;
            }
            Component::Normal(part) => {
                if part.to_string_lossy().eq_ignore_ascii_case(".git") {
                    return Err(TigError::Security(format!(
                        "Symlink '{rel_path_display}' points into .git metadata directory"
                    )));
                }
                current.push(part);
                if current.is_symlink() {
                    let link_target = std::fs::read_link(&current)?;
                    let (link_base, enforce_inner) = if link_target.is_absolute() {
                        (PathBuf::from("/"), false)
                    } else {
                        (
                            current.parent().unwrap_or(canonical_root).to_path_buf(),
                            enforce,
                        )
                    };
                    current = resolve_worktree_components(
                        canonical_root,
                        dot_git,
                        &link_base,
                        &link_target,
                        rel_path_display,
                        depth + 1,
                        enforce_inner,
                    )?;
                }
                if !enforce && current.starts_with(canonical_root) {
                    enforce = true;
                }
                if enforce {
                    verify_within_worktree_and_not_git(
                        &current,
                        canonical_root,
                        dot_git,
                        rel_path_display,
                    )?;
                }
            }
        }
    }

    if enforce_each_step || enforce {
        verify_within_worktree_and_not_git(&current, canonical_root, dot_git, rel_path_display)?;
    }
    Ok(current)
}

/// Verifies that a requested path inside `worktree_root` is safe to open and does not
/// escape the worktree via symlinks or directory traversal.
///
/// Returns the resolved `PathBuf` within `worktree_root`.
///
/// # Errors
///
/// Returns [`TigError::Security`] if the path or any intermediate symlink escapes `worktree_root`.
pub fn verify_worktree_path_safety(worktree_root: &Path, rel_path: &str) -> Result<PathBuf> {
    let clean_rel = verify_relative_path(rel_path)?;

    let canonical_root = worktree_root
        .canonicalize()
        .map_err(|e| TigError::Git(format!("Failed to resolve worktree root: {e}")))?;

    let dot_git = canonical_root.join(".git");
    let rel_p = Path::new(&clean_rel);

    // Resolve all path components and intermediate symlinks recursively so chained
    // symlinks (`link1 -> link2/nonexistent` where `link2 -> /outside` or `.git/hooks`)
    // are caught even when the final target file does not exist on disk.
    let fully_resolved = resolve_worktree_components(
        &canonical_root,
        &dot_git,
        &canonical_root,
        rel_p,
        rel_path,
        0,
        true,
    )?;

    // Resolve parent directory symlinks while preserving the final leaf name so callers
    // checking `symlink_metadata` / `is_symlink()` on the returned path still observe leaf
    // symlinks directly without traversing uncanonicalized intermediate parent symlinks.
    if let (Some(parent), Some(file_name)) = (rel_p.parent(), rel_p.file_name()) {
        let resolved_parent = if parent.as_os_str().is_empty() {
            canonical_root
        } else {
            resolve_worktree_components(
                &canonical_root,
                &dot_git,
                &canonical_root,
                parent,
                rel_path,
                0,
                true,
            )?
        };
        Ok(resolved_parent.join(file_name))
    } else {
        Ok(fully_resolved)
    }
}

/// Byte-accurate variant of [`verify_relative_path`] operating on [`std::ffi::OsStr`] (including non-UTF-8 Unix filenames).
pub fn verify_relative_os_path(path: &std::ffi::OsStr) -> Result<PathBuf> {
    #[cfg(unix)]
    let raw_bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_bytes()
    };
    #[cfg(not(unix))]
    let raw_bytes = path.to_string_lossy().into_owned().into_bytes();

    if raw_bytes.contains(&0) {
        return Err(TigError::Security(
            "Path contains forbidden null byte".to_string(),
        ));
    }
    if raw_bytes.is_empty() {
        return Ok(PathBuf::new());
    }
    if raw_bytes.starts_with(b":(")
        || raw_bytes.starts_with(b":/")
        || raw_bytes.starts_with(b":!")
        || raw_bytes.starts_with(b":^")
    {
        return Err(TigError::Security(format!(
            "Git pathspec magic prefixes are forbidden in repository paths: '{}'",
            path.to_string_lossy()
        )));
    }

    let p = Path::new(path);
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for comp in p.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => {
                return Err(TigError::Security(format!(
                    "Absolute paths are forbidden in repository queries: '{}'",
                    path.to_string_lossy()
                )));
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(TigError::Security(format!(
                        "Path traversal attempt escaping root: '{}'",
                        path.to_string_lossy()
                    )));
                }
            }
            Component::Normal(part) => {
                if part.to_string_lossy().eq_ignore_ascii_case(".git") {
                    return Err(TigError::Security(format!(
                        "Direct access to .git metadata directory is forbidden: '{}'",
                        path.to_string_lossy()
                    )));
                }
                parts.push(part);
            }
        }
    }

    let mut out = PathBuf::new();
    for part in parts {
        out.push(part);
    }
    Ok(out)
}

/// Byte-accurate variant of [`verify_worktree_path_safety`] operating on [`std::ffi::OsStr`].
pub fn verify_worktree_os_path_safety(
    worktree_root: &Path,
    rel_path: &std::ffi::OsStr,
) -> Result<PathBuf> {
    let clean_rel = verify_relative_os_path(rel_path)?;
    let display_lossy = rel_path.to_string_lossy();

    let canonical_root = worktree_root
        .canonicalize()
        .map_err(|e| TigError::Git(format!("Failed to resolve worktree root: {e}")))?;

    let dot_git = canonical_root.join(".git");
    let rel_p = clean_rel.as_path();

    let fully_resolved = resolve_worktree_components(
        &canonical_root,
        &dot_git,
        &canonical_root,
        rel_p,
        &display_lossy,
        0,
        true,
    )?;

    if let (Some(parent), Some(file_name)) = (rel_p.parent(), rel_p.file_name()) {
        let resolved_parent = if parent.as_os_str().is_empty() {
            canonical_root
        } else {
            resolve_worktree_components(
                &canonical_root,
                &dot_git,
                &canonical_root,
                parent,
                &display_lossy,
                0,
                true,
            )?
        };
        Ok(resolved_parent.join(file_name))
    } else {
        Ok(fully_resolved)
    }
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

const SHA1_EMPTY_TREE_HEX: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const SHA256_EMPTY_TREE_HEX: &str =
    "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321";

/// Resolves the active `.git` metadata directories (including linked worktree `commondir`)
/// for `work_dir`.
#[must_use]
pub fn discover_git_metadata_dirs(work_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for env_var in ["GIT_DIR", "GIT_COMMON_DIR"] {
        if let Some(val) = std::env::var_os(env_var) {
            let p = PathBuf::from(val);
            let resolved = if p.is_absolute() { p } else { work_dir.join(p) };
            for common in resolve_commondir(&resolved) {
                dirs.push(common);
            }
            dirs.push(resolved);
        }
    }

    let canonical_start = work_dir
        .canonicalize()
        .unwrap_or_else(|_| work_dir.to_path_buf());
    let mut cursor = Some(canonical_start.as_path());

    while let Some(dir) = cursor {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            let is_valid = dot_git.join("HEAD").is_file()
                || dot_git.join("config").is_file()
                || dot_git.join("commondir").is_file();
            dirs.push(dot_git.clone());
            for common in resolve_commondir(&dot_git) {
                dirs.push(common);
            }
            if is_valid {
                break;
            }
        } else if dot_git.is_file() {
            let resolved_gitdirs = resolve_gitfile(&dot_git, dir);
            let mut any_valid = false;
            for resolved_gitdir in resolved_gitdirs {
                let is_valid = resolved_gitdir.join("HEAD").is_file()
                    || resolved_gitdir.join("config").is_file()
                    || resolved_gitdir.join("commondir").is_file();
                for common in resolve_commondir(&resolved_gitdir) {
                    dirs.push(common);
                }
                dirs.push(resolved_gitdir);
                if is_valid {
                    any_valid = true;
                }
            }
            if any_valid {
                break;
            }
        } else if dir.join("HEAD").is_file() && dir.join("config").is_file() {
            dirs.push(dir.to_path_buf());
            for common in resolve_commondir(dir) {
                dirs.push(common);
            }
            break;
        }
        cursor = dir.parent();
    }

    dirs
}

fn bytes_to_latin1(bytes: &[u8]) -> String {
    let stripped = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    stripped.iter().copied().map(char::from).collect()
}

fn read_raw_latin1(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(bytes_to_latin1(&bytes))
}

fn latin1_to_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u8).collect()
}

fn latin1_to_os_string(s: &str) -> std::ffi::OsString {
    let bytes = latin1_to_bytes(s);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(bytes)
    }
    #[cfg(not(unix))]
    {
        std::ffi::OsString::from(String::from_utf8_lossy(&bytes).into_owned())
    }
}

fn latin1_to_path_buf(s: &str) -> PathBuf {
    PathBuf::from(latin1_to_os_string(s))
}

fn resolve_gitfile(gitfile: &Path, base_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut current_file = gitfile.to_path_buf();
    let mut current_base = base_dir.to_path_buf();
    for _ in 0..8 {
        let Ok(content) = read_raw_latin1(&current_file) else {
            break;
        };
        let c_str = content.split('\0').next().unwrap_or(&content);
        let mut step_targets = Vec::new();
        for slice in [c_str, content.as_str()] {
            for candidate_line in [
                Some(slice.trim_end_matches(['\r', '\n'])),
                slice.lines().next(),
            ]
            .into_iter()
            .flatten()
            {
                let trimmed_start = candidate_line.trim_ascii_start();
                if let Some(after_prefix) = trimmed_start.strip_prefix("gitdir:") {
                    let exact = after_prefix
                        .trim_start_matches([' ', '\t'])
                        .trim_end_matches(['\r', '\n']);
                    let fully_trimmed = after_prefix.trim_ascii();
                    for raw in [exact, fully_trimmed] {
                        if !raw.is_empty() && !raw.contains('\0') {
                            let target = latin1_to_path_buf(raw);
                            let resolved = if target.is_absolute() {
                                target
                            } else {
                                current_base.join(target)
                            };
                            if !step_targets.contains(&resolved) {
                                step_targets.push(resolved);
                            }
                        }
                    }
                }
            }
        }
        let Some(first_resolved) = step_targets.first().cloned() else {
            break;
        };
        for resolved in step_targets {
            if !resolved.is_file() && !out.contains(&resolved) {
                out.push(resolved);
            }
        }
        if first_resolved.is_file() {
            current_base = first_resolved
                .parent()
                .unwrap_or(&current_base)
                .to_path_buf();
            current_file = first_resolved;
        } else {
            break;
        }
    }
    out
}

fn resolve_commondir(git_dir: &Path) -> Vec<PathBuf> {
    let commondir_file = git_dir.join("commondir");
    let Ok(content) = read_raw_latin1(&commondir_file) else {
        return Vec::new();
    };
    // In Git's `setup.c` (`get_common_dir_noenv`), `commondir` is read into a `strbuf`
    // and passed as a C-string (`const char *`), which truncates at the first `\0` byte
    // and strips only trailing `\r` and `\n`.
    let c_str = content.split('\0').next().unwrap_or(&content);
    let mut results = Vec::new();
    for slice in [c_str, content.as_str()] {
        for candidate_line in [
            Some(slice.trim_end_matches(['\r', '\n'])),
            slice.lines().next(),
        ]
        .into_iter()
        .flatten()
        {
            let exact = candidate_line.trim_end_matches(['\r', '\n']);
            let trimmed = candidate_line.trim_ascii();
            for raw_target in [exact, trimmed] {
                if !raw_target.is_empty() && !raw_target.contains('\0') {
                    let target = latin1_to_path_buf(raw_target);
                    let resolved = if target.is_absolute() {
                        target
                    } else {
                        git_dir.join(target)
                    };
                    if !results.contains(&resolved) {
                        results.push(resolved);
                    }
                }
            }
        }
    }
    results
}

fn push_resolved_include_paths(
    git_dir: &Path,
    config_dir: &Path,
    raw_val: &str,
    include_paths: &mut Vec<PathBuf>,
) {
    let c_str = raw_val.split('\0').next().unwrap_or(raw_val);
    for candidate_str in [
        c_str,
        c_str.trim_ascii(),
        raw_val.trim_matches(['\r', '\n']),
        raw_val.trim_ascii(),
    ] {
        if candidate_str.is_empty() || candidate_str.contains('\0') {
            continue;
        }
        if let Some(rest) = candidate_str
            .strip_prefix("~/")
            .or_else(|| candidate_str.strip_prefix("~\\"))
        {
            if let Some(home) = std::env::var_os("HOME") {
                let p = PathBuf::from(home).join(latin1_to_path_buf(rest));
                if !include_paths.contains(&p) {
                    include_paths.push(p);
                }
            }
            continue;
        }
        let candidate = latin1_to_path_buf(candidate_str);
        if candidate.is_absolute() {
            if !include_paths.contains(&candidate) {
                include_paths.push(candidate);
            }
        } else {
            // Git's config.c (`handle_path_include`) resolves relative `include.path`
            // values against the directory of the config file containing the directive.
            // We also resolve against `git_dir` as defense-in-depth.
            let from_config_dir = config_dir.join(&candidate);
            if !include_paths.contains(&from_config_dir) {
                include_paths.push(from_config_dir.clone());
            }
            let from_git_dir = git_dir.join(&candidate);
            if from_git_dir != from_config_dir && !include_paths.contains(&from_git_dir) {
                include_paths.push(from_git_dir);
            }
        }
    }
}

/// Precomputed, immutable subprocess sanitization plan for a repository working directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RepoSanitizationPlan {
    /// Hash-appropriate empty tree object ID (`SHA1_EMPTY_TREE_HEX` or `SHA256_EMPTY_TREE_HEX`).
    pub empty_tree_hash: &'static str,
    /// Trusted worktree directory to pin via `GIT_WORK_TREE` and `core.worktree` when non-bare
    /// or when repository configuration attempts to override `core.worktree`.
    pub pin_work_tree: Option<PathBuf>,
    /// All `core.worktree` target paths discovered across `.git/config`, `config.worktree`,
    /// and recursive `[include]` / `[includeIf]` directives.
    pub core_worktree_targets: Vec<PathBuf>,
    /// `-c key=value` CLI arguments for keys free of `=`, quotes, and control characters.
    pub cli_overrides: Vec<std::ffi::OsString>,
    /// Full `(key, value)` pairs injected via `GIT_CONFIG_KEY_<N>` / `GIT_CONFIG_VALUE_<N>`.
    pub env_config_pairs: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    /// Canonical list of config and attribute file paths inspected while building this plan.
    pub watched_files: Vec<PathBuf>,
}

/// Stateful accumulator that discovers `filter`, `diff`, and `merge` drivers and object
/// format settings across repository config files, `[include]` chains, and `info/attributes`.
#[derive(Default)]
struct ConfigSanitizationScanner {
    visited_configs: std::collections::BTreeSet<PathBuf>,
    watched_files: Vec<PathBuf>,
    config_files: Vec<(PathBuf, gix::config::File<'static>)>,
    is_sha256: bool,
    has_core_worktree: bool,
    core_worktree_targets: Vec<PathBuf>,
    filter_drivers: std::collections::BTreeSet<String>,
    diff_drivers: std::collections::BTreeSet<String>,
    merge_drivers: std::collections::BTreeSet<String>,
    alias_keys: std::collections::BTreeSet<String>,
    pager_keys: std::collections::BTreeSet<String>,
    credential_urls: std::collections::BTreeSet<String>,
    remote_names: std::collections::BTreeSet<String>,
    tool_names: std::collections::BTreeSet<String>,
}

impl ConfigSanitizationScanner {
    fn record_driver_section(
        &mut self,
        sec_part: &str,
        sub_raw: &str,
        sub_unescaped: &str,
        sub_git_header: &str,
    ) {
        if sub_raw.is_empty() && sub_unescaped.is_empty() && sub_git_header.is_empty() {
            return;
        }
        let target = if sec_part.eq_ignore_ascii_case("filter") {
            Some(&mut self.filter_drivers)
        } else if sec_part.eq_ignore_ascii_case("diff") {
            Some(&mut self.diff_drivers)
        } else if sec_part.eq_ignore_ascii_case("merge") {
            Some(&mut self.merge_drivers)
        } else if sec_part.eq_ignore_ascii_case("credential") {
            Some(&mut self.credential_urls)
        } else if sec_part.eq_ignore_ascii_case("remote") {
            Some(&mut self.remote_names)
        } else if sec_part.eq_ignore_ascii_case("difftool")
            || sec_part.eq_ignore_ascii_case("mergetool")
            || sec_part.eq_ignore_ascii_case("guitool")
            || sec_part.eq_ignore_ascii_case("man")
        {
            Some(&mut self.tool_names)
        } else {
            None
        };
        if let Some(set) = target {
            for base_candidate in [sub_raw, sub_unescaped, sub_git_header] {
                let c_str = base_candidate.split('\0').next().unwrap_or(base_candidate);
                for candidate in [c_str, base_candidate] {
                    if !candidate.is_empty()
                        && !candidate.contains('\n')
                        && !candidate.contains('\r')
                        && !candidate.contains('\0')
                    {
                        set.insert(candidate.to_string());
                        set.insert(candidate.to_ascii_lowercase());
                    }
                }
            }
        }
    }

    fn record_core_worktree_target(&mut self, git_dir: &Path, raw_val: &str) {
        self.has_core_worktree = true;
        let c_str = raw_val.split('\0').next().unwrap_or(raw_val).trim_ascii();
        if !c_str.is_empty() {
            let wt_path = latin1_to_path_buf(c_str);
            let resolved = if wt_path.is_absolute() {
                wt_path
            } else {
                git_dir.join(wt_path)
            };
            if !self.core_worktree_targets.contains(&resolved) {
                self.core_worktree_targets.push(resolved);
            }
        }
    }

    /// Character-stream state machine matching Git's `config.c` (`git_parse_source`).
    ///
    /// Supports `\r\n` normalization (`get_next_char`), `\` + `\n` line continuations inside
    /// both quoted and unquoted strings, multiple `[section "sub"]` headers on the same line,
    /// `]` inside quoted subsection names (`[filter "a]b"]`), `parse_section_header` literal
    /// backslash stripping (`[filter "a\nb"]` -> `anb`), legacy `[section.subsection]`
    /// syntax, and quoted `;` / `#` / whitespace inside `include.path` values.
    fn scan_git_config_stream(
        &mut self,
        git_dir: &Path,
        config_dir: &Path,
        raw_content: &str,
        include_paths: &mut Vec<PathBuf>,
    ) {
        let normalized = raw_content.replace("\r\n", "\n");
        let mut chars = normalized.chars().peekable();
        let mut in_include_section = false;
        let mut in_extensions_section = false;
        let mut in_core_section = false;
        let mut in_alias_section = false;
        let mut in_pager_section = false;

        while let Some(&ch) = chars.peek() {
            // 1. Skip leading ASCII whitespace and newlines
            if ch.is_ascii_whitespace() {
                chars.next();
                continue;
            }
            // 2. Skip unquoted `#` or `;` comments to end of line
            if ch == '#' || ch == ';' {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
                continue;
            }
            // 3. Parse `[section "subsection"]` or `[section.subsection]` header
            if ch == '[' {
                chars.next(); // consume '['
                in_include_section = false;
                in_extensions_section = false;
                in_core_section = false;
                in_alias_section = false;
                in_pager_section = false;

                let mut sec_name = String::new();
                let mut sub_raw = String::new();
                let mut sub_git_header = String::new();
                let mut has_sub = false;
                let mut in_quote = false;
                let mut in_dot_sub = false;
                let mut escaped = false;

                while let Some(c) = chars.next() {
                    if in_quote {
                        if escaped {
                            if c == '\n' {
                                escaped = false;
                                continue;
                            }
                            sub_raw.push('\\');
                            sub_raw.push(c);
                            // In Git's `config.c` (`parse_section_header`), `\c` inside a
                            // quoted subsection header strips `\` and appends literal `c`
                            // (`\n` -> `'n'`, `\t` -> `'t'`, `\b` -> `'b'`).
                            sub_git_header.push(c);
                            escaped = false;
                        } else if c == '\\' {
                            escaped = true;
                        } else if c == '"' {
                            in_quote = false;
                        } else {
                            sub_raw.push(c);
                            sub_git_header.push(c);
                        }
                    } else if c == '"' {
                        in_quote = true;
                        has_sub = true;
                    } else if c == ']' {
                        break;
                    } else if !has_sub && c == '.' {
                        in_dot_sub = true;
                        has_sub = true;
                    } else if in_dot_sub {
                        sub_raw.push(c);
                        sub_git_header.push(c);
                    } else if c == '\\' {
                        if let Some(&next_c) = chars.peek()
                            && next_c == '\n'
                        {
                            chars.next();
                            continue;
                        }
                        sec_name.push(c);
                    } else {
                        sec_name.push(c);
                    }
                }

                let sec_trimmed = sec_name
                    .split('\0')
                    .next()
                    .unwrap_or(&sec_name)
                    .trim_ascii();
                if sec_trimmed.eq_ignore_ascii_case("include")
                    || sec_trimmed.eq_ignore_ascii_case("includeif")
                {
                    in_include_section = true;
                } else if sec_trimmed.eq_ignore_ascii_case("extensions") {
                    in_extensions_section = true;
                } else if sec_trimmed.eq_ignore_ascii_case("core") && !has_sub {
                    in_core_section = true;
                } else if sec_trimmed.eq_ignore_ascii_case("alias") && !has_sub {
                    in_alias_section = true;
                } else if sec_trimmed.eq_ignore_ascii_case("pager") && !has_sub {
                    in_pager_section = true;
                } else if has_sub {
                    let raw_ref = if in_dot_sub {
                        sub_raw.trim_ascii()
                    } else {
                        sub_raw.as_str()
                    };
                    let git_hdr_ref = if in_dot_sub {
                        sub_git_header.trim_ascii()
                    } else {
                        sub_git_header.as_str()
                    };
                    self.record_driver_section(sec_trimmed, raw_ref, git_hdr_ref, git_hdr_ref);
                }
                continue;
            }

            // 4. Parse `key = value` (or valueless boolean `key`) up to unquoted `\n`, `#`, or `;`
            let mut key_buf = String::new();
            let mut saw_equals = false;
            while let Some(&c) = chars.peek() {
                if c == '\\' {
                    chars.next();
                    if let Some(&next_c) = chars.peek()
                        && next_c == '\n'
                    {
                        chars.next();
                        continue;
                    }
                    key_buf.push('\\');
                    continue;
                } else if c == '=' {
                    chars.next();
                    saw_equals = true;
                    break;
                } else if c == '\n' || c == '#' || c == ';' {
                    break;
                }
                key_buf.push(c);
                chars.next();
            }

            // Mirror `config.c:parse_value()`:
            // - Unquoted whitespace is deferred (`pending_spaces`) and only emitted if followed
            //   by a subsequent character or quote, whereas spaces inside `"..."` or escaped via
            //   `\ ` are preserved immediately.
            // - `[` is NOT a delimiter in `parse_value()` (e.g. `dummy = [core]` assigns `"[core]"`).
            let mut val_buf = String::new();
            if saw_equals {
                let mut in_quote = false;
                let mut escaped = false;
                let mut pending_spaces = String::new();
                while let Some(&c) = chars.peek() {
                    if in_quote {
                        chars.next();
                        if escaped {
                            if c == '\n' {
                                escaped = false;
                                continue;
                            }
                            let decoded = match c {
                                'n' => '\n',
                                't' => '\t',
                                'b' => '\x08',
                                other => other,
                            };
                            val_buf.push(decoded);
                            escaped = false;
                        } else if c == '\\' {
                            escaped = true;
                        } else if c == '"' {
                            in_quote = false;
                        } else {
                            val_buf.push(c);
                        }
                    } else if c == '"' {
                        chars.next();
                        if !val_buf.is_empty() {
                            val_buf.push_str(&pending_spaces);
                        }
                        pending_spaces.clear();
                        in_quote = true;
                    } else if c == '\\' {
                        chars.next();
                        if let Some(next_c) = chars.next() {
                            if next_c == '\n' {
                                continue;
                            }
                            if !val_buf.is_empty() {
                                val_buf.push_str(&pending_spaces);
                            }
                            pending_spaces.clear();
                            let decoded = match next_c {
                                'n' => '\n',
                                't' => '\t',
                                'b' => '\x08',
                                other => other,
                            };
                            val_buf.push(decoded);
                        }
                    } else if c == '\n' || c == '#' || c == ';' {
                        break;
                    } else if c.is_ascii_whitespace() {
                        pending_spaces.push(c);
                        chars.next();
                    } else {
                        if !val_buf.is_empty() {
                            val_buf.push_str(&pending_spaces);
                        }
                        pending_spaces.clear();
                        val_buf.push(c);
                        chars.next();
                    }
                }
            }

            let key = key_buf.split('\0').next().unwrap_or(&key_buf).trim_ascii();
            let val_trimmed = val_buf.trim_ascii();
            if in_include_section && key.eq_ignore_ascii_case("path") && !val_buf.is_empty() {
                push_resolved_include_paths(git_dir, config_dir, &val_buf, include_paths);
            } else if in_core_section && key.eq_ignore_ascii_case("worktree") {
                self.record_core_worktree_target(git_dir, &val_buf);
            } else if in_extensions_section
                && key.eq_ignore_ascii_case("objectformat")
                && val_trimmed.eq_ignore_ascii_case("sha256")
            {
                self.is_sha256 = true;
            } else if in_alias_section
                && key.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                self.alias_keys.insert(key.to_ascii_lowercase());
            } else if in_pager_section
                && key.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                self.pager_keys.insert(key.to_ascii_lowercase());
            }
        }
    }

    fn collect_config_with_includes(&mut self, git_dir: &Path, initial_cfg: PathBuf, depth: usize) {
        // Git's `MAX_INCLUDE_DEPTH` in `config.c` is 10; scan up to depth 16 to guarantee
        // every config file Git could possibly evaluate is inspected.
        if depth > 16 {
            return;
        }
        let canonical = initial_cfg
            .canonicalize()
            .unwrap_or_else(|_| normalize_lexical_path(&initial_cfg));
        if !self.visited_configs.insert(canonical) {
            return;
        }
        self.watched_files.push(initial_cfg.clone());
        let config_dir = initial_cfg.parent().unwrap_or(git_dir).to_path_buf();

        let mut include_paths = Vec::new();
        if let Ok(raw_content) = read_raw_latin1(&initial_cfg) {
            self.scan_git_config_stream(git_dir, &config_dir, &raw_content, &mut include_paths);
        }

        if let Ok(file) =
            gix::config::File::from_path_no_includes(initial_cfg, gix::config::Source::Local)
        {
            for section in file.sections() {
                let section_name = section.header().name();
                if section_name.eq_ignore_ascii_case(b"include")
                    || section_name.eq_ignore_ascii_case(b"includeif")
                {
                    for key_variant in ["path", "PATH", "Path"] {
                        for val in section.values(key_variant) {
                            let raw_path = bytes_to_latin1(val.as_ref());
                            push_resolved_include_paths(
                                git_dir,
                                &config_dir,
                                &raw_path,
                                &mut include_paths,
                            );
                        }
                    }
                }
            }
            self.config_files.push((git_dir.to_path_buf(), file));
        }

        for inc in include_paths {
            self.collect_config_with_includes(git_dir, inc, depth + 1);
        }
    }

    fn scan_git_dir(&mut self, git_dir: &Path) {
        for cfg_name in ["config", "config.worktree"] {
            let cfg_path = git_dir.join(cfg_name);
            self.collect_config_with_includes(git_dir, cfg_path, 0);
        }

        for idx in 0..self.config_files.len() {
            let (cfg_git_dir, file) = &self.config_files[idx];
            let cfg_git_dir = cfg_git_dir.clone();
            for key_variant in ["objectformat", "objectFormat", "OBJECTFORMAT"] {
                if let Some(fmt) = file.string_by("extensions", None, key_variant)
                    && bytes_to_latin1(fmt.as_ref())
                        .trim_ascii()
                        .eq_ignore_ascii_case("sha256")
                {
                    self.is_sha256 = true;
                }
            }
            let mut worktree_vals = Vec::new();
            for key_variant in ["worktree", "workTree", "WORKTREE"] {
                if let Some(wt_val) = file.string_by("core", None, key_variant) {
                    worktree_vals.push(bytes_to_latin1(wt_val.as_ref()));
                }
            }
            let mut discovered = Vec::new();
            for section in file.sections() {
                let header = section.header();
                let section_name = bytes_to_latin1(header.name().as_ref());
                if section_name.eq_ignore_ascii_case("core") {
                    for key_name in section.value_names() {
                        let key_str: &str = key_name.as_ref();
                        if key_str.trim_ascii().eq_ignore_ascii_case("worktree") {
                            for val in section.values(key_str) {
                                worktree_vals.push(bytes_to_latin1(val.as_ref()));
                            }
                            if worktree_vals.is_empty() {
                                self.has_core_worktree = true;
                            }
                        }
                    }
                }
                if let Some(sub) = header.subsection_name() {
                    let sub_str = bytes_to_latin1(sub.as_ref());
                    if !sub_str.is_empty() {
                        discovered.push((section_name, sub_str));
                    }
                }
            }
            for wt_val in worktree_vals {
                self.record_core_worktree_target(&cfg_git_dir, &wt_val);
            }
            for (sec, sub) in discovered {
                self.record_driver_section(&sec, &sub, &sub, &sub);
            }
        }

        let info_attrs = git_dir.join("info").join("attributes");
        self.watched_files.push(info_attrs.clone());
        if let Ok(content) = read_raw_latin1(&info_attrs) {
            for line in content.lines() {
                extract_attribute_drivers_from_line(
                    line,
                    &mut self.filter_drivers,
                    &mut self.diff_drivers,
                    &mut self.merge_drivers,
                );
            }
        }
    }

    fn into_plan(self, work_dir: &Path) -> RepoSanitizationPlan {
        let mut raw_pairs: Vec<(String, String)> = vec![
            ("core.fsmonitor".to_string(), "false".to_string()),
            ("core.hooksPath".to_string(), "/dev/null".to_string()),
            ("core.attributesFile".to_string(), "/dev/null".to_string()),
            (
                "core.alternateRefsCommand".to_string(),
                "/bin/false".to_string(),
            ),
            ("diff.external".to_string(), String::new()),
            ("core.pager".to_string(), "cat".to_string()),
            ("interactive.diffFilter".to_string(), String::new()),
            ("core.sshCommand".to_string(), "/bin/false".to_string()),
            ("core.gitProxy".to_string(), "/bin/false".to_string()),
            ("core.askPass".to_string(), "/bin/false".to_string()),
            ("credential.helper".to_string(), String::new()),
            ("protocol.ext.allow".to_string(), "never".to_string()),
            ("gpg.program".to_string(), "/bin/false".to_string()),
            ("gpg.openpgp.program".to_string(), "/bin/false".to_string()),
            ("gpg.x509.program".to_string(), "/bin/false".to_string()),
            ("gpg.ssh.program".to_string(), "/bin/false".to_string()),
            (
                "gpg.ssh.defaultKeyCommand".to_string(),
                "/bin/false".to_string(),
            ),
            (
                "uploadpack.packObjectsHook".to_string(),
                "/bin/false".to_string(),
            ),
            ("sendemail.smtpServer".to_string(), "/bin/false".to_string()),
            ("sendemail.validate".to_string(), "false".to_string()),
            ("http.proxy".to_string(), String::new()),
            ("https.proxy".to_string(), String::new()),
            ("gc.auto".to_string(), "0".to_string()),
            ("maintenance.auto".to_string(), "false".to_string()),
            ("fetch.writeCommitGraph".to_string(), "false".to_string()),
            ("color.ui".to_string(), "false".to_string()),
            ("color.diff".to_string(), "false".to_string()),
            ("color.status".to_string(), "false".to_string()),
            ("diff.noprefix".to_string(), "false".to_string()),
            ("diff.mnemonicPrefix".to_string(), "false".to_string()),
            ("diff.suppressBlankEmpty".to_string(), "false".to_string()),
            ("diff.relative".to_string(), "false".to_string()),
            ("apply.whitespace".to_string(), "nowarn".to_string()),
        ];

        for name in self.filter_drivers.into_iter().take(2048) {
            raw_pairs.push((format!("filter.{name}.clean"), String::new()));
            raw_pairs.push((format!("filter.{name}.smudge"), String::new()));
            raw_pairs.push((format!("filter.{name}.process"), String::new()));
            raw_pairs.push((format!("filter.{name}.required"), "false".to_string()));
        }
        for name in self.diff_drivers.into_iter().take(2048) {
            raw_pairs.push((format!("diff.{name}.command"), String::new()));
            raw_pairs.push((format!("diff.{name}.textconv"), String::new()));
            raw_pairs.push((format!("diff.{name}.cachetextconv"), "false".to_string()));
        }
        for name in self.merge_drivers.into_iter().take(2048) {
            raw_pairs.push((format!("merge.{name}.driver"), String::new()));
        }
        for alias in self.alias_keys.into_iter().take(2048) {
            raw_pairs.push((format!("alias.{alias}"), String::new()));
        }
        for pager_cmd in self.pager_keys.into_iter().take(2048) {
            raw_pairs.push((format!("pager.{pager_cmd}"), "false".to_string()));
        }
        for cred_url in self.credential_urls.into_iter().take(2048) {
            raw_pairs.push((format!("credential.{cred_url}.helper"), String::new()));
        }
        for remote in self.remote_names.into_iter().take(2048) {
            raw_pairs.push((
                format!("remote.{remote}.uploadpack"),
                "/bin/false".to_string(),
            ));
            raw_pairs.push((
                format!("remote.{remote}.receivepack"),
                "/bin/false".to_string(),
            ));
            raw_pairs.push((format!("remote.{remote}.proxy"), String::new()));
            raw_pairs.push((format!("remote.{remote}.vcs"), String::new()));
        }
        for tool in self.tool_names.into_iter().take(2048) {
            raw_pairs.push((format!("difftool.{tool}.cmd"), String::new()));
            raw_pairs.push((format!("difftool.{tool}.path"), "/bin/false".to_string()));
            raw_pairs.push((format!("mergetool.{tool}.cmd"), String::new()));
            raw_pairs.push((format!("mergetool.{tool}.path"), "/bin/false".to_string()));
            raw_pairs.push((format!("guitool.{tool}.cmd"), String::new()));
            raw_pairs.push((format!("man.{tool}.cmd"), String::new()));
        }

        let mut env_config_pairs: Vec<(std::ffi::OsString, std::ffi::OsString)> = raw_pairs
            .iter()
            .map(|(k, v)| (latin1_to_os_string(k), latin1_to_os_string(v)))
            .collect();

        let mut cli_overrides: Vec<std::ffi::OsString> = Vec::new();
        for (k, v) in &raw_pairs {
            let safe_for_cli_flag = !k.contains('=')
                && !k.contains('"')
                && !k.contains('\'')
                && !k.contains('\n')
                && !k.contains('\r')
                && !k.contains('\0');
            if safe_for_cli_flag {
                cli_overrides.push(std::ffi::OsString::from("-c"));
                cli_overrides.push(latin1_to_os_string(&format!("{k}={v}")));
            }
        }

        let pin_work_tree = if work_dir.join(".git").exists() || self.has_core_worktree {
            let wt_os = work_dir.as_os_str().to_os_string();
            env_config_pairs.push((
                std::ffi::OsString::from("core.bare"),
                std::ffi::OsString::from("false"),
            ));
            env_config_pairs.push((std::ffi::OsString::from("core.worktree"), wt_os.clone()));
            cli_overrides.push(std::ffi::OsString::from("-c"));
            cli_overrides.push(std::ffi::OsString::from("core.bare=false"));
            cli_overrides.push(std::ffi::OsString::from("-c"));
            let mut wt_arg = std::ffi::OsString::from("core.worktree=");
            wt_arg.push(wt_os);
            cli_overrides.push(wt_arg);
            Some(work_dir.to_path_buf())
        } else {
            None
        };

        let empty_tree_hash = if self.is_sha256 {
            SHA256_EMPTY_TREE_HEX
        } else {
            SHA1_EMPTY_TREE_HEX
        };

        RepoSanitizationPlan {
            empty_tree_hash,
            pin_work_tree,
            core_worktree_targets: self.core_worktree_targets,
            cli_overrides,
            env_config_pairs,
            watched_files: self.watched_files,
        }
    }
}

fn insert_safe_driver_name(set: &mut std::collections::BTreeSet<String>, name: &str) {
    if !name.is_empty() && !name.contains('\0') && !name.contains('\n') && !name.contains('\r') {
        set.insert(name.to_string());
        set.insert(name.to_ascii_lowercase());
    }
}

/// Parses a single line from `$GIT_DIR/info/attributes` (including C-style quoted patterns
/// without trailing spaces such as `"*"filter=evil`, lines starting with vertical tab `\x0b#`,
/// and lines containing interior `\0` bytes truncated by `attr.c:parse_attr_line`)
/// and inserts any discovered `filter`, `diff`, or `merge` driver names.
fn extract_attribute_drivers_from_line(
    line: &str,
    filter_drivers: &mut std::collections::BTreeSet<String>,
    diff_drivers: &mut std::collections::BTreeSet<String>,
    merge_drivers: &mut std::collections::BTreeSet<String>,
) {
    // In Git's `attr.c` (`read_attr_from_buf` -> `parse_attr_line`), each line is passed
    // as a NUL-terminated C string (`const char *line`), so any `\0` byte truncates the
    // remainder of the line (`* filter=evil\x00_decoy` -> `* filter=evil`).
    // Process each `\0`-separated segment as well as the `\0`-stripped string.
    let stripped_nul = if line.contains('\0') {
        Some(line.replace('\0', ""))
    } else {
        None
    };
    let segments = line.split('\0').chain(stripped_nul.as_deref());

    for segment in segments {
        // In Git's `attr.c` (`parse_attr_line`), `blank` is strictly `" \t\r\n"`.
        // Characters like `\x0b` (vertical tab), `\x0c` (form feed), or Unicode spaces
        // are NOT stripped by `attr.c`, so `\x0b# filter=evil` is parsed by Git as a
        // file pattern `\x0b#` rather than a `#` comment!
        let line_no_bom = segment.strip_prefix('\u{FEFF}').unwrap_or(segment);
        let git_trimmed = line_no_bom.trim_matches([' ', '\t', '\r', '\n']);
        if git_trimmed.is_empty() || git_trimmed.starts_with('#') {
            continue;
        }

        let mut candidate_slices = vec![git_trimmed];
        let unicode_trimmed = line_no_bom.trim();
        if unicode_trimmed != git_trimmed && !unicode_trimmed.is_empty() {
            candidate_slices.push(unicode_trimmed);
        }
        for base_slice in [git_trimmed, unicode_trimmed] {
            if let Some(after_open_quote) = base_slice.strip_prefix('"') {
                let mut escaped = false;
                for (idx, ch) in after_open_quote.char_indices() {
                    if escaped {
                        escaped = false;
                        continue;
                    }
                    if ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        let remainder = after_open_quote[idx + 1..].trim();
                        if !remainder.is_empty() {
                            candidate_slices.push(remainder);
                        }
                        break;
                    }
                }
            }
        }

        for slice in candidate_slices {
            let iter_git = slice.split([' ', '\t', '\r', '\n']);
            let iter_unicode = slice.split_whitespace();
            for raw_token in iter_git.chain(iter_unicode) {
                if raw_token.is_empty() {
                    continue;
                }
                let mut subtokens = vec![raw_token];
                if let Some((_, after_q)) = raw_token.rsplit_once('"')
                    && !after_q.is_empty()
                {
                    subtokens.push(after_q);
                }
                for sub in subtokens {
                    let token = sub
                        .strip_prefix('-')
                        .or_else(|| sub.strip_prefix('!'))
                        .unwrap_or(sub);
                    if let Some((prefix, raw_name)) = token.split_once('=') {
                        let clean_name = raw_name.trim_matches('"').trim_matches('\'');
                        if prefix.eq_ignore_ascii_case("filter") {
                            insert_safe_driver_name(filter_drivers, raw_name);
                            insert_safe_driver_name(filter_drivers, clean_name);
                        } else if prefix.eq_ignore_ascii_case("diff") {
                            insert_safe_driver_name(diff_drivers, raw_name);
                            insert_safe_driver_name(diff_drivers, clean_name);
                        } else if prefix.eq_ignore_ascii_case("merge") {
                            insert_safe_driver_name(merge_drivers, raw_name);
                            insert_safe_driver_name(merge_drivers, clean_name);
                        }
                    }
                }
            }
        }
    }
}

/// Computes a deterministic 64-bit content fingerprint over the watched config and attribute files.
fn compute_files_fingerprint(files: &[PathBuf]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in files {
        path.hash(&mut hasher);
        match std::fs::read(path) {
            Ok(bytes) => {
                true.hash(&mut hasher);
                bytes.hash(&mut hasher);
            }
            Err(_) => {
                false.hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

/// Builds an uncached [`RepoSanitizationPlan`] for `work_dir`.
fn build_repo_config_sanitization(work_dir: &Path) -> RepoSanitizationPlan {
    let mut scanner = ConfigSanitizationScanner::default();
    for git_dir in discover_git_metadata_dirs(work_dir) {
        scanner.scan_git_dir(&git_dir);
    }
    scanner.into_plan(work_dir)
}

/// Scans `git_dir`, `common_dir`, and all recursive `[include]` / `[includeIf]` directives
/// up to depth 16, returning every resolved `core.worktree` path.
pub(crate) fn collect_core_worktree_targets(git_dir: &Path, common_dir: &Path) -> Vec<PathBuf> {
    let mut scanner = ConfigSanitizationScanner::default();
    scanner.scan_git_dir(git_dir);
    if common_dir != git_dir {
        scanner.scan_git_dir(common_dir);
    }
    scanner.core_worktree_targets
}

/// Returns a cached [`RepoSanitizationPlan`] if the content fingerprint of all watched
/// repository config and `info/attributes` files matches, or rebuilds and updates the cache.
fn get_or_build_repo_sanitization_plan(work_dir: &Path) -> std::sync::Arc<RepoSanitizationPlan> {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, OnceLock};

    type CacheKey = (
        PathBuf,
        Option<std::ffi::OsString>,
        Option<std::ffi::OsString>,
    );
    type CacheEntry = (CacheKey, u64, Arc<RepoSanitizationPlan>);
    static PLAN_CACHE: OnceLock<Mutex<VecDeque<CacheEntry>>> = OnceLock::new();
    let cache_mutex = PLAN_CACHE.get_or_init(|| Mutex::new(VecDeque::with_capacity(16)));
    let canonical_dir = work_dir
        .canonicalize()
        .unwrap_or_else(|_| work_dir.to_path_buf());
    let canonical_key: CacheKey = (
        canonical_dir.clone(),
        std::env::var_os("GIT_DIR"),
        std::env::var_os("GIT_COMMON_DIR"),
    );

    if let Ok(guard) = cache_mutex.lock()
        && let Some((_, cached_hash, cached_plan)) =
            guard.iter().find(|(key, _, _)| key == &canonical_key)
    {
        let current_hash = compute_files_fingerprint(&cached_plan.watched_files);
        if *cached_hash == current_hash {
            return Arc::clone(cached_plan);
        }
    }

    let plan = Arc::new(build_repo_config_sanitization(&canonical_dir));
    let fingerprint = compute_files_fingerprint(&plan.watched_files);

    if let Ok(mut guard) = cache_mutex.lock() {
        guard.retain(|(key, _, _)| key != &canonical_key);
        if guard.len() >= 16 {
            guard.pop_front();
        }
        guard.push_back((canonical_key, fingerprint, Arc::clone(&plan)));
    }

    plan
}

/// Injects repository-sanitization environment variables (`GIT_CONFIG_COUNT`,
/// `GIT_CONFIG_KEY_<N>`, `GIT_CONFIG_VALUE_<N>`, `GIT_ATTR_SOURCE`, `GIT_EDITOR`,
/// `GIT_LITERAL_PATHSPECS`, `GIT_WORK_TREE`, etc.) into `cmd` so that any `git` process
/// spawned directly or via a shell in an untrusted repository cannot execute repository-local
/// hooks, `fsmonitor`, `filter`/`diff`/`merge` drivers, GPG programs, repository-defined
/// `core.editor` / `sequence.editor`, `core.worktree` escapes, or pathspec magic injections.
pub fn apply_untrusted_repo_env(cmd: &mut std::process::Command, work_dir: &Path) {
    let plan = get_or_build_repo_sanitization_plan(work_dir);
    let trusted_editor = std::env::var("GIT_EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    cmd.env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_ATTR_SOURCE", plan.empty_tree_hash)
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("GIT_EDITOR", &trusted_editor);

    if let Some(ref wt) = plan.pin_work_tree {
        cmd.env("GIT_WORK_TREE", wt);
    }

    let base_count = std::env::var("GIT_CONFIG_COUNT")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);

    let mut idx = 0usize;
    for (key, value) in &plan.env_config_pairs {
        let slot = base_count + idx;
        cmd.env(format!("GIT_CONFIG_KEY_{slot}"), key);
        cmd.env(format!("GIT_CONFIG_VALUE_{slot}"), value);
        idx += 1;
    }
    for editor_key in ["core.editor", "sequence.editor"] {
        let slot = base_count + idx;
        cmd.env(format!("GIT_CONFIG_KEY_{slot}"), editor_key);
        cmd.env(format!("GIT_CONFIG_VALUE_{slot}"), &trusted_editor);
        idx += 1;
    }
    cmd.env("GIT_CONFIG_COUNT", (base_count + idx).to_string());
}

/// Resolves the `git` binary by walking only absolute directories in `PATH` (skipping `.`
/// and empty segments so a hostile repository containing `./git` cannot hijack `execvp`
/// when `current_dir(work_dir)` is set).
fn resolve_safe_git_binary() -> std::ffi::OsString {
    if let Some(path_os) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_os) {
            if !dir.is_absolute() {
                continue;
            }
            let candidate = dir.join("git");
            if let Ok(meta) = std::fs::metadata(&candidate)
                && meta.is_file()
            {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
                return candidate.into_os_string();
            }
        }
    }
    std::ffi::OsString::from("git")
}

/// Creates a hardened `git` subprocess `Command` scoped to `work_dir`.
///
/// Explicitly disables repository-local `core.fsmonitor`, `core.hooksPath`, `diff.external`,
/// `core.attributesFile`, external protocol handlers, SSH commands, credential helpers,
/// neutralizes repository-local `filter`, `diff`, and `merge` drivers (including those
/// referenced in `$GIT_DIR/info/attributes` or containing `=` in subsection names via
/// `GIT_CONFIG_COUNT` / `GIT_CONFIG_KEY_<N>` / `GIT_CONFIG_VALUE_<N>`), pins `GIT_WORK_TREE`
/// and `core.worktree` to `work_dir`, forces literal pathspecs (`GIT_LITERAL_PATHSPECS=1`),
/// and sets `GIT_ATTR_SOURCE` to the hash-appropriate empty tree (`sha1` or `sha256`).
#[must_use]
pub fn safe_git_command(work_dir: &Path) -> std::process::Command {
    let plan = get_or_build_repo_sanitization_plan(work_dir);
    let mut cmd = std::process::Command::new(resolve_safe_git_binary());
    cmd.current_dir(work_dir)
        .stdin(std::process::Stdio::null())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_ATTR_SOURCE", plan.empty_tree_hash)
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("GIT_EDITOR", "/bin/false")
        .args([
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.editor=/bin/false",
            "-c",
            "sequence.editor=/bin/false",
        ])
        .args(&plan.cli_overrides);

    if let Some(ref wt) = plan.pin_work_tree {
        cmd.env("GIT_WORK_TREE", wt);
    }

    let base_count = std::env::var("GIT_CONFIG_COUNT")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut idx = 0usize;
    for (key, value) in &plan.env_config_pairs {
        let slot = base_count + idx;
        cmd.env(format!("GIT_CONFIG_KEY_{slot}"), key);
        cmd.env(format!("GIT_CONFIG_VALUE_{slot}"), value);
        idx += 1;
    }
    for editor_key in ["core.editor", "sequence.editor"] {
        let slot = base_count + idx;
        cmd.env(format!("GIT_CONFIG_KEY_{slot}"), editor_key);
        cmd.env(format!("GIT_CONFIG_VALUE_{slot}"), "/bin/false");
        idx += 1;
    }
    cmd.env("GIT_CONFIG_COUNT", (base_count + idx).to_string());

    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_relative_path_valid() {
        assert_eq!(verify_relative_path("src/main.rs").unwrap(), "src/main.rs");
        assert_eq!(
            verify_relative_path("./src/../src/lib.rs").unwrap(),
            "src/lib.rs"
        );
        assert_eq!(verify_relative_path("foo/bar/baz").unwrap(), "foo/bar/baz");
        assert_eq!(verify_relative_path("").unwrap(), "");
        assert_eq!(verify_relative_path("./a/./b/./c").unwrap(), "a/b/c");
        assert_eq!(verify_relative_path("a/b/c/../../b/c").unwrap(), "a/b/c");
    }

    #[test]
    fn test_verify_relative_path_traversal_attacks() {
        assert!(verify_relative_path("../etc/passwd").is_err());
        assert!(verify_relative_path("a/../../etc/passwd").is_err());
        assert!(verify_relative_path("a/b/../../../c").is_err());
        assert!(verify_relative_path("../").is_err());
        assert!(verify_relative_path("/etc/shadow").is_err());
        assert!(verify_relative_path("foo\0bar").is_err());
    }

    #[test]
    fn test_verify_worktree_path_safety_symlink_inside() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        let target_file = root.join("file.txt");
        std::fs::write(&target_file, "hello").unwrap();

        let link_file = root.join("link.txt");
        std::os::unix::fs::symlink("file.txt", &link_file).unwrap();

        let res = verify_worktree_path_safety(root, "link.txt");
        assert!(res.is_ok());
    }

    #[test]
    fn test_verify_worktree_path_safety_absolute_symlink_via_symlinked_parent_root() {
        // Simulates macOS where $TMPDIR is under /var/folders/... and /var -> private/var.
        let base = tempfile::tempdir().unwrap();
        let private_var = base.path().join("private").join("var");
        let repo_real = private_var.join("folders").join("worktree");
        std::fs::create_dir_all(&repo_real).unwrap();

        let var_symlink = base.path().join("var");
        std::os::unix::fs::symlink("private/var", &var_symlink).unwrap();

        let uncanonical_repo = var_symlink.join("folders").join("worktree");
        let real_file = uncanonical_repo.join("real.txt");
        std::fs::write(&real_file, "hello").unwrap();

        // Absolute symlink pointing to uncanonicalized /.../var/folders/worktree/real.txt
        let link_in = uncanonical_repo.join("abs_in.txt");
        std::os::unix::fs::symlink(&real_file, &link_in).unwrap();
        assert!(verify_worktree_path_safety(&uncanonical_repo, "abs_in.txt").is_ok());

        // Absolute symlink escaping via symlinked parent must still be rejected
        let outside_file = var_symlink.join("folders").join("outside.txt");
        std::fs::write(&outside_file, "secret").unwrap();
        let link_out = uncanonical_repo.join("abs_out.txt");
        std::os::unix::fs::symlink(&outside_file, &link_out).unwrap();
        let err = verify_worktree_path_safety(&uncanonical_repo, "abs_out.txt").unwrap_err();
        assert!(err.to_string().contains("outside worktree"));
    }

    #[test]
    fn test_verify_worktree_path_safety_symlink_escape_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        let link_file = root.join("evil_link.txt");
        std::os::unix::fs::symlink("/etc/passwd", &link_file).unwrap();

        let res = verify_worktree_path_safety(root, "evil_link.txt");
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("outside worktree"));

        // Symlink pointing to a nonexistent absolute path outside worktree
        let broken_abs_link = root.join("broken_abs.txt");
        std::os::unix::fs::symlink(
            "/nonexistent_dir_12345/does_not_exist.txt",
            &broken_abs_link,
        )
        .unwrap();
        let res_broken = verify_worktree_path_safety(root, "broken_abs.txt");
        assert!(res_broken.is_err());
        assert!(
            res_broken
                .unwrap_err()
                .to_string()
                .contains("target escapes worktree")
        );
    }

    #[test]
    fn test_verify_relative_path_git_metadata_rejected() {
        assert!(verify_relative_path(".git").is_err());
        assert!(verify_relative_path(".git/config").is_err());
        assert!(verify_relative_path(".git/hooks/pre-commit").is_err());
        assert!(verify_relative_path("foo/../../.git/config").is_err());
        assert!(verify_relative_path(".GIT/config").is_err());
        assert!(verify_relative_path("./.git/index").is_err());

        // Standard tracked git metadata files in worktree MUST be permitted
        assert_eq!(verify_relative_path(".gitignore").unwrap(), ".gitignore");
        assert_eq!(
            verify_relative_path(".gitattributes").unwrap(),
            ".gitattributes"
        );
        assert_eq!(verify_relative_path(".gitmodules").unwrap(), ".gitmodules");
    }

    #[test]
    fn test_verify_worktree_path_safety_symlink_into_dotgit_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        let dot_git = root.join(".git");
        std::fs::create_dir_all(&dot_git).unwrap();
        std::fs::write(dot_git.join("config"), "[core]\n").unwrap();

        let link_to_config = root.join("pwn_config");
        std::os::unix::fs::symlink(".git/config", &link_to_config).unwrap();

        let res = verify_worktree_path_safety(root, "pwn_config");
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains(".git metadata directory")
        );
    }

    #[test]
    fn test_verify_worktree_path_safety_intermediate_symlink_nonexistent_leaf_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = temp.path();

        let dot_git = root.join(".git");
        std::fs::create_dir_all(dot_git.join("hooks")).unwrap();

        // Intermediate directory symlink pointing outside the worktree
        let ext_dir_link = root.join("ext_dir");
        std::os::unix::fs::symlink(outside.path(), &ext_dir_link).unwrap();

        let res_ext = verify_worktree_path_safety(root, "ext_dir/nonexistent_file.txt");
        assert!(
            res_ext.is_err(),
            "Intermediate symlink escaping worktree must be rejected even when leaf does not exist"
        );

        // Intermediate directory symlink pointing into .git metadata
        let git_dir_link = root.join("git_meta_dir");
        std::os::unix::fs::symlink(".git/hooks", &git_dir_link).unwrap();

        let res_git = verify_worktree_path_safety(root, "git_meta_dir/pre-commit");
        assert!(
            res_git.is_err(),
            "Intermediate symlink into .git must be rejected even when leaf does not exist"
        );
    }

    #[test]
    fn test_build_repo_config_sanitization_sha256_and_info_attributes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let git_dir = root.join(".git");
        std::fs::create_dir_all(git_dir.join("info")).unwrap();

        std::fs::write(
            git_dir.join("config"),
            "[extensions]\n\tobjectFormat = sha256\n[filter \"custom_flt\"]\n\tclean = echo test\n",
        )
        .unwrap();
        std::fs::write(
            git_dir.join("info").join("attributes"),
            "* filter=attr_flt filter=a=b diff=attr_diff merge=attr_merge\n",
        )
        .unwrap();

        let plan = build_repo_config_sanitization(root);
        assert_eq!(plan.empty_tree_hash, SHA256_EMPTY_TREE_HEX);
        assert!(
            plan.cli_overrides
                .contains(&std::ffi::OsString::from("filter.custom_flt.clean="))
        );
        assert!(
            plan.cli_overrides
                .contains(&std::ffi::OsString::from("filter.attr_flt.clean="))
        );
        assert!(
            plan.cli_overrides
                .contains(&std::ffi::OsString::from("filter.attr_flt.smudge="))
        );
        assert!(
            plan.cli_overrides
                .contains(&std::ffi::OsString::from("filter.attr_flt.process="))
        );
        assert!(
            plan.cli_overrides
                .contains(&std::ffi::OsString::from("diff.attr_diff.textconv="))
        );
        assert!(
            plan.cli_overrides
                .contains(&std::ffi::OsString::from("merge.attr_merge.driver="))
        );
        // Keys containing `=` must NOT be emitted as malformed `-c` flags,
        // and MUST be present in `env_config_pairs` for `GIT_CONFIG_KEY_<N>` injection.
        assert!(
            !plan
                .cli_overrides
                .iter()
                .any(|s| s.to_string_lossy().contains("a=b"))
        );
        assert!(plan.env_config_pairs.contains(&(
            std::ffi::OsString::from("filter.a=b.clean"),
            std::ffi::OsString::new()
        )));
        assert!(plan.env_config_pairs.contains(&(
            std::ffi::OsString::from("filter.a=b.smudge"),
            std::ffi::OsString::new()
        )));
        assert!(plan.env_config_pairs.contains(&(
            std::ffi::OsString::from("filter.a=b.process"),
            std::ffi::OsString::new()
        )));
        assert!(plan.env_config_pairs.contains(&(
            std::ffi::OsString::from("filter.a=b.required"),
            std::ffi::OsString::from("false")
        )));
    }

    #[test]
    fn test_stream_parser_same_line_headers_quoted_semicolons_and_cache_invalidation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let git_dir = root.join(".git");
        std::fs::create_dir_all(git_dir.join("info")).unwrap();
        std::fs::create_dir_all(git_dir.join("sub;dir#1")).unwrap();

        std::fs::write(
            git_dir.join("sub;dir#1").join("extra.inc"),
            "[core][Filter \"inline]drv\"] clean = echo pwn\n",
        )
        .unwrap();
        std::fs::write(
            git_dir.join("config"),
            "[core][Include] path = \"sub;dir#1/extra.inc\" # trailing comment\n",
        )
        .unwrap();

        let plan1 = get_or_build_repo_sanitization_plan(root);
        assert!(plan1.env_config_pairs.contains(&(
            std::ffi::OsString::from("filter.inline]drv.clean"),
            std::ffi::OsString::new()
        )));

        // Mutate .git/info/attributes and verify the cache invalidates immediately
        std::fs::write(
            git_dir.join("info").join("attributes"),
            "\"*\"filter=fresh_drv\n",
        )
        .unwrap();
        let plan2 = get_or_build_repo_sanitization_plan(root);
        assert!(plan2.env_config_pairs.contains(&(
            std::ffi::OsString::from("filter.fresh_drv.clean"),
            std::ffi::OsString::new()
        )));
    }
}
