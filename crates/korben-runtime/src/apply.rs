//! Call dispatch and argument binding.
//!
//! Both execution modes route every call through [`apply`], so arity rules,
//! keyword arguments, defaults, constructor construction, and protocol dispatch
//! have exactly one implementation.

// korben-vtx

use crate::loc::{Fault, Loc};
use crate::value::{Arg, Body, Caller, Flow, Function, Outcome, Param, Value};

use std::rc::Rc;

/// Invoke a value.
///
/// Calling an `async fn` does not run it: it yields a task, which `await`,
/// `join-all`, or the end of the enclosing scope will run.
pub fn apply(caller: &mut dyn Caller, function: &Value, args: Vec<Arg>, loc: Loc) -> Outcome {
    if let Value::Fn(callable) = function {
        if callable.is_async {
            return Ok(crate::task::defer(function.clone(), args, loc));
        }
    }
    apply_now(caller, function, args, loc)
}

/// Invoke a value, running an async function's body rather than deferring it.
/// The scheduler uses this to start a task.
pub fn apply_now(caller: &mut dyn Caller, function: &Value, args: Vec<Arg>, loc: Loc) -> Outcome {
    let Value::Fn(callable) = function else {
        // Applying a non-function to no arguments yields the value itself,
        // which is what makes `(None)` and `(user.name)` read naturally.
        if args.is_empty() {
            return Ok(function.clone());
        }
        return Err(Flow::fault(
            Fault::new("not-callable", format!("`{}` is not callable", function.type_name()), loc)
                .label(format!("tried to call `{function}`")),
        ));
    };

    // korben-ycd
    // Every call comes through here, which is what lets profiling need nothing
    // from the program being profiled.
    if crate::profile::enabled() {
        crate::profile::enter(&callable.name);
        let outcome = dispatch(caller, function, args, loc);
        // Recorded however the call ended, so a failing program still profiles.
        crate::profile::leave();
        return outcome;
    }
    dispatch(caller, function, args, loc)
}

/// Invoke a function value, whatever kind of body it has.
fn dispatch(caller: &mut dyn Caller, function: &Value, args: Vec<Arg>, loc: Loc) -> Outcome {
    let Value::Fn(callable) = function else {
        return Ok(function.clone());
    };
    match &callable.body {
        Body::Native(native) => {
            let flat = flatten(args);
            check_arity(callable, flat.len(), loc)?;
            native(caller, flat, loc)
        }
        Body::Rust(rust) => rust(caller, args, loc),
        Body::Host(_) => caller.call_host(function, args, loc),
        Body::Ctor { type_name, variant, fields } => {
            construct(type_name, variant.as_deref(), fields, args, loc)
        }
        Body::Method { protocol, name } => {
            let Some(receiver) = args.first().map(|arg| arg.value.clone()) else {
                return Err(Flow::fault(
                    Fault::new("arity", format!("`{name}` needs a receiver"), loc)
                        .label("protocol methods take the value they dispatch on"),
                ));
            };
            match caller.find_method(&receiver, name) {
                Some(implementation) => apply(caller, &implementation, args, loc),
                None => Err(Flow::fault(
                    Fault::new(
                        "missing-impl",
                        format!("`{}` does not implement `{protocol}`", receiver.type_name()),
                        loc,
                    )
                    .label(format!("no implementation of `{name}` for this type"))
                    .help(format!("write `(impl {protocol} {} ...)`", receiver.type_name())),
                )),
            }
        }
    }
}

fn check_arity(callable: &Function, given: usize, loc: Loc) -> Result<(), Flow> {
    let required = callable.params.iter().filter(|param| !param.has_default).count();
    if callable.params.is_empty() || given >= required {
        return Ok(());
    }
    Err(Flow::fault(
        Fault::new(
            "arity",
            format!("`{}` expects at least {required} argument(s)", callable.name),
            loc,
        )
        .label(format!("{given} given")),
    ))
}

/// Flatten keyword arguments back into positional form, which is what native
/// functions without declared keyword parameters expect.
fn flatten(args: Vec<Arg>) -> Vec<Value> {
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        if let Some(keyword) = arg.keyword {
            out.push(Value::Keyword(Rc::from(keyword.as_str())));
        }
        out.push(arg.value);
    }
    out
}

/// Bind call arguments to declared parameters.
///
/// Returns one entry per parameter: `Some(value)` when the caller supplied it,
/// `None` when the callee should evaluate its default. A keyword argument only
/// binds by name when the callee declares that keyword; otherwise it passes
/// through positionally as a keyword literal followed by its value, which keeps
/// `(f :key 1)` meaning what it looks like.
pub fn bind_args(
    name: &str,
    params: &[Param],
    def_loc: Loc,
    args: Vec<Arg>,
    loc: Loc,
) -> Result<Vec<Option<Value>>, Flow> {
    let mut positional: Vec<Value> = Vec::new();
    let mut named: Vec<(String, Value)> = Vec::new();
    for arg in args {
        match arg.keyword {
            Some(keyword)
                if params
                    .iter()
                    .any(|param| param.keyword.as_deref() == Some(keyword.as_str())) =>
            {
                named.push((keyword, arg.value));
            }
            Some(keyword) => {
                positional.push(Value::Keyword(Rc::from(keyword.as_str())));
                positional.push(arg.value);
            }
            None => positional.push(arg.value),
        }
    }

    let expected: Vec<&Param> = params.iter().filter(|param| param.keyword.is_none()).collect();
    if positional.len() != expected.len() {
        let mut fault = Fault::new(
            "arity",
            format!("`{name}` expects {} argument(s) but got {}", expected.len(), positional.len()),
            loc,
        )
        .label("wrong number of arguments");
        if !def_loc.is_none() {
            fault = fault.note(format!("defined at {}", crate::loc::describe(def_loc)));
        }
        return Err(Flow::fault(fault));
    }

    let mut bound = Vec::with_capacity(params.len());
    let mut supplied = positional.into_iter();
    for param in params {
        match &param.keyword {
            None => bound.push(Some(supplied.next().expect("checked above"))),
            Some(keyword) => match named.iter().find(|(given, _)| given == &**keyword) {
                Some((_, value)) => bound.push(Some(value.clone())),
                None if param.has_default => bound.push(None),
                None => {
                    return Err(Flow::fault(
                        Fault::new(
                            "missing-argument",
                            format!("`{name}` requires the named argument `:{keyword}`"),
                            loc,
                        )
                        .label("named argument not supplied"),
                    ))
                }
            },
        }
    }

    if let Some((unknown, _)) = named
        .iter()
        .find(|(given, _)| !params.iter().any(|param| param.keyword.as_deref() == Some(given)))
    {
        return Err(Flow::fault(
            Fault::new(
                "unknown-argument",
                format!("`{name}` has no named argument `:{unknown}`"),
                loc,
            )
            .label("unknown keyword"),
        ));
    }
    Ok(bound)
}

/// Build a record or an enum variant from call arguments.
pub fn construct(
    type_name: &str,
    variant: Option<&str>,
    fields: &[crate::value::Sym],
    args: Vec<Arg>,
    loc: Loc,
) -> Outcome {
    let label = variant.unwrap_or(type_name);
    let mut positional = Vec::new();
    let mut named: Vec<(String, Value)> = Vec::new();
    for arg in args {
        match arg.keyword {
            Some(keyword) => named.push((keyword, arg.value)),
            None => positional.push(arg.value),
        }
    }

    // A single record or map argument names the fields directly. With one
    // declared field that is ambiguous, so it only applies when the argument's
    // own field names match the declaration.
    if named.is_empty() && positional.len() == 1 {
        let supplied: Option<Vec<(String, Value)>> = match &positional[0] {
            Value::Record(record) => Some(
                record
                    .fields
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.clone()))
                    .collect(),
            ),
            Value::Map(map) => Some(
                map.entries
                    .iter()
                    .map(|(key, value)| (crate::value::display(key), value.clone()))
                    .collect(),
            ),
            _ => None,
        };
        if let Some(supplied) = supplied {
            let names_match = fields.len() != 1
                || (supplied.len() == fields.len()
                    && supplied
                        .iter()
                        .all(|(name, _)| fields.iter().any(|field| &**field == name)));
            if names_match {
                named = supplied;
                positional.clear();
            }
        }
    }

    if !positional.is_empty() && positional.len() != fields.len() {
        return Err(Flow::fault(
            Fault::new(
                "arity",
                format!("`{label}` expects {} field(s) but got {}", fields.len(), positional.len()),
                loc,
            )
            .label(format!("fields: {}", render_fields(fields))),
        ));
    }

    let mut values: Vec<(crate::value::Sym, Value)> = Vec::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        let value = if !positional.is_empty() {
            positional[index].clone()
        } else {
            match named.iter().find(|(name, _)| name.as_str() == &**field) {
                Some((_, value)) => value.clone(),
                None => {
                    return Err(Flow::fault(
                        Fault::new(
                            "missing-field",
                            format!("missing field `{field}` for `{label}`"),
                            loc,
                        )
                        .label(format!("fields: {}", render_fields(fields))),
                    ))
                }
            }
        };
        values.push((field.clone(), value));
    }

    if let Some((unknown, _)) =
        named.iter().find(|(name, _)| !fields.iter().any(|field| &**field == name.as_str()))
    {
        return Err(Flow::fault(
            Fault::new("unknown-field", format!("`{label}` has no field `{unknown}`"), loc)
                .label(format!("fields: {}", render_fields(fields))),
        ));
    }

    Ok(match variant {
        Some(variant) => Value::Variant(Rc::new(crate::value::VariantValue {
            type_name: Rc::from(type_name),
            variant: Rc::from(variant),
            fields: values,
        })),
        None => Value::Record(Rc::new(crate::value::RecordValue {
            type_name: Some(Rc::from(type_name)),
            fields: values,
        })),
    })
}

fn render_fields(fields: &[crate::value::Sym]) -> String {
    if fields.is_empty() {
        return "none".to_string();
    }
    fields.iter().map(|field| field.to_string()).collect::<Vec<_>>().join(", ")
}

// ------------------------------------------------------- pattern primitives

/// True when `value` is the named enum variant.
pub fn is_variant(value: &Value, name: &str) -> bool {
    matches!(value, Value::Variant(variant) if &*variant.variant == name)
}

/// The nth payload field of a variant.
pub fn variant_at(value: &Value, index: usize) -> Option<Value> {
    let Value::Variant(variant) = value else { return None };
    variant.fields.get(index).map(|(_, value)| value.clone())
}

/// True when `value` is a vector of the required shape.
pub fn vector_shape(value: &Value, count: usize, has_rest: bool) -> bool {
    let Value::Vector(items) = value else { return false };
    if has_rest {
        items.len() >= count
    } else {
        items.len() == count
    }
}

pub fn vector_at(value: &Value, index: usize) -> Option<Value> {
    let Value::Vector(items) = value else { return None };
    items.get(index).cloned()
}

/// The tail of a vector, for a `[head ...tail]` rest pattern.
pub fn vector_from(value: &Value, index: usize) -> Value {
    match value {
        Value::Vector(items) if items.len() >= index => Value::vector(items[index..].to_vec()),
        _ => Value::vector(Vec::new()),
    }
}

/// Unwrap `Ok`/`Some` for the postfix `?` operator, propagating `Err`/`None`.
pub fn propagate(value: Value, loc: Loc) -> Outcome {
    match &value {
        Value::Variant(variant) => match &*variant.variant {
            "Ok" | "Some" => {
                Ok(variant.fields.first().map(|(_, value)| value.clone()).unwrap_or(Value::Nil))
            }
            "Err" | "None" => Err(Flow::Propagate(value.clone())),
            other => Err(Flow::fault(
                Fault::new("propagate-type", format!("`?` cannot propagate `{other}`"), loc)
                    .label("expected a Result or an Option")
                    .help("`?` works on Ok/Err and Some/None"),
            )),
        },
        other => Err(Flow::fault(
            Fault::new("propagate-type", "`?` needs a Result or an Option", loc)
                .label(format!("found {}", other.type_name())),
        )),
    }
}

/// Whether a thrown value matches a `catch` clause.
pub fn condition_matches(condition: &str, value: &Value) -> bool {
    if condition == "_" || condition == "Condition" {
        return true;
    }
    if value.type_name() == condition {
        return true;
    }
    match value {
        Value::Variant(variant) => &*variant.variant == condition,
        Value::Record(record) => record.type_name.as_deref() == Some(condition),
        _ => false,
    }
}

/// Read a member, reporting a fault when the value has no such field.
pub fn field(value: &Value, name: &str, loc: Loc) -> Outcome {
    match crate::value::member(value, name) {
        Some(found) => Ok(found),
        None => Err(Flow::fault(
            Fault::new(
                "unknown-field",
                format!("`{}` has no field `{name}`", value.type_name()),
                loc,
            )
            .label("unknown field")
            .help(format!("available fields: {}", crate::value::member_names(value))),
        )),
    }
}

/// The fault reported when no `match` arm applies.
pub fn no_match(value: &Value, loc: Loc) -> Flow {
    Flow::fault(
        Fault::new("match-failure", "no match arm applied", loc)
            .label(format!("`{value}` matched none of the arms"))
            .help("add a `_` arm or handle the missing case"),
    )
}

/// The fault reported when a `let` pattern does not match its value.
pub fn bad_binding(value: &Value, loc: Loc) -> Flow {
    Flow::fault(
        Fault::new("let-pattern", "binding pattern did not match", loc)
            .label(format!("value `{value}` does not match this pattern"))
            .help("use `match` when a binding can fail"),
    )
}

/// Release a resource when a `with` scope exits, on every path.
pub fn close_resource(caller: &mut dyn Caller, value: &Value, loc: Loc) {
    if let Some(method) = caller.find_method(value, "drop") {
        let _ = apply(caller, &method, vec![Arg::positional(value.clone())], loc);
    }
}

/// Report control flow that escaped to the top of a program.
pub fn report(flow: Flow) -> String {
    match flow {
        Flow::Panic(fault) => fault.render(),
        Flow::Condition(value, loc) => Fault::new("condition", "unhandled condition", loc)
            .label(format!("{value}"))
            .help("wrap the call in `(try ... (catch ...))`")
            .render(),
        Flow::Propagate(value) => {
            Fault::new("propagate", "unhandled error propagated to the top level", Loc::NONE)
                .label(format!("{value}"))
                .render()
        }
        Flow::Recur(_) => {
            Fault::new("recur-scope", "`recur` outside a loop or function", Loc::NONE).render()
        }
    }
}

/// Call a member of a value: `(receiver.name args...)`.
///
/// A field holding a function is called directly; otherwise the name is looked
/// up as a method on the receiver's type. Reading a field with no arguments is
/// what makes `(user.name)` mean the field rather than a call.
pub fn call_member(
    caller: &mut dyn Caller,
    receiver: &Value,
    name: &str,
    args: Vec<Arg>,
    loc: Loc,
) -> Outcome {
    if let Some(found) = crate::value::member(receiver, name) {
        if matches!(found, Value::Fn(_)) {
            return apply(caller, &found, args, loc);
        }
        if args.is_empty() {
            return Ok(found);
        }
    }
    if let Some(method) = caller.find_method(receiver, name) {
        let mut args = args;
        args.insert(0, Arg::positional(receiver.clone()));
        return apply(caller, &method, args, loc);
    }
    Err(Flow::fault(
        Fault::new(
            "unknown-member",
            format!("`{}` has no member `{name}`", receiver.type_name()),
            loc,
        )
        .label("unknown field or method")
        .help(format!("available fields: {}", crate::value::member_names(receiver))),
    ))
}
