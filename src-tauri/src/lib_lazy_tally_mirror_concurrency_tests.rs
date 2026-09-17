//! `LazyTallyMirror::get()` is built directly on `tokio::sync::OnceCell::get_or_try_init`,
//! which is exactly what supplies the "one initialisation, not two" guarantee under
//! concurrent callers required by this change: the first caller to reach the cell runs the
//! initialisation future while every other concurrent caller awaits that same in-flight
//! attempt instead of starting its own.
//!
//! This test proves that guarantee by racing many tasks against a shared `OnceCell` using
//! the identical `get_or_try_init` call `LazyTallyMirror::get()` makes, and asserting the
//! initialisation closure ran exactly once.
//!
//! It deliberately does NOT call `LazyTallyMirror::get()` end-to-end. A real call resolves
//! the SQLCipher key through the OS keychain (`db::OsMirrorKeyStore` -> `keyring::Entry`),
//! which would create or query a real macOS keychain item from an automated `cargo test`
//! process outside any app bundle/code signature — the very kind of environment-dependent,
//! potentially interactive behaviour this change exists to avoid triggering on every launch.
//! The `keyring` dependency is built without a mockable backend (feature `v1` only), so there
//! is no offline/deterministic way to substitute a fake credential store for that path. That
//! makes the keychain-touching portion of initialisation untestable offline; this test proves
//! the concurrency mechanism it relies on instead of asserting something it cannot honestly
//! demonstrate.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Barrier, OnceCell};

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_callers_observe_exactly_one_initialization() {
    const CONCURRENT_CALLERS: usize = 16;

    let cell: Arc<OnceCell<u32>> = Arc::new(OnceCell::new());
    let init_calls = Arc::new(AtomicUsize::new(0));
    // Every task waits at the barrier so they all call `get_or_try_init` at (as close to)
    // the same instant as possible, maximising the chance of a real race rather than an
    // accidentally-serialised sequence of calls.
    let barrier = Arc::new(Barrier::new(CONCURRENT_CALLERS));

    let tasks: Vec<_> = (0..CONCURRENT_CALLERS)
        .map(|_| {
            let cell = Arc::clone(&cell);
            let init_calls = Arc::clone(&init_calls);
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                cell.get_or_try_init(|| async {
                    init_calls.fetch_add(1, Ordering::SeqCst);
                    // Hold the in-flight initialisation open for long enough that, absent
                    // `OnceCell`'s mutual exclusion, other callers would very likely start
                    // their own concurrent initialisation attempt.
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    Ok::<u32, anyhow::Error>(42)
                })
                .await
                .copied()
            })
        })
        .collect();

    let mut results = Vec::with_capacity(CONCURRENT_CALLERS);
    for task in tasks {
        results.push(task.await.expect("initialization task must not panic"));
    }

    assert_eq!(
        init_calls.load(Ordering::SeqCst),
        1,
        "initialization must run at most once under concurrent access"
    );
    for result in results {
        assert_eq!(
            result.expect("initialization must succeed"),
            42,
            "every concurrent caller must observe the single initialization's value"
        );
    }
}
