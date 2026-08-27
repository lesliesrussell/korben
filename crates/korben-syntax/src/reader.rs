//! The reader: turns tokens into syntax objects.
//!
//! A [`Syntax`] value is data plus provenance. It carries a source span, a
//! hygiene scope set, and (in formatting mode) the comments the author wrote.
//! Macro expansion operates on these objects rather than raw lists so that
//! spans, scopes, and expansion chains survive every phase.

// korben-6bc

use crate::diag::Diagnostic;
use crate::lexer::{Lexer, Token, TokenKind};
use crate::span::{FileId, Span};
use std::fmt;

/// A hygiene scope identifier. Every macro expansion introduces a fresh scope;
/// a symbol resolves against the bindings that share its scope set.
pub type Scope = u32;

#[derive(Clone, PartialEq, Debug)]
pub enum CommentKind {
    Line,
    Block,
    Doc,
}

#[derive(Clone, Debug)]
pub enum Datum {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Keyword(String),
    Symbol(String),
    List(Vec<Syntax>),
    Vector(Vec<Syntax>),
    /// Alternating key/value syntax objects, order preserved as written.
    Map(Vec<Syntax>),
    Set(Vec<Syntax>),
    /// `#tag payload`, e.g. `#uuid "..."`.
    Tagged(String, Box<Syntax>),
    /// Only produced in formatting mode.
    Comment(CommentKind, String),
}

/// Data plus provenance: source span, hygiene scopes, and expansion history.
#[derive(Clone, Debug)]
pub struct Syntax {
    pub datum: Datum,
    pub span: Span,
    /// Hygiene scope set. Empty for surface syntax the user typed.
    pub scopes: Vec<Scope>,
    /// True when the author left a blank line before this form. The formatter
    /// preserves exactly one blank line where this is set.
    pub blank_before: bool,
}

impl Syntax {
    pub fn new(datum: Datum, span: Span) -> Syntax {
        Syntax { datum, span, scopes: Vec::new(), blank_before: false }
    }

    /// A compiler-synthesized form with no source location.
    pub fn synthetic(datum: Datum) -> Syntax {
        Syntax::new(datum, Span::synthetic())
    }

    pub fn symbol(name: impl Into<String>, span: Span) -> Syntax {
        Syntax::new(Datum::Symbol(name.into()), span)
    }

    pub fn list(items: Vec<Syntax>, span: Span) -> Syntax {
        Syntax::new(Datum::List(items), span)
    }

    /// Add a hygiene scope to this form and everything inside it.
    pub fn add_scope(&mut self, scope: Scope) {
        if !self.scopes.contains(&scope) {
            self.scopes.push(scope);
        }
        match &mut self.datum {
            Datum::List(items) | Datum::Vector(items) | Datum::Map(items) | Datum::Set(items) => {
                for item in items {
                    item.add_scope(scope);
                }
            }
            Datum::Tagged(_, inner) => inner.add_scope(scope),
            _ => {}
        }
    }

    pub fn as_symbol(&self) -> Option<&str> {
        match &self.datum {
            Datum::Symbol(name) => Some(name),
            _ => None,
        }
    }

    pub fn as_keyword(&self) -> Option<&str> {
        match &self.datum {
            Datum::Keyword(name) => Some(name),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.datum {
            Datum::Str(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Syntax]> {
        match &self.datum {
            Datum::List(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_vector(&self) -> Option<&[Syntax]> {
        match &self.datum {
            Datum::Vector(items) => Some(items),
            _ => None,
        }
    }

    pub fn is_comment(&self) -> bool {
        matches!(self.datum, Datum::Comment(..))
    }

    /// The head symbol of a list form, if there is one.
    pub fn head_symbol(&self) -> Option<&str> {
        self.as_list()?.first()?.as_symbol()
    }

    /// A short description used in diagnostics.
    pub fn describe(&self) -> &'static str {
        match &self.datum {
            Datum::Nil => "nil",
            Datum::Bool(_) => "a boolean",
            Datum::Int(_) => "an integer",
            Datum::Float(_) => "a float",
            Datum::Str(_) => "a string",
            Datum::Keyword(_) => "a keyword",
            Datum::Symbol(_) => "a symbol",
            Datum::List(_) => "a list",
            Datum::Vector(_) => "a vector",
            Datum::Map(_) => "a map",
            Datum::Set(_) => "a set",
            Datum::Tagged(..) => "a tagged literal",
            Datum::Comment(..) => "a comment",
        }
    }
}

impl fmt::Display for Syntax {
    /// Compact single-line rendering, used in diagnostics and the REPL.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.datum {
            Datum::Nil => write!(out, "nil"),
            Datum::Bool(value) => write!(out, "{value}"),
            Datum::Int(value) => write!(out, "{value}"),
            Datum::Float(value) => write!(out, "{}", crate::format_float(*value)),
            Datum::Str(value) => write!(out, "{}", crate::diag::json_string(value)),
            Datum::Keyword(name) => write!(out, ":{name}"),
            Datum::Symbol(name) => write!(out, "{name}"),
            Datum::List(items) => write_seq(out, "(", items, ")"),
            Datum::Vector(items) => write_seq(out, "[", items, "]"),
            Datum::Map(items) => write_seq(out, "{", items, "}"),
            Datum::Set(items) => write_seq(out, "#{", items, "}"),
            Datum::Tagged(tag, inner) => write!(out, "#{tag} {inner}"),
            Datum::Comment(CommentKind::Doc, text) => write!(out, ";;; {text}"),
            Datum::Comment(CommentKind::Line, text) => write!(out, ";{text}"),
            Datum::Comment(CommentKind::Block, text) => write!(out, "#|{text}|#"),
        }
    }
}

fn write_seq(
    out: &mut fmt::Formatter<'_>,
    open: &str,
    items: &[Syntax],
    close: &str,
) -> fmt::Result {
    out.write_str(open)?;
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.write_str(" ")?;
        }
        write!(out, "{item}")?;
    }
    out.write_str(close)
}

/// Whether the reader keeps comments in its output.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Comments {
    /// Drop comments except doc comments, which stay attached for `korben doc`.
    Skip,
    /// Keep every comment as a `Datum::Comment` node. Used by the formatter.
    Keep,
}

pub struct Reader {
    tokens: Vec<Token>,
    pos: usize,
    comments: Comments,
    diagnostics: Vec<Diagnostic>,
    /// Nesting depth, so documentation comments survive only at the top level
    /// where they attach to a declaration.
    depth: usize,
}

/// Read a whole source file into a sequence of top-level syntax objects.
pub fn read_all(file: FileId, text: &str, comments: Comments) -> (Vec<Syntax>, Vec<Diagnostic>) {
    let (tokens, lex_errors) = Lexer::new(file, text).tokenize();
    let mut reader = Reader { tokens, pos: 0, comments, diagnostics: lex_errors, depth: 0 };
    let mut forms = Vec::new();
    loop {
        reader.skip_dropped_comments();
        if matches!(reader.peek().kind, TokenKind::Eof) {
            break;
        }
        match reader.read_form() {
            Some(form) => forms.push(form),
            None => break,
        }
    }
    (forms, reader.diagnostics)
}

impl Reader {
    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or_else(|| self.tokens.last().unwrap())
    }

    fn bump(&mut self) -> Token {
        let token = self.peek().clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        token
    }

    fn error(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// In `Comments::Skip` mode, comments never reach the caller, except for
    /// top-level `;;;` documentation comments, which `korben doc` needs.
    fn skip_dropped_comments(&mut self) {
        if self.comments == Comments::Keep {
            return;
        }
        loop {
            match self.peek().kind {
                TokenKind::DocComment(_) if self.depth == 0 => return,
                TokenKind::LineComment(_)
                | TokenKind::BlockComment(_)
                | TokenKind::DocComment(_) => {
                    self.bump();
                }
                _ => return,
            }
        }
    }

    fn read_form(&mut self) -> Option<Syntax> {
        self.skip_dropped_comments();
        let token = self.bump();
        let span = token.span;
        let blank = token.blank_before;
        let mut form = match token.kind {
            TokenKind::Eof => return None,
            TokenKind::Int(value) => Syntax::new(Datum::Int(value), span),
            TokenKind::Float(value) => Syntax::new(Datum::Float(value), span),
            TokenKind::Str(value) => Syntax::new(Datum::Str(value), span),
            TokenKind::Keyword(name) => Syntax::new(Datum::Keyword(name), span),
            TokenKind::Symbol(name) => match name.as_str() {
                "true" => Syntax::new(Datum::Bool(true), span),
                "false" => Syntax::new(Datum::Bool(false), span),
                "nil" => Syntax::new(Datum::Nil, span),
                _ => Syntax::new(Datum::Symbol(name), span),
            },
            TokenKind::LineComment(text) => {
                Syntax::new(Datum::Comment(CommentKind::Line, text), span)
            }
            TokenKind::BlockComment(text) => {
                Syntax::new(Datum::Comment(CommentKind::Block, text), span)
            }
            TokenKind::DocComment(text) => {
                Syntax::new(Datum::Comment(CommentKind::Doc, text), span)
            }
            TokenKind::LParen => self.read_seq(span, TokenKind::RParen, ")", Datum::List)?,
            TokenKind::LBracket => self.read_seq(span, TokenKind::RBracket, "]", Datum::Vector)?,
            TokenKind::LBrace => self.read_seq(span, TokenKind::RBrace, "}", Datum::Map)?,
            TokenKind::HashBrace => self.read_seq(span, TokenKind::RBrace, "}", Datum::Set)?,
            TokenKind::HashParen => {
                // `#(...)` expands to `(fn-shorthand ...)`; the parser turns it
                // into a lambda after scanning for `%` parameters.
                let inner = self.read_seq(span, TokenKind::RParen, ")", Datum::List)?;
                let Datum::List(mut items) = inner.datum else { unreachable!() };
                items.insert(0, Syntax::symbol("fn-shorthand", span));
                Syntax::new(Datum::List(items), inner.span)
            }
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                self.error(
                    Diagnostic::error("unexpected closing delimiter")
                        .with_code("reader-delimiter")
                        .at(span, "there is no matching opening delimiter"),
                );
                return self.read_form();
            }
            TokenKind::Quote => self.read_prefixed("quote", span)?,
            TokenKind::SyntaxQuote => self.read_prefixed("syntax-quote", span)?,
            TokenKind::Unquote => self.read_prefixed("unquote", span)?,
            TokenKind::UnquoteSplice => self.read_prefixed("unquote-splice", span)?,
            TokenKind::VarQuote => self.read_prefixed("var-ref", span)?,
            TokenKind::Tag(tag) => {
                let Some(payload) = self.read_form() else {
                    self.error(
                        Diagnostic::error(format!("`#{tag}` has no payload"))
                            .with_code("reader-tag")
                            .at(span, "a tagged literal must be followed by a value"),
                    );
                    return None;
                };
                let full = span.to(payload.span);
                Syntax::new(Datum::Tagged(tag, Box::new(payload)), full)
            }
        };
        form.blank_before = blank;
        Some(form)
    }

    /// Read `<prefix> <form>` into `(<name> <form>)`.
    fn read_prefixed(&mut self, name: &str, span: Span) -> Option<Syntax> {
        let inner = self.read_form()?;
        let full = span.to(inner.span);
        Some(Syntax::list(vec![Syntax::symbol(name, span), inner], full))
    }

    fn read_seq(
        &mut self,
        open: Span,
        close: TokenKind,
        close_text: &str,
        build: fn(Vec<Syntax>) -> Datum,
    ) -> Option<Syntax> {
        let mut items = Vec::new();
        self.depth += 1;
        let result = loop {
            self.skip_dropped_comments();
            let next = self.peek();
            if next.kind == close {
                let end = self.bump().span;
                break Some(Syntax::new(build(items), open.to(end)));
            }
            if matches!(next.kind, TokenKind::Eof) {
                let span = next.span;
                self.error(
                    Diagnostic::error("unclosed delimiter")
                        .with_code("reader-delimiter")
                        .at(open, "this delimiter is never closed")
                        .secondary(span, "end of file reached here")
                        .help(format!("add a `{close_text}`")),
                );
                break Some(Syntax::new(build(items), open));
            }
            // A mismatched closer belongs to an enclosing form; stop here so the
            // outer reader can report it instead of consuming to end-of-file.
            if matches!(next.kind, TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace) {
                let span = next.span;
                self.error(
                    Diagnostic::error("mismatched delimiter")
                        .with_code("reader-delimiter")
                        .at(span, "this does not close the form opened here")
                        .secondary(open, "unclosed form starts here")
                        .help(format!("expected `{close_text}`")),
                );
                break Some(Syntax::new(build(items), open.to(span)));
            }
            match self.read_form() {
                Some(form) => items.push(form),
                None => break Some(Syntax::new(build(items), open)),
            }
        };
        self.depth -= 1;
        result
    }
}
