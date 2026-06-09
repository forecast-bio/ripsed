//! Concurrent integration tests for `ripsed::apply_to_file`.
//!
//! The facade crate's contract (added in 0.2.7) is that concurrent calls
//! to `apply_to_file` on the same path are serialized by a read-modify-write
//! lock, so they never produce torn or interleaved output. These tests
//! hammer that guarantee from multiple threads and processes.
//!
//! Why these aren't in `ripsed-fs::lock`: lock acquisition can be correct
//! while the compound read-modify-write in the facade still races. The
//! only way to catch that is to drive the full public API concurrently.

use ripsed::{ApplyOptions, Op, apply_to_file};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;

fn make_replace_op(find: &str, replace: &str) -> Op {
    Op::Replace {
        count: Default::default(),
        multiline: false,
        find: find.to_string(),
        replace: replace.to_string(),
        regex: false,
        case_insensitive: false,
    }
}

/// N threads simultaneously replace a distinct tag with its own unique
/// replacement. After all threads join, each replacement must have happened
/// exactly once and the file must be in a valid, non-torn state.
///
/// Without proper RMW locking, this would interleave reads and writes and
/// some threads' replacements would be silently lost.
#[test]
fn concurrent_distinct_replacements_all_land() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("target.txt");

    // Seed the file with N distinct tokens.
    const N: usize = 8;
    let tokens: Vec<String> = (0..N).map(|i| format!("TAG_{i:03}")).collect();
    let seed: String = tokens.iter().map(|t| format!("{t}\n")).collect();
    fs::write(&path, &seed).unwrap();

    let barrier = Arc::new(Barrier::new(N));
    let path = Arc::new(path);

    let mut handles = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let p = Arc::clone(&path);
        let b = Arc::clone(&barrier);
        let token = token.clone();
        let replacement = format!("DONE_{i:03}");
        handles.push(thread::spawn(move || {
            b.wait(); // synchronize start to maximize contention
            let op = make_replace_op(&token, &replacement);
            let opts = ApplyOptions {
                dry_run: false,
                ..Default::default()
            };
            let res = apply_to_file(&p, &op, &opts);
            assert!(
                res.is_ok(),
                "thread {i}: apply_to_file failed: {:?}",
                res.err()
            );
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    // After all threads complete: every tag must have been replaced exactly once.
    let final_content = fs::read_to_string(&*path).unwrap();
    for (i, tag) in tokens.iter().enumerate() {
        let replacement = format!("DONE_{i:03}");
        assert!(
            !final_content.contains(tag),
            "Tag {tag} survived — a concurrent write lost this replacement (lock race).\nFinal file:\n{final_content}"
        );
        assert!(
            final_content.contains(&replacement),
            "Replacement {replacement} missing — a concurrent write dropped this change.\nFinal file:\n{final_content}"
        );
    }

    // The `.ripsed.lock` sentinel stays on disk by design (removing it
    // would race with new acquirers and allow flocking a different
    // inode at the same path). We only assert the lock itself is
    // released: a fresh acquire must succeed.
    let lock = PathBuf::from(format!("{}.ripsed.lock", path.display()));
    if lock.exists() {
        // Re-acquire to prove the flock is released.
        let _ = ripsed::FileLock::acquire(&path).expect("lock should be re-acquirable");
    }
}

/// N threads all attempt the same replacement. With proper RMW locking, at
/// most one of them should observe the pre-replacement state; the rest see
/// the already-replaced text and correctly do nothing. The file must end up
/// with the replacement applied exactly once.
#[test]
fn concurrent_idempotent_replacement_is_safe() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("target.txt");
    fs::write(&path, "BEFORE one\nBEFORE two\nBEFORE three\n").unwrap();

    const N: usize = 10;
    let barrier = Arc::new(Barrier::new(N));
    let path = Arc::new(path);
    let mut handles = Vec::new();
    for i in 0..N {
        let p = Arc::clone(&path);
        let b = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            b.wait();
            let op = make_replace_op("BEFORE", "AFTER");
            let opts = ApplyOptions {
                dry_run: false,
                ..Default::default()
            };
            let _ = apply_to_file(&p, &op, &opts)
                .unwrap_or_else(|e| panic!("thread {i}: apply_to_file failed: {e}"));
        }));
    }
    for h in handles {
        h.join().expect("thread panicked");
    }

    // Final state: file has AFTER three times, never torn.
    let final_content = fs::read_to_string(&*path).unwrap();
    assert_eq!(
        final_content, "AFTER one\nAFTER two\nAFTER three\n",
        "Torn or lost write under concurrent identical replacement"
    );

    // Sentinel stays on disk; verify the lock itself was released by
    // re-acquiring.
    let _lock = ripsed::FileLock::acquire(&path).expect("lock should be re-acquirable");
}

/// Dry-run from many threads must NEVER modify the file.
/// Regression guard for the 0.2.7 change that skips lock acquisition on
/// dry-run: a bug there could cause one dry-run to accidentally commit.
#[test]
fn concurrent_dry_runs_never_modify() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("target.txt");
    let original = "original content line one\noriginal content line two\n";
    fs::write(&path, original).unwrap();

    const N: usize = 16;
    let barrier = Arc::new(Barrier::new(N));
    let path = Arc::new(path);
    let mut handles = Vec::new();
    for _ in 0..N {
        let p = Arc::clone(&path);
        let b = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            b.wait();
            let op = make_replace_op("original", "MODIFIED");
            let opts = ApplyOptions::default(); // dry_run = true
            let res = apply_to_file(&p, &op, &opts).unwrap();
            // Dry run should still compute changes
            assert!(!res.changes.is_empty());
        }));
    }
    for h in handles {
        h.join().expect("thread panicked");
    }

    // Content must be exactly original.
    let final_content = fs::read_to_string(&*path).unwrap();
    assert_eq!(
        final_content, original,
        "Concurrent dry-runs silently modified the file"
    );

    // No lock file residue — dry_run skips acquisition entirely.
    let lock = PathBuf::from(format!("{}.ripsed.lock", path.display()));
    assert!(!lock.exists(), "lock file from dry-run leaked: {lock:?}");
}

/// Reader/writer race: while N reader threads call `apply_to_file` in
/// dry-run (no lock), M writer threads mutate the file. The final file
/// content must match one of the possible final states (after some
/// serialization of the M writes), not a torn intermediate.
///
/// Bounded by reads returning `Ok` (don't panic on a racing rename).
#[test]
fn concurrent_readers_and_writers_do_not_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("target.txt");
    fs::write(&path, "counter=0\n").unwrap();

    const WRITERS: usize = 4;
    const READERS: usize = 4;
    let barrier = Arc::new(Barrier::new(WRITERS + READERS));
    let path = Arc::new(path);
    let mut handles = Vec::new();

    for i in 0..WRITERS {
        let p = Arc::clone(&path);
        let b = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            b.wait();
            let op = make_replace_op(&format!("counter={}", i), &format!("counter={}", i + 1));
            let opts = ApplyOptions {
                dry_run: false,
                ..Default::default()
            };
            let _ = apply_to_file(&p, &op, &opts); // may or may not match
        }));
    }
    for _ in 0..READERS {
        let p = Arc::clone(&path);
        let b = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            b.wait();
            let op = make_replace_op("counter", "peek");
            let opts = ApplyOptions::default(); // dry_run = true
            // Readers in dry-run must not panic even when writers are racing.
            let _ = apply_to_file(&p, &op, &opts);
        }));
    }
    for h in handles {
        h.join().expect("thread panicked");
    }

    // Final state: file is a valid UTF-8 string and matches a counter= shape.
    let final_content = fs::read_to_string(&*path).unwrap();
    assert!(
        final_content.starts_with("counter=") && final_content.ends_with('\n'),
        "File is torn after concurrent R/W: {final_content:?}"
    );
}
