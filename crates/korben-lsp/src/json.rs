//! Plain JSON, for talking to an editor.
//!
//! `korben-runtime` already has a JSON codec, but it is a codec for Korben
//! data: object keys become keywords and `$tag` selects a variant. Those are
//! the right choices for `std.json` and the wrong ones for a transport, where a
//! message means exactly what the protocol says it means and nothing more. So
//! this is an ordinary JSON value with an ordinary parser.

// korben-efd

use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    /// An object built from pairs, which is how every response is assembled.
    pub fn object(fields: impl IntoIterator<Item = (&'static str, Json)>) -> Json {
        Json::Object(fields.into_iter().map(|(name, value)| (name.to_string(), value)).collect())
    }

    pub fn string(text: impl Into<String>) -> Json {
        Json::Str(text.into())
    }

    pub fn array(items: impl IntoIterator<Item = Json>) -> Json {
        Json::Array(items.into_iter().collect())
    }

    pub fn get(&self, name: &str) -> Option<&Json> {
        match self {
            Json::Object(fields) => fields.get(name),
            _ => None,
        }
    }

    /// A field reached through nested objects, as `params.textDocument.uri` is.
    pub fn path(&self, names: &[&str]) -> Option<&Json> {
        let mut current = self;
        for name in names {
            current = current.get(name)?;
        }
        Some(current)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(value) => Some(*value),
            Json::Float(value) => Some(*value as i64),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Int(value) => {
                let _ = write!(out, "{value}");
            }
            Json::Float(value) => {
                // A float that lands on a whole number still has to read as a
                // number to the editor, not as an integer-shaped string.
                if value.is_finite() {
                    let _ = write!(out, "{value}");
                } else {
                    out.push_str("null");
                }
            }
            Json::Str(text) => write_string(text, out),
            Json::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(fields) => {
                out.push('{');
                for (index, (name, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_string(name, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Control characters have no literal form in JSON.
            character if (character as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", character as u32);
            }
            character => out.push(character),
        }
    }
    out.push('"');
}

pub fn parse(text: &str) -> Result<Json, String> {
    let mut parser = Parser { bytes: text.as_bytes(), text, pos: 0 };
    parser.space();
    let value = parser.value()?;
    parser.space();
    if parser.pos != parser.bytes.len() {
        return Err(format!("trailing input at byte {}", parser.pos));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while matches!(self.bytes.get(self.pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.space();
        match self.bytes.get(self.pos) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(_) => self.number(),
            None => Err("unexpected end of input".to_string()),
        }
    }

    fn literal(&mut self, text: &str, value: Json) -> Result<Json, String> {
        if self.text[self.pos..].starts_with(text) {
            self.pos += text.len();
            return Ok(value);
        }
        Err(format!("invalid literal at byte {}", self.pos))
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.bytes.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        let mut float = false;
        while let Some(byte) = self.bytes.get(self.pos) {
            match byte {
                b'0'..=b'9' => self.pos += 1,
                b'.' | b'e' | b'E' | b'+' | b'-' => {
                    float = true;
                    self.pos += 1;
                }
                _ => break,
            }
        }
        let raw = &self.text[start..self.pos];
        if raw.is_empty() {
            return Err(format!("expected a value at byte {start}"));
        }
        if float {
            raw.parse::<f64>().map(Json::Float).map_err(|error| error.to_string())
        } else {
            raw.parse::<i64>().map(Json::Int).map_err(|error| error.to_string())
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(byte) = self.bytes.get(self.pos) else {
                return Err("unterminated string".to_string());
            };
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let Some(escape) = self.bytes.get(self.pos) else {
                        return Err("unterminated escape".to_string());
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
                        b'u' => out.push(self.escape()?),
                        other => return Err(format!("unknown escape `\\{}`", *other as char)),
                    }
                }
                _ => {
                    let rest = &self.text[self.pos..];
                    let character =
                        rest.chars().next().ok_or_else(|| "invalid UTF-8".to_string())?;
                    self.pos += character.len_utf8();
                    out.push(character);
                }
            }
        }
    }

    /// A `\uXXXX` escape, including the surrogate pair a non-BMP character
    /// arrives as.
    fn escape(&mut self) -> Result<char, String> {
        let first = self.hex()?;
        if !(0xD800..0xDC00).contains(&first) {
            return char::from_u32(first).ok_or_else(|| "invalid escape".to_string());
        }
        if self.bytes.get(self.pos) != Some(&b'\\') || self.bytes.get(self.pos + 1) != Some(&b'u') {
            return Err("a leading surrogate needs a trailing one".to_string());
        }
        self.pos += 2;
        let second = self.hex()?;
        if !(0xDC00..0xE000).contains(&second) {
            return Err("expected a trailing surrogate".to_string());
        }
        let combined = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
        char::from_u32(combined).ok_or_else(|| "invalid surrogate pair".to_string())
    }

    fn hex(&mut self) -> Result<u32, String> {
        let end = self.pos + 4;
        if end > self.bytes.len() {
            return Err("truncated escape".to_string());
        }
        let digits = &self.text[self.pos..end];
        self.pos = end;
        u32::from_str_radix(digits, 16).map_err(|error| error.to_string())
    }

    fn array(&mut self) -> Result<Json, String> {
        self.pos += 1;
        let mut items = Vec::new();
        loop {
            self.space();
            if self.bytes.get(self.pos) == Some(&b']') {
                self.pos += 1;
                return Ok(Json::Array(items));
            }
            items.push(self.value()?);
            self.space();
            match self.bytes.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b']') => {}
                _ => return Err(format!("expected `,` or `]` at byte {}", self.pos)),
            }
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.pos += 1;
        let mut fields = BTreeMap::new();
        loop {
            self.space();
            if self.bytes.get(self.pos) == Some(&b'}') {
                self.pos += 1;
                return Ok(Json::Object(fields));
            }
            if self.bytes.get(self.pos) != Some(&b'"') {
                return Err(format!("expected a key at byte {}", self.pos));
            }
            let key = self.string()?;
            self.space();
            if self.bytes.get(self.pos) != Some(&b':') {
                return Err(format!("expected `:` at byte {}", self.pos));
            }
            self.pos += 1;
            let value = self.value()?;
            fields.insert(key, value);
            self.space();
            match self.bytes.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b'}') => {}
                _ => return Err(format!("expected `,` or `}}` at byte {}", self.pos)),
            }
        }
    }
}
