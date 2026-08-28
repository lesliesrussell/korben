//! The `#[korben_export]` attribute.
//!
//! Specification 17.3 has a Rust library expose a Korben-compatible API through
//! an adapter. The attribute is that adapter's authoring end: it writes the
//! annotated function back unchanged and adds an `extern "C"` shim over the ABI
//! `korben-export` defines, so the function stays ordinary Rust that ordinary
//! Rust callers keep using.
//!
//! It needs `proc_macro`, which the compiler ships, and nothing else. `syn` and
//! `quote` are the usual way to write this, and they are ordinary crates, which
//! the toolchain does not take -- the reader, the TOML parser, and the JSON-RPC
//! codec are all in-tree for the same reason.
//!
//! Reading the signature lives in `korben-adapter`, where `korben ffi rust`
//! reads it too. One reader means the two halves of the boundary cannot
//! disagree about a type, a symbol name, or a rejection.

// korben-10s

use proc_macro::TokenStream;

/// Export a function across the Korben adapter boundary.
///
/// ```ignore
/// #[korben_export]
/// pub fn slugify(input: &str) -> Result<String, String> {
///     Ok(input.to_lowercase().replace(' ', "-"))
/// }
/// ```
///
/// Parameters and returns carry `i64`, `f64`, `bool`, `&str`, and `String`; a
/// fallible function returns `Result<T, E>` for any printable `E`. Anything
/// else is a compile error naming what could not cross, because a binding that
/// is wrong is worse than one that is missing.
#[proc_macro_attribute]
pub fn korben_export(attribute: TokenStream, item: TokenStream) -> TokenStream {
    if !attribute.is_empty() {
        return error("`#[korben_export]` takes no arguments");
    }
    let source = item.to_string();
    let signature = match korben_adapter::parse(&source) {
        Ok(signature) => signature,
        Err(message) => return error(&message),
    };
    korben_adapter::rust::render(&source, &signature).parse().unwrap_or_else(|_| {
        error(
            "`#[korben_export]` generated a shim that does not parse; \
             please report this as a compiler bug",
        )
    })
}

/// Report a rejection where the compiler will show it.
fn error(message: &str) -> TokenStream {
    format!("::std::compile_error!({message:?});").parse().expect("a literal compile_error")
}
