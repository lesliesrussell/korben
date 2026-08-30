//! Runtime values.
//!
//! Collections are immutable at the language level; the runtime uses reference
//! counting with copy-on-write so ordinary code never observes shared mutation.
//! Mutable state is explicit through `var` bindings and `Cell`.

// korben-vtx

use crate::loc::{Fault, Loc};
use std::any::Any;
use std::cell::RefCell;
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
    /// A quoted symbol, produced by `'name`.
    Symbol(Sym),
    Vector(Rc<Vec<Value>>),
    Set(Rc<Vec<Value>>),
    Map(Rc<MapValue>),
    Record(Rc<RecordValue>),
    Variant(Rc<VariantValue>),
    /// Explicit mutable state (`Cell.new`).
    Cell(Rc<RefCell<Value>>),
    Fn(Rc<Function>),
    /// A payload the host owns: an interpreter closure, a syntax object, or a
    /// native resource. The runtime moves these around without inspecting them.
    Foreign(Rc<Foreign>),
}

// korben-90o
/// A hash for keys whose equality is simple enough to mirror exactly.
///
/// Returning `None` means "do not index this key" and costs only the linear
/// scan that every lookup used to do, so the fallback is always correct. That
/// is the point: `eq_value` compares collections, records and variants
/// order-insensitively and structurally, and a hash that disagreed with it
/// even once would produce a silently wrong lookup. Only shapes where
/// agreement is obvious are indexed.
///
/// The numeric tower is the one subtlety. `eq_value` holds that `Int(1)`
/// equals `Float(1.0)`, so an integral float must hash as the integer it
/// equals. That also settles `-0.0`, which `==` says equals `0.0` while its
/// bits do not: both are integral and hash through the integer path.
fn hash_key(key: &Value) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match key {
        Value::Nil => 0u8.hash(&mut hasher),
        Value::Bool(flag) => (1u8, flag).hash(&mut hasher),
        Value::Int(number) => (2u8, number).hash(&mut hasher),
        Value::Float(number) => {
            // Hash as an integer when the value is one, so the numeric tower
            // agrees. Otherwise the bits are exact and only equal themselves.
            if number.is_finite()
                && number.fract() == 0.0
                && *number >= i64::MIN as f64
                && *number <= i64::MAX as f64
            {
                (2u8, *number as i64).hash(&mut hasher);
            } else {
                (3u8, number.to_bits()).hash(&mut hasher);
            }
        }
        Value::Str(text) => (4u8, text.as_str()).hash(&mut hasher),
        Value::Keyword(name) => (5u8, name.as_ref()).hash(&mut hasher),
        Value::Symbol(name) => (6u8, name.as_ref()).hash(&mut hasher),
        // Everything else compares structurally; fall back to scanning.
        _ => return None,
    }
    Some(hasher.finish())
}

/// An insertion-ordered map. Korben guarantees deterministic iteration order.
///
/// `entries` carries that order and is what every reader iterates.
///
/// `index` is a *cache*, not state: a lazily built map from key hash to the
/// positions holding it. Looking a key up used to walk the whole map calling
/// `eq_value` on every entry, which made ordinary application code quadratic,
/// because the language offers no other way to find a value by key.
///
/// It is deliberately lazy and deliberately dropped on clone. Korben's maps
/// are persistent -- `assoc` copies the map -- so building an index eagerly
/// would put the cost of one on every insert, and measurably did: building a
/// 4000-entry map went from 47ms to 178ms before this was made lazy. A clone
/// therefore starts with no index, and only a map that is actually looked up,
/// and big enough to be worth it, ever builds one.
#[derive(Default)]
pub struct MapValue {
    pub entries: Vec<(Value, Value)>,
    index: std::cell::RefCell<Option<std::collections::HashMap<u64, Vec<usize>>>>,
}

/// Below this many entries, scanning beats hashing and the index is not
/// built at all.
///
/// The number is set from measurement, not taste. Records, query strings,
/// header sets and decoded JSON objects are the common maps and they are
/// small: a beads issue decodes to 18 keys. At that size, comparing keywords
/// down a short vector is faster than hashing plus a `HashMap` probe, and an
/// earlier threshold of 16 made a real workload 10-13% *slower* by pulling
/// every one of those field accesses onto the indexed path. The index is for
/// maps large enough that the scan is the thing that hurts.
const INDEX_THRESHOLD: usize = 128;

impl Clone for MapValue {
    fn clone(&self) -> Self {
        MapValue { entries: self.entries.clone(), index: std::cell::RefCell::new(None) }
    }
}

impl MapValue {
    fn scan(&self, key: &Value) -> Option<usize> {
        self.entries.iter().position(|(existing, _)| existing.eq_value(key))
    }

    /// Where `key` lives, if anywhere.
    ///
    /// `may_build` is what keeps writes cheap. A lookup is happy to pay once
    /// for an index it will reuse, but `insert` and `remove` must not: maps
    /// are persistent, so a write usually lands on a freshly copied map whose
    /// index is empty, and building one per write is how this change first
    /// made building a 4000-entry map 8x slower instead of faster. A write
    /// uses an index that already exists and otherwise scans.
    fn position(&self, key: &Value, may_build: bool) -> Option<usize> {
        let hash = match hash_key(key) {
            Some(hash) if self.entries.len() >= INDEX_THRESHOLD => hash,
            // Either the key is one the index refuses, or the map is small
            // enough that scanning is the cheaper answer. Both are correct.
            _ => return self.scan(key),
        };
        if !may_build && self.index.borrow().is_none() {
            return self.scan(key);
        }

        let mut cache = self.index.borrow_mut();
        let index = cache.get_or_insert_with(|| {
            let mut built: std::collections::HashMap<u64, Vec<usize>> =
                std::collections::HashMap::with_capacity(self.entries.len());
            for (at, (existing, _)) in self.entries.iter().enumerate() {
                if let Some(hash) = hash_key(existing) {
                    built.entry(hash).or_default().push(at);
                }
            }
            built
        });
        // `index` borrows only `self.index`; `self.entries` is a separate
        // field and is read directly, so the candidates need not be copied out
        // -- an allocation per lookup would undo much of what the index buys.
        index.get(&hash)?.iter().copied().find(|at| self.entries[*at].0.eq_value(key))
    }

    pub fn get(&self, key: &Value) -> Option<&Value> {
        self.position(key, true).map(|at| &self.entries[at].1)
    }

    pub fn insert(&mut self, key: Value, value: Value) {
        match self.position(&key, false) {
            Some(at) => self.entries[at].1 = value,
            None => {
                // Appending keeps every existing position valid, so an index
                // that already exists is extended rather than thrown away.
                let at = self.entries.len();
                if let Some(cached) = self.index.borrow_mut().as_mut() {
                    if let Some(hash) = hash_key(&key) {
                        cached.entry(hash).or_default().push(at);
                    }
                }
                self.entries.push((key, value));
            }
        }
    }

    pub fn remove(&mut self, key: &Value) -> Option<Value> {
        let at = self.position(key, false)?;
        let removed = self.entries.remove(at).1;
        // Removing shifts every later position, so the cache is dropped rather
        // than patched. A patched index that drifted would be worse than a
        // rebuilt one, and the next lookup pays for it only if it needs to.
        *self.index.borrow_mut() = None;
        Some(removed)
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

/// A host-owned payload, tagged so diagnostics can name its type.
pub struct Foreign {
    pub kind: &'static str,
    pub payload: Box<dyn Any>,
}

impl Foreign {
    /// Wrap a host payload as a value. Returns a [`Value`] rather than a
    /// `Foreign` because callers never need the wrapper on its own.
    pub fn wrap(kind: &'static str, payload: impl Any) -> Value {
        Value::Foreign(Rc::new(Foreign { kind, payload: Box::new(payload) }))
    }

    pub fn downcast<T: Any>(&self) -> Option<&T> {
        self.payload.downcast_ref::<T>()
    }
}

/// One declared parameter, used for arity checks and keyword binding.
pub struct Param {
    pub name: Sym,
    /// `Some("port")` when the caller passes this argument as `:port value`.
    pub keyword: Option<Sym>,
    pub has_default: bool,
}

/// A callable value.
pub struct Function {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Body,
    /// Calling an async function yields a task rather than running it.
    pub is_async: bool,
}

/// Native functions receive a caller so higher-order builtins such as `map`
/// can invoke Korben functions in either execution mode.
pub type NativeFn = fn(&mut dyn Caller, Vec<Value>, Loc) -> Outcome;

/// Generated native code installs its functions as boxed Rust closures.
pub type RustFn = Box<dyn Fn(&mut dyn Caller, Vec<Arg>, Loc) -> Outcome>;

pub enum Body {
    /// A standard-library function, shared by both execution modes.
    Native(NativeFn),
    /// A function compiled to Rust.
    Rust(RustFn),
    /// A record or enum constructor.
    Ctor { type_name: Sym, variant: Option<Sym>, fields: Vec<Sym> },
    /// A protocol method dispatching on its first argument.
    Method { protocol: Sym, name: Sym },
    /// A closure the host evaluates itself, such as an interpreter closure.
    Host(Box<dyn Any>),
}

/// One argument at a call site.
#[derive(Clone)]
pub struct Arg {
    pub keyword: Option<String>,
    pub value: Value,
}

impl Arg {
    pub fn positional(value: Value) -> Arg {
        Arg { keyword: None, value }
    }

    pub fn named(keyword: &str, value: Value) -> Arg {
        Arg { keyword: Some(keyword.to_string()), value }
    }
}

/// Non-local control flow.
pub enum Flow {
    /// `recur` to the enclosing `loop` or function head.
    Recur(Vec<Value>),
    /// Postfix `?` propagating an `Err` or `None` out of the current function.
    Propagate(Value),
    /// A thrown condition, catchable by `try`.
    Condition(Value, Loc),
    /// An unrecoverable fault.
    Panic(Box<Fault>),
}

impl Flow {
    pub fn fault(fault: Fault) -> Flow {
        Flow::Panic(Box::new(fault))
    }
}

pub type Outcome = Result<Value, Flow>;

/// What the runtime needs from its host.
///
/// The interpreter and generated code both implement this, which is what lets a
/// single copy of the standard library serve both execution modes. Call a
/// function value with [`crate::apply`], never through this trait directly:
/// `call_host` exists only so `apply` can hand back the bodies it cannot run.
pub trait Caller {
    /// Invoke a closure whose body the host owns.
    fn call_host(&mut self, function: &Value, args: Vec<Arg>, loc: Loc) -> Outcome;

    /// Find an implementation of `method` for `receiver`, if one is registered.
    fn find_method(&mut self, receiver: &Value, method: &str) -> Option<Value>;

    /// Write program output. The REPL and test runner capture it.
    fn write(&mut self, text: &str);
}

// ------------------------------------------------------------- constructors

impl Value {
    pub fn str(text: impl Into<String>) -> Value {
        Value::Str(Rc::new(text.into()))
    }

    pub fn keyword(name: &str) -> Value {
        Value::Keyword(Rc::from(name))
    }

    pub fn symbol(name: &str) -> Value {
        Value::Symbol(Rc::from(name))
    }

    pub fn vector(items: Vec<Value>) -> Value {
        Value::Vector(Rc::new(items))
    }

    /// Sets keep insertion order and drop duplicates, which is what the
    /// reader and the evaluator both produce.
    pub fn set(items: Vec<Value>) -> Value {
        let mut unique: Vec<Value> = Vec::with_capacity(items.len());
        for item in items {
            if !unique.iter().any(|existing| existing.eq_value(&item)) {
                unique.push(item);
            }
        }
        Value::Set(Rc::new(unique))
    }

    pub fn map(entries: Vec<(Value, Value)>) -> Value {
        let mut map = MapValue::default();
        for (key, value) in entries {
            map.insert(key, value);
        }
        Value::Map(Rc::new(map))
    }

    pub fn record(type_name: Option<&str>, fields: Vec<(&str, Value)>) -> Value {
        Value::Record(Rc::new(RecordValue {
            type_name: type_name.map(Rc::from),
            fields: fields.into_iter().map(|(name, value)| (Rc::from(name), value)).collect(),
        }))
    }

    pub fn variant(type_name: &str, variant: &str, fields: Vec<(&str, Value)>) -> Value {
        Value::Variant(Rc::new(VariantValue {
            type_name: Rc::from(type_name),
            variant: Rc::from(variant),
            fields: fields.into_iter().map(|(name, value)| (Rc::from(name), value)).collect(),
        }))
    }

    pub fn ok(value: Value) -> Value {
        Value::variant("Result", "Ok", vec![("value", value)])
    }

    pub fn err(value: Value) -> Value {
        Value::variant("Result", "Err", vec![("error", value)])
    }

    pub fn some(value: Value) -> Value {
        Value::variant("Option", "Some", vec![("value", value)])
    }

    pub fn none() -> Value {
        Value::variant("Option", "None", Vec::new())
    }

    pub fn native(name: &str, params: Vec<Param>, func: NativeFn) -> Value {
        Value::Fn(Rc::new(Function {
            name: name.to_string(),
            params,
            body: Body::Native(func),
            is_async: false,
        }))
    }

    pub fn ctor(type_name: &str, variant: Option<&str>, fields: &[&str]) -> Value {
        Value::Fn(Rc::new(Function {
            name: variant.unwrap_or(type_name).to_string(),
            params: Vec::new(),
            is_async: false,
            body: Body::Ctor {
                type_name: Rc::from(type_name),
                variant: variant.map(Rc::from),
                fields: fields.iter().map(|field| Rc::from(*field)).collect(),
            },
        }))
    }

    pub fn method(protocol: &str, name: &str) -> Value {
        Value::Fn(Rc::new(Function {
            name: name.to_string(),
            params: Vec::new(),
            body: Body::Method { protocol: Rc::from(protocol), name: Rc::from(name) },
            is_async: false,
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
            Value::Cell(_) => "Cell".to_string(),
            Value::Fn(_) => "Fn".to_string(),
            Value::Record(record) => record
                .type_name
                .as_ref()
                .map(|name| name.to_string())
                .unwrap_or_else(|| "Record".to_string()),
            Value::Variant(variant) => variant.type_name.to_string(),
            Value::Foreign(foreign) => foreign.kind.to_string(),
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
            (Value::Fn(left), Value::Fn(right)) => Rc::ptr_eq(left, right),
            (Value::Foreign(left), Value::Foreign(right)) => Rc::ptr_eq(left, right),
            _ => false,
        }
    }

    /// Ordering for `<`, `sort`, and friends. `None` when incomparable.
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

/// Look up a member by name in a record, variant, or map, accepting keyword or
/// string keys. This is the single definition both execution modes use.
pub fn member(value: &Value, name: &str) -> Option<Value> {
    match value {
        Value::Record(record) => record.get(name).cloned(),
        Value::Variant(variant) => variant.get(name).cloned(),
        Value::Map(map) => {
            map.get(&Value::Keyword(Rc::from(name))).or_else(|| map.get(&Value::str(name))).cloned()
        }
        _ => None,
    }
}

/// The names a value exposes, for `no such field` diagnostics.
pub fn member_names(value: &Value) -> String {
    let names: Vec<String> = match value {
        Value::Record(record) => record.fields.iter().map(|(name, _)| name.to_string()).collect(),
        Value::Variant(variant) => {
            variant.fields.iter().map(|(name, _)| name.to_string()).collect()
        }
        Value::Map(map) => map.entries.iter().map(|(key, _)| display(key)).collect(),
        _ => Vec::new(),
    };
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    }
}

// ---------------------------------------------------------------- rendering

/// Canonical float rendering: always shows a decimal point.
pub fn format_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let mut text = format!("{value}");
    if !text.contains(['.', 'e', 'E']) {
        text.push_str(".0");
    }
    text
}

/// Escape a string as a JSON string literal.
pub fn quote_string(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Display form used by `println` and interpolation: strings unquoted.
pub fn display(value: &Value) -> String {
    match value {
        Value::Str(text) => (**text).clone(),
        other => other.to_string(),
    }
}

/// Debug form used by the REPL: strings quoted, fully structural.
impl fmt::Display for Value {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => out.write_str("nil"),
            Value::Bool(value) => write!(out, "{value}"),
            Value::Int(value) => write!(out, "{value}"),
            Value::Float(value) => out.write_str(&format_float(*value)),
            Value::Str(text) => out.write_str(&quote_string(text)),
            Value::Keyword(name) => write!(out, ":{name}"),
            Value::Symbol(name) => write!(out, "'{name}"),
            Value::Vector(items) => write_seq(out, "[", items, "]"),
            Value::Set(items) => write_seq(out, "#{", items, "}"),
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
            Value::Cell(cell) => write!(out, "#<cell {}>", cell.borrow()),
            Value::Fn(function) => write!(out, "#<fn {}>", function.name),
            Value::Foreign(foreign) => write!(out, "#<{}>", foreign.kind),
        }
    }
}

fn write_seq(
    out: &mut fmt::Formatter<'_>,
    open: &str,
    items: &[Value],
    close: &str,
) -> fmt::Result {
    out.write_str(open)?;
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.write_str(" ")?;
        }
        write!(out, "{item}")?;
    }
    out.write_str(close)
}

// korben-90o
#[cfg(test)]
mod map_tests {
    use super::*;

    fn map(pairs: &[(Value, Value)]) -> MapValue {
        let mut built = MapValue::default();
        for (key, value) in pairs {
            built.insert(key.clone(), value.clone());
        }
        built
    }

    /// The index must agree with `eq_value` exactly. `eq_value` holds that an
    /// integer equals an equal float, so a lookup by one must find the other
    /// -- the case a naive hash gets wrong and gets wrong silently.
    #[test]
    fn the_numeric_tower_agrees_with_equality() {
        let built = map(&[(Value::Int(1), Value::str("one"))]);
        assert!(Value::Int(1).eq_value(&Value::Float(1.0)));
        assert_eq!(built.get(&Value::Float(1.0)).map(display), Some("one".to_string()));

        let built = map(&[(Value::Float(2.0), Value::str("two"))]);
        assert_eq!(built.get(&Value::Int(2)).map(display), Some("two".to_string()));

        // Inserting the other spelling of the same number replaces it rather
        // than adding a second entry.
        let mut built = map(&[(Value::Int(3), Value::str("first"))]);
        built.insert(Value::Float(3.0), Value::str("second"));
        assert_eq!(built.len(), 1);
        assert_eq!(built.get(&Value::Int(3)).map(display), Some("second".to_string()));
    }

    /// `-0.0 == 0.0` is true while their bits differ, so hashing the bits
    /// would lose the entry.
    #[test]
    fn negative_zero_finds_positive_zero() {
        let built = map(&[(Value::Float(0.0), Value::str("zero"))]);
        assert!(Value::Float(-0.0).eq_value(&Value::Float(0.0)));
        assert_eq!(built.get(&Value::Float(-0.0)).map(display), Some("zero".to_string()));
    }

    /// A float that is not a whole number is only equal to itself, and a NaN
    /// key is equal to nothing at all -- including itself.
    #[test]
    fn fractional_and_nan_keys_behave_like_equality_says() {
        let built = map(&[(Value::Float(1.5), Value::str("half"))]);
        assert_eq!(built.get(&Value::Float(1.5)).map(display), Some("half".to_string()));
        assert!(built.get(&Value::Float(2.5)).is_none());

        let built = map(&[(Value::Float(f64::NAN), Value::str("nan"))]);
        assert!(built.get(&Value::Float(f64::NAN)).is_none());
    }

    /// Keys the index refuses still work, because the fallback is the scan
    /// every lookup used to do. Mixing them with indexed keys must not let
    /// either kind hide the other.
    #[test]
    fn unindexed_keys_still_resolve_alongside_indexed_ones() {
        let vector = Value::vector(vec![Value::Int(1), Value::Int(2)]);
        let mut built = MapValue::default();
        built.insert(vector.clone(), Value::str("by vector"));
        built.insert(Value::keyword("plain"), Value::str("by keyword"));

        assert_eq!(built.get(&vector).map(display), Some("by vector".to_string()));
        assert_eq!(
            built.get(&Value::keyword("plain")).map(display),
            Some("by keyword".to_string())
        );
        assert!(built.get(&Value::vector(vec![Value::Int(9)])).is_none());
    }

    /// Iteration order is a documented guarantee, so insertion order must
    /// survive both replacement and removal.
    #[test]
    fn insertion_order_survives_replacement_and_removal() {
        let mut built = MapValue::default();
        for name in ["a", "b", "c", "d"] {
            built.insert(Value::keyword(name), Value::str(name));
        }
        built.insert(Value::keyword("b"), Value::str("replaced"));
        let order: Vec<String> = built.entries.iter().map(|(key, _)| display(key)).collect();
        assert_eq!(order, [":a", ":b", ":c", ":d"]);

        assert_eq!(
            built.remove(&Value::keyword("b")).map(|v| display(&v)),
            Some("replaced".to_string())
        );
        let order: Vec<String> = built.entries.iter().map(|(key, _)| display(key)).collect();
        assert_eq!(order, [":a", ":c", ":d"]);

        // Everything still resolves after the positions shifted underneath.
        assert_eq!(built.get(&Value::keyword("d")).map(display), Some("d".to_string()));
        assert!(built.get(&Value::keyword("b")).is_none());
    }

    /// The shape the HTTP accept loop uses: many keys inserted, then removed
    /// one at a time. This is what was quadratic.
    #[test]
    fn many_keys_insert_find_and_remove_correctly() {
        let mut built = MapValue::default();
        for id in 0..500i64 {
            built.insert(Value::Int(id), Value::str(format!("connection {id}")));
        }
        assert_eq!(built.len(), 500);
        assert_eq!(built.get(&Value::Int(499)).map(display), Some("connection 499".to_string()));

        for id in (0..500i64).step_by(2) {
            assert_eq!(
                built.remove(&Value::Int(id)).map(|v| display(&v)),
                Some(format!("connection {id}"))
            );
        }
        assert_eq!(built.len(), 250);
        assert!(built.get(&Value::Int(250)).is_none());
        assert_eq!(built.get(&Value::Int(251)).map(display), Some("connection 251".to_string()));
    }
}
