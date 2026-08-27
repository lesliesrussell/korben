//! Installing the standard library into the interpreter's module table.
//!
//! The functions themselves live in `korben-runtime`, shared with generated
//! native code. This module only decides which Korben module each one is
//! reachable from.

// korben-vtx

use crate::eval::Interp;

/// The module whose exports are visible from every module without importing.
pub const PRELUDE: &str = "std.core";

pub fn install(interp: &mut Interp) {
    for name in korben_runtime::std::NAMES {
        let Some((module, member)) = name.split_once('/') else { continue };
        let Some(value) = korben_runtime::std::builtin(name) else { continue };
        let runtime = interp.module(module);
        runtime.exports.borrow_mut().insert(member.to_string(), value);
    }
    // `Cell` and `File` are addressed as type names rather than module paths.
    for name in ["Cell", "File"] {
        let module = interp.module(name);
        interp.modules.insert(name.to_string(), module);
    }
    // `Drop` is a compiler-known protocol: implementing it makes a type
    // resource-bearing, which is what the ownership analysis keys off.
    interp.protocols.insert("Drop".to_string(), vec!["drop".to_string()]);
    interp.method_owner.insert("drop".to_string(), "Drop".to_string());
}

/// The canonical runtime name for a member of a module, if it has one.
pub fn runtime_name(module: &str, member: &str) -> Option<&'static str> {
    let key = format!("{module}/{member}");
    korben_runtime::std::NAMES.iter().copied().find(|name| *name == key)
}
