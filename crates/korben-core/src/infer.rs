//! Type, effect, and exhaustiveness analysis.
//!
//! Inference is Hindley–Milner with let-polymorphism over the types in
//! [`crate::types`]. Where the checker cannot reach a sound conclusion it
//! produces [`Type::Unknown`], which unifies with anything: the goal for v0.1
//! is that every reported error is a real one.

// korben-6bc

use crate::ast::*;
use crate::project::Session;
use crate::types::{FnType, RecordType, Scheme, Type, TypeVar};
use korben_syntax::diag::{Diagnostic, Diagnostics};
use korben_syntax::span::Span;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

/// Analyze every loaded module.
pub fn check_session(session: &mut Session, strict_api: bool) {
    let mut checker = Checker::new(strict_api);
    // Nominal declarations across all modules are visible to the checker; the
    // module system already rejected anything that is not actually in scope.
    for module in &session.modules {
        checker.collect_types(module);
    }
    for module in &session.modules {
        checker.collect_signatures(module);
    }
    for module in &session.modules {
        checker.check_module(module);
    }
    session.diagnostics.extend(checker.diagnostics);
    // Ownership runs after inference so its diagnostics come second, and so a
    // program with type errors is not also buried in ownership noise.
    if !session.diagnostics.has_errors() {
        let ownership = crate::own::check_session(session);
        session.diagnostics.extend(ownership);
    }
}

/// Infer the type of a single expression against the session's declarations.
/// Used by the REPL's `:type` command and by editor hover.
pub fn type_of(session: &Session, expr: &Expr) -> String {
    let mut checker = Checker::new(false);
    for module in &session.modules {
        checker.collect_types(module);
    }
    for module in &session.modules {
        checker.collect_signatures(module);
    }
    let mut scope = Scope::new();
    let ty = checker.infer(expr, &mut scope);
    checker.resolve(&ty).to_string()
}

/// Lint rules from specification section 20.6 that do not need full inference.
pub fn lint_session(session: &Session) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    for module in &session.modules {
        for item in &module.items {
            match item {
                Item::Fn(decl) => lint_fn(decl, &mut diagnostics),
                Item::Impl(decl) => {
                    for method in &decl.methods {
                        lint_fn(method, &mut diagnostics);
                    }
                }
                Item::Test(decl) => {
                    let mut used = HashSet::new();
                    collect_used_in_body(&decl.body, &mut used);
                }
                _ => {}
            }
            if item.is_public() {
                if let Item::Fn(decl) = item {
                    if decl.doc.is_none() {
                        diagnostics.push(
                            Diagnostic::warning(format!(
                                "public function `{}` has no documentation",
                                decl.name
                            ))
                            .with_code("missing-docs")
                            .at(decl.span, "add a `;;;` documentation comment"),
                        );
                    }
                }
            }
        }
    }
    diagnostics
}

fn lint_fn(decl: &FnDecl, diagnostics: &mut Diagnostics) {
    let mut used = HashSet::new();
    collect_used_in_body(&decl.body, &mut used);
    for param in &decl.params {
        if param.name.starts_with('_') || param.name == "self" {
            continue;
        }
        if !used.contains(&param.name) {
            diagnostics.push(
                Diagnostic::warning(format!("unused parameter `{}`", param.name))
                    .with_code("unused-binding")
                    .at(param.span, "this parameter is never read")
                    .help(format!("rename it to `_{}` to silence this", param.name)),
            );
        }
    }
    // A `let` whose name is never read afterwards is usually a mistake.
    for (index, stmt) in decl.body.stmts.iter().enumerate() {
        let Stmt::Let { pattern, span, .. } = stmt else { continue };
        let mut names = Vec::new();
        pattern.bindings(&mut names);
        let mut later = HashSet::new();
        for stmt in &decl.body.stmts[index + 1..] {
            collect_used_in_stmt(stmt, &mut later);
        }
        for (name, _) in names {
            if name.starts_with('_') || later.contains(&name) {
                continue;
            }
            diagnostics.push(
                Diagnostic::warning(format!("unused binding `{name}`"))
                    .with_code("unused-binding")
                    .at(*span, "this binding is never read")
                    .help(format!("rename it to `_{name}` to silence this")),
            );
        }
    }
    if decl.is_unsafe {
        diagnostics.push(
            Diagnostic::warning(format!("`{}` is an unsafe function", decl.name))
                .with_code("unsafe-boundary")
                .at(decl.span, "callers must opt in with `unsafe`")
                .note("unsafe boundaries are surfaced in generated documentation"),
        );
    }
}

// ------------------------------------------------------------------ checker

struct FieldTable {
    fields: BTreeMap<String, Type>,
}

struct EnumTable {
    /// Variant name to its field list, in declaration order.
    variants: Vec<(String, Vec<(String, Type)>)>,
}

struct Checker {
    subst: Vec<Option<Type>>,
    next_var: TypeVar,
    diagnostics: Diagnostics,
    strict_api: bool,
    /// Nominal record types.
    records: HashMap<String, FieldTable>,
    /// Nominal enum types.
    enums: HashMap<String, EnumTable>,
    /// Variant constructor to owning enum.
    variant_owner: HashMap<String, String>,
    /// Type parameters declared on each nominal type.
    type_params: HashMap<String, Vec<String>>,
    /// Global value bindings across all loaded modules.
    globals: HashMap<String, Scheme>,
    /// Effects accumulated while checking the current function body.
    effects: Effects,
    /// Known type names, for `unknown type` diagnostics.
    known_types: HashSet<String>,
}

impl Checker {
    fn new(strict_api: bool) -> Checker {
        let mut checker = Checker {
            subst: Vec::new(),
            next_var: 0,
            diagnostics: Diagnostics::new(),
            strict_api,
            records: HashMap::new(),
            enums: HashMap::new(),
            variant_owner: HashMap::new(),
            type_params: HashMap::new(),
            globals: HashMap::new(),
            effects: Effects::NONE,
            known_types: builtin_type_names(),
        };
        checker.install_builtin_enums();
        checker.install_prelude_signatures();
        checker
    }

    fn fresh(&mut self) -> Type {
        let var = self.next_var;
        self.next_var += 1;
        self.subst.push(None);
        Type::Var(var)
    }

    /// Follow substitutions to the representative of a type.
    fn prune(&self, ty: &Type) -> Type {
        match ty {
            Type::Var(var) => match self.subst.get(*var as usize).and_then(|slot| slot.clone()) {
                Some(bound) => self.prune(&bound),
                None => ty.clone(),
            },
            _ => ty.clone(),
        }
    }

    /// Fully resolve a type for display.
    fn resolve(&self, ty: &Type) -> Type {
        match self.prune(ty) {
            Type::Con(name, args) => {
                Type::Con(name, args.iter().map(|arg| self.resolve(arg)).collect())
            }
            Type::Tuple(items) => {
                Type::Tuple(items.iter().map(|item| self.resolve(item)).collect())
            }
            Type::Fn(function) => Type::Fn(Rc::new(FnType {
                params: function.params.iter().map(|param| self.resolve(param)).collect(),
                ret: self.resolve(&function.ret),
                effects: function.effects,
                variadic: function.variadic,
            })),
            Type::Record(record) => Type::Record(Rc::new(RecordType {
                name: record.name.clone(),
                fields: record
                    .fields
                    .iter()
                    .map(|(name, ty)| (name.clone(), self.resolve(ty)))
                    .collect(),
            })),
            other => other,
        }
    }

    fn occurs(&self, var: TypeVar, ty: &Type) -> bool {
        match self.prune(ty) {
            Type::Var(other) => other == var,
            Type::Con(_, args) | Type::Tuple(args) => args.iter().any(|arg| self.occurs(var, arg)),
            Type::Fn(function) => {
                function.params.iter().any(|param| self.occurs(var, param))
                    || self.occurs(var, &function.ret)
            }
            Type::Record(record) => record.fields.values().any(|ty| self.occurs(var, ty)),
            Type::Unknown => false,
        }
    }

    fn unify(&mut self, left: &Type, right: &Type) -> Result<(), (Type, Type)> {
        let left = self.prune(left);
        let right = self.prune(right);
        match (&left, &right) {
            // `Unknown` is the checker's admission that it does not know.
            (Type::Unknown, _) | (_, Type::Unknown) => Ok(()),
            // `Never` is the type of an expression that does not return, so it
            // is compatible with every branch it appears in.
            (Type::Con(name, _), _) | (_, Type::Con(name, _)) if &**name == "Never" => Ok(()),
            (Type::Var(var), _) => {
                if self.occurs(*var, &right) {
                    return Err((left.clone(), right.clone()));
                }
                self.subst[*var as usize] = Some(right.clone());
                Ok(())
            }
            (_, Type::Var(_)) => self.unify(&right, &left),
            (Type::Con(left_name, left_args), Type::Con(right_name, right_args)) => {
                if left_name != right_name || left_args.len() != right_args.len() {
                    // Numeric literals adapt to the width they are used at.
                    if is_numeric(left_name) && is_numeric(right_name) {
                        return Ok(());
                    }
                    return Err((left.clone(), right.clone()));
                }
                for (left_arg, right_arg) in left_args.iter().zip(right_args.iter()) {
                    self.unify(left_arg, right_arg)?;
                }
                Ok(())
            }
            (Type::Tuple(left_items), Type::Tuple(right_items)) => {
                if left_items.len() != right_items.len() {
                    return Err((left.clone(), right.clone()));
                }
                for (left_item, right_item) in left_items.iter().zip(right_items.iter()) {
                    self.unify(left_item, right_item)?;
                }
                Ok(())
            }
            (Type::Fn(left_fn), Type::Fn(right_fn)) => {
                if left_fn.variadic || right_fn.variadic {
                    return Ok(());
                }
                if left_fn.params.len() != right_fn.params.len() {
                    return Err((left.clone(), right.clone()));
                }
                for (left_param, right_param) in left_fn.params.iter().zip(right_fn.params.iter()) {
                    self.unify(left_param, right_param)?;
                }
                self.unify(&left_fn.ret, &right_fn.ret)
            }
            (Type::Record(left_record), Type::Record(right_record)) => {
                for (name, left_field) in &left_record.fields {
                    match right_record.fields.get(name) {
                        Some(right_field) => self.unify(left_field, right_field)?,
                        None => return Err((left.clone(), right.clone())),
                    }
                }
                for name in right_record.fields.keys() {
                    if !left_record.fields.contains_key(name) {
                        return Err((left.clone(), right.clone()));
                    }
                }
                Ok(())
            }
            // A structural record unifies with the nominal type it matches.
            (Type::Record(record), Type::Con(name, _))
            | (Type::Con(name, _), Type::Record(record)) => {
                let Some(table) = self.records.get(&name.to_string()) else {
                    return Err((left.clone(), right.clone()));
                };
                let expected: BTreeMap<String, Type> = table.fields.clone();
                for (field, ty) in &record.fields {
                    match expected.get(field) {
                        Some(other) => {
                            let other = other.clone();
                            self.unify(ty, &other)?;
                        }
                        None => return Err((left.clone(), right.clone())),
                    }
                }
                for field in expected.keys() {
                    if !record.fields.contains_key(field) {
                        return Err((left.clone(), right.clone()));
                    }
                }
                Ok(())
            }
            _ => Err((left.clone(), right.clone())),
        }
    }

    /// Unify and report a mismatch against `span`.
    ///
    /// A borrow is taken implicitly: specification 12.3 says the compiler
    /// infers short lexical borrows in ordinary calls, so a `T` satisfies a
    /// `Borrow T` parameter. The reverse is not true, which is what keeps a
    /// borrow from escaping as owned data.
    fn expect(&mut self, expected: &Type, actual: &Type, span: Span, context: &str) {
        if let Some(inner) = borrowed_type(&self.prune(expected)) {
            let snapshot = self.subst.clone();
            if self.unify(&inner, actual).is_ok() {
                return;
            }
            self.subst = snapshot;
        }
        if let Err((left, right)) = self.unify(expected, actual) {
            let expected = self.resolve(&left);
            let actual = self.resolve(&right);
            self.diagnostics.push(
                Diagnostic::error(format!("type mismatch {context}"))
                    .with_code("type-mismatch")
                    .at(span, format!("expected `{expected}`, found `{actual}`"))
                    .note(format!("expected: {expected}"))
                    .note(format!("   found: {actual}")),
            );
        }
    }

    // ---------------------------------------------------------- declarations

    fn install_builtin_enums(&mut self) {
        let value = Type::Var(u32::MAX); // placeholder replaced below
        let _ = value;
        self.enums.insert(
            "Option".to_string(),
            EnumTable {
                variants: vec![
                    ("Some".to_string(), vec![("value".to_string(), Type::Unknown)]),
                    ("None".to_string(), Vec::new()),
                ],
            },
        );
        self.enums.insert(
            "Result".to_string(),
            EnumTable {
                variants: vec![
                    ("Ok".to_string(), vec![("value".to_string(), Type::Unknown)]),
                    ("Err".to_string(), vec![("error".to_string(), Type::Unknown)]),
                ],
            },
        );
        for (variant, owner) in
            [("Some", "Option"), ("None", "Option"), ("Ok", "Result"), ("Err", "Result")]
        {
            self.variant_owner.insert(variant.to_string(), owner.to_string());
        }
        self.type_params.insert("Option".to_string(), vec!["T".to_string()]);
        self.type_params.insert("Result".to_string(), vec!["T".to_string(), "E".to_string()]);
    }

    /// Signatures for the handful of prelude functions worth checking precisely.
    fn install_prelude_signatures(&mut self) {
        let mut define = |name: &str, scheme: Scheme| {
            self.globals.insert(name.to_string(), scheme);
        };
        let number = Type::con("Int");
        // Arithmetic folds over any number of operands.
        for name in ["+", "-", "*", "/"] {
            define(
                name,
                Scheme::mono(Type::variadic(
                    vec![number.clone(), number.clone()],
                    number.clone(),
                    Effects::NONE,
                )),
            );
        }
        define(
            "mod",
            Scheme::mono(Type::function(
                vec![number.clone(), number.clone()],
                number.clone(),
                Effects::NONE,
            )),
        );
        for name in ["inc", "dec"] {
            define(
                name,
                Scheme::mono(Type::function(vec![number.clone()], number.clone(), Effects::NONE)),
            );
        }
        // Comparison chains: `(< 1 2 3)` reads as `1 < 2 < 3`.
        for name in ["<", "<=", ">", ">=", "=", "not="] {
            define(
                name,
                Scheme {
                    vars: vec![0],
                    ty: Type::variadic(
                        vec![Type::Var(0), Type::Var(0)],
                        Type::bool(),
                        Effects::NONE,
                    ),
                },
            );
        }
        define(
            "not",
            Scheme::mono(Type::function(vec![Type::bool()], Type::bool(), Effects::NONE)),
        );
        define(
            "len",
            Scheme {
                vars: vec![0],
                ty: Type::function(vec![Type::Var(0)], Type::int(), Effects::NONE),
            },
        );
        define(
            "empty?",
            Scheme {
                vars: vec![0],
                ty: Type::function(vec![Type::Var(0)], Type::bool(), Effects::NONE),
            },
        );
        define(
            "map",
            Scheme {
                vars: vec![0, 1],
                ty: Type::function(
                    vec![
                        Type::vec(Type::Var(0)),
                        Type::function(vec![Type::Var(0)], Type::Var(1), Effects::NONE),
                    ],
                    Type::vec(Type::Var(1)),
                    Effects::NONE,
                ),
            },
        );
        define(
            "filter",
            Scheme {
                vars: vec![0],
                ty: Type::function(
                    vec![
                        Type::vec(Type::Var(0)),
                        Type::function(vec![Type::Var(0)], Type::bool(), Effects::NONE),
                    ],
                    Type::vec(Type::Var(0)),
                    Effects::NONE,
                ),
            },
        );
        define(
            "reduce",
            Scheme {
                vars: vec![0, 1],
                ty: Type::function(
                    vec![
                        Type::vec(Type::Var(0)),
                        Type::Var(1),
                        Type::function(
                            vec![Type::Var(1), Type::Var(0)],
                            Type::Var(1),
                            Effects::NONE,
                        ),
                    ],
                    Type::Var(1),
                    Effects::NONE,
                ),
            },
        );
        define(
            "first",
            Scheme {
                vars: vec![0],
                ty: Type::function(
                    vec![Type::vec(Type::Var(0))],
                    Type::option(Type::Var(0)),
                    Effects::NONE,
                ),
            },
        );
        define(
            "rest",
            Scheme {
                vars: vec![0],
                ty: Type::function(
                    vec![Type::vec(Type::Var(0))],
                    Type::vec(Type::Var(0)),
                    Effects::NONE,
                ),
            },
        );
        define(
            "Some",
            Scheme {
                vars: vec![0],
                ty: Type::function(vec![Type::Var(0)], Type::option(Type::Var(0)), Effects::NONE),
            },
        );
        define(
            "Ok",
            Scheme {
                vars: vec![0, 1],
                ty: Type::function(
                    vec![Type::Var(0)],
                    Type::result(Type::Var(0), Type::Var(1)),
                    Effects::NONE,
                ),
            },
        );
        define(
            "Err",
            Scheme {
                vars: vec![0, 1],
                ty: Type::function(
                    vec![Type::Var(1)],
                    Type::result(Type::Var(0), Type::Var(1)),
                    Effects::NONE,
                ),
            },
        );
        define("None", Scheme { vars: vec![0], ty: Type::option(Type::Var(0)) });
        for name in ["println", "print", "eprintln"] {
            define(
                name,
                Scheme {
                    vars: vec![0],
                    ty: Type::variadic(vec![Type::Var(0)], Type::unit(), Effects(EFFECT_IO)),
                },
            );
        }
        define(
            "str",
            Scheme {
                vars: vec![0],
                ty: Type::variadic(vec![Type::Var(0)], Type::string(), Effects::NONE),
            },
        );
        define(
            "assert",
            Scheme::mono(Type::variadic(vec![Type::bool()], Type::unit(), Effects::NONE)),
        );
        define(
            "assert-eq",
            Scheme {
                vars: vec![0],
                ty: Type::function(vec![Type::Var(0), Type::Var(0)], Type::unit(), Effects::NONE),
            },
        );
        define(
            "assert-ne",
            Scheme {
                vars: vec![0],
                ty: Type::function(vec![Type::Var(0), Type::Var(0)], Type::unit(), Effects::NONE),
            },
        );
        // Reserve the variables the schemes above refer to.
        self.next_var = 2;
        self.subst = vec![None, None];
    }

    fn collect_types(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Type(decl) = item else { continue };
            self.known_types.insert(decl.name.clone());
            self.type_params.insert(decl.name.clone(), decl.params.clone());
            match &decl.body {
                TypeBody::Record(fields) => {
                    let mut table = BTreeMap::new();
                    for (name, ty, _) in fields {
                        table.insert(name.clone(), self.lower_type(ty, &decl.params));
                    }
                    self.records.insert(decl.name.clone(), FieldTable { fields: table });
                }
                TypeBody::Enum(variants) => {
                    let mut entries = Vec::new();
                    for variant in variants {
                        let fields: Vec<(String, Type)> = variant
                            .fields
                            .iter()
                            .map(|(name, ty, _)| (name.clone(), self.lower_type(ty, &decl.params)))
                            .collect();
                        self.variant_owner.insert(variant.name.clone(), decl.name.clone());
                        entries.push((variant.name.clone(), fields));
                    }
                    self.enums.insert(decl.name.clone(), EnumTable { variants: entries });
                }
                TypeBody::Newtype(inner) => {
                    let mut table = BTreeMap::new();
                    table.insert("value".to_string(), self.lower_type(inner, &decl.params));
                    self.records.insert(decl.name.clone(), FieldTable { fields: table });
                }
                TypeBody::Alias(_) => {}
            }
        }
    }

    fn collect_signatures(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Fn(decl) => {
                    let scheme = self.signature_of(decl);
                    self.globals.insert(decl.name.clone(), scheme);
                }
                Item::Type(decl) => self.register_constructors(decl),
                Item::Protocol(decl) => {
                    for method in &decl.methods {
                        let params: Vec<Type> =
                            method.params.iter().map(|_| Type::Unknown).collect();
                        let ret = method
                            .ret
                            .as_ref()
                            .map(|ty| self.lower_type(ty, &[]))
                            .unwrap_or(Type::Unknown);
                        self.globals.insert(
                            method.name.clone(),
                            Scheme::mono(Type::function(params, ret, method.effects)),
                        );
                    }
                }
                Item::Const { name, ty, .. } => {
                    let ty =
                        ty.as_ref().map(|ty| self.lower_type(ty, &[])).unwrap_or(Type::Unknown);
                    self.globals.insert(name.clone(), Scheme::mono(ty));
                }
                _ => {}
            }
        }
    }

    fn register_constructors(&mut self, decl: &Rc<TypeDecl>) {
        let params = decl.params.clone();
        let result =
            Type::Con(Rc::from(decl.name.as_str()), params.iter().map(|_| Type::Unknown).collect());
        match &decl.body {
            TypeBody::Record(fields) => {
                let param_types: Vec<Type> =
                    fields.iter().map(|(_, ty, _)| self.lower_type(ty, &params)).collect();
                self.globals.insert(
                    decl.name.clone(),
                    Scheme::mono(Type::function(param_types, result, Effects::NONE)),
                );
            }
            TypeBody::Newtype(inner) => {
                let inner = self.lower_type(inner, &params);
                self.globals.insert(
                    decl.name.clone(),
                    Scheme::mono(Type::function(vec![inner], result, Effects::NONE)),
                );
            }
            TypeBody::Enum(variants) => {
                for variant in variants {
                    let param_types: Vec<Type> = variant
                        .fields
                        .iter()
                        .map(|(_, ty, _)| self.lower_type(ty, &params))
                        .collect();
                    let scheme = if param_types.is_empty() {
                        Scheme::mono(result.clone())
                    } else {
                        Scheme::mono(Type::function(param_types, result.clone(), Effects::NONE))
                    };
                    self.globals.insert(variant.name.clone(), scheme);
                }
            }
            TypeBody::Alias(_) => {}
        }
    }

    fn signature_of(&mut self, decl: &FnDecl) -> Scheme {
        // Keyword parameters are matched by name at the call site, so only the
        // positional ones take part in the function's arity.
        let params: Vec<Type> = decl
            .params
            .iter()
            .filter(|param| param.keyword.is_none())
            .map(|param| match &param.ty {
                Some(ty) => self.lower_type(ty, &[]),
                None => Type::Unknown,
            })
            .collect();
        let ret = match &decl.ret {
            Some(ty) => self.lower_type(ty, &[]),
            None => Type::Unknown,
        };
        Scheme::mono(Type::function(params, ret, decl.declared_effects))
    }

    /// Translate surface type syntax into an inference type.
    fn lower_type(&mut self, ty: &TypeExpr, params: &[String]) -> Type {
        match ty {
            TypeExpr::Name(name, args, span) => {
                if params.iter().any(|param| param == name) {
                    // Type parameters are opaque during checking.
                    return Type::Unknown;
                }
                let args: Vec<Type> = args.iter().map(|arg| self.lower_type(arg, params)).collect();
                let short = name.rsplit(['.', '/']).next().unwrap_or(name).to_string();
                if !self.known_types.contains(&short)
                    && !self.records.contains_key(&short)
                    && !self.enums.contains_key(&short)
                {
                    // Unknown names are reported once, then treated as opaque.
                    self.diagnostics.push(
                        Diagnostic::warning(format!("unknown type `{name}`"))
                            .with_code("unknown-type")
                            .at(*span, "no type with this name is declared")
                            .help(
                                "declare it with `(type ...)` or import the module that defines it",
                            ),
                    );
                    self.known_types.insert(short);
                    return Type::Unknown;
                }
                Type::Con(Rc::from(short.as_str()), args)
            }
            TypeExpr::Record(fields, _) => {
                let mut table = BTreeMap::new();
                for (name, ty) in fields {
                    table.insert(name.clone(), self.lower_type(ty, params));
                }
                Type::record(table, None)
            }
            TypeExpr::Tuple(items, _) => {
                Type::Tuple(items.iter().map(|item| self.lower_type(item, params)).collect())
            }
            TypeExpr::Fn(args, ret, effects, _) => Type::function(
                args.iter().map(|arg| self.lower_type(arg, params)).collect(),
                self.lower_type(ret, params),
                *effects,
            ),
        }
    }

    // -------------------------------------------------------------- checking

    fn check_module(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Fn(decl) => self.check_fn(decl),
                Item::Impl(decl) => {
                    for method in &decl.methods {
                        self.check_fn(method);
                    }
                }
                Item::Test(decl) => {
                    let mut scope = Scope::new();
                    for (name, generator) in &decl.generators {
                        let ty = self.infer(generator, &mut scope);
                        // A generator is either a value or a nullary function.
                        let element = match self.prune(&ty) {
                            Type::Fn(function) => function.ret.clone(),
                            other => other,
                        };
                        scope.define(name.clone(), Scheme::mono(element));
                    }
                    self.effects = Effects::NONE;
                    self.infer_body(&decl.body, &mut scope);
                }
                Item::Const { value, ty, span, .. } => {
                    let mut scope = Scope::new();
                    let actual = self.infer(value, &mut scope);
                    if let Some(ty) = ty {
                        let expected = self.lower_type(ty, &[]);
                        self.expect(&expected, &actual, *span, "in constant initializer");
                    }
                }
                _ => {}
            }
        }
    }

    fn check_fn(&mut self, decl: &FnDecl) {
        let mut scope = Scope::new();
        for param in &decl.params {
            let ty = match &param.ty {
                Some(ty) => self.lower_type(ty, &[]),
                None => self.fresh(),
            };
            if let Some(default) = &param.default {
                let actual = self.infer(default, &mut scope);
                self.expect(&ty, &actual, param.span, "in default argument");
            }
            scope.define(param.name.clone(), Scheme::mono(ty));
        }

        let outer_effects = std::mem::replace(&mut self.effects, Effects::NONE);
        let actual = self.infer_body(&decl.body, &mut scope);

        if let Some(ret) = &decl.ret {
            let expected = self.lower_type(ret, &[]);
            // A function returning Unit may end in any expression.
            if !matches!(&expected, Type::Con(name, _) if &**name == "Unit") {
                let span = decl.body.stmts.last().map(Stmt::span).unwrap_or(decl.span);
                self.expect(&expected, &actual, span, &format!("in the result of `{}`", decl.name));
            }
        }

        let inferred = self.effects;
        let declared = if decl.is_async {
            decl.declared_effects.union(Effects(EFFECT_ASYNC))
        } else {
            decl.declared_effects
        };
        let missing = declared.missing(inferred);
        if !missing.is_empty() && (!decl.declared_effects.is_empty() || decl.is_public) {
            self.diagnostics.push(
                Diagnostic::error(format!("`{}` performs undeclared effects", decl.name))
                    .with_code("undeclared-effect")
                    .at(decl.span, format!("add {} to the signature", missing.render()))
                    .note(format!("inferred effects: {}", inferred.render()))
                    .note(format!(
                        "declared effects: {}",
                        if declared.is_empty() { "none".to_string() } else { declared.render() }
                    )),
            );
        }
        if self.strict_api && decl.is_public {
            if decl.ret.is_none() {
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "public function `{}` has no return type",
                        decl.name
                    ))
                    .with_code("strict-api")
                    .at(decl.span, "annotate the return type")
                    .help("`--strict-api` requires complete types on exported functions"),
                );
            }
            for param in &decl.params {
                if param.ty.is_none() {
                    self.diagnostics.push(
                        Diagnostic::error(format!(
                            "public function `{}` has an unannotated parameter `{}`",
                            decl.name, param.name
                        ))
                        .with_code("strict-api")
                        .at(param.span, "annotate this parameter"),
                    );
                }
            }
        }
        self.effects = outer_effects;
    }

    fn infer_body(&mut self, body: &Body, scope: &mut Scope) -> Type {
        scope.push();
        let mut result = Type::unit();
        for stmt in &body.stmts {
            match stmt {
                Stmt::Let { pattern, ty, value, span } => {
                    let actual = self.infer(value, scope);
                    let bound = match ty {
                        Some(ty) => {
                            let expected = self.lower_type(ty, &[]);
                            self.expect(&expected, &actual, *span, "in a `let` binding");
                            expected
                        }
                        None => actual,
                    };
                    self.bind_pattern(pattern, &bound, scope);
                    result = Type::unit();
                }
                Stmt::Var { name, ty, value, span } => {
                    let actual = self.infer(value, scope);
                    let bound = match ty {
                        Some(ty) => {
                            let expected = self.lower_type(ty, &[]);
                            self.expect(&expected, &actual, *span, "in a `var` binding");
                            expected
                        }
                        None => actual,
                    };
                    scope.define(name.clone(), Scheme::mono(bound));
                    result = Type::unit();
                }
                Stmt::Defer { body, .. } => {
                    self.infer_body(body, scope);
                    result = Type::unit();
                }
                Stmt::Expr(expr) => result = self.infer(expr, scope),
            }
        }
        scope.pop();
        result
    }

    fn infer(&mut self, expr: &Expr, scope: &mut Scope) -> Type {
        match expr {
            Expr::Nil(_) => Type::unit(),
            Expr::Bool(_, _) => Type::bool(),
            Expr::Int(_, _) => Type::int(),
            Expr::Float(_, _) => Type::float(),
            Expr::Str(_, _) | Expr::Interp(_, _) => {
                if let Expr::Interp(parts, _) = expr {
                    for part in parts {
                        if let InterpPart::Expr(expr) = part {
                            self.infer(expr, scope);
                        }
                    }
                }
                Type::string()
            }
            Expr::Keyword(_, _) => Type::keyword(),
            Expr::Var(name, span) => self.lookup(name, *span, scope),
            // Cross-module references are checked by the module system, not here.
            Expr::Path { .. } => Type::Unknown,
            Expr::Vector(items, _) => {
                // A literal whose elements agree is a `Vec`; one whose elements
                // differ is a fixed-length tuple, per specification 9.5.
                let types: Vec<Type> = items.iter().map(|item| self.infer(item, scope)).collect();
                let element = self.fresh();
                let snapshot = self.subst.clone();
                let uniform = types.iter().all(|actual| self.unify(&element, actual).is_ok());
                if uniform {
                    Type::vec(element)
                } else {
                    self.subst = snapshot;
                    Type::Tuple(types)
                }
            }
            Expr::Set(items, _) => {
                let element = self.fresh();
                for item in items {
                    let actual = self.infer(item, scope);
                    self.expect(&element, &actual, item.span(), "in a set literal");
                }
                Type::app("Set", vec![element])
            }
            Expr::Map(entries, _) => {
                // Map literals are commonly heterogeneous, so a mismatch widens
                // the entry type rather than being reported as an error.
                let mut keys = Vec::new();
                let mut values = Vec::new();
                for (key_expr, value_expr) in entries {
                    keys.push(self.infer(key_expr, scope));
                    values.push(self.infer(value_expr, scope));
                }
                Type::app("Map", vec![self.join(keys), self.join(values)])
            }
            Expr::Record { type_name, fields, .. } => {
                let mut table = BTreeMap::new();
                for (name, value, _) in fields {
                    table.insert(name.clone(), self.infer(value, scope));
                }
                Type::record(table, type_name.as_deref().map(Rc::from))
            }
            Expr::If { cond, then, els, span } => {
                // Conditions use Korben truthiness — only `false` and `nil` are
                // falsey — so any type is accepted here.
                self.infer(cond, scope);
                let then_type = self.infer(then, scope);
                match els {
                    Some(els) => {
                        let else_type = self.infer(els, scope);
                        // A literal `nil` branch is the absent-value placeholder
                        // that control-flow macros expand into; it constrains
                        // nothing, so the other branch decides the type.
                        if matches!(**els, Expr::Nil(_)) {
                            return then_type;
                        }
                        if matches!(**then, Expr::Nil(_)) {
                            return else_type;
                        }
                        self.expect(&then_type, &else_type, *span, "between `if` branches");
                        then_type
                    }
                    // A one-armed `if` yields Unit because it may not run.
                    None => Type::unit(),
                }
            }
            // Truthiness combinators may yield operands of different types.
            Expr::And(operands, _) | Expr::Or(operands, _) => {
                let types: Vec<Type> =
                    operands.iter().map(|operand| self.infer(operand, scope)).collect();
                self.join(types)
            }
            Expr::Do(body, _) => self.infer_body(body, scope),
            Expr::Lambda(decl, _) => {
                scope.push();
                let mut params = Vec::new();
                for param in &decl.params {
                    let ty = match &param.ty {
                        Some(ty) => self.lower_type(ty, &[]),
                        None => self.fresh(),
                    };
                    scope.define(param.name.clone(), Scheme::mono(ty.clone()));
                    params.push(ty);
                }
                let ret = self.infer_body(&decl.body, scope);
                scope.pop();
                if let Some(annotated) = &decl.ret {
                    let expected = self.lower_type(annotated, &[]);
                    self.expect(&expected, &ret, decl.span, "in a lambda result");
                }
                Type::function(params, ret, decl.declared_effects)
            }
            Expr::Call { callee, args, span } => self.infer_call(callee, args, *span, scope),
            Expr::Field { target, name, span } => {
                let target_type = self.infer(target, scope);
                self.field_type(&target_type, name, *span)
            }
            Expr::Match { scrutinee, arms, span } => {
                let value = self.infer(scrutinee, scope);
                let result = self.fresh();
                for arm in arms {
                    scope.push();
                    self.bind_pattern(&arm.pattern, &value, scope);
                    if let Some(guard) = &arm.guard {
                        self.infer(guard, scope);
                    }
                    let arm_type = self.infer(&arm.body, scope);
                    self.expect(&result, &arm_type, arm.body.span(), "between match arms");
                    scope.pop();
                }
                self.check_exhaustive(&value, arms, *span);
                result
            }
            Expr::Loop { bindings, body, .. } => {
                scope.push();
                for (name, value) in bindings {
                    let ty = self.infer(value, scope);
                    scope.define(name.clone(), Scheme::mono(ty));
                }
                let result = self.infer_body(body, scope);
                scope.pop();
                result
            }
            Expr::Recur(args, _) => {
                for arg in args {
                    self.infer(arg, scope);
                }
                // `recur` never returns to its call site.
                Type::con("Never")
            }
            Expr::Assign { name, value, span } => {
                let expected = self.lookup(name, *span, scope);
                let actual = self.infer(value, scope);
                self.expect(&expected, &actual, *span, &format!("in `(set! {name} ...)`"));
                Type::unit()
            }
            Expr::Propagate(inner, span) => {
                let ty = self.infer(inner, scope);
                match self.prune(&ty) {
                    Type::Con(name, args) if &*name == "Result" && !args.is_empty() => {
                        args[0].clone()
                    }
                    Type::Con(name, args) if &*name == "Option" && !args.is_empty() => {
                        args[0].clone()
                    }
                    Type::Unknown | Type::Var(_) => Type::Unknown,
                    other => {
                        let other = self.resolve(&other);
                        self.diagnostics.push(
                            Diagnostic::error("`?` needs a Result or an Option")
                                .with_code("propagate-type")
                                .at(*span, format!("found `{other}`"))
                                .help("`?` propagates `Err` and `None` to the caller"),
                        );
                        Type::Unknown
                    }
                }
            }
            Expr::Throw(inner, _) => {
                self.infer(inner, scope);
                Type::con("Never")
            }
            Expr::Try { body, catches, finally, span } => {
                let result = self.infer_body(body, scope);
                for arm in catches {
                    scope.push();
                    scope.define(arm.binding.clone(), Scheme::mono(Type::Unknown));
                    let arm_type = self.infer_body(&arm.body, scope);
                    self.expect(&result, &arm_type, *span, "between `try` and `catch`");
                    scope.pop();
                }
                if let Some(finally) = finally {
                    self.infer_body(finally, scope);
                }
                result
            }
            Expr::With { name, value, body, .. } => {
                let resource = self.infer(value, scope);
                scope.push();
                scope.define(name.clone(), Scheme::mono(resource));
                let result = self.infer_body(body, scope);
                scope.pop();
                result
            }
            Expr::Unsafe(body, _) => {
                self.effects = self.effects.union(Effects(EFFECT_UNSAFE));
                self.infer_body(body, scope)
            }
            Expr::Await(inner, _) => {
                self.effects = self.effects.union(Effects(EFFECT_ASYNC));
                let ty = self.infer(inner, scope);
                match self.prune(&ty) {
                    Type::Con(name, args) if &*name == "Task" && !args.is_empty() => {
                        args[0].clone()
                    }
                    other => other,
                }
            }
            Expr::TaskScope { body, .. } => {
                self.effects = self.effects.union(Effects(EFFECT_ASYNC));
                self.infer_body(body, scope)
            }
            Expr::Quote(_, _) => Type::Unknown,
            Expr::SyntaxQuote(_, _) => Type::con("Syntax"),
        }
    }

    fn infer_call(&mut self, callee: &Expr, args: &[Arg], span: Span, scope: &mut Scope) -> Type {
        // `(User { id .. name .. })` constructs by field name rather than
        // position, so check it against the declaration directly.
        if let Expr::Var(name, _) = callee {
            if self.records.contains_key(name) && args.len() == 1 && args[0].keyword.is_none() {
                if let Expr::Record { fields, span: record_span, .. } = &args[0].value {
                    let expected = self.records[name].fields.clone();
                    let mut seen = HashSet::new();
                    for (field, value, field_span) in fields {
                        seen.insert(field.clone());
                        let actual = self.infer(value, scope);
                        match expected.get(field) {
                            Some(ty) => {
                                let ty = ty.clone();
                                self.expect(
                                    &ty,
                                    &actual,
                                    *field_span,
                                    &format!("in field `{field}`"),
                                );
                            }
                            None => self.diagnostics.push(
                                Diagnostic::error(format!("`{name}` has no field `{field}`"))
                                    .with_code("unknown-field")
                                    .at(*field_span, "unknown field")
                                    .help(format!(
                                        "fields: {}",
                                        expected.keys().cloned().collect::<Vec<_>>().join(", ")
                                    )),
                            ),
                        }
                    }
                    let missing: Vec<String> =
                        expected.keys().filter(|field| !seen.contains(*field)).cloned().collect();
                    if !missing.is_empty() {
                        self.diagnostics.push(
                            Diagnostic::error(format!(
                                "`{name}` is missing {} field(s)",
                                missing.len()
                            ))
                            .with_code("missing-field")
                            .at(*record_span, format!("not supplied: {}", missing.join(", "))),
                        );
                    }
                    return Type::Con(Rc::from(name.as_str()), Vec::new());
                }
            }
        }
        let callee_type = self.infer(callee, scope);
        let mut arg_types = Vec::with_capacity(args.len());
        for arg in args {
            arg_types.push((arg, self.infer(&arg.value, scope)));
        }
        match self.prune(&callee_type) {
            Type::Fn(function) => {
                self.effects = self.effects.union(function.effects);
                // Named arguments are matched by the callee's declaration, which
                // the type language does not carry; only check positional shape.
                let positional: Vec<_> =
                    arg_types.iter().filter(|(arg, _)| arg.keyword.is_none()).collect();
                let arity_ok = if function.variadic {
                    arg_types.len() >= function.params.len()
                } else {
                    function.params.len() == arg_types.len()
                };
                if positional.len() == arg_types.len() && !arity_ok {
                    self.diagnostics.push(
                        Diagnostic::error(format!(
                            "expected {} argument(s) but got {}",
                            function.params.len(),
                            arg_types.len()
                        ))
                        .with_code("arity")
                        .at(
                            span,
                            format!(
                                "this call passes {} argument(s){}",
                                arg_types.len(),
                                if function.variadic {
                                    " (at least that many are required)"
                                } else {
                                    ""
                                }
                            ),
                        )
                        .note(format!("callee type: {}", self.resolve(&callee_type))),
                    );
                    return function.ret.clone();
                }
                for (param, (arg, actual)) in function.params.iter().zip(arg_types.iter()) {
                    self.expect(param, actual, arg.span, "in an argument");
                }
                function.ret.clone()
            }
            Type::Unknown | Type::Var(_) => Type::Unknown,
            // `(None)` and `(InvalidCredentials)` apply a payload-free value to
            // no arguments, which yields the value itself.
            other if args.is_empty() => other,
            other => {
                let other = self.resolve(&other);
                self.diagnostics.push(
                    Diagnostic::error(format!("`{other}` is not callable"))
                        .with_code("not-callable")
                        .at(span, "this expression is used as a function"),
                );
                Type::Unknown
            }
        }
    }

    fn field_type(&mut self, target: &Type, name: &str, span: Span) -> Type {
        match self.prune(target) {
            Type::Record(record) => match record.fields.get(name) {
                Some(ty) => ty.clone(),
                None => {
                    if record.name.is_some() {
                        return self.nominal_field(&record.name.clone().unwrap(), name, span);
                    }
                    let available: Vec<&str> = record.fields.keys().map(String::as_str).collect();
                    self.diagnostics.push(
                        Diagnostic::error(format!("no field `{name}` on this record"))
                            .with_code("unknown-field")
                            .at(span, "unknown field")
                            .help(format!("available fields: {}", available.join(", "))),
                    );
                    Type::Unknown
                }
            },
            Type::Con(type_name, _) => self.nominal_field(&type_name, name, span),
            _ => Type::Unknown,
        }
    }

    fn nominal_field(&mut self, type_name: &str, name: &str, span: Span) -> Type {
        if let Some(table) = self.records.get(type_name) {
            if let Some(ty) = table.fields.get(name) {
                return ty.clone();
            }
            let available: Vec<&str> = table.fields.keys().map(String::as_str).collect();
            self.diagnostics.push(
                Diagnostic::error(format!("`{type_name}` has no field `{name}`"))
                    .with_code("unknown-field")
                    .at(span, "unknown field")
                    .help(format!("available fields: {}", available.join(", "))),
            );
            return Type::Unknown;
        }
        if let Some(table) = self.enums.get(type_name) {
            // A field access on an enum is only sound inside a match arm; the
            // pattern language is the supported way to reach payloads.
            let has_field = table
                .variants
                .iter()
                .any(|(_, fields)| fields.iter().any(|(field, _)| field == name));
            if !has_field {
                self.diagnostics.push(
                    Diagnostic::error(format!("no variant of `{type_name}` has a field `{name}`"))
                        .with_code("unknown-field")
                        .at(span, "unknown field")
                        .help("use `match` to destructure an enum"),
                );
            }
            return Type::Unknown;
        }
        Type::Unknown
    }

    fn lookup(&mut self, name: &str, span: Span, scope: &Scope) -> Type {
        if let Some(scheme) = scope.lookup(name) {
            return self.instantiate(&scheme);
        }
        if let Some(scheme) = self.globals.get(name).cloned() {
            return self.instantiate(&scheme);
        }
        // Unbound names are reported by the evaluator with better context; the
        // checker stays quiet so that one mistake is not reported twice.
        let _ = span;
        Type::Unknown
    }

    /// The common type of a group, or `Unknown` when they do not agree.
    fn join(&mut self, types: Vec<Type>) -> Type {
        let common = self.fresh();
        let snapshot = self.subst.clone();
        if types.iter().all(|actual| self.unify(&common, actual).is_ok()) {
            return common;
        }
        self.subst = snapshot;
        Type::Unknown
    }

    fn instantiate(&mut self, scheme: &Scheme) -> Type {
        if scheme.vars.is_empty() {
            return scheme.ty.clone();
        }
        let mut mapping = HashMap::new();
        for var in &scheme.vars {
            let fresh = self.fresh();
            mapping.insert(*var, fresh);
        }
        substitute(&scheme.ty, &mapping)
    }

    fn bind_pattern(&mut self, pattern: &Pattern, value: &Type, scope: &mut Scope) {
        match pattern {
            Pattern::Wildcard(_) => {}
            Pattern::Binding(name, _) => scope.define(name.clone(), Scheme::mono(value.clone())),
            Pattern::Nil(span) => self.expect(&Type::unit(), value, *span, "in a pattern"),
            Pattern::Bool(_, span) => self.expect(&Type::bool(), value, *span, "in a pattern"),
            Pattern::Int(_, span) => self.expect(&Type::int(), value, *span, "in a pattern"),
            Pattern::Float(_, span) => self.expect(&Type::float(), value, *span, "in a pattern"),
            Pattern::Str(_, span) => self.expect(&Type::string(), value, *span, "in a pattern"),
            Pattern::Keyword(_, span) => {
                self.expect(&Type::keyword(), value, *span, "in a pattern")
            }
            Pattern::Typed { inner, ty, span } => {
                let expected = self.lower_type(ty, &[]);
                self.expect(&expected, value, *span, "in a typed pattern");
                self.bind_pattern(inner, &expected, scope);
            }
            Pattern::Variant { name, positional, named, span } => {
                let Some(owner) = self.variant_owner.get(name).cloned() else {
                    // An unknown constructor is reported once, by the evaluator.
                    for sub in positional {
                        self.bind_pattern(sub, &Type::Unknown, scope);
                    }
                    for (_, sub) in named {
                        self.bind_pattern(sub, &Type::Unknown, scope);
                    }
                    return;
                };
                let fields = self
                    .enums
                    .get(&owner)
                    .and_then(|table| {
                        table.variants.iter().find(|(variant, _)| variant == name).cloned()
                    })
                    .map(|(_, fields)| fields)
                    .unwrap_or_default();
                if !positional.is_empty() && positional.len() != fields.len() && !fields.is_empty()
                {
                    self.diagnostics.push(
                        Diagnostic::error(format!(
                            "`{name}` binds {} field(s) but has {}",
                            positional.len(),
                            fields.len()
                        ))
                        .with_code("pattern-arity")
                        .at(*span, "wrong number of sub-patterns"),
                    );
                }
                for (index, sub) in positional.iter().enumerate() {
                    let ty = fields.get(index).map(|(_, ty)| ty.clone()).unwrap_or(Type::Unknown);
                    self.bind_pattern(sub, &ty, scope);
                }
                for (field, sub) in named {
                    let ty = fields
                        .iter()
                        .find(|(name, _)| name == field)
                        .map(|(_, ty)| ty.clone())
                        .unwrap_or(Type::Unknown);
                    self.bind_pattern(sub, &ty, scope);
                }
            }
            Pattern::Vector { items, rest, span } => {
                let element = self.fresh();
                self.expect(&Type::vec(element.clone()), value, *span, "in a vector pattern");
                for item in items {
                    self.bind_pattern(item, &element, scope);
                }
                if let Some(Some(name)) = rest {
                    scope.define(name.clone(), Scheme::mono(Type::vec(element)));
                }
            }
            Pattern::Map { entries, .. } | Pattern::Record { fields: entries, .. } => {
                for (key, sub) in entries {
                    let field = self.field_type_quiet(value, key);
                    self.bind_pattern(sub, &field, scope);
                }
            }
        }
    }

    /// Field lookup used by patterns, which must not complain about maps.
    fn field_type_quiet(&mut self, target: &Type, name: &str) -> Type {
        match self.prune(target) {
            Type::Record(record) => record.fields.get(name).cloned().unwrap_or(Type::Unknown),
            Type::Con(type_name, _) => self
                .records
                .get(&type_name.to_string())
                .and_then(|table| table.fields.get(name).cloned())
                .unwrap_or(Type::Unknown),
            _ => Type::Unknown,
        }
    }

    /// Report non-exhaustive matches and unreachable arms.
    fn check_exhaustive(&mut self, value: &Type, arms: &[MatchArm], span: Span) {
        for (index, arm) in arms.iter().enumerate() {
            if arm.guard.is_none() && arm.pattern.is_irrefutable() && index + 1 < arms.len() {
                self.diagnostics.push(
                    Diagnostic::warning("unreachable match arm")
                        .with_code("unreachable")
                        .at(arms[index + 1].span, "this arm can never be reached")
                        .secondary(arm.span, "because this pattern always matches"),
                );
                break;
            }
        }

        let has_catch_all =
            arms.iter().any(|arm| arm.guard.is_none() && arm.pattern.is_irrefutable());
        if has_catch_all {
            return;
        }
        let Type::Con(name, _) = self.prune(value) else { return };
        let Some(table) = self.enums.get(&name.to_string()) else { return };
        let declared: Vec<String> = table.variants.iter().map(|(name, _)| name.clone()).collect();
        let mut covered = HashSet::new();
        for arm in arms {
            if arm.guard.is_some() {
                continue;
            }
            if let Pattern::Variant { name, .. } = &arm.pattern {
                covered.insert(name.clone());
            }
        }
        let missing: Vec<String> =
            declared.into_iter().filter(|name| !covered.contains(name)).collect();
        if missing.is_empty() {
            return;
        }
        let suggestions: Vec<String> =
            missing.iter().map(|variant| format!("({variant} ...)")).collect();
        self.diagnostics.push(
            Diagnostic::error(format!("non-exhaustive match on `{name}`"))
                .with_code("non-exhaustive")
                .at(span, format!("missing {} case(s)", missing.len()))
                .note(format!("not covered: {}", missing.join(", ")))
                .help(format!("add {} or a `_` arm", suggestions.join(", "))),
        );
    }
}

fn substitute(ty: &Type, mapping: &HashMap<TypeVar, Type>) -> Type {
    match ty {
        Type::Var(var) => mapping.get(var).cloned().unwrap_or_else(|| ty.clone()),
        Type::Con(name, args) => {
            Type::Con(name.clone(), args.iter().map(|arg| substitute(arg, mapping)).collect())
        }
        Type::Tuple(items) => {
            Type::Tuple(items.iter().map(|item| substitute(item, mapping)).collect())
        }
        Type::Fn(function) => Type::Fn(Rc::new(FnType {
            params: function.params.iter().map(|param| substitute(param, mapping)).collect(),
            ret: substitute(&function.ret, mapping),
            effects: function.effects,
            variadic: function.variadic,
        })),
        Type::Record(record) => Type::Record(Rc::new(RecordType {
            name: record.name.clone(),
            fields: record
                .fields
                .iter()
                .map(|(name, ty)| (name.clone(), substitute(ty, mapping)))
                .collect(),
        })),
        Type::Unknown => Type::Unknown,
    }
}

/// The type behind `Borrow T` or `BorrowMut T`.
fn borrowed_type(ty: &Type) -> Option<Type> {
    match ty {
        Type::Con(name, args) if matches!(&**name, "Borrow" | "BorrowMut") => args.first().cloned(),
        _ => None,
    }
}

fn is_numeric(name: &str) -> bool {
    matches!(
        name,
        "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "Int128"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "UInt128"
            | "Float32"
            | "Float64"
    )
}

fn builtin_type_names() -> HashSet<String> {
    [
        "Bool",
        "Char",
        "String",
        "Bytes",
        "Unit",
        "Never",
        "Symbol",
        "Keyword",
        "Int",
        "Int8",
        "Int16",
        "Int32",
        "Int64",
        "Int128",
        "UInt",
        "UInt8",
        "UInt16",
        "UInt32",
        "UInt64",
        "UInt128",
        "Float32",
        "Float64",
        "Option",
        "Result",
        "Vec",
        "Map",
        "Set",
        "Box",
        "Rc",
        "Arc",
        "Weak",
        "Channel",
        "Task",
        "Stream",
        "Cell",
        "Uuid",
        "Date",
        "Duration",
        "Path",
        "Syntax",
        "IoError",
        "Fn",
        "File",
        // Ownership qualifiers from specification 12.1.
        "Owned",
        "Borrow",
        "BorrowMut",
        "Shared",
        "Managed",
        "Copy",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

// -------------------------------------------------------------------- scope

/// A stack of lexical scopes mapping names to type schemes.
struct Scope {
    frames: Vec<Vec<(String, Scheme)>>,
}

impl Scope {
    fn new() -> Scope {
        Scope { frames: vec![Vec::new()] }
    }

    fn push(&mut self) {
        self.frames.push(Vec::new());
    }

    fn pop(&mut self) {
        self.frames.pop();
    }

    fn define(&mut self, name: String, scheme: Scheme) {
        if let Some(frame) = self.frames.last_mut() {
            frame.retain(|(existing, _)| existing != &name);
            frame.push((name, scheme));
        }
    }

    fn lookup(&self, name: &str) -> Option<Scheme> {
        for frame in self.frames.iter().rev() {
            if let Some((_, scheme)) = frame.iter().rev().find(|(existing, _)| existing == name) {
                return Some(scheme.clone());
            }
        }
        None
    }
}

// ---------------------------------------------------------------- lint scans

fn collect_used_in_body(body: &Body, out: &mut HashSet<String>) {
    for stmt in &body.stmts {
        collect_used_in_stmt(stmt, out);
    }
}

fn collect_used_in_stmt(stmt: &Stmt, out: &mut HashSet<String>) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Var { value, .. } => collect_used(value, out),
        Stmt::Defer { body, .. } => collect_used_in_body(body, out),
        Stmt::Expr(expr) => collect_used(expr, out),
    }
}

fn collect_used(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Var(name, _) => {
            out.insert(name.clone());
        }
        Expr::Assign { name, value, .. } => {
            out.insert(name.clone());
            collect_used(value, out);
        }
        Expr::Interp(parts, _) => {
            for part in parts {
                if let InterpPart::Expr(expr) = part {
                    collect_used(expr, out);
                }
            }
        }
        Expr::And(items, _) | Expr::Or(items, _) => {
            for item in items {
                collect_used(item, out);
            }
        }
        Expr::Vector(items, _) | Expr::Set(items, _) | Expr::Recur(items, _) => {
            for item in items {
                collect_used(item, out);
            }
        }
        Expr::Map(entries, _) => {
            for (key, value) in entries {
                collect_used(key, out);
                collect_used(value, out);
            }
        }
        Expr::Record { fields, .. } => {
            for (_, value, _) in fields {
                collect_used(value, out);
            }
        }
        Expr::If { cond, then, els, .. } => {
            collect_used(cond, out);
            collect_used(then, out);
            if let Some(els) = els {
                collect_used(els, out);
            }
        }
        Expr::Do(body, _) | Expr::Unsafe(body, _) | Expr::TaskScope { body, .. } => {
            collect_used_in_body(body, out)
        }
        Expr::Lambda(decl, _) => collect_used_in_body(&decl.body, out),
        Expr::Call { callee, args, .. } => {
            collect_used(callee, out);
            for arg in args {
                collect_used(&arg.value, out);
            }
        }
        Expr::Field { target, .. } => collect_used(target, out),
        Expr::Match { scrutinee, arms, .. } => {
            collect_used(scrutinee, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_used(guard, out);
                }
                collect_used(&arm.body, out);
            }
        }
        Expr::Loop { bindings, body, .. } => {
            for (_, value) in bindings {
                collect_used(value, out);
            }
            collect_used_in_body(body, out);
        }
        Expr::Propagate(inner, _) | Expr::Throw(inner, _) | Expr::Await(inner, _) => {
            collect_used(inner, out)
        }
        Expr::Try { body, catches, finally, .. } => {
            collect_used_in_body(body, out);
            for arm in catches {
                collect_used_in_body(&arm.body, out);
            }
            if let Some(finally) = finally {
                collect_used_in_body(finally, out);
            }
        }
        Expr::With { value, body, .. } => {
            collect_used(value, out);
            collect_used_in_body(body, out);
        }
        _ => {}
    }
}
