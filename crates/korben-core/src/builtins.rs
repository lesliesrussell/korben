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
    // `Cell` is addressed as a type name rather than a module path.
    let cell = interp.module("Cell");
    interp.modules.insert("Cell".to_string(), cell);
}

/// The canonical runtime name for a member of a module, if it has one.
pub fn runtime_name(module: &str, member: &str) -> Option<&'static str> {
    let key = format!("{module}/{member}");
    korben_runtime::std::NAMES.iter().copied().find(|name| *name == key)
}
