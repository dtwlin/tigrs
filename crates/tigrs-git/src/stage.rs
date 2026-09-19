// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Interactive staging and worktree operations via canonical Git plumbing.
//!
//! `tigrs` never writes
//! `.git/index` directly. All index mutations pass through Git's canonical
//! plumbing (`git update-index`, `git add`, `git restore --staged`,
//! `git apply --cached --unidiff-zero -`). This guarantees round-trip preservation
//! of index extensions (tree cache, untracked cache, split index), v4 prefix
//! compression, conflict stages, and the trailing SHA checksum.

use crate::diff::{DiffHunk, DiffLineKind};
use crate::patch::range;
use std::io::Write as _;
use std::path::Path;
#[cfg(test)]
use std::process::Command;
use std::process::Stdio;
use tigrs_core::error::{Result, TigError};

/// Stages an entire file (unstaged or untracked) using raw OS path bytes.
pub fn stage_file_os(
    work_dir: &Path,
    path: &std::ffi::OsStr,
    old_path: Option<&std::ffi::OsStr>,
) -> Result<()> {
    crate::path_security::verify_worktree_os_path_safety(work_dir, path)?;
    if let Some(old) = old_path {
        crate::path_security::verify_worktree_os_path_safety(work_dir, old)?;
    }

    let mut cmd = crate::path_security::safe_git_command(work_dir);
    cmd.args(["add", "-A", "--"]).arg(path);
    if let Some(old) = old_path {
        cmd.arg(old);
    }

    let output = cmd
        .output()
        .map_err(|err| TigError::Git(format!("Failed to execute git add: {err}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TigError::Git(format!("git add failed: {stderr}")));
    }

    Ok(())
}

/// Stages an entire file (unstaged or untracked).
pub fn stage_file(work_dir: &Path, path: &str, old_path: Option<&str>) -> Result<()> {
    stage_file_os(
        work_dir,
        std::ffi::OsStr::new(path),
        old_path.map(std::ffi::OsStr::new),
    )
}

/// Unstages an entire staged file from the index using raw OS path bytes.
pub fn unstage_file_os(
    work_dir: &Path,
    path: &std::ffi::OsStr,
    old_path: Option<&std::ffi::OsStr>,
) -> Result<()> {
    crate::path_security::verify_worktree_os_path_safety(work_dir, path)?;
    if let Some(old) = old_path {
        crate::path_security::verify_worktree_os_path_safety(work_dir, old)?;
    }

    // 1. Try git restore --staged
    let mut cmd = crate::path_security::safe_git_command(work_dir);
    cmd.args(["restore", "--staged", "--"]).arg(path);
    if let Some(old) = old_path {
        cmd.arg(old);
    }

    if let Ok(output) = cmd.output()
        && output.status.success()
    {
        return Ok(());
    }

    // 2. Fallback for older Git versions with a valid HEAD: git reset -q HEAD -- <path>
    let mut reset_cmd = crate::path_security::safe_git_command(work_dir);
    reset_cmd.args(["reset", "-q", "HEAD", "--"]).arg(path);
    if let Some(old) = old_path {
        reset_cmd.arg(old);
    }

    if let Ok(output) = reset_cmd.output()
        && output.status.success()
    {
        return Ok(());
    }

    // 3. Fallback for unborn branches (no HEAD commit yet): git rm --cached -r --quiet
    let mut rm_cmd = crate::path_security::safe_git_command(work_dir);
    rm_cmd
        .args(["rm", "--cached", "-r", "--quiet", "--"])
        .arg(path);
    if let Some(old) = old_path {
        rm_cmd.arg(old);
    }

    let output = rm_cmd
        .output()
        .map_err(|err| TigError::Git(format!("Failed to execute git rm --cached: {err}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TigError::Git(format!("git unstage failed: {stderr}")));
    }

    Ok(())
}

/// Unstages an entire staged file from the index.
pub fn unstage_file(work_dir: &Path, path: &str, old_path: Option<&str>) -> Result<()> {
    unstage_file_os(
        work_dir,
        std::ffi::OsStr::new(path),
        old_path.map(std::ffi::OsStr::new),
    )
}

/// Discards unstaged modifications in the working tree for a tracked file using raw OS path bytes.
pub fn discard_file_changes_os(
    work_dir: &Path,
    path: &std::ffi::OsStr,
    old_path: Option<&std::ffi::OsStr>,
) -> Result<()> {
    crate::path_security::verify_worktree_os_path_safety(work_dir, path)?;
    if let Some(old) = old_path {
        crate::path_security::verify_worktree_os_path_safety(work_dir, old)?;
    }

    // 1. Try git restore --worktree
    let mut cmd = crate::path_security::safe_git_command(work_dir);
    cmd.args(["restore", "--worktree", "--"]).arg(path);
    if let Some(old) = old_path {
        cmd.arg(old);
    }

    if let Ok(output) = cmd.output()
        && output.status.success()
    {
        return Ok(());
    }

    // 2. Fallback: git checkout -- <path>
    let mut co_cmd = crate::path_security::safe_git_command(work_dir);
    co_cmd.args(["checkout", "--"]).arg(path);
    if let Some(old) = old_path {
        co_cmd.arg(old);
    }

    let output = co_cmd
        .output()
        .map_err(|err| TigError::Git(format!("Failed to execute git checkout: {err}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TigError::Git(format!("git discard failed: {stderr}")));
    }

    Ok(())
}

/// Discards unstaged modifications in the working tree for a tracked file.
pub fn discard_file_changes(work_dir: &Path, path: &str, old_path: Option<&str>) -> Result<()> {
    discard_file_changes_os(
        work_dir,
        std::ffi::OsStr::new(path),
        old_path.map(std::ffi::OsStr::new),
    )
}

/// Discards an untracked file or directory using raw OS path bytes.
pub fn discard_untracked_file_os(work_dir: &Path, path: &std::ffi::OsStr) -> Result<()> {
    crate::path_security::verify_relative_os_path(path)?;
    let _ = crate::path_security::verify_worktree_os_path_safety(work_dir, path)?;

    let mut cmd = crate::path_security::safe_git_command(work_dir);
    cmd.args(["clean", "-f", "-d", "--"]).arg(path);

    let output = cmd
        .output()
        .map_err(|err| TigError::Git(format!("Failed to execute git clean: {err}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TigError::Git(format!(
            "git clean failed: {}",
            stderr.trim()
        )));
    }

    Ok(())
}

/// Discards an untracked file or directory.
pub fn discard_untracked_file(work_dir: &Path, path: &str) -> Result<()> {
    discard_untracked_file_os(work_dir, std::ffi::OsStr::new(path))
}

/// Synthesizes a raw-byte unified diff patch covering a single diff hunk.
#[must_use]
pub fn synthesize_hunk_patch_bytes(raw_path: &[u8], hunk: &DiffHunk) -> Vec<u8> {
    let mut out = Vec::new();
    let mut a_path = Vec::with_capacity(2 + raw_path.len());
    a_path.extend_from_slice(b"a/");
    a_path.extend_from_slice(raw_path);
    let mut b_path = Vec::with_capacity(2 + raw_path.len());
    b_path.extend_from_slice(b"b/");
    b_path.extend_from_slice(raw_path);

    let q_a = crate::patch::quote_path_bytes(&a_path);
    let q_b = crate::patch::quote_path_bytes(&b_path);
    let _ = writeln!(out, "diff --git {q_a} {q_b}");

    let is_add = hunk.old_start == 0 && hunk.old_len == 0;
    let is_del = hunk.new_start == 0 && hunk.new_len == 0;
    if is_add {
        let _ = writeln!(out, "new file mode 100644");
    } else if is_del {
        let _ = writeln!(out, "deleted file mode 100644");
    }
    let _ = writeln!(
        out,
        "--- {}",
        crate::patch::side_bytes(b'a', raw_path, !is_add, true)
    );
    let _ = writeln!(
        out,
        "+++ {}",
        crate::patch::side_bytes(b'b', raw_path, !is_del, true)
    );

    match &hunk.func_context {
        Some(ctx) if !ctx.is_empty() => {
            let _ = writeln!(
                out,
                "@@ -{} +{} @@ {ctx}",
                range(hunk.old_start, hunk.old_len),
                range(hunk.new_start, hunk.new_len)
            );
        }
        _ => {
            let _ = writeln!(
                out,
                "@@ -{} +{} @@",
                range(hunk.old_start, hunk.old_len),
                range(hunk.new_start, hunk.new_len)
            );
        }
    }

    for (line_idx, line) in hunk.lines.iter().enumerate() {
        let prefix = match line.kind {
            DiffLineKind::Context => b' ',
            DiffLineKind::Add => b'+',
            DiffLineKind::Remove => b'-',
        };
        out.push(prefix);
        let raw_line = crate::patch::resolve_raw_hunk_line(
            hunk.old_start,
            hunk.old_len,
            hunk.new_start,
            hunk.new_len,
            line_idx,
            &line.content,
        );
        out.extend_from_slice(&raw_line);
        out.push(b'\n');
        if line.no_newline_at_eof {
            out.extend_from_slice(b"\\ No newline at end of file\n");
        }
    }

    out
}

/// Synthesizes a unified diff patch covering a single diff hunk.
#[must_use]
pub fn synthesize_hunk_patch(path: &str, hunk: &DiffHunk) -> String {
    String::from_utf8_lossy(&synthesize_hunk_patch_bytes(path.as_bytes(), hunk)).into_owned()
}

/// Synthesizes a raw-byte unified diff patch covering one or more specific lines within a diff hunk,
/// accounting for direction (`reverse = false` for staging, `reverse = true` for unstaging).
///
/// Returns `None` if no valid non-context lines are selected.
#[must_use]
pub fn synthesize_lines_patch_directed_bytes(
    raw_path: &[u8],
    hunk: &DiffHunk,
    line_indices: &[usize],
    reverse: bool,
) -> Option<Vec<u8>> {
    let valid_targets: Vec<usize> = line_indices
        .iter()
        .copied()
        .filter(|&idx| idx < hunk.lines.len() && hunk.lines[idx].kind != DiffLineKind::Context)
        .collect();

    if valid_targets.is_empty() {
        return None;
    }

    let mut raw_patch_lines: Vec<(u8, Vec<u8>, bool)> = Vec::new();
    let mut old_len: u32 = 0;
    let mut new_len: u32 = 0;

    for (idx, line) in hunk.lines.iter().enumerate() {
        let line_bytes = || {
            crate::patch::resolve_raw_hunk_line(
                hunk.old_start,
                hunk.old_len,
                hunk.new_start,
                hunk.new_len,
                idx,
                &line.content,
            )
            .into_owned()
        };
        if valid_targets.contains(&idx) {
            match line.kind {
                DiffLineKind::Add => {
                    raw_patch_lines.push((b'+', line_bytes(), line.no_newline_at_eof));
                    new_len += 1;
                }
                DiffLineKind::Remove => {
                    raw_patch_lines.push((b'-', line_bytes(), line.no_newline_at_eof));
                    old_len += 1;
                }
                DiffLineKind::Context => unreachable!(),
            }
        } else if !reverse {
            match line.kind {
                DiffLineKind::Context | DiffLineKind::Remove => {
                    raw_patch_lines.push((b' ', line_bytes(), line.no_newline_at_eof));
                    old_len += 1;
                    new_len += 1;
                }
                DiffLineKind::Add => {}
            }
        } else {
            match line.kind {
                DiffLineKind::Context | DiffLineKind::Add => {
                    raw_patch_lines.push((b' ', line_bytes(), line.no_newline_at_eof));
                    old_len += 1;
                    new_len += 1;
                }
                DiffLineKind::Remove => {}
            }
        }
    }

    let last_old_idx = raw_patch_lines
        .iter()
        .rposition(|&(p, _, _)| p == b' ' || p == b'-');
    let last_new_idx = raw_patch_lines
        .iter()
        .rposition(|&(p, _, _)| p == b' ' || p == b'+');

    let mut patch_lines: Vec<(u8, Vec<u8>, bool)> = Vec::with_capacity(raw_patch_lines.len() + 2);
    for (idx, (prefix, content, no_nl)) in raw_patch_lines.into_iter().enumerate() {
        if no_nl && prefix == b' ' {
            let is_last_old = last_old_idx == Some(idx);
            let is_last_new = last_new_idx == Some(idx);
            if is_last_old && !is_last_new {
                patch_lines.push((b'-', content.clone(), true));
                patch_lines.push((b'+', content, false));
                continue;
            } else if is_last_new && !is_last_old {
                patch_lines.push((b'-', content.clone(), false));
                patch_lines.push((b'+', content, true));
                continue;
            } else if !is_last_old && !is_last_new {
                patch_lines.push((b' ', content, false));
                continue;
            }
        }
        let valid_no_nl = no_nl
            && ((prefix == b'-' && last_old_idx == Some(idx))
                || (prefix == b'+' && last_new_idx == Some(idx))
                || (prefix == b' ' && last_old_idx == Some(idx) && last_new_idx == Some(idx)));
        patch_lines.push((prefix, content, valid_no_nl));
    }

    let mut out = Vec::new();
    let mut a_path = Vec::with_capacity(2 + raw_path.len());
    a_path.extend_from_slice(b"a/");
    a_path.extend_from_slice(raw_path);
    let mut b_path = Vec::with_capacity(2 + raw_path.len());
    b_path.extend_from_slice(b"b/");
    b_path.extend_from_slice(raw_path);

    let q_a = crate::patch::quote_path_bytes(&a_path);
    let q_b = crate::patch::quote_path_bytes(&b_path);
    let _ = writeln!(out, "diff --git {q_a} {q_b}");

    let is_add = hunk.old_start == 0 && old_len == 0;
    let is_del = hunk.new_start == 0 && new_len == 0;
    if is_add {
        let _ = writeln!(out, "new file mode 100644");
    } else if is_del {
        let _ = writeln!(out, "deleted file mode 100644");
    }
    let _ = writeln!(
        out,
        "--- {}",
        crate::patch::side_bytes(b'a', raw_path, !is_add, true)
    );
    let _ = writeln!(
        out,
        "+++ {}",
        crate::patch::side_bytes(b'b', raw_path, !is_del, true)
    );

    let old_start = if is_add { 0 } else { hunk.old_start.max(1) };
    let new_start = if is_del { 0 } else { hunk.new_start.max(1) };
    let _ = writeln!(
        out,
        "@@ -{} +{} @@",
        range(old_start, old_len),
        range(new_start, new_len)
    );

    for (prefix, content, no_nl) in patch_lines {
        out.push(prefix);
        out.extend_from_slice(&content);
        out.push(b'\n');
        if no_nl {
            out.extend_from_slice(b"\\ No newline at end of file\n");
        }
    }

    Some(out)
}

/// Synthesizes a unified diff patch covering one or more specific lines within a diff hunk,
/// accounting for direction (`reverse = false` for staging, `reverse = true` for unstaging).
///
/// Returns `None` if no valid non-context lines are selected.
#[must_use]
pub fn synthesize_lines_patch_directed(
    path: &str,
    hunk: &DiffHunk,
    line_indices: &[usize],
    reverse: bool,
) -> Option<String> {
    let bytes =
        synthesize_lines_patch_directed_bytes(path.as_bytes(), hunk, line_indices, reverse)?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Synthesizes a unified diff patch covering one or more specific lines within a diff hunk for forward staging.
pub fn synthesize_lines_patch(
    path: &str,
    hunk: &DiffHunk,
    line_indices: &[usize],
) -> Option<String> {
    synthesize_lines_patch_directed(path, hunk, line_indices, false)
}

/// Synthesizes a unified diff patch covering a single line within a diff hunk.
pub fn synthesize_line_patch(path: &str, hunk: &DiffHunk, line_idx: usize) -> Option<String> {
    synthesize_lines_patch_directed(path, hunk, &[line_idx], false)
}

/// Applies a raw-byte patch directly to the index or working tree using `git apply`.
pub fn apply_patch_bytes(
    work_dir: &Path,
    patch_bytes: &[u8],
    reverse: bool,
    cached: bool,
) -> Result<()> {
    let mut cmd = crate::path_security::safe_git_command(work_dir);
    cmd.arg("apply");
    if cached {
        cmd.arg("--cached");
    }
    cmd.args(["--unidiff-zero", "--whitespace=nowarn"]);
    if reverse {
        cmd.arg("--reverse");
    }
    cmd.arg("-");
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|err| TigError::Git(format!("Failed to spawn git apply: {err}")))?;

    let mut stdin = child.stdin.take();
    if let Some(ref pipe) = stdin {
        crate::status::try_set_pipe_size_1mb(pipe);
    }

    let (write_res, output_res) = std::thread::scope(|s| {
        let writer = s.spawn(move || -> std::io::Result<()> {
            if let Some(mut pipe) = stdin.take()
                && let Err(err) = pipe.write_all(patch_bytes)
                && err.kind() != std::io::ErrorKind::BrokenPipe
            {
                return Err(err);
            }
            Ok(())
        });

        let output = child.wait_with_output();
        let write_res = writer.join().unwrap_or_else(|_| {
            Err(std::io::Error::other(
                "git apply stdin writer thread panicked",
            ))
        });
        (write_res, output)
    });

    let output =
        output_res.map_err(|err| TigError::Git(format!("Failed to wait for git apply: {err}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TigError::Git(format!(
            "git apply failed: {}",
            stderr.trim()
        )));
    }

    write_res.map_err(|err| TigError::Git(format!("Failed to write patch to git apply: {err}")))?;

    Ok(())
}

/// Applies a patch directly to the index or working tree using `git apply`.
pub fn apply_patch(work_dir: &Path, patch: &str, reverse: bool, cached: bool) -> Result<()> {
    apply_patch_bytes(work_dir, patch.as_bytes(), reverse, cached)
}

/// Stages a single hunk into the index using raw path bytes.
pub fn stage_hunk_bytes(work_dir: &Path, raw_path: &[u8], hunk: &DiffHunk) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        crate::path_security::verify_relative_os_path(std::ffi::OsStr::from_bytes(raw_path))?;
    }
    let hunk_patch = synthesize_hunk_patch_bytes(raw_path, hunk);
    apply_patch_bytes(work_dir, &hunk_patch, false, true)
}

/// Stages a single hunk into the index.
pub fn stage_hunk(work_dir: &Path, path: &str, hunk: &DiffHunk) -> Result<()> {
    crate::path_security::verify_relative_path(path)?;
    stage_hunk_bytes(work_dir, path.as_bytes(), hunk)
}

/// Unstages a single hunk from the index using raw path bytes.
pub fn unstage_hunk_bytes(work_dir: &Path, raw_path: &[u8], hunk: &DiffHunk) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        crate::path_security::verify_relative_os_path(std::ffi::OsStr::from_bytes(raw_path))?;
    }
    let hunk_patch = synthesize_hunk_patch_bytes(raw_path, hunk);
    apply_patch_bytes(work_dir, &hunk_patch, true, true)
}

/// Unstages a single hunk from the index.
pub fn unstage_hunk(work_dir: &Path, path: &str, hunk: &DiffHunk) -> Result<()> {
    crate::path_security::verify_relative_path(path)?;
    unstage_hunk_bytes(work_dir, path.as_bytes(), hunk)
}

/// Stages one or more lines within a hunk into the index using raw path bytes.
pub fn stage_lines_bytes(
    work_dir: &Path,
    raw_path: &[u8],
    hunk: &DiffHunk,
    line_indices: &[usize],
) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        crate::path_security::verify_relative_os_path(std::ffi::OsStr::from_bytes(raw_path))?;
    }
    if let Some(line_patch) =
        synthesize_lines_patch_directed_bytes(raw_path, hunk, line_indices, false)
    {
        apply_patch_bytes(work_dir, &line_patch, false, true)
    } else {
        Ok(())
    }
}

/// Stages one or more lines within a hunk into the index.
pub fn stage_lines(
    work_dir: &Path,
    path: &str,
    hunk: &DiffHunk,
    line_indices: &[usize],
) -> Result<()> {
    crate::path_security::verify_relative_path(path)?;
    stage_lines_bytes(work_dir, path.as_bytes(), hunk, line_indices)
}

/// Unstages one or more lines within a hunk from the index using raw path bytes.
pub fn unstage_lines_bytes(
    work_dir: &Path,
    raw_path: &[u8],
    hunk: &DiffHunk,
    line_indices: &[usize],
) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        crate::path_security::verify_relative_os_path(std::ffi::OsStr::from_bytes(raw_path))?;
    }
    if let Some(line_patch) =
        synthesize_lines_patch_directed_bytes(raw_path, hunk, line_indices, true)
    {
        apply_patch_bytes(work_dir, &line_patch, true, true)
    } else {
        Ok(())
    }
}

/// Unstages one or more lines within a hunk from the index.
pub fn unstage_lines(
    work_dir: &Path,
    path: &str,
    hunk: &DiffHunk,
    line_indices: &[usize],
) -> Result<()> {
    crate::path_security::verify_relative_path(path)?;
    unstage_lines_bytes(work_dir, path.as_bytes(), hunk, line_indices)
}

/// Stages a single line within a hunk into the index.
pub fn stage_line(work_dir: &Path, path: &str, hunk: &DiffHunk, line_idx: usize) -> Result<()> {
    stage_lines(work_dir, path, hunk, &[line_idx])
}

/// Unstages a single line within a hunk from the index.
pub fn unstage_line(work_dir: &Path, path: &str, hunk: &DiffHunk, line_idx: usize) -> Result<()> {
    unstage_lines(work_dir, path, hunk, &[line_idx])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::HunkLine;
    use std::fs;

    fn create_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();

        let run = |args: &[&str]| {
            let status = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(&path)
                .status()
                .expect("git cmd");
            assert!(status.success());
        };

        run(&["init"]);
        run(&["config", "user.name", "Tester"]);
        run(&["config", "user.email", "tester@example.com"]);
        run(&["config", "core.autocrlf", "false"]);
        run(&["config", "commit.gpgsign", "false"]);

        (dir, path)
    }

    #[test]
    fn test_stage_and_unstage_file() {
        let (_dir, path) = create_test_repo();
        let file_path = path.join("file.txt");
        fs::write(&file_path, "hello world\n").expect("write");

        // 1. Stage file
        stage_file(&path, "file.txt", None).expect("stage_file");

        let output = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["status", "--porcelain=v2"])
            .current_dir(&path)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.starts_with("1 A."));

        // 2. Unstage file
        unstage_file(&path, "file.txt", None).expect("unstage_file");

        let output = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["status", "--porcelain=v2"])
            .current_dir(&path)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.starts_with("? file.txt"));
    }

    #[test]
    fn test_discard_file_changes() {
        let (_dir, path) = create_test_repo();
        let file_path = path.join("file.txt");
        fs::write(&file_path, "initial content\n").expect("write");
        stage_file(&path, "file.txt", None).expect("stage");

        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "initial"])
            .current_dir(&path)
            .status()
            .unwrap();
        assert!(status.success());

        // Modify file
        fs::write(&file_path, "modified content\n").expect("write");

        discard_file_changes(&path, "file.txt", None).expect("discard");
        let content = fs::read_to_string(&file_path).expect("read");
        assert_eq!(content, "initial content\n");
    }

    #[test]
    fn test_stage_and_unstage_hunk() {
        let (_dir, path) = create_test_repo();
        let file_path = path.join("multihunk.txt");
        fs::write(&file_path, "line1\nline2\nline3\nline4\nline5\n").expect("write");
        stage_file(&path, "multihunk.txt", None).expect("stage");
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "initial"])
            .current_dir(&path)
            .status()
            .unwrap();
        assert!(status.success());

        // Modify lines
        fs::write(&file_path, "line1_mod\nline2\nline3\nline4\nline5_mod\n").expect("write");

        // Construct hunk for line1
        let hunk1 = DiffHunk {
            old_start: 1,
            old_len: 3,
            new_start: 1,
            new_len: 3,
            func_context: None,
            lines: vec![
                HunkLine {
                    kind: DiffLineKind::Remove,
                    content: "line1".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Add,
                    content: "line1_mod".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Context,
                    content: "line2".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Context,
                    content: "line3".to_string(),
                    no_newline_at_eof: false,
                },
            ],
        };

        stage_hunk(&path, "multihunk.txt", &hunk1).expect("stage_hunk");

        let diff = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["diff", "--cached"])
            .current_dir(&path)
            .output()
            .unwrap();
        let diff_str = String::from_utf8_lossy(&diff.stdout);
        assert!(diff_str.contains("+line1_mod"));
        assert!(!diff_str.contains("+line5_mod"));

        // Unstage hunk1
        unstage_hunk(&path, "multihunk.txt", &hunk1).expect("unstage_hunk");
        let diff_after = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["diff", "--cached"])
            .current_dir(&path)
            .output()
            .unwrap();
        let diff_after_str = String::from_utf8_lossy(&diff_after.stdout);
        assert!(!diff_after_str.contains("+line1_mod"));
    }

    #[test]
    fn test_stage_single_line() {
        let (_dir, path) = create_test_repo();
        let file_path = path.join("singleline.txt");
        fs::write(&file_path, "alpha\nbeta\ngamma\n").expect("write");
        stage_file(&path, "singleline.txt", None).expect("stage");
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "initial"])
            .current_dir(&path)
            .status()
            .unwrap();
        assert!(status.success());

        // Modify alpha and add delta
        fs::write(&file_path, "alpha_mod\nbeta\ngamma\ndelta\n").expect("write");

        let hunk = DiffHunk {
            old_start: 1,
            old_len: 3,
            new_start: 1,
            new_len: 4,
            func_context: None,
            lines: vec![
                HunkLine {
                    kind: DiffLineKind::Remove,
                    content: "alpha".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Add,
                    content: "alpha_mod".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Context,
                    content: "beta".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Context,
                    content: "gamma".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Add,
                    content: "delta".to_string(),
                    no_newline_at_eof: false,
                },
            ],
        };

        // Stage line_idx 4 ("delta")
        stage_line(&path, "singleline.txt", &hunk, 4).expect("stage_line");

        let diff = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["diff", "--cached"])
            .current_dir(&path)
            .output()
            .unwrap();
        let diff_str = String::from_utf8_lossy(&diff.stdout);
        assert!(diff_str.contains("+delta"));
        assert!(!diff_str.contains("+alpha_mod"));

        // Unstage delta from the staged diff (HEAD vs index)
        let staged_item = crate::status::StatusItem::new(
            'M',
            crate::status::StatusSection::Staged,
            "singleline.txt",
            None,
        );
        let staged_diff =
            crate::status::compute_status_item_diff(&path, &staged_item).expect("staged diff");
        let staged_hunk = &staged_diff.files[0].hunks[0];
        let delta_idx = staged_hunk
            .lines
            .iter()
            .position(|l| l.kind == DiffLineKind::Add && l.content == "delta")
            .expect("delta in staged hunk");
        unstage_line(&path, "singleline.txt", staged_hunk, delta_idx).expect("unstage_line");
        let diff_after = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["diff", "--cached"])
            .current_dir(&path)
            .output()
            .unwrap();
        let diff_after_str = String::from_utf8_lossy(&diff_after.stdout);
        assert!(!diff_after_str.contains("+delta"));
    }

    #[test]
    fn test_stage_hunk_crlf_preservation() {
        let (_dir, path) = create_test_repo();
        let file_path = path.join("crlf.txt");

        // Write file with explicit CRLF endings
        fs::write(&file_path, b"line1\r\nline2\r\nline3\r\n").expect("write");
        stage_file(&path, "crlf.txt", None).expect("stage");
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "add crlf.txt"])
            .current_dir(&path)
            .status()
            .unwrap();
        assert!(status.success());

        // Modify line2
        fs::write(&file_path, b"line1\r\nline2_modified\r\nline3\r\n").expect("write modified");

        let hunk = DiffHunk {
            old_start: 1,
            old_len: 3,
            new_start: 1,
            new_len: 3,
            func_context: None,
            lines: vec![
                HunkLine {
                    kind: DiffLineKind::Context,
                    content: "line1\r".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Remove,
                    content: "line2\r".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Add,
                    content: "line2_modified\r".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Context,
                    content: "line3\r".to_string(),
                    no_newline_at_eof: false,
                },
            ],
        };

        stage_hunk(&path, "crlf.txt", &hunk).expect("stage_hunk with crlf");

        // Verify staged index blob maintains CRLF
        let show = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["show", ":crlf.txt"])
            .current_dir(&path)
            .output()
            .unwrap();
        assert!(show.status.success());
        assert_eq!(show.stdout, b"line1\r\nline2_modified\r\nline3\r\n");
    }

    #[test]
    fn test_stage_and_unstage_untracked_directory_tree() {
        let (_dir, path) = create_test_repo();
        let nested_dir = path.join("sub").join("nested").join("deep");
        fs::create_dir_all(&nested_dir).expect("create nested dirs");
        fs::write(nested_dir.join("a.txt"), "hello a\n").expect("write a");
        fs::write(nested_dir.join("b.txt"), "hello b\n").expect("write b");

        // Stage entire untracked directory
        stage_file(&path, "sub", None).expect("stage untracked dir");

        let cached_diff = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["diff", "--cached", "--name-only"])
            .current_dir(&path)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&cached_diff.stdout);
        assert!(stdout.contains("sub/nested/deep/a.txt"));
        assert!(stdout.contains("sub/nested/deep/b.txt"));

        // Unstage entire directory tree
        unstage_file(&path, "sub", None).expect("unstage directory");

        let cached_after = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["diff", "--cached", "--name-only"])
            .current_dir(&path)
            .output()
            .unwrap();
        let stdout_after = String::from_utf8_lossy(&cached_after.stdout);
        assert!(!stdout_after.contains("sub/nested/deep/a.txt"));
        assert!(!stdout_after.contains("sub/nested/deep/b.txt"));
    }

    #[test]
    fn test_discard_untracked_directory_tree() {
        let (_dir, path) = create_test_repo();
        let dir_to_discard = path.join("untracked_dir").join("child");
        fs::create_dir_all(&dir_to_discard).expect("create dir");
        fs::write(dir_to_discard.join("junk.txt"), "garbage").expect("write junk");

        assert!(path.join("untracked_dir").exists());
        discard_untracked_file(&path, "untracked_dir").expect("discard untracked dir");
        assert!(!path.join("untracked_dir").exists());
    }

    #[test]
    fn test_discard_file_changes_tracked() {
        let (_dir, path) = create_test_repo();
        let tracked_file = path.join("tracked.txt");
        fs::write(&tracked_file, "original\n").expect("write");
        stage_file(&path, "tracked.txt", None).expect("stage");
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "initial tracked"])
            .current_dir(&path)
            .status()
            .unwrap();
        assert!(status.success());

        // Modify working copy
        fs::write(&tracked_file, "dirtied content\n").expect("write dirty");

        // Discard changes
        discard_file_changes(&path, "tracked.txt", None).expect("discard changes");
        let content = fs::read_to_string(&tracked_file).expect("read");
        assert_eq!(content, "original\n");
    }

    #[test]
    fn test_unstage_and_discard_file_with_old_path() {
        let (_dir, path) = create_test_repo();
        let old_file = path.join("old.txt");
        fs::write(&old_file, "orig content\n").expect("write");
        stage_file(&path, "old.txt", None).expect("stage");
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "commit old"])
            .current_dir(&path)
            .status()
            .unwrap();
        assert!(status.success());

        // Rename via git mv
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["mv", "old.txt", "new.txt"])
            .current_dir(&path)
            .status()
            .unwrap();
        assert!(status.success());

        // Unstage rename with old_path
        unstage_file(&path, "new.txt", Some("old.txt")).expect("unstage rename");

        // Now old.txt is missing in working tree, restore it via discard_file_changes
        discard_file_changes(&path, "old.txt", None).expect("discard old.txt deletion");
        assert!(path.join("old.txt").exists());

        // Discard untracked new.txt
        discard_untracked_file(&path, "new.txt").expect("discard untracked new");
        assert!(!path.join("new.txt").exists());

        // Security check: passing path escaping repo fails
        assert!(stage_file(&path, "../escape.txt", None).is_err());
        assert!(unstage_file(&path, "../escape.txt", None).is_err());
        assert!(discard_file_changes(&path, "../escape.txt", None).is_err());
        assert!(discard_untracked_file(&path, "../escape.txt").is_err());
    }

    #[test]
    fn test_discard_untracked_single_file_and_symlink() {
        let (_dir, path) = create_test_repo();
        let target_file = path.join("single_junk.txt");
        fs::write(&target_file, "trash").expect("write");
        assert!(target_file.exists());
        discard_untracked_file(&path, "single_junk.txt").expect("discard file");
        assert!(!target_file.exists());

        // Symlink
        let real_file = path.join("real.txt");
        fs::write(&real_file, "real").expect("write");
        let link_file = path.join("symlink_junk.txt");
        std::os::unix::fs::symlink(&real_file, &link_file).expect("symlink");
        assert!(link_file.exists());
        discard_untracked_file(&path, "symlink_junk.txt").expect("discard symlink");
        assert!(!link_file.exists());
    }

    #[test]
    fn test_unstage_hunk_func_context_and_remove_line() {
        let (_dir, path) = create_test_repo();
        let file_path = path.join("func_ctx.rs");
        fs::write(&file_path, "fn hello() {\n    line1\n    line2\n}\n").expect("write");
        stage_file(&path, "func_ctx.rs", None).expect("stage");
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "add func_ctx.rs"])
            .current_dir(&path)
            .status()
            .unwrap();
        assert!(status.success());

        // Delete line2
        fs::write(&file_path, "fn hello() {\n    line1\n}\n").expect("write mod");

        let hunk = DiffHunk {
            old_start: 1,
            old_len: 4,
            new_start: 1,
            new_len: 3,
            func_context: Some("fn hello()".to_string()),
            lines: vec![
                HunkLine {
                    kind: DiffLineKind::Context,
                    content: "fn hello() {".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Context,
                    content: "    line1".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Remove,
                    content: "    line2".to_string(),
                    no_newline_at_eof: false,
                },
                HunkLine {
                    kind: DiffLineKind::Context,
                    content: "}".to_string(),
                    no_newline_at_eof: false,
                },
            ],
        };

        // Stage removal of line2
        stage_hunk(&path, "func_ctx.rs", &hunk).expect("stage hunk");

        // Unstage line 2 (which is DiffLineKind::Remove)
        unstage_line(&path, "func_ctx.rs", &hunk, 2).expect("unstage remove line");

        // Re-stage and unstage hunk with func_context
        stage_hunk(&path, "func_ctx.rs", &hunk).expect("stage hunk again");
        unstage_hunk(&path, "func_ctx.rs", &hunk).expect("unstage hunk with func_context");
    }

    #[test]
    fn test_apply_patch_large_payload_early_error_preserves_stderr_without_deadlock() {
        let (_dir, path) = create_test_repo();
        // Create a malformed >256 KiB patch so git apply rejects line 1 immediately
        // while the writer attempts to push well beyond OS pipe buffer limits.
        let mut huge_patch = String::with_capacity(300_000);
        huge_patch.push_str("diff --git a/nonexistent.txt b/nonexistent.txt\n");
        huge_patch.push_str("--- a/nonexistent.txt\n+++ b/nonexistent.txt\n");
        huge_patch.push_str("@@ -1,1 +1,1 @@\n-corrupt\n+corrupt\n");
        for _ in 0..5000 {
            huge_patch.push_str("+padding line to exceed OS pipe buffer 0123456789abcdef\n");
        }

        let err = apply_patch(&path, &huge_patch, false, true)
            .expect_err("expected malformed patch to fail");
        let msg = err.to_string();
        assert!(
            msg.contains("git apply failed:"),
            "expected stderr diagnostic in error message, got: {msg}"
        );
    }

    #[test]
    fn test_parse_unified_diff_crlf_staging_roundtrip() {
        let (_dir, path) = create_test_repo();
        let file_path = path.join("crlf.txt");
        fs::write(&file_path, b"line1\r\nline2\r\nline3\r\n").unwrap();

        std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["add", "crlf.txt"])
            .current_dir(&path)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "Add crlf.txt"])
            .current_dir(&path)
            .output()
            .unwrap();

        // Modify CRLF file in working tree
        fs::write(&file_path, b"line1\r\nline2_mod\r\nline3\r\n").unwrap();

        // Compute status item diff via status::compute_status_item_diff (which calls parse_unified_diff)
        let item = crate::status::StatusItem::new(
            'M',
            crate::status::StatusSection::Unstaged,
            "crlf.txt",
            None,
        );
        let diff = crate::status::compute_status_item_diff(&path, &item).expect("compute diff");
        assert_eq!(diff.files.len(), 1);
        let hunk = &diff.files[0].hunks[0];

        // Stage hunk into index: must succeed because parse_unified_diff preserved \r on CRLF hunk lines
        stage_hunk(&path, "crlf.txt", hunk).expect("stage CRLF hunk");

        // Unstage hunk from index
        unstage_hunk(&path, "crlf.txt", hunk).expect("unstage CRLF hunk");
    }

    #[test]
    fn test_unstage_lines_reverse_preserves_unselected_additions() {
        let (_dir, path) = create_test_repo();
        let file_path = path.join("multi.txt");
        fs::write(&file_path, "base\n").unwrap();

        std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["add", "multi.txt"])
            .current_dir(&path)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "Base commit"])
            .current_dir(&path)
            .output()
            .unwrap();

        // Add two lines and stage both
        fs::write(&file_path, "base\nadd_one\nadd_two\n").unwrap();
        std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["add", "multi.txt"])
            .current_dir(&path)
            .output()
            .unwrap();

        // Get staged diff hunk
        let item = crate::status::StatusItem::new(
            'M',
            crate::status::StatusSection::Staged,
            "multi.txt",
            None,
        );
        let staged_diff =
            crate::status::compute_status_item_diff(&path, &item).expect("staged diff");
        let hunk = &staged_diff.files[0].hunks[0];

        // Unstage only the second added line (`add_two` at index 2); `add_one` (index 1) must remain staged
        unstage_line(&path, "multi.txt", hunk, 2).expect("unstage single line");

        let after_diff =
            crate::status::compute_status_item_diff(&path, &item).expect("staged diff after");
        let remaining_hunk = &after_diff.files[0].hunks[0];
        assert!(
            remaining_hunk
                .lines
                .iter()
                .any(|l| l.kind == DiffLineKind::Add && l.content == "add_one"),
            "unselected addition add_one must remain staged in index"
        );
        assert!(
            !remaining_hunk
                .lines
                .iter()
                .any(|l| l.kind == DiffLineKind::Add && l.content == "add_two"),
            "unstaged addition add_two must be removed from index"
        );
    }

    #[test]
    fn test_stage_and_unstage_hunk_crlf_with_autocrlf_input() {
        let (_dir, path) = create_test_repo();
        let file_path = path.join("crlf_input.txt");

        // Commit initial file with CRLF in the index while core.autocrlf=false
        fs::write(&file_path, b"line1\r\nline2\r\nline3\r\n").expect("write");
        stage_file(&path, "crlf_input.txt", None).expect("stage");
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["commit", "-m", "add crlf_input.txt"])
            .current_dir(&path)
            .status()
            .unwrap();
        assert!(status.success());

        // Enable core.autocrlf=input and modify the CRLF file
        let cfg_status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["config", "core.autocrlf", "input"])
            .current_dir(&path)
            .status()
            .unwrap();
        assert!(cfg_status.success());

        fs::write(&file_path, b"line1\r\nline2_modified\r\nline3\r\n").expect("write modified");

        let unstaged_item = crate::status::StatusItem::new(
            'M',
            crate::status::StatusSection::Unstaged,
            "crlf_input.txt",
            None,
        );
        let unstaged_diff =
            crate::status::compute_status_item_diff(&path, &unstaged_item).expect("unstaged diff");
        let hunk = &unstaged_diff.files[0].hunks[0];

        stage_hunk(&path, "crlf_input.txt", hunk).expect("stage_hunk under autocrlf=input");

        let staged_item = crate::status::StatusItem::new(
            'M',
            crate::status::StatusSection::Staged,
            "crlf_input.txt",
            None,
        );
        let staged_diff =
            crate::status::compute_status_item_diff(&path, &staged_item).expect("staged diff");
        let staged_hunk = &staged_diff.files[0].hunks[0];
        unstage_hunk(&path, "crlf_input.txt", staged_hunk)
            .expect("unstage_hunk under autocrlf=input");
    }
}
