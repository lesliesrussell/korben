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
pub fn apply(caller: &mut dyn Caller, function: &Value, args: Vec<Arg>, loc: Loc) -> Outcome {
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
