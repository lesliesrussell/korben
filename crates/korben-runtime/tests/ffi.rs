//! Foreign calls against the C runtime, which is linked into every process.

use korben_runtime::ffi::{call, CSignature, CType};
use korben_runtime::{Loc, Value};

fn signature(symbol: &str, params: Vec<CType>, ret: CType) -> CSignature {
    CSignature { library: "c".to_string(), symbol: symbol.to_string(), params, ret }
}

/// `Flow` carries values that are not `Debug`, so tests unwrap it by hand.
fn ok(result: korben_runtime::Outcome, what: &str) -> Value {
    match result {
        Ok(value) => value,
        Err(korben_runtime::Flow::Panic(fault)) => panic!("{what} failed: {}", fault.render()),
        Err(_) => panic!("{what} produced unexpected control flow"),
    }
}

fn fault(result: korben_runtime::Outcome) -> korben_runtime::Fault {
    match result {
        Err(korben_runtime::Flow::Panic(fault)) => *fault,
        Err(_) => panic!("expected a fault, got other control flow"),
        Ok(value) => panic!("expected a fault, got `{value}`"),
    }
}

#[test]
fn calls_a_function_taking_a_string() {
    let sig = signature("strlen", vec![CType::Str], CType::Long);
    let result = ok(call(&sig, vec![Value::str("korben")], "strlen", Loc::NONE), "strlen");
    assert!(matches!(result, Value::Int(6)), "{result}");
}

#[test]
fn calls_a_function_taking_an_integer() {
    let sig = signature("abs", vec![CType::Int], CType::Int);
    let result = ok(call(&sig, vec![Value::Int(-42)], "abs", Loc::NONE), "abs");
    assert!(matches!(result, Value::Int(42)), "{result}");
}

#[test]
fn calls_a_function_taking_a_double() {
    let sig = signature("sqrt", vec![CType::Double], CType::Double);
    let result = ok(call(&sig, vec![Value::Float(16.0)], "sqrt", Loc::NONE), "sqrt");
    assert!(matches!(result, Value::Float(value) if value == 4.0), "{result}");
}

#[test]
fn a_returned_string_is_an_option() {
    let sig = signature("getenv", vec![CType::Str], CType::Str);
    // A variable that certainly exists, and one that certainly does not.
    std::env::set_var("KORBEN_FFI_TEST", "present");
    let found = ok(call(&sig, vec![Value::str("KORBEN_FFI_TEST")], "getenv", Loc::NONE), "getenv");
    assert_eq!(found.to_string(), "(Some \"present\")");

    let missing = ok(
        call(&sig, vec![Value::str("KORBEN_FFI_DEFINITELY_UNSET_XYZ")], "getenv", Loc::NONE),
        "getenv",
    );
    assert_eq!(missing.to_string(), "(None)");
}

#[test]
fn a_missing_symbol_is_reported() {
    let sig = signature("korben_no_such_symbol", vec![], CType::Int);
    let fault = fault(call(&sig, vec![], "korben_no_such_symbol", Loc::NONE));
    assert_eq!(fault.code, "ffi-symbol");
    assert!(fault.message.contains("korben_no_such_symbol"), "{}", fault.message);
}

#[test]
fn a_missing_library_is_reported() {
    let sig = CSignature {
        library: "korben-no-such-library".to_string(),
        symbol: "anything".to_string(),
        params: vec![],
        ret: CType::Int,
    };
    let fault = fault(call(&sig, vec![], "anything", Loc::NONE));
    assert_eq!(fault.code, "ffi-library");
}

#[test]
fn a_wrong_argument_type_is_reported_before_the_call() {
    let sig = signature("abs", vec![CType::Int], CType::Int);
    let fault = fault(call(&sig, vec![Value::str("not a number")], "abs", Loc::NONE));
    assert_eq!(fault.code, "ffi-argument");
    assert!(fault.message.contains("must be an Int"), "{}", fault.message);
}

#[test]
fn a_mixed_signature_is_rejected_rather_than_miscalled() {
    let sig = signature("ldexp", vec![CType::Double, CType::Int], CType::Double);
    let fault = fault(call(&sig, vec![Value::Float(1.0), Value::Int(2)], "ldexp", Loc::NONE));
    assert_eq!(fault.code, "ffi-signature");
}

#[test]
fn an_interior_nul_is_rejected() {
    let sig = signature("strlen", vec![CType::Str], CType::Long);
    let fault = fault(call(&sig, vec![Value::str("bad\0string")], "strlen", Loc::NONE));
    assert_eq!(fault.code, "ffi-argument");
}
