//! The evaluator.
//!
//! Korben's development execution mode is a direct interpreter over the typed
//! AST. It shares reader semantics, macro behavior, and observable language
//! semantics with the eventual native backend, which is what the specification
//! requires of the two execution modes.

// korben-6bc

use crate::ast::*;
use crate::value::{
    apply, as_closure, as_syntax, bind_args, closure_value, display, loc_of, member, span_of,
    syntax_value, Arg as RtArg, Assign, Caller, Closure, Env, EvalResult, Fault, Flow, Loc,
    MapValue, ModuleRuntime, Param as RtParam, RecordValue, Value,
};
use korben_syntax::span::Span;
use korben_syntax::Syntax;
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;

/// Where a running program's output goes. The REPL and test runner capture it.
pub enum Output {
    Stdout,
    Captured(String),
}

impl Output {
    pub fn write(&mut self, text: &str) {
        match self {
            Output::Stdout => {
                let mut stdout = std::io::stdout();
                let _ = stdout.write_all(text.as_bytes());
                let _ = stdout.flush();
            }
            Output::Captured(buffer) => buffer.push_str(text),
        }
    }
}

/// Runtime metadata for a nominal type.
pub struct TypeInfo {
    pub name: String,
    pub fields: Vec<String>,
    pub variants: Vec<(String, Vec<String>)>,
    pub is_enum: bool,
}

pub struct Interp {
    pub modules: HashMap<String, Rc<ModuleRuntime>>,
    pub current: Rc<ModuleRuntime>,
    pub types: HashMap<String, Rc<TypeInfo>>,
    /// Variant constructor name to owning enum type.
    pub variant_owner: HashMap<String, String>,
    pub protocols: HashMap<String, Vec<String>>,
    /// `(protocol, type)` to method implementations.
    pub impls: HashMap<(String, String), HashMap<String, Value>>,
    /// Method name to the protocol that declares it.
    pub method_owner: HashMap<String, String>,
    pub out: Output,
    depth: usize,
    /// Maximum call depth before reporting recursion as a fault. The default is
    /// conservative for a standard 2 MiB stack; the CLI raises it after moving
    /// evaluation onto a thread with a much larger stack.
    pub max_depth: usize,
    /// Names of tests registered by loaded modules.
    pub tests: Vec<(String, String, Rc<TestDecl>, Rc<ModuleRuntime>)>,
    /// True while running inside `unsafe`, for diagnostics.
    pub in_unsafe: bool,
    /// Macros visible during expansion, keyed by name.
    pub macros: HashMap<String, Rc<crate::expand::MacroEntry>>,
}

/// The runtime calls back into the interpreter through this trait: it owns
/// closure bodies, protocol implementations, and program output.
impl Caller for Interp {
    fn call_host(&mut self, function: &Value, args: Vec<RtArg>, loc: Loc) -> EvalResult {
        match as_closure(function) {
            Some(closure) => self.call_closure(&closure, args, span_of(loc)),
            None => Err(Flow::fault(
                Fault::error("not callable")
                    .with_code("not-callable")
                    .at(loc, "this value has no callable body"),
            )),
        }
    }

    fn find_method(&mut self, receiver: &Value, method: &str) -> Option<Value> {
        Interp::find_method(self, receiver, method)
    }

    fn write(&mut self, text: &str) {
        self.out.write(text);
    }
}

impl Default for Interp {
    fn default() -> Interp {
        Interp::new()
    }
}

impl Interp {
    pub fn new() -> Interp {
        let root = Rc::new(ModuleRuntime::new("user"));
        let mut modules = HashMap::new();
        modules.insert("user".to_string(), root.clone());
        let mut interp = Interp {
            modules,
            current: root,
            types: HashMap::new(),
            variant_owner: HashMap::new(),
            protocols: HashMap::new(),
            impls: HashMap::new(),
            method_owner: HashMap::new(),
            out: Output::Stdout,
            depth: 0,
            max_depth: 128,
            tests: Vec::new(),
            in_unsafe: false,
            macros: HashMap::new(),
        };
        crate::builtins::install(&mut interp);
        interp
    }

    pub fn module(&mut self, name: &str) -> Rc<ModuleRuntime> {
        if let Some(module) = self.modules.get(name) {
            return module.clone();
        }
        let module = Rc::new(ModuleRuntime::new(name));
        self.modules.insert(name.to_string(), module.clone());
        module
    }

    // ------------------------------------------------------------- lookup

    pub fn lookup_global(&self, module: &Rc<ModuleRuntime>, name: &str) -> Option<Value> {
        if let Some(value) = module.globals.borrow().get(name) {
            return Some(value.clone());
        }
        let imported = module.imported.borrow().get(name).cloned();
        if let Some((source, original)) = imported {
            if let Some(source) = self.modules.get(&source) {
                if let Some(value) = source.exports.borrow().get(&original) {
                    return Some(value.clone());
                }
            }
        }
        // Every module sees the prelude without importing it.
        self.modules.get(crate::builtins::PRELUDE)?.exports.borrow().get(name).cloned()
    }

    /// Resolve `alias.name` / `alias/name` against imports and known modules.
    pub fn lookup_path(&self, alias: &str, name: &str) -> Option<Value> {
        let target = self
            .current
            .aliases
            .borrow()
            .get(alias)
            .cloned()
            .or_else(|| self.modules.contains_key(alias).then(|| alias.to_string()))?;
        let module = self.modules.get(&target)?;
        module
            .exports
            .borrow()
            .get(name)
            .cloned()
            .or_else(|| module.globals.borrow().get(name).cloned())
    }

    // --------------------------------------------------------- evaluation

    pub fn eval_body(&mut self, body: &Body, env: &Env) -> EvalResult {
        let scope = env.child();
        let mut deferred: Vec<(Body, Env)> = Vec::new();
        let mut result = Value::Nil;
        let mut outcome: Result<(), Flow> = Ok(());

        for stmt in &body.stmts {
            match stmt {
                Stmt::Let { pattern, value, span, .. } => match self.eval(value, &scope) {
                    Ok(value) => {
                        if !self.bind_pattern(pattern, &value, &scope) {
                            outcome = Err(Flow::fault(
                                Fault::error("binding pattern did not match")
                                    .with_code("let-pattern")
                                    .at(
                                        loc_of(*span),
                                        format!("value `{value}` does not match this pattern"),
                                    )
                                    .help("use `match` when a binding can fail"),
                            ));
                            break;
                        }
                        result = Value::Nil;
                    }
                    Err(flow) => {
                        outcome = Err(flow);
                        break;
                    }
                },
                Stmt::Var { name, value, .. } => match self.eval(value, &scope) {
                    Ok(value) => {
                        scope.define_var(Rc::from(name.as_str()), value);
                        result = Value::Nil;
                    }
                    Err(flow) => {
                        outcome = Err(flow);
                        break;
                    }
                },
                Stmt::Defer { body, .. } => {
                    deferred.push((body.clone(), scope.clone()));
                    result = Value::Nil;
                }
                Stmt::Expr(expr) => match self.eval(expr, &scope) {
                    Ok(value) => result = value,
                    Err(flow) => {
                        outcome = Err(flow);
                        break;
                    }
                },
            }
        }

        // `defer` runs on every exit path, last-in-first-out.
        while let Some((body, env)) = deferred.pop() {
            if let Err(flow) = self.eval_body(&body, &env) {
                if outcome.is_ok() {
                    outcome = Err(flow);
                }
            }
        }
        outcome?;
        Ok(result)
    }

    pub fn eval(&mut self, expr: &Expr, env: &Env) -> EvalResult {
        match expr {
            Expr::Nil(_) => Ok(Value::Nil),
            Expr::Bool(value, _) => Ok(Value::Bool(*value)),
            Expr::Int(value, _) => Ok(Value::Int(*value)),
            Expr::Float(value, _) => Ok(Value::Float(*value)),
            Expr::Str(value, _) => Ok(Value::str(value.clone())),
            Expr::Keyword(name, _) => Ok(Value::Keyword(Rc::from(name.as_str()))),
            Expr::Interp(parts, _) => {
                let mut text = String::new();
                for part in parts {
                    match part {
                        InterpPart::Text(literal) => text.push_str(literal),
                        InterpPart::Expr(expr) => {
                            let value = self.eval(expr, env)?;
                            text.push_str(&display(&value));
                        }
                    }
                }
                Ok(Value::str(text))
            }
            Expr::Var(name, span) => self.lookup_var(name, *span, env),
            Expr::Path { module, name, span } => self.lookup_path(module, name).ok_or_else(|| {
                Flow::fault(
                    Fault::error(format!("`{module}/{name}` is not defined"))
                        .with_code("unbound-name")
                        .at(loc_of(*span), "not found in that module")
                        .help(format!("check that `{module}` is imported and exports `{name}`")),
                )
            }),
            Expr::Vector(items, _) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.eval(item, env)?);
                }
                Ok(Value::vector(values))
            }
            Expr::Set(items, _) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.eval(item, env)?);
                }
                Ok(Value::set(values))
            }
            Expr::Map(entries, _) => {
                let mut map = MapValue::default();
                for (key, value) in entries {
                    let key = self.eval(key, env)?;
                    let value = self.eval(value, env)?;
                    map.insert(key, value);
                }
                Ok(Value::Map(Rc::new(map)))
            }
            Expr::Record { type_name, fields, .. } => {
                let mut values = Vec::with_capacity(fields.len());
                for (name, expr, _) in fields {
                    values.push((Rc::from(name.as_str()), self.eval(expr, env)?));
                }
                Ok(Value::Record(Rc::new(RecordValue {
                    type_name: type_name.as_deref().map(Rc::from),
                    fields: values,
                })))
            }
            Expr::If { cond, then, els, .. } => {
                if self.eval(cond, env)?.is_truthy() {
                    self.eval(then, env)
                } else {
                    match els {
                        Some(els) => self.eval(els, env),
                        None => Ok(Value::Nil),
                    }
                }
            }
            Expr::And(operands, _) => {
                let mut result = Value::Bool(true);
                for operand in operands {
                    result = self.eval(operand, env)?;
                    if !result.is_truthy() {
                        break;
                    }
                }
                Ok(result)
            }
            Expr::Or(operands, _) => {
                let mut result = Value::Bool(false);
                for operand in operands {
                    result = self.eval(operand, env)?;
                    if result.is_truthy() {
                        break;
                    }
                }
                Ok(result)
            }
            Expr::Do(body, _) => self.eval_body(body, env),
            Expr::Lambda(decl, _) => Ok(closure_value(Rc::new(Closure {
                decl: decl.clone(),
                env: env.clone(),
                module: self.current.name.clone(),
            }))),
            Expr::Field { target, name, span } => {
                let value = self.eval_field_target(target, name, *span, env)?;
                match value {
                    FieldTarget::Value(value) => self.field_of(&value, name, *span),
                    FieldTarget::Resolved(value) => Ok(value),
                }
            }
            Expr::Call { callee, args, span } => self.eval_call(callee, args, *span, env),
            Expr::Match { scrutinee, arms, span } => {
                let value = self.eval(scrutinee, env)?;
                for arm in arms {
                    let scope = env.child();
                    if !self.bind_pattern(&arm.pattern, &value, &scope) {
                        continue;
                    }
                    if let Some(guard) = &arm.guard {
                        if !self.eval(guard, &scope)?.is_truthy() {
                            continue;
                        }
                    }
                    return self.eval(&arm.body, &scope);
                }
                Err(Flow::fault(
                    Fault::error("no match arm applied")
                        .with_code("match-failure")
                        .at(loc_of(*span), format!("`{value}` matched none of the arms"))
                        .help("add a `_` arm or handle the missing case"),
                ))
            }
            Expr::Loop { bindings, body, span } => {
                let mut values = Vec::with_capacity(bindings.len());
                for (_, expr) in bindings {
                    values.push(self.eval(expr, env)?);
                }
                loop {
                    let scope = env.child();
                    for ((name, _), value) in bindings.iter().zip(values.iter()) {
                        scope.define(Rc::from(name.as_str()), value.clone());
                    }
                    match self.eval_body(body, &scope) {
                        Ok(value) => return Ok(value),
                        Err(Flow::Recur(next)) => {
                            if next.len() != bindings.len() {
                                return Err(Flow::fault(
                                    Fault::error("`recur` argument count does not match the loop")
                                        .with_code("recur-arity")
                                        .at(
                                            loc_of(*span),
                                            format!(
                                                "loop binds {} value(s) but `recur` passed {}",
                                                bindings.len(),
                                                next.len()
                                            ),
                                        ),
                                ));
                            }
                            values = next;
                        }
                        Err(flow) => return Err(flow),
                    }
                }
            }
            Expr::Recur(args, _) => {
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(self.eval(arg, env)?);
                }
                Err(Flow::Recur(values))
            }
            Expr::Assign { name, value, span } => {
                let value = self.eval(value, env)?;
                match env.assign(name, value) {
                    Assign::Done => Ok(Value::Nil),
                    Assign::Immutable => Err(Flow::fault(
                        Fault::error(format!("cannot assign to `{name}`"))
                            .with_code("immutable-assign")
                            .at(loc_of(*span), "this binding is immutable")
                            .help(format!("declare it with `(var {name} ...)` to allow `set!`")),
                    )),
                    Assign::Unbound => Err(Flow::fault(
                        Fault::error(format!("cannot assign to `{name}`"))
                            .with_code("unbound-assign")
                            .at(loc_of(*span), "no binding with this name is in scope")
                            .help(format!("declare it with `(var {name} ...)` first")),
                    )),
                }
            }
            Expr::Propagate(inner, span) => {
                let value = self.eval(inner, env)?;
                match &value {
                    Value::Variant(variant) => match &*variant.variant {
                        "Ok" | "Some" => Ok(variant
                            .fields
                            .first()
                            .map(|(_, value)| value.clone())
                            .unwrap_or(Value::Nil)),
                        "Err" | "None" => Err(Flow::Propagate(value.clone())),
                        other => Err(Flow::fault(
                            Fault::error(format!("`?` cannot propagate `{other}`"))
                                .with_code("propagate-type")
                                .at(loc_of(*span), "expected a Result or an Option")
                                .help("`?` works on Ok/Err and Some/None"),
                        )),
                    },
                    other => Err(Flow::fault(
                        Fault::error("`?` needs a Result or an Option")
                            .with_code("propagate-type")
                            .at(loc_of(*span), format!("found {}", other.type_name())),
                    )),
                }
            }
            Expr::Throw(inner, span) => {
                let value = self.eval(inner, env)?;
                Err(Flow::Condition(value, loc_of(*span)))
            }
            Expr::Try { body, catches, finally, .. } => {
                let result = self.eval_body(body, env);
                let result = match result {
                    Err(Flow::Condition(value, span)) => {
                        let mut handled = None;
                        for arm in catches {
                            if condition_matches(&arm.condition, &value) {
                                let scope = env.child();
                                scope.define(Rc::from(arm.binding.as_str()), value.clone());
                                handled = Some(self.eval_body(&arm.body, &scope));
                                break;
                            }
                        }
                        handled.unwrap_or(Err(Flow::Condition(value, span)))
                    }
                    other => other,
                };
                if let Some(finally) = finally {
                    // Cleanup errors never mask the primary failure.
                    let cleanup = self.eval_body(finally, env);
                    if result.is_ok() {
                        cleanup?;
                    }
                }
                result
            }
            Expr::With { name, value, body, span } => {
                let resource = self.eval(value, env)?;
                let scope = env.child();
                scope.define(Rc::from(name.as_str()), resource.clone());
                let result = self.eval_body(body, &scope);
                self.close_resource(&resource, *span);
                result
            }
            Expr::Unsafe(body, _) => {
                let previous = self.in_unsafe;
                self.in_unsafe = true;
                let result = self.eval_body(body, env);
                self.in_unsafe = previous;
                result
            }
            // The v0.1 runtime executes async work eagerly on the calling task.
            // Structured concurrency lands with the async runtime in Milestone D.
            Expr::Await(inner, _) => self.eval(inner, env),
            Expr::TaskScope { body, .. } => self.eval_body(body, env),
            Expr::Quote(syntax, _) => Ok(quote_value(syntax)),
            Expr::SyntaxQuote(template, _) => {
                let built = self.build_template(template, env)?;
                Ok(syntax_value(Rc::new(built)))
            }
        }
    }

    fn lookup_var(&mut self, name: &str, span: Span, env: &Env) -> EvalResult {
        if let Some(value) = env.lookup(name) {
            return Ok(value);
        }
        let current = self.current.clone();
        if let Some(value) = self.lookup_global(&current, name) {
            return Ok(value);
        }
        let suggestion = self.suggest_name(name);
        let mut diagnostic = Fault::error(format!("`{name}` is not defined"))
            .with_code("unbound-name")
            .at(loc_of(span), "no binding with this name is in scope");
        if let Some(suggestion) = suggestion {
            diagnostic = diagnostic.help(format!("did you mean `{suggestion}`?"));
        }
        Err(Flow::fault(diagnostic))
    }

    /// Closest known name by edit distance, used for `did you mean` help.
    fn suggest_name(&self, name: &str) -> Option<String> {
        let mut best: Option<(usize, String)> = None;
        let globals = self.current.globals.borrow();
        let imported = self.current.imported.borrow();
        let candidates = globals.keys().chain(imported.keys());
        for candidate in candidates {
            let distance = edit_distance(name, candidate);
            if distance <= 2 && best.as_ref().map(|(best, _)| distance < *best).unwrap_or(true) {
                best = Some((distance, candidate.clone()));
            }
        }
        best.map(|(_, name)| name)
    }

    // --------------------------------------------------------------- calls

    fn eval_call(&mut self, callee: &Expr, args: &[Arg], span: Span, env: &Env) -> EvalResult {
        // `(target.method args)` binds the receiver when `method` is a method.
        if let Expr::Field { target, name, span: field_span } = callee {
            match self.eval_field_target(target, name, *field_span, env)? {
                FieldTarget::Resolved(value) => {
                    let arguments = self.eval_args(args, env)?;
                    return apply(self, &value, arguments, loc_of(span));
                }
                FieldTarget::Value(receiver) => {
                    let arguments = self.eval_args(args, env)?;
                    let _ = field_span;
                    return korben_runtime::apply::call_member(
                        self,
                        &receiver,
                        name,
                        arguments,
                        loc_of(span),
                    );
                }
            }
        }
        let function = self.eval(callee, env)?;
        let arguments = self.eval_args(args, env)?;
        apply(self, &function, arguments, loc_of(span))
    }

    fn eval_args(&mut self, args: &[Arg], env: &Env) -> Result<Vec<RtArg>, Flow> {
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(RtArg { keyword: arg.keyword.clone(), value: self.eval(&arg.value, env)? });
        }
        Ok(values)
    }

    /// Invoke a value. Dispatch itself lives in the runtime, so the interpreter
    /// and generated code agree on arity, keyword arguments, and construction.
    pub fn apply(
        &mut self,
        function: Value,
        args: Vec<(Option<String>, Value)>,
        span: Span,
    ) -> EvalResult {
        let args = args.into_iter().map(|(keyword, value)| RtArg { keyword, value }).collect();
        apply(self, &function, args, loc_of(span))
    }

    fn call_closure(&mut self, closure: &Rc<Closure>, args: Vec<RtArg>, span: Span) -> EvalResult {
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return Err(Flow::fault(
                Fault::error("recursion limit reached")
                    .with_code("stack-overflow")
                    .at(loc_of(span), format!("while calling `{}`", closure.decl.name))
                    .help("use `loop`/`recur` for unbounded iteration"),
            ));
        }

        let previous_module = self.current.clone();
        if let Some(module) = self.modules.get(&*closure.module) {
            self.current = module.clone();
        }

        let mut arguments = args;
        let result = loop {
            let scope = closure.env.child();
            match self.bind_params(&closure.decl, arguments, &scope, span) {
                Ok(()) => {}
                Err(flow) => break Err(flow),
            }
            // Named self-recursion: `recur` re-enters with new arguments.
            match self.eval_body(&closure.decl.body, &scope) {
                Err(Flow::Recur(values)) => {
                    arguments = values.into_iter().map(RtArg::positional).collect();
                    continue;
                }
                // `?` propagation stops at the function boundary.
                Err(Flow::Propagate(value)) => break Ok(value),
                other => break other,
            }
        };

        self.current = previous_module;
        self.depth -= 1;
        result
    }

    fn bind_params(
        &mut self,
        decl: &FnDecl,
        args: Vec<RtArg>,
        scope: &Env,
        span: Span,
    ) -> Result<(), Flow> {
        let params: Vec<RtParam> = decl
            .params
            .iter()
            .map(|param| RtParam {
                name: Rc::from(param.name.as_str()),
                keyword: param.keyword.as_deref().map(Rc::from),
                has_default: param.default.is_some(),
            })
            .collect();
        let bound = bind_args(&decl.name, &params, loc_of(decl.span), args, loc_of(span))?;
        for (param, value) in decl.params.iter().zip(bound) {
            let value = match value {
                Some(value) => value,
                None => match &param.default {
                    Some(default) => self.eval(default, scope)?,
                    None => Value::Nil,
                },
            };
            scope.define(Rc::from(param.name.as_str()), value);
        }
        Ok(())
    }

    // -------------------------------------------------------------- fields

    fn eval_field_target(
        &mut self,
        target: &Expr,
        name: &str,
        span: Span,
        env: &Env,
    ) -> Result<FieldTarget, Flow> {
        // `alias.name` may be a module member rather than a field access.
        if let Expr::Var(root, _) = target {
            if env.lookup(root).is_none() {
                let current = self.current.clone();
                if self.lookup_global(&current, root).is_none() {
                    if let Some(value) = self.lookup_path(root, name) {
                        return Ok(FieldTarget::Resolved(value));
                    }
                    let suggestion = self.suggest_name(root);
                    let mut diagnostic = Fault::error(format!("`{root}` is not defined"))
                        .with_code("unbound-name")
                        .at(loc_of(span), format!("no binding or module named `{root}`"));
                    if let Some(suggestion) = suggestion {
                        diagnostic = diagnostic.help(format!("did you mean `{suggestion}`?"));
                    }
                    return Err(Flow::fault(diagnostic));
                }
            }
        }
        Ok(FieldTarget::Value(self.eval(target, env)?))
    }

    pub fn field_of(&mut self, value: &Value, name: &str, span: Span) -> EvalResult {
        match member(value, name) {
            Some(value) => Ok(value),
            None => Err(Flow::fault(
                Fault::error(format!("`{}` has no field `{name}`", value.type_name()))
                    .with_code("unknown-field")
                    .at(loc_of(span), "unknown field")
                    .help(format!("available fields: {}", field_names(value))),
            )),
        }
    }

    /// Find a protocol implementation or a built-in method for a receiver.
    pub fn find_method(&self, receiver: &Value, name: &str) -> Option<Value> {
        let type_name = receiver.type_name();
        if let Some(protocol) = self.method_owner.get(name) {
            if let Some(methods) = self.impls.get(&(protocol.clone(), type_name.clone())) {
                if let Some(method) = methods.get(name) {
                    return Some(method.clone());
                }
            }
        }
        korben_runtime::std::method_of(&type_name, name)
    }

    /// Release a resource when a `with` scope exits, on every path.
    fn close_resource(&mut self, value: &Value, span: Span) {
        if let Some(method) = self.find_method(value, "drop") {
            let _ = self.apply(method, vec![(None, value.clone())], span);
        }
    }

    // ------------------------------------------------------------ patterns

    /// Try to match `value` against `pattern`, defining bindings in `scope`.
    pub fn bind_pattern(&mut self, pattern: &Pattern, value: &Value, scope: &Env) -> bool {
        match pattern {
            Pattern::Wildcard(_) => true,
            Pattern::Binding(name, _) => {
                scope.define(Rc::from(name.as_str()), value.clone());
                true
            }
            Pattern::Nil(_) => matches!(value, Value::Nil),
            Pattern::Bool(expected, _) => {
                matches!(value, Value::Bool(actual) if actual == expected)
            }
            Pattern::Int(expected, _) => value.eq_value(&Value::Int(*expected)),
            Pattern::Float(expected, _) => value.eq_value(&Value::Float(*expected)),
            Pattern::Str(expected, _) => {
                matches!(value, Value::Str(actual) if actual.as_str() == expected)
            }
            Pattern::Keyword(expected, _) => {
                matches!(value, Value::Keyword(actual) if &**actual == expected)
            }
            Pattern::Typed { inner, .. } => self.bind_pattern(inner, value, scope),
            Pattern::Variant { name, positional, named, .. } => {
                let Value::Variant(variant) = value else { return false };
                if &*variant.variant != name.as_str() {
                    return false;
                }
                for (index, sub) in positional.iter().enumerate() {
                    let Some((_, field)) = variant.fields.get(index) else { return false };
                    let field = field.clone();
                    if !self.bind_pattern(sub, &field, scope) {
                        return false;
                    }
                }
                for (field_name, sub) in named {
                    let Some(field) = variant.get(field_name).cloned() else { return false };
                    if !self.bind_pattern(sub, &field, scope) {
                        return false;
                    }
                }
                true
            }
            Pattern::Vector { items, rest, .. } => {
                let Value::Vector(values) = value else { return false };
                match rest {
                    Some(_) if values.len() < items.len() => return false,
                    None if values.len() != items.len() => return false,
                    _ => {}
                }
                for (sub, value) in items.iter().zip(values.iter()) {
                    let value = value.clone();
                    if !self.bind_pattern(sub, &value, scope) {
                        return false;
                    }
                }
                if let Some(Some(name)) = rest {
                    let tail = values[items.len()..].to_vec();
                    scope.define(Rc::from(name.as_str()), Value::vector(tail));
                }
                true
            }
            Pattern::Map { entries, .. } | Pattern::Record { fields: entries, .. } => {
                for (key, sub) in entries {
                    let Some(found) = member(value, key) else { return false };
                    if !self.bind_pattern(sub, &found, scope) {
                        return false;
                    }
                }
                true
            }
        }
    }

    // ------------------------------------------------- syntax construction

    /// Build a syntax object from a syntax-quote template, filling `~` holes.
    fn build_template(&mut self, template: &Template, env: &Env) -> Result<Syntax, Flow> {
        use korben_syntax::reader::Datum;
        match template {
            Template::Literal(form) => Ok(form.clone()),
            Template::Unquote(expr) => {
                let value = self.eval(expr, env)?;
                Ok(value_to_syntax(&value, expr.span()))
            }
            Template::Splice(expr) => {
                // A bare splice outside a sequence behaves like an unquote.
                let value = self.eval(expr, env)?;
                Ok(value_to_syntax(&value, expr.span()))
            }
            Template::List(items, span) => {
                Ok(Syntax::new(Datum::List(self.build_items(items, env)?), *span))
            }
            Template::Vector(items, span) => {
                Ok(Syntax::new(Datum::Vector(self.build_items(items, env)?), *span))
            }
            Template::Map(items, span) => {
                Ok(Syntax::new(Datum::Map(self.build_items(items, env)?), *span))
            }
            Template::Set(items, span) => {
                Ok(Syntax::new(Datum::Set(self.build_items(items, env)?), *span))
            }
        }
    }

    fn build_items(&mut self, items: &[Template], env: &Env) -> Result<Vec<Syntax>, Flow> {
        let mut built = Vec::with_capacity(items.len());
        for item in items {
            if let Template::Splice(expr) = item {
                let value = self.eval(expr, env)?;
                built.extend(splice_parts(&value, expr.span()));
                continue;
            }
            built.push(self.build_template(item, env)?);
        }
        Ok(built)
    }
}

/// Flatten a spliced value into the syntax forms it contributes.
fn splice_parts(value: &Value, span: Span) -> Vec<Syntax> {
    use korben_syntax::reader::Datum;
    match value {
        Value::Vector(items) => items.iter().map(|item| value_to_syntax(item, span)).collect(),
        Value::Nil => Vec::new(),
        other => match as_syntax(other) {
            Some(syntax) => match &syntax.datum {
                Datum::List(items) | Datum::Vector(items) => items.clone(),
                _ => vec![(*syntax).clone()],
            },
            None => vec![value_to_syntax(other, span)],
        },
    }
}

enum FieldTarget {
    /// The expression evaluated to a value; look the field up on it.
    Value(Value),
    /// The whole `a.b` resolved to a module member.
    Resolved(Value),
}

fn field_names(value: &Value) -> String {
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

fn condition_matches(condition: &str, value: &Value) -> bool {
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

/// `'form` produces plain data, not a syntax object.
pub fn quote_value(syntax: &Syntax) -> Value {
    use korben_syntax::reader::Datum;
    match &syntax.datum {
        Datum::Nil => Value::Nil,
        Datum::Bool(value) => Value::Bool(*value),
        Datum::Int(value) => Value::Int(*value),
        Datum::Float(value) => Value::Float(*value),
        Datum::Str(value) => Value::str(value.clone()),
        Datum::Keyword(name) => Value::Keyword(Rc::from(name.as_str())),
        Datum::Symbol(name) => Value::Symbol(Rc::from(name.as_str())),
        Datum::List(items) | Datum::Vector(items) => {
            Value::vector(items.iter().map(quote_value).collect())
        }
        Datum::Set(items) => Value::Set(Rc::new(items.iter().map(quote_value).collect())),
        Datum::Map(items) => {
            let mut map = MapValue::default();
            for pair in items.chunks(2) {
                if let [key, value] = pair {
                    map.insert(quote_value(key), quote_value(value));
                }
            }
            Value::Map(Rc::new(map))
        }
        Datum::Tagged(_, inner) => quote_value(inner),
        Datum::Comment(..) => Value::Nil,
    }
}

/// Convert a runtime value back into syntax, for macro results.
pub fn value_to_syntax(value: &Value, span: Span) -> Syntax {
    use korben_syntax::reader::Datum;
    if let Some(syntax) = as_syntax(value) {
        return (*syntax).clone();
    }
    match value {
        Value::Nil => Syntax::new(Datum::Nil, span),
        Value::Bool(value) => Syntax::new(Datum::Bool(*value), span),
        Value::Int(value) => Syntax::new(Datum::Int(*value), span),
        Value::Float(value) => Syntax::new(Datum::Float(*value), span),
        Value::Str(value) => Syntax::new(Datum::Str((**value).clone()), span),
        Value::Keyword(name) => Syntax::new(Datum::Keyword(name.to_string()), span),
        Value::Symbol(name) => Syntax::new(Datum::Symbol(name.to_string()), span),
        Value::Vector(items) => Syntax::new(
            Datum::List(items.iter().map(|item| value_to_syntax(item, span)).collect()),
            span,
        ),
        other => Syntax::new(Datum::Str(other.to_string()), span),
    }
}

/// Levenshtein distance, capped for short identifiers.
pub fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0usize; right.len() + 1];
    for (i, left_char) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, right_char) in right.iter().enumerate() {
            let cost = usize::from(left_char != right_char);
            current[j + 1] = (previous[j + 1] + 1).min(current[j] + 1).min(previous[j] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}
