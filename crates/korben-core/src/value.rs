//! Values and environments.
//!
//! The value representation, the standard library, and call dispatch live in
//! `korben-runtime`, which generated native code links against too. That is
//! what makes the two execution modes share observable semantics by
//! construction rather than by convention. This module re-exports them and adds
//! the pieces only the interpreter needs: lexical environments and the runtime
//! module table.

// korben-vtx

use korben_syntax::span::Span;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub use korben_runtime::apply::{apply, bind_args, construct};
pub use korben_runtime::loc::{Fault, Loc};
pub use korben_runtime::value::{
    display, member, member_names, Arg, Body, Caller, Flow, Foreign, Function, MapValue, Outcome,
    Param, RecordValue, Sym, Value, VariantValue,
};

pub type EvalResult = Outcome;

/// Convert a front-end span into a runtime location. The layouts match.
pub fn loc_of(span: Span) -> Loc {
    if span.is_synthetic() {
        return Loc::NONE;
    }
    Loc::new(span.file, span.start, span.end)
}

/// Convert a runtime location back into a front-end span.
pub fn span_of(loc: Loc) -> Span {
    if loc.is_none() {
        return Span::synthetic();
    }
    Span::new(loc.file, loc.start, loc.end)
}

/// An interpreter closure: a declaration plus the environment it captured.
pub struct Closure {
    pub decl: Rc<crate::ast::FnDecl>,
    pub env: Env,
    /// Module this closure was defined in, for global lookup.
    pub module: Sym,
}

/// Wrap an interpreter closure as a callable value.
pub fn closure_value(closure: Rc<Closure>) -> Value {
    let params = closure
        .decl
        .params
        .iter()
        .map(|param| Param {
            name: Rc::from(param.name.as_str()),
            keyword: param.keyword.as_deref().map(Rc::from),
            has_default: param.default.is_some(),
        })
        .collect();
    let is_async = closure.decl.is_async;
    Value::Fn(Rc::new(Function {
        name: closure.decl.name.clone(),
        params,
        body: Body::Host(Box::new(closure)),
        is_async,
    }))
}

/// Recover the interpreter closure behind a value, if it is one.
pub fn as_closure(value: &Value) -> Option<Rc<Closure>> {
    let Value::Fn(function) = value else { return None };
    let Body::Host(payload) = &function.body else { return None };
    payload.downcast_ref::<Rc<Closure>>().cloned()
}

/// A syntax object travelling as a value, which is how macros see their input.
pub fn syntax_value(syntax: Rc<korben_syntax::Syntax>) -> Value {
    Foreign::wrap("Syntax", syntax)
}

pub fn as_syntax(value: &Value) -> Option<Rc<korben_syntax::Syntax>> {
    let Value::Foreign(foreign) = value else { return None };
    foreign.downcast::<Rc<korben_syntax::Syntax>>().cloned()
}

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
