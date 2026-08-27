//! Tokenizer for Korben source text.
//!
//! Errors are reported as full [`Diagnostic`] values, which carry their spans
//! and suggestions; the size of that error type is deliberate.
#![allow(clippy::result_large_err)]
//!
//! The lexer is deliberately small: the specification requires reader syntax to
//! stay minimal, so anything that changes semantics belongs in a macro or core
//! form rather than here. Comments are emitted as tokens because the formatter
//! must preserve them.

// korben-6bc

use crate::diag::Diagnostic;
use crate::span::{FileId, Span};

#[derive(Clone, PartialEq, Debug)]
pub enum TokenKind {
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    /// `#{` set opener.
    HashBrace,
    /// `#(` anonymous function shorthand opener.
    HashParen,
    /// `'`
    Quote,
    /// `` ` ``
    SyntaxQuote,
    /// `~`
    Unquote,
    /// `~@`
    UnquoteSplice,
    /// `#'`
    VarQuote,
    /// `#tag`, e.g. `#uuid`, `#date`, `#duration`.
    Tag(String),
    Int(i64),
    Float(f64),
    Str(String),
    Keyword(String),
    Symbol(String),
    LineComment(String),
    BlockComment(String),
    /// `;;;` documentation comment attached to the following declaration.
    DocComment(String),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// True when at least one newline appeared before this token. The formatter
    /// uses this to preserve author-intended blank lines.
    pub newline_before: bool,
    /// Number of blank lines immediately before this token, capped at 1.
    pub blank_before: bool,
}

pub struct Lexer<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    file: FileId,
}

impl<'a> Lexer<'a> {
    pub fn new(file: FileId, text: &'a str) -> Lexer<'a> {
        Lexer { text, bytes: text.as_bytes(), pos: 0, file }
    }

    fn span(&self, start: usize) -> Span {
        Span::new(self.file, start as u32, self.pos as u32)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        // Advance by a whole UTF-8 scalar so byte offsets stay on char boundaries.
        self.pos += utf8_len(byte);
        Some(byte)
    }

    /// Skip whitespace and commas, reporting whether newlines/blank lines were crossed.
    fn skip_trivia(&mut self) -> (bool, bool) {
        let mut newline = false;
        let mut newline_count = 0usize;
        loop {
            match self.peek() {
                Some(b'\n') => {
                    newline = true;
                    newline_count += 1;
                    self.pos += 1;
                }
                // Commas are whitespace, a common Lisp affordance for map literals.
                Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b',') => self.pos += 1,
                _ => break,
            }
        }
        (newline, newline_count > 1)
    }

    pub fn tokenize(mut self) -> (Vec<Token>, Vec<Diagnostic>) {
        let mut tokens = Vec::new();
        let mut errors = Vec::new();
        loop {
            let (newline_before, blank_before) = self.skip_trivia();
            let start = self.pos;
            let Some(byte) = self.peek() else {
                tokens.push(Token {
                    kind: TokenKind::Eof,
                    span: self.span(start),
                    newline_before,
                    blank_before,
                });
                break;
            };

            let kind = match byte {
                b'(' => {
                    self.pos += 1;
                    TokenKind::LParen
                }
                b')' => {
                    self.pos += 1;
                    TokenKind::RParen
                }
                b'[' => {
                    self.pos += 1;
                    TokenKind::LBracket
                }
                b']' => {
                    self.pos += 1;
                    TokenKind::RBracket
                }
                b'{' => {
                    self.pos += 1;
                    TokenKind::LBrace
                }
                b'}' => {
                    self.pos += 1;
                    TokenKind::RBrace
                }
                b'\'' => {
                    self.pos += 1;
                    TokenKind::Quote
                }
                b'`' => {
                    self.pos += 1;
                    TokenKind::SyntaxQuote
                }
                b'~' => {
                    self.pos += 1;
                    if self.peek() == Some(b'@') {
                        self.pos += 1;
                        TokenKind::UnquoteSplice
                    } else {
                        TokenKind::Unquote
                    }
                }
                b';' => {
                    let doc = self.peek_at(1) == Some(b';') && self.peek_at(2) == Some(b';');
                    while self.peek().is_some() && self.peek() != Some(b'\n') {
                        self.pos += 1;
                    }
                    let raw = &self.text[start..self.pos];
                    if doc {
                        TokenKind::DocComment(raw.trim_start_matches(';').trim().to_string())
                    } else {
                        TokenKind::LineComment(raw.trim_start_matches(';').trim_end().to_string())
                    }
                }
                b'"' => match self.lex_string(start) {
                    Ok(value) => TokenKind::Str(value),
                    Err(diagnostic) => {
                        errors.push(diagnostic);
                        TokenKind::Str(String::new())
                    }
                },
                b'r' if matches!(self.peek_at(1), Some(b'"') | Some(b'#')) => {
                    match self.lex_raw_string(start) {
                        Ok(Some(value)) => TokenKind::Str(value),
                        Ok(None) => self.lex_atom(start),
                        Err(diagnostic) => {
                            errors.push(diagnostic);
                            TokenKind::Str(String::new())
                        }
                    }
                }
                b'#' => match self.peek_at(1) {
                    Some(b'{') => {
                        self.pos += 2;
                        TokenKind::HashBrace
                    }
                    Some(b'(') => {
                        self.pos += 2;
                        TokenKind::HashParen
                    }
                    Some(b'\'') => {
                        self.pos += 2;
                        TokenKind::VarQuote
                    }
                    Some(b'|') => {
                        self.pos += 2;
                        match self.lex_block_comment(start) {
                            Ok(text) => TokenKind::BlockComment(text),
                            Err(diagnostic) => {
                                errors.push(diagnostic);
                                TokenKind::BlockComment(String::new())
                            }
                        }
                    }
                    Some(byte) if is_symbol_byte(byte) => {
                        self.pos += 1;
                        let name_start = self.pos;
                        while self.peek().is_some_and(is_symbol_byte) {
                            self.pos += 1;
                        }
                        TokenKind::Tag(self.text[name_start..self.pos].to_string())
                    }
                    _ => {
                        self.pos += 1;
                        errors.push(
                            Diagnostic::error("unsupported reader dispatch")
                                .with_code("reader-dispatch")
                                .at(
                                    self.span(start),
                                    "`#` must be followed by a tag, `{`, `(`, `'`, or `|`",
                                ),
                        );
                        TokenKind::Symbol("#".to_string())
                    }
                },
                b':' => {
                    self.pos += 1;
                    let name_start = self.pos;
                    while self.peek().is_some_and(is_symbol_byte) {
                        self.pos += 1;
                    }
                    if name_start == self.pos {
                        errors.push(
                            Diagnostic::error("empty keyword")
                                .with_code("reader-keyword")
                                .at(self.span(start), "a keyword needs a name after `:`"),
                        );
                    }
                    TokenKind::Keyword(self.text[name_start..self.pos].to_string())
                }
                _ => self.lex_atom(start),
            };

            tokens.push(Token { kind, span: self.span(start), newline_before, blank_before });
        }
        (tokens, errors)
    }

    /// Lex a symbol or number starting at `start`.
    fn lex_atom(&mut self, start: usize) -> TokenKind {
        while self.peek().is_some_and(is_symbol_byte) {
            self.pos += 1;
        }
        if self.pos == start {
            // Not a legal symbol byte at all; consume one scalar to make progress.
            self.bump();
        }
        let raw = &self.text[start..self.pos];
        classify_atom(raw)
    }

    fn lex_string(&mut self, start: usize) -> Result<String, Diagnostic> {
        self.pos += 1; // opening quote
        let mut value = String::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err(Diagnostic::error("unterminated string literal")
                    .with_code("reader-string")
                    .at(self.span(start), "this string is never closed")
                    .help("add a closing `\"`"));
            };
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(value);
                }
                b'\\' => {
                    self.pos += 1;
                    let escape_start = self.pos;
                    let Some(escape) = self.bump() else {
                        return Err(Diagnostic::error("unterminated escape sequence")
                            .with_code("reader-escape")
                            .at(self.span(escape_start), "expected an escape character"));
                    };
                    match escape {
                        b'n' => value.push('\n'),
                        b't' => value.push('\t'),
                        b'r' => value.push('\r'),
                        b'0' => value.push('\0'),
                        b'\\' => value.push('\\'),
                        b'"' => value.push('"'),
                        b'{' => value.push('{'),
                        b'}' => value.push('}'),
                        b'u' => {
                            if self.peek() != Some(b'{') {
                                return Err(Diagnostic::error("malformed unicode escape")
                                    .with_code("reader-escape")
                                    .at(self.span(escape_start), "expected `{` after `\\u`")
                                    .help("write `\\u{1f600}`"));
                            }
                            self.pos += 1;
                            let digits_start = self.pos;
                            while self.peek().is_some_and(|byte| byte.is_ascii_hexdigit()) {
                                self.pos += 1;
                            }
                            let digits = &self.text[digits_start..self.pos];
                            if self.peek() != Some(b'}') {
                                return Err(Diagnostic::error("malformed unicode escape")
                                    .with_code("reader-escape")
                                    .at(self.span(escape_start), "expected `}` to close `\\u{`"));
                            }
                            self.pos += 1;
                            let code =
                                u32::from_str_radix(digits, 16).ok().and_then(char::from_u32);
                            match code {
                                Some(ch) => value.push(ch),
                                None => {
                                    return Err(Diagnostic::error("invalid unicode scalar value")
                                        .with_code("reader-escape")
                                        .at(
                                            self.span(escape_start),
                                            format!("`{digits}` is not a valid code point"),
                                        ))
                                }
                            }
                        }
                        other => {
                            return Err(Diagnostic::error(format!(
                                "unknown escape sequence `\\{}`",
                                other as char
                            ))
                            .with_code("reader-escape")
                            .at(self.span(escape_start), "escape not recognized")
                            .help("use `r\"...\"` for a raw string"));
                        }
                    }
                }
                _ => {
                    let ch_start = self.pos;
                    self.bump();
                    value.push_str(&self.text[ch_start..self.pos]);
                }
            }
        }
    }

    /// Lex `r"..."` or `r#"..."#` with any number of hashes.
    /// Returns `Ok(None)` when this is actually an ordinary symbol beginning with `r`.
    fn lex_raw_string(&mut self, start: usize) -> Result<Option<String>, Diagnostic> {
        let mut probe = self.pos + 1;
        let mut hashes = 0usize;
        while self.bytes.get(probe) == Some(&b'#') {
            hashes += 1;
            probe += 1;
        }
        if self.bytes.get(probe) != Some(&b'"') {
            return Ok(None);
        }
        self.pos = probe + 1;
        let body_start = self.pos;
        let closing: Vec<u8> =
            std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
        loop {
            if self.pos >= self.bytes.len() {
                return Err(Diagnostic::error("unterminated raw string literal")
                    .with_code("reader-string")
                    .at(self.span(start), "this raw string is never closed"));
            }
            if self.bytes[self.pos..].starts_with(&closing) {
                let value = self.text[body_start..self.pos].to_string();
                self.pos += closing.len();
                return Ok(Some(value));
            }
            self.bump();
        }
    }

    /// Lex `#| ... |#`, honoring nesting.
    fn lex_block_comment(&mut self, start: usize) -> Result<String, Diagnostic> {
        let body_start = self.pos;
        let mut depth = 1usize;
        while depth > 0 {
            if self.pos >= self.bytes.len() {
                return Err(Diagnostic::error("unterminated block comment")
                    .with_code("reader-comment")
                    .at(self.span(start), "this `#|` is never closed by `|#`"));
            }
            if self.bytes[self.pos..].starts_with(b"#|") {
                depth += 1;
                self.pos += 2;
            } else if self.bytes[self.pos..].starts_with(b"|#") {
                depth -= 1;
                self.pos += 2;
            } else {
                self.bump();
            }
        }
        Ok(self.text[body_start..self.pos - 2].to_string())
    }
}

/// Bytes that may appear inside a symbol or keyword name.
fn is_symbol_byte(byte: u8) -> bool {
    !matches!(
        byte,
        b'(' | b')'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'"'
            | b'\''
            | b'`'
            | b'~'
            | b';'
            | b','
            | b' '
            | b'\t'
            | b'\n'
            | b'\r'
    )
}

fn utf8_len(byte: u8) -> usize {
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

/// Decide whether an atom is an integer, float, or symbol.
fn classify_atom(raw: &str) -> TokenKind {
    if raw.is_empty() {
        return TokenKind::Symbol(String::new());
    }
    let looks_numeric = {
        let mut chars = raw.chars();
        match chars.next() {
            Some(ch) if ch.is_ascii_digit() => true,
            Some('-') | Some('+') => chars.next().map(|ch| ch.is_ascii_digit()).unwrap_or(false),
            _ => false,
        }
    };
    if looks_numeric {
        let cleaned: String = raw.chars().filter(|ch| *ch != '_').collect();
        if let Some(hex) = cleaned.strip_prefix("0x").or_else(|| cleaned.strip_prefix("0X")) {
            if let Ok(value) = i64::from_str_radix(hex, 16) {
                return TokenKind::Int(value);
            }
        }
        if let Some(binary) = cleaned.strip_prefix("0b").or_else(|| cleaned.strip_prefix("0B")) {
            if let Ok(value) = i64::from_str_radix(binary, 2) {
                return TokenKind::Int(value);
            }
        }
        if let Ok(value) = cleaned.parse::<i64>() {
            return TokenKind::Int(value);
        }
        if let Ok(value) = cleaned.parse::<f64>() {
            return TokenKind::Float(value);
        }
    }
    TokenKind::Symbol(raw.to_string())
}
