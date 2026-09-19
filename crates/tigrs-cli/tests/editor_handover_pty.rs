// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! End-to-end terminal handover regression tests.
//!
//! These drive the real `tigrs` binary through a pseudo-terminal, in its own
//! session with a controlling terminal, because the bug they guard against only
//! exists under terminal job control:
//!
//! Handing the terminal to a child with `tcsetpgrp()` leaves tigrs in a
//! *background* process group. Taking it back from there raises `SIGTTOU`,
//! whose default disposition stops the process — so leaving the editor
//! suspended tigrs and dropped the user back at the shell prompt, exactly as if
//! tigrs had quit. Without job control (orphaned process group) the same call
//! fails with `EIO` and leaves tigrs backgrounded with its TUI torn down, and
//! its input reader then dies on `EIO` too, which really does exit the process.
//!
//! The fix keeps the child in tigrs' own (foreground) process group, so the
//! terminal never has to be reclaimed. The assertion below is deliberately
//! behavioural: after the editor exits, tigrs must repaint and still respond to
//! keys.
#![cfg(target_os = "linux")]

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{Winsize, tcsetwinsize};

/// Alternate screen buffer enable; tigrs emits it on entry and on every
/// restore after a child process handed the terminal back.
const ALT_SCREEN: &str = "\x1b[?1049h";
const EDITOR_MARKER: &str = "TIGRS-EDITOR-RAN";

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

    // A zero-sized window makes the renderer degenerate; give it a normal one.
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

/// Streams the terminal output of the child into a shared buffer.
struct Screen {
    buf: Arc<Mutex<String>>,
}

impl Screen {
    fn capture(mut master: File) -> Self {
        let buf = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&buf);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            // Reading the controlling side returns `EIO` once the child side is
            // gone; either way the loop simply ends.
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

    /// Waits until `marker` has been written at least `count` times.
    fn wait_for(&self, marker: &str, count: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.contents().matches(marker).count() >= count {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    /// Waits until `needle` appears in the output written *after* the last
    /// occurrence of `anchor`.
    fn wait_for_after(&self, anchor: &str, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let contents = self.contents();
            if let Some(idx) = contents.rfind(anchor)
                && contents[idx + anchor.len()..].contains(needle)
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
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

fn init_repo(dir: &std::path::Path) {
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
    run(&["config", "user.name", "Test"]);
    std::fs::write(dir.join("hello.txt"), "one\ntwo\nthree\n").expect("write file");
    run(&["add", "hello.txt"]);
    run(&["commit", "-qm", "initial"]);
    // Leave an unstaged modification so the status view has something to edit.
    std::fs::write(dir.join("hello.txt"), "one\ntwo\nthree\nfour\n").expect("modify file");
}

/// Writes a stand-in `$EDITOR` that announces itself and exits immediately.
fn write_fake_editor(path: &std::path::Path) {
    std::fs::write(path, format!("#!/bin/sh\nprintf '{EDITOR_MARKER}\\n'\n"))
        .expect("write editor");
    let mut perms = std::fs::metadata(path).expect("stat editor").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod editor");
}

#[test]
fn test_tigrs_survives_the_editor_exiting() {
    // `setsid -c` puts the binary in its own session with the pty as its
    // controlling terminal, which is what makes this a job-control test.
    if !tool_exists("setsid") || !tool_exists("git") {
        eprintln!("skipping: setsid or git unavailable");
        return;
    }
    let Some(pty) = open_pty() else {
        eprintln!("skipping: no pseudo-terminal available");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    init_repo(dir.path());
    let editor = dir.path().join("fake-editor.sh");
    write_fake_editor(&editor);

    let mut child = Command::new("setsid")
        .arg("--wait")
        .arg("--ctty")
        .arg(env!("CARGO_BIN_EXE_tigrs"))
        .arg("--read-only")
        .arg("status")
        .current_dir(dir.path())
        .stdin(pty.slave.try_clone().expect("clone pts"))
        .stdout(pty.slave.try_clone().expect("clone pts"))
        .stderr(pty.slave.try_clone().expect("clone pts"))
        .env("TERM", "xterm-256color")
        // Highest-precedence editor override, so a `core.editor` in the
        // developer's global git config cannot hijack the test.
        .env("TIG_EDITOR", &editor)
        .env("EDITOR", &editor)
        .env_remove("GIT_EDITOR")
        .env_remove("VISUAL")
        .spawn()
        .expect("spawn tigrs under setsid");

    // The parent holds no pts handle, so the reader sees EOF when tigrs exits.
    drop(pty.slave);
    let mut master = pty.master.try_clone().expect("clone pty master");
    let screen = Screen::capture(pty.master);

    let quit = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };

    assert!(
        screen.wait_for(ALT_SCREEN, 1, Duration::from_secs(20)),
        "tigrs never painted its first frame; output so far: {:?}",
        screen.contents()
    );

    // 1. In default Read-Only mode, pressing `e` must be blocked and display the status bar warning.
    master
        .write_all(b"e")
        .expect("send edit key in read-only mode");
    master.flush().expect("flush");
    if !screen.wait_for(
        "Read-only mode: repository modifications are disabled",
        1,
        Duration::from_secs(10),
    ) {
        quit(&mut child);
        panic!(
            "tigrs did not display read-only status bar warning on 'e'; output: {:?}",
            screen.contents()
        );
    }

    // 2. Unlock Update Mode via command prompt (`:set read-only = false`) and press `e` to launch $EDITOR.
    master
        .write_all(b":set read-only = false\re")
        .expect("unlock read-only mode and send edit key");
    master.flush().expect("flush");
    if !screen.wait_for(EDITOR_MARKER, 1, Duration::from_secs(20)) {
        quit(&mut child);
        panic!(
            "editor was never launched after unlocking read-only mode; output: {:?}",
            screen.contents()
        );
    }

    // The editor has exited. tigrs must take the screen back rather than being
    // stopped by SIGTTOU (or backgrounded with a dead input reader).
    if !screen.wait_for(ALT_SCREEN, 2, Duration::from_secs(20)) {
        quit(&mut child);
        panic!(
            "tigrs did not restore its screen after the editor exited; output: {:?}",
            screen.contents()
        );
    }

    // Re-entering the alternate screen yields a *blank* buffer, so the view has
    // to be painted again from scratch. Without invalidating the renderer's
    // damage-tracking cache the unchanged view diffs down to zero bytes and the
    // user is left staring at an empty terminal.
    if !screen.wait_for_after(EDITOR_MARKER, "hello.txt", Duration::from_secs(20)) {
        quit(&mut child);
        panic!(
            "tigrs restored the screen but never repainted its content; output: {:?}",
            screen.contents()
        );
    }

    // ...and it must still be reading the keyboard.
    master.write_all(b"q").expect("send quit key");
    master.flush().expect("flush");

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait().expect("poll child") {
            Some(status) => {
                assert!(
                    status.success(),
                    "tigrs exited unsuccessfully after the editor: {status}"
                );
                break;
            }
            None if Instant::now() >= deadline => {
                quit(&mut child);
                panic!(
                    "tigrs stopped responding to input after the editor exited; output: {:?}",
                    screen.contents()
                );
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}
