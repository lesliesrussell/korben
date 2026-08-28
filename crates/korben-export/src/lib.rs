//! The Rust side of the Korben adapter ABI.
//!
//! Specification 17.3 describes consuming a Rust library from Korben through an
//! adapter rather than by calling arbitrary Rust APIs directly. This crate is
//! the adapter's runtime half: the helpers a shim function is written against,
//! by hand or by `#[korben_export]`.
//!
//! It rides the C boundary Korben already speaks instead of inventing a second
//! one, because two facts about that boundary rule out anything richer. A
//! foreign signature must be all-integer or all-floating, so a shim cannot
//! return a struct by value and cannot mix an `f64` with an `i64`. And a
//! returned `CStr` is copied into a Korben string and never freed by Korben,
//! valid only until it is copied -- which is exactly what a thread-local
//! buffer provides, with no allocator crossing the boundary in either
//! direction.
//!
//! Failure and panic share one channel. Each `call_*` helper clears the error,
//! runs the body, and on `Err` or panic records a message and returns a zero or
//! null. The caller asks [`korben_export_last_error`] afterwards rather than
//! reading the return value, so a function that legitimately returns `0` is not
//! mistaken for a failure.
//!
//! The panic boundary is not optional: Korben's release profile sets
//! `panic = "abort"`, and an unwind across `extern "C"` is undefined behaviour.

// korben-4ka

use std::any::Any;
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

thread_local! {
    /// The message from the most recent call, cleared as each one begins.
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
    /// The string most recently handed back, kept alive until the next call.
    static RETURNED: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// The message from the most recent failed call, or null after one that
/// succeeded. The pointer stays valid until the next call on this thread.
///
/// This is the error channel for every adapter function: a shim reports failure
/// by leaving a message here, not by its return value.
#[no_mangle]
pub extern "C" fn korben_export_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| match slot.borrow().as_ref() {
        Some(message) => message.as_ptr(),
        None => std::ptr::null(),
    })
}

/// Record a failure for the call in progress.
pub fn set_error(message: &str) {
    // A message containing a zero byte cannot be carried as a C string, and
    // saying so beats reporting no error at all for a call that did fail.
    let carried = CString::new(message)
        .unwrap_or_else(|_| CString::new("the error message contains a zero byte").unwrap());
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(carried));
}

/// Forget the previous call's failure.
pub fn clear_error() {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
}

/// Hand a string back across the boundary.
///
/// The pointer stays valid until the next call on this thread, which is long
/// enough: Korben copies a returned string as it unmarshals it.
pub fn returned_str(text: &str) -> *const c_char {
    match CString::new(text) {
        Ok(owned) => RETURNED.with(|slot| {
            let mut slot = slot.borrow_mut();
            *slot = Some(owned);
            // The `CString` is owned by the thread-local, which outlives the
            // call, so handing out its pointer does not dangle.
            slot.as_ref().map(|owned| owned.as_ptr()).unwrap_or(std::ptr::null())
        }),
        Err(_) => {
            set_error("the returned string contains a zero byte");
            std::ptr::null()
        }
    }
}

/// Read an argument Korben passed as a C string.
///
/// # Safety
///
/// `pointer` must be null or point to a NUL-terminated string that stays valid
/// for the duration of the call. Korben's marshalling upholds this: it keeps
/// the `CString` it built alive until the call returns.
pub unsafe fn borrowed_str<'a>(pointer: *const c_char) -> Result<&'a str, String> {
    if pointer.is_null() {
        return Err("a string argument was null".to_string());
    }
    CStr::from_ptr(pointer)
        .to_str()
        .map_err(|_| "a string argument was not valid UTF-8".to_string())
}

/// Run a shim body that returns an integer, containing failure and panic.
pub fn call_int(body: impl FnOnce() -> Result<i64, String>) -> i64 {
    guard(body).unwrap_or(0)
}

/// Run a shim body that returns a float.
pub fn call_float(body: impl FnOnce() -> Result<f64, String>) -> f64 {
    guard(body).unwrap_or(0.0)
}

/// Run a shim body that returns a boolean, as the C integer Korben reads.
pub fn call_bool(body: impl FnOnce() -> Result<bool, String>) -> i64 {
    guard(body).map(i64::from).unwrap_or(0)
}

/// Run a shim body that returns a string, and hand the result back.
pub fn call_str(body: impl FnOnce() -> Result<String, String>) -> *const c_char {
    match guard(body) {
        Some(text) => returned_str(&text),
        None => std::ptr::null(),
    }
}

/// Run a shim body that returns nothing.
pub fn call_unit(body: impl FnOnce() -> Result<(), String>) {
    let _ = guard(body);
}

/// Clear the error, run `body`, and turn failure or panic into a recorded
/// message and `None`.
fn guard<T>(body: impl FnOnce() -> Result<T, String>) -> Option<T> {
    clear_error();
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(value)) => Some(value),
        Ok(Err(message)) => {
            set_error(&message);
            None
        }
        Err(panic) => {
            set_error(&format!("the adapter panicked: {}", panic_message(&panic)));
            None
        }
    }
}

/// What a caught panic was carrying, for the two payload types a panic can have.
fn panic_message(panic: &Box<dyn Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "no message".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The error channel is thread-local, so a test must not see another's.
    fn last_error() -> Option<String> {
        let pointer = korben_export_last_error();
        if pointer.is_null() {
            return None;
        }
        Some(unsafe { CStr::from_ptr(pointer) }.to_string_lossy().to_string())
    }

    #[test]
    fn a_call_that_succeeds_leaves_no_error() {
        assert_eq!(call_int(|| Ok(7)), 7);
        assert_eq!(last_error(), None);
        assert_eq!(call_float(|| Ok(1.5)), 1.5);
        assert_eq!(last_error(), None);
        assert_eq!(call_bool(|| Ok(true)), 1);
        assert_eq!(call_bool(|| Ok(false)), 0);
        assert_eq!(last_error(), None);
    }

    #[test]
    fn a_failure_is_reported_through_the_error_channel() {
        // The sentinel is not the signal: a call may return zero and succeed.
        assert_eq!(call_int(|| Err("no such row".to_string())), 0);
        assert_eq!(last_error().as_deref(), Some("no such row"));
        assert_eq!(call_int(|| Ok(0)), 0);
        assert_eq!(last_error(), None);
    }

    #[test]
    fn a_panic_is_contained_and_reported() {
        let value = call_int(|| panic!("the adapter fell over"));
        assert_eq!(value, 0);
        let message = last_error().expect("a panic should be reported");
        assert!(message.contains("the adapter panicked"), "{message}");
        assert!(message.contains("the adapter fell over"), "{message}");
    }

    #[test]
    fn a_returned_string_survives_until_the_next_call() {
        let pointer = call_str(|| Ok("Korben Is Fast".to_string()));
        assert!(!pointer.is_null());
        let text = unsafe { CStr::from_ptr(pointer) }.to_string_lossy().to_string();
        assert_eq!(text, "Korben Is Fast");
        assert_eq!(last_error(), None);
    }

    #[test]
    fn a_string_that_cannot_cross_is_reported_rather_than_truncated() {
        let pointer = call_str(|| Ok("before\0after".to_string()));
        assert!(pointer.is_null());
        assert_eq!(last_error().as_deref(), Some("the returned string contains a zero byte"));
    }

    #[test]
    fn an_argument_is_read_back_and_a_null_one_is_refused() {
        let owned = CString::new("slugify me").unwrap();
        let borrowed = unsafe { borrowed_str(owned.as_ptr()) };
        assert_eq!(borrowed.as_deref(), Ok("slugify me"));

        let refused = unsafe { borrowed_str(std::ptr::null()) };
        assert!(refused.is_err(), "{refused:?}");
    }

    #[test]
    fn a_unit_call_still_reports_failure() {
        call_unit(|| Err("nothing to do".to_string()));
        assert_eq!(last_error().as_deref(), Some("nothing to do"));
        call_unit(|| Ok(()));
        assert_eq!(last_error(), None);
    }
}
