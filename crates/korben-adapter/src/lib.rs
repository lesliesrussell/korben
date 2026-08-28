//! The signature subset the Korben adapter boundary carries.
//!
//! Specification 17.3 has a Rust library expose a Korben-compatible API through
//! an adapter. That adapter has two halves -- an `extern "C"` shim on the Rust
//! side, written by `#[korben_export]`, and a binding module on the Korben
//! side, written by `korben ffi rust` -- and they have to agree about every
//! type, every symbol name, and every rejection. Reading the signature twice
//! would let them drift, so it is read once, here, and each half is rendered
//! from the same [`Signature`].
//!
//! The subset is what the foreign boundary can carry, and no more: a foreign
//! signature must be all-integer or all-floating, so `i64`, `bool`, `&str`, and
//! `String` travel together and `f64` travels alone. Anything else is declined
//! with a reason, the way `cheader.rs` declines a C prototype it cannot type,
//! because a binding that is wrong is worse than one that is missing.
//!
//! This crate deliberately exports no symbols of its own. `korben-export` has a
//! `#[no_mangle]` error channel, and linking that into the `korben` binary
//! would put a second definition in the process, where it could interpose on
//! the one inside an adapter and answer with the wrong thread's error.

// korben-rm1

pub mod korben;
pub mod rust;

/// How a Rust type crosses the boundary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Abi {
    Int,
    Float,
    Bool,
    Str,
    Unit,
}

impl Abi {
    /// The type the shim declares, in Rust.
    pub fn rust_type(self) -> &'static str {
        match self {
            // Korben marshals a boolean as an integer, so the shim takes one.
            Abi::Int | Abi::Bool => "i64",
            Abi::Float => "f64",
            Abi::Str => "*const ::std::os::raw::c_char",
            Abi::Unit => "()",
        }
    }

    /// The C type the Korben declaration names.
    pub fn c_type(self) -> &'static str {
        match self {
            Abi::Int => "CLong",
            Abi::Float => "CDouble",
            Abi::Bool => "CBool",
            Abi::Str => "CStr",
            Abi::Unit => "CVoid",
        }
    }

    /// The Korben type a parameter of this class is written as.
    pub fn korben_type(self) -> &'static str {
        match self {
            Abi::Int => "Int",
            Abi::Float => "Float64",
            Abi::Bool => "Bool",
            Abi::Str => "String",
            Abi::Unit => "Unit",
        }
    }

    /// Parameters of one class go in one register file, and the trampolines
    /// cannot mix them.
    pub fn is_floating(self) -> bool {
        matches!(self, Abi::Float)
    }
}

/// A parameter the shim carries.
#[derive(Clone, Debug)]
pub struct Param {
    /// The name as the Rust function wrote it.
    pub name: String,
    pub abi: Abi,
    /// `String` arrives borrowed and is owned before the call.
    pub owned_string: bool,
}

/// One exported function, read once and rendered twice.
#[derive(Clone, Debug)]
pub struct Signature {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Abi,
    /// The Rust function returns a `Result`.
    pub fallible: bool,
}

impl Signature {
    /// The `extern "C"` symbol both halves resolve through.
    pub fn symbol(&self) -> String {
        format!("korben_export_{}", self.name)
    }
}

/// A function the generator declined, and why.
pub struct Skipped {
    pub text: String,
    pub reason: String,
}

/// What a source file exports, and what it could not.
pub struct Extracted {
    pub exported: Vec<Signature>,
    pub skipped: Vec<Skipped>,
}

/// The symbol the error channel is read through, declared once per module.
pub const LAST_ERROR_SYMBOL: &str = "korben_export_last_error";

/// Read every `#[korben_export]` function in a source file.
pub fn extract(source: &str) -> Extracted {
    let mut exported = Vec::new();
    let mut skipped = Vec::new();
    for header in annotated_headers(source) {
        // `parse` reads up to the body, which an extracted header does not
        // include, so give it an empty one.
        match parse(&format!("{header} {{}}")) {
            Ok(signature) => exported.push(signature),
            Err(reason) => skipped.push(Skipped { text: compact(&header), reason }),
        }
    }
    Extracted { exported, skipped }
}

/// Replace every comment with blanks, so a mention of the attribute in prose is
/// not read as a use of it, and so a `//` inside a string literal survives.
///
/// Comments are blanked rather than removed to keep every other byte where it
/// was, which is what lets the scan below slice the original text.
fn strip_comments(source: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Code,
        Line,
        Block,
        Text(char),
    }
    let bytes: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut state = State::Code;
    let mut index = 0usize;
    while index < bytes.len() {
        let current = bytes[index];
        let next = bytes.get(index + 1).copied();
        match state {
            State::Code => match (current, next) {
                ('/', Some('/')) => {
                    state = State::Line;
                    out.push_str("  ");
                    index += 2;
                }
                ('/', Some('*')) => {
                    state = State::Block;
                    out.push_str("  ");
                    index += 2;
                }
                ('"', _) | ('\'', _) => {
                    state = State::Text(current);
                    out.push(current);
                    index += 1;
                }
                _ => {
                    out.push(current);
                    index += 1;
                }
            },
            State::Line => {
                if current == '\n' {
                    state = State::Code;
                    out.push('\n');
                } else {
                    out.push(' ');
                }
                index += 1;
            }
            State::Block => {
                if current == '*' && next == Some('/') {
                    state = State::Code;
                    out.push_str("  ");
                    index += 2;
                } else {
                    out.push(if current == '\n' { '\n' } else { ' ' });
                    index += 1;
                }
            }
            State::Text(quote) => {
                out.push(current);
                if current == '\\' {
                    // An escape carries the next character with it, so a
                    // closing quote is not read out of `\"`.
                    if let Some(escaped) = next {
                        out.push(escaped);
                        index += 2;
                        continue;
                    }
                }
                if current == quote {
                    state = State::Code;
                }
                index += 1;
            }
        }
    }
    out
}

/// The signature of every annotated item, from the attribute to the body.
fn annotated_headers(source: &str) -> Vec<String> {
    let source = &strip_comments(source);
    let mut headers = Vec::new();
    for (offset, _) in source.match_indices("korben_export") {
        // Walk back over `#[` and any whitespace between its parts, so both
        // `#[korben_export]` and the spaced form a token stream prints match.
        let before = source[..offset].trim_end();
        let Some(before) = before.strip_suffix('[') else { continue };
        if !before.trim_end().ends_with('#') {
            continue;
        }
        let after = &source[offset + "korben_export".len()..];
        let Some(after) = after.trim_start().strip_prefix(']') else { continue };
        match after.find('{') {
            Some(brace) => headers.push(after[..brace].trim().to_string()),
            None => headers.push(after.trim().to_string()),
        }
    }
    headers
}

/// Read one annotated item.
pub fn parse(source: &str) -> Result<Signature, String> {
    let header = match source.find('{') {
        Some(brace) => &source[..brace],
        None => return Err("`#[korben_export]` expects a function".to_string()),
    };
    let Some(start) = find_fn_keyword(header) else {
        return Err("`#[korben_export]` expects a function".to_string());
    };

    // Qualifiers sit between the attributes and `fn`. Looking any earlier would
    // read a doc comment's prose as a qualifier.
    let qualifiers = match header[..start].rfind(']') {
        Some(end) => &header[end + 1..],
        None => &header[..start],
    };
    for qualifier in ["async", "unsafe", "extern", "const"] {
        if qualifiers.split_whitespace().any(|word| word == qualifier) {
            return Err(format!(
                "`#[korben_export]` cannot export an `{qualifier}` function: the adapter \
                 boundary is a plain call"
            ));
        }
    }

    let rest = header[start..].trim_start();
    let Some(open) = rest.find('(') else {
        return Err("`#[korben_export]` expects a function".to_string());
    };
    let name = rest[..open].trim().to_string();
    if name.is_empty() {
        return Err("`#[korben_export]` expects a named function".to_string());
    }
    if name.contains('<') {
        return Err(format!(
            "`{}` is generic; the adapter boundary needs one concrete signature",
            name.split('<').next().unwrap_or(&name).trim()
        ));
    }

    let close = matching(rest, open)
        .ok_or_else(|| "`#[korben_export]` expects a parameter list".to_string())?;
    let params = parse_params(rest[open + 1..close].trim())?;
    let tail = rest[close + 1..].trim();
    let returns = tail.strip_prefix("->").map(str::trim).unwrap_or("");
    let (ret, fallible) = parse_return(returns, &name)?;

    let floating = params.iter().filter(|param| param.abi.is_floating()).count();
    // The C ABI passes integers and floats in different register files, and
    // Korben's trampolines are uniform, so a mixed list has no calling
    // convention to use. Declining beats calling it incorrectly.
    if floating != 0 && floating != params.len() {
        return Err(format!(
            "`{name}` mixes floating-point and other parameters, which the foreign \
             boundary cannot carry; use all `f64` parameters, or none"
        ));
    }
    if floating > 4 {
        return Err(format!("`{name}` takes {floating} floating-point parameters; the limit is 4"));
    }
    if floating == 0 && params.len() > 8 {
        return Err(format!("`{name}` takes {} parameters; the limit is 8", params.len()));
    }

    Ok(Signature { name, params, ret, fallible })
}

/// Where the signature starts, skipping attributes and visibility.
fn find_fn_keyword(header: &str) -> Option<usize> {
    header.match_indices("fn").find_map(|(offset, _)| {
        let before = header[..offset].chars().next_back();
        let after = header[offset + 2..].chars().next();
        let standalone = before.map(|c| !c.is_alphanumeric() && c != '_').unwrap_or(true)
            && after.map(|c| !c.is_alphanumeric() && c != '_').unwrap_or(false);
        standalone.then_some(offset + 2)
    })
}

/// The index of the parenthesis closing the one at `open`.
fn matching(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, character) in text.char_indices().skip(open) {
        if character == '(' {
            depth += 1;
        } else if character == ')' {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

/// Split on commas that are not inside brackets.
fn split_top_level(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for character in text.chars() {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(current.trim().to_string());
                current = String::new();
                continue;
            }
            _ => {}
        }
        current.push(character);
    }
    let last = current.trim();
    if !last.is_empty() {
        parts.push(last.to_string());
    }
    parts
}

fn parse_params(text: &str) -> Result<Vec<Param>, String> {
    let mut params = Vec::new();
    for part in split_top_level(text) {
        if part == "self" || part.starts_with("&self") || part.starts_with("mut self") {
            return Err(
                "`#[korben_export]` cannot export a method; export a free function".to_string()
            );
        }
        let Some((name, ty)) = part.split_once(':') else {
            return Err(format!("parameter `{part}` needs a name and a type"));
        };
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(format!(
                "parameter `{name}` is a pattern; the adapter boundary needs plain names"
            ));
        }
        let (abi, owned_string) = match normalize(ty).as_str() {
            "i64" => (Abi::Int, false),
            "f64" => (Abi::Float, false),
            "bool" => (Abi::Bool, false),
            "&str" => (Abi::Str, false),
            "String" => (Abi::Str, true),
            other => return Err(unsupported(name, other)),
        };
        params.push(Param { name: name.to_string(), abi, owned_string });
    }
    Ok(params)
}

/// The return type, and whether the function is fallible.
fn parse_return(text: &str, name: &str) -> Result<(Abi, bool), String> {
    let ty = normalize(text);
    if ty.is_empty() || ty == "()" {
        return Ok((Abi::Unit, false));
    }
    if let Some(inner) = ty.strip_prefix("Result<").and_then(|rest| rest.strip_suffix('>')) {
        let parts = split_top_level(inner);
        if parts.len() != 2 {
            return Err(format!("`{name}` returns a `Result` the adapter cannot read"));
        }
        // The error type only has to be printable; it reaches Korben as text.
        return Ok((return_abi(&parts[0], name)?, true));
    }
    Ok((return_abi(&ty, name)?, false))
}

fn return_abi(ty: &str, name: &str) -> Result<Abi, String> {
    Ok(match normalize(ty).as_str() {
        "()" => Abi::Unit,
        "i64" => Abi::Int,
        "f64" => Abi::Float,
        "bool" => Abi::Bool,
        "String" => Abi::Str,
        "&str" => {
            return Err(format!(
                "`{name}` returns a borrowed `&str`; return an owned `String`, which the \
                 adapter keeps alive until the next call"
            ))
        }
        other => return Err(unsupported(name, other)),
    })
}

fn unsupported(what: &str, ty: &str) -> String {
    format!(
        "`{what}` uses `{ty}`, which the adapter boundary does not carry; it carries \
         `i64`, `f64`, `bool`, `&str`, and `String`"
    )
}

/// Token trees stringify with spaces around punctuation, and none of the types
/// in the subset contain a meaningful space.
fn normalize(ty: &str) -> String {
    ty.chars().filter(|c| !c.is_whitespace()).collect()
}

/// One line of a declined signature, for the note that reports it.
fn compact(text: &str) -> String {
    let collapsed: Vec<&str> = text.split_whitespace().collect();
    let joined = collapsed.join(" ");
    if joined.len() > 72 {
        format!("{}...", &joined[..69])
    } else {
        joined
    }
}

/// A Rust name as Korben writes it.
pub fn kebab(name: &str) -> String {
    name.replace('_', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_is_read_once_for_both_halves() {
        let signature = parse("pub fn slugify (input : & str) -> String { }").unwrap();
        assert_eq!(signature.name, "slugify");
        assert_eq!(signature.symbol(), "korben_export_slugify");
        assert_eq!(signature.params.len(), 1);
        assert_eq!(signature.params[0].abi, Abi::Str);
        assert!(!signature.params[0].owned_string);
        assert_eq!(signature.ret, Abi::Str);
        assert!(!signature.fallible);
    }

    #[test]
    fn a_result_is_read_as_fallible_whatever_its_error_type() {
        let signature = parse("fn parse (text : String) -> Result < i64 , MyError > { }").unwrap();
        assert!(signature.fallible);
        assert_eq!(signature.ret, Abi::Int);
        assert!(signature.params[0].owned_string);
    }

    #[test]
    fn a_mixed_parameter_list_is_declined() {
        let error = parse("fn scale (text : & str , factor : f64) -> f64 { }").unwrap_err();
        assert!(error.contains("mixes floating-point and other parameters"), "{error}");
    }

    #[test]
    fn the_parameter_limits_are_the_trampolines_limits() {
        let many = (0..9).map(|n| format!("a{n} : i64")).collect::<Vec<_>>().join(" , ");
        let error = parse(&format!("fn wide ({many}) -> i64 {{ }}")).unwrap_err();
        assert!(error.contains("the limit is 8"), "{error}");

        let floats = (0..5).map(|n| format!("a{n} : f64")).collect::<Vec<_>>().join(" , ");
        let error = parse(&format!("fn wide ({floats}) -> f64 {{ }}")).unwrap_err();
        assert!(error.contains("the limit is 4"), "{error}");
    }

    #[test]
    fn what_cannot_cross_is_named_rather_than_guessed_at() {
        let error = parse("fn take (rows : Vec < i64 >) -> i64 { }").unwrap_err();
        assert!(error.contains("Vec<i64>"), "{error}");

        let error = parse("fn borrow (text : & str) -> & str { }").unwrap_err();
        assert!(error.contains("return an owned `String`"), "{error}");

        let error = parse("async fn later (n : i64) -> i64 { }").unwrap_err();
        assert!(error.contains("`async`"), "{error}");

        let error = parse("fn generic < T > (value : T) -> i64 { }").unwrap_err();
        assert!(error.contains("generic"), "{error}");
    }

    #[test]
    fn a_doc_comment_is_not_read_as_a_qualifier() {
        // `# [doc = " ... unsafe ..."]` is what a doc comment becomes, and the
        // word inside it says nothing about the function.
        let signature =
            parse("# [doc = \" Wraps an unsafe call.\"] pub fn wrap (n : i64) -> i64 { }").unwrap();
        assert_eq!(signature.name, "wrap");
    }

    #[test]
    fn the_attribute_named_in_prose_is_not_a_use_of_it() {
        // A crate's own documentation talks about the attribute constantly.
        let source = r#"
//! `#[korben_export]` adds a shim beside each function.

use korben_export::korben_export;

/// See `#[korben_export]` for what crosses.
#[korben_export]
pub fn slugify(input: &str) -> String {
    let separator = "//";
    input.replace(separator, "-")
}
"#;
        let extracted = extract(source);
        assert_eq!(extracted.exported.len(), 1, "{:?}", extracted.exported);
        assert_eq!(extracted.exported[0].name, "slugify");
        assert!(extracted.skipped.is_empty());
    }

    #[test]
    fn a_file_yields_every_annotated_function_and_declines_the_rest() {
        let source = r#"
use korben_export::korben_export;

#[korben_export]
pub fn slugify(input: &str) -> String {
    input.to_lowercase()
}

/// Not exported at all.
pub fn helper(rows: Vec<i64>) -> i64 { rows.len() as i64 }

#[korben_export]
pub fn tally(rows: Vec<i64>) -> i64 {
    rows.len() as i64
}
"#;
        let extracted = extract(source);
        assert_eq!(extracted.exported.len(), 1);
        assert_eq!(extracted.exported[0].name, "slugify");
        assert_eq!(extracted.skipped.len(), 1);
        assert!(
            extracted.skipped[0].reason.contains("Vec<i64>"),
            "{:?}",
            extracted.skipped[0].reason
        );
        assert!(extracted.skipped[0].text.contains("tally"), "{}", extracted.skipped[0].text);
    }
}
