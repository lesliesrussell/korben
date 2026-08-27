//! Generating typed bindings from C function prototypes.
//!
//! Specification 17.2 describes a binding generator that consumes C headers.
//! This one reads the declaration subset that appears in ordinary library
//! headers — prototypes over built-in types and pointers — and emits a Korben
//! module of `(ffi/c-fn ...)` declarations. Anything it cannot type is skipped
//! with a note rather than guessed at, because a wrong binding is worse than a
//! missing one.
//!
//! Extraction through libclang, which would handle typedefs, structs, and
//! macros, is future work; this covers the common case without adding a
//! toolchain dependency.

// korben-v3q

use korben_runtime::ffi::CType;

/// One prototype the generator understood.
pub struct Binding {
    /// The C symbol.
    pub symbol: String,
    /// The Korben name, in kebab case.
    pub name: String,
    pub params: Vec<(String, CType)>,
    pub ret: CType,
}

/// A prototype the generator declined to translate, and why.
pub struct Skipped {
    pub text: String,
    pub reason: String,
}

pub struct Extracted {
    pub bindings: Vec<Binding>,
    pub skipped: Vec<Skipped>,
}

/// Extract every prototype the generator can type.
pub fn extract(source: &str) -> Extracted {
    let cleaned = strip_noise(source);
    let mut bindings = Vec::new();
    let mut skipped = Vec::new();

    for declaration in cleaned.split(';') {
        let text = declaration.trim();
        if text.is_empty() || !text.contains('(') {
            continue;
        }
        match translate(text) {
            Ok(Some(binding)) => bindings.push(binding),
            Ok(None) => {}
            Err(reason) => skipped.push(Skipped { text: compact(text), reason }),
        }
    }
    Extracted { bindings, skipped }
}

/// Remove comments and preprocessor lines, which this parser does not model.
fn strip_noise(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut index = 0usize;
    let mut at_line_start = true;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"/*") {
            match source[index + 2..].find("*/") {
                Some(end) => index += 2 + end + 2,
                None => break,
            }
            out.push(' ');
            continue;
        }
        if bytes[index..].starts_with(b"//") {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if at_line_start && bytes[index] == b'#' {
            // A continued directive keeps going past the newline.
            while index < bytes.len() {
                if bytes[index] == b'\n' && (index == 0 || bytes[index - 1] != b'\\') {
                    break;
                }
                index += 1;
            }
            continue;
        }
        at_line_start = bytes[index] == b'\n';
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

fn compact(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Translate one declaration, or explain why it cannot be.
fn translate(text: &str) -> Result<Option<Binding>, String> {
    let Some(open) = text.find('(') else { return Ok(None) };
    let Some(close) = text.rfind(')') else { return Ok(None) };
    if close < open {
        return Ok(None);
    }
    let head = text[..open].trim();
    let body = &text[open + 1..close];

    // A declaration whose name is parenthesized is a function pointer.
    if head.ends_with('*') && head.contains('(') {
        return Err("function pointer".to_string());
    }
    if text[close + 1..].trim_start().starts_with('(') {
        return Err("function returning a function pointer".to_string());
    }

    let Some(split) = head.rfind(|ch: char| !(ch.is_alphanumeric() || ch == '_')) else {
        return Ok(None);
    };
    let symbol = head[split + 1..].trim();
    if symbol.is_empty() || symbol.chars().next().unwrap().is_ascii_digit() {
        return Ok(None);
    }
    let return_type = head[..=split].trim();
    if return_type.is_empty() {
        return Ok(None);
    }
    if return_type.starts_with("typedef") {
        return Err("typedef".to_string());
    }

    let ret =
        c_type(return_type).ok_or_else(|| format!("unsupported return type `{return_type}`"))?;

    let mut params = Vec::new();
    let trimmed = body.trim();
    if trimmed.contains("...") {
        return Err("variadic".to_string());
    }
    // A parenthesis inside the parameter list means a function pointer, whose
    // calling convention this backend does not model.
    if trimmed.contains('(') {
        return Err("function pointer parameter".to_string());
    }
    if !trimmed.is_empty() && trimmed != "void" {
        for (index, part) in trimmed.split(',').enumerate() {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (name, type_text) = split_parameter(part, index);
            let ty = c_type(&type_text)
                .ok_or_else(|| format!("unsupported parameter type `{}`", compact(&type_text)))?;
            if ty == CType::Void {
                return Err("`void` parameter".to_string());
            }
            params.push((name, ty));
        }
    }

    Ok(Some(Binding { symbol: symbol.to_string(), name: symbol.replace('_', "-"), params, ret }))
}

/// Split `const char *name` into a parameter name and its type text.
fn split_parameter(text: &str, index: usize) -> (String, String) {
    let fallback = format!("a{index}");
    let trimmed = text.trim_end_matches(' ');
    // An unnamed parameter ends in a type token or a `*`.
    let Some(split) = trimmed.rfind(|ch: char| !(ch.is_alphanumeric() || ch == '_')) else {
        return (fallback, trimmed.to_string());
    };
    let tail = trimmed[split + 1..].trim();
    let head = trimmed[..=split].trim();
    if tail.is_empty() || head.is_empty() || is_type_word(tail) {
        return (fallback, trimmed.to_string());
    }
    (sanitize(tail, index), head.to_string())
}

fn sanitize(name: &str, index: usize) -> String {
    let cleaned = name.replace('_', "-");
    if cleaned.is_empty() || cleaned.chars().next().unwrap().is_ascii_digit() {
        return format!("a{index}");
    }
    cleaned
}

fn is_type_word(word: &str) -> bool {
    matches!(
        word,
        "void"
            | "char"
            | "short"
            | "int"
            | "long"
            | "float"
            | "double"
            | "signed"
            | "unsigned"
            | "size_t"
            | "ssize_t"
            | "intptr_t"
            | "uintptr_t"
            | "bool"
            | "_Bool"
    )
}

/// Map a C type to the closest Korben C type, or `None` when it is not modeled.
fn c_type(text: &str) -> Option<CType> {
    let normalized = compact(text);
    let words: Vec<&str> = normalized
        .split_whitespace()
        .filter(|word| {
            !matches!(
                *word,
                "const" | "volatile" | "restrict" | "extern" | "static" | "inline" | "__restrict"
            )
        })
        .collect();
    let joined = words.join(" ");
    let stars = joined.matches('*').count();
    let base = joined.replace('*', "");
    let base = base.trim();

    if stars > 0 {
        // A character pointer is a string; anything else, including a pointer
        // to an opaque struct, is carried as a pointer without dereferencing.
        return Some(if stars == 1 && (base == "char" || base == "signed char") {
            CType::Str
        } else {
            CType::Ptr
        });
    }
    // A struct passed or returned by value needs a layout this backend has no
    // way to know, so it is declined rather than guessed at.
    if base.starts_with("struct ") || base.starts_with("union ") || base.starts_with("enum ") {
        return None;
    }

    Some(match base {
        "void" => CType::Void,
        "bool" | "_Bool" => CType::Bool,
        "char" | "signed char" => CType::Char,
        "unsigned char" => CType::UInt,
        "short" | "short int" | "signed short" | "int" | "signed" | "signed int" => CType::Int,
        "unsigned" | "unsigned int" | "unsigned short" => CType::UInt,
        "long" | "long int" | "long long" | "long long int" | "ssize_t" | "intptr_t"
        | "ptrdiff_t" | "int64_t" => CType::Long,
        "unsigned long" | "unsigned long long" | "size_t" | "uintptr_t" | "uint64_t" => {
            CType::ULong
        }
        "int32_t" => CType::Int,
        "uint32_t" => CType::UInt,
        "float" => CType::Float,
        "double" | "long double" => CType::Double,
        _ => return None,
    })
}

/// Render extracted bindings as a Korben module.
pub fn render(module: &str, library: &str, header: &str, extracted: &Extracted) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, ";;; Bindings for `{library}`, generated from `{header}`.");
    let _ = writeln!(out, ";;;");
    let _ = writeln!(out, ";;; Generated by `korben ffi c`. Edits are lost on regeneration.");
    let _ = writeln!(out, ";;; Every declaration here is an `unsafe fn`: wrap them in a safe");
    let _ = writeln!(out, ";;; Korben API before using them elsewhere.");
    let _ = writeln!(out);
    let _ = writeln!(out, "(module {module})");
    let _ = writeln!(out);
    let _ = writeln!(out, "(ffi/c-library \"{library}\")");
    let _ = writeln!(out);

    for binding in &extracted.bindings {
        let params: Vec<String> =
            binding.params.iter().map(|(name, ty)| format!("{name}: {}", type_name(*ty))).collect();
        // Name the symbol explicitly when it is not the obvious translation.
        let derived = binding.name.replace('-', "_");
        let symbol = if derived == binding.symbol {
            String::new()
        } else {
            format!(" \"{}\"", binding.symbol)
        };
        let _ = writeln!(
            out,
            "(pub ffi/c-fn {}{symbol} [{}] -> {})",
            binding.name,
            params.join(" "),
            type_name(binding.ret)
        );
    }

    if !extracted.skipped.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, ";; Not translated:");
        for skipped in &extracted.skipped {
            let _ = writeln!(out, ";;   {} — {}", skipped.text, skipped.reason);
        }
    }
    out
}

fn type_name(ty: CType) -> &'static str {
    match ty {
        CType::Void => "CVoid",
        CType::Bool => "CBool",
        CType::Char => "CChar",
        CType::Int => "CInt",
        CType::UInt => "CUInt",
        CType::Long => "CLong",
        CType::ULong => "CULong",
        CType::Float => "CFloat",
        CType::Double => "CDouble",
        CType::Str => "CStr",
        CType::Ptr => "Ptr",
    }
}
