//! Reading an annotated signature, and writing the shim for it.
//!
//! This is deliberately separate from the attribute itself, and works on the
//! item's source text rather than on token trees, for two reasons. A proc-macro
//! crate cannot run its own macro, so logic that stays behind `proc_macro`
//! types cannot be tested at all; and the toolchain already reads a restricted
//! subset of C this way, in `cheader.rs`, declining what it cannot type rather
//! than guessing. The cost is that a rejection points at the whole item instead
//! of the offending token.

// korben-10s

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
    /// The C type Korben marshals this as.
    fn c_type(self) -> &'static str {
        match self {
            // Korben passes a boolean as an integer, so the shim takes one.
            Abi::Int | Abi::Bool => "i64",
            Abi::Float => "f64",
            Abi::Str => "*const ::std::os::raw::c_char",
            Abi::Unit => "()",
        }
    }

    /// The helper that contains failure and panic for this return type.
    fn helper(self) -> &'static str {
        match self {
            Abi::Int => "call_int",
            Abi::Float => "call_float",
            Abi::Bool => "call_bool",
            Abi::Str => "call_str",
            Abi::Unit => "call_unit",
        }
    }

    /// Parameters of one class go in one register file, and the trampolines
    /// cannot mix them.
    fn is_floating(self) -> bool {
        matches!(self, Abi::Float)
    }
}

/// A parameter the shim carries.
struct Param {
    abi: Abi,
    /// `String` arrives as a borrowed `&str` and is owned before the call.
    owned_string: bool,
}

/// Write the original item back, followed by its shim.
pub fn generate(source: &str) -> Result<String, String> {
    let signature = split_signature(source)?;
    let name = signature.name;
    let params = parse_params(&signature.params)?;

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

    let (ret, fallible) = parse_return(&signature.ret, &name)?;

    let declared: Vec<String> = params
        .iter()
        .enumerate()
        .map(|(index, param)| format!("p{index}: {}", param.abi.c_type()))
        .collect();
    let returns = match ret {
        Abi::Unit => String::new(),
        other => format!(" -> {}", other.c_type()),
    };

    let mut body = String::new();
    for (index, param) in params.iter().enumerate() {
        let bound = match param.abi {
            Abi::Str if param.owned_string => format!(
                "let a{index} = unsafe {{ ::korben_export::borrowed_str(p{index}) }}?.to_string();"
            ),
            Abi::Str => {
                format!("let a{index} = unsafe {{ ::korben_export::borrowed_str(p{index}) }}?;")
            }
            Abi::Bool => format!("let a{index} = p{index} != 0;"),
            _ => format!("let a{index} = p{index};"),
        };
        body.push_str("        ");
        body.push_str(&bound);
        body.push('\n');
    }

    let arguments: Vec<String> = (0..params.len()).map(|index| format!("a{index}")).collect();
    let call = format!("{name}({})", arguments.join(", "));
    // A fallible function's error only has to be printable: anything that
    // implements `Display` reaches Korben as the message on the error channel.
    let outcome = match (fallible, ret) {
        (true, Abi::Unit) => format!(
            "{call}.map(|_| ()).map_err(|error| ::std::string::ToString::to_string(&error))"
        ),
        (true, _) => {
            format!("{call}.map_err(|error| ::std::string::ToString::to_string(&error))")
        }
        (false, Abi::Unit) => format!("{{ {call}; Ok(()) }}"),
        (false, _) => format!("Ok({call})"),
    };

    Ok(format!(
        "{source}\n\
         /// The adapter shim. Any string argument must be a valid, \
         NUL-terminated pointer that outlives the call.\n\
         #[no_mangle]\n\
         pub unsafe extern \"C\" fn korben_export_{name}({}){} {{\n\
         \x20   ::korben_export::{}(move || {{\n\
         {body}        {outcome}\n\
         \x20   }})\n\
         }}\n",
        declared.join(", "),
        returns,
        ret.helper(),
    ))
}

/// The parts of the signature the shim is written from.
struct Signature {
    name: String,
    params: String,
    ret: String,
}

/// Split the item into a name, a parameter list, and a return type.
fn split_signature(source: &str) -> Result<Signature, String> {
    let header = match source.find('{') {
        Some(brace) => &source[..brace],
        None => return Err("`#[korben_export]` expects a function".to_string()),
    };
    for qualifier in ["async", "unsafe", "extern", "const"] {
        if header.split_whitespace().any(|word| word == qualifier) {
            return Err(format!(
                "`#[korben_export]` cannot export an `{qualifier}` function: the adapter \
                 boundary is a plain call"
            ));
        }
    }
    let Some(start) = find_fn_keyword(header) else {
        return Err("`#[korben_export]` expects a function".to_string());
    };
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

    let close = matching(rest, open, '(', ')')
        .ok_or_else(|| "`#[korben_export]` expects a parameter list".to_string())?;
    let params = rest[open + 1..close].trim().to_string();
    let tail = rest[close + 1..].trim();
    let ret = match tail.strip_prefix("->") {
        Some(ret) => ret.trim().to_string(),
        None => String::new(),
    };
    Ok(Signature { name, params, ret })
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

/// The index of the delimiter closing the one at `open`.
fn matching(text: &str, open: usize, opening: char, closing: char) -> Option<usize> {
    let mut depth = 0usize;
    for (index, character) in text.char_indices().skip(open) {
        if character == opening {
            depth += 1;
        } else if character == closing {
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
        if !name.chars().all(|c| c.is_alphanumeric() || c == '_') || name.is_empty() {
            return Err(format!(
                "parameter `{name}` is a pattern; the adapter boundary needs plain names"
            ));
        }
        let ty = normalize(ty);
        let (abi, owned_string) = match ty.as_str() {
            "i64" => (Abi::Int, false),
            "f64" => (Abi::Float, false),
            "bool" => (Abi::Bool, false),
            "&str" => (Abi::Str, false),
            "String" => (Abi::Str, true),
            other => return Err(unsupported(name, other)),
        };
        params.push(Param { abi, owned_string });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn shim(source: &str) -> String {
        generate(source).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn a_string_function_becomes_a_c_string_shim() {
        let rendered = shim("pub fn slugify (input : & str) -> String { input.to_string () }");
        assert!(
            rendered.contains("pub unsafe extern \"C\" fn korben_export_slugify"),
            "{rendered}"
        );
        assert!(rendered.contains("p0: *const ::std::os::raw::c_char"), "{rendered}");
        assert!(rendered.contains("-> *const ::std::os::raw::c_char"), "{rendered}");
        assert!(rendered.contains("::korben_export::call_str"), "{rendered}");
        assert!(rendered.contains("borrowed_str(p0)"), "{rendered}");
        assert!(rendered.contains("Ok(slugify(a0))"), "{rendered}");
        // The original function is written back untouched.
        assert!(rendered.contains("pub fn slugify (input : & str) -> String"), "{rendered}");
    }

    #[test]
    fn a_fallible_function_reports_through_the_error_channel() {
        let rendered = shim("fn parse (text : & str) -> Result < i64 , String > { todo ! () }");
        assert!(rendered.contains("::korben_export::call_int"), "{rendered}");
        assert!(rendered.contains("map_err"), "{rendered}");
        assert!(rendered.contains("-> i64"), "{rendered}");
    }

    #[test]
    fn an_owned_string_parameter_is_owned_before_the_call() {
        let rendered = shim("fn shout (text : String) -> String { text }");
        assert!(rendered.contains("borrowed_str(p0) }?.to_string();"), "{rendered}");
    }

    #[test]
    fn a_boolean_crosses_as_an_integer() {
        let rendered = shim("fn toggle (flag : bool) -> bool { ! flag }");
        assert!(rendered.contains("p0: i64"), "{rendered}");
        assert!(rendered.contains("let a0 = p0 != 0;"), "{rendered}");
        assert!(rendered.contains("::korben_export::call_bool"), "{rendered}");
        assert!(rendered.contains("-> i64"), "{rendered}");
    }

    #[test]
    fn a_function_returning_nothing_needs_no_return_type() {
        let rendered = shim("fn log (text : & str) { let _ = text ; }");
        assert!(
            rendered.contains("fn korben_export_log(p0: *const ::std::os::raw::c_char) {"),
            "{rendered}"
        );
        assert!(rendered.contains("::korben_export::call_unit"), "{rendered}");
        assert!(rendered.contains("{ log(a0); Ok(()) }"), "{rendered}");
    }

    #[test]
    fn floating_parameters_are_allowed_on_their_own() {
        let rendered = shim("fn scale (value : f64 , factor : f64) -> f64 { value * factor }");
        assert!(rendered.contains("p0: f64, p1: f64"), "{rendered}");
        assert!(rendered.contains("::korben_export::call_float"), "{rendered}");
    }

    #[test]
    fn a_mixed_parameter_list_is_declined() {
        let error = generate("fn scale (text : & str , factor : f64) -> f64 { 0.0 }").unwrap_err();
        assert!(error.contains("mixes floating-point and other parameters"), "{error}");
    }

    #[test]
    fn a_return_type_may_differ_in_class_from_the_parameters() {
        // Only the parameter list has to be uniform; the return is separate.
        let rendered = shim("fn measure (text : & str) -> f64 { 0.0 }");
        assert!(rendered.contains("::korben_export::call_float"), "{rendered}");
    }

    #[test]
    fn the_parameter_limits_are_the_trampolines_limits() {
        let many = (0..9).map(|n| format!("a{n} : i64")).collect::<Vec<_>>().join(" , ");
        let error = generate(&format!("fn wide ({many}) -> i64 {{ 0 }}")).unwrap_err();
        assert!(error.contains("the limit is 8"), "{error}");

        let floats = (0..5).map(|n| format!("a{n} : f64")).collect::<Vec<_>>().join(" , ");
        let error = generate(&format!("fn wide ({floats}) -> f64 {{ 0.0 }}")).unwrap_err();
        assert!(error.contains("the limit is 4"), "{error}");
    }

    #[test]
    fn what_cannot_cross_is_named_rather_than_guessed_at() {
        let error = generate("fn take (rows : Vec < i64 >) -> i64 { 0 }").unwrap_err();
        assert!(error.contains("Vec<i64>"), "{error}");
        assert!(error.contains("does not carry"), "{error}");

        let error = generate("fn borrow (text : & str) -> & str { text }").unwrap_err();
        assert!(error.contains("return an owned `String`"), "{error}");

        let error = generate("async fn later (n : i64) -> i64 { n }").unwrap_err();
        assert!(error.contains("`async`"), "{error}");

        let error = generate("fn generic < T > (value : T) -> i64 { 0 }").unwrap_err();
        assert!(error.contains("generic"), "{error}");
    }

    #[test]
    fn attributes_and_doc_comments_survive() {
        let rendered = shim("# [doc = \" Slugify.\"] pub fn slugify (input : & str) -> String { input.to_string () }");
        assert!(rendered.contains("# [doc = \" Slugify.\"]"), "{rendered}");
        assert!(rendered.contains("korben_export_slugify"), "{rendered}");
    }
}
