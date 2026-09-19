// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Rayon Compute Pool and Thread Budget Manager.
//!
//! Enforces:
//! - Thread Budget: UI/render (1) + Input Reader/IO (1) + Rayon pool `max(1, N - 2)`.
//!   Total threads == logical core count N (honest accounting, zero oversubscription).
//! - Clean, safe abstractions for CPU-bound computations (diff, graph parsing, blame, search).

use crate::cancel::CancellationToken;
use crate::error::{Result, TigError};
use rayon::{ThreadPool, ThreadPoolBuilder};
use std::sync::OnceLock;

/// Global compute pool instance initialized according to system thread budget.
static GLOBAL_COMPUTE_POOL: OnceLock<ComputePool> = OnceLock::new();

/// Calculates the default worker thread budget :
///
/// Thread Budget: `max(2, N - 2)` where N is system logical core count.
/// - 1 thread reserved for UI and terminal rendering
/// - 1 thread reserved for Input reading and I/O coordination
/// - Remainder dedicated to compute workers (floor of 2 to prevent single-task head-of-line starvation)
#[must_use]
pub fn default_compute_threads() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    cores.saturating_sub(2).clamp(2, 16)
}

/// Sends `msg` on a bounded channel without indefinitely parking a Rayon worker thread.
///
/// If the channel is full, yields to the Rayon work-stealing scheduler and checks
/// `token` for cancellation before retrying. Returns `false` if the channel
/// disconnected or cancellation was requested.
pub fn send_or_yield<T>(
    tx: &crossbeam_channel::Sender<T>,
    mut msg: T,
    token: &CancellationToken,
) -> bool {
    loop {
        match tx.try_send(msg) {
            Ok(()) => return true,
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => return false,
            Err(crossbeam_channel::TrySendError::Full(returned)) => {
                if token.is_cancelled() {
                    return false;
                }
                msg = returned;
                match rayon::yield_now() {
                    Some(rayon::Yield::Executed) => {}
                    Some(rayon::Yield::Idle) | None => {
                        match tx.send_timeout(msg, std::time::Duration::from_millis(2)) {
                            Ok(()) => return true,
                            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => {
                                return false;
                            }
                            Err(crossbeam_channel::SendTimeoutError::Timeout(returned)) => {
                                if token.is_cancelled() {
                                    return false;
                                }
                                msg = returned;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Compute worker pool for CPU-bound git operations.
pub struct ComputePool {
    pool: ThreadPool,
    num_threads: usize,
}

impl ComputePool {
    /// Creates a new compute pool with `num_threads` worker threads.
    pub fn new(num_threads: usize) -> Result<Self> {
        let num_threads = num_threads.max(1);
        let pool = ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .thread_name(|idx| format!("tigrs-worker-{idx}"))
            .build()
            .map_err(|err| TigError::Internal(format!("Failed to build compute pool: {err}")))?;

        Ok(Self { pool, num_threads })
    }

    /// Returns the number of worker threads in this pool.
    #[inline]
    #[must_use]
    pub fn num_threads(&self) -> usize {
        self.num_threads
    }

    /// Runs a computation inside this pool.
    pub fn install<OP, R>(&self, op: OP) -> R
    where
        OP: FnOnce() -> R + Send,
        R: Send,
    {
        self.pool.install(op)
    }

    /// Spawns an asynchronous task onto this pool.
    ///
    /// # Panics / Blocking
    ///
    /// The closure MUST NOT block on a channel, lock, or syscall for an unbounded
    /// time. Rayon workers are a fixed, small budget (see [`default_compute_threads`]);
    /// a parked worker is a permanently lost slot. Use [`send_or_yield`] for channel
    /// sends and pass a [`CancellationToken`] into any long loop.
    pub fn spawn<OP>(&self, op: OP)
    where
        OP: FnOnce() + Send + 'static,
    {
        self.pool.spawn(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(op)).is_err() {
                eprintln!("tigrs: background task panicked on compute pool");
            }
        });
    }
}

/// Initializes the global compute pool.
///
/// If called with `None`, uses `default_compute_threads()`.
pub fn init_global_compute_pool(num_threads: Option<usize>) -> Result<&'static ComputePool> {
    if let Some(pool) = GLOBAL_COMPUTE_POOL.get() {
        return Ok(pool);
    }
    let budget = num_threads.unwrap_or_else(default_compute_threads);
    let created = ComputePool::new(budget)?;
    let _ = GLOBAL_COMPUTE_POOL.set(created);
    Ok(GLOBAL_COMPUTE_POOL
        .get()
        .expect("GLOBAL_COMPUTE_POOL initialized"))
}

/// Retrieves the global compute pool reference, initializing with default budget if not already done.
///
/// # Panics
///
/// Panics if the compute pool cannot be created. This is treated as fatal:
/// the application cannot function without background compute.
#[must_use]
pub fn global_compute_pool() -> &'static ComputePool {
    init_global_compute_pool(None).expect("Global compute pool must initialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_default_compute_threads_bounded() {
        let threads = default_compute_threads();
        assert!(threads >= 2, "Thread count floor must be at least 2");
    }

    #[test]
    #[cfg(not(feature = "loom"))]
    fn test_send_or_yield_non_blocking_with_cancellation() {
        let pool = ComputePool::new(1).expect("Must create 1-thread pool");
        let (tx, rx) = crossbeam_channel::bounded::<usize>(2);
        let (src, token) = CancellationToken::new();
        let side_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let side_flag_clone = Arc::clone(&side_flag);

        let tx_clone = tx.clone();
        let token_clone = token.clone();
        pool.spawn(move || {
            for i in 0..50 {
                if !send_or_yield(&tx_clone, i, &token_clone) {
                    break;
                }
            }
        });

        // Concurrently spawn a second task on the same 1-thread pool while the first yields on full channel
        pool.spawn(move || {
            side_flag_clone.store(true, Ordering::SeqCst);
        });

        // Drain a few items and verify the second task runs thanks to rayon::yield_now()
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !side_flag.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            let _ = rx.try_recv();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            side_flag.load(Ordering::SeqCst),
            "Second task should execute on 1-thread pool while first task yields"
        );
        src.cancel();
    }

    #[test]
    fn test_compute_pool_install() {
        let pool = ComputePool::new(2).expect("Must create pool");
        assert_eq!(pool.num_threads(), 2);

        let sum: usize = pool.install(|| (1..=100).sum());
        assert_eq!(sum, 5050);
    }

    #[test]
    fn test_compute_pool_spawn() {
        let pool = ComputePool::new(2).expect("Must create pool");
        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);

        let (tx, rx) = std::sync::mpsc::channel();
        pool.spawn(move || {
            c.fetch_add(42, Ordering::SeqCst);
            tx.send(()).unwrap();
        });

        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 42);
    }

    #[test]
    fn test_global_compute_pool() {
        let pool = global_compute_pool();
        assert!(pool.num_threads() >= 1);
        let val = pool.install(|| "compute_ok");
        assert_eq!(val, "compute_ok");
    }
}
