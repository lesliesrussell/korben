//! The Korben compiler core: lowering, macro expansion, type inference,
//! evaluation, and the standard library.

// korben-6bc

pub mod ast;
pub mod builtins;
pub mod bundle;
pub mod cheader;
pub mod codegen;
pub mod docs;
pub mod eval;
pub mod expand;
pub mod infer;
pub mod ir;
pub mod lower;
pub mod manifest;
pub mod own;
pub mod project;
pub mod types;
pub mod value;

pub use korben_syntax as syntax;
