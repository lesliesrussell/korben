//! The standard library, implemented natively.
//!
//! Modules mirror specification section 16.1. Every module is a real runtime
//! module, so `(use std.string :as string)` and `string.split` resolve through
//! the ordinary import machinery.

// korben-6bc

use crate::eval::Interp;
use crate::value::*;
use korben_syntax::diag::Diagnostic;
use korben_syntax::span::Span;
use std::cell::RefCell;
use std::rc::Rc;

/// Modules whose exports are also injected into every module's scope.
pub const PRELUDE: &str = "std.core";

macro_rules! native {
    ($interp:expr, $module:expr, $name:literal, $arity:expr, $func:expr) => {{
        let value = Value::Native(Rc::new(NativeFn { name: $name, arity: $arity, func: $func }));
        $module.exports.borrow_mut().insert($name.to_string(), value);
        let _ = &$interp;
    }};
}

pub fn install(interp: &mut Interp) {
    install_core(interp);
    install_string(interp);
    install_math(interp);
    install_io(interp);
    install_fs(interp);
    install_json(interp);
    install_log(interp);
    install_time(interp);
    install_process(interp);
    install_test(interp);
    install_types(interp);
    install_syntax(interp);
}

fn wrong_type(name: &str, expected: &str, got: &Value, span: Span) -> Flow {
    Flow::panic(
        Diagnostic::error(format!("`{name}` expected {expected}"))
            .with_code("type-error")
            .at(span, format!("found {} (`{got}`)", got.type_name())),
    )
}

fn as_int(name: &str, value: &Value, span: Span) -> Result<i64, Flow> {
    match value {
        Value::Int(value) => Ok(*value),
        other => Err(wrong_type(name, "an Int", other, span)),
    }
}

fn as_string(name: &str, value: &Value, span: Span) -> Result<String, Flow> {
    match value {
        Value::Str(text) => Ok((**text).clone()),
        other => Err(wrong_type(name, "a String", other, span)),
    }
}

fn as_vector(name: &str, value: &Value, span: Span) -> Result<Rc<Vec<Value>>, Flow> {
    match value {
        Value::Vector(items) => Ok(items.clone()),
        other => Err(wrong_type(name, "a Vec", other, span)),
    }
}

/// Numeric tower: integers stay integers unless a float is involved.
fn numeric_pair(
    name: &str,
    left: &Value,
    right: &Value,
    span: Span,
) -> Result<(f64, f64, bool), Flow> {
    let (left_value, left_float) = match left {
        Value::Int(value) => (*value as f64, false),
        Value::Float(value) => (*value, true),
        other => return Err(wrong_type(name, "a number", other, span)),
    };
    let (right_value, right_float) = match right {
        Value::Int(value) => (*value as f64, false),
        Value::Float(value) => (*value, true),
        other => return Err(wrong_type(name, "a number", other, span)),
    };
    Ok((left_value, right_value, left_float || right_float))
}

fn arith(
    name: &'static str,
    args: &[Value],
    span: Span,
    identity: i64,
    op: fn(f64, f64) -> f64,
    int_op: fn(i64, i64) -> Option<i64>,
) -> EvalResult {
    if args.is_empty() {
        return Ok(Value::Int(identity));
    }
    if args.len() == 1 {
        // Unary `-` and `/` negate and invert respectively.
        let identity_value = Value::Int(identity);
        return arith(name, &[identity_value, args[0].clone()], span, identity, op, int_op);
    }
    let mut accumulator = args[0].clone();
    for next in &args[1..] {
        let (left, right, is_float) = numeric_pair(name, &accumulator, next, span)?;
        accumulator = if is_float {
            Value::Float(op(left, right))
        } else {
            match int_op(left as i64, right as i64) {
                Some(value) => Value::Int(value),
                None => {
                    return Err(Flow::panic(
                        Diagnostic::error(format!("integer overflow in `{name}`"))
                            .with_code("overflow")
                            .at(span, format!("{left} {name} {right} does not fit in Int")),
                    ))
                }
            }
        };
    }
    Ok(accumulator)
}

fn install_core(interp: &mut Interp) {
    let module = interp.module(PRELUDE);

    native!(interp, module, "+", None, |_, args, span| arith(
        "+",
        &args,
        span,
        0,
        |a, b| a + b,
        i64::checked_add
    ));
    native!(interp, module, "-", None, |_, args, span| arith(
        "-",
        &args,
        span,
        0,
        |a, b| a - b,
        i64::checked_sub
    ));
    native!(interp, module, "*", None, |_, args, span| arith(
        "*",
        &args,
        span,
        1,
        |a, b| a * b,
        i64::checked_mul
    ));
    native!(interp, module, "/", None, |_, args, span| {
        // Division by zero is a fault, not a silent NaN.
        for value in args.iter().skip(1) {
            if value.eq_value(&Value::Int(0)) {
                return Err(Flow::panic(
                    Diagnostic::error("division by zero")
                        .with_code("divide-by-zero")
                        .at(span, "the divisor is zero"),
                ));
            }
        }
        arith("/", &args, span, 1, |a, b| a / b, |a, b| a.checked_div(b))
    });
    native!(interp, module, "mod", Some(2), |_, args, span| {
        let right = as_int("mod", &args[1], span)?;
        if right == 0 {
            return Err(Flow::panic(
                Diagnostic::error("modulo by zero")
                    .with_code("divide-by-zero")
                    .at(span, "the divisor is zero"),
            ));
        }
        Ok(Value::Int(as_int("mod", &args[0], span)?.rem_euclid(right)))
    });
    native!(interp, module, "inc", Some(1), |_, args, span| Ok(Value::Int(
        as_int("inc", &args[0], span)? + 1
    )));
    native!(interp, module, "dec", Some(1), |_, args, span| Ok(Value::Int(
        as_int("dec", &args[0], span)? - 1
    )));

    native!(interp, module, "=", Some(2), |_, args, _| {
        Ok(Value::Bool(args.windows(2).all(|pair| pair[0].eq_value(&pair[1]))))
    });
    native!(interp, module, "not=", Some(2), |_, args, _| {
        Ok(Value::Bool(!args.windows(2).all(|pair| pair[0].eq_value(&pair[1]))))
    });
    native!(interp, module, "not", Some(1), |_, args, _| Ok(Value::Bool(!args[0].is_truthy())));

    for (name, func) in
        [("<", compare_lt as NativeImpl), ("<=", compare_le), (">", compare_gt), (">=", compare_ge)]
    {
        let value = Value::Native(Rc::new(NativeFn { name: leak(name), arity: Some(2), func }));
        module.exports.borrow_mut().insert(name.to_string(), value);
    }

    native!(interp, module, "min", Some(2), |_, args, span| {
        let (left, right, is_float) = numeric_pair("min", &args[0], &args[1], span)?;
        Ok(if is_float {
            Value::Float(left.min(right))
        } else {
            Value::Int(left.min(right) as i64)
        })
    });
    native!(interp, module, "max", Some(2), |_, args, span| {
        let (left, right, is_float) = numeric_pair("max", &args[0], &args[1], span)?;
        Ok(if is_float {
            Value::Float(left.max(right))
        } else {
            Value::Int(left.max(right) as i64)
        })
    });

    // ------------------------------------------------------- collections
    native!(interp, module, "len", Some(1), |_, args, span| {
        let length = match &args[0] {
            Value::Vector(items) => items.len(),
            Value::Set(items) => items.len(),
            Value::Map(map) => map.len(),
            Value::Str(text) => text.chars().count(),
            Value::Record(record) => record.fields.len(),
            other => return Err(wrong_type("len", "a collection", other, span)),
        };
        Ok(Value::Int(length as i64))
    });
    native!(interp, module, "empty?", Some(1), |_, args, span| {
        let empty = match &args[0] {
            Value::Vector(items) => items.is_empty(),
            Value::Set(items) => items.is_empty(),
            Value::Map(map) => map.is_empty(),
            Value::Str(text) => text.is_empty(),
            Value::Nil => true,
            other => return Err(wrong_type("empty?", "a collection", other, span)),
        };
        Ok(Value::Bool(empty))
    });
    native!(interp, module, "first", Some(1), |_, args, span| {
        Ok(as_vector("first", &args[0], span)?
            .first()
            .cloned()
            .map(Value::some)
            .unwrap_or_else(Value::none))
    });
    native!(interp, module, "last", Some(1), |_, args, span| {
        Ok(as_vector("last", &args[0], span)?
            .last()
            .cloned()
            .map(Value::some)
            .unwrap_or_else(Value::none))
    });
    native!(interp, module, "rest", Some(1), |_, args, span| {
        let items = as_vector("rest", &args[0], span)?;
        Ok(Value::vector(items.iter().skip(1).cloned().collect()))
    });
    native!(interp, module, "nth", Some(2), |_, args, span| {
        let items = as_vector("nth", &args[0], span)?;
        let index = as_int("nth", &args[1], span)?;
        if index < 0 || index as usize >= items.len() {
            return Ok(Value::none());
        }
        Ok(Value::some(items[index as usize].clone()))
    });
    native!(interp, module, "conj", Some(2), |_, args, span| match &args[0] {
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
            Ok(Value::Set(Rc::new(next)))
        }
        other => Err(wrong_type("conj", "a Vec or Set", other, span)),
    });
    native!(interp, module, "concat", None, |_, args, span| {
        let mut out = Vec::new();
        for value in &args {
            out.extend(as_vector("concat", value, span)?.iter().cloned());
        }
        Ok(Value::vector(out))
    });
    native!(interp, module, "reverse", Some(1), |_, args, span| {
        let mut items = (*as_vector("reverse", &args[0], span)?).clone();
        items.reverse();
        Ok(Value::vector(items))
    });
    native!(interp, module, "contains?", Some(2), |_, args, span| {
        let found = match &args[0] {
            Value::Vector(items) | Value::Set(items) => {
                items.iter().any(|item| item.eq_value(&args[1]))
            }
            Value::Map(map) => map.get(&args[1]).is_some(),
            Value::Str(text) => match &args[1] {
                Value::Str(needle) => text.contains(needle.as_str()),
                other => return Err(wrong_type("contains?", "a String", other, span)),
            },
            other => return Err(wrong_type("contains?", "a collection", other, span)),
        };
        Ok(Value::Bool(found))
    });
    native!(interp, module, "get", Some(2), |interp, args, span| {
        let fallback = args.get(2).cloned();
        let found = match &args[0] {
            Value::Map(map) => map.get(&args[1]).cloned(),
            Value::Record(record) => match &args[1] {
                Value::Keyword(name) | Value::Symbol(name) => record.get(name).cloned(),
                Value::Str(name) => record.get(name).cloned(),
                _ => None,
            },
            Value::Variant(variant) => match &args[1] {
                Value::Keyword(name) | Value::Symbol(name) => variant.get(name).cloned(),
                _ => None,
            },
            Value::Vector(items) => match &args[1] {
                Value::Int(index) if *index >= 0 => items.get(*index as usize).cloned(),
                _ => None,
            },
            other => return Err(wrong_type("get", "a Map, Record, or Vec", other, span)),
        };
        let _ = &interp;
        Ok(found.or(fallback).unwrap_or(Value::Nil))
    });
    native!(interp, module, "assoc", Some(3), |_, args, span| match &args[0] {
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
                        other => return Err(wrong_type("assoc", "a field name", other, span)),
                    };
                    match next.fields.iter_mut().find(|(field, _)| *field == name) {
                        Some(slot) => slot.1 = value.clone(),
                        None => next.fields.push((name, value.clone())),
                    }
                }
            }
            Ok(Value::Record(Rc::new(next)))
        }
        other => Err(wrong_type("assoc", "a Map or Record", other, span)),
    });
    native!(interp, module, "dissoc", Some(2), |_, args, span| match &args[0] {
        Value::Map(map) => {
            let mut next = (**map).clone();
            for key in &args[1..] {
                next.remove(key);
            }
            Ok(Value::Map(Rc::new(next)))
        }
        other => Err(wrong_type("dissoc", "a Map", other, span)),
    });
    native!(interp, module, "keys", Some(1), |_, args, span| match &args[0] {
        Value::Map(map) =>
            Ok(Value::vector(map.entries.iter().map(|(key, _)| key.clone()).collect())),
        Value::Record(record) => Ok(Value::vector(
            record.fields.iter().map(|(name, _)| Value::Keyword(name.clone())).collect(),
        )),
        other => Err(wrong_type("keys", "a Map or Record", other, span)),
    });
    native!(interp, module, "values", Some(1), |_, args, span| match &args[0] {
        Value::Map(map) =>
            Ok(Value::vector(map.entries.iter().map(|(_, value)| value.clone()).collect())),
        Value::Record(record) =>
            Ok(Value::vector(record.fields.iter().map(|(_, value)| value.clone()).collect())),
        other => Err(wrong_type("values", "a Map or Record", other, span)),
    });
    native!(interp, module, "range", Some(1), |_, args, span| {
        let (start, end) = if args.len() >= 2 {
            (as_int("range", &args[0], span)?, as_int("range", &args[1], span)?)
        } else {
            (0, as_int("range", &args[0], span)?)
        };
        Ok(Value::vector((start..end).map(Value::Int).collect()))
    });

    // ------------------------------------------------ higher-order functions
    native!(interp, module, "map", Some(2), |interp, args, span| {
        let items = as_vector("map", &args[0], span)?;
        let mut out = Vec::with_capacity(items.len());
        for item in items.iter() {
            out.push(interp.apply(args[1].clone(), vec![(None, item.clone())], span)?);
        }
        Ok(Value::vector(out))
    });
    native!(interp, module, "filter", Some(2), |interp, args, span| {
        let items = as_vector("filter", &args[0], span)?;
        let mut out = Vec::new();
        for item in items.iter() {
            if interp.apply(args[1].clone(), vec![(None, item.clone())], span)?.is_truthy() {
                out.push(item.clone());
            }
        }
        Ok(Value::vector(out))
    });
    native!(interp, module, "reduce", Some(3), |interp, args, span| {
        let items = as_vector("reduce", &args[0], span)?;
        let mut accumulator = args[1].clone();
        for item in items.iter() {
            accumulator = interp.apply(
                args[2].clone(),
                vec![(None, accumulator), (None, item.clone())],
                span,
            )?;
        }
        Ok(accumulator)
    });
    native!(interp, module, "each", Some(2), |interp, args, span| {
        let items = as_vector("each", &args[0], span)?;
        for item in items.iter() {
            interp.apply(args[1].clone(), vec![(None, item.clone())], span)?;
        }
        Ok(Value::Nil)
    });
    native!(interp, module, "any?", Some(2), |interp, args, span| {
        let items = as_vector("any?", &args[0], span)?;
        for item in items.iter() {
            if interp.apply(args[1].clone(), vec![(None, item.clone())], span)?.is_truthy() {
                return Ok(Value::Bool(true));
            }
        }
        Ok(Value::Bool(false))
    });
    native!(interp, module, "all?", Some(2), |interp, args, span| {
        let items = as_vector("all?", &args[0], span)?;
        for item in items.iter() {
            if !interp.apply(args[1].clone(), vec![(None, item.clone())], span)?.is_truthy() {
                return Ok(Value::Bool(false));
            }
        }
        Ok(Value::Bool(true))
    });
    native!(interp, module, "sort", Some(1), |_, args, span| {
        let mut items = (*as_vector("sort", &args[0], span)?).clone();
        items.sort_by(|left, right| left.cmp_value(right).unwrap_or(std::cmp::Ordering::Equal));
        Ok(Value::vector(items))
    });
    native!(interp, module, "sort-by", Some(2), |interp, args, span| {
        let items = as_vector("sort-by", &args[0], span)?;
        // Compute keys once so the comparator cannot re-enter the interpreter.
        let mut keyed = Vec::with_capacity(items.len());
        for item in items.iter() {
            let key = interp.apply(args[1].clone(), vec![(None, item.clone())], span)?;
            keyed.push((key, item.clone()));
        }
        keyed
            .sort_by(|left, right| left.0.cmp_value(&right.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(Value::vector(keyed.into_iter().map(|(_, item)| item).collect()))
    });

    // ----------------------------------------------------- Option and Result
    native!(interp, module, "Some", Some(1), |_, args, _| Ok(Value::some(args[0].clone())));
    native!(interp, module, "Ok", Some(1), |_, args, _| Ok(Value::ok(args[0].clone())));
    native!(interp, module, "Err", Some(1), |_, args, _| Ok(Value::err(args[0].clone())));
    module.exports.borrow_mut().insert("None".to_string(), Value::none());
    native!(interp, module, "some?", Some(1), |_, args, _| {
        Ok(Value::Bool(matches!(&args[0], Value::Variant(variant) if &*variant.variant == "Some")))
    });
    native!(interp, module, "ok?", Some(1), |_, args, _| {
        Ok(Value::Bool(matches!(&args[0], Value::Variant(variant) if &*variant.variant == "Ok")))
    });
    native!(interp, module, "nil?", Some(1), |_, args, _| Ok(Value::Bool(matches!(
        &args[0],
        Value::Nil
    ))));
    native!(interp, module, "unwrap-or", Some(2), |_, args, _| match &args[0] {
        Value::Variant(variant) if matches!(&*variant.variant, "Some" | "Ok") => {
            Ok(variant.fields.first().map(|(_, value)| value.clone()).unwrap_or(Value::Nil))
        }
        _ => Ok(args[1].clone()),
    });

    // ------------------------------------------------------------ conversion
    native!(interp, module, "str", None, |_, args, _| {
        let mut text = String::new();
        for value in &args {
            text.push_str(&crate::value::Display(value).to_string());
        }
        Ok(Value::str(text))
    });
    native!(interp, module, "type-of", Some(1), |_, args, _| Ok(Value::str(args[0].type_name())));
    native!(interp, module, "identity", Some(1), |_, args, _| Ok(args[0].clone()));

    // --------------------------------------------------------- mutable cells
    let cell = interp.module("Cell");
    native!(interp, cell, "new", Some(1), |_, args, _| Ok(Value::Cell(Rc::new(RefCell::new(
        args[0].clone()
    )))));
    native!(interp, cell, "get", Some(1), |_, args, span| match &args[0] {
        Value::Cell(cell) => Ok(cell.borrow().clone()),
        other => Err(wrong_type("Cell.get", "a Cell", other, span)),
    });
    native!(interp, cell, "set", Some(2), |_, args, span| match &args[0] {
        Value::Cell(cell) => {
            *cell.borrow_mut() = args[1].clone();
            Ok(Value::Nil)
        }
        other => Err(wrong_type("Cell.set", "a Cell", other, span)),
    });
    native!(interp, cell, "update", Some(2), |interp, args, span| match &args[0] {
        Value::Cell(cell) => {
            let current = cell.borrow().clone();
            let next = interp.apply(args[1].clone(), vec![(None, current)], span)?;
            *cell.borrow_mut() = next.clone();
            Ok(next)
        }
        other => Err(wrong_type("Cell.update", "a Cell", other, span)),
    });
    // Methods reachable as `(counter.update f)`.
    interp.modules.insert("Cell".to_string(), cell);
}

fn leak(name: &str) -> &'static str {
    Box::leak(name.to_string().into_boxed_str())
}

fn compare(
    name: &'static str,
    args: &[Value],
    span: Span,
    test: fn(std::cmp::Ordering) -> bool,
) -> EvalResult {
    for pair in args.windows(2) {
        match pair[0].cmp_value(&pair[1]) {
            Some(ordering) if test(ordering) => continue,
            Some(_) => return Ok(Value::Bool(false)),
            None => {
                return Err(Flow::panic(
                    Diagnostic::error(format!("`{name}` cannot compare these values"))
                        .with_code("type-error")
                        .at(span, format!("{} and {}", pair[0].type_name(), pair[1].type_name())),
                ))
            }
        }
    }
    Ok(Value::Bool(true))
}

fn compare_lt(_: &mut Interp, args: Vec<Value>, span: Span) -> EvalResult {
    compare("<", &args, span, |ordering| ordering.is_lt())
}
fn compare_le(_: &mut Interp, args: Vec<Value>, span: Span) -> EvalResult {
    compare("<=", &args, span, |ordering| ordering.is_le())
}
fn compare_gt(_: &mut Interp, args: Vec<Value>, span: Span) -> EvalResult {
    compare(">", &args, span, |ordering| ordering.is_gt())
}
fn compare_ge(_: &mut Interp, args: Vec<Value>, span: Span) -> EvalResult {
    compare(">=", &args, span, |ordering| ordering.is_ge())
}

fn install_string(interp: &mut Interp) {
    let module = interp.module("std.string");
    native!(interp, module, "split", Some(2), |_, args, span| {
        let text = as_string("string.split", &args[0], span)?;
        let separator = as_string("string.split", &args[1], span)?;
        let parts = if separator.is_empty() {
            text.chars().map(|ch| Value::str(ch.to_string())).collect()
        } else {
            text.split(separator.as_str()).map(Value::str).collect()
        };
        Ok(Value::vector(parts))
    });
    native!(interp, module, "join", Some(2), |_, args, span| {
        let items = as_vector("string.join", &args[0], span)?;
        let separator = as_string("string.join", &args[1], span)?;
        let parts: Vec<String> =
            items.iter().map(|item| crate::value::Display(item).to_string()).collect();
        Ok(Value::str(parts.join(&separator)))
    });
    native!(interp, module, "trim", Some(1), |_, args, span| {
        Ok(Value::str(as_string("string.trim", &args[0], span)?.trim().to_string()))
    });
    native!(interp, module, "upper", Some(1), |_, args, span| {
        Ok(Value::str(as_string("string.upper", &args[0], span)?.to_uppercase()))
    });
    native!(interp, module, "lower", Some(1), |_, args, span| {
        Ok(Value::str(as_string("string.lower", &args[0], span)?.to_lowercase()))
    });
    native!(interp, module, "starts-with?", Some(2), |_, args, span| {
        let text = as_string("string.starts-with?", &args[0], span)?;
        let prefix = as_string("string.starts-with?", &args[1], span)?;
        Ok(Value::Bool(text.starts_with(&prefix)))
    });
    native!(interp, module, "ends-with?", Some(2), |_, args, span| {
        let text = as_string("string.ends-with?", &args[0], span)?;
        let suffix = as_string("string.ends-with?", &args[1], span)?;
        Ok(Value::Bool(text.ends_with(&suffix)))
    });
    native!(interp, module, "replace", Some(3), |_, args, span| {
        let text = as_string("string.replace", &args[0], span)?;
        let from = as_string("string.replace", &args[1], span)?;
        let to = as_string("string.replace", &args[2], span)?;
        Ok(Value::str(text.replace(&from, &to)))
    });
    native!(interp, module, "chars", Some(1), |_, args, span| {
        let text = as_string("string.chars", &args[0], span)?;
        Ok(Value::vector(text.chars().map(|ch| Value::str(ch.to_string())).collect()))
    });
    native!(interp, module, "parse-int", Some(1), |_, args, span| {
        let text = as_string("string.parse-int", &args[0], span)?;
        Ok(match text.trim().parse::<i64>() {
            Ok(value) => Value::ok(Value::Int(value)),
            Err(error) => Value::err(Value::str(format!("`{text}` is not an integer: {error}"))),
        })
    });
    native!(interp, module, "parse-float", Some(1), |_, args, span| {
        let text = as_string("string.parse-float", &args[0], span)?;
        Ok(match text.trim().parse::<f64>() {
            Ok(value) => Value::ok(Value::Float(value)),
            Err(error) => Value::err(Value::str(format!("`{text}` is not a number: {error}"))),
        })
    });
    // Also reachable as methods on a String receiver.
    interp.modules.insert("String".to_string(), module);
}

fn install_math(interp: &mut Interp) {
    let module = interp.module("std.math");
    native!(interp, module, "abs", Some(1), |_, args, span| match &args[0] {
        Value::Int(value) => Ok(Value::Int(value.abs())),
        Value::Float(value) => Ok(Value::Float(value.abs())),
        other => Err(wrong_type("math.abs", "a number", other, span)),
    });
    native!(interp, module, "sqrt", Some(1), |_, args, span| {
        let (value, _, _) = numeric_pair("math.sqrt", &args[0], &Value::Int(0), span)?;
        Ok(Value::Float(value.sqrt()))
    });
    native!(interp, module, "pow", Some(2), |_, args, span| {
        let (base, exponent, is_float) = numeric_pair("math.pow", &args[0], &args[1], span)?;
        Ok(if is_float {
            Value::Float(base.powf(exponent))
        } else {
            Value::Int(base.powf(exponent) as i64)
        })
    });
    native!(interp, module, "floor", Some(1), |_, args, span| {
        let (value, _, _) = numeric_pair("math.floor", &args[0], &Value::Int(0), span)?;
        Ok(Value::Int(value.floor() as i64))
    });
    native!(interp, module, "ceil", Some(1), |_, args, span| {
        let (value, _, _) = numeric_pair("math.ceil", &args[0], &Value::Int(0), span)?;
        Ok(Value::Int(value.ceil() as i64))
    });
}

fn install_io(interp: &mut Interp) {
    let module = interp.module("std.io");
    native!(interp, module, "println", None, |interp: &mut Interp, args: Vec<Value>, _| {
        let text: Vec<String> =
            args.iter().map(|value| crate::value::Display(value).to_string()).collect();
        interp.out.write(&text.join(" "));
        interp.out.write("\n");
        Ok(Value::Nil)
    });
    native!(interp, module, "print", None, |interp: &mut Interp, args: Vec<Value>, _| {
        let text: Vec<String> =
            args.iter().map(|value| crate::value::Display(value).to_string()).collect();
        interp.out.write(&text.join(" "));
        Ok(Value::Nil)
    });
    native!(interp, module, "eprintln", None, |_, args: Vec<Value>, _| {
        let text: Vec<String> =
            args.iter().map(|value| crate::value::Display(value).to_string()).collect();
        eprintln!("{}", text.join(" "));
        Ok(Value::Nil)
    });
    native!(interp, module, "read-line", Some(0), |_, _args, _| {
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => Ok(Value::none()),
            Ok(_) => Ok(Value::some(Value::str(line.trim_end_matches(['\n', '\r']).to_string()))),
            Err(error) => Ok(Value::err(Value::str(error.to_string()))),
        }
    });
    // `println` and friends are convenient enough to live in the prelude.
    let prelude = interp.module(PRELUDE);
    for name in ["println", "print", "eprintln"] {
        if let Some(value) = module.exports.borrow().get(name) {
            prelude.exports.borrow_mut().insert(name.to_string(), value.clone());
        }
    }
}

fn install_fs(interp: &mut Interp) {
    let module = interp.module("std.fs");
    native!(interp, module, "read-text", Some(1), |_, args, span| {
        let path = as_string("fs.read-text", &args[0], span)?;
        Ok(match std::fs::read_to_string(&path) {
            Ok(text) => Value::ok(Value::str(text)),
            Err(error) => Value::err(io_error(&path, &error)),
        })
    });
    native!(interp, module, "write-text", Some(2), |_, args, span| {
        let path = as_string("fs.write-text", &args[0], span)?;
        let text = as_string("fs.write-text", &args[1], span)?;
        Ok(match std::fs::write(&path, text) {
            Ok(()) => Value::ok(Value::Nil),
            Err(error) => Value::err(io_error(&path, &error)),
        })
    });
    native!(interp, module, "exists?", Some(1), |_, args, span| {
        Ok(Value::Bool(std::path::Path::new(&as_string("fs.exists?", &args[0], span)?).exists()))
    });
    native!(interp, module, "read-lines", Some(1), |_, args, span| {
        let path = as_string("fs.read-lines", &args[0], span)?;
        Ok(match std::fs::read_to_string(&path) {
            Ok(text) => Value::ok(Value::vector(text.lines().map(Value::str).collect())),
            Err(error) => Value::err(io_error(&path, &error)),
        })
    });
    native!(interp, module, "list-dir", Some(1), |_, args, span| {
        let path = as_string("fs.list-dir", &args[0], span)?;
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
    });
}

/// Build a structured `IoError` record rather than a bare string.
fn io_error(path: &str, error: &std::io::Error) -> Value {
    Value::Record(Rc::new(RecordValue {
        type_name: Some(Rc::from("IoError")),
        fields: vec![
            (Rc::from("path"), Value::str(path.to_string())),
            (Rc::from("kind"), Value::keyword(&format!("{:?}", error.kind()).to_lowercase())),
            (Rc::from("message"), Value::str(error.to_string())),
        ],
    }))
}

fn install_json(interp: &mut Interp) {
    let module = interp.module("std.json");
    native!(interp, module, "encode", Some(1), |_, args, _| Ok(Value::str(crate::json::encode(
        &args[0], false
    ))));
    native!(interp, module, "encode-pretty", Some(1), |_, args, _| {
        Ok(Value::str(crate::json::encode(&args[0], true)))
    });
    native!(interp, module, "decode", Some(1), |_, args, span| {
        let text = as_string("json.decode", &args[0], span)?;
        Ok(match crate::json::decode(&text) {
            Ok(value) => Value::ok(value),
            Err(message) => Value::err(Value::str(message)),
        })
    });
}

fn install_log(interp: &mut Interp) {
    let module = interp.module("std.log");
    for level in ["debug", "info", "warn", "error"] {
        let value =
            Value::Native(Rc::new(NativeFn { name: leak(level), arity: Some(1), func: log_impl }));
        module.exports.borrow_mut().insert(level.to_string(), value);
    }
}

fn log_impl(interp: &mut Interp, args: Vec<Value>, _span: Span) -> EvalResult {
    let message =
        args.first().map(|value| crate::value::Display(value).to_string()).unwrap_or_default();
    let fields = args.get(1).map(|value| format!(" {value}")).unwrap_or_default();
    interp.out.write(&format!("{message}{fields}\n"));
    Ok(Value::Nil)
}

fn install_time(interp: &mut Interp) {
    let module = interp.module("std.time");
    native!(interp, module, "now-millis", Some(0), |_, _args, _| {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        Ok(Value::Int(millis))
    });
    native!(interp, module, "sleep-millis", Some(1), |_, args, span| {
        let millis = as_int("time.sleep-millis", &args[0], span)?;
        std::thread::sleep(std::time::Duration::from_millis(millis.max(0) as u64));
        Ok(Value::Nil)
    });
}

fn install_process(interp: &mut Interp) {
    let module = interp.module("std.process");
    native!(interp, module, "args", Some(0), |interp: &mut Interp, _args: Vec<Value>, _| {
        Ok(Value::vector(interp.program_args.iter().cloned().map(Value::str).collect()))
    });
    native!(interp, module, "env", Some(1), |_, args, span| {
        let name = as_string("process.env", &args[0], span)?;
        Ok(match std::env::var(&name) {
            Ok(value) => Value::some(Value::str(value)),
            Err(_) => Value::none(),
        })
    });
    native!(interp, module, "exit", Some(1), |_, args, span| {
        let code = as_int("process.exit", &args[0], span)?;
        std::process::exit(code as i32)
    });
}

fn install_test(interp: &mut Interp) {
    let module = interp.module("std.test");
    native!(interp, module, "assert", Some(1), |_, args, span| {
        if args[0].is_truthy() {
            return Ok(Value::Nil);
        }
        let message = args
            .get(1)
            .map(|value| crate::value::Display(value).to_string())
            .unwrap_or_else(|| "assertion failed".to_string());
        Err(Flow::panic(
            Diagnostic::error(message).with_code("assert").at(span, "this assertion is false"),
        ))
    });
    native!(interp, module, "assert-eq", Some(2), |_, args, span| {
        if args[0].eq_value(&args[1]) {
            return Ok(Value::Nil);
        }
        Err(Flow::panic(
            Diagnostic::error("values are not equal")
                .with_code("assert-eq")
                .at(span, "assertion failed here")
                .note(format!("expected: {}", args[0]))
                .note(format!("  actual: {}", args[1])),
        ))
    });
    native!(interp, module, "assert-ne", Some(2), |_, args, span| {
        if !args[0].eq_value(&args[1]) {
            return Ok(Value::Nil);
        }
        Err(Flow::panic(
            Diagnostic::error("values are equal but should not be")
                .with_code("assert-ne")
                .at(span, "assertion failed here")
                .note(format!("both sides: {}", args[0])),
        ))
    });
    let prelude = interp.module(PRELUDE);
    for name in ["assert", "assert-eq", "assert-ne"] {
        if let Some(value) = module.exports.borrow().get(name) {
            prelude.exports.borrow_mut().insert(name.to_string(), value.clone());
        }
    }
}

/// Tagged literal constructors: `#uuid "..."`, `#date "..."`, `#duration "..."`.
fn install_types(interp: &mut Interp) {
    let module = interp.module(PRELUDE);
    native!(interp, module, "uuid/parse", Some(1), |_, args, span| {
        let text = as_string("#uuid", &args[0], span)?;
        let hex: String = text.chars().filter(|ch| *ch != '-').collect();
        if hex.len() != 32 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(Flow::panic(
                Diagnostic::error("malformed UUID literal")
                    .with_code("uuid-literal")
                    .at(span, format!("`{text}` is not a UUID")),
            ));
        }
        Ok(Value::Record(Rc::new(RecordValue {
            type_name: Some(Rc::from("Uuid")),
            fields: vec![(Rc::from("text"), Value::str(text))],
        })))
    });
    native!(interp, module, "date/parse", Some(1), |_, args, span| {
        let text = as_string("#date", &args[0], span)?;
        Ok(Value::Record(Rc::new(RecordValue {
            type_name: Some(Rc::from("Date")),
            fields: vec![(Rc::from("text"), Value::str(text))],
        })))
    });
    native!(interp, module, "duration/parse", Some(1), |_, args, span| {
        let text = as_string("#duration", &args[0], span)?;
        let millis = parse_duration(&text).ok_or_else(|| {
            Flow::panic(
                Diagnostic::error("malformed duration literal")
                    .with_code("duration-literal")
                    .at(span, format!("`{text}` is not a duration"))
                    .help("write durations like `250ms`, `3s`, or `5m`"),
            )
        })?;
        Ok(Value::Record(Rc::new(RecordValue {
            type_name: Some(Rc::from("Duration")),
            fields: vec![(Rc::from("millis"), Value::Int(millis))],
        })))
    });
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

/// Syntax manipulation available to macros at compile time.
fn install_syntax(interp: &mut Interp) {
    let module = interp.module("std.syntax");
    native!(interp, module, "syntax?", Some(1), |_, args, _| {
        Ok(Value::Bool(matches!(&args[0], Value::Syntax(_))))
    });
    native!(interp, module, "symbol", Some(1), |_, args, span| {
        Ok(Value::Symbol(Rc::from(as_string("syntax.symbol", &args[0], span)?.as_str())))
    });
    native!(interp, module, "gensym", None, |_, args, _| {
        use std::cell::Cell as StdCell;
        thread_local! { static COUNTER: StdCell<u64> = const { StdCell::new(0) }; }
        let prefix = match args.first() {
            Some(Value::Str(text)) => (**text).clone(),
            _ => "g".to_string(),
        };
        let index = COUNTER.with(|counter| {
            let value = counter.get() + 1;
            counter.set(value);
            value
        });
        Ok(Value::Symbol(Rc::from(format!("{prefix}__{index}").as_str())))
    });
    let prelude = interp.module(PRELUDE);
    for name in ["gensym", "symbol"] {
        if let Some(value) = module.exports.borrow().get(name) {
            prelude.exports.borrow_mut().insert(name.to_string(), value.clone());
        }
    }
}
