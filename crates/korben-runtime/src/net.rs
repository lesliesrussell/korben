//! TCP sockets.
//!
//! Listeners and connections are resource-bearing handles: they own an
//! operating-system resource, they implement the runtime side of `Drop`, and
//! `with` releases them on every exit path. Socket operations are methods on
//! the receiver, so they borrow it rather than consuming it, which is what lets
//! an accept loop keep using its listener.
//!
//! Reads and writes block. The async runtime schedules cooperatively and a
//! started task cannot suspend, so a blocking read stalls the whole scheduler
//! and a server handles one request at a time on the accepting task. Handling
//! connections concurrently needs non-blocking sockets driven by the scheduler,
//! which these operations do not yet use.

// korben-7zt

use crate::loc::Loc;
use crate::value::{Flow, Foreign, Outcome, Value};
use std::cell::RefCell;
use std::io::{Read, Write};
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

pub fn accept(value: &Value, loc: Loc) -> Outcome {
    let Some(handle) = as_listener(value) else {
        return Err(wrong("Listener.accept", "a Listener", value, loc));
    };
    let borrowed = handle.borrow();
    let Some(listener) = borrowed.as_ref() else { return Ok(closed("this listener")) };
    Ok(match listener.accept() {
        Ok((stream, _)) => Value::ok(connection_value(stream)),
        Err(error) => Value::err(crate::std::io_error("", &error)),
    })
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
pub fn read(value: &Value, loc: Loc) -> Outcome {
    let Some(handle) = as_connection(value) else {
        return Err(wrong("Connection.read", "a Connection", value, loc));
    };
    let mut borrowed = handle.borrow_mut();
    let Some(stream) = borrowed.as_mut() else { return Ok(closed("this connection")) };
    let mut buffer = vec![0u8; READ_CHUNK];
    Ok(match stream.read(&mut buffer) {
        Ok(count) => {
            buffer.truncate(count);
            Value::ok(Value::str(String::from_utf8_lossy(&buffer).to_string()))
        }
        Err(error) => Value::err(crate::std::io_error("", &error)),
    })
}

pub fn write(value: &Value, text: &str, loc: Loc) -> Outcome {
    let Some(handle) = as_connection(value) else {
        return Err(wrong("Connection.write", "a Connection", value, loc));
    };
    let mut borrowed = handle.borrow_mut();
    let Some(stream) = borrowed.as_mut() else { return Ok(closed("this connection")) };
    Ok(match stream.write_all(text.as_bytes()).and_then(|()| stream.flush()) {
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
