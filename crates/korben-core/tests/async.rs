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

/// A receive nothing can ever satisfy is reported rather than hung. Tasks can
/// suspend now, so this is no longer "a started task cannot wait" -- it is the
/// narrower thing: nobody holds the other end.
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

// ----------------------------------------------------------------- suspension

// korben-5wu
/// The capability this whole design exists for: a task parks partway through,
/// something else runs while it is parked, and it resumes where it stopped.
///
/// The assertion that matters is `:suspended`, seen by another task. Before
/// tasks had stacks of their own there was no such state -- a task that
/// blocked ran everybody else from inside its own frames, so it stayed
/// `:running` the entire time it was waiting, and the output order alone
/// cannot tell those two worlds apart.
#[test]
fn a_task_parks_and_another_task_sees_it_suspended() {
    let result = run(&format!(
        "{HEADER}
(async fn consume [receiver: Receiver] -> Int !async !io
  (println \"consumer waits\")
  (let value (match (receiver.recv) (Some v) v (None) 0))
  (println \"consumer resumed with\" value)
  value)
(async fn produce [sender: Sender other: Task] -> Unit !async !io
  (println \"producer sees consumer\" (other.state))
  (sender.send 7)
  (sender.close))
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (let ends (task.channel))
    (let sender (get ends 0))
    (let receiver (get ends 1))
    (let consumer (spawn scope (consume receiver)))
    (spawn scope (produce sender consumer))
    (println \"total\" (await consumer))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.output,
        "consumer waits\nproducer sees consumer :suspended\nconsumer resumed with 7\ntotal 7\n"
    );
}

// korben-5wu
/// A task waiting on another task is ordinary now, not a cycle. The waiter
/// parks and the task it wants runs; both finish.
#[test]
fn a_task_may_wait_on_another_task() {
    let result = run(&format!(
        "{HEADER}
(async fn slow [] -> Int !async !io (println \"slow ran\") 41)
(async fn waiter [other: Task] -> Int !async !io
  (println \"waiter waits\")
  (+ 1 (await other)))
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (let first (spawn scope (slow)))
    (let second (spawn scope (waiter first)))
    (println (await second))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "waiter waits\nslow ran\n42\n");
}

// korben-5wu
/// Suspension makes waiting legal, so the deadlock diagnostics have to earn
/// their keep on what is still genuinely stuck. A task parked on a channel
/// nobody holds the other end of must be reported, not hung -- and now it is
/// reported from inside a parked task rather than from a task that never
/// started.
#[test]
fn a_parked_task_nobody_can_satisfy_is_reported() {
    let result = run(&format!(
        "{HEADER}
(async fn consume [receiver: Receiver] -> Int !async !io
  (match (receiver.recv) (Some v) v (None) 0))
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (let ends (task.channel))
    (let receiver (get ends 1))
    (println (await (spawn scope (consume receiver))))))"
    ));
    assert_eq!(result.diagnostics, vec!["channel-deadlock"]);
}

// ---------------------------------------------------------------- the reactor

// korben-8h8
/// A task waiting on a socket parks, and the scheduler waits on the socket set
/// rather than on one socket.
///
/// This test hangs against the older behaviour, which is the point. Two tasks
/// each wait to accept; the client connects to the SECOND listener and refuses
/// to touch the first until that task has answered it. With a blocking accept
/// the first task owns the thread, the second never runs, and nothing ever
/// answers -- so the client waits forever. Parking both and polling the pair
/// serves them in the order their peers actually arrive.
///
/// Verified in both directions: it passes in milliseconds as written, and
/// `KORBEN_NO_POLL=1` -- which switches the reactor off and restores the older
/// path -- makes it fail after ten seconds on the client's read timeout, with
/// the second listener never having answered.
#[test]
fn tasks_waiting_on_different_sockets_are_served_as_their_peers_arrive() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    // Reserve two ports the way the other socket tests do, then let them go.
    let port = |()| {
        let probe = TcpListener::bind("127.0.0.1:0").expect("bind");
        probe.local_addr().expect("addr").port()
    };
    let (first, second) = (port(()), port(()));

    let client = std::thread::spawn(move || {
        // Talk to the second listener first, and wait for its answer before
        // going anywhere near the first.
        let mut early = loop {
            if let Ok(stream) = TcpStream::connect(("127.0.0.1", second)) {
                break stream;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let mut answer = String::new();
        early.set_read_timeout(Some(std::time::Duration::from_secs(10))).expect("timeout");
        let _ = early.read_to_string(&mut answer);
        let _ = early.write_all(b"");
        let mut late = TcpStream::connect(("127.0.0.1", first)).expect("connect first");
        let mut second_answer = String::new();
        late.set_read_timeout(Some(std::time::Duration::from_secs(10))).expect("timeout");
        let _ = late.read_to_string(&mut second_answer);
        (answer, second_answer)
    });

    let result = run(&format!(
        "(module m (use std.async :as task) (use std.net :as net))
(async fn serve [address: String name: String] -> String !async !io
  (match (net.listen address)
    (Err _) \"listen failed\"
    (Ok listener)
      (match (listener.accept)
        (Err _) (do (listener.close) \"accept failed\")
        (Ok connection)
          (do (connection.write name)
              (connection.close)
              (listener.close)
              name))))
(pub fn main [] -> Unit !io !async
  (task-scope scope
    (let a (spawn scope (serve \"127.0.0.1:{first}\" \"first\")))
    (let b (spawn scope (serve \"127.0.0.1:{second}\" \"second\")))
    (println (await b))
    (println (await a))))"
    ));

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "second\nfirst\n");
    let (early, late) = client.join().expect("client thread");
    assert_eq!(early, "second", "the second listener answered first");
    assert_eq!(late, "first");
}
