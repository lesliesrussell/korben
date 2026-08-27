//! Foreign function calls.
//!
//! Specification section 17 makes typed native interoperation a first-class
//! capability while keeping foreign unsafety contained. This module is the
//! containment: it loads a dynamic library, resolves a symbol, marshals Korben
//! values into the C ABI, and marshals the result back. Everything above it
//! sees ordinary typed functions.
//!
//! Both execution modes share this code, so a native executable and the
//! interpreter call foreign functions the same way.
//!
//! The unsafety here is real and deliberate: a declaration asserts a contract
//! the compiler cannot verify. That is why every foreign declaration is an
//! `unsafe fn`, and why the ordinary user-facing form is a safe Korben wrapper.

// korben-v3q

use crate::loc::{Fault, Loc};
use crate::value::{Flow, Outcome, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};

/// The C types a declaration may mention.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CType {
    Void,
    Bool,
    Char,
    Int,
    UInt,
    Long,
    ULong,
    Float,
    Double,
    /// `const char *`, marshalled to and from a Korben `String`.
    Str,
    /// An opaque pointer, carried as a value without being dereferenced.
    Ptr,
}

impl CType {
    /// Parse the type name a Korben declaration uses.
    pub fn parse(name: &str) -> Option<CType> {
        Some(match name {
            "CVoid" | "Unit" => CType::Void,
            "CBool" => CType::Bool,
            "CChar" => CType::Char,
            "CInt" => CType::Int,
            "CUInt" => CType::UInt,
            "CLong" => CType::Long,
            "CULong" | "CSize" => CType::ULong,
            "CFloat" => CType::Float,
            "CDouble" => CType::Double,
            "CStr" => CType::Str,
            "Ptr" => CType::Ptr,
            _ => return None,
        })
    }

    /// The Korben type a C type surfaces as.
    pub fn korben_type(self) -> &'static str {
        match self {
            CType::Void => "Unit",
            CType::Bool => "Bool",
            CType::Char | CType::Int | CType::UInt | CType::Long | CType::ULong => "Int",
            CType::Float | CType::Double => "Float64",
            // A foreign string or pointer may be null, so it surfaces as an
            // Option rather than as something that can be null.
            CType::Str => "Option String",
            CType::Ptr => "Option Ptr",
        }
    }

    fn is_integral(self) -> bool {
        !matches!(self, CType::Float | CType::Double | CType::Void)
    }

    fn name(self) -> &'static str {
        match self {
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
}

/// A declared foreign function.
#[derive(Clone, Debug)]
pub struct CSignature {
    /// The library name as written in `(ffi/c-library "...")`.
    pub library: String,
    /// The symbol to resolve, which may differ from the Korben name.
    pub symbol: String,
    pub params: Vec<CType>,
    pub ret: CType,
}

impl CSignature {
    pub fn render(&self) -> String {
        let params: Vec<&str> = self.params.iter().map(|ty| ty.name()).collect();
        format!("{}({}) -> {}", self.symbol, params.join(", "), self.ret.name())
    }
}

// ------------------------------------------------------------ library loading

#[cfg(unix)]
extern "C" {
    fn dlopen(path: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

/// Resolve immediately, so a missing symbol is reported at load time.
#[cfg(unix)]
const RTLD_NOW: c_int = 2;

thread_local! {
    static LIBRARIES: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
    static SYMBOLS: RefCell<HashMap<(String, String), usize>> = RefCell::new(HashMap::new());
}

/// File names to try for a library named `name`.
fn candidates(name: &str) -> Vec<String> {
    if name.contains('/') || name.contains('.') {
        return vec![name.to_string()];
    }
    if cfg!(target_os = "macos") {
        vec![format!("lib{name}.dylib"), format!("{name}.dylib"), name.to_string()]
    } else {
        vec![
            format!("lib{name}.so"),
            format!("lib{name}.so.6"),
            format!("{name}.so"),
            name.to_string(),
        ]
    }
}

/// The C runtime and math library are already linked into this process, so the
/// process handle resolves them without naming a file.
fn is_process_library(name: &str) -> bool {
    matches!(name, "c" | "m" | "libc" | "libm" | "" | "self")
}

#[cfg(unix)]
fn load_library(name: &str, loc: Loc) -> Result<usize, Flow> {
    if let Some(handle) = LIBRARIES.with(|cache| cache.borrow().get(name).copied()) {
        return Ok(handle);
    }
    let mut attempts = Vec::new();
    let mut handle = std::ptr::null_mut();

    if is_process_library(name) {
        // SAFETY: a null path asks for a handle to the running process, which
        // is always valid and needs no file to exist.
        handle = unsafe { dlopen(std::ptr::null(), RTLD_NOW) };
    }
    if handle.is_null() {
        for candidate in candidates(name) {
            let Ok(path) = CString::new(candidate.clone()) else { continue };
            // SAFETY: `path` is a valid NUL-terminated string for this call.
            handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
            if !handle.is_null() {
                break;
            }
            attempts.push(candidate);
        }
    }
    if handle.is_null() {
        let detail = last_error().unwrap_or_else(|| "not found".to_string());
        return Err(Flow::fault(
            Fault::new("ffi-library", format!("cannot load library `{name}`"), loc)
                .label("declared by `(ffi/c-library ...)`")
                .note(format!("tried: {}", attempts.join(", ")))
                .note(detail)
                .help("install the library, or name the file directly"),
        ));
    }
    let handle = handle as usize;
    LIBRARIES.with(|cache| cache.borrow_mut().insert(name.to_string(), handle));
    Ok(handle)
}

#[cfg(not(unix))]
fn load_library(name: &str, loc: Loc) -> Result<usize, Flow> {
    let _ = name;
    Err(Flow::fault(
        Fault::new("ffi-unsupported", "foreign calls are only supported on Unix", loc)
            .help("build for a Unix target, or call the library from generated Rust"),
    ))
}

#[cfg(unix)]
fn last_error() -> Option<String> {
    // SAFETY: `dlerror` returns either null or a NUL-terminated static string.
    let message = unsafe { dlerror() };
    if message.is_null() {
        return None;
    }
    Some(unsafe { CStr::from_ptr(message) }.to_string_lossy().to_string())
}

#[cfg(unix)]
fn resolve(signature: &CSignature, loc: Loc) -> Result<usize, Flow> {
    let key = (signature.library.clone(), signature.symbol.clone());
    if let Some(address) = SYMBOLS.with(|cache| cache.borrow().get(&key).copied()) {
        return Ok(address);
    }
    let handle = load_library(&signature.library, loc)?;
    let Ok(symbol) = CString::new(signature.symbol.clone()) else {
        return Err(Flow::fault(Fault::new("ffi-symbol", "symbol name contains a NUL byte", loc)));
    };
    // SAFETY: `handle` came from `dlopen` and `symbol` is NUL-terminated.
    let address = unsafe { dlsym(handle as *mut c_void, symbol.as_ptr()) };
    if address.is_null() {
        return Err(Flow::fault(
            Fault::new(
                "ffi-symbol",
                format!("`{}` is not in library `{}`", signature.symbol, signature.library),
                loc,
            )
            .label("declared by `(ffi/c-fn ...)`")
            .help("check the symbol name, or the library version"),
        ));
    }
    let address = address as usize;
    SYMBOLS.with(|cache| cache.borrow_mut().insert(key, address));
    Ok(address)
}

#[cfg(not(unix))]
fn resolve(signature: &CSignature, loc: Loc) -> Result<usize, Flow> {
    let _ = signature;
    load_library("", loc).map(|_| 0)
}

// ---------------------------------------------------------------- marshalling

/// A C argument, plus any owned storage it points into.
struct Marshalled {
    integer: i64,
    double: f64,
    /// Kept alive for the duration of the call.
    _owned: Option<CString>,
}

fn marshal(
    ty: CType,
    value: &Value,
    index: usize,
    name: &str,
    loc: Loc,
) -> Result<Marshalled, Flow> {
    let wrong = |expected: &str| {
        Flow::fault(
            Fault::new(
                "ffi-argument",
                format!("argument {} of `{name}` must be {expected}", index + 1),
                loc,
            )
            .label(format!("found {}", value.type_name()))
            .note(format!("the declaration says `{}`", ty.name())),
        )
    };
    Ok(match ty {
        CType::Bool => {
            Marshalled { integer: i64::from(value.is_truthy()), double: 0.0, _owned: None }
        }
        CType::Char | CType::Int | CType::UInt | CType::Long | CType::ULong => match value {
            Value::Int(number) => Marshalled { integer: *number, double: 0.0, _owned: None },
            _ => return Err(wrong("an Int")),
        },
        CType::Float | CType::Double => match value {
            Value::Float(number) => Marshalled { integer: 0, double: *number, _owned: None },
            Value::Int(number) => Marshalled { integer: 0, double: *number as f64, _owned: None },
            _ => return Err(wrong("a number")),
        },
        CType::Str => match value {
            Value::Str(text) => {
                let Ok(owned) = CString::new(text.as_str()) else {
                    return Err(Flow::fault(
                        Fault::new(
                            "ffi-argument",
                            format!("argument {} of `{name}` contains a NUL byte", index + 1),
                            loc,
                        )
                        .help("C strings cannot carry an interior NUL"),
                    ));
                };
                Marshalled { integer: owned.as_ptr() as i64, double: 0.0, _owned: Some(owned) }
            }
            Value::Nil => Marshalled { integer: 0, double: 0.0, _owned: None },
            _ => return Err(wrong("a String")),
        },
        CType::Ptr => match as_pointer(value) {
            Some(address) => Marshalled { integer: address as i64, double: 0.0, _owned: None },
            None => return Err(wrong("a Ptr")),
        },
        CType::Void => return Err(wrong("not `CVoid`")),
    })
}

/// A foreign pointer travelling as a Korben value.
pub fn pointer_value(address: usize) -> Value {
    crate::value::Foreign::wrap("Ptr", address)
}

fn as_pointer(value: &Value) -> Option<usize> {
    match value {
        Value::Nil => Some(0),
        Value::Foreign(foreign) if foreign.kind == "Ptr" => foreign.downcast::<usize>().copied(),
        // `Some(ptr)` unwraps, because a pointer surfaces as an Option.
        Value::Variant(variant) if &*variant.variant == "Some" => {
            as_pointer(&variant.fields.first()?.1)
        }
        Value::Variant(variant) if &*variant.variant == "None" => Some(0),
        _ => None,
    }
}

/// Turn a returned C value into a Korben value.
///
/// # Safety
///
/// For `CStr` the callee must have returned either null or a pointer to a
/// NUL-terminated string that stays valid until it is copied here. That is part
/// of the contract a `(ffi/c-fn ...)` declaration asserts.
unsafe fn unmarshal(ty: CType, integer: i64, double: f64) -> Value {
    match ty {
        CType::Void => Value::Nil,
        CType::Bool => Value::Bool(integer != 0),
        CType::Char | CType::Int | CType::UInt | CType::Long | CType::ULong => Value::Int(integer),
        CType::Float | CType::Double => Value::Float(double),
        CType::Str => {
            let pointer = integer as *const c_char;
            if pointer.is_null() {
                Value::none()
            } else {
                Value::some(Value::str(CStr::from_ptr(pointer).to_string_lossy().to_string()))
            }
        }
        CType::Ptr => {
            if integer == 0 {
                Value::none()
            } else {
                Value::some(pointer_value(integer as usize))
            }
        }
    }
}

// ----------------------------------------------------------------- invocation

/// Call a declared foreign function.
pub fn call(signature: &CSignature, args: Vec<Value>, name: &str, loc: Loc) -> Outcome {
    if args.len() != signature.params.len() {
        return Err(Flow::fault(
            Fault::new(
                "arity",
                format!(
                    "`{name}` expects {} argument(s) but got {}",
                    signature.params.len(),
                    args.len()
                ),
                loc,
            )
            .note(format!("declared as {}", signature.render())),
        ));
    }

    let mut marshalled = Vec::with_capacity(args.len());
    for (index, (ty, value)) in signature.params.iter().zip(args.iter()).enumerate() {
        marshalled.push(marshal(*ty, value, index, name, loc)?);
    }
    let address = resolve(signature, loc)?;

    let all_integral = signature.params.iter().all(|ty| ty.is_integral());
    let all_floating = signature.params.iter().all(|ty| matches!(ty, CType::Float | CType::Double));

    // The C ABI passes integers and pointers in one register class and floats
    // in another, so a uniform trampoline only works for a uniform signature.
    if !all_integral && !all_floating {
        return Err(Flow::fault(
            Fault::new(
                "ffi-signature",
                format!("`{name}` mixes integer and floating-point parameters"),
                loc,
            )
            .note(format!("declared as {}", signature.render()))
            .help("this backend supports all-integer or all-floating parameter lists"),
        ));
    }

    let integers: Vec<i64> = marshalled.iter().map(|arg| arg.integer).collect();
    let doubles: Vec<f64> = marshalled.iter().map(|arg| arg.double).collect();
    let returns_double = matches!(signature.ret, CType::Float | CType::Double);

    // SAFETY: `address` is a resolved symbol, and the transmuted signature is
    // the one the declaration asserts. Argument storage outlives the call
    // because `marshalled` is still in scope.
    let (integer, double) = unsafe {
        if all_floating && !doubles.is_empty() {
            invoke_floating(address, &doubles, returns_double, name, loc)?
        } else {
            invoke_integral(address, &integers, returns_double, name, loc)?
        }
    };
    let result = unsafe { unmarshal(signature.ret, integer, double) };
    drop(marshalled);
    Ok(result)
}

/// # Safety
/// `address` must be a function with `count` integer-class parameters.
unsafe fn invoke_integral(
    address: usize,
    args: &[i64],
    returns_double: bool,
    name: &str,
    loc: Loc,
) -> Result<(i64, f64), Flow> {
    macro_rules! dispatch {
        ($($count:literal => ($($index:literal),*)),* $(,)?) => {
            match args.len() {
                $(
                    $count => {
                        if returns_double {
                            let f: extern "C" fn($(kb_i64!($index)),*) -> f64 =
                                std::mem::transmute(address);
                            Ok((0, f($(args[$index]),*)))
                        } else {
                            let f: extern "C" fn($(kb_i64!($index)),*) -> i64 =
                                std::mem::transmute(address);
                            Ok((f($(args[$index]),*), 0.0))
                        }
                    }
                )*
                other => Err(too_many(name, other, loc)),
            }
        };
    }
    dispatch! {
        0 => (),
        1 => (0),
        2 => (0, 1),
        3 => (0, 1, 2),
        4 => (0, 1, 2, 3),
        5 => (0, 1, 2, 3, 4),
        6 => (0, 1, 2, 3, 4, 5),
        7 => (0, 1, 2, 3, 4, 5, 6),
        8 => (0, 1, 2, 3, 4, 5, 6, 7),
    }
}

/// # Safety
/// `address` must be a function with `count` floating-point parameters.
unsafe fn invoke_floating(
    address: usize,
    args: &[f64],
    returns_double: bool,
    name: &str,
    loc: Loc,
) -> Result<(i64, f64), Flow> {
    macro_rules! dispatch {
        ($($count:literal => ($($index:literal),*)),* $(,)?) => {
            match args.len() {
                $(
                    $count => {
                        if returns_double {
                            let f: extern "C" fn($(kb_f64!($index)),*) -> f64 =
                                std::mem::transmute(address);
                            Ok((0, f($(args[$index]),*)))
                        } else {
                            let f: extern "C" fn($(kb_f64!($index)),*) -> i64 =
                                std::mem::transmute(address);
                            Ok((f($(args[$index]),*), 0.0))
                        }
                    }
                )*
                other => Err(too_many(name, other, loc)),
            }
        };
    }
    dispatch! {
        1 => (0),
        2 => (0, 1),
        3 => (0, 1, 2),
        4 => (0, 1, 2, 3),
    }
}

/// Expand an index to the parameter type it stands for.
macro_rules! kb_i64 {
    ($index:literal) => {
        i64
    };
}
macro_rules! kb_f64 {
    ($index:literal) => {
        f64
    };
}
use {kb_f64, kb_i64};

fn too_many(name: &str, count: usize, loc: Loc) -> Flow {
    Flow::fault(
        Fault::new(
            "ffi-signature",
            format!("`{name}` takes {count} arguments, which this backend cannot call"),
            loc,
        )
        .help("foreign calls support up to 8 integer or 4 floating-point parameters"),
    )
}
