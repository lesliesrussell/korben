//! Runtime values and environments.
//!
//! Collections are immutable at the language level; the interpreter uses
//! reference counting with copy-on-write so that ordinary code never observes
//! shared mutation. Mutable state is explicit via `var` bindings and `Cell`.

// korben-6bc

use crate::ast::FnDecl;
use korben_syntax::diag::Diagnostic;
use korben_syntax::span::Span;
use korben_syntax::Syntax;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

pub type Sym = Rc<str>;

#[derive(Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<String>),
    Keyword(Sym),
    /// A quoted symbol, produced by `'name` and by macro arguments.
    Symbol(Sym),
    Vector(Rc<Vec<Value>>),
    Map(Rc<MapValue>),
    Set(Rc<Vec<Value>>),
    Record(Rc<RecordValue>),
    Variant(Rc<VariantValue>),
    Closure(Rc<Closure>),
    Native(Rc<NativeFn>),
    /// Explicit mutable cell (`Cell.new`).
    Cell(Rc<RefCell<Value>>),
    /// A syntax object, the currency of compile-time metaprogramming.
    Syntax(Rc<Syntax>),
    /// A resource with deterministic cleanup, e.g. an open file.
    Resource(Rc<Resource>),
    /// Compiler-generated callables: type constructors and protocol methods.
    Builtin(Rc<Builtin>),
}

/// A callable the compiler generates from a declaration.
pub enum Builtin {
    /// A record constructor or an enum variant constructor.
    Ctor { type_name: Sym, variant: Option<Sym>, fields: Vec<Sym> },
    /// A protocol method that dispatches on its first argument.
    Method { protocol: Sym, name: Sym },
}

/// An insertion-ordered map. Korben guarantees deterministic iteration order.
#[derive(Clone, Default)]
pub struct MapValue {
    pub entries: Vec<(Value, Value)>,
}

impl MapValue {
    pub fn get(&self, key: &Value) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k.eq_value(key)).map(|(_, v)| v)
    }

    pub fn insert(&mut self, key: Value, value: Value) {
        match self.entries.iter_mut().find(|(k, _)| k.eq_value(&key)) {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key, value)),
        }
    }

    pub fn remove(&mut self, key: &Value) -> Option<Value> {
        let index = self.entries.iter().position(|(k, _)| k.eq_value(key))?;
        Some(self.entries.remove(index).1)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Clone)]
pub struct RecordValue {
    /// `None` for anonymous structural records.
    pub type_name: Option<Sym>,
    pub fields: Vec<(Sym, Value)>,
}

impl RecordValue {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.iter().find(|(field, _)| &**field == name).map(|(_, value)| value)
    }
}

#[derive(Clone)]
pub struct VariantValue {
    pub type_name: Sym,
    pub variant: Sym,
    pub fields: Vec<(Sym, Value)>,
}

impl VariantValue {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.iter().find(|(field, _)| &**field == name).map(|(_, value)| value)
    }
}

pub struct Closure {
    pub decl: Rc<FnDecl>,
    pub env: Env,
    /// Module this closure was defined in, for global lookup.
    pub module: Sym,
}

pub struct NativeFn {
    pub name: &'static str,
    /// Minimum arity; `None` for variadic with no minimum.
    pub arity: Option<usize>,
    pub func: NativeImpl,
}

pub type NativeImpl = fn(&mut crate::eval::Interp, Vec<Value>, Span) -> Result<Value, Flow>;

/// A native resource with deterministic cleanup, per specification 12.5.
pub struct Resource {
    pub kind: &'static str,
    pub state: RefCell<ResourceState>,
}

pub enum ResourceState {
    File(Option<std::fs::File>),
    Closed,
}

impl Resource {
    pub fn close(&self) {
        *self.state.borrow_mut() = ResourceState::Closed;
    }

    pub fn is_closed(&self) -> bool {
        matches!(&*self.state.borrow(), ResourceState::Closed)
    }
}

/// Non-local control flow produced while evaluating.
pub enum Flow {
    /// `recur` to the enclosing `loop` or function head.
    Recur(Vec<Value>),
    /// Postfix `?` propagating an `Err` or `None` out of the current function.
    Propagate(Value),
    /// A thrown condition, catchable by `try`.
    Condition(Value, Span),
    /// An unrecoverable fault: a compiler-detected runtime error.
    Panic(Box<Diagnostic>),
}

impl Flow {
    pub fn panic(diagnostic: Diagnostic) -> Flow {
        Flow::Panic(Box::new(diagnostic))
    }

    pub fn error(message: impl Into<String>, span: Span) -> Flow {
        Flow::panic(Diagnostic::error(message).with_code("runtime").span(span))
    }
}

pub type EvalResult = Result<Value, Flow>;

// --------------------------------------------------------------- environment

struct Binding {
    name: Sym,
    value: Value,
    /// `let` and parameters are immutable; only `var` may be reassigned.
    mutable: bool,
}

struct ScopeData {
    vars: RefCell<Vec<Binding>>,
    parent: Option<Env>,
}

/// The outcome of assigning to a name, so `set!` can explain what went wrong.
pub enum Assign {
    Done,
    Immutable,
    Unbound,
}

/// A lexical environment. Cheap to clone; scopes form a parent chain.
#[derive(Clone)]
pub struct Env(Rc<ScopeData>);

impl Env {
    pub fn root() -> Env {
        Env(Rc::new(ScopeData { vars: RefCell::new(Vec::new()), parent: None }))
    }

    pub fn child(&self) -> Env {
        Env(Rc::new(ScopeData { vars: RefCell::new(Vec::new()), parent: Some(self.clone()) }))
    }

    /// Introduce an immutable binding: `let`, parameters, patterns, loops.
    pub fn define(&self, name: impl Into<Sym>, value: Value) {
        self.bind(name.into(), value, false);
    }

    /// Introduce a mutable binding, as `var` does.
    pub fn define_var(&self, name: impl Into<Sym>, value: Value) {
        self.bind(name.into(), value, true);
    }

    fn bind(&self, name: Sym, value: Value, mutable: bool) {
        let mut vars = self.0.vars.borrow_mut();
        match vars.iter_mut().find(|binding| binding.name == name) {
            // Shadowing in the same scope replaces the previous binding.
            Some(slot) => {
                slot.value = value;
                slot.mutable = mutable;
            }
            None => vars.push(Binding { name, value, mutable }),
        }
    }

    pub fn lookup(&self, name: &str) -> Option<Value> {
        let mut scope = Some(self.clone());
        while let Some(current) = scope {
            if let Some(binding) =
                current.0.vars.borrow().iter().find(|binding| &*binding.name == name)
            {
                return Some(binding.value.clone());
            }
            scope = current.0.parent.clone();
        }
        None
    }

    /// Assign to an existing mutable binding, walking outward.
    pub fn assign(&self, name: &str, value: Value) -> Assign {
        let mut scope = Some(self.clone());
        while let Some(current) = scope {
            {
                let mut vars = current.0.vars.borrow_mut();
                if let Some(slot) = vars.iter_mut().find(|binding| &*binding.name == name) {
                    if !slot.mutable {
                        return Assign::Immutable;
                    }
                    slot.value = value;
                    return Assign::Done;
                }
            }
            scope = current.0.parent.clone();
        }
        Assign::Unbound
    }
}

/// Everything a loaded module exposes at runtime.
pub struct ModuleRuntime {
    pub name: Sym,
    pub globals: RefCell<HashMap<String, Value>>,
    /// Names visible to importers.
    pub exports: RefCell<HashMap<String, Value>>,
    /// Import alias to fully-qualified module name.
    pub aliases: RefCell<HashMap<String, String>>,
    /// Names imported directly into this module's scope.
    pub imported: RefCell<HashMap<String, (String, String)>>,
}

impl ModuleRuntime {
    pub fn new(name: impl Into<Sym>) -> ModuleRuntime {
        ModuleRuntime {
            name: name.into(),
            globals: RefCell::new(HashMap::new()),
            exports: RefCell::new(HashMap::new()),
            aliases: RefCell::new(HashMap::new()),
            imported: RefCell::new(HashMap::new()),
        }
    }
}

// ------------------------------------------------------------------ helpers

impl Value {
    pub fn str(text: impl Into<String>) -> Value {
        Value::Str(Rc::new(text.into()))
    }

    pub fn keyword(name: &str) -> Value {
        Value::Keyword(Rc::from(name))
    }

    pub fn vector(items: Vec<Value>) -> Value {
        Value::Vector(Rc::new(items))
    }

    pub fn map(entries: Vec<(Value, Value)>) -> Value {
        Value::Map(Rc::new(MapValue { entries }))
    }

    pub fn ok(value: Value) -> Value {
        Value::Variant(Rc::new(VariantValue {
            type_name: Rc::from("Result"),
            variant: Rc::from("Ok"),
            fields: vec![(Rc::from("value"), value)],
        }))
    }

    pub fn err(value: Value) -> Value {
        Value::Variant(Rc::new(VariantValue {
            type_name: Rc::from("Result"),
            variant: Rc::from("Err"),
            fields: vec![(Rc::from("error"), value)],
        }))
    }

    pub fn some(value: Value) -> Value {
        Value::Variant(Rc::new(VariantValue {
            type_name: Rc::from("Option"),
            variant: Rc::from("Some"),
            fields: vec![(Rc::from("value"), value)],
        }))
    }

    pub fn none() -> Value {
        Value::Variant(Rc::new(VariantValue {
            type_name: Rc::from("Option"),
            variant: Rc::from("None"),
            fields: Vec::new(),
        }))
    }

    /// Truthiness: only `false` and `nil` are falsey.
    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Bool(false) | Value::Nil)
    }

    /// The source-level type name shown in diagnostics and by `type-of`.
    pub fn type_name(&self) -> String {
        match self {
            Value::Nil => "Unit".to_string(),
            Value::Bool(_) => "Bool".to_string(),
            Value::Int(_) => "Int".to_string(),
            Value::Float(_) => "Float64".to_string(),
            Value::Str(_) => "String".to_string(),
            Value::Keyword(_) => "Keyword".to_string(),
            Value::Symbol(_) => "Symbol".to_string(),
            Value::Vector(_) => "Vec".to_string(),
            Value::Map(_) => "Map".to_string(),
            Value::Set(_) => "Set".to_string(),
            Value::Record(record) => record
                .type_name
                .as_ref()
                .map(|name| name.to_string())
                .unwrap_or_else(|| "Record".to_string()),
            Value::Variant(variant) => variant.type_name.to_string(),
            Value::Closure(_) | Value::Native(_) => "Fn".to_string(),
            Value::Cell(_) => "Cell".to_string(),
            Value::Syntax(_) => "Syntax".to_string(),
            Value::Resource(resource) => resource.kind.to_string(),
            Value::Builtin(_) => "Fn".to_string(),
        }
    }

    /// Structural equality, per `std.core/=`.
    pub fn eq_value(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(left), Value::Bool(right)) => left == right,
            (Value::Int(left), Value::Int(right)) => left == right,
            (Value::Float(left), Value::Float(right)) => left == right,
            // Integers and floats compare across the numeric tower.
            (Value::Int(left), Value::Float(right)) | (Value::Float(right), Value::Int(left)) => {
                (*left as f64) == *right
            }
            (Value::Str(left), Value::Str(right)) => left == right,
            (Value::Keyword(left), Value::Keyword(right)) => left == right,
            (Value::Symbol(left), Value::Symbol(right)) => left == right,
            (Value::Vector(left), Value::Vector(right)) => {
                left.len() == right.len()
                    && left.iter().zip(right.iter()).all(|(a, b)| a.eq_value(b))
            }
            (Value::Set(left), Value::Set(right)) => {
                left.len() == right.len()
                    && left.iter().all(|item| right.iter().any(|other| item.eq_value(other)))
            }
            (Value::Map(left), Value::Map(right)) => {
                left.len() == right.len()
                    && left.entries.iter().all(|(key, value)| {
                        right.get(key).map(|other| value.eq_value(other)).unwrap_or(false)
                    })
            }
            (Value::Record(left), Value::Record(right)) => {
                left.type_name == right.type_name
                    && left.fields.len() == right.fields.len()
                    && left.fields.iter().all(|(name, value)| {
                        right.get(name).map(|other| value.eq_value(other)).unwrap_or(false)
                    })
            }
            (Value::Variant(left), Value::Variant(right)) => {
                left.type_name == right.type_name
                    && left.variant == right.variant
                    && left.fields.len() == right.fields.len()
                    && left
                        .fields
                        .iter()
                        .zip(right.fields.iter())
                        .all(|((_, a), (_, b))| a.eq_value(b))
            }
            (Value::Cell(left), Value::Cell(right)) => Rc::ptr_eq(left, right),
            (Value::Closure(left), Value::Closure(right)) => Rc::ptr_eq(left, right),
            (Value::Native(left), Value::Native(right)) => Rc::ptr_eq(left, right),
            (Value::Syntax(left), Value::Syntax(right)) => left.to_string() == right.to_string(),
            (Value::Resource(left), Value::Resource(right)) => Rc::ptr_eq(left, right),
            (Value::Builtin(left), Value::Builtin(right)) => Rc::ptr_eq(left, right),
            _ => false,
        }
    }

    /// Total ordering for `<`, `sort`, and friends. `None` when incomparable.
    pub fn cmp_value(&self, other: &Value) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Value::Int(left), Value::Int(right)) => Some(left.cmp(right)),
            (Value::Float(left), Value::Float(right)) => left.partial_cmp(right),
            (Value::Int(left), Value::Float(right)) => (*left as f64).partial_cmp(right),
            (Value::Float(left), Value::Int(right)) => left.partial_cmp(&(*right as f64)),
            (Value::Str(left), Value::Str(right)) => Some(left.cmp(right)),
            (Value::Keyword(left), Value::Keyword(right)) => Some(left.cmp(right)),
            (Value::Bool(left), Value::Bool(right)) => Some(left.cmp(right)),
            (Value::Vector(left), Value::Vector(right)) => {
                for (a, b) in left.iter().zip(right.iter()) {
                    match a.cmp_value(b)? {
                        std::cmp::Ordering::Equal => continue,
                        other => return Some(other),
                    }
                }
                Some(left.len().cmp(&right.len()))
            }
            _ => None,
        }
    }
}

/// Display form used by `println` and string interpolation: strings unquoted.
pub struct Display<'a>(pub &'a Value);

impl fmt::Display for Display<'_> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Value::Str(text) => out.write_str(text),
            other => write!(out, "{other}"),
        }
    }
}

/// Debug form used by the REPL and `inspect`: strings quoted, fully structural.
impl fmt::Display for Value {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => out.write_str("nil"),
            Value::Bool(value) => write!(out, "{value}"),
            Value::Int(value) => write!(out, "{value}"),
            Value::Float(value) => out.write_str(&korben_syntax::format_float(*value)),
            Value::Str(text) => out.write_str(&korben_syntax::diag::json_string(text)),
            Value::Keyword(name) => write!(out, ":{name}"),
            Value::Symbol(name) => write!(out, "'{name}"),
            Value::Vector(items) => {
                out.write_str("[")?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.write_str(" ")?;
                    }
                    write!(out, "{item}")?;
                }
                out.write_str("]")
            }
            Value::Set(items) => {
                out.write_str("#{")?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.write_str(" ")?;
                    }
                    write!(out, "{item}")?;
                }
                out.write_str("}")
            }
            Value::Map(map) => {
                out.write_str("{")?;
                for (index, (key, value)) in map.entries.iter().enumerate() {
                    if index > 0 {
                        out.write_str(" ")?;
                    }
                    write!(out, "{key} {value}")?;
                }
                out.write_str("}")
            }
            Value::Record(record) => {
                if let Some(name) = &record.type_name {
                    write!(out, "{name}")?;
                }
                out.write_str("{")?;
                for (index, (field, value)) in record.fields.iter().enumerate() {
                    if index > 0 {
                        out.write_str(" ")?;
                    }
                    write!(out, "{field} {value}")?;
                }
                out.write_str("}")
            }
            Value::Variant(variant) => {
                if variant.fields.is_empty() {
                    return write!(out, "({})", variant.variant);
                }
                write!(out, "({}", variant.variant)?;
                for (_, value) in &variant.fields {
                    write!(out, " {value}")?;
                }
                out.write_str(")")
            }
            Value::Closure(closure) => write!(out, "#<fn {}>", closure.decl.name),
            Value::Native(native) => write!(out, "#<native {}>", native.name),
            Value::Cell(cell) => write!(out, "#<cell {}>", cell.borrow()),
            Value::Syntax(syntax) => write!(out, "#<syntax {syntax}>"),
            Value::Resource(resource) => {
                let state = if resource.is_closed() { "closed" } else { "open" };
                write!(out, "#<{} {state}>", resource.kind)
            }
            Value::Builtin(builtin) => match &**builtin {
                Builtin::Ctor { type_name, variant, .. } => match variant {
                    Some(variant) => write!(out, "#<constructor {type_name}/{variant}>"),
                    None => write!(out, "#<constructor {type_name}>"),
                },
                Builtin::Method { name, .. } => write!(out, "#<method {name}>"),
            },
        }
    }
}
