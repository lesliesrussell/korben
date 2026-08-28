//! Writing the Rust half: the `extern "C"` shim `#[korben_export]` adds.

// korben-rm1

use crate::{Abi, Signature};

/// The helper that contains failure and panic for a return of this class.
fn helper(ret: Abi) -> &'static str {
    match ret {
        Abi::Int => "call_int",
        Abi::Float => "call_float",
        Abi::Bool => "call_bool",
        Abi::Str => "call_str",
        Abi::Unit => "call_unit",
    }
}

/// Write the original item back, followed by its shim.
pub fn render(source: &str, signature: &Signature) -> String {
    let name = &signature.name;
    let declared: Vec<String> = signature
        .params
        .iter()
        .enumerate()
        .map(|(index, param)| format!("p{index}: {}", param.abi.rust_type()))
        .collect();
    let returns = match signature.ret {
        Abi::Unit => String::new(),
        other => format!(" -> {}", other.rust_type()),
    };

    let mut body = String::new();
    for (index, param) in signature.params.iter().enumerate() {
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

    let arguments: Vec<String> =
        (0..signature.params.len()).map(|index| format!("a{index}")).collect();
    let call = format!("{name}({})", arguments.join(", "));
    // A fallible function's error only has to be printable: anything that
    // implements `Display` reaches Korben as the message on the error channel.
    let outcome = match (signature.fallible, signature.ret) {
        (true, Abi::Unit) => {
            format!(
                "{call}.map(|_| ()).map_err(|error| ::std::string::ToString::to_string(&error))"
            )
        }
        (true, _) => format!("{call}.map_err(|error| ::std::string::ToString::to_string(&error))"),
        (false, Abi::Unit) => format!("{{ {call}; Ok(()) }}"),
        (false, _) => format!("Ok({call})"),
    };

    format!(
        "{source}\n\
         /// The adapter shim. Any string argument must be a valid, \
         NUL-terminated pointer that outlives the call.\n\
         #[no_mangle]\n\
         pub unsafe extern \"C\" fn {}({}){} {{\n\
         \x20   ::korben_export::{}(move || {{\n\
         {body}        {outcome}\n\
         \x20   }})\n\
         }}\n",
        signature.symbol(),
        declared.join(", "),
        returns,
        helper(signature.ret),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn shim(source: &str) -> String {
        let signature = parse(source).unwrap_or_else(|error| panic!("{error}"));
        render(source, &signature)
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
    fn a_return_type_may_differ_in_class_from_the_parameters() {
        // Only the parameter list has to be uniform; the return is separate.
        let rendered = shim("fn measure (text : & str) -> f64 { 0.0 }");
        assert!(rendered.contains("::korben_export::call_float"), "{rendered}");
    }

    #[test]
    fn attributes_and_doc_comments_survive() {
        let rendered = shim(
            "# [doc = \" Slugify.\"] pub fn slugify (input : & str) -> String { input.into () }",
        );
        assert!(rendered.contains("# [doc = \" Slugify.\"]"), "{rendered}");
        assert!(rendered.contains("korben_export_slugify"), "{rendered}");
    }
}
