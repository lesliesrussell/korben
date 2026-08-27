//! Ownership, moves, and borrowing.
//!
//! Specification section 12 makes native resources safe without imposing
//! lifetime syntax on ordinary code. The key rule is in 12.2: only
//! *resource-bearing* values move. Immutable, freely shareable data — strings,
//! vectors, maps, plain records — is never move-checked, so ordinary programs
//! see no ownership diagnostics at all.
//!
//! A type is resource-bearing when it owns something that must be released: it
//! implements `Drop`, it is written `Owned T`, it is a native handle such as
//! `File`, or it contains one of those.
//!
//! The analysis is flow-sensitive. Branches are analyzed from a common state
//! and joined afterwards, so a value moved on one path but not another is
//! reported as *may have been moved* rather than being missed or over-reported.

// korben-6v7

use crate::ast::*;
use crate::project::Session;
use korben_syntax::diag::{Diagnostic, Diagnostics};
use korben_syntax::span::Span;
use std::collections::{HashMap, HashSet};

/// Where a value sits in the ownership model of specification 12.1.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    /// Bitwise-copyable: booleans and fixed-width numbers.
    Copy,
    /// Immutable and safely shareable; never moves.
    Value,
    /// Uniquely owned; moving transfers responsibility for release.
    Owned,
    /// An immutable scoped reference.
    Borrow,
    /// An exclusive mutable scoped reference.
    BorrowMut,
    /// Explicit reference-counted sharing.
    Shared,
}

impl Category {
    fn moves(self) -> bool {
        self == Category::Owned
    }

    fn is_borrow(self) -> bool {
        matches!(self, Category::Borrow | Category::BorrowMut)
    }

    fn describe(self) -> &'static str {
        match self {
            Category::Copy => "a copyable value",
            Category::Value => "an immutable value",
            Category::Owned => "an owned resource",
            Category::Borrow => "a borrow",
            Category::BorrowMut => "an exclusive borrow",
            Category::Shared => "a shared value",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Live,
    /// Moved on every path reaching here.
    Moved(Span),
    /// Moved on at least one path.
    MaybeMoved(Span),
}

/// How an expression position uses a value.
#[derive(Clone, Copy, PartialEq)]
enum Use {
    /// Takes ownership: assignment, argument passing, return.
    Consume,
    /// Reads without taking ownership.
    Borrowed,
}

struct BindingInfo {
    name: String,
    /// Where the binding was introduced.
    span: Span,
    category: Category,
    /// Source-level type name, for diagnostics.
    type_name: String,
    /// True for function parameters, which outlive the body. Kept so that a
    /// future lifetime analysis can tell a caller's storage from a local's.
    #[allow(dead_code)]
    is_param: bool,
    /// True for a `with` binding, whose resource is released at scope exit.
    scoped: bool,
    /// Scope depth, so a loop can tell inner bindings from outer ones.
    depth: usize,
}

struct Signature {
    params: Vec<(Category, Option<String>)>,
    ret: Option<TypeExpr>,
    is_unsafe: bool,
    span: Span,
}

/// Analyze every loaded module and return the ownership diagnostics.
pub fn check_session(session: &Session) -> Diagnostics {
    let mut tables = Tables::default();
    tables.collect(&session.modules);
    let mut checker = Own {
        tables,
        diagnostics: Diagnostics::new(),
        bindings: Vec::new(),
        states: Vec::new(),
        scopes: Vec::new(),
        depth: 0,
        in_unsafe: false,
        fn_is_unsafe: false,
        fn_name: String::new(),
        in_tail: false,
        task_boundary: None,
    };
    for module in &session.modules {
        for item in &module.items {
            match item {
                Item::Fn(decl) => checker.function(decl),
                Item::Impl(decl) => {
                    for method in &decl.methods {
                        checker.function(method);
                    }
                }
                Item::Test(decl) => checker.test(decl),
                _ => {}
            }
        }
    }
    checker.diagnostics
}

// -------------------------------------------------------------------- tables

#[derive(Default)]
struct Tables {
    /// Types that own a resource and therefore move.
    resources: HashSet<String>,
    /// Record and enum field types, for propagating resource-ness.
    fields: HashMap<String, Vec<TypeExpr>>,
    signatures: HashMap<String, Signature>,
}

impl Tables {
    fn collect(&mut self, modules: &[Module]) {
        for name in korben_runtime::std::RESOURCE_TYPES {
            self.resources.insert(name.to_string());
        }
        for module in modules {
            for item in &module.items {
                match item {
                    Item::Type(decl) => {
                        let mut types = Vec::new();
                        match &decl.body {
                            TypeBody::Record(fields) => {
                                types.extend(fields.iter().map(|(_, ty, _)| ty.clone()));
                            }
                            TypeBody::Enum(variants) => {
                                for variant in variants {
                                    types
                                        .extend(variant.fields.iter().map(|(_, ty, _)| ty.clone()));
                                }
                            }
                            TypeBody::Newtype(inner) | TypeBody::Alias(inner) => {
                                types.push(inner.clone())
                            }
                        }
                        self.fields.insert(decl.name.clone(), types);
                    }
                    // Implementing `Drop` is how a type declares it owns something.
                    Item::Impl(decl) if decl.protocol == "Drop" => {
                        self.resources.insert(decl.type_name.clone());
                    }
                    _ => {}
                }
            }
        }
        // A type containing a resource is itself resource-bearing.
        loop {
            let mut grew = false;
            let names: Vec<String> = self.fields.keys().cloned().collect();
            for name in names {
                if self.resources.contains(&name) {
                    continue;
                }
                let owns = self.fields[&name].clone().iter().any(|ty| self.mentions_resource(ty));
                if owns {
                    self.resources.insert(name);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }

        for module in modules {
            for item in &module.items {
                let (name, decl) = match item {
                    Item::Fn(decl) => (decl.name.clone(), decl),
                    _ => continue,
                };
                self.signatures.insert(
                    name,
                    Signature {
                        params: decl
                            .params
                            .iter()
                            .map(|param| {
                                (
                                    param
                                        .ty
                                        .as_ref()
                                        .map(|ty| self.category(ty))
                                        .unwrap_or(Category::Value),
                                    param.keyword.clone(),
                                )
                            })
                            .collect(),
                        ret: decl.ret.clone(),
                        is_unsafe: decl.is_unsafe,
                        span: decl.span,
                    },
                );
            }
        }
    }

    fn mentions_resource(&self, ty: &TypeExpr) -> bool {
        match ty {
            TypeExpr::Name(name, args, _) => {
                let short = short_name(name);
                // A borrow of a resource does not itself own one.
                if matches!(short, "Borrow" | "BorrowMut") {
                    return false;
                }
                self.resources.contains(short) || args.iter().any(|arg| self.mentions_resource(arg))
            }
            TypeExpr::Record(fields, _) => fields.iter().any(|(_, ty)| self.mentions_resource(ty)),
            TypeExpr::Tuple(items, _) => items.iter().any(|ty| self.mentions_resource(ty)),
            // A function value does not own its argument or result types.
            TypeExpr::Fn(..) => false,
        }
    }

    /// The ownership category a declared type puts a value in.
    fn category(&self, ty: &TypeExpr) -> Category {
        let TypeExpr::Name(name, args, _) = ty else {
            return if self.mentions_resource(ty) { Category::Owned } else { Category::Value };
        };
        match short_name(name) {
            "Borrow" => Category::Borrow,
            "BorrowMut" => Category::BorrowMut,
            "Owned" => Category::Owned,
            "Shared" | "Rc" | "Arc" => Category::Shared,
            "Bool" | "Char" | "Unit" | "Keyword" | "Int" | "Int8" | "Int16" | "Int32" | "Int64"
            | "Int128" | "UInt" | "UInt8" | "UInt16" | "UInt32" | "UInt64" | "UInt128"
            | "Float32" | "Float64" => Category::Copy,
            other => {
                if self.resources.contains(other)
                    || args.iter().any(|arg| self.mentions_resource(arg))
                {
                    Category::Owned
                } else {
                    Category::Value
                }
            }
        }
    }

    fn type_name(&self, ty: &TypeExpr) -> String {
        crate::docs::render_type(ty)
    }
}

fn short_name(name: &str) -> &str {
    name.rsplit(['.', '/']).next().unwrap_or(name)
}

// ------------------------------------------------------------------- checker

struct Own {
    tables: Tables,
    diagnostics: Diagnostics,
    bindings: Vec<BindingInfo>,
    states: Vec<State>,
    scopes: Vec<Vec<(String, usize)>>,
    depth: usize,
    in_unsafe: bool,
    fn_is_unsafe: bool,
    fn_name: String,
    /// True while visiting an expression whose value leaves the function.
    in_tail: bool,
    /// Set inside a task scope: the depth outside it, and the scope's span.
    task_boundary: Option<(usize, Span)>,
}

impl Own {
    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
        self.depth += 1;
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.depth -= 1;
    }

    /// Branch analysis restores earlier state snapshots, which can be shorter
    /// than the binding table if the branch introduced names. Every access
    /// grows the table first so the two stay aligned.
    fn sync(&mut self) {
        if self.states.len() < self.bindings.len() {
            self.states.resize(self.bindings.len(), State::Live);
        }
    }

    fn state_of(&mut self, id: usize) -> State {
        self.sync();
        self.states[id]
    }

    fn set_state(&mut self, id: usize, state: State) {
        self.sync();
        self.states[id] = state;
    }

    fn bind(
        &mut self,
        name: &str,
        span: Span,
        category: Category,
        type_name: String,
        is_param: bool,
    ) -> usize {
        self.sync();
        let id = self.bindings.len();
        self.bindings.push(BindingInfo {
            name: name.to_string(),
            span,
            category,
            type_name,
            is_param,
            scoped: false,
            depth: self.depth,
        });
        self.states.push(State::Live);
        if let Some(scope) = self.scopes.last_mut() {
            scope.push((name.to_string(), id));
        }
        id
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        for scope in self.scopes.iter().rev() {
            if let Some((_, id)) = scope.iter().rev().find(|(existing, _)| existing == name) {
                return Some(*id);
            }
        }
        None
    }

    fn function(&mut self, decl: &FnDecl) {
        self.push_scope();
        let outer_unsafe = std::mem::replace(&mut self.fn_is_unsafe, decl.is_unsafe);
        let outer_name = std::mem::replace(&mut self.fn_name, decl.name.clone());

        for param in &decl.params {
            let (category, type_name) = match &param.ty {
                Some(ty) => (self.tables.category(ty), self.tables.type_name(ty)),
                None => (Category::Value, "_".to_string()),
            };
            self.bind(&param.name, param.span, category, type_name, true);
        }
        self.body(&decl.body, true);

        self.fn_name = outer_name;
        self.fn_is_unsafe = outer_unsafe;
        self.pop_scope();
    }

    fn test(&mut self, decl: &TestDecl) {
        self.push_scope();
        for (name, generator) in &decl.generators {
            self.expr(generator, Use::Consume);
            self.bind(name, decl.span, Category::Value, "_".to_string(), false);
        }
        self.body(&decl.body, false);
        self.pop_scope();
    }

    /// Walk a block. `is_tail` marks a body whose final expression is returned.
    fn body(&mut self, body: &Body, is_tail: bool) {
        self.push_scope();
        for (index, stmt) in body.stmts.iter().enumerate() {
            let last = index + 1 == body.stmts.len();
            match stmt {
                Stmt::Let { pattern, ty, value, span } => {
                    self.expr(value, Use::Consume);
                    let (category, type_name) = match ty {
                        Some(ty) => (self.tables.category(ty), self.tables.type_name(ty)),
                        None => self.infer_category(value),
                    };
                    self.bind_pattern(pattern, category, &type_name, *span);
                }
                Stmt::Var { name, ty, value, span } => {
                    self.expr(value, Use::Consume);
                    let (category, type_name) = match ty {
                        Some(ty) => (self.tables.category(ty), self.tables.type_name(ty)),
                        None => self.infer_category(value),
                    };
                    self.bind(name, *span, category, type_name, false);
                }
                Stmt::Defer { body, .. } => self.body(body, false),
                Stmt::Expr(expr) => {
                    // Tail position is where a value escapes the function, so
                    // it is tracked through the forms that can hold a tail.
                    self.in_tail = last && is_tail;
                    self.expr(expr, Use::Consume);
                    self.in_tail = false;
                }
            }
        }
        self.pop_scope();
    }

    fn bind_pattern(&mut self, pattern: &Pattern, category: Category, type_name: &str, span: Span) {
        let mut names = Vec::new();
        pattern.bindings(&mut names);
        for (name, name_span) in names {
            // Destructuring a resource distributes ownership to its parts.
            let span = if name_span.is_synthetic() { span } else { name_span };
            self.bind(&name, span, category, type_name.to_string(), false);
        }
    }

    /// The category a value expression produces, when it can be determined
    /// from declared types. Anything else is treated as an immutable value.
    fn infer_category(&self, expr: &Expr) -> (Category, String) {
        match expr {
            Expr::Var(name, _) => match self.lookup(name) {
                Some(id) => (self.bindings[id].category, self.bindings[id].type_name.clone()),
                None => (Category::Value, "_".to_string()),
            },
            // `?` unwraps a Result or Option, keeping the payload's ownership.
            Expr::Propagate(inner, _) => match &**inner {
                Expr::Call { callee, .. } => self.call_result(callee),
                other => self.infer_category(other),
            },
            Expr::Call { callee, .. } => self.call_result(callee),
            Expr::Record { type_name: Some(name), .. } => self.named_category(name),
            Expr::Do(body, _) => match body.stmts.last() {
                Some(Stmt::Expr(expr)) => self.infer_category(expr),
                _ => (Category::Value, "_".to_string()),
            },
            _ => (Category::Value, "_".to_string()),
        }
    }

    /// What a call to `callee` yields, from the callee's declared return type.
    fn call_result(&self, callee: &Expr) -> (Category, String) {
        let name = match callee {
            Expr::Var(name, _) => name.clone(),
            Expr::Path { name, .. } => name.clone(),
            Expr::Field { name, .. } => name.clone(),
            _ => return (Category::Value, "_".to_string()),
        };
        // A constructor yields its own type.
        if let Some(result) = self.named_category_opt(&name) {
            return result;
        }
        match self.tables.signatures.get(&name).and_then(|sig| sig.ret.as_ref()) {
            Some(ty) => (self.tables.category(ty), self.tables.type_name(ty)),
            // `fs.open` and friends are declared by the runtime, not in source.
            None => match native_result(&name) {
                Some(type_name) => (Category::Owned, type_name.to_string()),
                None => (Category::Value, "_".to_string()),
            },
        }
    }

    fn named_category(&self, name: &str) -> (Category, String) {
        self.named_category_opt(name).unwrap_or((Category::Value, name.to_string()))
    }

    fn named_category_opt(&self, name: &str) -> Option<(Category, String)> {
        if !self.tables.fields.contains_key(name) {
            return None;
        }
        let category =
            if self.tables.resources.contains(name) { Category::Owned } else { Category::Value };
        Some((category, name.to_string()))
    }

    // ---------------------------------------------------------- expressions

    fn expr(&mut self, expr: &Expr, mode: Use) {
        // Tail position propagates only through the forms whose value is the
        // enclosing form's value.
        let tail = std::mem::take(&mut self.in_tail);
        match expr {
            Expr::Var(name, span) => {
                if tail {
                    self.check_return(expr);
                }
                self.use_binding(name, *span, mode)
            }
            Expr::Call { callee, args, span } => self.call(callee, args, *span),
            Expr::Field { target, .. } => self.expr(target, Use::Borrowed),
            Expr::Propagate(inner, _) => self.expr(inner, mode),
            Expr::Throw(inner, _) | Expr::Await(inner, _) => self.expr(inner, Use::Consume),
            Expr::Vector(items, _) | Expr::Set(items, _) | Expr::Recur(items, _) => {
                for item in items {
                    self.expr(item, Use::Consume);
                }
            }
            Expr::And(items, _) | Expr::Or(items, _) => {
                // Short-circuiting: later operands may not run, so a move in one
                // is only a possible move.
                let before = self.states.clone();
                for item in items {
                    self.expr(item, Use::Consume);
                }
                let after = std::mem::replace(&mut self.states, before);
                self.join(after);
            }
            Expr::Map(entries, _) => {
                for (key, value) in entries {
                    self.expr(key, Use::Consume);
                    self.expr(value, Use::Consume);
                }
            }
            Expr::Record { fields, .. } => {
                for (_, value, _) in fields {
                    self.expr(value, Use::Consume);
                }
            }
            Expr::Interp(parts, _) => {
                for part in parts {
                    if let InterpPart::Expr(expr) = part {
                        self.expr(expr, Use::Borrowed);
                    }
                }
            }
            Expr::If { cond, then, els, .. } => {
                self.expr(cond, Use::Borrowed);
                let before = self.states.clone();
                self.in_tail = tail;
                self.expr(then, mode);
                let then_states = std::mem::replace(&mut self.states, before);
                if let Some(els) = els {
                    self.in_tail = tail;
                    self.expr(els, mode);
                }
                self.join(then_states);
            }
            Expr::Match { scrutinee, arms, .. } => {
                self.expr(scrutinee, Use::Borrowed);
                let (category, type_name) = self.infer_category(scrutinee);
                let before = self.states.clone();
                let mut joined: Option<Vec<State>> = None;
                for arm in arms {
                    self.states = before.clone();
                    self.push_scope();
                    self.bind_pattern(&arm.pattern, category, &type_name, arm.span);
                    if let Some(guard) = &arm.guard {
                        self.expr(guard, Use::Borrowed);
                    }
                    self.in_tail = tail;
                    self.expr(&arm.body, mode);
                    self.pop_scope();
                    joined = Some(match joined {
                        Some(previous) => merge(previous, &self.states),
                        None => self.states.clone(),
                    });
                }
                self.states = joined.unwrap_or(before);
            }
            Expr::Loop { bindings, body, span } => {
                for (_, value) in bindings {
                    self.expr(value, Use::Consume);
                }
                self.push_scope();
                let outer_depth = self.depth;
                for (name, value) in bindings {
                    let (category, type_name) = self.infer_category(value);
                    self.bind(name, *span, category, type_name, false);
                }
                let before = self.states.clone();
                self.body(body, false);
                self.report_loop_moves(&before, outer_depth, *span);
                self.pop_scope();
            }
            Expr::Do(body, _) => self.body(body, tail),
            Expr::Unsafe(body, _) => {
                let outer = std::mem::replace(&mut self.in_unsafe, true);
                self.body(body, false);
                self.in_unsafe = outer;
            }
            Expr::TaskScope { body, span, .. } => {
                // Specification 12.3: a borrow may not cross a task boundary.
                let boundary = self.depth;
                self.push_scope();
                let outer = self.task_boundary.replace((boundary, *span));
                self.body(body, false);
                self.task_boundary = outer;
                self.pop_scope();
            }
            Expr::Assign { name, value, span } => {
                self.expr(value, Use::Consume);
                // Overwriting a moved binding makes it live again.
                if let Some(id) = self.lookup(name) {
                    self.set_state(id, State::Live);
                }
                let _ = span;
            }
            Expr::Try { body, catches, finally, .. } => {
                let before = self.states.clone();
                self.body(body, false);
                let body_states = std::mem::replace(&mut self.states, before);
                for arm in catches {
                    self.push_scope();
                    self.bind(&arm.binding, arm.span, Category::Value, "_".to_string(), false);
                    self.body(&arm.body, false);
                    self.pop_scope();
                }
                self.join(body_states);
                if let Some(finally) = finally {
                    self.body(finally, false);
                }
            }
            Expr::With { name, value, body, span } => {
                self.expr(value, Use::Consume);
                let (category, type_name) = self.infer_category(value);
                self.push_scope();
                // The resource is released when the scope exits, so the body
                // only borrows it.
                let id = self.bind(name, *span, Category::Borrow, type_name.clone(), false);
                self.bindings[id].scoped = true;
                let _ = category;
                self.body(body, tail);
                self.pop_scope();
            }
            Expr::Lambda(decl, _) => {
                self.push_scope();
                for param in &decl.params {
                    let (category, type_name) = match &param.ty {
                        Some(ty) => (self.tables.category(ty), self.tables.type_name(ty)),
                        None => (Category::Value, "_".to_string()),
                    };
                    self.bind(&param.name, param.span, category, type_name, true);
                }
                self.body(&decl.body, false);
                self.pop_scope();
            }
            _ => {}
        }
    }

    fn use_binding(&mut self, name: &str, span: Span, mode: Use) {
        let Some(id) = self.lookup(name) else { return };
        if !self.bindings[id].category.moves() && !self.bindings[id].category.is_borrow() {
            return;
        }
        if let Some((boundary, scope_span)) = self.task_boundary {
            let binding = &self.bindings[id];
            if binding.category.is_borrow() && binding.depth <= boundary {
                let name = binding.name.clone();
                let definition = binding.span;
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "`{name}` is a borrow and cannot cross a task boundary"
                    ))
                    .with_code("borrow-across-task")
                    .at(span, "used inside a task scope")
                    .secondary(definition, "borrowed outside the scope here")
                    .secondary(scope_span, "this task scope may outlive the borrow")
                    .note("specification 12.3: borrows may not cross async task boundaries")
                    .help(
                        "pass an owned value into the scope instead of borrowing one from outside",
                    ),
                );
            }
        }
        match self.state_of(id) {
            State::Moved(origin) => self.report_use_after_move(id, span, origin, true),
            State::MaybeMoved(origin) => self.report_use_after_move(id, span, origin, false),
            State::Live => {
                if mode == Use::Consume && self.bindings[id].category.moves() {
                    self.set_state(id, State::Moved(span));
                }
            }
        }
    }

    fn call(&mut self, callee: &Expr, args: &[Arg], span: Span) {
        // `clone` reads its argument and yields a fresh value.
        if let Expr::Var(name, _) = callee {
            if name == "clone" {
                if let Some(arg) = args.first() {
                    let (category, type_name) = self.infer_category(&arg.value);
                    if category == Category::Owned {
                        self.diagnostics.push(
                            Diagnostic::error(format!("`{type_name}` cannot be cloned"))
                                .with_code("clone-resource")
                                .at(arg.span, "this value owns a resource")
                                .note("cloning would duplicate the handle and release it twice")
                                .help("take it by `Borrow` instead, or read the data you need out of it"),
                        );
                    }
                    self.expr(&arg.value, Use::Borrowed);
                }
                for arg in args.iter().skip(1) {
                    self.expr(&arg.value, Use::Consume);
                }
                return;
            }
        }

        let signature = match callee {
            Expr::Var(name, _) => self
                .tables
                .signatures
                .get(name)
                .map(|sig| (name.clone(), sig.params.clone(), sig.is_unsafe, sig.span)),
            _ => None,
        };

        if let Some((name, _, is_unsafe, definition)) = &signature {
            if *is_unsafe && !self.in_unsafe && !self.fn_is_unsafe {
                self.diagnostics.push(
                    Diagnostic::error(format!("`{name}` is unsafe and cannot be called here"))
                        .with_code("unsafe-call")
                        .at(span, "called from safe code")
                        .secondary(*definition, "declared `unsafe fn` here")
                        .note("specification 12.7: unsafe operations are lexically contained")
                        .help(format!(
                            "wrap this in `(unsafe ...)`, or declare `{}` as `(unsafe fn ...)`",
                            self.fn_name
                        )),
                );
            }
        }

        self.expr(callee, Use::Borrowed);

        let modes: Vec<Use> = match &signature {
            Some((_, params, _, _)) => positional_modes(params, args),
            None => args.iter().map(|_| Use::Consume).collect(),
        };
        if let Some((name, params, _, _)) = &signature {
            self.check_exclusive(name, params, args, span);
        }
        for (arg, mode) in args.iter().zip(modes) {
            self.expr(&arg.value, mode);
        }
    }

    /// An exclusive borrow may not alias another argument in the same call.
    fn check_exclusive(
        &mut self,
        name: &str,
        params: &[(Category, Option<String>)],
        args: &[Arg],
        _span: Span,
    ) {
        let mut exclusive: Vec<(String, Span)> = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            let Some((category, _)) = params.get(index) else { continue };
            if *category != Category::BorrowMut {
                continue;
            }
            if let Expr::Var(binding, binding_span) = &arg.value {
                exclusive.push((binding.clone(), *binding_span));
            }
        }
        for (binding, first) in exclusive {
            let aliases = args
                .iter()
                .enumerate()
                .filter(|(index, arg)| {
                    matches!(&arg.value, Expr::Var(other, other_span)
                        if other == &binding && *other_span != first)
                        || matches!(&arg.value, Expr::Var(other, _)
                            if other == &binding
                                && params.get(*index).map(|(c, _)| *c) != Some(Category::BorrowMut))
                })
                .map(|(_, arg)| arg.span)
                .collect::<Vec<_>>();
            if let Some(other) = aliases.first() {
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "`{binding}` is borrowed exclusively and cannot also be passed here"
                    ))
                    .with_code("exclusive-borrow")
                    .at(*other, "second use in the same call")
                    .secondary(first, "exclusive borrow taken here")
                    .note(format!("`{name}` takes this parameter as `BorrowMut`"))
                    .help("an exclusive borrow rules out any other access for its duration"),
                );
            }
        }
    }

    /// A resource released at scope exit may not escape that scope.
    ///
    /// Confusing a borrow with owned data is a type error, and inference
    /// reports it. What only ownership can see is a value whose lifetime ends
    /// when its scope does, which is exactly what `with` introduces.
    fn check_return(&mut self, expr: &Expr) {
        let Expr::Var(name, span) = expr else { return };
        let Some(id) = self.lookup(name) else { return };
        let binding = &self.bindings[id];
        if !binding.scoped {
            return;
        }
        let definition = binding.span;
        let type_name = binding.type_name.clone();
        self.diagnostics.push(
            Diagnostic::error(format!("`{name}` does not live long enough to be returned"))
                .with_code("borrow-escape")
                .at(*span, "returned here")
                .secondary(definition, format!("`{type_name}` is released when this scope exits"))
                .note("specification 12.3: a borrow cannot outlive its owner")
                .help("return the data you need from the resource, not the resource itself"),
        );
    }

    fn report_use_after_move(&mut self, id: usize, span: Span, origin: Span, certain: bool) {
        let binding = &self.bindings[id];
        let name = binding.name.clone();
        let type_name = binding.type_name.clone();
        let category = binding.category;
        let definition = binding.span;
        let message = if certain {
            format!("`{name}` was moved and cannot be used again")
        } else {
            format!("`{name}` may have been moved already")
        };
        let label =
            if certain { "used after the move" } else { "used on a path where it was moved" };
        let mut diagnostic = Diagnostic::error(message)
            .with_code(if certain { "use-after-move" } else { "maybe-moved" })
            .at(span, label)
            .secondary(origin, "moved here")
            .secondary(
                definition,
                format!("`{name}` is {} of type `{type_name}`", category.describe()),
            );
        diagnostic = if certain {
            diagnostic
                .note("an owned resource is released exactly once, so moving it transfers responsibility")
                .help("pass it by `Borrow`, or restructure so the value is used before it moves")
        } else {
            diagnostic
                .note("one branch moves the value and another does not")
                .help("move it on every path, or on none")
        };
        self.diagnostics.push(diagnostic);
    }

    /// A move inside a loop consumes the value on the first iteration only.
    fn report_loop_moves(&mut self, before: &[State], outer_depth: usize, span: Span) {
        self.sync();
        let mut reports = Vec::new();
        for (id, state) in self.states.iter().enumerate() {
            if self.bindings[id].depth >= outer_depth {
                continue;
            }
            if before.get(id) != Some(&State::Live) {
                continue;
            }
            if let State::Moved(origin) | State::MaybeMoved(origin) = state {
                reports.push((id, *origin));
            }
        }
        for (id, origin) in reports {
            let binding = &self.bindings[id];
            let name = binding.name.clone();
            let type_name = binding.type_name.clone();
            self.diagnostics.push(
                Diagnostic::error(format!("`{name}` is moved inside a loop"))
                    .with_code("move-in-loop")
                    .at(origin, "moved here, on every iteration")
                    .secondary(span, "this loop can run more than once")
                    .note(format!("`{type_name}` owns a resource, so it can only be moved once"))
                    .help("move the value out of the loop, or take it by `Borrow` inside"),
            );
            // Report once per binding.
            self.set_state(id, State::Live);
        }
    }

    /// Merge the current states with an alternative branch's.
    fn join(&mut self, other: Vec<State>) {
        self.sync();
        self.states = merge(std::mem::take(&mut self.states), &other);
    }
}

/// Join two branch states: moved on both paths stays moved, moved on one
/// becomes *may have been moved*.
fn merge(left: Vec<State>, right: &[State]) -> Vec<State> {
    let length = left.len().max(right.len());
    (0..length)
        .map(|id| {
            let a = left.get(id).copied().unwrap_or(State::Live);
            let b = right.get(id).copied().unwrap_or(State::Live);
            match (a, b) {
                (State::Live, State::Live) => State::Live,
                (State::Moved(origin), State::Moved(_)) => State::Moved(origin),
                (State::Moved(origin), _) | (_, State::Moved(origin)) => State::MaybeMoved(origin),
                (State::MaybeMoved(origin), _) | (_, State::MaybeMoved(origin)) => {
                    State::MaybeMoved(origin)
                }
            }
        })
        .collect()
}

/// Match call arguments to declared parameters, and say how each is used.
fn positional_modes(params: &[(Category, Option<String>)], args: &[Arg]) -> Vec<Use> {
    let positional: Vec<Category> =
        params.iter().filter(|(_, keyword)| keyword.is_none()).map(|(c, _)| *c).collect();
    let mut index = 0usize;
    args.iter()
        .map(|arg| {
            let category = match &arg.keyword {
                Some(keyword) => params
                    .iter()
                    .find(|(_, name)| name.as_deref() == Some(keyword.as_str()))
                    .map(|(category, _)| *category),
                None => {
                    let category = positional.get(index).copied();
                    index += 1;
                    category
                }
            };
            match category {
                Some(category) if category.is_borrow() => Use::Borrowed,
                _ => Use::Consume,
            }
        })
        .collect()
}

/// Runtime functions that hand back an owned resource.
fn native_result(name: &str) -> Option<&'static str> {
    match name {
        "open" | "create" => Some("File"),
        _ => None,
    }
}
