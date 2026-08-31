//! The standard library.
//!
//! One implementation, shared by the interpreter and by generated native code.
//! Every function is addressed by its canonical Korben name, `module/name`, so
//! the core IR can name exactly what a program needs and the backend can prove
//! it is available before generating anything.

// korben-vtx

use crate::apply::apply;
use crate::loc::{Fault, Loc};
use crate::value::{
    display, member, Arg, Caller, Flow, MapValue, Outcome, Param, RecordValue, Sym, Value,
};
use std::cell::RefCell;
use std::rc::Rc;

/// Build a native function value with `arity` required parameters.
fn native(name: &str, arity: usize, func: crate::value::NativeFn) -> Value {
    let params = (0..arity)
        .map(|index| Param {
            name: Rc::from(format!("a{index}").as_str()),
            keyword: None,
            has_default: false,
        })
        .collect();
    Value::native(name, params, func)
}

fn wrong_type(name: &str, expected: &str, got: &Value, loc: Loc) -> Flow {
    Flow::fault(
        Fault::new("type-error", format!("`{name}` expected {expected}"), loc)
            .label(format!("found {} (`{got}`)", got.type_name())),
    )
}

fn as_int(name: &str, value: &Value, loc: Loc) -> Result<i64, Flow> {
    match value {
        Value::Int(value) => Ok(*value),
        other => Err(wrong_type(name, "an Int", other, loc)),
    }
}

fn as_string(name: &str, value: &Value, loc: Loc) -> Result<String, Flow> {
    match value {
        Value::Str(text) => Ok((**text).clone()),
        other => Err(wrong_type(name, "a String", other, loc)),
    }
}

fn as_vector(name: &str, value: &Value, loc: Loc) -> Result<Rc<Vec<Value>>, Flow> {
    match value {
        Value::Vector(items) => Ok(items.clone()),
        other => Err(wrong_type(name, "a Vec", other, loc)),
    }
}

fn numeric(name: &str, left: &Value, right: &Value, loc: Loc) -> Result<(f64, f64, bool), Flow> {
    let (left_value, left_float) = match left {
        Value::Int(value) => (*value as f64, false),
        Value::Float(value) => (*value, true),
        other => return Err(wrong_type(name, "a number", other, loc)),
    };
    let (right_value, right_float) = match right {
        Value::Int(value) => (*value as f64, false),
        Value::Float(value) => (*value, true),
        other => return Err(wrong_type(name, "a number", other, loc)),
    };
    Ok((left_value, right_value, left_float || right_float))
}

fn arith(
    name: &'static str,
    args: &[Value],
    loc: Loc,
    identity: i64,
    op: fn(f64, f64) -> f64,
    int_op: fn(i64, i64) -> Option<i64>,
) -> Outcome {
    if args.is_empty() {
        return Ok(Value::Int(identity));
    }
    if args.len() == 1 {
        // Unary `-` negates and unary `/` inverts.
        return arith(name, &[Value::Int(identity), args[0].clone()], loc, identity, op, int_op);
    }
    let mut accumulator = args[0].clone();
    for next in &args[1..] {
        let (left, right, is_float) = numeric(name, &accumulator, next, loc)?;
        accumulator = if is_float {
            Value::Float(op(left, right))
        } else {
            match int_op(left as i64, right as i64) {
                Some(value) => Value::Int(value),
                None => {
                    return Err(Flow::fault(
                        Fault::new("overflow", format!("integer overflow in `{name}`"), loc)
                            .label(format!("{left} {name} {right} does not fit in Int")),
                    ))
                }
            }
        };
    }
    Ok(accumulator)
}

fn compare(
    name: &'static str,
    args: &[Value],
    loc: Loc,
    test: fn(std::cmp::Ordering) -> bool,
) -> Outcome {
    for pair in args.windows(2) {
        match pair[0].cmp_value(&pair[1]) {
            Some(ordering) if test(ordering) => continue,
            Some(_) => return Ok(Value::Bool(false)),
            None => {
                return Err(Flow::fault(
                    Fault::new("type-error", format!("`{name}` cannot compare these values"), loc)
                        .label(format!("{} and {}", pair[0].type_name(), pair[1].type_name())),
                ))
            }
        }
    }
    Ok(Value::Bool(true))
}

pub(crate) fn io_error(path: &str, error: &std::io::Error) -> Value {
    Value::Record(Rc::new(RecordValue {
        type_name: Some(Rc::from("IoError")),
        fields: vec![
            (Rc::from("path"), Value::str(path.to_string())),
            (Rc::from("kind"), Value::keyword(&format!("{:?}", error.kind()).to_lowercase())),
            (Rc::from("message"), Value::str(error.to_string())),
        ],
    }))
}

/// Every function the runtime provides, by canonical name.
pub const NAMES: &[&str] = &[
    // std.core: arithmetic and comparison
    "std.core/+",
    "std.core/-",
    "std.core/*",
    "std.core//",
    "std.core/mod",
    "std.core/inc",
    "std.core/dec",
    "std.core/min",
    "std.core/max",
    "std.core/=",
    "std.core/not=",
    "std.core/not",
    "std.core/<",
    "std.core/<=",
    "std.core/>",
    "std.core/>=",
    // std.core: collections
    "std.core/len",
    "std.core/empty?",
    "std.core/first",
    "std.core/last",
    "std.core/rest",
    "std.core/nth",
    "std.core/conj",
    "std.core/concat",
    "std.core/reverse",
    "std.core/contains?",
    "std.core/get",
    "std.core/assoc",
    "std.core/dissoc",
    "std.core/keys",
    "std.core/values",
    "std.core/range",
    "std.core/map",
    "std.core/filter",
    "std.core/reduce",
    "std.core/each",
    "std.core/any?",
    "std.core/all?",
    "std.core/sort",
    "std.core/sort-by",
    // std.core: Option and Result
    "std.core/Some",
    "std.core/Ok",
    "std.core/Err",
    "std.core/None",
    "std.core/some?",
    "std.core/ok?",
    "std.core/nil?",
    "std.core/unwrap-or",
    // std.core: conversion and output
    "std.core/str",
    "std.core/type-of",
    "std.core/clone",
    "std.core/keyword",
    "std.core/identity",
    "std.core/println",
    "std.core/print",
    "std.core/eprintln",
    "std.core/assert",
    "std.core/assert-eq",
    "std.core/assert-ne",
    "std.core/uuid/parse",
    "std.core/date/parse",
    "std.core/duration/parse",
    // std.io
    "std.io/println",
    "std.io/print",
    "std.io/eprintln",
    "std.io/read-line",
    // std.string
    "std.string/split",
    "std.string/join",
    "std.string/trim",
    "std.string/upper",
    "std.string/lower",
    "std.string/starts-with?",
    "std.string/ends-with?",
    "std.string/replace",
    "std.string/chars",
    "std.string/parse-int",
    "std.string/parse-float",
    "std.string/split-once",
    "std.string/byte-length",
    "std.string/repeat",
    // std.math
    "std.math/abs",
    "std.math/sqrt",
    "std.math/pow",
    "std.math/floor",
    "std.math/ceil",
    // std.fs
    "std.fs/read-text",
    "std.fs/write-text",
    // korben-0mo
    "std.fs/rename",
    "std.fs/exists?",
    "std.fs/read-lines",
    "std.fs/list-dir",
    // std.net: blocking TCP
    "std.net/listen",
    "std.net/connect",
    // korben-ggd
    "std.net/connect-tls",
    // korben-ae2
    "std.net/pool",
    "Pool/wait",
    "Pool/read",
    "Pool/write",
    "Pool/close-connection",
    "Pool/evict",
    "Pool/address",
    "Pool/close",
    "Pool/closed?",
    "Pool/drop",
    "Listener/accept",
    "Listener/address",
    "Listener/close",
    "Listener/closed?",
    "Listener/drop",
    "Connection/read",
    "Connection/write",
    "Connection/peer",
    "Connection/close",
    "Connection/closed?",
    "Connection/drop",
    "std.fs/open",
    "std.fs/create",
    // File: a resource with deterministic cleanup.
    "File/write",
    "File/read-text",
    "File/close",
    "File/closed?",
    "File/drop",
    // std.json
    "std.json/encode",
    "std.json/encode-pretty",
    "std.json/decode",
    // std.log
    "std.log/debug",
    "std.log/info",
    "std.log/warn",
    "std.log/error",
    // std.time
    "std.time/now-millis",
    "std.time/sleep-millis",
    // std.process
    "std.process/args",
    "std.process/shutdown-requested?",
    "std.process/env",
    "std.process/exit",
    // std.test
    "std.test/assert",
    "std.test/assert-eq",
    "std.test/assert-ne",
    // std.async: tasks, scopes, and channels
    "std.async/join-all",
    "std.async/join",
    "std.async/channel",
    "std.async/bounded",
    "Scope/cancel",
    "Scope/cancelled?",
    "Task/cancel",
    "Task/state",
    "Sender/send",
    "Sender/close",
    "Sender/len",
    "Receiver/recv",
    "Receiver/try-recv",
    "Receiver/close",
    "Receiver/len",
    // Cell
    "Cell/new",
    "Cell/get",
    "Cell/set",
    "Cell/update",
];

/// Look up a runtime function by canonical name.
pub fn builtin(name: &str) -> Option<Value> {
    let value = match name {
        "std.core/+" => {
            native("+", 0, |_, args, loc| arith("+", &args, loc, 0, |a, b| a + b, i64::checked_add))
        }
        "std.core/-" => {
            native("-", 0, |_, args, loc| arith("-", &args, loc, 0, |a, b| a - b, i64::checked_sub))
        }
        "std.core/*" => {
            native("*", 0, |_, args, loc| arith("*", &args, loc, 1, |a, b| a * b, i64::checked_mul))
        }
        "std.core//" => native("/", 0, |_, args, loc| {
            // Division by zero is a fault, not a silent NaN.
            for value in args.iter().skip(1) {
                if value.eq_value(&Value::Int(0)) {
                    return Err(Flow::fault(
                        Fault::new("divide-by-zero", "division by zero", loc)
                            .label("the divisor is zero"),
                    ));
                }
            }
            arith("/", &args, loc, 1, |a, b| a / b, |a, b| a.checked_div(b))
        }),
        "std.core/mod" => native("mod", 2, |_, args, loc| {
            let right = as_int("mod", &args[1], loc)?;
            if right == 0 {
                return Err(Flow::fault(
                    Fault::new("divide-by-zero", "modulo by zero", loc)
                        .label("the divisor is zero"),
                ));
            }
            Ok(Value::Int(as_int("mod", &args[0], loc)?.rem_euclid(right)))
        }),
        "std.core/inc" => {
            native("inc", 1, |_, args, loc| Ok(Value::Int(as_int("inc", &args[0], loc)? + 1)))
        }
        "std.core/dec" => {
            native("dec", 1, |_, args, loc| Ok(Value::Int(as_int("dec", &args[0], loc)? - 1)))
        }
        "std.core/min" => native("min", 2, |_, args, loc| {
            let (left, right, is_float) = numeric("min", &args[0], &args[1], loc)?;
            Ok(if is_float {
                Value::Float(left.min(right))
            } else {
                Value::Int(left.min(right) as i64)
            })
        }),
        "std.core/max" => native("max", 2, |_, args, loc| {
            let (left, right, is_float) = numeric("max", &args[0], &args[1], loc)?;
            Ok(if is_float {
                Value::Float(left.max(right))
            } else {
                Value::Int(left.max(right) as i64)
            })
        }),
        "std.core/=" => native("=", 2, |_, args, _| {
            Ok(Value::Bool(args.windows(2).all(|pair| pair[0].eq_value(&pair[1]))))
        }),
        "std.core/not=" => native("not=", 2, |_, args, _| {
            Ok(Value::Bool(!args.windows(2).all(|pair| pair[0].eq_value(&pair[1]))))
        }),
        "std.core/not" => native("not", 1, |_, args, _| Ok(Value::Bool(!args[0].is_truthy()))),
        "std.core/<" => native("<", 2, |_, args, loc| compare("<", &args, loc, |o| o.is_lt())),
        "std.core/<=" => native("<=", 2, |_, args, loc| compare("<=", &args, loc, |o| o.is_le())),
        "std.core/>" => native(">", 2, |_, args, loc| compare(">", &args, loc, |o| o.is_gt())),
        "std.core/>=" => native(">=", 2, |_, args, loc| compare(">=", &args, loc, |o| o.is_ge())),

        "std.core/len" => native("len", 1, |_, args, loc| {
            let length = match &args[0] {
                Value::Vector(items) | Value::Set(items) => items.len(),
                Value::Map(map) => map.len(),
                Value::Str(text) => text.chars().count(),
                Value::Record(record) => record.fields.len(),
                other => return Err(wrong_type("len", "a collection", other, loc)),
            };
            Ok(Value::Int(length as i64))
        }),
        "std.core/empty?" => native("empty?", 1, |_, args, loc| {
            let empty = match &args[0] {
                Value::Vector(items) | Value::Set(items) => items.is_empty(),
                Value::Map(map) => map.is_empty(),
                Value::Str(text) => text.is_empty(),
                Value::Nil => true,
                other => return Err(wrong_type("empty?", "a collection", other, loc)),
            };
            Ok(Value::Bool(empty))
        }),
        "std.core/first" => native("first", 1, |_, args, loc| {
            Ok(as_vector("first", &args[0], loc)?
                .first()
                .cloned()
                .map(Value::some)
                .unwrap_or_else(Value::none))
        }),
        "std.core/last" => native("last", 1, |_, args, loc| {
            Ok(as_vector("last", &args[0], loc)?
                .last()
                .cloned()
                .map(Value::some)
                .unwrap_or_else(Value::none))
        }),
        "std.core/rest" => native("rest", 1, |_, args, loc| {
            let items = as_vector("rest", &args[0], loc)?;
            Ok(Value::vector(items.iter().skip(1).cloned().collect()))
        }),
        "std.core/nth" => native("nth", 2, |_, args, loc| {
            let items = as_vector("nth", &args[0], loc)?;
            let index = as_int("nth", &args[1], loc)?;
            if index < 0 || index as usize >= items.len() {
                return Ok(Value::none());
            }
            Ok(Value::some(items[index as usize].clone()))
        }),
        "std.core/conj" => native("conj", 2, |_, args, loc| match &args[0] {
            Value::Vector(items) => {
                let mut next = (**items).clone();
                next.extend(args[1..].iter().cloned());
                Ok(Value::vector(next))
            }
            Value::Set(items) => {
                let mut next = (**items).clone();
                for value in &args[1..] {
                    if !next.iter().any(|existing| existing.eq_value(value)) {
                        next.push(value.clone());
                    }
                }
                Ok(Value::set(next))
            }
            other => Err(wrong_type("conj", "a Vec or Set", other, loc)),
        }),
        "std.core/concat" => native("concat", 0, |_, args, loc| {
            let mut out = Vec::new();
            for value in &args {
                out.extend(as_vector("concat", value, loc)?.iter().cloned());
            }
            Ok(Value::vector(out))
        }),
        "std.core/reverse" => native("reverse", 1, |_, args, loc| {
            let mut items = (*as_vector("reverse", &args[0], loc)?).clone();
            items.reverse();
            Ok(Value::vector(items))
        }),
        "std.core/contains?" => native("contains?", 2, |_, args, loc| {
            let found = match &args[0] {
                Value::Vector(items) | Value::Set(items) => {
                    items.iter().any(|item| item.eq_value(&args[1]))
                }
                Value::Map(map) => map.get(&args[1]).is_some(),
                Value::Str(text) => match &args[1] {
                    Value::Str(needle) => text.contains(needle.as_str()),
                    other => return Err(wrong_type("contains?", "a String", other, loc)),
                },
                other => return Err(wrong_type("contains?", "a collection", other, loc)),
            };
            Ok(Value::Bool(found))
        }),
        "std.core/get" => native("get", 2, |_, args, loc| {
            let fallback = args.get(2).cloned();
            let found = match &args[0] {
                Value::Map(map) => map.get(&args[1]).cloned(),
                Value::Record(_) | Value::Variant(_) => match &args[1] {
                    Value::Keyword(name) | Value::Symbol(name) => member(&args[0], name),
                    Value::Str(name) => member(&args[0], name),
                    _ => None,
                },
                Value::Vector(items) => match &args[1] {
                    Value::Int(index) if *index >= 0 => items.get(*index as usize).cloned(),
                    _ => None,
                },
                other => return Err(wrong_type("get", "a Map, Record, or Vec", other, loc)),
            };
            Ok(found.or(fallback).unwrap_or(Value::Nil))
        }),
        "std.core/assoc" => native("assoc", 3, |_, args, loc| match &args[0] {
            Value::Map(map) => {
                let mut next = (**map).clone();
                for pair in args[1..].chunks(2) {
                    if let [key, value] = pair {
                        next.insert(key.clone(), value.clone());
                    }
                }
                Ok(Value::Map(Rc::new(next)))
            }
            Value::Record(record) => {
                let mut next = (**record).clone();
                for pair in args[1..].chunks(2) {
                    if let [key, value] = pair {
                        let name: Sym = match key {
                            Value::Keyword(name) | Value::Symbol(name) => name.clone(),
                            Value::Str(name) => Rc::from(name.as_str()),
                            other => return Err(wrong_type("assoc", "a field name", other, loc)),
                        };
                        match next.fields.iter_mut().find(|(field, _)| *field == name) {
                            Some(slot) => slot.1 = value.clone(),
                            None => next.fields.push((name, value.clone())),
                        }
                    }
                }
                Ok(Value::Record(Rc::new(next)))
            }
            other => Err(wrong_type("assoc", "a Map or Record", other, loc)),
        }),
        "std.core/dissoc" => native("dissoc", 2, |_, args, loc| match &args[0] {
            Value::Map(map) => {
                let mut next = (**map).clone();
                for key in &args[1..] {
                    next.remove(key);
                }
                Ok(Value::Map(Rc::new(next)))
            }
            other => Err(wrong_type("dissoc", "a Map", other, loc)),
        }),
        "std.core/keys" => native("keys", 1, |_, args, loc| match &args[0] {
            Value::Map(map) => {
                Ok(Value::vector(map.entries.iter().map(|(key, _)| key.clone()).collect()))
            }
            Value::Record(record) => Ok(Value::vector(
                record.fields.iter().map(|(name, _)| Value::Keyword(name.clone())).collect(),
            )),
            other => Err(wrong_type("keys", "a Map or Record", other, loc)),
        }),
        "std.core/values" => native("values", 1, |_, args, loc| match &args[0] {
            Value::Map(map) => {
                Ok(Value::vector(map.entries.iter().map(|(_, value)| value.clone()).collect()))
            }
            Value::Record(record) => {
                Ok(Value::vector(record.fields.iter().map(|(_, value)| value.clone()).collect()))
            }
            other => Err(wrong_type("values", "a Map or Record", other, loc)),
        }),
        "std.core/range" => native("range", 1, |_, args, loc| {
            let (start, end) = if args.len() >= 2 {
                (as_int("range", &args[0], loc)?, as_int("range", &args[1], loc)?)
            } else {
                (0, as_int("range", &args[0], loc)?)
            };
            Ok(Value::vector((start..end).map(Value::Int).collect()))
        }),

        "std.core/map" => native("map", 2, |caller, args, loc| {
            let items = as_vector("map", &args[0], loc)?;
            let mut out = Vec::with_capacity(items.len());
            for item in items.iter() {
                out.push(apply(caller, &args[1], vec![Arg::positional(item.clone())], loc)?);
            }
            Ok(Value::vector(out))
        }),
        "std.core/filter" => native("filter", 2, |caller, args, loc| {
            let items = as_vector("filter", &args[0], loc)?;
            let mut out = Vec::new();
            for item in items.iter() {
                if apply(caller, &args[1], vec![Arg::positional(item.clone())], loc)?.is_truthy() {
                    out.push(item.clone());
                }
            }
            Ok(Value::vector(out))
        }),
        "std.core/reduce" => native("reduce", 3, |caller, args, loc| {
            let items = as_vector("reduce", &args[0], loc)?;
            let mut accumulator = args[1].clone();
            for item in items.iter() {
                accumulator = apply(
                    caller,
                    &args[2],
                    vec![Arg::positional(accumulator), Arg::positional(item.clone())],
                    loc,
                )?;
            }
            Ok(accumulator)
        }),
        "std.core/each" => native("each", 2, |caller, args, loc| {
            let items = as_vector("each", &args[0], loc)?;
            for item in items.iter() {
                apply(caller, &args[1], vec![Arg::positional(item.clone())], loc)?;
            }
            Ok(Value::Nil)
        }),
        "std.core/any?" => native("any?", 2, |caller, args, loc| {
            let items = as_vector("any?", &args[0], loc)?;
            for item in items.iter() {
                if apply(caller, &args[1], vec![Arg::positional(item.clone())], loc)?.is_truthy() {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        }),
        "std.core/all?" => native("all?", 2, |caller, args, loc| {
            let items = as_vector("all?", &args[0], loc)?;
            for item in items.iter() {
                if !apply(caller, &args[1], vec![Arg::positional(item.clone())], loc)?.is_truthy() {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        }),
        "std.core/sort" => native("sort", 1, |_, args, loc| {
            let mut items = (*as_vector("sort", &args[0], loc)?).clone();
            items.sort_by(|left, right| left.cmp_value(right).unwrap_or(std::cmp::Ordering::Equal));
            Ok(Value::vector(items))
        }),
        "std.core/sort-by" => native("sort-by", 2, |caller, args, loc| {
            let items = as_vector("sort-by", &args[0], loc)?;
            // Keys are computed once so the comparator cannot re-enter the host.
            let mut keyed = Vec::with_capacity(items.len());
            for item in items.iter() {
                let key = apply(caller, &args[1], vec![Arg::positional(item.clone())], loc)?;
                keyed.push((key, item.clone()));
            }
            keyed.sort_by(|left, right| {
                left.0.cmp_value(&right.0).unwrap_or(std::cmp::Ordering::Equal)
            });
            Ok(Value::vector(keyed.into_iter().map(|(_, item)| item).collect()))
        }),

        "std.core/Some" => native("Some", 1, |_, args, _| Ok(Value::some(args[0].clone()))),
        "std.core/Ok" => native("Ok", 1, |_, args, _| Ok(Value::ok(args[0].clone()))),
        "std.core/Err" => native("Err", 1, |_, args, _| Ok(Value::err(args[0].clone()))),
        "std.core/None" => Value::none(),
        "std.core/some?" => native("some?", 1, |_, args, _| {
            Ok(Value::Bool(
                matches!(&args[0], Value::Variant(variant) if &*variant.variant == "Some"),
            ))
        }),
        "std.core/ok?" => native("ok?", 1, |_, args, _| {
            Ok(Value::Bool(
                matches!(&args[0], Value::Variant(variant) if &*variant.variant == "Ok"),
            ))
        }),
        "std.core/nil?" => {
            native("nil?", 1, |_, args, _| Ok(Value::Bool(matches!(&args[0], Value::Nil))))
        }
        "std.core/unwrap-or" => native("unwrap-or", 2, |_, args, _| match &args[0] {
            Value::Variant(variant) if matches!(&*variant.variant, "Some" | "Ok") => {
                Ok(variant.fields.first().map(|(_, value)| value.clone()).unwrap_or(Value::Nil))
            }
            _ => Ok(args[1].clone()),
        }),

        "std.core/str" => native("str", 0, |_, args, _| {
            let mut text = String::new();
            for value in &args {
                text.push_str(&display(value));
            }
            Ok(Value::str(text))
        }),
        "std.core/type-of" => {
            native("type-of", 1, |_, args, _| Ok(Value::str(args[0].type_name())))
        }
        // Collections are persistent, so a clone is a new handle on the same
        // immutable structure. Cloning a resource is rejected at compile time.
        "std.core/clone" => native("clone", 1, |_, args, _| Ok(args[0].clone())),
        "std.core/keyword" => native("keyword", 1, |_, args, loc| {
            Ok(Value::keyword(&as_string("keyword", &args[0], loc)?))
        }),
        "std.core/identity" => native("identity", 1, |_, args, _| Ok(args[0].clone())),

        "std.core/println" | "std.io/println" => native("println", 0, |caller, args, _| {
            let text: Vec<String> = args.iter().map(display).collect();
            caller.write(&text.join(" "));
            caller.write("\n");
            Ok(Value::Nil)
        }),
        "std.core/print" | "std.io/print" => native("print", 0, |caller, args, _| {
            let text: Vec<String> = args.iter().map(display).collect();
            caller.write(&text.join(" "));
            Ok(Value::Nil)
        }),
        "std.core/eprintln" | "std.io/eprintln" => native("eprintln", 0, |_, args, _| {
            let text: Vec<String> = args.iter().map(display).collect();
            eprintln!("{}", text.join(" "));
            Ok(Value::Nil)
        }),
        "std.io/read-line" => native("read-line", 0, |_, _args, _| {
            let mut line = String::new();
            match std::io::stdin().read_line(&mut line) {
                Ok(0) => Ok(Value::none()),
                Ok(_) => {
                    Ok(Value::some(Value::str(line.trim_end_matches(['\n', '\r']).to_string())))
                }
                Err(error) => Ok(Value::err(Value::str(error.to_string()))),
            }
        }),

        "std.core/assert" | "std.test/assert" => native("assert", 1, |_, args, loc| {
            if args[0].is_truthy() {
                return Ok(Value::Nil);
            }
            let message =
                args.get(1).map(display).unwrap_or_else(|| "assertion failed".to_string());
            Err(Flow::fault(Fault::new("assert", message, loc).label("this assertion is false")))
        }),
        "std.core/assert-eq" | "std.test/assert-eq" => native("assert-eq", 2, |_, args, loc| {
            if args[0].eq_value(&args[1]) {
                return Ok(Value::Nil);
            }
            Err(Flow::fault(
                Fault::new("assert-eq", "values are not equal", loc)
                    .label("assertion failed here")
                    .note(format!("expected: {}", args[0]))
                    .note(format!("  actual: {}", args[1])),
            ))
        }),
        "std.core/assert-ne" | "std.test/assert-ne" => native("assert-ne", 2, |_, args, loc| {
            if !args[0].eq_value(&args[1]) {
                return Ok(Value::Nil);
            }
            Err(Flow::fault(
                Fault::new("assert-ne", "values are equal but should not be", loc)
                    .label("assertion failed here")
                    .note(format!("both sides: {}", args[0])),
            ))
        }),

        "std.core/uuid/parse" => native("#uuid", 1, |_, args, loc| {
            let text = as_string("#uuid", &args[0], loc)?;
            let hex: String = text.chars().filter(|ch| *ch != '-').collect();
            if hex.len() != 32 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Err(Flow::fault(
                    Fault::new("uuid-literal", "malformed UUID literal", loc)
                        .label(format!("`{text}` is not a UUID")),
                ));
            }
            Ok(Value::record(Some("Uuid"), vec![("text", Value::str(text))]))
        }),
        "std.core/date/parse" => native("#date", 1, |_, args, loc| {
            let text = as_string("#date", &args[0], loc)?;
            Ok(Value::record(Some("Date"), vec![("text", Value::str(text))]))
        }),
        "std.core/duration/parse" => native("#duration", 1, |_, args, loc| {
            let text = as_string("#duration", &args[0], loc)?;
            let millis = parse_duration(&text).ok_or_else(|| {
                Flow::fault(
                    Fault::new("duration-literal", "malformed duration literal", loc)
                        .label(format!("`{text}` is not a duration"))
                        .help("write durations like `250ms`, `3s`, or `5m`"),
                )
            })?;
            Ok(Value::record(Some("Duration"), vec![("millis", Value::Int(millis))]))
        }),

        "std.string/split" => native("split", 2, |_, args, loc| {
            let text = as_string("string.split", &args[0], loc)?;
            let separator = as_string("string.split", &args[1], loc)?;
            let parts = if separator.is_empty() {
                text.chars().map(|ch| Value::str(ch.to_string())).collect()
            } else {
                text.split(separator.as_str()).map(Value::str).collect()
            };
            Ok(Value::vector(parts))
        }),
        "std.string/join" => native("join", 2, |_, args, loc| {
            let items = as_vector("string.join", &args[0], loc)?;
            let separator = as_string("string.join", &args[1], loc)?;
            let parts: Vec<String> = items.iter().map(display).collect();
            Ok(Value::str(parts.join(&separator)))
        }),
        "std.string/trim" => native("trim", 1, |_, args, loc| {
            Ok(Value::str(as_string("string.trim", &args[0], loc)?.trim().to_string()))
        }),
        "std.string/upper" => native("upper", 1, |_, args, loc| {
            Ok(Value::str(as_string("string.upper", &args[0], loc)?.to_uppercase()))
        }),
        "std.string/lower" => native("lower", 1, |_, args, loc| {
            Ok(Value::str(as_string("string.lower", &args[0], loc)?.to_lowercase()))
        }),
        "std.string/starts-with?" => native("starts-with?", 2, |_, args, loc| {
            let text = as_string("string.starts-with?", &args[0], loc)?;
            let prefix = as_string("string.starts-with?", &args[1], loc)?;
            Ok(Value::Bool(text.starts_with(&prefix)))
        }),
        "std.string/ends-with?" => native("ends-with?", 2, |_, args, loc| {
            let text = as_string("string.ends-with?", &args[0], loc)?;
            let suffix = as_string("string.ends-with?", &args[1], loc)?;
            Ok(Value::Bool(text.ends_with(&suffix)))
        }),
        "std.string/replace" => native("replace", 3, |_, args, loc| {
            let text = as_string("string.replace", &args[0], loc)?;
            let from = as_string("string.replace", &args[1], loc)?;
            let to = as_string("string.replace", &args[2], loc)?;
            Ok(Value::str(text.replace(&from, &to)))
        }),
        "std.string/chars" => native("chars", 1, |_, args, loc| {
            let text = as_string("string.chars", &args[0], loc)?;
            Ok(Value::vector(text.chars().map(|ch| Value::str(ch.to_string())).collect()))
        }),
        "std.string/parse-int" => native("parse-int", 1, |_, args, loc| {
            let text = as_string("string.parse-int", &args[0], loc)?;
            Ok(match text.trim().parse::<i64>() {
                Ok(value) => Value::ok(Value::Int(value)),
                Err(error) => {
                    Value::err(Value::str(format!("`{text}` is not an integer: {error}")))
                }
            })
        }),
        "std.string/parse-float" => native("parse-float", 1, |_, args, loc| {
            let text = as_string("string.parse-float", &args[0], loc)?;
            Ok(match text.trim().parse::<f64>() {
                Ok(value) => Value::ok(Value::Float(value)),
                Err(error) => Value::err(Value::str(format!("`{text}` is not a number: {error}"))),
            })
        }),

        // Splitting once keeps the remainder intact, which matters when the
        // separator can also appear inside the part being split off.
        "std.string/split-once" => native("split-once", 2, |_, args, loc| {
            let text = as_string("string.split-once", &args[0], loc)?;
            let separator = as_string("string.split-once", &args[1], loc)?;
            Ok(match text.split_once(separator.as_str()) {
                Some((head, tail)) => Value::some(Value::vector(vec![
                    Value::str(head.to_string()),
                    Value::str(tail.to_string()),
                ])),
                None => Value::none(),
            })
        }),
        // Byte length, which is what a `content-length` header counts.
        "std.string/byte-length" => native("byte-length", 1, |_, args, loc| {
            Ok(Value::Int(as_string("string.byte-length", &args[0], loc)?.len() as i64))
        }),
        "std.string/repeat" => native("repeat", 2, |_, args, loc| {
            let text = as_string("string.repeat", &args[0], loc)?;
            let count = as_int("string.repeat", &args[1], loc)?.max(0) as usize;
            Ok(Value::str(text.repeat(count)))
        }),

        "std.math/abs" => native("abs", 1, |_, args, loc| match &args[0] {
            Value::Int(value) => Ok(Value::Int(value.abs())),
            Value::Float(value) => Ok(Value::Float(value.abs())),
            other => Err(wrong_type("math.abs", "a number", other, loc)),
        }),
        "std.math/sqrt" => native("sqrt", 1, |_, args, loc| {
            let (value, _, _) = numeric("math.sqrt", &args[0], &Value::Int(0), loc)?;
            Ok(Value::Float(value.sqrt()))
        }),
        "std.math/pow" => native("pow", 2, |_, args, loc| {
            let (base, exponent, is_float) = numeric("math.pow", &args[0], &args[1], loc)?;
            Ok(if is_float {
                Value::Float(base.powf(exponent))
            } else {
                Value::Int(base.powf(exponent) as i64)
            })
        }),
        "std.math/floor" => native("floor", 1, |_, args, loc| {
            let (value, _, _) = numeric("math.floor", &args[0], &Value::Int(0), loc)?;
            Ok(Value::Int(value.floor() as i64))
        }),
        "std.math/ceil" => native("ceil", 1, |_, args, loc| {
            let (value, _, _) = numeric("math.ceil", &args[0], &Value::Int(0), loc)?;
            Ok(Value::Int(value.ceil() as i64))
        }),

        "std.fs/read-text" => native("read-text", 1, |_, args, loc| {
            let path = as_string("fs.read-text", &args[0], loc)?;
            Ok(match std::fs::read_to_string(&path) {
                Ok(text) => Value::ok(Value::str(text)),
                Err(error) => Value::err(io_error(&path, &error)),
            })
        }),
        "std.fs/write-text" => native("write-text", 2, |_, args, loc| {
            let path = as_string("fs.write-text", &args[0], loc)?;
            let text = as_string("fs.write-text", &args[1], loc)?;
            Ok(match std::fs::write(&path, text) {
                Ok(()) => Value::ok(Value::Nil),
                Err(error) => Value::err(io_error(&path, &error)),
            })
        }),
        // korben-0mo
        // Renaming within a filesystem is atomic, which is what makes
        // write-then-rename safe: a reader sees either the old file or the
        // new one, never a half-written one. Without it a korben program has
        // no way to replace a file without a window where it is truncated.
        "std.fs/rename" => native("rename", 2, |_, args, loc| {
            let from = as_string("fs.rename", &args[0], loc)?;
            let to = as_string("fs.rename", &args[1], loc)?;
            Ok(match std::fs::rename(&from, &to) {
                Ok(()) => Value::ok(Value::Nil),
                // The failure names the destination, which is the path the
                // caller was trying to produce.
                Err(error) => Value::err(io_error(&to, &error)),
            })
        }),
        "std.fs/exists?" => native("exists?", 1, |_, args, loc| {
            Ok(Value::Bool(std::path::Path::new(&as_string("fs.exists?", &args[0], loc)?).exists()))
        }),
        "std.fs/read-lines" => native("read-lines", 1, |_, args, loc| {
            let path = as_string("fs.read-lines", &args[0], loc)?;
            Ok(match std::fs::read_to_string(&path) {
                Ok(text) => Value::ok(Value::vector(text.lines().map(Value::str).collect())),
                Err(error) => Value::err(io_error(&path, &error)),
            })
        }),
        "std.fs/list-dir" => native("list-dir", 1, |_, args, loc| {
            let path = as_string("fs.list-dir", &args[0], loc)?;
            Ok(match std::fs::read_dir(&path) {
                Ok(entries) => {
                    let mut names: Vec<Value> = entries
                        .filter_map(|entry| entry.ok())
                        .map(|entry| Value::str(entry.file_name().to_string_lossy().to_string()))
                        .collect();
                    names.sort_by(|left, right| {
                        left.cmp_value(right).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    Value::ok(Value::vector(names))
                }
                Err(error) => Value::err(io_error(&path, &error)),
            })
        }),

        // A file handle is a resource: it owns an operating-system handle and
        // must be closed exactly once, which is what `with` and `Drop` are for.
        "std.fs/open" => native("open", 1, |_, args, loc| {
            let path = as_string("fs.open", &args[0], loc)?;
            Ok(match std::fs::File::open(&path) {
                Ok(file) => Value::ok(file_value(file)),
                Err(error) => Value::err(io_error(&path, &error)),
            })
        }),
        "std.fs/create" => native("create", 1, |_, args, loc| {
            let path = as_string("fs.create", &args[0], loc)?;
            Ok(match std::fs::File::create(&path) {
                Ok(file) => Value::ok(file_value(file)),
                Err(error) => Value::err(io_error(&path, &error)),
            })
        }),
        "File/write" => native("write", 2, |_, args, loc| {
            let text = as_string("File.write", &args[1], loc)?;
            with_file("File.write", &args[0], loc, |file| {
                use std::io::Write;
                file.write_all(text.as_bytes())
            })
        }),
        "File/read-text" => native("read-text", 1, |_, args, loc| {
            let mut text = String::new();
            let outcome = with_file("File.read-text", &args[0], loc, |file| {
                use std::io::Read;
                file.read_to_string(&mut text).map(|_| ())
            })?;
            Ok(match outcome {
                Value::Variant(variant) if &*variant.variant == "Ok" => Value::ok(Value::str(text)),
                other => other,
            })
        }),
        "File/close" | "File/drop" => native("close", 1, |_, args, loc| {
            let Some(handle) = as_file(&args[0]) else {
                return Err(wrong_type("File.close", "a File", &args[0], loc));
            };
            // Closing twice is not an error; `with` closes on every exit path.
            *handle.borrow_mut() = None;
            Ok(Value::Nil)
        }),
        "File/closed?" => native("closed?", 1, |_, args, loc| {
            let Some(handle) = as_file(&args[0]) else {
                return Err(wrong_type("File.closed?", "a File", &args[0], loc));
            };
            let closed = handle.borrow().is_none();
            Ok(Value::Bool(closed))
        }),

        "std.net/listen" => native("listen", 1, |_, args, loc| {
            crate::net::listen(&as_string("net.listen", &args[0], loc)?)
        }),
        // korben-ggd
        "std.net/connect-tls" => native("connect-tls", 1, |_, args, loc| {
            let address = as_string("net.connect-tls", &args[0], loc)?;
            crate::net::connect_tls(&address)
        }),
        "std.net/connect" => native("connect", 1, |_, args, loc| {
            crate::net::connect(&as_string("net.connect", &args[0], loc)?)
        }),
        // korben-ae2
        "std.net/pool" => native("pool", 1, |_, args, loc| {
            let address = as_string("net.pool", &args[0], loc)?;
            crate::net::pool(&address)
        }),
        "Pool/wait" => native("wait", 2, |_, args, loc| {
            let timeout = as_int("Pool.wait", &args[1], loc)?;
            crate::net::pool_wait(&args[0], timeout, loc)
        }),
        "Pool/read" => native("read", 2, |_, args, loc| {
            let id = as_int("Pool.read", &args[1], loc)?;
            crate::net::pool_read(&args[0], id, loc)
        }),
        "Pool/write" => native("write", 3, |_, args, loc| {
            let id = as_int("Pool.write", &args[1], loc)?;
            let text = as_string("Pool.write", &args[2], loc)?;
            crate::net::pool_write(&args[0], id, &text, loc)
        }),
        "Pool/close-connection" => native("close-connection", 2, |_, args, loc| {
            let id = as_int("Pool.close-connection", &args[1], loc)?;
            crate::net::pool_drop(&args[0], id, loc)
        }),
        // korben-c6k
        "Pool/evict" => native("evict", 2, |_, args, loc| {
            let idle = as_int("Pool.evict", &args[1], loc)?;
            crate::net::pool_evict(&args[0], idle, loc)
        }),
        "Pool/address" => {
            native("address", 1, |_, args, loc| crate::net::pool_address(&args[0], loc))
        }
        "Pool/close" | "Pool/drop" => {
            native("close", 1, |_, args, loc| crate::net::pool_close(&args[0], loc))
        }
        // korben-48e
        "Listener/accept" => {
            native("accept", 1, |caller, args, loc| crate::net::accept(caller, &args[0], loc))
        }
        "Listener/address" => {
            native("address", 1, |_, args, loc| crate::net::local_address(&args[0], loc))
        }
        "Connection/read" => {
            native("read", 1, |caller, args, loc| crate::net::read(caller, &args[0], loc))
        }
        "Connection/write" => native("write", 2, |caller, args, loc| {
            let text = as_string("Connection.write", &args[1], loc)?;
            crate::net::write(caller, &args[0], &text, loc)
        }),
        "Connection/peer" => {
            native("peer", 1, |_, args, loc| crate::net::peer_address(&args[0], loc))
        }
        "Listener/close" | "Listener/drop" | "Connection/close" | "Connection/drop" => {
            native("close", 1, |_, args, _| crate::net::close(&args[0]))
        }
        "Listener/closed?" | "Connection/closed?" => {
            native("closed?", 1, |_, args, _| crate::net::is_closed(&args[0]))
        }

        "std.json/encode" => {
            native("encode", 1, |_, args, _| Ok(Value::str(crate::json::encode(&args[0], false))))
        }
        "std.json/encode-pretty" => native("encode-pretty", 1, |_, args, _| {
            Ok(Value::str(crate::json::encode(&args[0], true)))
        }),
        "std.json/decode" => native("decode", 1, |_, args, loc| {
            let text = as_string("json.decode", &args[0], loc)?;
            Ok(match crate::json::decode(&text) {
                Ok(value) => Value::ok(value),
                Err(message) => Value::err(Value::str(message)),
            })
        }),

        // korben-2fo
        "std.log/debug" => native("debug", 1, |caller, args, _| log_at(caller, DEBUG, &args)),
        "std.log/info" => native("info", 1, |caller, args, _| log_at(caller, INFO, &args)),
        "std.log/warn" => native("warn", 1, |caller, args, _| log_at(caller, WARN, &args)),
        "std.log/error" => native("error", 1, |caller, args, _| log_at(caller, ERROR, &args)),

        "std.time/now-millis" => native("now-millis", 0, |_, _args, _| {
            let millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            Ok(Value::Int(millis))
        }),
        "std.time/sleep-millis" => native("sleep-millis", 1, |_, args, loc| {
            let millis = as_int("time.sleep-millis", &args[0], loc)?;
            std::thread::sleep(std::time::Duration::from_millis(millis.max(0) as u64));
            Ok(Value::Nil)
        }),

        // korben-5k7
        "std.process/shutdown-requested?" => native("shutdown-requested?", 0, |_, _args, _| {
            Ok(Value::Bool(crate::signal::requested()))
        }),
        "std.process/args" => native("args", 0, |_, _args, _| {
            Ok(Value::vector(program_args().into_iter().map(Value::str).collect()))
        }),
        "std.process/env" => native("env", 1, |_, args, loc| {
            let name = as_string("process.env", &args[0], loc)?;
            Ok(match std::env::var(&name) {
                Ok(value) => Value::some(Value::str(value)),
                Err(_) => Value::none(),
            })
        }),
        "std.process/exit" => native("exit", 1, |_, args, loc| {
            let code = as_int("process.exit", &args[0], loc)?;
            std::process::exit(code as i32)
        }),

        "std.async/join-all" => {
            native("join-all", 1, |caller, args, loc| crate::task::join_all(caller, &args[0], loc))
        }
        "std.async/join" => {
            native("join", 1, |caller, args, loc| crate::task::await_value(caller, &args[0], loc))
        }
        // An unbounded channel never makes a sender wait.
        "std.async/channel" => native("channel", 0, |_, _args, _| Ok(crate::task::channel(None))),
        "std.async/bounded" => native("bounded", 1, |_, args, loc| {
            let capacity = as_int("channel.bounded", &args[0], loc)?.max(0) as usize;
            Ok(crate::task::channel(Some(capacity)))
        }),
        "Scope/cancel" => native("cancel", 1, |_, args, _| crate::task::cancel_scope(&args[0])),
        "Scope/cancelled?" => {
            native("cancelled?", 1, |_, args, _| crate::task::scope_cancelled(&args[0]))
        }
        "Task/cancel" => native("cancel", 1, |_, args, _| crate::task::cancel_task(&args[0])),
        "Task/state" => native("state", 1, |_, args, _| crate::task::task_state_name(&args[0])),
        "Sender/send" => native("send", 2, |caller, args, loc| {
            crate::task::send(caller, &args[0], args[1].clone(), loc)
        }),
        "Receiver/recv" => {
            native("recv", 1, |caller, args, loc| crate::task::recv(caller, &args[0], loc))
        }
        "Receiver/try-recv" => {
            native("try-recv", 1, |_, args, loc| crate::task::try_recv(&args[0], loc))
        }
        "Sender/close" | "Receiver/close" => {
            native("close", 1, |_, args, _| crate::task::close_channel(&args[0]))
        }
        "Sender/len" | "Receiver/len" => {
            native("len", 1, |_, args, _| crate::task::channel_len(&args[0]))
        }

        "Cell/new" => {
            native("new", 1, |_, args, _| Ok(Value::Cell(Rc::new(RefCell::new(args[0].clone())))))
        }
        "Cell/get" => native("get", 1, |_, args, loc| match &args[0] {
            Value::Cell(cell) => Ok(cell.borrow().clone()),
            other => Err(wrong_type("Cell.get", "a Cell", other, loc)),
        }),
        "Cell/set" => native("set", 2, |_, args, loc| match &args[0] {
            Value::Cell(cell) => {
                *cell.borrow_mut() = args[1].clone();
                Ok(Value::Nil)
            }
            other => Err(wrong_type("Cell.set", "a Cell", other, loc)),
        }),
        "Cell/update" => native("update", 2, |caller, args, loc| match &args[0] {
            Value::Cell(cell) => {
                let current = cell.borrow().clone();
                let next = apply(caller, &args[1], vec![Arg::positional(current)], loc)?;
                *cell.borrow_mut() = next.clone();
                Ok(next)
            }
            other => Err(wrong_type("Cell.update", "a Cell", other, loc)),
        }),

        _ => return None,
    };
    Some(value)
}

/// Built-in methods reachable as `(receiver.method ...)`.
pub fn method_of(type_name: &str, method: &str) -> Option<Value> {
    let module = match type_name {
        "String" => "std.string",
        "Cell" => "Cell",
        "File" => "File",
        "Listener" => "Listener",
        "Connection" => "Connection",
        // korben-ae2
        "Pool" => "Pool",
        "Scope" => "Scope",
        "Task" => "Task",
        "Sender" => "Sender",
        "Receiver" => "Receiver",
        _ => return None,
    };
    builtin(&format!("{module}/{method}"))
}

/// Native types that own an external resource and must be released.
// korben-2fo
// Logging with levels, timestamps and stream routing.
//
// The four log functions used to share one implementation that printed only
// the message and its fields, so `error` was byte-identical to `debug`, no
// line carried a time, and everything went to stdout. Logs could not be
// filtered by severity, correlated in time, or separated from ordinary output
// by a supervisor or log shipper.
//
// Levels are ordered so a threshold can suppress the ones below it.
const DEBUG: u8 = 0;
const INFO: u8 = 1;
const WARN: u8 = 2;
const ERROR: u8 = 3;

fn level_name(level: u8) -> &'static str {
    match level {
        DEBUG => "DEBUG",
        INFO => "INFO",
        WARN => "WARN",
        _ => "ERROR",
    }
}

/// The lowest level that is printed, from `KORBEN_LOG`.
///
/// Defaults to `info`, so `debug` is off unless asked for — a program should
/// not have to be rebuilt to quieten its debug logging. An unrecognised value
/// is treated as the default rather than failing: a typo in an environment
/// variable should not stop a service from starting.
fn threshold() -> u8 {
    match std::env::var("KORBEN_LOG").ok().as_deref().map(str::trim) {
        Some("debug") => DEBUG,
        Some("warn") => WARN,
        Some("error") => ERROR,
        _ => INFO,
    }
}

/// An RFC 3339 timestamp in UTC, to the second.
///
/// Written out here because `korben-runtime` deliberately has no dependencies.
/// The days-to-civil conversion is the usual era-based algorithm.
fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let time = secs % 86_400;
    let (hour, minute, second) = (time / 3_600, (time % 3_600) / 60, time % 60);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// Emit one log line, if the level clears the threshold.
///
/// `warn` and `error` go to stderr and the quieter levels to the captured
/// output stream, matching how `std.io/eprintln` and `std.io/println` already
/// divide. That is what lets a supervisor separate failure output from
/// ordinary output.
fn log_at(caller: &mut dyn Caller, level: u8, args: &[Value]) -> Outcome {
    if level < threshold() {
        return Ok(Value::Nil);
    }
    let message = args.first().map(display).unwrap_or_default();
    let fields = args.get(1).map(|value| format!(" {value}")).unwrap_or_default();
    let line = format!("{} {:<5} {message}{fields}", rfc3339(now_secs()), level_name(level));
    if level >= WARN {
        eprintln!("{line}");
    } else {
        caller.write(&format!("{line}\n"));
    }
    Ok(Value::Nil)
}

pub const RESOURCE_TYPES: &[&str] = &["File", "Listener", "Connection", "Pool"];

type FileHandle = RefCell<Option<std::fs::File>>;

fn file_value(file: std::fs::File) -> Value {
    crate::value::Foreign::wrap("File", RefCell::new(Some(file)))
}

fn as_file(value: &Value) -> Option<&FileHandle> {
    let Value::Foreign(foreign) = value else { return None };
    foreign.downcast::<FileHandle>()
}

/// Run an operation on an open file, reporting use-after-close as an `Err`.
fn with_file(
    name: &str,
    value: &Value,
    loc: Loc,
    work: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Outcome {
    let Some(handle) = as_file(value) else {
        return Err(wrong_type(name, "a File", value, loc));
    };
    let mut borrowed = handle.borrow_mut();
    let Some(file) = borrowed.as_mut() else {
        return Ok(Value::err(Value::record(
            Some("IoError"),
            vec![
                ("path", Value::str("")),
                ("kind", Value::keyword("closed")),
                ("message", Value::str(format!("{name} was called on a closed file"))),
            ],
        )));
    };
    Ok(match work(file) {
        Ok(()) => Value::ok(Value::Nil),
        Err(error) => Value::err(io_error("", &error)),
    })
}

thread_local! {
    static PROGRAM_ARGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Set the arguments `std.process/args` reports.
pub fn set_program_args(args: Vec<String>) {
    PROGRAM_ARGS.with(|slot| *slot.borrow_mut() = args);
}

pub fn program_args() -> Vec<String> {
    PROGRAM_ARGS.with(|slot| slot.borrow().clone())
}

fn parse_duration(text: &str) -> Option<i64> {
    let digits: String = text.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    let unit = &text[digits.len()..];
    let amount: i64 = digits.parse().ok()?;
    let multiplier = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return None,
    };
    Some(amount * multiplier)
}

/// A map literal built from alternating keys and values.
pub fn map_from(entries: Vec<(Value, Value)>) -> Value {
    let mut map = MapValue::default();
    for (key, value) in entries {
        map.insert(key, value);
    }
    Value::Map(Rc::new(map))
}

// korben-2fo
#[cfg(test)]
mod log_tests {
    use super::{level_name, rfc3339, DEBUG, ERROR, INFO, WARN};

    /// The timestamp arithmetic is written out by hand, because the runtime
    /// has no dependencies. These are the cases that catch a wrong one: the
    /// epoch itself, a leap day, a century that is not a leap year, and the
    /// last second of a year.
    #[test]
    fn formats_known_instants() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_234_567_890), "2009-02-13T23:31:30Z");
        assert_eq!(rfc3339(1_583_020_800), "2020-03-01T00:00:00Z");
        assert_eq!(rfc3339(4_102_444_800), "2100-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_767_225_599), "2025-12-31T23:59:59Z");
    }

    /// The ordering of the level constants is what makes a threshold able to
    /// suppress everything below it, so the names must line up with it.
    #[test]
    fn level_names_follow_the_ordering() {
        let mut named: Vec<(u8, &str)> =
            vec![(ERROR, "ERROR"), (DEBUG, "DEBUG"), (WARN, "WARN"), (INFO, "INFO")];
        named.sort_by_key(|(level, _)| *level);
        assert_eq!(
            named.iter().map(|(_, name)| *name).collect::<Vec<_>>(),
            ["DEBUG", "INFO", "WARN", "ERROR"]
        );
        for (level, name) in named {
            assert_eq!(level_name(level), name);
        }
    }
}
