//! Where a program's time goes, when it is asked.
//!
//! Specification 23 wants profiling "without changing ordinary source code", so
//! there is nothing to annotate: `apply_now` is the one funnel every call passes
//! through, and a function value carries its own name, so a single hook there
//! covers user functions, builtins, protocol methods, and constructors alike.
//!
//! What is reported is *self* time -- the time inside a function's own body,
//! with everything it called subtracted. Inclusive time is the more familiar
//! number, but it double-counts a recursive call and has to be explained every
//! time someone reads it; self time says plainly which body the program is
//! actually sitting in.
//!
//! Off unless asked for. When it is off the cost is one thread-local read per
//! call, which is why the check is a `Cell<bool>` and not a lock.

// korben-ycd

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Instant;

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static STATE: RefCell<State> = RefCell::new(State::new());
}

struct State {
    functions: HashMap<String, Entry>,
    stack: Vec<Frame>,
}

impl State {
    fn new() -> State {
        State { functions: HashMap::new(), stack: Vec::new() }
    }
}

/// A call in progress, and how much of it belongs to what it called.
struct Frame {
    name: String,
    started: Instant,
    in_children: u128,
}

/// What one function did.
#[derive(Clone, Copy, Default)]
pub struct Entry {
    pub calls: u64,
    /// Nanoseconds in this function's own body, excluding what it called.
    pub self_nanos: u128,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{} calls, {}ns", self.calls, self.self_nanos)
    }
}

/// Begin recording. Nothing is measured until this is called.
pub fn start() {
    ENABLED.with(|enabled| enabled.set(true));
    STATE.with(|state| *state.borrow_mut() = State::new());
}

/// Whether calls are being measured, checked once per call.
#[inline]
pub fn enabled() -> bool {
    ENABLED.with(|enabled| enabled.get())
}

/// Note that a call to `name` has begun.
pub fn enter(name: &str) {
    STATE.with(|state| {
        state.borrow_mut().stack.push(Frame {
            name: name.to_string(),
            started: Instant::now(),
            in_children: 0,
        });
    });
}

/// Note that the innermost call has returned, however it returned.
pub fn leave() {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(frame) = state.stack.pop() else { return };
        let elapsed = frame.started.elapsed().as_nanos();
        // Time spent inside this call belongs to whoever made it, so the caller
        // does not count it as its own.
        if let Some(parent) = state.stack.last_mut() {
            parent.in_children += elapsed;
        }
        let entry = state.functions.entry(frame.name).or_default();
        entry.calls += 1;
        entry.self_nanos += elapsed.saturating_sub(frame.in_children);
    });
}

/// Every function that ran, the one it sat in longest first.
pub fn report() -> Vec<(String, Entry)> {
    STATE.with(|state| {
        let state = state.borrow();
        let mut rows: Vec<(String, Entry)> =
            state.functions.iter().map(|(name, entry)| (name.clone(), *entry)).collect();
        rows.sort_by(|left, right| {
            right.1.self_nanos.cmp(&left.1.self_nanos).then_with(|| left.0.cmp(&right.0))
        });
        rows
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The thread-local state is per test thread, so these do not collide.
    #[test]
    fn time_belongs_to_the_body_it_was_spent_in() {
        start();
        enter("outer");
        enter("inner");
        std::thread::sleep(std::time::Duration::from_millis(20));
        leave();
        leave();

        let rows = report();
        let outer = rows.iter().find(|(name, _)| name == "outer").expect("outer").1;
        let inner = rows.iter().find(|(name, _)| name == "inner").expect("inner").1;
        assert_eq!(outer.calls, 1);
        assert_eq!(inner.calls, 1);
        // The sleep happened in `inner`, so `outer` is not charged for it.
        assert!(inner.self_nanos > outer.self_nanos, "inner {inner:?} outer {outer:?}");
        // And the longest-running body is reported first.
        assert_eq!(rows[0].0, "inner");
    }

    #[test]
    fn a_recursive_call_is_counted_once_per_call() {
        start();
        for _ in 0..3 {
            enter("recurse");
        }
        for _ in 0..3 {
            leave();
        }
        let rows = report();
        let entry = rows.iter().find(|(name, _)| name == "recurse").expect("recurse").1;
        // Three calls, and the nested time is not charged three times over.
        assert_eq!(entry.calls, 3);
    }
}
