//! The type language used by inference.
//!
//! Types are rendered with source-level names: diagnostics never leak
//! implementation terminology, per specification section 10.1.

// korben-6bc

use crate::ast::Effects;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

pub type TypeVar = u32;

#[derive(Clone, Debug)]
pub enum Type {
    /// An inference variable.
    Var(TypeVar),
    /// A named type applied to arguments: `Int`, `Vec T`, `Result T E`.
    Con(Rc<str>, Vec<Type>),
    Fn(Rc<FnType>),
    /// A structural record. `name` is set once it is known to be nominal.
    Record(Rc<RecordType>),
    Tuple(Vec<Type>),
    /// Deliberately unconstrained: unifies with anything without complaint.
    ///
    /// Produced where the checker cannot reach a conclusion — a value crossing
    /// an unannotated boundary, for instance — so that inference reports real
    /// mismatches rather than speculative ones.
    Unknown,
}

#[derive(Clone, Debug)]
pub struct FnType {
    pub params: Vec<Type>,
    pub ret: Type,
    pub effects: Effects,
    /// True for functions such as `println` that accept extra arguments.
    pub variadic: bool,
}

#[derive(Clone, Debug)]
pub struct RecordType {
    pub name: Option<Rc<str>>,
    /// Sorted by field name so structural comparison is order-independent.
    pub fields: BTreeMap<String, Type>,
}

impl Type {
    pub fn con(name: &str) -> Type {
        Type::Con(Rc::from(name), Vec::new())
    }

    pub fn app(name: &str, args: Vec<Type>) -> Type {
        Type::Con(Rc::from(name), args)
    }

    pub fn function(params: Vec<Type>, ret: Type, effects: Effects) -> Type {
        Type::Fn(Rc::new(FnType { params, ret, effects, variadic: false }))
    }

    /// A function that accepts at least `params` arguments and then any number more.
    pub fn variadic(params: Vec<Type>, ret: Type, effects: Effects) -> Type {
        Type::Fn(Rc::new(FnType { params, ret, effects, variadic: true }))
    }

    pub fn unit() -> Type {
        Type::con("Unit")
    }

    pub fn bool() -> Type {
        Type::con("Bool")
    }

    pub fn int() -> Type {
        Type::con("Int")
    }

    pub fn float() -> Type {
        Type::con("Float64")
    }

    pub fn string() -> Type {
        Type::con("String")
    }

    pub fn keyword() -> Type {
        Type::con("Keyword")
    }

    pub fn vec(item: Type) -> Type {
        Type::app("Vec", vec![item])
    }

    pub fn option(item: Type) -> Type {
        Type::app("Option", vec![item])
    }

    pub fn result(ok: Type, err: Type) -> Type {
        Type::app("Result", vec![ok, err])
    }

    pub fn record(fields: BTreeMap<String, Type>, name: Option<Rc<str>>) -> Type {
        Type::Record(Rc::new(RecordType { name, fields }))
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Type::Unknown)
    }

    /// Collect the free variables of a type.
    pub fn free_vars(&self, out: &mut Vec<TypeVar>) {
        match self {
            Type::Var(var) => {
                if !out.contains(var) {
                    out.push(*var);
                }
            }
            Type::Con(_, args) | Type::Tuple(args) => {
                for arg in args {
                    arg.free_vars(out);
                }
            }
            Type::Fn(function) => {
                for param in &function.params {
                    param.free_vars(out);
                }
                function.ret.free_vars(out);
            }
            Type::Record(record) => {
                for field in record.fields.values() {
                    field.free_vars(out);
                }
            }
            Type::Unknown => {}
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Var(var) => write!(out, "{}", var_name(*var)),
            Type::Unknown => out.write_str("_"),
            Type::Con(name, args) if args.is_empty() => out.write_str(name),
            Type::Con(name, args) => {
                write!(out, "{name}")?;
                for arg in args {
                    match arg {
                        Type::Con(_, inner) if !inner.is_empty() => write!(out, " ({arg})")?,
                        Type::Fn(_) => write!(out, " ({arg})")?,
                        _ => write!(out, " {arg}")?,
                    }
                }
                Ok(())
            }
            Type::Fn(function) => {
                out.write_str("(-> [")?;
                for (index, param) in function.params.iter().enumerate() {
                    if index > 0 {
                        out.write_str(" ")?;
                    }
                    write!(out, "{param}")?;
                }
                if function.variadic {
                    out.write_str(" ...")?;
                }
                write!(out, "] {}", function.ret)?;
                if !function.effects.is_empty() {
                    write!(out, " {}", function.effects.render())?;
                }
                out.write_str(")")
            }
            Type::Tuple(items) => {
                out.write_str("[")?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.write_str(" ")?;
                    }
                    write!(out, "{item}")?;
                }
                out.write_str("]")
            }
            Type::Record(record) => {
                if let Some(name) = &record.name {
                    return out.write_str(name);
                }
                out.write_str("{ ")?;
                for (index, (name, ty)) in record.fields.iter().enumerate() {
                    if index > 0 {
                        out.write_str(" ")?;
                    }
                    write!(out, "{name}: {ty}")?;
                }
                out.write_str(" }")
            }
        }
    }
}

/// Inference variables print as `T`, `U`, ... so diagnostics stay readable.
fn var_name(var: TypeVar) -> String {
    let letter = (b'T' + (var % 7) as u8) as char;
    if var < 7 {
        letter.to_string()
    } else {
        format!("{letter}{}", var / 7)
    }
}

/// A polymorphic type: `vars` are generalized.
#[derive(Clone, Debug)]
pub struct Scheme {
    pub vars: Vec<TypeVar>,
    pub ty: Type,
}

impl Scheme {
    pub fn mono(ty: Type) -> Scheme {
        Scheme { vars: Vec::new(), ty }
    }
}
