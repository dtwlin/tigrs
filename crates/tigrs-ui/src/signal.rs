// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Asynchronous signal coordination for window resize and process lifecycle.
//!
//! Enforces POSIX signal handling with zero unsafe code by leveraging
//! safe signal iteration over self-pipe/socketpair IPC.

use crossbeam_channel::{Receiver, Sender, bounded};
use signal_hook::consts::{SIGCONT, SIGHUP, SIGINT, SIGTERM, SIGTSTP, SIGWINCH};
use signal_hook::iterator::{Handle, Signals};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread::JoinHandle;
use tigrs_core::error::{Result, TigError};

/// Process-wide record of the fatal signal (`SIGINT`, `SIGTERM`, `SIGHUP`) that caused
/// the interactive TUI loop to exit, allowing `main` to return `128 + signo`.
static LAST_EXIT_SIGNAL: AtomicI32 = AtomicI32::new(0);

/// Records the signal number that triggered application termination.
pub fn record_exit_signal(signo: i32) {
    LAST_EXIT_SIGNAL.store(signo, Ordering::SeqCst);
}

/// Takes and clears the recorded termination signal number, if any.
pub fn take_last_exit_signal() -> Option<i32> {
    let sig = LAST_EXIT_SIGNAL.swap(0, Ordering::SeqCst);
    (sig != 0).then_some(sig)
}

/// Signal events dispatched to the UI event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiSignal {
    /// Terminal window was resized (`SIGWINCH`).
    Resize,
    /// Process suspend requested (`SIGTSTP`).
    Suspend,
    /// Process execution resumed (`SIGCONT`).
    Resume,
    /// Immediate termination requested (`SIGINT`, `SIGTERM`, `SIGHUP`).
    Quit,
}

/// Coordinates OS signals and dispatches them across a thread-safe channel.
pub struct SignalCoordinator {
    sender: Sender<UiSignal>,
    receiver: Receiver<UiSignal>,
    handle: Handle,
    thread: Option<JoinHandle<()>>,
    /// Sticky signal number for non-interactive termination (`SIGTERM` or `SIGHUP`),
    /// preserved across [`Self::drain`] after child handover.
    terminated_signo: Arc<AtomicI32>,
    /// Most recent termination signal number (`SIGINT`, `SIGTERM`, or `SIGHUP`).
    last_quit_signo: Arc<AtomicI32>,
}

impl SignalCoordinator {
    /// Starts the signal coordinator thread listening for POSIX lifecycle signals.
    pub fn new() -> Result<Self> {
        let mut signals = Signals::new([SIGWINCH, SIGTSTP, SIGCONT, SIGHUP, SIGINT, SIGTERM])
            .map_err(|err| {
                TigError::Terminal(format!("Failed to register signal handlers: {err}"))
            })?;

        let handle = signals.handle();
        let (sender, receiver) = bounded::<UiSignal>(64);
        let thread_sender = sender.clone();
        let terminated_signo = Arc::new(AtomicI32::new(0));
        let last_quit_signo = Arc::new(AtomicI32::new(0));
        let term_clone = Arc::clone(&terminated_signo);
        let quit_clone = Arc::clone(&last_quit_signo);

        let thread = std::thread::Builder::new()
            .name("tigrs-sig".to_string())
            .spawn(move || {
                for signal in &mut signals {
                    let ui_sig = match signal {
                        SIGWINCH => UiSignal::Resize,
                        SIGTSTP => UiSignal::Suspend,
                        SIGCONT => UiSignal::Resume,
                        SIGHUP | SIGTERM => {
                            term_clone.store(signal, Ordering::SeqCst);
                            quit_clone.store(signal, Ordering::SeqCst);
                            UiSignal::Quit
                        }
                        SIGINT => {
                            let _ = quit_clone.compare_exchange(
                                0,
                                signal,
                                Ordering::SeqCst,
                                Ordering::SeqCst,
                            );
                            UiSignal::Quit
                        }
                        _ => continue,
                    };

                    // Non-blocking try_send: if buffer is full, coalescing drops older duplicates.
                    // Never `break` on `UiSignal::Quit`: during an interactive child handover
                    // (`$EDITOR`, `:!cmd`), Ctrl-C sends SIGINT to the shared foreground process
                    // group; breaking here would disconnect `receiver` after `drain()` and cause
                    // `select!` in the main loop to spin at 100% CPU.
                    let _ = thread_sender.try_send(ui_sig);
                }
            })
            .map_err(|err| TigError::Terminal(format!("Failed to spawn signal thread: {err}")))?;

        Ok(Self {
            sender,
            receiver,
            handle,
            thread: Some(thread),
            terminated_signo,
            last_quit_signo,
        })
    }

    /// Returns the receiver channel for listening to signal events.
    pub fn receiver(&self) -> &Receiver<UiSignal> {
        &self.receiver
    }

    /// Returns the signal number (`SIGINT`, `SIGTERM`, `SIGHUP`) that triggered `UiSignal::Quit`.
    pub fn exit_signal(&self) -> Option<i32> {
        let sig = self.last_quit_signo.load(Ordering::SeqCst);
        (sig != 0).then_some(sig)
    }

    /// Discards interactive signal events (`SIGINT`, `SIGTSTP`, `SIGWINCH`) queued during handover,
    /// while preserving genuine process termination signals (`SIGTERM`, `SIGHUP`).
    pub fn drain(&self) {
        while self.receiver.try_recv().is_ok() {}
        let term_sig = self.terminated_signo.load(Ordering::SeqCst);
        if term_sig != 0 {
            self.last_quit_signo.store(term_sig, Ordering::SeqCst);
            let _ = self.sender.try_send(UiSignal::Quit);
        } else {
            self.last_quit_signo.store(0, Ordering::SeqCst);
        }
    }
}

impl Drop for SignalCoordinator {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_signal_variants() {
        assert_eq!(UiSignal::Resize, UiSignal::Resize);
        assert_ne!(UiSignal::Resize, UiSignal::Quit);
        assert_eq!(UiSignal::Suspend, UiSignal::Suspend);
        assert_eq!(UiSignal::Resume, UiSignal::Resume);
        let cloned = UiSignal::Quit;
        assert_eq!(cloned, UiSignal::Quit);

        // Debug format representation
        assert_eq!(format!("{:?}", UiSignal::Resize), "Resize");
        assert_eq!(format!("{:?}", UiSignal::Suspend), "Suspend");
        assert_eq!(format!("{:?}", UiSignal::Resume), "Resume");
        assert_eq!(format!("{:?}", UiSignal::Quit), "Quit");
    }

    #[test]
    fn test_signal_coordinator_lifecycle() {
        let coordinator = SignalCoordinator::new().expect("create signal coordinator");
        assert!(coordinator.receiver().is_empty());
        // Dropping must close handle and join worker thread cleanly
        drop(coordinator);
    }

    #[test]
    fn test_sequential_signal_coordinators() {
        // Verify multiple coordinator lifecycles in sequence cleanly teardown and re-register
        for _ in 0..3 {
            let coordinator = SignalCoordinator::new().expect("sequential signal coordinator");
            assert!(coordinator.receiver().is_empty());
            drop(coordinator);
        }
    }

    #[test]
    fn test_drain_discards_queued_signals() {
        let coordinator = SignalCoordinator::new().expect("create signal coordinator");

        // Stand in for a Ctrl-C the user aimed at an editor running in tigrs'
        // process group: the signal reaches tigrs too and must not survive the
        // handover as a queued quit request.
        rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::WINCH)
            .expect("raise SIGWINCH");

        let received = coordinator
            .receiver()
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("signal delivered");
        assert_eq!(received, UiSignal::Resize);

        rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::WINCH)
            .expect("raise SIGWINCH again");
        // Give the signal thread a moment to enqueue before draining.
        std::thread::sleep(std::time::Duration::from_millis(50));

        coordinator.drain();
        assert!(coordinator.receiver().is_empty());

        // Test SIGCONT delivery
        rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::CONT)
            .expect("raise SIGCONT");
        let received = coordinator
            .receiver()
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("signal delivered");
        assert_eq!(received, UiSignal::Resume);
    }
}
