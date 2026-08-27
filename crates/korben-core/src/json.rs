//! JSON encoding and decoding for `std.json`.
//!
//! Serialization is a library, not privileged syntax: records encode as
//! objects, keywords as strings, and decoding produces maps with string keys.

// korben-6bc

use crate::value::{MapValue, RecordValue, Value};
use korben_syntax::diag::json_string;
use std::rc::Rc;

pub fn encode(value: &Value, pretty: bool) -> String {
    let mut out = String::new();
    write_value(&mut out, value, pretty, 0);
    out
}

fn write_value(out: &mut String, value: &Value, pretty: bool, depth: usize) {
    match value {
        Value::Nil => out.push_str("null"),
        Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Value::Int(value) => out.push_str(&value.to_string()),
        Value::Float(value) => out.push_str(&korben_syntax::format_float(*value)),
        Value::Str(text) => out.push_str(&json_string(text)),
        Value::Keyword(name) | Value::Symbol(name) => out.push_str(&json_string(name)),
        Value::Vector(items) | Value::Set(items) => {
            write_seq(out, items.iter(), pretty, depth, '[', ']', |out, item, pretty, depth| {
                write_value(out, item, pretty, depth)
            });
        }
        Value::Map(map) => {
            write_seq(
                out,
                map.entries.iter(),
                pretty,
                depth,
                '{',
                '}',
                |out, (key, value), pretty, depth| {
                    out.push_str(&json_string(&key_text(key)));
                    out.push(':');
                    if pretty {
                        out.push(' ');
                    }
                    write_value(out, value, pretty, depth);
                },
            );
        }
        Value::Record(record) => {
            write_seq(
                out,
                record.fields.iter(),
                pretty,
                depth,
                '{',
                '}',
                |out, (name, value), pretty, depth| {
                    out.push_str(&json_string(name));
                    out.push(':');
                    if pretty {
                        out.push(' ');
                    }
                    write_value(out, value, pretty, depth);
                },
            );
        }
        Value::Variant(variant) => {
            // Enums encode as a tag plus payload so decoding stays unambiguous.
            if variant.fields.is_empty() {
                out.push_str(&json_string(&variant.variant));
                return;
            }
            if variant.fields.len() == 1 && matches!(&*variant.type_name, "Option" | "Result") {
                if &*variant.variant == "None" {
                    out.push_str("null");
                    return;
                }
                write_value(out, &variant.fields[0].1, pretty, depth);
                return;
            }
            let entries: Vec<(String, &Value)> = std::iter::once(("$tag".to_string(), &Value::Nil))
                .map(|_| ("$tag".to_string(), &Value::Nil))
                .collect();
            let _ = entries;
            out.push('{');
            out.push_str(&json_string("$tag"));
            out.push(':');
            out.push_str(&json_string(&variant.variant));
            for (name, value) in &variant.fields {
                out.push(',');
                out.push_str(&json_string(name));
                out.push(':');
                write_value(out, value, pretty, depth);
            }
            out.push('}');
        }
        other => out.push_str(&json_string(&other.to_string())),
    }
}

fn key_text(key: &Value) -> String {
    match key {
        Value::Str(text) => (**text).clone(),
        Value::Keyword(name) | Value::Symbol(name) => name.to_string(),
        other => crate::value::Display(other).to_string(),
    }
}

fn write_seq<T>(
    out: &mut String,
    items: impl Iterator<Item = T>,
    pretty: bool,
    depth: usize,
    open: char,
    close: char,
    mut write: impl FnMut(&mut String, T, bool, usize),
) {
    let items: Vec<T> = items.collect();
    out.push(open);
    if items.is_empty() {
        out.push(close);
        return;
    }
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        if pretty {
            out.push('\n');
            out.push_str(&"  ".repeat(depth + 1));
        }
        write(out, item, pretty, depth + 1);
    }
    if pretty {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    }
    out.push(close);
}

pub fn decode(text: &str) -> Result<Value, String> {
    let mut parser = Parser { bytes: text.as_bytes(), text, pos: 0 };
    parser.skip_whitespace();
    let value = parser.value()?;
    parser.skip_whitespace();
    if parser.pos != parser.bytes.len() {
        return Err(format!("unexpected trailing input at byte {}", parser.pos));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn skip_whitespace(&mut self) {
        while matches!(self.bytes.get(self.pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        self.skip_whitespace();
        match self.bytes.get(self.pos) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Value::str(self.string()?)),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Nil),
            Some(_) => self.number(),
            None => Err("unexpected end of JSON input".to_string()),
        }
    }

    fn literal(&mut self, text: &str, value: Value) -> Result<Value, String> {
        if self.text[self.pos..].starts_with(text) {
            self.pos += text.len();
            return Ok(value);
        }
        Err(format!("invalid JSON literal at byte {}", self.pos))
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if matches!(self.bytes.get(self.pos), Some(b'-' | b'+')) {
            self.pos += 1;
        }
        let mut is_float = false;
        while let Some(byte) = self.bytes.get(self.pos) {
            match byte {
                b'0'..=b'9' => self.pos += 1,
                b'.' | b'e' | b'E' | b'+' | b'-' => {
                    is_float = true;
                    self.pos += 1;
                }
                _ => break,
            }
        }
        let raw = &self.text[start..self.pos];
        if raw.is_empty() {
            return Err(format!("expected a value at byte {start}"));
        }
        if is_float {
            raw.parse::<f64>().map(Value::Float).map_err(|error| error.to_string())
        } else {
            raw.parse::<i64>().map(Value::Int).map_err(|error| error.to_string())
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(byte) = self.bytes.get(self.pos) else {
                return Err("unterminated JSON string".to_string());
            };
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let Some(escape) = self.bytes.get(self.pos) else {
                        return Err("unterminated JSON escape".to_string());
                    };
                    self.pos += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self
                                .text
                                .get(self.pos..self.pos + 4)
                                .ok_or_else(|| "truncated \\u escape".to_string())?;
                            self.pos += 4;
                            let code =
                                u32::from_str_radix(hex, 16).map_err(|error| error.to_string())?;
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                        other => return Err(format!("invalid escape `\\{}`", *other as char)),
                    }
                }
                _ => {
                    let start = self.pos;
                    let width = utf8_width(*byte);
                    self.pos += width;
                    out.push_str(&self.text[start..self.pos]);
                }
            }
        }
    }

    fn array(&mut self) -> Result<Value, String> {
        self.pos += 1;
        let mut items = Vec::new();
        loop {
            self.skip_whitespace();
            if self.bytes.get(self.pos) == Some(&b']') {
                self.pos += 1;
                return Ok(Value::vector(items));
            }
            items.push(self.value()?);
            self.skip_whitespace();
            match self.bytes.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b']') => {}
                _ => return Err(format!("expected `,` or `]` at byte {}", self.pos)),
            }
        }
    }

    fn object(&mut self) -> Result<Value, String> {
        self.pos += 1;
        let mut map = MapValue::default();
        let mut tag: Option<String> = None;
        loop {
            self.skip_whitespace();
            if self.bytes.get(self.pos) == Some(&b'}') {
                self.pos += 1;
                break;
            }
            if self.bytes.get(self.pos) != Some(&b'"') {
                return Err(format!("expected a key string at byte {}", self.pos));
            }
            let key = self.string()?;
            self.skip_whitespace();
            if self.bytes.get(self.pos) != Some(&b':') {
                return Err(format!("expected `:` at byte {}", self.pos));
            }
            self.pos += 1;
            let value = self.value()?;
            if key == "$tag" {
                if let Value::Str(text) = &value {
                    tag = Some((**text).clone());
                }
            } else {
                map.insert(Value::keyword(&key), value);
            }
            self.skip_whitespace();
            match self.bytes.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b'}') => {}
                _ => return Err(format!("expected `,` or `}}` at byte {}", self.pos)),
            }
        }
        match tag {
            Some(tag) => Ok(Value::Record(Rc::new(RecordValue {
                type_name: Some(Rc::from(tag.as_str())),
                fields: map
                    .entries
                    .into_iter()
                    .map(|(key, value)| (Rc::from(key_text(&key).as_str()), value))
                    .collect(),
            }))),
            None => Ok(Value::Map(Rc::new(map))),
        }
    }
}

fn utf8_width(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if byte >> 5 == 0b110 {
        2
    } else if byte >> 4 == 0b1110 {
        3
    } else if byte >> 3 == 0b11110 {
        4
    } else {
        1
    }
}
