// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Incremental file annotation and blame engine using in-process `gix`
//! with automatic Tier-2 `git blame --line-porcelain` fallback.

use crate::tree::{decode_blob_lines, is_binary_data};
use gix::ObjectId;
use gix::bstr::ByteSlice;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tigrs_core::ansi::strip_control_chars;
use tigrs_core::error::{Result, TigError};

/// Number of hex characters shown in the blame view's abbreviated commit column.
const SHORT_COMMIT_ID_LEN: usize = 8;

/// Formats a Unix epoch timestamp (seconds) into a `YYYY-MM-DD` civil date string.
#[inline]
pub fn format_epoch_date(secs: i64) -> String {
    let mut buf = [0u8; 10];
    crate::types::format_epoch_date_buf(secs, &mut buf).to_string()
}

/// A single blamed line in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    /// 1-based line number in the blamed file.
    pub line_number: usize,
    /// The commit ID that introduced or last modified this line.
    pub commit_id: ObjectId,
    /// 8-character short commit hash.
    pub short_commit_id: Arc<str>,
    /// Author name of the commit.
    pub author: Arc<str>,
    /// Author timestamp formatted as YYYY-MM-DD.
    pub author_date: Arc<str>,
    /// One-line commit summary / title.
    pub summary: Arc<str>,
    /// The text content of the line.
    pub content: String,
    /// True if this line is the start of a hunk from this commit.
    pub is_hunk_start: bool,
    /// The parent commit ID of `commit_id`, if one exists (for `,` navigation).
    pub parent_commit_id: Option<ObjectId>,
    /// Original file path in the source commit (if renamed).
    pub source_path: Option<String>,
    /// 1-based line number in the source commit.
    pub source_line_number: usize,
}

/// The result of blaming a file.
#[derive(Debug, Clone)]
pub struct BlameResult {
    /// The commit ID at which the blame was computed.
    pub commit_id: ObjectId,
    /// The path of the file blamed.
    pub path: String,
    /// Whether the file is binary (cannot be blamed/annotated).
    pub is_binary: bool,
    /// All blamed lines in order (1..=N).
    pub lines: Vec<BlameLine>,
}

/// Computes blame annotation for `path` at `commit_oid` using in-process `gix`,
/// falling back to `git blame --line-porcelain` if necessary.
pub fn compute_blame(
    repo: &gix::Repository,
    work_dir: Option<&Path>,
    commit_oid: ObjectId,
    path: &str,
) -> Result<BlameResult> {
    compute_blame_cancellable(
        repo,
        work_dir,
        commit_oid,
        path,
        &tigrs_core::CancellationToken::none(),
    )
}

/// Cancellable variant of [`compute_blame`].
pub fn compute_blame_cancellable(
    repo: &gix::Repository,
    work_dir: Option<&Path>,
    commit_oid: ObjectId,
    path: &str,
    cancel: &tigrs_core::CancellationToken,
) -> Result<BlameResult> {
    cancel.check_cancelled()?;
    match compute_blame_gix_cancellable(repo, commit_oid, path, cancel) {
        Ok(result) => Ok(result),
        Err(TigError::Cancelled) => Err(TigError::Cancelled),
        Err(_err) => compute_blame_via_cli_cancellable(work_dir, Some(commit_oid), path, cancel),
    }
}

#[derive(Clone)]
struct CommitMeta {
    short_id: Arc<str>,
    author: Arc<str>,
    date: Arc<str>,
    summary: Arc<str>,
    parent_id: Option<ObjectId>,
}

/// In-process blame via `gix`.
pub fn compute_blame_gix(
    repo: &gix::Repository,
    commit_oid: ObjectId,
    path: &str,
) -> Result<BlameResult> {
    compute_blame_gix_cancellable(
        repo,
        commit_oid,
        path,
        &tigrs_core::CancellationToken::none(),
    )
}

/// Cancellable in-process blame via `gix`.
pub fn compute_blame_gix_cancellable(
    repo: &gix::Repository,
    commit_oid: ObjectId,
    path: &str,
    cancel: &tigrs_core::CancellationToken,
) -> Result<BlameResult> {
    cancel.check_cancelled()?;
    let clean_path = crate::path_security::verify_relative_path(path)?;
    let bpath = clean_path.as_bytes().as_bstr();
    let options = gix::repository::blame_file::Options::default();

    let outcome = repo
        .blame_file(bpath, commit_oid, options)
        .map_err(|e| TigError::Git(format!("gix blame failed: {e}")))?;
    cancel.check_cancelled()?;

    if is_binary_data(&outcome.blob) {
        return Ok(BlameResult {
            commit_id: commit_oid,
            path: clean_path.clone(),
            is_binary: true,
            lines: Vec::new(),
        });
    }

    let (_, text_lines) = decode_blob_lines(&outcome.blob);
    if text_lines.is_empty() {
        return Ok(BlameResult {
            commit_id: commit_oid,
            path: clean_path.clone(),
            is_binary: false,
            lines: Vec::new(),
        });
    }

    let mut commit_cache: HashMap<ObjectId, CommitMeta> = HashMap::new();

    let mut get_meta = |cid: ObjectId| -> CommitMeta {
        if let Some(m) = commit_cache.get(&cid) {
            return m.clone();
        }

        let hex = cid.to_hex().to_string();
        let short_id: Arc<str> = Arc::from(&hex[..SHORT_COMMIT_ID_LEN.min(hex.len())]);

        let (author, date, summary, parent_id) = match repo
            .find_object(cid)
            .ok()
            .and_then(|obj| obj.peel_to_kind(gix::object::Kind::Commit).ok())
            .and_then(|obj| obj.try_into_commit().ok())
        {
            Some(commit) => {
                let author_name: Arc<str> = commit.author().map_or_else(
                    |_| Arc::from("<unknown>"),
                    |a| Arc::from(&*strip_control_chars(&a.name.to_str_lossy())),
                );
                let time_secs = commit
                    .author()
                    .ok()
                    .and_then(|a| a.time().ok())
                    .map_or(0, |t| t.seconds);
                let date_str: Arc<str> = Arc::from(format_epoch_date(time_secs).as_str());
                let title: Arc<str> = commit.message().map_or_else(
                    |_| Arc::from(""),
                    |m| Arc::from(&*strip_control_chars(&m.summary().to_str_lossy())),
                );
                let parent = commit.parent_ids().next().map(gix::Id::detach);
                (author_name, date_str, title, parent)
            }
            None => (
                Arc::from("<unknown>"),
                Arc::from("1970-01-01"),
                Arc::from(""),
                None,
            ),
        };

        let meta = CommitMeta {
            short_id,
            author,
            date,
            summary,
            parent_id,
        };
        commit_cache.insert(cid, meta.clone());
        meta
    };

    let mut lines = Vec::with_capacity(text_lines.len());
    let mut current_entry_idx = 0;

    for (line_idx, content) in text_lines.into_iter().enumerate() {
        if line_idx % 256 == 0 {
            cancel.check_cancelled()?;
        }
        let line_num_0 = line_idx as u32;

        while current_entry_idx < outcome.entries.len() {
            let entry = &outcome.entries[current_entry_idx];
            let entry_end = entry.start_in_blamed_file + entry.len.get();
            if line_num_0 < entry.start_in_blamed_file {
                break;
            }
            if line_num_0 < entry_end {
                break;
            }
            current_entry_idx += 1;
        }

        if let Some(entry) = outcome.entries.get(current_entry_idx)
            && line_num_0 >= entry.start_in_blamed_file
            && line_num_0 < entry.start_in_blamed_file + entry.len.get()
        {
            let offset = line_num_0 - entry.start_in_blamed_file;
            let is_hunk_start = offset == 0;
            let source_line = (entry.start_in_source_file + offset + 1) as usize;
            let meta = get_meta(entry.commit_id);

            lines.push(BlameLine {
                line_number: line_idx + 1,
                commit_id: entry.commit_id,
                short_commit_id: meta.short_id,
                author: meta.author,
                author_date: meta.date,
                summary: meta.summary,
                content: content.to_string(),
                is_hunk_start,
                parent_commit_id: meta.parent_id,
                source_path: entry
                    .source_file_name
                    .as_ref()
                    .map(std::string::ToString::to_string),
                source_line_number: source_line,
            });
            continue;
        }

        lines.push(BlameLine {
            line_number: line_idx + 1,
            commit_id: commit_oid,
            short_commit_id: {
                let hex = commit_oid.to_hex().to_string();
                Arc::from(&hex[..SHORT_COMMIT_ID_LEN.min(hex.len())])
            },
            author: Arc::from("<unknown>"),
            author_date: Arc::from(""),
            summary: Arc::from(""),
            content: content.to_string(),
            is_hunk_start: line_idx == 0,
            parent_commit_id: None,
            source_path: None,
            source_line_number: line_idx + 1,
        });
    }

    Ok(BlameResult {
        commit_id: commit_oid,
        path: clean_path.clone(),
        is_binary: false,
        lines,
    })
}

/// Fallback blame via `git blame --line-porcelain`.
pub fn compute_blame_via_cli(
    work_dir: Option<&Path>,
    commit_oid: Option<ObjectId>,
    path: &str,
) -> Result<BlameResult> {
    compute_blame_via_cli_cancellable(
        work_dir,
        commit_oid,
        path,
        &tigrs_core::CancellationToken::none(),
    )
}

/// Cancellable fallback blame via `git blame --line-porcelain`.
pub fn compute_blame_via_cli_cancellable(
    work_dir: Option<&Path>,
    commit_oid: Option<ObjectId>,
    path: &str,
    cancel: &tigrs_core::CancellationToken,
) -> Result<BlameResult> {
    cancel.check_cancelled()?;
    let clean_path = crate::path_security::verify_relative_path(path)?;
    let mut cmd = if let Some(cwd) = work_dir {
        crate::path_security::safe_git_command(cwd)
    } else {
        crate::path_security::safe_git_command(Path::new("."))
    };
    // `--porcelain` repeats a commit's header block only once, unlike
    // `--line-porcelain` which repeats it for every annotated line. On a file
    // with many lines per commit that is an order of magnitude less output to
    // pipe and parse; `parse_line_porcelain_blame` understands both.
    cmd.args(["blame", "--no-textconv", "--porcelain"]);
    if let Some(cid) = commit_oid {
        cmd.arg(cid.to_hex().to_string());
    }
    cmd.arg("--").arg(&clean_path);

    let output = crate::status::run_command_cancellable(&mut cmd, cancel)?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(TigError::Git(format!("git blame failed: {err}")));
    }

    parse_line_porcelain_blame(
        &output.stdout,
        commit_oid.unwrap_or_else(|| ObjectId::null(gix::hash::Kind::Sha1)),
        &clean_path,
    )
}

/// Commit-scoped metadata from a `git blame` porcelain header block.
///
/// Held behind [`Arc`] handles so each blamed line attributed to the commit
/// shares one allocation per field instead of reallocating per line.
#[derive(Clone)]
struct PorcelainCommitMeta {
    short_id: Arc<str>,
    author: Arc<str>,
    date: Arc<str>,
    summary: Arc<str>,
    parent_id: Option<ObjectId>,
    source_path: Option<String>,
}

impl PorcelainCommitMeta {
    fn new(commit_id: ObjectId) -> Self {
        let hex = commit_id.to_hex().to_string();
        Self {
            short_id: Arc::from(&hex[..SHORT_COMMIT_ID_LEN.min(hex.len())]),
            author: Arc::from(""),
            date: Arc::from(""),
            summary: Arc::from(""),
            parent_id: None,
            source_path: None,
        }
    }
}

/// Recognizes a porcelain group header and returns its commit, source line, and whether it starts a new hunk group.
///
/// The format is `<sha> <source-lno> <result-lno> [<num-lines>]`. `git blame`
/// emits the four-field form for the first line of a group and a three-field
/// form for every subsequent line in that group, so both must be accepted.
fn parse_porcelain_group_header(line: &str) -> Option<(ObjectId, usize, bool)> {
    let mut parts = line.split_whitespace();
    let hex = parts.next()?;
    let hex = hex.strip_prefix('^').unwrap_or(hex);
    let commit_id = ObjectId::from_hex(hex.as_bytes()).ok()?;
    let source_line = parts.next()?.parse::<usize>().ok()?;
    // Reject anything without the result line number; `previous <sha> <path>`
    // is already handled above, but this keeps the discriminator strict.
    parts.next()?.parse::<usize>().ok()?;
    let is_group_start = parts.next().and_then(|n| n.parse::<usize>().ok()).is_some();
    Some((commit_id, source_line, is_group_start))
}

/// Parses the output of `git blame --porcelain` or `git blame --line-porcelain`.
///
/// Both formats are accepted. `--porcelain` emits a commit's header block only
/// the first time that commit is seen, so metadata is accumulated per commit
/// rather than in running scalars; otherwise a later group belonging to an
/// earlier commit silently inherits the preceding commit's author, date,
/// summary, parent, and source path. `--line-porcelain` repeats every block,
/// which simply overwrites the same entry.
pub fn parse_line_porcelain_blame(
    raw_output: &[u8],
    fallback_commit_id: ObjectId,
    path: &str,
) -> Result<BlameResult> {
    // Git blame porcelain headers never contain NUL bytes; any NUL byte anywhere
    // in the stream indicates binary file content.
    if memchr::memchr(0, raw_output).is_some() {
        return Ok(BlameResult {
            commit_id: fallback_commit_id,
            path: path.to_string(),
            is_binary: true,
            lines: Vec::new(),
        });
    }

    let text = String::from_utf8_lossy(raw_output);
    let mut lines: Vec<BlameLine> = Vec::new();
    let mut commits: HashMap<ObjectId, PorcelainCommitMeta> = HashMap::new();

    let mut current_id = fallback_commit_id;
    let mut current_source_line = 1usize;
    let mut last_commit_id: Option<ObjectId> = None;
    let mut current_is_group_start = false;
    let mut current_parent_id: Option<ObjectId> = None;
    let mut current_source_path: Option<String> = None;

    for line in text.lines() {
        // Content lines are the only ones prefixed with a tab, and every one of
        // them is preceded by a group header naming its commit and source line.
        if let Some(content) = line.strip_prefix('\t') {
            let meta = commits
                .entry(current_id)
                .or_insert_with(|| PorcelainCommitMeta::new(current_id));

            let is_hunk_start = current_is_group_start || last_commit_id != Some(current_id);
            current_is_group_start = false;
            last_commit_id = Some(current_id);

            lines.push(BlameLine {
                line_number: lines.len() + 1,
                commit_id: current_id,
                short_commit_id: Arc::clone(&meta.short_id),
                author: Arc::clone(&meta.author),
                author_date: Arc::clone(&meta.date),
                summary: Arc::clone(&meta.summary),
                content: strip_control_chars(content).into_owned(),
                is_hunk_start,
                parent_commit_id: current_parent_id.or(meta.parent_id),
                source_path: current_source_path
                    .clone()
                    .or_else(|| meta.source_path.clone()),
                source_line_number: current_source_line,
            });
            continue;
        }

        if let Some((commit_id, source_line, is_group_start)) = parse_porcelain_group_header(line) {
            current_id = commit_id;
            current_source_line = source_line;
            if is_group_start {
                current_is_group_start = true;
            }
            let meta = commits
                .entry(current_id)
                .or_insert_with(|| PorcelainCommitMeta::new(current_id));
            current_parent_id = meta.parent_id;
            current_source_path.clone_from(&meta.source_path);
            continue;
        }

        // Remaining lines are header fields describing the commit/hunk named by the
        // most recent group header.
        let Some(meta) = commits.get_mut(&current_id) else {
            continue;
        };

        if let Some(author) = line.strip_prefix("author ") {
            meta.author = Arc::from(&*strip_control_chars(author.trim()));
        } else if let Some(time_str) = line.strip_prefix("author-time ") {
            if let Ok(secs) = time_str.trim().parse::<i64>() {
                meta.date = Arc::from(format_epoch_date(secs).as_str());
            }
        } else if let Some(summary) = line.strip_prefix("summary ") {
            meta.summary = Arc::from(&*strip_control_chars(summary.trim()));
        } else if let Some(prev) = line.strip_prefix("previous ") {
            // Format: previous <sha> <filename>
            let pid = prev
                .split_whitespace()
                .next()
                .and_then(|sha| ObjectId::from_hex(sha.as_bytes()).ok());
            meta.parent_id = pid;
            current_parent_id = pid;
        } else if let Some(fname) = line.strip_prefix("filename ") {
            let clean_fname = strip_control_chars(fname.trim()).into_owned();
            meta.source_path = Some(clean_fname.clone());
            current_source_path = Some(clean_fname);
        }
    }

    Ok(BlameResult {
        commit_id: fallback_commit_id,
        path: path.to_string(),
        is_binary: false,
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::process::Command;
    use tempfile::TempDir;

    fn create_test_repo_with_history() -> (TempDir, gix::Repository, ObjectId) {
        let dir = TempDir::new().unwrap();
        let path = dir.path();

        let run = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(path)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .status()
                .unwrap();
            assert!(status.success(), "Command failed: git {args:?}");
        };

        run(&["init"]);
        run(&["config", "user.name", "Blame Author"]);
        run(&["config", "user.email", "blame@example.com"]);

        // Commit 1: Add hello.txt
        let file_path = path.join("hello.txt");
        let mut f = File::create(&file_path).unwrap();
        writeln!(f, "Line 1 - Initial").unwrap();
        writeln!(f, "Line 2 - Initial").unwrap();
        drop(f);

        run(&["add", "hello.txt"]);
        run(&["commit", "-m", "Initial commit"]);

        // Commit 2: Modify Line 2, add Line 3
        let mut f = File::create(&file_path).unwrap();
        writeln!(f, "Line 1 - Initial").unwrap();
        writeln!(f, "Line 2 - Modified").unwrap();
        writeln!(f, "Line 3 - New").unwrap();
        drop(f);

        run(&["add", "hello.txt"]);
        run(&["commit", "-m", "Second commit"]);

        let output = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap();
        let head_hex = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let head_oid = ObjectId::from_hex(head_hex.as_bytes()).unwrap();

        let repo = gix::open(path).unwrap();
        (dir, repo, head_oid)
    }

    #[test]
    fn test_format_epoch_date() {
        assert_eq!(format_epoch_date(0), "1970-01-01");
        assert_eq!(format_epoch_date(1_789_286_400), "2026-09-13");
        assert_eq!(format_epoch_date(-500), "1970-01-01");
    }

    #[test]
    fn test_compute_blame_gix_nonexistent_file() {
        let (_dir, repo, head_oid) = create_test_repo_with_history();
        let err = compute_blame_gix(&repo, head_oid, "nonexistent.txt");
        assert!(err.is_err());
    }

    #[test]
    fn test_parse_line_porcelain_blame_empty() {
        let res =
            parse_line_porcelain_blame(b"", ObjectId::null(gix::hash::Kind::Sha1), "empty.txt")
                .unwrap();
        assert_eq!(res.lines.len(), 0);
        assert!(!res.is_binary);
    }

    #[test]
    fn test_compute_blame_gix_and_cli_parity() {
        let (_dir, repo, head_oid) = create_test_repo_with_history();

        let blame_res = compute_blame_gix(&repo, head_oid, "hello.txt").unwrap();
        assert_eq!(blame_res.lines.len(), 3);
        assert!(!blame_res.is_binary);

        assert_eq!(blame_res.lines[0].line_number, 1);
        assert_eq!(blame_res.lines[0].content, "Line 1 - Initial");
        assert_eq!(&*blame_res.lines[0].author, "Blame Author");
        assert_eq!(&*blame_res.lines[0].summary, "Initial commit");

        assert_eq!(blame_res.lines[1].line_number, 2);
        assert_eq!(blame_res.lines[1].content, "Line 2 - Modified");
        assert_eq!(&*blame_res.lines[1].summary, "Second commit");
        assert!(blame_res.lines[1].parent_commit_id.is_some());

        assert_eq!(blame_res.lines[2].line_number, 3);
        assert_eq!(blame_res.lines[2].content, "Line 3 - New");
        assert_eq!(&*blame_res.lines[2].summary, "Second commit");
    }

    #[test]
    fn test_parse_line_porcelain_blame() {
        let raw = b"456a973123456789abcdef0123456789abcdef01 1 1 1\nauthor Alice\nauthor-time 1789286400\nsummary My Test Commit\nfilename test.txt\n\tHello World\n";
        let res =
            parse_line_porcelain_blame(raw, ObjectId::null(gix::hash::Kind::Sha1), "test.txt")
                .unwrap();
        assert_eq!(res.lines.len(), 1);
        assert_eq!(&*res.lines[0].author, "Alice");
        assert_eq!(&*res.lines[0].author_date, "2026-09-13");
        assert_eq!(&*res.lines[0].summary, "My Test Commit");
        assert_eq!(res.lines[0].content, "Hello World");
    }

    #[test]
    fn test_compute_blame_via_cli() {
        let (dir, _repo, head_oid) = create_test_repo_with_history();
        let path = dir.path();

        let res = compute_blame_via_cli(Some(path), Some(head_oid), "hello.txt").unwrap();
        assert_eq!(res.lines.len(), 3);
        assert!(!res.is_binary);
        assert_eq!(res.lines[0].content, "Line 1 - Initial");

        // Without specifying commit_oid (blaming HEAD via CLI)
        let res_head = compute_blame_via_cli(Some(path), None, "hello.txt").unwrap();
        assert_eq!(res_head.lines.len(), 3);

        // Nonexistent file should error
        let err = compute_blame_via_cli(Some(path), Some(head_oid), "nonexistent.txt");
        assert!(err.is_err());
    }

    #[test]
    fn test_compute_blame_binary_and_empty_file() {
        let (dir, repo, _head_oid) = create_test_repo_with_history();
        let path = dir.path();

        // Create empty file and binary file, then commit them
        std::fs::write(path.join("empty.txt"), b"").unwrap();
        std::fs::write(path.join("binary.bin"), b"binary\0content\n").unwrap();

        let run = |args: &[&str]| {
            let status = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(path)
                .status()
                .unwrap();
            assert!(status.success());
        };
        run(&["add", "empty.txt", "binary.bin"]);
        run(&["commit", "-m", "Add empty and binary"]);

        let output = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap();
        let head_hex = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let new_head = ObjectId::from_hex(head_hex.as_bytes()).unwrap();

        // Blame empty file
        let empty_blame = compute_blame_gix(&repo, new_head, "empty.txt").unwrap();
        assert_eq!(empty_blame.lines.len(), 0);
        assert!(!empty_blame.is_binary);

        // Blame binary file
        let bin_blame = compute_blame_gix(&repo, new_head, "binary.bin").unwrap();
        assert_eq!(bin_blame.lines.len(), 0);
        assert!(bin_blame.is_binary);

        // Top level compute_blame helper
        let top_blame = compute_blame(&repo, Some(path), new_head, "hello.txt").unwrap();
        assert_eq!(top_blame.lines.len(), 3);
    }

    #[test]
    fn test_parse_line_porcelain_with_previous_and_binary() {
        let raw = b"456a973123456789abcdef0123456789abcdef01 1 1 1\n\
author Bob\n\
author-time 1789286400\n\
summary Commit With Parent\n\
previous 1111222233334444555566667777888899990000 prev_file.txt\n\
filename current_file.txt\n\
\tHello with previous\n";
        let res = parse_line_porcelain_blame(
            raw,
            ObjectId::null(gix::hash::Kind::Sha1),
            "current_file.txt",
        )
        .unwrap();
        assert_eq!(res.lines.len(), 1);
        assert!(res.lines[0].parent_commit_id.is_some());
        assert_eq!(
            res.lines[0].source_path.as_deref(),
            Some("current_file.txt")
        );

        // Binary content in raw porcelain
        let bin_raw = b"binary\0content";
        let res_bin =
            parse_line_porcelain_blame(bin_raw, ObjectId::null(gix::hash::Kind::Sha1), "bin.dat")
                .unwrap();
        assert!(res_bin.is_binary);
    }

    #[test]
    fn test_parse_porcelain_repeat_commit_keeps_own_metadata() {
        const FIRST: &str = "1111111111111111111111111111111111111111";
        const SECOND: &str = "2222222222222222222222222222222222222222";
        const PARENT: &str = "3333333333333333333333333333333333333333";

        // Abbreviated `git blame --porcelain` output: the header block is
        // emitted only the first time a commit appears, line 3 repeats FIRST
        // with a bare header, and line 4 is a 3-field continuation of line 3's
        // group.
        let raw = format!(
            "{FIRST} 1 1 1\n\
             author Alice\n\
             author-time 0\n\
             summary First commit\n\
             filename a.txt\n\
             \ta1\n\
             {SECOND} 2 2 1\n\
             author Bob\n\
             author-time 86400\n\
             summary Second commit\n\
             previous {PARENT} a.txt\n\
             filename a.txt\n\
             \tb2\n\
             {FIRST} 3 3 2\n\
             \ta3\n\
             {FIRST} 4 4\n\
             \ta4\n"
        );

        let res = parse_line_porcelain_blame(
            raw.as_bytes(),
            ObjectId::null(gix::hash::Kind::Sha1),
            "a.txt",
        )
        .unwrap();

        assert_eq!(res.lines.len(), 4);

        let first_id = ObjectId::from_hex(FIRST.as_bytes()).unwrap();
        let second_id = ObjectId::from_hex(SECOND.as_bytes()).unwrap();
        let parent_id = ObjectId::from_hex(PARENT.as_bytes()).unwrap();

        // Line 2 is the only line belonging to the second commit.
        assert_eq!(res.lines[1].commit_id, second_id);
        assert_eq!(&*res.lines[1].author, "Bob");
        assert_eq!(res.lines[1].parent_commit_id, Some(parent_id));

        // Lines 3 and 4 repeat the first commit with no header block. They must
        // recover its metadata rather than inherit the second commit's.
        for line in &res.lines[2..] {
            assert_eq!(line.commit_id, first_id);
            assert_eq!(
                &*line.author, "Alice",
                "repeat group inherited wrong author"
            );
            assert_eq!(&*line.summary, "First commit");
            assert_eq!(&*line.author_date, "1970-01-01");
            assert_eq!(line.source_path.as_deref(), Some("a.txt"));
            assert_eq!(
                line.parent_commit_id, None,
                "repeat group inherited the other commit's parent"
            );
        }

        // The 3-field continuation header still advances the source line.
        assert_eq!(res.lines[2].source_line_number, 3);
        assert_eq!(res.lines[3].source_line_number, 4);

        // Hunk starts follow commit transitions, not line positions.
        assert!(res.lines[0].is_hunk_start);
        assert!(res.lines[1].is_hunk_start);
        assert!(res.lines[2].is_hunk_start);
        assert!(!res.lines[3].is_hunk_start);
    }

    #[test]
    fn test_cli_blame_reports_correct_author_for_interleaved_commits() {
        let (dir, _repo, _head) = create_test_repo_with_history();
        let path = dir.path();

        // `interleaved.txt` ends up with line 1 and line 3 from the first
        // commit and line 2 from the second, so the first commit's group is
        // emitted twice in `--porcelain` output.
        std::fs::write(path.join("interleaved.txt"), "x1\nx2\nx3\n").unwrap();
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(path)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["add", "interleaved.txt"]);
        run(&[
            "-c",
            "user.email=a@e.com",
            "-c",
            "user.name=Author One",
            "commit",
            "-qm",
            "base",
        ]);

        std::fs::write(path.join("interleaved.txt"), "x1\nCHANGED\nx3\n").unwrap();
        run(&["add", "interleaved.txt"]);
        run(&[
            "-c",
            "user.email=b@e.com",
            "-c",
            "user.name=Author Two",
            "commit",
            "-qm",
            "edit",
        ]);

        let res = compute_blame_via_cli_cancellable(
            Some(path),
            None,
            "interleaved.txt",
            &tigrs_core::CancellationToken::none(),
        )
        .unwrap();

        assert_eq!(res.lines.len(), 3);
        assert_eq!(&*res.lines[0].author, "Author One");
        assert_eq!(&*res.lines[1].author, "Author Two");
        assert_eq!(
            &*res.lines[2].author, "Author One",
            "third line inherited the preceding group's author"
        );
        assert_eq!(res.lines[0].commit_id, res.lines[2].commit_id);
        assert_ne!(res.lines[1].commit_id, res.lines[0].commit_id);
    }

    #[test]
    fn test_parse_porcelain_group_header_boundary_commit_caret() {
        let oid_hex = "0123456789abcdef0123456789abcdef01234567";
        let header_normal = format!("{oid_hex} 1 1 5");
        let header_caret = format!("^{oid_hex} 1 1 5");

        let parsed_normal = parse_porcelain_group_header(&header_normal);
        let parsed_caret = parse_porcelain_group_header(&header_caret);

        assert!(parsed_normal.is_some());
        assert!(parsed_caret.is_some());
        assert_eq!(parsed_normal, parsed_caret);
    }

    #[test]
    fn test_blame_binary_and_empty_file_and_rename_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        let run = |args: &[&str]| {
            let status = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(path)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };

        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "alice@example.com"]);
        run(&["config", "user.name", "Alice Developer"]);

        std::fs::write(path.join("bin.dat"), b"\x00\x01\x02binary\x00").unwrap();
        std::fs::write(path.join("empty.txt"), b"").unwrap();
        std::fs::write(
            path.join("old_name.txt"),
            "unchanged_line_1\nunchanged_line_2\nunchanged_line_3\nunchanged_line_4\nline_5\n",
        )
        .unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "initial files"]);

        run(&["mv", "old_name.txt", "new_name.txt"]);
        std::fs::write(
            path.join("new_name.txt"),
            "unchanged_line_1\nunchanged_line_2\nunchanged_line_3\nunchanged_line_4\nline_5_modified\n",
        )
        .unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "rename and edit"]);

        let repo = gix::open(path).unwrap();
        let head = repo.head_id().unwrap().detach();

        let bin_blame = compute_blame(&repo, Some(path), head, "bin.dat").unwrap();
        assert!(bin_blame.is_binary);
        assert!(bin_blame.lines.is_empty());

        let empty_blame = compute_blame(&repo, Some(path), head, "empty.txt").unwrap();
        assert!(!empty_blame.is_binary);
        assert!(empty_blame.lines.is_empty());

        let renamed_blame = compute_blame(&repo, Some(path), head, "new_name.txt").unwrap();
        assert_eq!(renamed_blame.lines.len(), 5);
        assert_eq!(renamed_blame.lines[0].content, "unchanged_line_1");
        assert_eq!(renamed_blame.lines[4].content, "line_5_modified");
        assert!(renamed_blame.lines[4].parent_commit_id.is_some());

        let cli_renamed = compute_blame_via_cli(Some(path), Some(head), "new_name.txt").unwrap();
        assert_eq!(cli_renamed.lines.len(), 5);
        assert!(cli_renamed.lines[0].parent_commit_id.is_none());
        assert!(cli_renamed.lines[4].parent_commit_id.is_some());
        assert_eq!(
            cli_renamed.lines[0].source_path.as_deref(),
            Some("old_name.txt")
        );
    }
}
