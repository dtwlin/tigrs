// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Suite 1: Linux VFS Invalidation (`CacheInvalidator`) and `RepoWatcher` Stability Tests.
//!
//! Covers:
//! - Packfile stamp verification across `.pack`/`.idx` additions, deletions, atomic renames,
//!   and size/inode changes.
//! - Immunity against false-positive invalidation from auxiliary git files (`.bitmap`, `.rev`, `.keep`, `.tmp_pack_*`).
//! - Multi-threaded concurrent `verify_pack_stamps` and `record_pack_stamps` under contention.
//! - `purge_caches` on live `gix::Repository` instances.
//! - `RepoWatcher` high-frequency filesystem churn (100 rapid mutations across HEAD, index, refs, and pack dirs)
//!   and deterministic thread shutdown safety.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use tigrs_git::invalidation::{CacheInvalidator, PackFileStamp};
use tigrs_git::watcher::{RepoChangeEvent, RepoWatcher};

#[test]
fn test_vfs_pack_stamps_ignore_auxiliary_git_files_and_detect_real_changes() {
    let temp = tempfile::tempdir().unwrap();
    let pack_dir = temp.path().join("objects").join("pack");
    fs::create_dir_all(&pack_dir).unwrap();

    let pack_a = pack_dir.join("pack-aaaa.pack");
    let idx_a = pack_dir.join("pack-aaaa.idx");
    fs::write(&pack_a, b"PACK\x00\x00\x00\x02\x00\x00\x00\x00").unwrap();
    fs::write(&idx_a, b"\xfftOc\x00\x00\x00\x02").unwrap();

    let invalidator = CacheInvalidator::new();
    invalidator.record_pack_stamps(&pack_dir).unwrap();
    assert!(
        invalidator.verify_pack_stamps(&pack_dir).unwrap(),
        "Initial stamp verification must succeed"
    );

    // 1. Adding auxiliary non-.pack/.idx files MUST NOT invalidate stamps
    for aux_name in [
        "pack-aaaa.bitmap",
        "pack-aaaa.rev",
        "pack-aaaa.keep",
        "pack-aaaa.promisor",
        "multi-pack-index",
        ".tmp_pack_xyz123",
    ] {
        let aux_path = pack_dir.join(aux_name);
        fs::write(&aux_path, b"auxiliary metadata").unwrap();
        assert!(
            invalidator.verify_pack_stamps(&pack_dir).unwrap(),
            "Auxiliary file {aux_name} must not trigger false-positive pack cache invalidation"
        );
    }

    // 2. Atomic rename of temporary file to new `.pack` file MUST invalidate stamps
    let tmp_pack = pack_dir.join(".tmp_pack_new");
    let new_pack = pack_dir.join("pack-bbbb.pack");
    fs::write(&tmp_pack, b"new pack data").unwrap();
    assert!(
        invalidator.verify_pack_stamps(&pack_dir).unwrap(),
        "Unrenamed .tmp_pack must be ignored"
    );
    fs::rename(&tmp_pack, &new_pack).unwrap();
    assert!(
        !invalidator.verify_pack_stamps(&pack_dir).unwrap(),
        "Renaming to .pack must be detected immediately"
    );

    // Re-record stamps
    invalidator.record_pack_stamps(&pack_dir).unwrap();
    assert!(invalidator.verify_pack_stamps(&pack_dir).unwrap());

    // 3. Size change on existing `.idx` file MUST invalidate stamps
    fs::write(&idx_a, b"\xfftOc\x00\x00\x00\x02extra_bytes_appended").unwrap();
    assert!(
        !invalidator.verify_pack_stamps(&pack_dir).unwrap(),
        "Size modification on .idx must be detected"
    );

    // Re-record stamps
    invalidator.record_pack_stamps(&pack_dir).unwrap();
    assert!(invalidator.verify_pack_stamps(&pack_dir).unwrap());

    // 4. Inode replacement (unlink + recreate with identical size) MUST invalidate stamps on Unix
    #[cfg(unix)]
    {
        let old_meta = fs::metadata(&pack_a).unwrap();
        let old_stamp = PackFileStamp::from_metadata(&old_meta);
        fs::remove_file(&pack_a).unwrap();
        // Create another file in between to encourage new inode allocation or mtime difference
        let spacer = pack_dir.join("spacer.tmp");
        fs::write(&spacer, b"spacer").unwrap();
        thread::sleep(Duration::from_millis(15));
        fs::write(&pack_a, b"PACK\x00\x00\x00\x02\x00\x00\x00\x00").unwrap();
        let new_meta = fs::metadata(&pack_a).unwrap();
        let new_stamp = PackFileStamp::from_metadata(&new_meta);
        if old_stamp != new_stamp {
            assert!(
                !invalidator.verify_pack_stamps(&pack_dir).unwrap(),
                "Inode or mtime change on recreated packfile must invalidate stamps"
            );
        }
        let _ = fs::remove_file(&spacer);
    }
}

#[test]
fn test_vfs_cache_invalidator_multithreaded_contention_stress() {
    let temp = tempfile::tempdir().unwrap();
    let pack_dir = temp.path().join("pack");
    fs::create_dir_all(&pack_dir).unwrap();

    for i in 0..10 {
        fs::write(
            pack_dir.join(format!("pack-{i:04}.pack")),
            format!("pack content {i}"),
        )
        .unwrap();
        fs::write(
            pack_dir.join(format!("pack-{i:04}.idx")),
            format!("idx content {i}"),
        )
        .unwrap();
    }

    let invalidator = Arc::new(CacheInvalidator::new());
    invalidator.record_pack_stamps(&pack_dir).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::new();

    // Spawn 6 reader threads continuously verifying stamps
    for _ in 0..6 {
        let inv = Arc::clone(&invalidator);
        let dir = pack_dir.clone();
        let stop_flag = Arc::clone(&stop);
        handles.push(thread::spawn(move || {
            let mut checks = 0usize;
            while !stop_flag.load(Ordering::Relaxed) {
                let _ = inv.verify_pack_stamps(&dir).unwrap();
                let _ = inv.current_generation();
                checks += 1;
            }
            assert!(checks > 0);
        }));
    }

    // Spawn 2 writer threads mutating pack files, bumping generations, and re-recording stamps
    for writer_id in 0..2 {
        let inv = Arc::clone(&invalidator);
        let dir = pack_dir.clone();
        handles.push(thread::spawn(move || {
            for step in 0..30 {
                let p = dir.join(format!("pack-dyn-{writer_id}-{step}.pack"));
                fs::write(&p, format!("dynamic pack {step}")).unwrap();
                inv.record_pack_stamps(&dir).unwrap();
                let generation = inv.bump_generation();
                assert!(generation > 1);
                let _ = fs::remove_file(&p);
                inv.record_pack_stamps(&dir).unwrap();
            }
        }));
    }

    // Wait for writers to finish, then stop readers
    for h in handles.drain(6..) {
        h.join().expect("writer thread must not panic");
    }
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        h.join().expect("reader thread must not panic");
    }
}

#[test]
fn test_purge_caches_on_live_gix_repository() {
    let temp = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();

    // Initialize a real git repository using git CLI
    let status = std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["init", "-b", "main"])
        .current_dir(repo_dir)
        .status()
        .expect("git init");
    assert!(status.success());

    std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "user.name", "SQE Tester"])
        .current_dir(repo_dir)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["config", "user.email", "sqe@example.com"])
        .current_dir(repo_dir)
        .status()
        .unwrap();

    fs::write(repo_dir.join("file.txt"), "initial\n").unwrap();
    std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["add", "file.txt"])
        .current_dir(repo_dir)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["commit", "-m", "commit 1"])
        .current_dir(repo_dir)
        .status()
        .unwrap();

    let mut gix_repo = gix::open(repo_dir).expect("gix open");
    let invalidator = CacheInvalidator::new();
    let gen_before = invalidator.current_generation();

    let gen_after = invalidator
        .purge_caches(&mut gix_repo)
        .expect("purge_caches must succeed");
    assert_eq!(gen_after, gen_before + 1);
    assert_eq!(invalidator.current_generation(), gen_after);
}

#[test]
fn test_repo_watcher_high_frequency_churn_and_clean_shutdown() {
    let temp = tempfile::tempdir().unwrap();
    let git_dir = temp.path().join(".git");
    let refs_heads = git_dir.join("refs").join("heads");
    let pack_dir = git_dir.join("objects").join("pack");
    fs::create_dir_all(&refs_heads).unwrap();
    fs::create_dir_all(&pack_dir).unwrap();

    fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(git_dir.join("index"), b"DIRC\x00\x00\x00\x02").unwrap();

    // Start watcher and immediately subject it to 100 rapid filesystem mutations
    let watcher = RepoWatcher::start(&git_dir, &git_dir).expect("RepoWatcher::start");

    for step in 0..100 {
        match step % 5 {
            0 => fs::write(git_dir.join("HEAD"), format!("ref: refs/heads/b{step}\n")).unwrap(),
            1 => fs::write(git_dir.join("index"), format!("DIRC-step-{step}")).unwrap(),
            2 => fs::write(refs_heads.join("main"), format!("{step:040x}\n")).unwrap(),
            3 => fs::write(git_dir.join("MERGE_HEAD"), format!("{step:040x}\n")).unwrap(),
            _ => fs::write(pack_dir.join("pack-churn.pack"), format!("PACK-{step}")).unwrap(),
        }
        if step % 10 == 0 {
            let events = watcher.drain_events();
            for ev in events {
                // Ensure every classified event is one of our valid enum variants
                assert!(matches!(
                    ev,
                    RepoChangeEvent::Head
                        | RepoChangeEvent::Index
                        | RepoChangeEvent::Refs
                        | RepoChangeEvent::Status
                        | RepoChangeEvent::Objects
                        | RepoChangeEvent::Generic
                ));
            }
        }
    }

    // Drain any remaining events and drop watcher cleanly (verifying Drop joins heartbeat thread promptly)
    let _ = watcher.drain_events();
    let drop_start = std::time::Instant::now();
    drop(watcher);
    assert!(
        drop_start.elapsed() < Duration::from_secs(2),
        "RepoWatcher::drop must join background thread promptly without stalling"
    );
}
