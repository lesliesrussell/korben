//! Installing the standard library into the interpreter's module table.
//!
//! The functions themselves live in `korben-runtime`, shared with generated
//! native code. This module only decides which Korben module each one is
//! reachable from.

// korben-vtx

use crate::eval::Interp;

/// The module whose exports are visible from every module without importing.
pub const PRELUDE: &str = "std.core";

pub fn install(interp: &Interp) {
    for name in korben_runtime::std::NAMES {
        let Some((module, member)) = name.split_once('/') else { continue };
        let Some(value) = korben_runtime::std::builtin(name) else { continue };
        let runtime = interp.module(module);
        runtime.exports.borrow_mut().insert(member.to_string(), value);
    }
    // `Cell` and `File` are addressed as type names rather than module paths.
    for name in
        ["Cell", "File", "Listener", "Connection", "Pool", "Scope", "Task", "Sender", "Receiver"]
    {
        let module = interp.module(name);
        interp.modules.borrow_mut().insert(name.to_string(), module);
    }
    // `Drop` is a compiler-known protocol: implementing it makes a type
    // resource-bearing, which is what the ownership analysis keys off.
    interp.protocols.borrow_mut().insert("Drop".to_string(), vec!["drop".to_string()]);
    interp.method_owner.borrow_mut().insert("drop".to_string(), "Drop".to_string());
}

// korben-4io
/// Whether `name` addresses a runtime module -- either a standard-library
/// module such as `std.string`, or a type addressed like one, as `Cell.new` is.
pub fn is_runtime_module(name: &str) -> bool {
    let prefix = format!("{name}/");
    korben_runtime::std::NAMES.iter().any(|entry| entry.starts_with(&prefix))
}

/// The canonical runtime name for a member of a module, if it has one.
pub fn runtime_name(module: &str, member: &str) -> Option<&'static str> {
    let key = format!("{module}/{member}");
    korben_runtime::std::NAMES.iter().copied().find(|name| *name == key)
}
