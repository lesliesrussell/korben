//! `#[korben_export]` exercised the way a Rust library would use it.
//!
//! A proc-macro crate cannot run its own macro, so this is where the attribute
//! is actually applied and the symbols it generates are actually called. The
//! shims here are the ones a Korben binding module would resolve through
//! `dlsym`; calling them directly proves the same thing without a dynamic
//! library in the way.

// korben-10s

use korben_export::korben_export;
use std::ffi::{CStr, CString};

#[korben_export]
/// Ordinary Rust, still callable as ordinary Rust.
pub fn slugify(input: &str) -> String {
    input.trim().to_lowercase().replace(' ', "-")
}

#[korben_export]
pub fn add(left: i64, right: i64) -> i64 {
    left + right
}

#[korben_export]
pub fn scale(value: f64, factor: f64) -> f64 {
    value * factor
}

#[korben_export]
pub fn toggle(flag: bool) -> bool {
    !flag
}

#[korben_export]
pub fn parse_port(text: String) -> Result<i64, String> {
    text.parse::<i64>().map_err(|error| format!("`{text}` is not a port: {error}"))
}

#[korben_export]
pub fn fall_over(text: &str) -> i64 {
    panic!("cannot handle {text}");
}

fn last_error() -> Option<String> {
    let pointer = korben_export::korben_export_last_error();
    if pointer.is_null() {
        return None;
    }
    Some(unsafe { CStr::from_ptr(pointer) }.to_string_lossy().to_string())
}

fn c(text: &str) -> CString {
    CString::new(text).expect("a test string without a zero byte")
}

#[test]
fn the_annotated_function_is_still_ordinary_rust() {
    assert_eq!(slugify("Korben Is Fast"), "korben-is-fast");
    assert_eq!(add(2, 3), 5);
}

#[test]
fn a_string_crosses_and_comes_back() {
    let input = c("  Korben Is Fast  ");
    let returned = unsafe { korben_export_slugify(input.as_ptr()) };
    assert!(!returned.is_null());
    let text = unsafe { CStr::from_ptr(returned) }.to_string_lossy().to_string();
    assert_eq!(text, "korben-is-fast");
    assert_eq!(last_error(), None);
}

#[test]
fn scalars_cross_in_their_own_register_classes() {
    assert_eq!(unsafe { korben_export_add(2, 3) }, 5);
    assert_eq!(unsafe { korben_export_scale(1.5, 4.0) }, 6.0);
    // A boolean crosses as the integer Korben marshals it to.
    assert_eq!(unsafe { korben_export_toggle(0) }, 1);
    assert_eq!(unsafe { korben_export_toggle(1) }, 0);
}

#[test]
fn a_failure_arrives_on_the_error_channel() {
    let good = c("8080");
    assert_eq!(unsafe { korben_export_parse_port(good.as_ptr()) }, 8080);
    assert_eq!(last_error(), None);

    let bad = c("http");
    assert_eq!(unsafe { korben_export_parse_port(bad.as_ptr()) }, 0);
    let message = last_error().expect("a failure should be reported");
    assert!(message.contains("`http` is not a port"), "{message}");
}

#[test]
fn a_panic_does_not_cross_the_boundary() {
    let input = c("this");
    assert_eq!(unsafe { korben_export_fall_over(input.as_ptr()) }, 0);
    let message = last_error().expect("a panic should be reported");
    assert!(message.contains("the adapter panicked"), "{message}");
    assert!(message.contains("cannot handle this"), "{message}");
}
