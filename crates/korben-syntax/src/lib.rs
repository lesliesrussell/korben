//! Korben front-end syntax services.
//!
//! This crate owns everything between source text and syntax objects: source
//! maps and spans, diagnostics, the lexer, the reader, and the canonical
//! formatter. It has no dependencies outside the standard library so that the
//! `korben` executable stays a single self-contained binary.

// korben-6bc

pub mod diag;
pub mod fmt;
pub mod lexer;
pub mod reader;
pub mod span;

pub use diag::{Diagnostic, Diagnostics, Severity};
pub use reader::{read_all, Comments, Datum, Syntax};
pub use span::{FileId, SourceMap, Span};

/// Canonical float rendering: always shows a decimal point so that `1.0` never
/// round-trips to the integer `1`.
pub fn format_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let mut text = format!("{value}");
    if !text.contains(['.', 'e', 'E']) {
        text.push_str(".0");
    }
    text
}
