//! Shutdown signals.
//!
//! A service needs to know that it has been asked to stop, so it can finish
//! what it is doing and exit rather than being killed mid-request. Before
//! this, `SIGINT` and `SIGTERM` were not merely un-drained but invisible: the
//! poll loop in [`crate::net`] treats `EINTR` as "ask again", so a signal
//! delivered while the server waited was swallowed and the process could only
//! be killed.
//!
//! `korben-runtime` deliberately has no dependencies, so the handler is
//! installed through the C library directly, as `poll` already is.
//!
//! The handler does one thing: set a flag. That is async-signal-safe, which
//! almost nothing else is — allocating, locking or formatting inside a signal
//! handler is undefined behaviour.

// korben-5k7

use std::os::raw::c_int;
use std::sync::atomic::{AtomicBool, Ordering};

const SIGINT: c_int = 2;
const SIGTERM: c_int = 15;

static REQUESTED: AtomicBool = AtomicBool::new(false);
static INSTALLED: AtomicBool = AtomicBool::new(false);

extern "C" {
    fn signal(signum: c_int, handler: extern "C" fn(c_int)) -> usize;
}

extern "C" fn handle(_signum: c_int) {
    REQUESTED.store(true, Ordering::SeqCst);
}

/// Start watching for `SIGINT` and `SIGTERM`. Safe to call repeatedly.
///
/// This is called when a listener or a connection pool is created, so a
/// program that serves gets shutdown handling without asking for it, and a
/// script that does not serve keeps the default behaviour of dying on Ctrl-C.
pub fn watch() {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    // SAFETY: `handle` is an `extern "C"` function of the right shape, and it
    // only stores to an atomic, which is async-signal-safe.
    unsafe {
        signal(SIGINT, handle);
        signal(SIGTERM, handle);
    }
}

/// Whether a shutdown signal has arrived.
pub fn requested() -> bool {
    REQUESTED.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watching_is_idempotent_and_starts_clear() {
        watch();
        watch();
        // No signal has been raised in this test process, so nothing is
        // pending. This also pins the default: a program that never receives
        // a signal never sees a shutdown request.
        assert!(!requested());
    }
}
