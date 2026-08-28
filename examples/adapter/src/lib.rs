//! A small Rust library exposed to Korben through the adapter ABI.
//!
//! Every function here is ordinary Rust that ordinary Rust callers can use.
//! `#[korben_export]` adds an `extern "C"` shim beside each one; nothing about
//! the function itself changes.
//!
//! Build it, then generate the Korben half:
//!
//! ```sh
//! cargo build                                  # in this directory
//! korben ffi rust src/lib.rs --library slug --module slug --out slug.kb
//! ```

use korben_export::korben_export;

/// Lowercase a title and join its words with hyphens.
#[korben_export]
pub fn slugify(input: &str) -> String {
    normalize(input).split_whitespace().collect::<Vec<_>>().join("-").to_lowercase()
}

/// How many words a string has.
#[korben_export]
pub fn word_count(input: &str) -> i64 {
    input.split_whitespace().count() as i64
}

/// Cut a string to `limit` characters, adding an ellipsis when it was longer.
///
/// Fallible, and the failure reaches Korben as an `Err` carrying this message.
#[korben_export]
pub fn truncate(input: &str, limit: i64) -> Result<String, String> {
    if limit < 1 {
        return Err(format!("a limit of {limit} leaves nothing to show"));
    }
    let limit = limit as usize;
    if input.chars().count() <= limit {
        return Ok(input.to_string());
    }
    Ok(input.chars().take(limit).collect::<String>() + "…")
}

/// Divide, refusing the case that has no answer.
#[korben_export]
pub fn ratio(numerator: f64, denominator: f64) -> Result<f64, String> {
    if denominator == 0.0 {
        return Err("a ratio needs a denominator".to_string());
    }
    Ok(numerator / denominator)
}

/// Not annotated, so the generator leaves it alone: it is this library's own
/// business, not part of the boundary.
fn normalize(input: &str) -> String {
    input.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_functions_are_ordinary_rust() {
        assert_eq!(slugify("Korben Is Fast"), "korben-is-fast");
        assert_eq!(word_count("one two three"), 3);
        assert_eq!(truncate("hello", 4).unwrap(), "hell…");
        assert!(ratio(1.0, 0.0).is_err());
        assert_eq!(normalize("  x  "), "x");
    }
}
