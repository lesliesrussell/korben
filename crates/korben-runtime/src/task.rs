//! The async runtime: tasks, scopes, cancellation, and channels.
//!
//! Korben values are reference counted, so they belong to one thread. The
//! scheduler here is therefore cooperative and single-threaded: tasks are
//! concurrent, not simultaneous.
//!
//! Calling an `async fn` yields a [`Task`] instead of running it. A started
//! task runs on a stack of its own, so it can genuinely suspend: an operation
//! that would block -- awaiting, receiving from an empty channel, sending to a
//! full one, waiting on a socket -- parks the task where it stands and hands
//! control back to whoever resumed it. Its frames, and the `Rc` values in
//! them, stay exactly as they were until it is resumed.
//!
//! That the stack switch never leaves this thread is what makes reference
//! counted values safe to hold across a suspend, and it is why the scheduler
//! is built this way rather than on threads.
//!
//! Suspension is what the deadlock diagnostics are now *for*. A task waiting
//! on another task is ordinary, not a cycle: the waiter parks and the other
//! runs. A deadlock is the narrower thing it always should have been -- a
//! whole round of the scheduler in which nothing anywhere made progress.

// korben-du1

use crate::apply::apply_now;
use crate::loc::{Fault, Loc};
use crate::value::{Arg, Caller, Flow, Foreign, Outcome, Value};
use corosensei::{Coroutine, CoroutineResult, Yielder};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

// --------------------------------------------------------------------- tasks

// korben-5wu
/// A task's own stack, parked. Resuming it returns whatever it produces next:
/// `Yield` when it parks again, `Return` when it is finished.
type TaskStack = Coroutine<(), (), Outcome>;

pub enum TaskState {
    /// Not started: the call it will make, once something needs its value.
    Pending {
        function: Value,
        args: Vec<Arg>,
        loc: Loc,
    },
    // korben-5wu
    /// Started, then parked on its own stack partway through. This is the
    /// state that did not exist before: a task used to run to completion or
    /// not at all.
    Suspended(Box<TaskStack>),
    /// Executing right now, somewhere below this frame.
    Running,
    Done(Value),
    Failed(Box<Fault>),
    Cancelled,
}

pub struct TaskCell {
    pub label: String,
    pub state: RefCell<TaskState>,
}

pub struct ScopeCell {
    pub label: String,
    cancelled: Cell<bool>,
    tasks: RefCell<Vec<Rc<TaskCell>>>,
}

thread_local! {
    /// Innermost-last stack of open scopes, which is where ready work is found.
    static SCOPES: RefCell<Vec<Rc<ScopeCell>>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<usize> = const { Cell::new(0) };

    // korben-5wu
    /// The host, as an owned handle. A task's stack outlives the call that
    /// started it, so it cannot borrow the host -- see korben-c9v.
    static HOST: RefCell<Option<Rc<dyn Caller>>> = const { RefCell::new(None) };

    /// The yielder belonging to the task currently on the stack, or null at
    /// the top level. Reading it is how a blocking operation five frames deep
    /// finds the task it is inside without every signature carrying it.
    static YIELDER: Cell<*const Yielder<(), ()>> = const { Cell::new(std::ptr::null()) };

    /// Bumped whenever anything anywhere moves forward. A round of the
    /// scheduler that does not change it is a round in which nothing can.
    static PROGRESS: Cell<u64> = const { Cell::new(0) };

    /// Where the last round left off, so resuming is round-robin rather than
    /// always restarting at the first task. Without it a task that parks and
    /// is immediately resumed starves every task behind it.
    static CURSOR: Cell<usize> = const { Cell::new(0) };

    /// Set for one last pass once the scheduler has established that nothing
    /// can move. Parking is refused during it, so the operation a task is
    /// stuck on reports its own diagnostic -- naming the channel, and what to
    /// do about it -- instead of the scheduler reporting a generic stall from
    /// somewhere that knows nothing about why.
    static STUCK: Cell<bool> = const { Cell::new(false) };
}

// korben-5wu
/// Record that something moved forward: a task finished, a value crossed a
/// channel, a socket became ready. Deadlock detection is the difference
/// between rounds of this.
pub(crate) fn note_progress() {
    PROGRESS.with(|counter| counter.set(counter.get().wrapping_add(1)));
}

fn progress() -> u64 {
    PROGRESS.with(|counter| counter.get())
}

// korben-5wu
/// Install the handle tasks will run against. The program's entry point calls
/// this once, before anything can spawn.
pub fn set_host(host: Rc<dyn Caller>) {
    HOST.with(|slot| *slot.borrow_mut() = Some(host));
}

fn host() -> Option<Rc<dyn Caller>> {
    HOST.with(|slot| slot.borrow().clone())
}

// korben-5wu
/// Park the task this code is running inside, if it is running inside one.
///
/// Returns `false` at the top level, where there is no stack to park -- `main`
/// is not a task, so a blocking operation there has to fall back to running
/// somebody else.
fn park() -> bool {
    if STUCK.with(|flag| flag.get()) {
        return false;
    }
    let yielder = YIELDER.with(|slot| slot.get());
    if yielder.is_null() {
        return false;
    }
    // SAFETY: the pointer is non-null only between entering a task's stack and
    // leaving it. It is set by the coroutine body on entry, restored here on
    // the way back from a suspend, and restored to the resumer's own value by
    // `resume_task` once the coroutine hands control back. Only one coroutine
    // runs at a time on this thread, so the yielder it names is the one whose
    // stack this frame is on, and that stack is alive for as long as this
    // frame is.
    unsafe {
        (*yielder).suspend(());
    }
    // Resumed. The scheduler restored its own yielder when we parked, so put
    // ours back before returning into the task's frames.
    YIELDER.with(|slot| slot.set(yielder));
    true
}

fn next_id() -> usize {
    NEXT_ID.with(|counter| {
        let id = counter.get() + 1;
        counter.set(id);
        id
    })
}

pub fn as_task(value: &Value) -> Option<Rc<TaskCell>> {
    let Value::Foreign(foreign) = value else { return None };
    if foreign.kind != "Task" {
        return None;
    }
    foreign.downcast::<Rc<TaskCell>>().cloned()
}

pub fn as_scope(value: &Value) -> Option<Rc<ScopeCell>> {
    let Value::Foreign(foreign) = value else { return None };
    if foreign.kind != "Scope" {
        return None;
    }
    foreign.downcast::<Rc<ScopeCell>>().cloned()
}

fn task_value(cell: Rc<TaskCell>) -> Value {
    Foreign::wrap("Task", cell)
}

/// Build a task for a call that has not run yet.
///
/// This is what calling an `async fn` produces. The task is attached to the
/// innermost open scope, so a scope always knows about the work started under
/// it — the guarantee in specification 15.2 that a task never silently outlives
/// the operation that created it.
pub fn defer(function: Value, args: Vec<Arg>, loc: Loc) -> Value {
    let label = match &function {
        Value::Fn(callable) => callable.name.clone(),
        other => other.type_name(),
    };
    let cell = Rc::new(TaskCell {
        label,
        state: RefCell::new(TaskState::Pending { function, args, loc }),
    });
    SCOPES.with(|scopes| {
        if let Some(scope) = scopes.borrow().last() {
            scope.tasks.borrow_mut().push(cell.clone());
        }
    });
    task_value(cell)
}

/// Defer a thunk into a named scope. This is what `spawn` compiles to.
pub fn spawn(scope: &Value, thunk: Value, loc: Loc) -> Outcome {
    let Some(scope) = as_scope(scope) else {
        return Err(Flow::fault(
            Fault::new("spawn-scope", "`spawn` needs a task scope", loc)
                .label(format!("found {}", scope.type_name()))
                .help("spawn inside `(task-scope name ...)`"),
        ));
    };
    let label = match &thunk {
        Value::Fn(callable) => callable.name.clone(),
        other => other.type_name(),
    };
    let cell = Rc::new(TaskCell {
        label,
        state: RefCell::new(TaskState::Pending { function: thunk, args: Vec::new(), loc }),
    });
    scope.tasks.borrow_mut().push(cell.clone());
    Ok(task_value(cell))
}

// korben-5wu
/// Every task in every open scope, innermost scope first.
fn open_tasks() -> Vec<Rc<TaskCell>> {
    SCOPES.with(|scopes| {
        let mut all = Vec::new();
        for scope in scopes.borrow().iter().rev() {
            all.extend(scope.tasks.borrow().iter().cloned());
        }
        all
    })
}

fn is_runnable(task: &Rc<TaskCell>) -> bool {
    matches!(&*task.state.borrow(), TaskState::Pending { .. } | TaskState::Suspended(_))
}

/// Resume one task that can still run, round-robin from where the last round
/// left off.
///
/// This is what lets an operation that would block let something else make
/// progress instead of stalling. `None` means no task anywhere can be resumed
/// -- which is not the same as a deadlock, since the tasks that exist may all
/// be parked on a socket that is about to become ready.
pub(crate) fn drive_one() -> Option<Result<(), Flow>> {
    let tasks = open_tasks();
    if tasks.is_empty() {
        return None;
    }
    let start = CURSOR.with(|cursor| cursor.get());
    for step in 0..tasks.len() {
        let at = (start + step) % tasks.len();
        if is_runnable(&tasks[at]) {
            CURSOR.with(|cursor| cursor.set((at + 1) % tasks.len()));
            return Some(resume_task(&tasks[at]).map(|_| ()));
        }
    }
    None
}

// korben-5wu
/// What a resume produced.
enum Step {
    /// The task finished, one way or another.
    Settled(Outcome),
    /// The task parked partway through and is waiting for something.
    Parked,
}

/// Resume `task` once: start it if it has not started, continue it if it has.
///
/// The task's state is left `Running` for the duration, so a task that reaches
/// itself finds a state it can recognise rather than its own suspended stack.
///
/// No `Caller` argument: a task owns its handle on the host from the moment it
/// is built, because its stack outlives the call that built it.
fn resume_task(task: &Rc<TaskCell>) -> Outcome {
    match step_task(task)? {
        Step::Settled(outcome) => outcome,
        // A parked task has no value yet. Whoever wanted one is responsible
        // for coming back; `run` is the loop that does.
        Step::Parked => Ok(Value::Nil),
    }
}

fn step_task(task: &Rc<TaskCell>) -> Result<Step, Flow> {
    // The borrow must be released before the task runs: its body reaches back
    // into the scope stack, and into this very task when it inspects itself.
    let started = {
        let mut state = task.state.borrow_mut();
        match &*state {
            TaskState::Done(value) => return Ok(Step::Settled(Ok(value.clone()))),
            TaskState::Failed(fault) => return Ok(Step::Settled(Err(Flow::Panic(fault.clone())))),
            TaskState::Cancelled => return Ok(Step::Settled(Ok(cancelled_value()))),
            TaskState::Running => {
                return Err(Flow::fault(
                    Fault::new(
                        "task-deadlock",
                        format!("`{}` is waiting on itself", task.label),
                        Loc::NONE,
                    )
                    .note("the task is already running further down this stack")
                    .help("await the task from outside it, or restructure the dependency"),
                ))
            }
            TaskState::Suspended(_) => {
                let TaskState::Suspended(stack) =
                    std::mem::replace(&mut *state, TaskState::Running)
                else {
                    unreachable!("checked above")
                };
                stack
            }
            TaskState::Pending { .. } => {
                let TaskState::Pending { function, args, loc } =
                    std::mem::replace(&mut *state, TaskState::Running)
                else {
                    unreachable!("checked above")
                };
                let Some(host) = host() else {
                    return Err(Flow::fault(
                        Fault::new(
                            "task-host",
                            "no host is installed for tasks to run against",
                            loc,
                        )
                        .note("the program entry point installs it before anything spawns"),
                    ));
                };
                // The body owns its handle on the host. It has to: this stack
                // outlives the call that built it, so it cannot borrow.
                Box::new(Coroutine::new(move |yielder: &Yielder<(), ()>, ()| {
                    YIELDER.with(|slot| slot.set(yielder as *const _));
                    // `apply_now` runs the call rather than deferring it
                    // again, which is what makes an async function's body
                    // execute exactly once.
                    let mut outcome = apply_now(&*host, &function, args, loc);
                    // A thunk that itself called an async function produces
                    // another task; awaiting a task means awaiting through to
                    // a value.
                    while let Some(inner) = outcome.as_ref().ok().and_then(as_task) {
                        outcome = run(&inner);
                    }
                    outcome
                }))
            }
        }
    };

    let mut stack = started;
    let outer = YIELDER.with(|slot| slot.get());
    let resumed = stack.resume(());
    // Back on the resumer's stack, so the resumer's yielder is current again.
    YIELDER.with(|slot| slot.set(outer));

    match resumed {
        CoroutineResult::Yield(()) => {
            *task.state.borrow_mut() = TaskState::Suspended(stack);
            Ok(Step::Parked)
        }
        CoroutineResult::Return(outcome) => {
            note_progress();
            Ok(Step::Settled(settle(task, outcome)))
        }
    }
}

/// Record how a task ended and hand the outcome on.
fn settle(task: &Rc<TaskCell>, outcome: Outcome) -> Outcome {
    match outcome {
        Ok(value) => {
            *task.state.borrow_mut() = TaskState::Done(value.clone());
            Ok(value)
        }
        Err(Flow::Panic(fault)) => {
            *task.state.borrow_mut() = TaskState::Failed(fault.clone());
            Err(Flow::Panic(fault))
        }
        Err(other) => {
            // Non-local control flow escaping a task has nowhere to go.
            *task.state.borrow_mut() = TaskState::Failed(Box::new(Fault::new(
                "task-control-flow",
                format!("`{}` used control flow that escaped the task", task.label),
                Loc::NONE,
            )));
            Err(other)
        }
    }
}

// korben-8h8
/// Park the current task, with no fallback. `net` calls this once it has
/// registered the descriptor that will wake the task, so there is nothing to
/// decide here.
pub(crate) fn park_now() -> bool {
    park()
}

// korben-5wu
/// Wait for something else to change, from an operation that would block.
///
/// Inside a task this parks it where it stands, which is the whole point of
/// this bead: the task keeps its frames and its values, and whoever resumed it
/// gets a turn. At the top level -- `main`, which is not a task -- there is no
/// stack to park, so the only way to make progress is to run somebody else
/// here and now.
///
/// `false` means nothing can move: the caller turns that into the diagnostic
/// that fits what it was waiting for.
pub(crate) fn wait() -> bool {
    if STUCK.with(|flag| flag.get()) {
        // The final pass: the scheduler already knows nothing can move, so
        // say so here rather than looking again.
        return false;
    }
    if park() {
        return true;
    }
    drive_a_round()
}

// korben-5wu
/// Give every runnable task one turn, stopping as soon as anything moves.
///
/// The bound is what makes this terminate. A suspended task is always
/// *runnable* -- resuming it is always possible, it just may park again
/// immediately -- so a loop that ran until `drive_one` found nothing would
/// never end. A full round that changes nothing is the evidence that nothing
/// can change, which is the only honest definition of deadlock here.
fn drive_a_round() -> bool {
    let before = progress();
    for _ in 0..open_tasks().len() {
        match drive_one() {
            Some(Ok(())) => {}
            // A task failed. That is progress: somebody is no longer waiting.
            Some(Err(_)) => return true,
            None => break,
        }
        if progress() != before {
            return true;
        }
    }
    if progress() != before {
        return true;
    }
    // korben-8h8
    // No task can move. That is only a deadlock if nothing outside the
    // scheduler can move either -- and a task parked on a socket is waiting
    // for a peer, which the scheduler cannot see. Ask the sockets before
    // concluding anything.
    if crate::net::wait_for_readiness() {
        note_progress();
        return true;
    }
    false
}

// korben-5wu
/// Run a task to completion, letting everything else run while it waits.
///
/// The order here is the point of this bead. When the task parks, the first
/// thing tried is parking *ourselves*: if this code is running inside another
/// task -- which it is whenever one task awaits another, and whenever `spawn`
/// wraps work in a task of its own -- then giving our turn back is what lets
/// the scheduler treat us as suspended rather than as running. Driving other
/// tasks from inside our own frames is the re-entrancy this replaces, and it
/// is what made a waiting task look busy.
///
/// Only at the top level, where `main` has no stack to park, does this run
/// somebody else itself. A round of that which changes nothing is a deadlock.
fn run(task: &Rc<TaskCell>) -> Outcome {
    loop {
        match step_task(task)? {
            Step::Settled(outcome) => return outcome,
            Step::Parked => {}
        }

        if park() {
            continue;
        }

        if drive_a_round() {
            continue;
        }

        // Nothing anywhere can move. Resume the task one more time with
        // parking refused, so whatever it is waiting on gets to report the
        // failure itself: `recv` knows it is a channel and what would fix it,
        // and this loop does not.
        let previous = STUCK.with(|flag| flag.replace(true));
        let last = step_task(task);
        STUCK.with(|flag| flag.set(previous));
        return match last? {
            Step::Settled(outcome) => outcome,
            // It parked again even with parking refused, so it is waiting on
            // another task rather than on an operation that can speak for
            // itself. This is the only case left for a generic report.
            Step::Parked => Err(Flow::fault(
                Fault::new(
                    "task-deadlock",
                    format!("`{}` cannot make progress", task.label),
                    Loc::NONE,
                )
                .note("it is waiting, and no other task can run to satisfy it")
                .help("check for a cycle between tasks, or a channel nobody sends on"),
            )),
        };
    }
}

/// The value a cancelled task reports.
fn cancelled_value() -> Value {
    Value::err(Value::variant("Cancelled", "Cancelled", Vec::new()))
}

/// Await a value. Anything that is not a task is already its own result.
pub fn await_value(value: &Value, _loc: Loc) -> Outcome {
    match as_task(value) {
        Some(task) => run(&task),
        None => Ok(value.clone()),
    }
}

/// Await several tasks, short-circuiting on the first failure.
///
/// A task whose value is an `Err` is a failure too, which is what makes
/// `(join-all tasks)?` read the way specification 15.2 writes it.
pub fn join_all(tasks: &Value, loc: Loc) -> Outcome {
    let Value::Vector(items) = tasks else {
        return Err(Flow::fault(
            Fault::new("join-all", "`join-all` needs a vector of tasks", loc)
                .label(format!("found {}", tasks.type_name())),
        ));
    };
    let mut values = Vec::with_capacity(items.len());
    for item in items.iter() {
        let value = await_value(item, loc)?;
        if let Value::Variant(variant) = &value {
            if &*variant.variant == "Err" {
                return Ok(value);
            }
            if &*variant.variant == "Ok" {
                values.push(variant.fields.first().map(|(_, v)| v.clone()).unwrap_or(Value::Nil));
                continue;
            }
        }
        values.push(value);
    }
    Ok(Value::ok(Value::vector(values)))
}

// -------------------------------------------------------------------- scopes

/// Open a task scope.
pub fn enter_scope(label: &str) -> Value {
    let scope = Rc::new(ScopeCell {
        label: format!("{label}#{}", next_id()),
        cancelled: Cell::new(false),
        tasks: RefCell::new(Vec::new()),
    });
    SCOPES.with(|scopes| scopes.borrow_mut().push(scope.clone()));
    Foreign::wrap("Scope", scope)
}

/// Close a task scope.
///
/// On the ordinary path every child is joined, so nothing outlives the scope
/// and a child's failure reaches the code that started it. When the body is
/// already failing, children are cancelled instead.
pub fn exit_scope(scope: &Value, failing: bool) -> Result<(), Flow> {
    let cell = as_scope(scope);
    SCOPES.with(|scopes| {
        scopes.borrow_mut().pop();
    });
    let Some(cell) = cell else { return Ok(()) };
    let tasks: Vec<Rc<TaskCell>> = cell.tasks.borrow().clone();

    if failing || cell.cancelled.get() {
        for task in &tasks {
            let mut state = task.state.borrow_mut();
            if matches!(&*state, TaskState::Pending { .. }) {
                *state = TaskState::Cancelled;
            }
        }
        return Ok(());
    }

    let mut first: Option<Flow> = None;
    for task in &tasks {
        match run(task) {
            Ok(_) => {}
            Err(flow) => {
                if first.is_none() {
                    first = Some(flow);
                }
            }
        }
    }
    match first {
        Some(flow) => Err(flow),
        None => Ok(()),
    }
}

/// Ask every unstarted task in a scope to stop. Cancellation is cooperative:
/// a task already running keeps going, and can check `cancelled?`.
pub fn cancel_scope(scope: &Value) -> Outcome {
    let Some(cell) = as_scope(scope) else { return Ok(Value::Nil) };
    cell.cancelled.set(true);
    for task in cell.tasks.borrow().iter() {
        let mut state = task.state.borrow_mut();
        if matches!(&*state, TaskState::Pending { .. }) {
            *state = TaskState::Cancelled;
        }
    }
    Ok(Value::Nil)
}

pub fn scope_cancelled(scope: &Value) -> Outcome {
    Ok(Value::Bool(as_scope(scope).map(|cell| cell.cancelled.get()).unwrap_or(false)))
}

pub fn task_state_name(task: &Value) -> Outcome {
    let Some(cell) = as_task(task) else { return Ok(Value::keyword("unknown")) };
    let name = match &*cell.state.borrow() {
        TaskState::Pending { .. } => "pending",
        // korben-5wu
        TaskState::Suspended(_) => "suspended",
        TaskState::Running => "running",
        TaskState::Done(_) => "done",
        TaskState::Failed(_) => "failed",
        TaskState::Cancelled => "cancelled",
    };
    Ok(Value::keyword(name))
}

pub fn cancel_task(task: &Value) -> Outcome {
    if let Some(cell) = as_task(task) {
        let mut state = cell.state.borrow_mut();
        if matches!(&*state, TaskState::Pending { .. }) {
            *state = TaskState::Cancelled;
        }
    }
    Ok(Value::Nil)
}

// ------------------------------------------------------------------ channels

pub struct ChannelCell {
    /// `None` for an unbounded channel.
    capacity: Option<usize>,
    queue: RefCell<VecDeque<Value>>,
    closed: Cell<bool>,
}

fn as_channel(value: &Value, kind: &str) -> Option<Rc<ChannelCell>> {
    let Value::Foreign(foreign) = value else { return None };
    if foreign.kind != kind {
        return None;
    }
    foreign.downcast::<Rc<ChannelCell>>().cloned()
}

/// A channel is a sender and a receiver over shared state.
pub fn channel(capacity: Option<usize>) -> Value {
    let cell = Rc::new(ChannelCell {
        capacity,
        queue: RefCell::new(VecDeque::new()),
        closed: Cell::new(false),
    });
    Value::vector(vec![Foreign::wrap("Sender", cell.clone()), Foreign::wrap("Receiver", cell)])
}

fn channel_error(message: &str) -> Value {
    Value::err(Value::variant("ChannelError", "Closed", vec![("message", Value::str(message))]))
}

/// Send a value, driving other tasks if the channel is full.
pub fn send(sender: &Value, value: Value, loc: Loc) -> Outcome {
    let Some(cell) = as_channel(sender, "Sender") else {
        return Err(wrong("send", "a Sender", sender, loc));
    };
    loop {
        if cell.closed.get() {
            return Ok(channel_error("the channel is closed"));
        }
        let full = match cell.capacity {
            Some(capacity) => cell.queue.borrow().len() >= capacity,
            None => false,
        };
        if !full {
            cell.queue.borrow_mut().push_back(value);
            note_progress();
            return Ok(Value::ok(Value::Nil));
        }
        // korben-5wu: wait for room. Inside a task this parks where it stands
        // and a receiver gets a turn; at the top level there is no stack to
        // park, so `main` runs a receiver itself.
        if !wait() {
            return Err(Flow::fault(
                Fault::new("channel-deadlock", "the channel is full and nothing can drain it", loc)
                    .note("no other task can run to receive from it")
                    .help("give the channel more capacity, or receive before sending"),
            ));
        }
    }
}

/// Receive a value, driving other tasks if the channel is empty.
///
/// Returns `None` once the channel is closed and drained.
pub fn recv(receiver: &Value, loc: Loc) -> Outcome {
    let Some(cell) = as_channel(receiver, "Receiver") else {
        return Err(wrong("recv", "a Receiver", receiver, loc));
    };
    loop {
        if let Some(value) = cell.queue.borrow_mut().pop_front() {
            note_progress();
            return Ok(Value::some(value));
        }
        if cell.closed.get() {
            return Ok(Value::none());
        }
        // korben-5wu: wait for a value, the same way `send` waits for room.
        if !wait() {
            return Err(Flow::fault(
                Fault::new("channel-deadlock", "the channel is empty and nothing can fill it", loc)
                    .note("no other task can run to send on it, and the channel is still open")
                    .help("close the channel when sending is finished, or send before receiving"),
            ));
        }
    }
}

/// Take a value if one is already there, without driving anything.
pub fn try_recv(receiver: &Value, loc: Loc) -> Outcome {
    let Some(cell) = as_channel(receiver, "Receiver") else {
        return Err(wrong("try-recv", "a Receiver", receiver, loc));
    };
    let value = cell.queue.borrow_mut().pop_front();
    Ok(match value {
        Some(value) => Value::some(value),
        None => Value::none(),
    })
}

pub fn close_channel(value: &Value) -> Outcome {
    for kind in ["Sender", "Receiver"] {
        if let Some(cell) = as_channel(value, kind) {
            cell.closed.set(true);
        }
    }
    Ok(Value::Nil)
}

pub fn channel_len(value: &Value) -> Outcome {
    for kind in ["Sender", "Receiver"] {
        if let Some(cell) = as_channel(value, kind) {
            return Ok(Value::Int(cell.queue.borrow().len() as i64));
        }
    }
    Ok(Value::Int(0))
}

fn wrong(name: &str, expected: &str, got: &Value, loc: Loc) -> Flow {
    Flow::fault(
        Fault::new("type-error", format!("`{name}` expected {expected}"), loc)
            .label(format!("found {}", got.type_name())),
    )
}
