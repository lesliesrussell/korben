//! TCP sockets.
//!
//! Listeners and connections are resource-bearing handles: they own an
//! operating-system resource, they implement the runtime side of `Drop`, and
//! `with` releases them on every exit path. Socket operations are methods on
//! the receiver, so they borrow it rather than consuming it, which is what lets
//! an accept loop keep using its listener.
//!
//! An operation that would block does not stall the scheduler. Sockets are
//! non-blocking, and `accept`, `read`, and `write` take the shape channels
//! already have in `task.rs`: attempt the operation, and if it would block, run
//! another ready task and try again.
//!
//! One thing differs from a channel. For a channel, nothing left to drive is a
//! deadlock, because only another task can fill it. For a socket it is not --
//! the operating system may deliver bytes later -- so with nothing else to run,
//! the operation waits on the socket instead, in blocking mode. That is exactly
//! right when there is no other work, and it needs no `poll` and no dependency.
//!
//! A started task still cannot suspend: a blocked one keeps its stack frame
//! while it drives the next. Concurrency is therefore bounded by the recursion
//! limit rather than by memory.

// korben-7zt
// korben-48e

use crate::loc::Loc;
use crate::value::{Caller, Flow, Foreign, Outcome, Value};
use std::cell::RefCell;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};

/// The largest chunk a single read returns.
const READ_CHUNK: usize = 64 * 1024;

type ListenerHandle = RefCell<Option<TcpListener>>;
type ConnectionHandle = RefCell<Option<TcpStream>>;

pub fn listener_value(listener: TcpListener) -> Value {
    Foreign::wrap("Listener", RefCell::new(Some(listener)))
}

pub fn connection_value(stream: TcpStream) -> Value {
    Foreign::wrap("Connection", RefCell::new(Some(stream)))
}

fn as_listener(value: &Value) -> Option<&ListenerHandle> {
    let Value::Foreign(foreign) = value else { return None };
    if foreign.kind != "Listener" {
        return None;
    }
    foreign.downcast::<ListenerHandle>()
}

fn as_connection(value: &Value) -> Option<&ConnectionHandle> {
    let Value::Foreign(foreign) = value else { return None };
    if foreign.kind != "Connection" {
        return None;
    }
    foreign.downcast::<ConnectionHandle>()
}

/// A closed handle is a recoverable error, not a fault: `with` may already have
/// released it, and a program should be able to notice rather than crash.
fn closed(what: &str) -> Value {
    Value::err(Value::record(
        Some("IoError"),
        vec![
            ("path", Value::str("")),
            ("kind", Value::keyword("closed")),
            ("message", Value::str(format!("{what} was used after it was closed"))),
        ],
    ))
}

// korben-48e
/// What an operation should do next when it would block.
enum Step {
    /// Something else ran; try again without waiting.
    Retry,
    /// Nothing else can run, so wait on the socket itself.
    Wait,
}

/// Let another ready task run, or report that none can.
fn yield_or_wait(caller: &mut dyn Caller) -> Result<Step, Flow> {
    match crate::task::drive_one(caller) {
        Some(result) => {
            result?;
            Ok(Step::Retry)
        }
        None => Ok(Step::Wait),
    }
}

pub fn listen(address: &str) -> Outcome {
    Ok(match TcpListener::bind(address) {
        Ok(listener) => Value::ok(listener_value(listener)),
        Err(error) => Value::err(crate::std::io_error(address, &error)),
    })
}

pub fn connect(address: &str) -> Outcome {
    Ok(match TcpStream::connect(address) {
        Ok(stream) => Value::ok(connection_value(stream)),
        Err(error) => Value::err(crate::std::io_error(address, &error)),
    })
}

// korben-48e
pub fn accept(caller: &mut dyn Caller, value: &Value, loc: Loc) -> Outcome {
    let Some(handle) = as_listener(value) else {
        return Err(wrong("Listener.accept", "a Listener", value, loc));
    };
    let mut waiting = false;
    loop {
        // The handle is borrowed only for the attempt. Driving another task
        // runs arbitrary Korben code, which may reach this same listener.
        let attempt = {
            let borrowed = handle.borrow();
            let Some(listener) = borrowed.as_ref() else { return Ok(closed("this listener")) };
            let _ = listener.set_nonblocking(!waiting);
            listener.accept()
        };
        match attempt {
            Ok((stream, _)) => return Ok(Value::ok(connection_value(stream))),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock && !waiting => {
                waiting = matches!(yield_or_wait(caller)?, Step::Wait);
            }
            Err(error) => return Ok(Value::err(crate::std::io_error("", &error))),
        }
    }
}

pub fn local_address(value: &Value, loc: Loc) -> Outcome {
    let Some(handle) = as_listener(value) else {
        return Err(wrong("Listener.address", "a Listener", value, loc));
    };
    let borrowed = handle.borrow();
    let Some(listener) = borrowed.as_ref() else { return Ok(closed("this listener")) };
    Ok(match listener.local_addr() {
        Ok(address) => Value::ok(Value::str(address.to_string())),
        Err(error) => Value::err(crate::std::io_error("", &error)),
    })
}

pub fn peer_address(value: &Value, loc: Loc) -> Outcome {
    let Some(handle) = as_connection(value) else {
        return Err(wrong("Connection.peer", "a Connection", value, loc));
    };
    let borrowed = handle.borrow();
    let Some(stream) = borrowed.as_ref() else { return Ok(closed("this connection")) };
    Ok(match stream.peer_addr() {
        Ok(address) => Value::ok(Value::str(address.to_string())),
        Err(error) => Value::err(crate::std::io_error("", &error)),
    })
}

/// Read whatever has arrived. An empty string means the peer is done.
// korben-48e
pub fn read(caller: &mut dyn Caller, value: &Value, loc: Loc) -> Outcome {
    let Some(handle) = as_connection(value) else {
        return Err(wrong("Connection.read", "a Connection", value, loc));
    };
    let mut buffer = vec![0u8; READ_CHUNK];
    let mut waiting = false;
    loop {
        let attempt = {
            let mut borrowed = handle.borrow_mut();
            let Some(stream) = borrowed.as_mut() else { return Ok(closed("this connection")) };
            let _ = stream.set_nonblocking(!waiting);
            stream.read(&mut buffer)
        };
        match attempt {
            Ok(count) => {
                buffer.truncate(count);
                return Ok(Value::ok(Value::str(String::from_utf8_lossy(&buffer).to_string())));
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock && !waiting => {
                waiting = matches!(yield_or_wait(caller)?, Step::Wait);
            }
            Err(error) => return Ok(Value::err(crate::std::io_error("", &error))),
        }
    }
}

// korben-48e
pub fn write(caller: &mut dyn Caller, value: &Value, text: &str, loc: Loc) -> Outcome {
    let Some(handle) = as_connection(value) else {
        return Err(wrong("Connection.write", "a Connection", value, loc));
    };
    // `write_all` cannot be used on a non-blocking socket: it reports that it
    // would block without saying how much it had already sent, so the offset
    // is tracked here instead.
    let bytes = text.as_bytes();
    let mut sent = 0usize;
    let mut waiting = false;
    while sent < bytes.len() {
        let attempt = {
            let mut borrowed = handle.borrow_mut();
            let Some(stream) = borrowed.as_mut() else { return Ok(closed("this connection")) };
            let _ = stream.set_nonblocking(!waiting);
            stream.write(&bytes[sent..])
        };
        match attempt {
            Ok(0) => {
                return Ok(Value::err(crate::std::io_error(
                    "",
                    &std::io::Error::from(ErrorKind::WriteZero),
                )))
            }
            Ok(count) => {
                sent += count;
                waiting = false;
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock && !waiting => {
                waiting = matches!(yield_or_wait(caller)?, Step::Wait);
            }
            Err(error) => return Ok(Value::err(crate::std::io_error("", &error))),
        }
    }
    let mut borrowed = handle.borrow_mut();
    let Some(stream) = borrowed.as_mut() else { return Ok(closed("this connection")) };
    Ok(match stream.flush() {
        Ok(()) => Value::ok(Value::Nil),
        Err(error) => Value::err(crate::std::io_error("", &error)),
    })
}

/// Release a handle. Closing twice is not an error, because `with` closes on
/// every exit path and a program may also close explicitly.
pub fn close(value: &Value) -> Outcome {
    if let Some(handle) = as_listener(value) {
        *handle.borrow_mut() = None;
    }
    if let Some(handle) = as_connection(value) {
        // Shutting the socket down tells the peer the response is complete.
        if let Some(stream) = handle.borrow().as_ref() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        *handle.borrow_mut() = None;
    }
    Ok(Value::Nil)
}

pub fn is_closed(value: &Value) -> Outcome {
    let closed = as_listener(value)
        .map(|handle| handle.borrow().is_none())
        .or_else(|| as_connection(value).map(|handle| handle.borrow().is_none()))
        .unwrap_or(true);
    Ok(Value::Bool(closed))
}

fn wrong(name: &str, expected: &str, got: &Value, loc: Loc) -> Flow {
    Flow::fault(
        crate::loc::Fault::new("type-error", format!("`{name}` expected {expected}"), loc)
            .label(format!("found {}", got.type_name())),
    )
}
