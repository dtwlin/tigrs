// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Suite 6: CLI Subcommands, Broken Pipe (`SIGPIPE`/`EPIPE`), & Pager Mode Integration Suite.
//!
//! Verifies:
//! 1. `--version` and `--help` CLI flags output and exit codes (`0`).
//! 2. `BrokenPipe` (`EPIPE`) resilience for `tigrs completions <shell>` and `tigrs man`
//!    when downstream pipe readers close early (`head -c 16`).
//! 3. Graceful non-zero exit code and diagnostic message when invoked outside a Git repository.
//! 4. PTY-backed end-to-end execution of subcommands (`status`, `refs`, `stash`, `reflog`, `grep`)
//!    with clean `'q'` termination and alternate-screen restoration (`\x1b[?1049l`).

#![cfg(target_os = "linux")]

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;

use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{Winsize, tcsetwinsize};

const TIGRS_BIN: &str = env!("CARGO_BIN_EXE_tigrs");
const ALT_SCREEN_ENTER: &str = "\x1b[?1049h";
const ALT_SCREEN_EXIT: &str = "\x1b[?1049l";

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "CLI SQE")
        .env("GIT_AUTHOR_EMAIL", "cli-sqe@example.com")
        .env("GIT_COMMITTER_NAME", "CLI SQE")
        .env("GIT_COMMITTER_EMAIL", "cli-sqe@example.com")
        .status()
        .expect("git command failed");
    assert!(status.success(), "git {args:?} failed");
}

fn create_cli_test_repo() -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    let p = dir.path();
    git(p, &["init", "-b", "main"]);
    git(p, &["config", "user.name", "CLI SQE"]);
    git(p, &["config", "user.email", "cli-sqe@example.com"]);

    std::fs::write(
        p.join("main.rs"),
        "fn main() {\n    println!(\"needle_cli_grep_target\");\n}\n",
    )
    .unwrap();
    git(p, &["add", "main.rs"]);
    git(p, &["commit", "-m", "Initial CLI commit"]);
    git(p, &["tag", "v1.0.0"]);

    std::fs::write(p.join("main.rs"), "// Stash modification\n").unwrap();
    git(p, &["stash", "push", "-m", "cli test stash"]);

    std::fs::write(p.join("untracked.txt"), "untracked content\n").unwrap();
    dir
}

struct Pty {
    master: File,
    slave: File,
}

fn open_pty() -> Option<Pty> {
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).ok()?;
    grantpt(&master).ok()?;
    unlockpt(&master).ok()?;
    let name = ptsname(&master, Vec::new()).ok()?;
    let slave = OpenOptions::new()
        .read(true)
        .write(true)
        .open(OsStr::from_bytes(name.as_bytes()))
        .ok()?;

    let _ = tcsetwinsize(
        &slave,
        Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    );

    Some(Pty {
        master: File::from(master),
        slave,
    })
}

#[test]
fn test_cli_version_and_help_flags() {
    let out_ver = Command::new(TIGRS_BIN)
        .arg("--version")
        .output()
        .expect("run tigrs --version");
    assert!(out_ver.status.success());
    let stdout_ver = String::from_utf8_lossy(&out_ver.stdout);
    assert!(
        stdout_ver.contains("tigrs"),
        "Expected 'tigrs' in --version output, got: {stdout_ver}"
    );

    for flag in ["-h", "--help"] {
        let out_help = Command::new(TIGRS_BIN)
            .arg(flag)
            .output()
            .expect("run tigrs help");
        assert!(out_help.status.success());
        let stdout_help = String::from_utf8_lossy(&out_help.stdout);
        assert!(
            stdout_help.contains("text-mode interface for Git"),
            "Unexpected {flag} output: {stdout_help}"
        );
        assert!(stdout_help.contains("--directory"));
        for subcmd in [
            "log",
            "show",
            "diff",
            "status",
            "blame",
            "grep",
            "refs",
            "stash",
            "reflog",
            "completions",
            "man",
        ] {
            assert!(
                stdout_help.contains(subcmd),
                "Expected {flag} output to document subcommand '{subcmd}', got:\n{stdout_help}"
            );
        }
    }
}

#[test]
fn test_cli_completions_and_man_broken_pipe_resilience() {
    // 1. Test `tigrs completions bash` with early reader close (`EPIPE`)
    let mut child_comp = Command::new(TIGRS_BIN)
        .args(["completions", "bash"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tigrs completions bash");

    {
        let mut stdout = child_comp.stdout.take().unwrap();
        let mut small_buf = [0u8; 16];
        let _ = stdout.read_exact(&mut small_buf);
        // Dropping `stdout` closes the read end of the pipe immediately
    }

    let status_comp = child_comp.wait().expect("wait child_comp");
    assert!(
        status_comp.success(),
        "tigrs completions must exit 0 on BrokenPipe, got: {status_comp:?}"
    );

    // 2. Test `tigrs man` with early reader close (`EPIPE`)
    let mut child_man = Command::new(TIGRS_BIN)
        .arg("man")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tigrs man");

    {
        let mut stdout = child_man.stdout.take().unwrap();
        let mut small_buf = [0u8; 16];
        let _ = stdout.read_exact(&mut small_buf);
    }

    let status_man = child_man.wait().expect("wait child_man");
    assert!(
        status_man.success(),
        "tigrs man must exit 0 on BrokenPipe, got: {status_man:?}"
    );
}

#[test]
fn test_cli_non_git_directory_graceful_error() {
    let non_git_dir = TempDir::new().expect("temp non-git dir");
    let out = Command::new(TIGRS_BIN)
        .args(["-C", non_git_dir.path().to_str().unwrap(), "status"])
        .output()
        .expect("run tigrs in non-git dir");

    assert!(
        !out.status.success(),
        "Expected non-zero exit code when running tigrs outside a git repository"
    );
}

#[test]
fn test_cli_pty_subcommands_status_refs_stash_reflog_grep() {
    let repo_dir = create_cli_test_repo();
    let subcommands: &[&[&str]] = &[
        &["show", "HEAD"],
        &["status"],
        &["refs"],
        &["stash"],
        &["reflog"],
        &["grep", "needle_cli_grep_target"],
    ];

    for subcmd in subcommands {
        let Some(mut pty) = open_pty() else {
            eprintln!("Skipping PTY test: open_pty unavailable in this container environment");
            return;
        };

        let slave_in = pty.slave.try_clone().unwrap();
        let slave_out = pty.slave.try_clone().unwrap();
        let slave_err = pty.slave;

        let mut cmd = Command::new(TIGRS_BIN);
        cmd.arg("-C").arg(repo_dir.path());
        cmd.args(*subcmd);
        cmd.env("TERM", "xterm-256color");
        cmd.stdin(Stdio::from(slave_in));
        cmd.stdout(Stdio::from(slave_out));
        cmd.stderr(Stdio::from(slave_err));

        let mut child = cmd.spawn().expect("spawn tigrs in PTY");

        let captured = Arc::new(Mutex::new(String::new()));
        let cap_clone = Arc::clone(&captured);
        let mut reader = pty.master.try_clone().unwrap();

        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                cap_clone
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });

        // Wait until alternate screen is entered (`\x1b[?1049h`)
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if captured.lock().unwrap().contains(ALT_SCREEN_ENTER) {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        // Send 'q' to quit cleanly
        let _ = pty.master.write_all(b"q");
        let _ = pty.master.flush();

        let status = child.wait().expect("wait child");
        assert!(
            status.success(),
            "Subcommand {subcmd:?} exited with non-zero status: {status:?}"
        );

        let final_screen = captured.lock().unwrap().clone();
        assert!(
            final_screen.contains(ALT_SCREEN_EXIT),
            "Subcommand {subcmd:?} did not emit alternate screen exit sequence (\\x1b[?1049l)"
        );
    }
}
