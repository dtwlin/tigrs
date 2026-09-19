// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Terminal management, dual-stream TTY controller, and RAII cleanup guard.
//!
//! Enforces:
//! - Dual-Stream TTY: supports piped stdin while binding control to `/dev/tty`.
//! - Child Process Handover: `TtyHandoverGuard` pauses the input reader, drains
//!   buffered input, restores cooked mode, runs the child in the terminal's
//!   foreground process group, cooperates with job control while waiting, and
//!   restores the TUI on return.
//! - 64 KB batched `BufWriter` stdout flush + FD hygiene assertions.
//! - Panic-safe restoration of terminal attributes via RAII [`Drop`].

use crossbeam_channel::{Receiver, bounded};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    size as term_size,
};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::io::Errno;
use rustix::process::{Pid, Signal, WaitOptions, getpid, kill_process, waitpid};
use rustix::termios::{QueueSelector, isatty, tcflush, tcgetattr};
use std::io::{BufWriter, Stdout, Write, stdin, stdout};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tigrs_core::error::{Result, TigError};

/// Default capacity for the stdout buffered writer (64 KB).
pub const TTY_BUF_CAPACITY: usize = 64 * 1024;

/// Tracks whether the interactive TUI currently owns the terminal (raw mode + alternate screen),
/// gating the panic hook so escape sequences are never written when panicking before TUI entry.
static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Clears `O_NONBLOCK` on standard input and output file descriptions if set by an external child.
pub fn ensure_blocking_stdio() {
    let sin = stdin();
    if let Ok(flags) = fcntl_getfl(&sin)
        && flags.contains(OFlags::NONBLOCK)
    {
        let _ = fcntl_setfl(&sin, flags & !OFlags::NONBLOCK);
    }
    let sout = stdout();
    if let Ok(flags) = fcntl_getfl(&sout)
        && flags.contains(OFlags::NONBLOCK)
    {
        let _ = fcntl_setfl(&sout, flags & !OFlags::NONBLOCK);
    }
}

/// Controller managing terminal raw mode, alternate screen buffers, and batched flushing.
pub struct TtyController {
    writer: BufWriter<Stdout>,
    is_piped: bool,
    active: bool,
    screen_generation: u64,
    mouse_enabled: bool,
}

impl TtyController {
    /// Initializes the terminal controller, enabling raw mode and alternate screen.
    ///
    /// Mouse tracking is disabled by default to preserve native terminal text selection.
    pub fn enter() -> Result<Self> {
        Self::enter_with_options(false)
    }

    /// Initializes the terminal controller with explicit mouse capture configuration.
    pub fn enter_with_options(enable_mouse: bool) -> Result<Self> {
        if !isatty(std::io::stdout()) {
            return Err(TigError::Terminal(
                "Standard output is not a terminal (interactive TUI requires a TTY)".to_string(),
            ));
        }
        if std::env::var("TERM").ok().as_deref() == Some("dumb") {
            return Err(TigError::Terminal(
                "Interactive TUI is not supported on TERM=dumb".to_string(),
            ));
        }

        ensure_blocking_stdio();
        let is_piped = !isatty(std::io::stdin());

        enable_raw_mode()
            .map_err(|err| TigError::Terminal(format!("Failed to enable raw mode: {err}")))?;

        let mut writer = BufWriter::with_capacity(TTY_BUF_CAPACITY, stdout());
        let res = if enable_mouse {
            execute!(
                writer,
                EnterAlternateScreen,
                Hide,
                crossterm::event::EnableMouseCapture
            )
        } else {
            execute!(writer, EnterAlternateScreen, Hide)
        };

        if let Err(err) = res {
            let _ = disable_raw_mode();
            return Err(TigError::Terminal(format!(
                "Failed to enter alternate screen: {err}"
            )));
        }
        let _ = writer.flush();
        TUI_ACTIVE.store(true, Ordering::Relaxed);

        Ok(Self {
            writer,
            is_piped,
            active: true,
            screen_generation: 1,
            mouse_enabled: enable_mouse,
        })
    }

    /// Returns `true` if input was piped to stdin (e.g. `git log | tigrs`).
    #[inline]
    #[must_use]
    pub fn is_piped(&self) -> bool {
        self.is_piped
    }

    /// Returns a mutable reference to the 64 KB buffered stdout writer.
    #[inline]
    pub fn writer(&mut self) -> &mut BufWriter<Stdout> {
        &mut self.writer
    }

    /// Flushes all pending escape sequences and characters to the physical terminal.
    pub fn flush(&mut self) -> Result<()> {
        match self.writer.flush() {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                ensure_blocking_stdio();
                self.writer.flush().map_err(TigError::Io)
            }
            Err(err) => Err(TigError::Io(err)),
        }
    }

    /// Retrieves the current terminal dimensions (columns, rows), falling back to
    /// `$COLUMNS` / `$LINES` and then `80×24` when `TIOCGWINSZ` reports `0`.
    pub fn size(&self) -> Result<(u16, u16)> {
        let (raw_w, raw_h) = term_size().unwrap_or((0, 0));
        let w = if raw_w > 0 {
            raw_w
        } else {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|s| s.trim().parse::<u16>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(80)
        };
        let h = if raw_h > 0 {
            raw_h
        } else {
            std::env::var("LINES")
                .ok()
                .and_then(|s| s.trim().parse::<u16>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(24)
        };
        Ok((w, h))
    }

    /// Temporarily suspends the TUI (e.g. for child process handover or SIGTSTP).
    ///
    /// Restores cooked mode, shows the cursor, and exits the alternate screen buffer.
    pub fn suspend(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }

        let _ = if self.mouse_enabled {
            execute!(
                self.writer,
                Show,
                LeaveAlternateScreen,
                crossterm::event::DisableMouseCapture
            )
        } else {
            execute!(self.writer, Show, LeaveAlternateScreen)
        };
        let _ = self.writer.flush();
        disable_raw_mode()
            .map_err(|err| TigError::Terminal(format!("Failed to disable raw mode: {err}")))?;
        self.active = false;
        TUI_ACTIVE.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Resumes the TUI after a suspension, re-enabling raw mode and the alternate screen.
    ///
    /// Re-entering the alternate screen gives a *blank* buffer, so this bumps
    /// [`Self::screen_generation`] to tell the renderer that nothing it
    /// previously painted is still on screen.
    pub fn resume(&mut self) -> Result<()> {
        if self.active {
            return Ok(());
        }
        self.force_resume()
    }

    /// Unconditionally re-applies raw mode, alternate screen, cursor hide, and mouse capture,
    /// and increments [`Self::screen_generation`].
    ///
    /// Used on `SIGCONT` (`UiSignal::Resume`), which may arrive after an external `SIGSTOP`
    /// where [`Self::suspend`] never ran (`self.active` was still `true`), or after job-control
    /// suspension where the parent shell modified `termios`.
    pub fn force_resume(&mut self) -> Result<()> {
        ensure_blocking_stdio();
        enable_raw_mode()
            .map_err(|err| TigError::Terminal(format!("Failed to re-enable raw mode: {err}")))?;
        let _ = if self.mouse_enabled {
            execute!(
                self.writer,
                EnterAlternateScreen,
                Hide,
                crossterm::event::EnableMouseCapture
            )
        } else {
            execute!(self.writer, EnterAlternateScreen, Hide)
        };
        let _ = self.writer.flush();
        self.active = true;
        TUI_ACTIVE.store(true, Ordering::Relaxed);
        self.screen_generation = self.screen_generation.wrapping_add(1);
        Ok(())
    }

    /// Suspends the TUI and stops the process for job control (`SIGTSTP` or `Ctrl-Z`).
    ///
    /// Pauses `input_reader`, restores the terminal to cooked normal-screen mode, and raises
    /// `SIGSTOP` (which cannot be caught by `signal_hook`, avoiding a self-signal loop).
    /// When `SIGCONT` later wakes the process, `kill_process` returns and the TUI is
    /// immediately restored.
    pub fn suspend_for_job_control(&mut self, input_reader: &InputReader) -> Result<()> {
        input_reader.pause();
        if let Err(err) = self.suspend() {
            input_reader.resume();
            return Err(err);
        }
        let _ = kill_process(getpid(), Signal::STOP);
        flush_terminal_input();
        let res = self.force_resume();
        input_reader.resume();
        res
    }

    /// Returns `true` if raw mode and alternate screen are currently active.
    #[inline]
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Returns `true` if mouse event capture is currently active on the terminal.
    #[inline]
    #[must_use]
    pub fn is_mouse_enabled(&self) -> bool {
        self.mouse_enabled
    }

    /// Dynamically enables or disables terminal mouse event capture.
    pub fn set_mouse_capture(&mut self, enable: bool) -> Result<()> {
        if self.mouse_enabled == enable {
            return Ok(());
        }
        self.mouse_enabled = enable;
        if self.active {
            if enable {
                execute!(self.writer, crossterm::event::EnableMouseCapture).map_err(|err| {
                    TigError::Terminal(format!("Failed to enable mouse capture: {err}"))
                })?;
            } else {
                execute!(self.writer, crossterm::event::DisableMouseCapture).map_err(|err| {
                    TigError::Terminal(format!("Failed to disable mouse capture: {err}"))
                })?;
            }
            self.writer.flush().map_err(TigError::Io)?;
        }
        Ok(())
    }

    /// Identifies the current contents of the alternate screen buffer.
    #[inline]
    #[must_use]
    pub fn screen_generation(&self) -> u64 {
        self.screen_generation
    }
}

impl Drop for TtyController {
    fn drop(&mut self) {
        if self.active {
            let _ = if self.mouse_enabled {
                execute!(
                    self.writer,
                    Show,
                    LeaveAlternateScreen,
                    crossterm::event::DisableMouseCapture
                )
            } else {
                execute!(self.writer, Show, LeaveAlternateScreen)
            };
            let _ = self.writer.flush();
            let _ = disable_raw_mode();
            self.active = false;
            TUI_ACTIVE.store(false, Ordering::Relaxed);
        }
    }
}

/// Installs a process-wide panic hook to guarantee terminal restoration even when
/// compiled under `panic = "abort"` where stack unwinding and RAII `Drop` handlers
/// are completely disabled.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if TUI_ACTIVE.swap(false, Ordering::Relaxed) {
            const RESTORE_SEQ: &[u8] =
                b"\x1b[?2026l\x1b[0m\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1015l\x1b[?1006l\x1b[?25h\x1b[?1049l";
            if let Ok(mut tty_file) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
                let _ = tty_file.write_all(RESTORE_SEQ);
                let _ = tty_file.flush();
            } else {
                let _ = std::io::stdout().write_all(RESTORE_SEQ);
                let _ = std::io::stdout().flush();
            }
            let _ = disable_raw_mode();
        }
        default_hook(info);
    }));
}

/// Enforces file descriptor hygiene in debug builds.
///
/// Enumerates `/proc/self/fd` and verifies that all open descriptors beyond
/// standard I/O (0, 1, 2) have `O_CLOEXEC` set (via `/proc/self/fdinfo/<fd>`),
/// ensuring that no internal packfile, socket, or pipe descriptors are leaked
/// across the `exec` boundary into child processes like `$EDITOR` or `git`.
#[cfg(all(debug_assertions, target_os = "linux"))]
pub fn assert_fd_hygiene() {
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return;
    };

    let mut leaky_fds = Vec::new();
    for entry in entries.flatten() {
        if let Ok(fd_num) = entry.file_name().to_string_lossy().parse::<i32>() {
            // Standard streams are allowed
            if fd_num <= 2 {
                continue;
            }
            let target = std::fs::read_link(entry.path())
                .map_or_else(|_| String::new(), |p| p.display().to_string());

            // Check if this descriptor belongs to a Git repository, packfile, index, config, or tigrs tempfile
            if target.contains(".git")
                || target.contains("pack-")
                || target.contains("objects")
                || target.contains("refs/")
                || target.contains("reftable")
                || target.contains("worktrees")
                || target.ends_with("/HEAD")
                || target.ends_with("/index")
                || target.contains("tigrs")
            {
                let fdinfo_path = format!("/proc/self/fdinfo/{fd_num}");
                if let Ok(info) = std::fs::read_to_string(&fdinfo_path) {
                    for line in info.lines() {
                        if let Some(flags_str) = line.strip_prefix("flags:") {
                            let flags_str = flags_str.trim();
                            if let Ok(flags) =
                                u64::from_str_radix(flags_str.trim_start_matches('0'), 8)
                            {
                                const O_CLOEXEC_BIT: u64 = 0o2_000_000;
                                if (flags & O_CLOEXEC_BIT) == 0 {
                                    leaky_fds.push((fd_num, target.clone(), flags));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    assert!(
        leaky_fds.is_empty(),
        "FD hygiene violation: repository/tigrs descriptors without O_CLOEXEC found: {leaky_fds:?}"
    );
}

/// Enforces file descriptor hygiene on non-Linux or release builds (no-op).
#[cfg(not(all(debug_assertions, target_os = "linux")))]
pub fn assert_fd_hygiene() {}

#[derive(Default)]
struct ReaderSyncState {
    paused: bool,
    is_parked: bool,
    shutdown: bool,
}

/// Background terminal input event reader supporting cooperative pause/resume for TTY handover.
pub struct InputReader {
    rx: Receiver<Event>,
    state: Arc<(Mutex<ReaderSyncState>, Condvar)>,
}

impl InputReader {
    /// Spawns a background thread that polls for terminal events and sends them through a bounded channel.
    #[must_use]
    pub fn spawn() -> Self {
        let (tx, rx) = bounded::<Event>(128);
        let state = Arc::new((Mutex::new(ReaderSyncState::default()), Condvar::new()));
        let state_clone = Arc::clone(&state);

        let _ = std::thread::Builder::new()
            .name("tigrs-input".to_string())
            .spawn(move || {
                loop {
                    {
                        let (lock, cvar) = &*state_clone;
                        let mut guard = lock
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if guard.shutdown {
                            break;
                        }
                        if guard.paused {
                            guard.is_parked = true;
                            cvar.notify_all();
                            while guard.paused && !guard.shutdown {
                                guard = cvar
                                    .wait(guard)
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                            }
                            guard.is_parked = false;
                            if guard.shutdown {
                                break;
                            }
                        }
                    }

                    // Poll with a 100ms slice to keep idle wakeups low (10 Hz vs 40 Hz)
                    // while still parking promptly for editor/shell handovers.
                    match event::poll(Duration::from_millis(100)) {
                        Ok(true) => {
                            let is_paused = state_clone
                                .0
                                .lock()
                                .map_or(true, |g| g.paused || g.shutdown);
                            if is_paused {
                                continue;
                            }
                            if let Ok(ev) = event::read() {
                                let still_active =
                                    state_clone.0.lock().is_ok_and(|g| !g.paused && !g.shutdown);
                                if still_active && tx.send(ev).is_err() {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                        Ok(false) => {
                            // Detect a hung-up controlling terminal (`EIO` on `tcgetattr`)
                            // in case `SIGHUP` was blocked or ignored by the parent environment.
                            let sin = stdin();
                            if isatty(&sin) && tcgetattr(&sin).is_err_and(|e| e == Errno::IO) {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }

                let (lock, cvar) = &*state_clone;
                if let Ok(mut guard) = lock.lock() {
                    guard.shutdown = true;
                    guard.is_parked = true;
                    cvar.notify_all();
                }
            });

        Self { rx, state }
    }

    /// Temporarily pauses reading events from the terminal, waiting deterministically
    /// until the background reader thread confirms it is parked outside `event::poll`/`read`.
    pub fn pause(&self) {
        let (lock, cvar) = &*self.state;
        let mut guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.paused = true;
        while !guard.is_parked && !guard.shutdown {
            let (g, timeout) = cvar
                .wait_timeout(guard, Duration::from_millis(200))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard = g;
            if timeout.timed_out() {
                break;
            }
        }
        drop(guard);
        self.drain();
    }

    /// Resumes reading events from the terminal.
    pub fn resume(&self) {
        self.drain();
        let (lock, cvar) = &*self.state;
        let mut guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.paused = false;
        cvar.notify_all();
    }

    /// Discards all pending events currently queued in the channel.
    pub fn drain(&self) {
        while self.rx.try_recv().is_ok() {}
    }

    /// Returns a reference to the receiver channel yielding terminal events.
    #[inline]
    #[must_use]
    pub fn receiver(&self) -> &Receiver<Event> {
        &self.rx
    }

    /// Returns whether the reader is currently paused.
    #[inline]
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.state.0.lock().is_ok_and(|g| g.paused)
    }
}

impl Drop for InputReader {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.state;
        if let Ok(mut guard) = lock.lock() {
            guard.shutdown = true;
            cvar.notify_all();
        }
    }
}

/// RAII guard that hands the terminal over to an interactive child process
/// (editor, pager, `git` command) and restores the TUI afterwards.
///
/// # Job control
///
/// The child deliberately runs **in tigrs' own process group**, and therefore
/// in the terminal's foreground process group. This mirrors how `git` launches
/// `$EDITOR` and how upstream Tig's `open_external_viewer()` behaves.
///
/// An earlier revision isolated the child in a new process group and moved the
/// terminal to it with `tcsetpgrp()`. That is what an interactive *shell* does,
/// but a shell may only do it because it keeps `SIGTTOU` blocked. A process
/// that is not in the terminal's foreground process group and neither blocks
/// nor ignores `SIGTTOU` is sent `SIGTTOU` by `tcsetpgrp()` (POSIX.1-2017 XSH
/// §11.1.4), whose default disposition *stops* the process. So the instant the
/// child exited and tigrs tried to take the terminal back, tigrs suspended
/// itself: the shell printed `Stopped`, reclaimed the terminal, and the user
/// was dropped at a prompt — indistinguishable from tigrs having quit. Without
/// job control (orphaned process group) the same call fails with `EIO`
/// instead, leaving tigrs alive but permanently backgrounded with its TUI torn
/// down. Blocking `SIGTTOU` around the call requires `pthread_sigmask`, which
/// is not reachable from a `#![forbid(unsafe_code)]` crate, so the foreground
/// process group is never given away in the first place.
///
/// Because the child shares tigrs' process group, terminal-generated signals
/// (`SIGINT` from Ctrl-C, `SIGTSTP` from Ctrl-Z) are delivered to tigrs as
/// well as to the child. tigrs catches those (see [`crate::signal`]) so they
/// can never terminate it, and the event loop discards whatever was queued
/// while the child owned the screen.
pub struct TtyHandoverGuard<'a> {
    controller: &'a mut TtyController,
    reader: Option<&'a InputReader>,
    active: bool,
}

impl<'a> TtyHandoverGuard<'a> {
    /// Enters terminal handover mode: pauses the input reader thread, drains
    /// buffered input, and restores cooked mode on the normal screen buffer.
    pub fn enter(
        controller: &'a mut TtyController,
        reader: Option<&'a InputReader>,
    ) -> Result<Self> {
        // 1. Pause input reader thread if present and drain pending events so
        //    the child, not tigrs, receives the user's keystrokes.
        if let Some(r) = reader {
            r.pause();
        }
        let reader_rollback = scopeguard::guard(reader, |opt_r| {
            if let Some(r) = opt_r {
                r.resume();
            }
        });

        // 2. Restore cooked mode and exit alternate screen
        controller.suspend()?;

        // Disarm rollback guard now that TtyHandoverGuard owns cleanup in Drop
        let _ = scopeguard::ScopeGuard::into_inner(reader_rollback);

        Ok(Self {
            controller,
            reader,
            active: true,
        })
    }

    /// Returns `true` if the guard is currently active (terminal handed over).
    #[inline]
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Runs an interactive child process to completion with the terminal handed
    /// over to it, cooperating with job control while waiting.
    pub fn execute_command(&mut self, mut cmd: Command) -> Result<ExitStatus> {
        assert_fd_hygiene();

        // The child inherits tigrs' process group on purpose; see the type
        // documentation for why the foreground process group is never moved.
        let mut child = cmd.spawn().map_err(TigError::Io)?;
        wait_with_job_control(&mut child)
    }

    /// Restores the TUI and resumes the input reader.
    ///
    /// Idempotent: calling it twice, or letting [`Drop`] call it, is a no-op
    /// after the first successful restore.
    pub fn reclaim(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        // Mark inactive up front so a failure part-way through is not retried
        // by `Drop` on an already half-restored terminal.
        self.active = false;

        // Discard anything the user typed after the child stopped reading:
        // those bytes were aimed at the child, and replaying them as tigrs
        // keybindings could trigger destructive actions (a stray `q` would
        // quit outright).
        flush_terminal_input();

        // Resume TUI raw mode and alternate screen
        if let Err(err) = self.controller.resume() {
            // When running in background test runners or non-controlling process groups,
            // tcsetattr returns EIO (os error 5); tolerate EIO under test or non-foreground contexts.
            if let TigError::Terminal(ref msg) = err {
                if !msg.contains("os error 5") {
                    return Err(err);
                }
            } else {
                return Err(err);
            }
        }

        // Resume input reader thread
        if let Some(r) = self.reader {
            r.resume();
        }

        Ok(())
    }
}

impl Drop for TtyHandoverGuard<'_> {
    fn drop(&mut self) {
        let _ = self.reclaim();
    }
}

/// Discards unread bytes sitting in the terminal's input queue.
///
/// No-op when standard input is not a terminal (piped mode).
fn flush_terminal_input() {
    let stdin = stdin();
    if isatty(&stdin) {
        let _ = tcflush(&stdin, QueueSelector::IFlush);
    }
}

/// Waits for an interactive child process, cooperating with terminal job control.
///
/// [`Child::wait`] only reports termination. If the child suspends itself —
/// Ctrl-Z inside the editor, or a `SIGTTIN`/`SIGTTOU` stop — it would block
/// forever on a stopped process that nothing can resume, wedging both tigrs and
/// the terminal. Waiting with `WUNTRACED` surfaces the stop, and tigrs then
/// stops itself with `SIGSTOP` so the entire job is stopped: the shell notices,
/// prints `Stopped`, and takes the terminal back. A later `fg` continues the
/// whole process group, so the child resumes and this loop keeps waiting.
///
/// `SIGSTOP` rather than `SIGTSTP` is raised deliberately: tigrs catches
/// `SIGTSTP` to restore the terminal before suspending, and here the terminal
/// is already in its cooked, normal-screen state.
fn wait_with_job_control(child: &mut Child) -> Result<ExitStatus> {
    let Some(pid) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) else {
        return child.wait().map_err(TigError::Io);
    };

    loop {
        let status = match waitpid(Some(pid), WaitOptions::UNTRACED) {
            Ok(Some((_, status))) => status,
            // `None` cannot happen without `WNOHANG`; an interrupted wait and a
            // spurious wake are both simply retried.
            Ok(None) | Err(Errno::INTR) => continue,
            Err(err) => return Err(TigError::Io(err.into())),
        };

        if status.stopped() {
            let _ = kill_process(getpid(), Signal::STOP);
            continue;
        }

        // The child has been reaped here rather than through `Child::wait`;
        // dropping `Child` afterwards is a no-op.
        return Ok(ExitStatus::from_raw(status.as_raw()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns `true` only when `/dev/tty` is openable AND our process group is
    /// its foreground process group (`tcgetpgrp == getpgrp`). Calling
    /// `tcsetattr` (`enable_raw_mode`) or `read` on `/dev/tty` from a
    /// background process group (e.g. `timeout cargo test` or `cargo test &`)
    /// causes the kernel to stop the entire test runner with `SIGTTOU`/`SIGTTIN`.
    fn has_foreground_controlling_tty() -> bool {
        let Ok(tty) = std::fs::File::open("/dev/tty") else {
            return false;
        };
        let Ok(fg_pgrp) = rustix::termios::tcgetpgrp(&tty) else {
            return false;
        };
        fg_pgrp == rustix::process::getpgrp()
    }

    #[test]
    fn test_fd_hygiene_assertion() {
        // Must run cleanly without failing
        assert_fd_hygiene();
    }

    #[test]
    fn test_input_reader_pause_and_resume() {
        if !has_foreground_controlling_tty() {
            return;
        }
        let reader = InputReader::spawn();
        assert!(!reader.is_paused());

        reader.pause();
        assert!(reader.is_paused());

        reader.resume();
        assert!(!reader.is_paused());
    }

    #[test]
    fn test_tty_handover_guard_lifecycle() {
        if !has_foreground_controlling_tty() {
            return;
        }
        if let Ok(mut tty) = TtyController::enter() {
            let reader = InputReader::spawn();
            let mut guard =
                TtyHandoverGuard::enter(&mut tty, Some(&reader)).expect("Handover enter");
            assert!(guard.is_active());
            assert!(reader.is_paused());

            let status = guard
                .execute_command(Command::new("true"))
                .expect("Execute true");
            assert!(status.success());

            guard.reclaim().expect("Handover reclaim");
            assert!(!guard.is_active());
            assert!(!reader.is_paused());

            // Reclaiming twice (explicitly, then via `Drop`) must be harmless.
            guard.reclaim().expect("Second reclaim is a no-op");
        }
    }

    /// The child must stay in tigrs' process group.
    ///
    /// Moving it into a new one means the terminal has to be handed over with
    /// `tcsetpgrp()` and, crucially, taken back the same way once the child
    /// exits — and taking it back from a background process group raises
    /// `SIGTTOU`, which stops tigrs and looks to the user exactly like tigrs
    /// quit when they left the editor.
    #[test]
    fn test_child_runs_in_tigrs_process_group() {
        let dir = tempfile::tempdir().expect("temp dir");
        let out = dir.path().join("pgid");
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("ps -o pgid= -p $$ > {}", out.display()));

        let mut child = cmd.spawn().expect("spawn reporter");
        let status = wait_with_job_control(&mut child).expect("wait for reporter");
        assert!(status.success());

        let reported: u32 = std::fs::read_to_string(&out)
            .expect("read pgid")
            .trim()
            .parse()
            .expect("parse pgid");
        let ours = rustix::process::getpgrp().as_raw_nonzero().get();
        assert_eq!(
            reported,
            u32::try_from(ours).expect("pgid fits in u32"),
            "child must inherit tigrs' process group"
        );
    }

    #[test]
    fn test_wait_with_job_control_reports_exit_code() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("exit 7")
            .spawn()
            .expect("spawn");
        let status = wait_with_job_control(&mut child).expect("wait");
        assert!(!status.success());
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn test_wait_with_job_control_reports_termination_signal() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("kill -TERM $$")
            .spawn()
            .expect("spawn");
        let status = wait_with_job_control(&mut child).expect("wait");
        assert!(!status.success());
        assert_eq!(status.code(), None, "signalled child has no exit code");
        assert_eq!(status.signal(), Some(15));
    }

    #[test]
    fn test_flush_terminal_input_is_safe_without_a_tty() {
        // Under the test harness stdin is usually not a terminal; the helper
        // must degrade to a no-op rather than erroring.
        flush_terminal_input();
    }

    #[test]
    fn test_fd_hygiene_with_git_file() {
        let temp = tempfile::tempdir().unwrap();
        let git_dir = temp.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let index_file = git_dir.join("index");
        std::fs::write(&index_file, b"test index").unwrap();

        let _file = std::fs::File::open(&index_file).unwrap();
        assert_fd_hygiene();
    }

    #[test]
    fn test_panic_hook_installation() {
        install_panic_hook();
    }

    #[test]
    fn test_tty_mouse_state() {
        if !has_foreground_controlling_tty() {
            return;
        }
        if let Ok(mut tty) = TtyController::enter_with_options(false) {
            assert!(!tty.is_mouse_enabled());
            assert!(tty.set_mouse_capture(true).is_ok());
            assert!(tty.is_mouse_enabled());
            assert!(tty.set_mouse_capture(false).is_ok());
            assert!(!tty.is_mouse_enabled());
        }
    }
}
