// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Cancellation tokens and generation counters for asynchronous backpressure.
//!
//! Provides lock-free coordination between UI navigation events and background
//! worker threads (revwalk streaming, diff generation, grep search, blame).

use crate::error::{Result, TigError};

#[cfg(feature = "loom")]
use loom::sync::Arc;
#[cfg(feature = "loom")]
use loom::sync::atomic::{AtomicBool, AtomicU64};

#[cfg(not(feature = "loom"))]
use std::sync::Arc;
#[cfg(not(feature = "loom"))]
use std::sync::atomic::{AtomicBool, AtomicU64};

use std::sync::atomic::Ordering;

/// A thread-safe token used by worker loops to detect cancellation requests.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Creates a new cancellation token in the uncancelled state.
    pub fn new() -> (CancellationSource, Self) {
        let flag = Arc::new(AtomicBool::new(false));
        (
            CancellationSource {
                cancelled: Arc::clone(&flag),
            },
            Self { cancelled: flag },
        )
    }

    /// Creates a dummy cancellation token that is never cancelled.
    #[must_use]
    pub fn none() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns `true` if no external [`CancellationSource`] exists that could ever trigger this token.
    #[inline]
    #[must_use]
    pub fn is_never_cancelled(&self) -> bool {
        Arc::strong_count(&self.cancelled) == 1 && !self.is_cancelled()
    }

    /// Returns `true` if cancellation has been requested.
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Returns `Err(TigError::Cancelled)` if cancellation was requested, otherwise `Ok(())`.
    #[inline]
    pub fn check_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(TigError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Returns a reference to the underlying atomic boolean flag.
    #[inline]
    pub fn flag(&self) -> &Arc<AtomicBool> {
        &self.cancelled
    }

    /// Clones the underlying `Arc<AtomicBool>` for third-party cancellation integrations.
    #[inline]
    pub fn clone_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }
}

/// The controller handle capable of triggering cancellation.
#[derive(Debug, Clone)]
pub struct CancellationSource {
    cancelled: Arc<AtomicBool>,
}

impl CancellationSource {
    /// Triggers cancellation, notifying all clones of the associated [`CancellationToken`].
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Cancels existing tokens and resets this source with a fresh cancellation flag.
    pub fn reset(&mut self) -> CancellationToken {
        self.cancelled.store(true, Ordering::SeqCst);
        self.cancelled = Arc::new(AtomicBool::new(false));
        self.token()
    }

    /// Returns `true` if cancellation has been requested.
    #[inline]
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Returns a new child token linked to this cancellation source.
    pub fn token(&self) -> CancellationToken {
        CancellationToken {
            cancelled: Arc::clone(&self.cancelled),
        }
    }
}

/// Monotonically increasing generation counter for backpressure and supersedure.
///
/// When the user fast-scrolls or triggers rapid view reloads, worker threads
/// bound to older generations can detect that they have been superseded and abort
/// early before wasting CPU and memory.
#[derive(Debug, Clone)]
pub struct GenerationCounter {
    generation: Arc<AtomicU64>,
}

impl Default for GenerationCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl GenerationCounter {
    /// Creates a new generation counter initialized at generation 0.
    pub fn new() -> Self {
        Self {
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Advances to the next generation and returns the new value.
    pub fn bump(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Returns the current active generation value.
    #[inline]
    pub fn current(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Checks if a given generation is still current.
    #[inline]
    pub fn is_current(&self, generation: u64) -> bool {
        self.current() == generation
    }
}

#[cfg(all(test, not(feature = "loom")))]
mod tests {
    use super::*;

    #[test]
    fn test_cancellation() {
        let (mut source, token) = CancellationToken::new();
        assert!(!token.is_cancelled());
        assert!(token.check_cancelled().is_ok());

        source.cancel();
        assert!(token.is_cancelled());
        assert!(matches!(token.check_cancelled(), Err(TigError::Cancelled)));

        let new_token = source.reset();
        // Old token remains permanently cancelled
        assert!(token.is_cancelled());
        // New token is active
        assert!(!new_token.is_cancelled());
        assert!(new_token.check_cancelled().is_ok());
    }

    #[test]
    fn test_generation_counter() {
        let counter = GenerationCounter::new();
        assert_eq!(counter.current(), 0);
        assert!(counter.is_current(0));

        let gen1 = counter.bump();
        assert_eq!(gen1, 1);
        assert_eq!(counter.current(), 1);
        assert!(!counter.is_current(0));
        assert!(counter.is_current(1));

        let gen2 = counter.bump();
        assert_eq!(gen2, 2);
        assert!(!counter.is_current(1));
        assert!(counter.is_current(2));
    }

    #[test]
    fn test_cancellation_multithreaded_and_none() {
        let none_token = CancellationToken::none();
        assert!(!none_token.is_cancelled());
        assert!(none_token.check_cancelled().is_ok());

        let (source, token) = CancellationToken::new();
        let token_clone1 = token.clone();
        let token_clone2 = token.clone();

        let handle = std::thread::spawn(move || {
            while !token_clone1.is_cancelled() {
                std::hint::spin_loop();
            }
            token_clone1.check_cancelled()
        });

        // Ensure multiple cancel calls are idempotent
        source.cancel();
        source.cancel();

        assert!(token_clone2.is_cancelled());
        let res = handle.join().expect("thread should join");
        assert!(matches!(res, Err(TigError::Cancelled)));
    }

    #[test]
    fn test_generation_counter_concurrent_bumps() {
        use std::sync::Arc;
        let counter = Arc::new(GenerationCounter::default());
        let mut handles = Vec::new();

        for _ in 0..10 {
            let c = Arc::clone(&counter);
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    c.bump();
                }
            }));
        }

        for h in handles {
            h.join().expect("join");
        }

        assert_eq!(counter.current(), 1000);
    }

    #[test]
    fn test_cancellation_source_and_token_methods() {
        use std::sync::atomic::Ordering;
        let (source, token) = CancellationToken::new();
        assert!(!source.is_cancelled());
        assert!(!token.flag().load(Ordering::Relaxed));
        let flag_clone = token.clone_flag();
        assert!(!flag_clone.load(Ordering::Relaxed));

        let token2 = source.token();
        assert!(!token2.is_cancelled());

        source.cancel();
        assert!(source.is_cancelled());
        assert!(flag_clone.load(Ordering::Relaxed));
        assert!(token.is_cancelled());
        assert!(token2.is_cancelled());
    }
}
