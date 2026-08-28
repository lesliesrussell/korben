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

// ------------------------------------------------------------- readiness

// korben-ae2
// A server has to ask which of several sockets is ready at once. That is the
// one thing the standard library cannot express, so `poll(2)` is declared here
// the way `ffi.rs` declares `dlopen` and `dlsym`.
//
// Waiting on a single socket is not a substitute. It is what the driving
// approach did, and it is why that approach failed: waiting on one connection
// stops the server accepting another.

#[cfg(unix)]
#[repr(C)]
#[derive(Clone, Copy)]
struct PollFd {
    fd: std::os::raw::c_int,
    events: std::os::raw::c_short,
    revents: std::os::raw::c_short,
}

/// `nfds_t` is not the same width everywhere, and it is an argument, so it has
/// to match.
#[cfg(all(unix, target_os = "macos"))]
type NfdsT = std::os::raw::c_uint;
#[cfg(all(unix, not(target_os = "macos")))]
type NfdsT = std::os::raw::c_ulong;

/// There is data to read, or the peer has closed.
#[cfg(unix)]
const POLLIN: std::os::raw::c_short = 0x0001;

#[cfg(unix)]
extern "C" {
    fn poll(fds: *mut PollFd, nfds: NfdsT, timeout: std::os::raw::c_int) -> std::os::raw::c_int;
}

/// Which of `fds` can be read without blocking, waiting up to `timeout_ms`.
///
/// A negative timeout waits indefinitely, which is what an idle server wants.
#[cfg(unix)]
fn readable(fds: &[std::os::raw::c_int], timeout_ms: i32) -> std::io::Result<Vec<bool>> {
    if fds.is_empty() {
        return Ok(Vec::new());
    }
    let mut polled: Vec<PollFd> =
        fds.iter().map(|fd| PollFd { fd: *fd, events: POLLIN, revents: 0 }).collect();
    loop {
        // SAFETY: `polled` is a valid, correctly sized array of `pollfd` for
        // the length passed alongside it, and it outlives the call.
        let count = unsafe { poll(polled.as_mut_ptr(), polled.len() as NfdsT, timeout_ms) };
        if count < 0 {
            let error = std::io::Error::last_os_error();
            // A signal is not a failure; ask again.
            if error.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        return Ok(polled.iter().map(|entry| entry.revents & POLLIN != 0).collect());
    }
}

#[cfg(not(unix))]
fn readable(_fds: &[std::os::raw::c_int], _timeout_ms: i32) -> std::io::Result<Vec<bool>> {
    Err(std::io::Error::new(
        ErrorKind::Unsupported,
        "waiting on several sockets at once is only supported on Unix",
    ))
}

// ------------------------------------------------------------------ pools

// korben-ae2
/// A server's sockets: its listener, and every connection it has accepted.
///
/// One resource owning many sockets, rather than a Korben collection of them,
/// because a resource-bearing value moves -- a connection cannot be taken out
/// of a vector and put back on every pass of a loop. Connections are addressed
/// by id instead, and the protocol state that belongs with them stays in
/// Korben, where the rest of `std.http` is.
pub struct Pool {
    listener: TcpListener,
    connections: Vec<Connected>,
    next: i64,
}

// korben-c6k
/// One accepted connection, and when it last did anything.
struct Connected {
    id: i64,
    stream: TcpStream,
    active: std::time::Instant,
}

// korben-c6k
/// A response is small, and a peer that has not taken one in this long is not
/// going to. Without a bound here one stuck reader holds the whole server,
/// because the write is deliberately the one blocking call in the loop.
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

type PoolHandle = RefCell<Option<Pool>>;

fn as_pool(value: &Value) -> Option<&PoolHandle> {
    let Value::Foreign(foreign) = value else { return None };
    if foreign.kind != "Pool" {
        return None;
    }
    foreign.downcast::<PoolHandle>()
}

/// Bind an address and start a pool on it.
pub fn pool(address: &str) -> Outcome {
    let listener = match TcpListener::bind(address) {
        Ok(listener) => listener,
        Err(error) => return Ok(Value::err(crate::std::io_error(address, &error))),
    };
    if let Err(error) = listener.set_nonblocking(true) {
        return Ok(Value::err(crate::std::io_error(address, &error)));
    }
    Ok(Value::ok(Foreign::wrap(
        "Pool",
        RefCell::new(Some(Pool { listener, connections: Vec::new(), next: 1 })),
    )))
}

/// The ids of every connection with something to read, waiting up to
/// `timeout_ms` for one to appear.
///
/// Connections waiting to be accepted are accepted here and registered, but not
/// reported: a connection that has said nothing is not readable, which is
/// exactly why a silent client cannot hold up the server.
pub fn pool_wait(value: &Value, timeout_ms: i64, loc: Loc) -> Outcome {
    let Some(handle) = as_pool(value) else {
        return Err(wrong("Pool.wait", "a Pool", value, loc));
    };
    let mut borrowed = handle.borrow_mut();
    let Some(pool) = borrowed.as_mut() else { return Ok(closed("this pool")) };

    #[cfg(unix)]
    let mut descriptors = {
        use std::os::unix::io::AsRawFd;
        let mut descriptors = vec![pool.listener.as_raw_fd()];
        descriptors.extend(pool.connections.iter().map(|open| open.stream.as_raw_fd()));
        descriptors
    };
    #[cfg(not(unix))]
    let mut descriptors: Vec<std::os::raw::c_int> = Vec::new();
    let ready = match readable(&descriptors, timeout_ms as i32) {
        Ok(ready) => ready,
        Err(error) => return Ok(Value::err(crate::std::io_error("", &error))),
    };
    descriptors.clear();

    if ready.first().copied().unwrap_or(false) {
        // Take everything the backlog holds, not just one: the listener will
        // not report itself readable again for connections already waiting.
        loop {
            match pool.listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(true);
                    let id = pool.next;
                    pool.next += 1;
                    pool.connections.push(Connected {
                        id,
                        stream,
                        active: std::time::Instant::now(),
                    });
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    }

    // `ready` describes the connections as they were before accepting, so
    // zipping stops at the ones it knows about.
    let ids: Vec<Value> = pool
        .connections
        .iter()
        .zip(ready.iter().skip(1))
        .filter(|(_, readable)| **readable)
        .map(|(open, _)| Value::Int(open.id))
        .collect();
    Ok(Value::ok(Value::vector(ids)))
}

/// Read whatever has arrived on one connection.
///
/// `None` means the peer is finished; an empty string means nothing new.
pub fn pool_read(value: &Value, id: i64, loc: Loc) -> Outcome {
    let Some(handle) = as_pool(value) else {
        return Err(wrong("Pool.read", "a Pool", value, loc));
    };
    let mut borrowed = handle.borrow_mut();
    let Some(pool) = borrowed.as_mut() else { return Ok(closed("this pool")) };
    let Some(open) = pool.connections.iter_mut().find(|open| open.id == id) else {
        return Ok(closed("this connection"));
    };
    // korben-c6k
    // Reading is activity, whether or not it completes a request: a client
    // sending a large body slowly is making progress, not stalling.
    open.active = std::time::Instant::now();
    let mut buffer = vec![0u8; READ_CHUNK];
    match open.stream.read(&mut buffer) {
        Ok(0) => Ok(Value::ok(Value::none())),
        Ok(count) => {
            buffer.truncate(count);
            Ok(Value::ok(Value::some(Value::str(String::from_utf8_lossy(&buffer).to_string()))))
        }
        Err(error) if error.kind() == ErrorKind::WouldBlock => {
            Ok(Value::ok(Value::some(Value::str(""))))
        }
        Err(error) => Ok(Value::err(crate::std::io_error("", &error))),
    }
}

/// Write a whole response to one connection.
///
/// This one call may block, and deliberately: a response is small, and a peer
/// that will not read it is a different problem from one that will not write.
pub fn pool_write(value: &Value, id: i64, text: &str, loc: Loc) -> Outcome {
    let Some(handle) = as_pool(value) else {
        return Err(wrong("Pool.write", "a Pool", value, loc));
    };
    let mut borrowed = handle.borrow_mut();
    let Some(pool) = borrowed.as_mut() else { return Ok(closed("this pool")) };
    let Some(open) = pool.connections.iter_mut().find(|open| open.id == id) else {
        return Ok(closed("this connection"));
    };
    let _ = open.stream.set_nonblocking(false);
    // korben-c6k
    let _ = open.stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let outcome = match open.stream.write_all(text.as_bytes()).and_then(|()| open.stream.flush()) {
        Ok(()) => Value::ok(Value::Nil),
        Err(error) => Value::err(crate::std::io_error("", &error)),
    };
    let _ = open.stream.set_nonblocking(true);
    Ok(outcome)
}

/// Close one connection and forget it.
pub fn pool_drop(value: &Value, id: i64, loc: Loc) -> Outcome {
    let Some(handle) = as_pool(value) else {
        return Err(wrong("Pool.close-connection", "a Pool", value, loc));
    };
    let mut borrowed = handle.borrow_mut();
    let Some(pool) = borrowed.as_mut() else { return Ok(closed("this pool")) };
    pool.connections.retain(|open| open.id != id);
    Ok(Value::ok(Value::Nil))
}

/// Release the listener and every connection still open.
pub fn pool_close(value: &Value, loc: Loc) -> Outcome {
    let Some(handle) = as_pool(value) else {
        return Err(wrong("Pool.close", "a Pool", value, loc));
    };
    handle.borrow_mut().take();
    Ok(Value::Nil)
}

// korben-c6k
/// Close every connection that has done nothing for `idle_ms`, and say which.
///
/// A connection that never speaks is invisible to the code above: it is never
/// ready, so it is never reported, so nothing up there can decide to give up on
/// it. The pool is the only place that knows it exists.
pub fn pool_evict(value: &Value, idle_ms: i64, loc: Loc) -> Outcome {
    let Some(handle) = as_pool(value) else {
        return Err(wrong("Pool.evict", "a Pool", value, loc));
    };
    let mut borrowed = handle.borrow_mut();
    let Some(pool) = borrowed.as_mut() else { return Ok(closed("this pool")) };
    let limit = std::time::Duration::from_millis(idle_ms.max(0) as u64);
    let now = std::time::Instant::now();
    let mut dropped = Vec::new();
    pool.connections.retain(|open| {
        if now.duration_since(open.active) >= limit {
            dropped.push(Value::Int(open.id));
            false
        } else {
            true
        }
    });
    Ok(Value::ok(Value::vector(dropped)))
}

/// The address the pool is listening on, which is how a test finds the port
/// when it asked for zero.
pub fn pool_address(value: &Value, loc: Loc) -> Outcome {
    let Some(handle) = as_pool(value) else {
        return Err(wrong("Pool.address", "a Pool", value, loc));
    };
    let borrowed = handle.borrow();
    let Some(pool) = borrowed.as_ref() else { return Ok(closed("this pool")) };
    Ok(match pool.listener.local_addr() {
        Ok(address) => Value::ok(Value::str(address.to_string())),
        Err(error) => Value::err(crate::std::io_error("", &error)),
    })
}
