//! The async runtime: tasks, scopes, cancellation, and channels.

mod common;
use common::{check, run};

const HEADER: &str = "(module m (use std.async :as task))\n";

#[test]
fn calling_an_async_function_does_not_run_it() {
    let result = run(&format!(
        "{HEADER}
(async fn work [] -> Int !async !io (println \"ran\") 1)
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (let pending (work))
    (println \"still pending\")
    (println (pending.state))
    (println (await pending))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "still pending\n:pending\nran\n1\n");
}

#[test]
fn spawned_tasks_run_when_they_are_joined() {
    let result = run(&format!(
        "{HEADER}
(async fn work [n: Int] -> Int !async !io (println n) n)
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (let tasks (map [1 2 3] (fn [n] (spawn scope (work n)))))
    (println \"spawned\")
    (println (task.join-all tasks))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "spawned\n1\n2\n3\n(Ok [1 2 3])\n");
}

/// Specification 15.2: a task must not silently outlive the operation that
/// created it, so a scope runs children nobody awaited.
#[test]
fn a_scope_joins_children_nobody_awaited() {
    let result = run(&format!(
        "{HEADER}
(async fn work [n: Int] -> Int !async !io (println n) n)
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (spawn scope (work 1))
    (spawn scope (work 2))
    (println \"body done\"))
  (println \"after the scope\"))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "body done\n1\n2\nafter the scope\n");
}

#[test]
fn a_failing_child_short_circuits_join_all() {
    let result = run(&format!(
        "{HEADER}
(async fn work [n: Int] -> Result Int String !async
  (if (= n 2) (Err \"two is bad\") (Ok n)))
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (let tasks (map [1 2 3] (fn [n] (spawn scope (work n)))))
    (println (task.join-all tasks))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "(Err \"two is bad\")\n");
}

#[test]
fn a_child_that_faults_reaches_the_code_that_started_it() {
    let result = run(&format!(
        "{HEADER}
(async fn boom [] -> Int !async (throw \"child failed\"))
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (spawn scope (boom))
    (println \"body done\")))"
    ));
    // The body finishes, then the scope joins the child and the failure escapes.
    assert_eq!(result.output, "body done\n");
    assert_eq!(result.diagnostics, vec!["condition"]);
}

#[test]
fn cancellation_stops_work_that_has_not_started() {
    let result = run(&format!(
        "{HEADER}
(async fn work [] -> Int !async !io (println \"should not run\") 1)
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (let pending (spawn scope (work)))
    (scope.cancel)
    (println (scope.cancelled?))
    (println (pending.state))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "true\n:cancelled\n");
}

#[test]
fn a_channel_connects_a_producer_and_a_consumer() {
    let result = run(&format!(
        "{HEADER}
(async fn produce [sender: Sender] -> Unit !async
  (each [1 2 3] (fn [n] (sender.send n)))
  (sender.close))
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (let ends (task.channel))
    (let sender (get ends 0))
    (let receiver (get ends 1))
    (spawn scope (produce sender))
    (loop [total 0]
      (match (receiver.recv)
        (Some value) (recur (+ total value))
        (None) (println total)))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "6\n");
}

#[test]
fn try_recv_does_not_drive_anything() {
    let result = run(&format!(
        "{HEADER}
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (let ends (task.channel))
    (let sender (get ends 0))
    (let receiver (get ends 1))
    (println (receiver.try-recv))
    (sender.send 42)
    (println (receiver.try-recv))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "(None)\n(Some 42)\n");
}

/// A started task cannot suspend, so a cycle is reported rather than hung.
#[test]
fn a_receive_with_nothing_to_drive_is_a_reported_deadlock() {
    let result = run(&format!(
        "{HEADER}
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (let ends (task.channel))
    (let receiver (get ends 1))
    (println (receiver.recv))))"
    ));
    assert_eq!(result.diagnostics, vec!["channel-deadlock"]);
}

#[test]
fn a_closed_channel_ends_the_consumer_loop() {
    let result = run(&format!(
        "{HEADER}
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (let ends (task.channel))
    (let sender (get ends 0))
    (let receiver (get ends 1))
    (sender.send 1)
    (sender.close)
    (println (receiver.recv))
    (println (receiver.recv))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "(Some 1)\n(None)\n");
}

// ------------------------------------------------------------------- checking

#[test]
fn an_async_function_is_typed_as_returning_a_task() {
    // Awaiting unwraps the task, so the result is an ordinary `Int`.
    assert!(check(
        "(async fn work [] -> Int !async 1)
         (fn caller [] -> Int !async (async (await (work))))"
    )
    .is_empty());

    // Using the call without awaiting it is a task, not an `Int`.
    assert_eq!(
        check(
            "(async fn work [] -> Int !async 1)
             (fn caller [] -> Int !async (work))"
        ),
        vec!["type-mismatch"]
    );
}

#[test]
fn await_outside_asynchronous_code_is_rejected() {
    assert_eq!(
        check("(fn f [] -> Int !async (await 1))"),
        vec!["await-context"],
        "specification 15.1 limits `await` to asynchronous code"
    );
    assert!(
        check("(async fn f [] -> Int !async (await 1))").is_empty(),
        "an async function is an asynchronous context"
    );
    assert!(
        check("(fn f [] -> Int !async (async (await 1)))").is_empty(),
        "so is an `async` block"
    );
}

#[test]
fn spawn_needs_a_scope() {
    assert_eq!(
        check(
            "(async fn work [] -> Int !async 1)
             (fn f [] -> Unit !async (let s 1) (spawn s (work)) nil)"
        ),
        vec!["type-mismatch"]
    );
}

#[test]
fn asynchronous_work_infers_the_async_effect() {
    assert_eq!(
        check(
            "(async fn work [] -> Int !async 1)
             (pub fn caller [] -> Unit (task-scope scope (spawn scope (work)) nil))"
        ),
        vec!["undeclared-effect"]
    );
}
