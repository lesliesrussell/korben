//! JSON-RPC over the Language Server Protocol's header framing.
//!
//! A message is a `Content-Length` header, a blank line, and exactly that many
//! bytes of JSON. Reading is deliberately byte-oriented: the body length is
//! counted in bytes, and a UTF-8 character may straddle any buffer boundary, so
//! nothing may be decoded as text until the whole body is in hand.

// korben-efd

use std::io::{BufRead, Write};

use crate::json::{parse, Json};

/// One message read from the client.
#[derive(Debug)]
pub struct Message {
    pub id: Option<Json>,
    pub method: String,
    pub params: Json,
}

impl Message {
    /// A request expects a response; a notification does not.
    pub fn is_request(&self) -> bool {
        self.id.is_some()
    }
}

/// Read the next message, or `None` at end of input.
pub fn read(input: &mut impl BufRead) -> Result<Option<Message>, String> {
    let Some(length) = read_headers(input)? else {
        return Ok(None);
    };
    let mut body = vec![0u8; length];
    read_exact(input, &mut body)?;
    let text = String::from_utf8(body).map_err(|error| error.to_string())?;
    let value = parse(&text)?;
    // A response to a server-initiated request has no method; the server sends
    // none, so anything shaped that way is not ours to act on.
    let Some(method) = value.get("method").and_then(Json::as_str) else {
        return Ok(Some(Message {
            id: value.get("id").cloned(),
            method: String::new(),
            params: Json::Null,
        }));
    };
    Ok(Some(Message {
        id: value.get("id").cloned(),
        method: method.to_string(),
        params: value.get("params").cloned().unwrap_or(Json::Null),
    }))
}

/// The body length from a header block, or `None` at a clean end of input.
fn read_headers(input: &mut impl BufRead) -> Result<Option<usize>, String> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let read = input.read_line(&mut line).map_err(|error| error.to_string())?;
        if read == 0 {
            return match length {
                // A header block cut off mid-way is a truncated message, not a
                // clean shutdown, and saying so beats hanging.
                Some(_) => Err("input ended inside a message header".to_string()),
                None => Ok(None),
            };
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return length.map(Some).ok_or_else(|| "a message had no Content-Length".to_string());
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = Some(
                    value
                        .trim()
                        .parse()
                        .map_err(|_| format!("Content-Length is not a number: {value}"))?,
                );
            }
        }
    }
}

fn read_exact(input: &mut impl BufRead, buffer: &mut [u8]) -> Result<(), String> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = input.read(&mut buffer[filled..]).map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("input ended inside a message body".to_string());
        }
        filled += read;
    }
    Ok(())
}

fn send(out: &mut impl Write, value: &Json) -> Result<(), String> {
    let body = value.render();
    write!(out, "Content-Length: {}\r\n\r\n{body}", body.len())
        .and_then(|_| out.flush())
        .map_err(|error| error.to_string())
}

/// Answer a request.
pub fn respond(out: &mut impl Write, id: Json, result: Json) -> Result<(), String> {
    send(out, &Json::object([("jsonrpc", Json::string("2.0")), ("id", id), ("result", result)]))
}

/// Answer a request the server could not carry out.
pub fn respond_error(
    out: &mut impl Write,
    id: Json,
    code: i64,
    message: &str,
) -> Result<(), String> {
    let error =
        Json::object([("code", Json::Int(code)), ("message", Json::string(message.to_string()))]);
    send(out, &Json::object([("jsonrpc", Json::string("2.0")), ("id", id), ("error", error)]))
}

/// Tell the client something it did not ask about, such as new diagnostics.
pub fn notify(out: &mut impl Write, method: &str, params: Json) -> Result<(), String> {
    send(
        out,
        &Json::object([
            ("jsonrpc", Json::string("2.0")),
            ("method", Json::string(method.to_string())),
            ("params", params),
        ]),
    )
}

/// The protocol's code for a method the server does not implement.
pub const METHOD_NOT_FOUND: i64 = -32601;
