//! The Korben runtime.
//!
//! Values, operations, and the standard library, shared verbatim by the
//! interpreter and by generated native code. The specification requires both
//! execution modes to have identical observable semantics; sharing this crate
//! is how that is guaranteed rather than merely tested.

// korben-vtx

pub mod apply;
pub mod ffi;
pub mod json;
pub mod loc;
pub mod net;
pub mod std;
pub mod task;
pub mod value;

pub use apply::{apply, bind_args, construct};
pub use loc::{Fault, Loc};
pub use value::display;
pub use value::{
    Arg, Body, Caller, Flow, Foreign, Function, MapValue, Outcome, Param, RecordValue, Sym, Value,
    VariantValue,
};
