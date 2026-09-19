// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Pseudo-terminal (PTY) integration tests verifying CLI subcommand modes and terminal handling.

#![cfg(target_os = "linux")]

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{Winsize, tcsetwinsize};

const ALT_SCREEN: &str = "\x1b[?1049h";

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

struct Screen {
    buf: Arc<Mutex<String>>,
}

impl Screen {
    fn capture(mut master: File) -> Self {
        let buf = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&buf);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = master.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                if let Ok(mut sink) = sink.lock() {
                    sink.push_str(&String::from_utf8_lossy(&chunk[..n]));
                }
            }
        });
        Self { buf }
    }

    fn contents(&self) -> String {
        self.buf.lock().map(|b| b.clone()).unwrap_or_default()
    }

    fn wait_for(&self, marker: &str, count: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.contents().matches(marker).count() >= count {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

fn tool_exists(name: &str) -> bool {
    Command::new(name)
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn init_comprehensive_repo(dir: &std::path::Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "-q", "."]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test User"]);
    std::fs::write(
        dir.join("hello.txt"),
        "first line\nneedle: hello world\nthird line\n",
    )
    .expect("write file");
    std::fs::write(dir.join("other.txt"), "another file content\n").expect("write file");
    run(&["add", "."]);
    run(&["commit", "-qm", "initial commit"]);

    // Create a tag
    run(&["tag", "v1.0"]);

    // Create a second commit
    std::fs::write(
        dir.join("hello.txt"),
        "first line\nneedle: hello world updated\nthird line\n",
    )
    .expect("modify file");
    run(&["commit", "-a", "-qm", "second commit"]);

    // Create a branch
    run(&["branch", "feature"]);

    // Create a stash
    std::fs::write(dir.join("hello.txt"), "stashed modification\n").expect("modify for stash");
    run(&["stash", "push", "-qm", "wip stash"]);

    // Leave an unstaged modification so status view has rows
    std::fs::write(dir.join("hello.txt"), "unstaged change\n").expect("modify for status");
}

fn test_subcommand_in_pty(args: &[&str], repo_dir: &std::path::Path) {
    if !tool_exists("setsid") || !tool_exists("git") {
        return;
    }
    let Some(pty) = open_pty() else {
        return;
    };

    let mut cmd = Command::new("setsid");
    cmd.arg("--wait")
        .arg("--ctty")
        .arg(env!("CARGO_BIN_EXE_tigrs"))
        .args(args)
        .current_dir(repo_dir)
        .stdin(pty.slave.try_clone().expect("clone pts"))
        .stdout(pty.slave.try_clone().expect("clone pts"))
        .stderr(pty.slave.try_clone().expect("clone pts"))
        .env("TERM", "xterm-256color")
        .env_remove("TIG_EDITOR")
        .env_remove("EDITOR")
        .env_remove("GIT_EDITOR");

    let mut child = cmd.spawn().expect("spawn tigrs under setsid");
    drop(pty.slave);

    let mut master = pty.master.try_clone().expect("clone pty master");
    let screen = Screen::capture(pty.master);

    let quit = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };

    if !screen.wait_for(ALT_SCREEN, 1, Duration::from_secs(10)) {
        quit(&mut child);
        panic!(
            "tigrs {args:?} never painted first frame; output: {:?}",
            screen.contents()
        );
    }

    // Send 'q' to quit cleanly
    master.write_all(b"q").expect("send quit key");
    master.flush().expect("flush");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait().expect("poll child") {
            Some(status) => {
                assert!(
                    status.success(),
                    "tigrs {args:?} exited with failure status: {status}"
                );
                break;
            }
            None if Instant::now() >= deadline => {
                quit(&mut child);
                panic!(
                    "tigrs {args:?} did not exit within timeout; output: {:?}",
                    screen.contents()
                );
            }
            None => std::thread::sleep(Duration::from_millis(30)),
        }
    }
}

#[test]
fn test_pty_all_subcommand_views() {
    let dir = tempfile::tempdir().expect("temp dir");
    init_comprehensive_repo(dir.path());

    // 0. Default main view (no subcommand) and `+line` initial line jump
    test_subcommand_in_pty(&[], dir.path());
    test_subcommand_in_pty(&["+2"], dir.path());

    // 1. Default log mode
    test_subcommand_in_pty(&["log"], dir.path());

    // 2. Status view
    test_subcommand_in_pty(&["status"], dir.path());

    // 3. Show (diff) view
    test_subcommand_in_pty(&["show", "HEAD"], dir.path());

    // 4. Tree view
    test_subcommand_in_pty(&["tree", "HEAD"], dir.path());

    // 5. Blob view
    test_subcommand_in_pty(&["blob", "HEAD", "hello.txt"], dir.path());

    // 6. Blame view
    test_subcommand_in_pty(&["blame", "hello.txt"], dir.path());

    // 7. Refs view
    test_subcommand_in_pty(&["refs"], dir.path());

    // 8. Stash view
    test_subcommand_in_pty(&["stash"], dir.path());

    // 9. Reflog view
    test_subcommand_in_pty(&["reflog"], dir.path());

    // 10. Grep view
    test_subcommand_in_pty(&["grep", "needle"], dir.path());

    // 11. Debug frame stats flag with log mode
    test_subcommand_in_pty(&["--debug-frame-stats", "log"], dir.path());
}

#[test]
fn test_pty_piped_mode() {
    if !tool_exists("setsid") {
        return;
    }
    let Some(pty) = open_pty() else {
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");

    let mut child = Command::new("setsid")
        .arg("--wait")
        .arg("--ctty")
        .arg("sh")
        .arg("-c")
        .arg(format!(
            "echo 'test piped line' | '{}'",
            env!("CARGO_BIN_EXE_tigrs")
        ))
        .current_dir(dir.path())
        .stdin(pty.slave.try_clone().expect("clone pts"))
        .stdout(pty.slave.try_clone().expect("clone pts"))
        .stderr(pty.slave.try_clone().expect("clone pts"))
        .env("TERM", "xterm-256color")
        .spawn()
        .expect("spawn tigrs piped under setsid");

    drop(pty.slave);

    let mut master = pty.master.try_clone().expect("clone pty master");
    let screen = Screen::capture(pty.master);

    let quit = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };

    if !screen.wait_for(ALT_SCREEN, 1, Duration::from_secs(10)) {
        quit(&mut child);
        panic!(
            "tigrs in piped mode never painted first frame; output: {:?}",
            screen.contents()
        );
    }

    // Send 'q' to quit cleanly
    master.write_all(b"q").expect("send quit key");
    master.flush().expect("flush");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait().expect("poll child") {
            Some(status) => {
                assert!(
                    status.success(),
                    "tigrs in piped mode exited with failure status: {status}"
                );
                break;
            }
            None if Instant::now() >= deadline => {
                quit(&mut child);
                panic!(
                    "tigrs in piped mode did not exit within timeout; output: {:?}",
                    screen.contents()
                );
            }
            None => std::thread::sleep(Duration::from_millis(30)),
        }
    }
}
