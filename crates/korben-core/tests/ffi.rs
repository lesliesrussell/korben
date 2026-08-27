//! Foreign declarations: typing, safety, and the binding generator.

mod common;
use common::{check, run};

const LIBC: &str = r#"(module m)

(ffi/c-library "c")
(ffi/c-fn strlen [text: CStr] -> CULong)
(ffi/c-fn abs [value: CInt] -> CInt)
(ffi/c-fn getenv [name: CStr] -> CStr)
(ffi/c-fn pow [base: CDouble exponent: CDouble] -> CDouble)
"#;

#[test]
fn a_foreign_function_can_be_called_from_unsafe_code() {
    let result = run(&format!(
        "{LIBC}
(pub fn main [] -> Unit !io !ffi !unsafe
  (println (unsafe (strlen \"korben\")))
  (println (unsafe (abs -7)))
  (println (unsafe (pow 2.0 8.0))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "6\n7\n256.0\n");
}

#[test]
fn a_foreign_function_cannot_be_called_from_safe_code() {
    let codes = check(&format!(
        "{LIBC}
(fn length [text: String] -> Int (strlen text))"
    ));
    assert_eq!(codes, vec!["unsafe-call"]);
}

#[test]
fn a_safe_wrapper_lets_callers_stay_safe() {
    // The wrapper still declares the effects, per specification 22.3, but its
    // callers do not need `unsafe`.
    let result = run(&format!(
        "{LIBC}
(fn length [text: String] -> Int !ffi !unsafe (unsafe (strlen text)))
(pub fn main [] -> Unit !io !ffi !unsafe (println (length \"abcd\")))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "4\n");
}

#[test]
fn a_nullable_c_string_surfaces_as_an_option() {
    std::env::set_var("KORBEN_FFI_LANG_TEST", "yes");
    let result = run(&format!(
        "{LIBC}
(pub fn main [] -> Unit !io !ffi !unsafe
  (println (unsafe (getenv \"KORBEN_FFI_LANG_TEST\")))
  (println (unsafe (getenv \"KORBEN_FFI_MISSING_XYZ\"))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "(Some \"yes\")\n(None)\n");
}

#[test]
fn foreign_signatures_are_type_checked() {
    let codes = check(&format!(
        "{LIBC}
(fn bad [] -> Int !ffi !unsafe (unsafe (abs \"not a number\")))"
    ));
    assert_eq!(codes, vec!["type-mismatch"]);
}

#[test]
fn undeclared_ffi_effects_are_reported() {
    let codes = check(&format!(
        "{LIBC}
(pub fn length [text: String] -> Int (unsafe (strlen text)))"
    ));
    assert_eq!(codes, vec!["undeclared-effect"]);
}

#[test]
fn a_declaration_without_a_library_is_rejected() {
    let codes = check("(module m)\n(ffi/c-fn abs [value: CInt] -> CInt)");
    assert_eq!(codes, vec!["ffi-library"]);
}

#[test]
fn an_unsupported_c_type_is_rejected() {
    let codes = check("(module m)\n(ffi/c-library \"c\")\n(ffi/c-fn f [x: Widget] -> CInt)");
    assert_eq!(codes, vec!["ffi-type"]);
}

#[test]
fn a_missing_symbol_is_reported_at_the_call_site() {
    let result = run("(module m)
(ffi/c-library \"c\")
(ffi/c-fn nope \"korben_definitely_missing\" [] -> CInt)
(pub fn main [] -> Unit !io !ffi !unsafe (println (unsafe (nope))))");
    assert_eq!(result.diagnostics, vec!["ffi-symbol"]);
}

// ------------------------------------------------------- binding generation

const HEADER: &str = r#"/* A header in the shape real ones take. */
#ifndef DEMO_H
#define DEMO_H
#include <stddef.h>

size_t strlen(const char *s);
int abs(int value);
char *getenv(const char *name);
double pow(double base, double exponent);
void demo_reset(void);
void *demo_alloc(size_t bytes, unsigned flags);
struct demo_state *demo_open(const char *path);   // an opaque handle

int printf(const char *format, ...);              // variadic
void demo_on_event(void (*callback)(int));        // function pointer
struct demo_point demo_origin(void);              // struct by value
#endif
"#;

#[test]
fn the_generator_translates_ordinary_prototypes() {
    let extracted = korben_core::cheader::extract(HEADER);
    let names: Vec<&str> = extracted.bindings.iter().map(|binding| binding.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["strlen", "abs", "getenv", "pow", "demo-reset", "demo-alloc", "demo-open"]
    );

    let rendered = korben_core::cheader::render("demo", "c", "demo.h", &extracted);
    assert!(rendered.contains("(ffi/c-library \"c\")"), "{rendered}");
    assert!(rendered.contains("(pub ffi/c-fn strlen [s: CStr] -> CULong)"), "{rendered}");
    assert!(rendered.contains("(pub ffi/c-fn pow [base: CDouble exponent: CDouble] -> CDouble)"));
    assert!(rendered.contains("(pub ffi/c-fn demo-reset [] -> CVoid)"), "{rendered}");
    // An opaque struct pointer is carried without being dereferenced.
    assert!(rendered.contains("(pub ffi/c-fn demo-open [path: CStr] -> Ptr)"), "{rendered}");
}

#[test]
fn the_generator_declines_what_it_cannot_type() {
    let extracted = korben_core::cheader::extract(HEADER);
    let reasons: Vec<&str> =
        extracted.skipped.iter().map(|skipped| skipped.reason.as_str()).collect();
    assert!(reasons.contains(&"variadic"), "{reasons:?}");
    assert!(reasons.contains(&"function pointer parameter"), "{reasons:?}");
    assert!(
        reasons.iter().any(|reason| reason.contains("unsupported return type")),
        "a struct returned by value has no known layout: {reasons:?}"
    );
}

#[test]
fn generated_bindings_compile_and_run() {
    // The whole point of criterion 8: generate, then actually use them.
    let extracted = korben_core::cheader::extract("size_t strlen(const char *s);\nint abs(int v);");
    let module = korben_core::cheader::render("bindings", "c", "test.h", &extracted);
    let result = run(&format!(
        "{module}
(pub fn main [] -> Unit !io !ffi !unsafe
  (println (unsafe (strlen \"generated\")))
  (println (unsafe (abs -3))))"
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output, "9\n3\n");
}
