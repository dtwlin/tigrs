// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Concurrency, permutation, and multi-threaded stress testing for cancellation tokens,
//! generation counters, and task pools.
//!
//! When compiled with `--features loom`, runs deterministic Loom model checking.
//! When compiled under standard `cargo test`, runs multi-threaded OS thread stress tests.

#[cfg(feature = "loom")]
#[test]
fn test_loom_cancellation_permutations() {
    loom::model(|| {
        let (source, token) = tigrs_core::CancellationToken::new();

        let t1 = loom::thread::spawn(move || {
            source.cancel();
        });

        let t2 = loom::thread::spawn(move || {
            let _ = token.is_cancelled();
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}

#[cfg(feature = "loom")]
#[test]
fn test_loom_generation_counter_permutations() {
    loom::model(|| {
        let counter = tigrs_core::GenerationCounter::new();
        let c1 = counter.clone();
        let c2 = counter.clone();

        let t1 = loom::thread::spawn(move || {
            c1.bump();
        });

        let t2 = loom::thread::spawn(move || {
            c2.bump();
        });

        t1.join().unwrap();
        t2.join().unwrap();

        assert_eq!(counter.current(), 2);
    });
}

#[cfg(not(feature = "loom"))]
mod os_concurrency_stress {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use tigrs_core::{CancellationToken, ComputePool, GenerationCounter, send_or_yield};

    #[test]
    fn test_multithreaded_cancellation_visibility_and_drop() {
        for _ in 0..50 {
            let (source, token) = CancellationToken::new();
            let observed_cancel = Arc::new(AtomicUsize::new(0));

            let mut readers = Vec::with_capacity(8);
            for _ in 0..8 {
                let tok = token.clone();
                let obs = Arc::clone(&observed_cancel);
                readers.push(thread::spawn(move || {
                    while !tok.is_cancelled() {
                        thread::yield_now();
                    }
                    obs.fetch_add(1, Ordering::SeqCst);
                }));
            }

            thread::yield_now();
            source.cancel();

            for r in readers {
                r.join().expect("reader thread should not panic");
            }
            assert_eq!(observed_cancel.load(Ordering::SeqCst), 8);
        }
    }

    #[test]
    fn test_multithreaded_generation_counter_strict_monotonicity() {
        let counter = GenerationCounter::new();
        let num_threads = 8;
        let increments_per_thread = 1_000;
        let mut handles = Vec::with_capacity(num_threads);

        for _ in 0..num_threads {
            let c = counter.clone();
            handles.push(thread::spawn(move || {
                let mut prev = 0;
                for _ in 0..increments_per_thread {
                    let next = c.bump();
                    assert!(next > prev, "GenerationCounter must strictly increase");
                    prev = next;
                }
            }));
        }

        for h in handles {
            h.join().expect("thread should not panic");
        }

        assert_eq!(
            counter.current(),
            (num_threads * increments_per_thread) as u64
        );
    }

    #[test]
    fn test_compute_pool_rapid_cancellation_and_send_or_yield_backpressure() {
        let pool = ComputePool::new(4).expect("pool creation must succeed");
        let completed = Arc::new(AtomicUsize::new(0));
        let aborted = Arc::new(AtomicUsize::new(0));

        // Test bounded backpressure with send_or_yield under rapid cancellation
        for i in 0..150 {
            let (source, token) = CancellationToken::new();
            let (tx, rx) = crossbeam_channel::bounded::<usize>(2);
            let comp = Arc::clone(&completed);
            let abrt = Arc::clone(&aborted);
            let tok_clone = token.clone();

            pool.spawn(move || {
                for step in 0..20 {
                    if !send_or_yield(&tx, step, &tok_clone) {
                        abrt.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                }
                comp.fetch_add(1, Ordering::Relaxed);
            });

            if i % 2 == 0 {
                // Cancel while channel might be full or filling
                source.cancel();
            } else {
                // Drain channel
                while rx.recv().is_ok() {}
            }
        }

        // Verify the pool remains healthy and responsive after rapid cancellation storms
        let (tx_final, rx_final) = crossbeam_channel::bounded(1);
        pool.spawn(move || {
            let _ = tx_final.send(42usize);
        });
        assert_eq!(rx_final.recv().unwrap(), 42);
    }
}
