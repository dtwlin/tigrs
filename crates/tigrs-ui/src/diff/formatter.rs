// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! External diff-formatter pipeline.
//!
//! Pipes git-compatible unified patches through an external command
//! (such as `delta` or `diff-so-fancy`), enforcing timeouts, maximum output caps,
//! and ANSI SGR sanitization.

use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tigrs_core::ansi::filter_sgr_only;
use tigrs_core::error::{Result, TigError};
use tigrs_git::diff::CommitDiff;
use tigrs_git::patch::{IndexAbbrev, format_patch};

use crate::term_cap::{TerminalCapabilities, downgrade_ansi};

use super::document::{DiffDocument, DiffLineType, LineMarker, RowCell, RowPair};

/// Maximum execution time for an external diff formatter (5 seconds).
pub const FORMATTER_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum captured stdout buffer from an external diff formatter (32 MiB).
pub const MAX_FORMATTER_BYTES: usize = 32 * 1024 * 1024;

/// Runs an external diff formatter command on `diff`, returning a read-only `DiffDocument`.
pub fn run_external_formatter(
    diff: &CommitDiff,
    cmd_str: &str,
    columns: u16,
    caps: &TerminalCapabilities,
) -> Result<DiffDocument> {
    if cmd_str.trim().is_empty() {
        return Err(TigError::Command("Empty formatter command".to_string()));
    }

    let patch_text = format_patch(diff, IndexAbbrev::Chars(7));

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(cmd_str)
        .env("COLUMNS", columns.max(20).to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Ok(cwd) = std::env::current_dir() {
        tigrs_git::apply_untrusted_repo_env(&mut cmd, &cwd);
    }
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd
        .spawn()
        .map_err(|e| TigError::Command(format!("Failed to spawn formatter: {e}")))?;

    let mut stdin_pipe = child.stdin.take();
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| TigError::Command("Failed to capture stdout".to_string()))?;
    #[cfg(target_os = "linux")]
    let _ = rustix::pipe::fcntl_setpipe_size(&stdout, 1_048_576);

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| TigError::Command("Failed to capture stderr".to_string()))?;

    let kill_child = |child: &mut std::process::Child| {
        #[cfg(unix)]
        {
            let pid = child.id() as i32;
            if let Some(p) = rustix::process::Pid::from_raw(pid) {
                let _ = rustix::process::kill_process_group(p, rustix::process::Signal::KILL);
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    };

    let output_bytes = std::thread::scope(|s| -> Result<Vec<u8>> {
        if let Some(mut stdin) = stdin_pipe.take() {
            #[cfg(target_os = "linux")]
            let _ = rustix::pipe::fcntl_setpipe_size(&stdin, 1_048_576);
            s.spawn(move || {
                let _ = stdin.write_all(patch_text.as_bytes());
            });
        }

        let stderr_thread = s.spawn(move || {
            let mut captured = Vec::with_capacity(1024);
            let mut take_reader = stderr.take(4096);
            let _ = take_reader.read_to_end(&mut captured);
            let _ = std::io::copy(&mut take_reader.into_inner(), &mut std::io::sink());
            String::from_utf8_lossy(&captured).trim().to_string()
        });

        let (tx, rx) =
            crossbeam_channel::bounded::<std::result::Result<Vec<u8>, std::io::Error>>(16);
        s.spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(Ok(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
            }
        });

        let start = Instant::now();
        let mut collected = Vec::new();

        loop {
            let elapsed = start.elapsed();
            if elapsed >= FORMATTER_TIMEOUT {
                kill_child(&mut child);
                return Err(TigError::Command(
                    "Formatter timed out after 5 seconds".to_string(),
                ));
            }

            let remaining = FORMATTER_TIMEOUT.saturating_sub(elapsed);
            match rx.recv_timeout(remaining) {
                Ok(Ok(chunk)) => {
                    if collected.len() + chunk.len() > MAX_FORMATTER_BYTES {
                        kill_child(&mut child);
                        return Err(TigError::Command(
                            "Formatter output exceeded 32 MiB cap".to_string(),
                        ));
                    }
                    collected.extend_from_slice(&chunk);
                }
                Ok(Err(e)) => {
                    kill_child(&mut child);
                    return Err(TigError::Command(format!(
                        "Failed reading formatter stdout: {e}"
                    )));
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    kill_child(&mut child);
                    return Err(TigError::Command(
                        "Formatter timed out after 5 seconds".to_string(),
                    ));
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }

        let status = child
            .wait()
            .map_err(|e| TigError::Command(format!("Failed waiting for formatter: {e}")))?;

        let stderr_msg = stderr_thread.join().unwrap_or_default();

        if !status.success() {
            let detail = if stderr_msg.is_empty() {
                format!("Formatter exited with status {status}")
            } else {
                format!("Formatter exited with status {status}: {stderr_msg}")
            };
            return Err(TigError::Command(detail));
        }

        Ok(collected)
    })?;

    let text = String::from_utf8_lossy(&output_bytes);
    let mut rows = Vec::new();
    let file_indices = Vec::new();
    let hunk_indices = Vec::new();
    let mut line_to_file = Vec::new();
    let mut line_to_hunk = Vec::new();

    for line in text.lines() {
        let sanitized = filter_sgr_only(line);
        let downgraded = downgrade_ansi(&sanitized, caps.color_profile);
        let cell = RowCell::new(LineMarker::None, Arc::from(downgraded.as_ref()), None);

        // All lines from external formatter have anchor: None (read-only)
        rows.push(RowPair::unified(cell, None, DiffLineType::DiffHeader));
        line_to_file.push(None);
        line_to_hunk.push(None);
    }

    Ok(DiffDocument::new(
        rows,
        file_indices,
        hunk_indices,
        line_to_file,
        line_to_hunk,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigrs_git::ObjectId;

    fn sample_diff() -> CommitDiff {
        CommitDiff {
            commit_id: ObjectId::empty_tree(gix::hash::Kind::Sha1),
            parent_ids: vec![],
            author_name: Arc::from("Test"),
            author_email: Arc::from("test@example.com"),
            author_date: "Mon Sep 1 12:00:00 2026 +0000".to_string(),
            committer_name: Arc::from("Test"),
            committer_email: Arc::from("test@example.com"),
            committer_date: "Mon Sep 1 12:00:00 2026 +0000".to_string(),
            title: Arc::from("Test commit"),
            body: None,
            stats: tigrs_git::DiffSummaryStats::default(),
            files: Vec::new(),
        }
    }

    #[test]
    fn test_formatter_large_stderr_does_not_deadlock() {
        let diff = sample_diff();
        let caps = TerminalCapabilities::default();
        // Emit >128 KiB to stderr before writing stdout
        let cmd = "dd if=/dev/zero bs=1024 count=140 2>/dev/null | tr '\\0' 'E' >&2; echo 'formatted output'";
        let doc = run_external_formatter(&diff, cmd, 80, &caps)
            .expect("should not deadlock on >64KB stderr");
        assert_eq!(doc.rows.len(), 1);
    }

    #[test]
    fn test_formatter_error_includes_stderr_diagnostic() {
        let diff = sample_diff();
        let caps = TerminalCapabilities::default();
        let cmd = "echo 'fatal formatter error' >&2; exit 2";
        let err = run_external_formatter(&diff, cmd, 80, &caps).unwrap_err();
        assert!(
            err.to_string().contains("fatal formatter error"),
            "got: {err}"
        );
    }
}
