//! The async runtime: tasks, scopes, cancellation, and channels.
//!
//! Korben values are reference counted, so they belong to one thread. The
//! scheduler here is therefore cooperative and single-threaded: tasks are
//! concurrent, not simultaneous.
//!
//! Calling an `async fn` yields a [`Task`] instead of running it. A task runs to
//! completion once started; an operation that would block — awaiting, receiving
//! from an empty channel, sending to a full one — instead *drives* other ready
//! tasks re-entrantly and then tries again. That makes ordinary producer and
//! consumer patterns work, and turns a genuine cycle into a reported deadlock
//! rather than a hang.
//!
//! Because a started task cannot suspend, a task blocked on another task that
//! is itself already running is a deadlock. The scheduler says so, with both
//! tasks named, rather than waiting forever.

// korben-du1

use crate::apply::apply_now;
use crate::loc::{Fault, Loc};
use crate::value::{Arg, Caller, Flow, Foreign, Outcome, Value};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

// --------------------------------------------------------------------- tasks

pub enum TaskState {
    /// Not started: the call it will make, once something needs its value.
    Pending {
        function: Value,
        args: Vec<Arg>,
        loc: Loc,
    },
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

/// Run one ready task from the open scopes, if any is ready.
///
/// This is what lets an operation that would block let something else make
/// progress instead of stalling: a channel with nothing in it, and a socket
/// with nothing on it, both come here before giving up.
pub(crate) fn drive_one(caller: &dyn Caller) -> Option<Result<(), Flow>> {
    let ready = SCOPES.with(|scopes| {
        for scope in scopes.borrow().iter().rev() {
            for task in scope.tasks.borrow().iter() {
                if matches!(&*task.state.borrow(), TaskState::Pending { .. }) {
                    return Some(task.clone());
                }
            }
        }
        None
    });
    let task = ready?;
    Some(run(caller, &task).map(|_| ()))
}

/// Run a task to completion and record its outcome.
fn run(caller: &dyn Caller, task: &Rc<TaskCell>) -> Outcome {
    let pending = {
        let mut state = task.state.borrow_mut();
        match &*state {
            TaskState::Done(value) => return Ok(value.clone()),
            TaskState::Failed(fault) => return Err(Flow::Panic(fault.clone())),
            TaskState::Cancelled => return Ok(cancelled_value()),
            TaskState::Running => {
                return Err(Flow::fault(
                    Fault::new(
                        "task-deadlock",
                        format!("`{}` is waiting on itself", task.label),
                        Loc::NONE,
                    )
                    .note("a started task cannot suspend, so it cannot await its own result")
                    .help("await the task from outside it, or restructure the dependency"),
                ))
            }
            TaskState::Pending { .. } => {
                let TaskState::Pending { function, args, loc } =
                    std::mem::replace(&mut *state, TaskState::Running)
                else {
                    unreachable!("checked above")
                };
                (function, args, loc)
            }
        }
    };

    let (function, args, loc) = pending;
    // `apply_now` runs the call rather than deferring it again, which is what
    // makes an async function's body execute exactly once.
    let mut outcome = apply_now(caller, &function, args, loc);
    // A thunk that itself called an async function produces another task;
    // awaiting a task means awaiting through to a value.
    while let Some(inner) = outcome.as_ref().ok().and_then(as_task) {
        outcome = run(caller, &inner);
    }

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
                loc,
            )));
            Err(other)
        }
    }
}

/// The value a cancelled task reports.
fn cancelled_value() -> Value {
    Value::err(Value::variant("Cancelled", "Cancelled", Vec::new()))
}

/// Await a value. Anything that is not a task is already its own result.
pub fn await_value(caller: &dyn Caller, value: &Value, _loc: Loc) -> Outcome {
    match as_task(value) {
        Some(task) => run(caller, &task),
        None => Ok(value.clone()),
    }
}

/// Await several tasks, short-circuiting on the first failure.
///
/// A task whose value is an `Err` is a failure too, which is what makes
/// `(join-all tasks)?` read the way specification 15.2 writes it.
pub fn join_all(caller: &dyn Caller, tasks: &Value, loc: Loc) -> Outcome {
    let Value::Vector(items) = tasks else {
        return Err(Flow::fault(
            Fault::new("join-all", "`join-all` needs a vector of tasks", loc)
                .label(format!("found {}", tasks.type_name())),
        ));
    };
    let mut values = Vec::with_capacity(items.len());
    for item in items.iter() {
        let value = await_value(caller, item, loc)?;
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
pub fn exit_scope(caller: &dyn Caller, scope: &Value, failing: bool) -> Result<(), Flow> {
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
        match run(caller, task) {
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
pub fn send(caller: &dyn Caller, sender: &Value, value: Value, loc: Loc) -> Outcome {
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
            return Ok(Value::ok(Value::Nil));
        }
        // Make room by letting a receiver run.
        match drive_one(caller) {
            Some(result) => result?,
            None => {
                return Err(Flow::fault(
                    Fault::new(
                        "channel-deadlock",
                        "the channel is full and nothing can drain it",
                        loc,
                    )
                    .note("no other task is ready to receive")
                    .help("give the channel more capacity, or receive before sending"),
                ))
            }
        }
    }
}

/// Receive a value, driving other tasks if the channel is empty.
///
/// Returns `None` once the channel is closed and drained.
pub fn recv(caller: &dyn Caller, receiver: &Value, loc: Loc) -> Outcome {
    let Some(cell) = as_channel(receiver, "Receiver") else {
        return Err(wrong("recv", "a Receiver", receiver, loc));
    };
    loop {
        if let Some(value) = cell.queue.borrow_mut().pop_front() {
            return Ok(Value::some(value));
        }
        if cell.closed.get() {
            return Ok(Value::none());
        }
        match drive_one(caller) {
            Some(result) => result?,
            None => {
                return Err(Flow::fault(
                    Fault::new(
                        "channel-deadlock",
                        "the channel is empty and nothing can fill it",
                        loc,
                    )
                    .note("no other task is ready to send, and the channel is still open")
                    .help("close the channel when sending is finished, or send before receiving"),
                ))
            }
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
