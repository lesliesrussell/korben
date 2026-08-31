//! The typed core intermediate representation.
//!
//! Lowering resolves every name — local, module global, constructor, protocol
//! method, or runtime builtin — so that a backend never has to reproduce scope
//! rules. It also desugars the pieces that only matter to the front end:
//! string interpolation becomes concatenation, dotted paths become resolved
//! references, and module-qualified names become mangled symbols.
//!
//! Every node keeps its source span, which is what lets generated code carry
//! debug information back to the text the user wrote.

// korben-vtx

use crate::ast::{self, Effects};
use crate::eval::Interp;
use crate::project::Session;
use korben_syntax::span::Span;
use std::collections::HashSet;

/// A whole program, ready for a backend.
pub struct Program {
    pub modules: Vec<Module>,
    /// Mangled symbol of the entry function.
    pub entry: Option<String>,
    /// Runtime builtins the program actually references.
    pub builtins: Vec<String>,
    // korben-vok
    /// What the checker concluded about each expression, keyed by span.
    ///
    /// Every `Expr` here already carries the span it was lowered from, and the
    /// checker keys its conclusions the same way, so the two meet without
    /// annotating each node. An expression the compiler synthesised -- from a
    /// macro, or from desugaring -- has no entry, and `type_of` answers `None`
    /// for it rather than guessing.
    pub types: std::collections::HashMap<Span, crate::types::Type>,
}

impl Program {
    // korben-vok
    /// What the checker concluded about the expression at `span`.
    ///
    /// `None` means the checker reached no conclusion worth recording, or the
    /// expression was synthesised rather than written. A caller that wants to
    /// specialise on a type has to be prepared for that and stay general.
    pub fn type_of(&self, span: Span) -> Option<&crate::types::Type> {
        self.types.get(&span)
    }
}

pub struct Module {
    pub name: String,
    pub types: Vec<TypeDef>,
    pub functions: Vec<Function>,
    pub foreign: Vec<ForeignFn>,
    pub consts: Vec<Const>,
    pub impls: Vec<ImplDef>,
    pub protocols: Vec<ProtocolDef>,
}

pub struct TypeDef {
    pub name: String,
    /// Mangled constructor symbol.
    pub symbol: String,
    pub kind: TypeKind,
    pub span: Span,
}

pub enum TypeKind {
    Record {
        fields: Vec<String>,
    },
    Enum {
        variants: Vec<Variant>,
    },
    /// Aliases carry no runtime representation.
    Alias,
}

pub struct Variant {
    pub name: String,
    pub symbol: String,
    pub fields: Vec<String>,
}

/// A foreign function, resolved at run time through the library it names.
pub struct ForeignFn {
    pub name: String,
    pub symbol: String,
    pub library: String,
    pub c_symbol: String,
    pub params: Vec<korben_runtime::ffi::CType>,
    pub ret: korben_runtime::ffi::CType,
    pub span: Span,
}

pub struct ProtocolDef {
    pub name: String,
    pub methods: Vec<String>,
}

pub struct ImplDef {
    pub protocol: String,
    pub type_name: String,
    /// Method name paired with the mangled symbol of its implementation.
    pub methods: Vec<(String, String)>,
}

pub struct Const {
    pub name: String,
    pub symbol: String,
    pub value: Expr,
    pub span: Span,
}

pub struct Function {
    pub name: String,
    pub symbol: String,
    pub module: String,
    pub params: Vec<Param>,
    pub body: Block,
    pub effects: Effects,
    pub is_public: bool,
    /// Calling an async function yields a task rather than running it.
    pub is_async: bool,
    /// True when the body contains a `recur` targeting this function.
    pub self_recursive: bool,
    pub span: Span,
}

pub struct Param {
    /// Mangled local name.
    pub slot: String,
    pub name: String,
    /// `Some("port")` when this parameter is passed as `:port value`.
    pub keyword: Option<String>,
    pub default: Option<Expr>,
    pub span: Span,
}

pub struct Block {
    pub stmts: Vec<Stmt>,
    /// True when any statement in this block registers a deferred body.
    pub has_defer: bool,
    pub span: Span,
}

pub enum Stmt {
    Let { pattern: Pattern, value: Expr, span: Span },
    Var { slot: String, value: Expr, span: Span },
    Defer { body: Block, span: Span },
    Expr(Expr),
}

/// A resolved reference. The front end has already decided what a name means.
#[derive(Clone, Debug)]
pub enum Ref {
    /// A lexical binding, by mangled name.
    Local(String),
    /// A module-level definition, by mangled symbol.
    Global(String),
    /// A function provided by the runtime, by its canonical Korben name.
    Builtin(String),
    /// A record or enum constructor.
    Ctor { type_name: String, variant: Option<String>, fields: Vec<String> },
    /// A protocol method dispatching on its first argument.
    Method { protocol: String, name: String },
    /// A payload-free enum variant used as a value.
    Unit { type_name: String, variant: String },
}

pub struct Arg {
    pub keyword: Option<String>,
    pub value: Expr,
    pub span: Span,
}

pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

pub struct CatchArm {
    pub condition: String,
    pub slot: String,
    pub body: Block,
    pub span: Span,
}

pub enum Expr {
    Nil(Span),
    Bool(bool, Span),
    Int(i64, Span),
    Float(f64, Span),
    Str(String, Span),
    Keyword(String, Span),
    /// A quoted symbol.
    Symbol(String, Span),
    Ref(Ref, Span),
    Vector(Vec<Expr>, Span),
    Map(Vec<(Expr, Expr)>, Span),
    Set(Vec<Expr>, Span),
    Record {
        type_name: Option<String>,
        fields: Vec<(String, Expr)>,
        span: Span,
    },
    /// String interpolation, lowered from `format`.
    Concat(Vec<Expr>, Span),
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Option<Box<Expr>>,
        span: Span,
    },
    And(Vec<Expr>, Span),
    Or(Vec<Expr>, Span),
    Do(Box<Block>, Span),
    Lambda(Box<Lambda>, Span),
    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
        span: Span,
    },
    Field {
        target: Box<Expr>,
        name: String,
        span: Span,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    Loop {
        bindings: Vec<(String, Expr)>,
        body: Box<Block>,
        span: Span,
    },
    Recur(Vec<Expr>, Span),
    Assign {
        slot: String,
        value: Box<Expr>,
        span: Span,
    },
    Propagate(Box<Expr>, Span),
    Try {
        body: Box<Block>,
        catches: Vec<CatchArm>,
        finally: Option<Box<Block>>,
        span: Span,
    },
    Throw(Box<Expr>, Span),
    With {
        slot: String,
        value: Box<Expr>,
        body: Box<Block>,
        span: Span,
    },
    /// `unsafe` is lexical; the backend records it but the body is ordinary.
    Unsafe(Box<Block>, Span),
    /// A task scope, binding `slot` to the scope for the duration of the body.
    TaskScope {
        slot: String,
        body: Box<Block>,
        span: Span,
    },
    /// Defer a thunk as a task in a scope.
    Spawn {
        scope: Box<Expr>,
        thunk: Box<Expr>,
        span: Span,
    },
    /// Run a task to completion and take its value.
    Await(Box<Expr>, Span),
    /// Data produced by `'form`.
    Quote(crate::value::Value, Span),
}

impl Expr {
    // korben-vok
    /// Where this expression came from.
    ///
    /// The checker keys what it concluded by span, so this is how a lowered
    /// expression is matched back to its type. Every variant carries one
    /// already; this only reaches for it.
    pub fn span(&self) -> Span {
        match self {
            Expr::Nil(span)
            | Expr::Bool(_, span)
            | Expr::Int(_, span)
            | Expr::Float(_, span)
            | Expr::Str(_, span)
            | Expr::Keyword(_, span)
            | Expr::Symbol(_, span)
            | Expr::Ref(_, span)
            | Expr::Vector(_, span)
            | Expr::Map(_, span)
            | Expr::Set(_, span)
            | Expr::Concat(_, span)
            | Expr::And(_, span)
            | Expr::Or(_, span)
            | Expr::Do(_, span)
            | Expr::Lambda(_, span)
            | Expr::Recur(_, span)
            | Expr::Propagate(_, span)
            | Expr::Throw(_, span)
            | Expr::Unsafe(_, span)
            | Expr::Await(_, span)
            | Expr::Quote(_, span) => *span,
            Expr::Record { span, .. }
            | Expr::If { span, .. }
            | Expr::Call { span, .. }
            | Expr::Field { span, .. }
            | Expr::Match { span, .. }
            | Expr::Loop { span, .. }
            | Expr::Assign { span, .. }
            | Expr::Try { span, .. }
            | Expr::With { span, .. }
            | Expr::TaskScope { span, .. }
            | Expr::Spawn { span, .. } => *span,
        }
    }
}

pub struct Lambda {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Block,
    /// Enclosing locals the body reads. A backend that compiles a closure to a
    /// value has to copy these in; the interpreter captures its environment
    /// directly and ignores the list.
    pub captures: Vec<String>,
    pub self_recursive: bool,
    pub span: Span,
}

pub enum Pattern {
    Wildcard(Span),
    Bind(String, Span),
    Nil(Span),
    Bool(bool, Span),
    Int(i64, Span),
    Float(f64, Span),
    Str(String, Span),
    Keyword(String, Span),
    Variant {
        name: String,
        positional: Vec<Pattern>,
        named: Vec<(String, Pattern)>,
        span: Span,
    },
    Vector {
        items: Vec<Pattern>,
        rest: Option<Option<String>>,
        span: Span,
    },
    /// Map and record patterns are the same operation at runtime: look a member
    /// up by name, accepting keyword or string keys.
    Members {
        entries: Vec<(String, Pattern)>,
        span: Span,
    },
}

impl Pattern {
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wildcard(span)
            | Pattern::Bind(_, span)
            | Pattern::Nil(span)
            | Pattern::Bool(_, span)
            | Pattern::Int(_, span)
            | Pattern::Float(_, span)
            | Pattern::Str(_, span)
            | Pattern::Keyword(_, span)
            | Pattern::Variant { span, .. }
            | Pattern::Vector { span, .. }
            | Pattern::Members { span, .. } => *span,
        }
    }
}

// ------------------------------------------------------------------ mangling

/// Rust keywords that a mangled identifier must not collide with.
const RESERVED: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
];

/// Turn a Korben name into a distinct, valid Rust identifier.
///
/// Names that are already plain identifiers pass through with a prefix so they
/// stay readable in generated source. Anything else is sanitized and given a
/// short digest so that `read-line` and `read_line` cannot collide.
pub fn mangle(prefix: &str, name: &str) -> String {
    let plain = !name.is_empty()
        && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !name.chars().next().unwrap().is_ascii_digit();
    if plain && !RESERVED.contains(&name) {
        return format!("{prefix}{name}");
    }
    let mut sanitized = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }
    if sanitized.is_empty() || sanitized.chars().next().unwrap().is_ascii_digit() {
        sanitized.insert(0, '_');
    }
    // korben-bdg
    // `complete?` sanitises to `complete_`, and the digest would then follow a
    // second underscore. rustc does not call a name with two in a row snake
    // case, and a generated crate should not warn about its own names.
    while sanitized.ends_with('_') {
        sanitized.pop();
    }
    if sanitized.is_empty() {
        sanitized.push('x');
    }
    format!("{prefix}{sanitized}_{:08x}", digest(name))
}

/// A stable 32-bit FNV-1a digest, used only to keep mangled names distinct.
fn digest(text: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// The mangled symbol for a module-level definition.
///
// korben-bdg
/// The module and the name are joined by a single underscore and a digest of
/// the pair. Two underscores would read as the obvious separator, but rustc
/// does not consider a name containing them snake case, and a generated crate
/// that warns about its own names teaches a reader to ignore warnings. The
/// digest is what keeps `a` + `b_c` apart from `a_b` + `c`.
pub fn global_symbol(module: &str, name: &str) -> String {
    format!(
        "{}_{}_{:08x}",
        mangle("m_", module),
        mangle("", name),
        digest(&format!("{module}/{name}"))
    )
}

// korben-bdg
/// A local's symbol.
///
/// Underscore-prefixed because a binding the program does not read is the
/// Korben `unused-binding` lint's business, reported against the code someone
/// wrote; rustc reporting it again against generated code says the same thing
/// about a name that reader never chose. Rust does not warn about a name
/// beginning with an underscore, used or not.
fn local_symbol(name: &str) -> String {
    mangle("_v_", name)
}

// ------------------------------------------------------------------ lowering

/// Lower a loaded session into core IR.
///
/// Resolution consults the runtime module table the loader already built, so
/// the IR agrees with the interpreter about what every name means.
pub fn lower_session(session: &Session, entry_module: &str) -> Result<Program, Vec<Diagnostic>> {
    let mut lowerer = Lowerer {
        interp: &session.interp,
        module: String::new(),
        scopes: Vec::new(),
        builtins: HashSet::new(),
        diagnostics: Vec::new(),
        in_loop: 0,
        saw_self_recur: false,
    };
    let mut modules = Vec::new();
    for module in &session.modules {
        modules.push(lowerer.module(module));
    }
    if !lowerer.diagnostics.is_empty() {
        return Err(lowerer.diagnostics);
    }
    let entry = session
        .interp
        .modules
        .borrow()
        .get(entry_module)
        .filter(|runtime| runtime.globals.borrow().contains_key("main"))
        .map(|_| global_symbol(entry_module, "main"));
    let mut builtins: Vec<String> = lowerer.builtins.into_iter().collect();
    builtins.sort();
    // korben-vok: the checker's conclusions travel with the program.
    Ok(Program { modules, entry, builtins, types: session.types.clone() })
}

use korben_syntax::diag::Diagnostic;

struct Lowerer<'a> {
    interp: &'a Interp,
    module: String,
    /// Lexical scopes mapping source names to mangled slots.
    scopes: Vec<Vec<(String, String)>>,
    builtins: HashSet<String>,
    diagnostics: Vec<Diagnostic>,
    in_loop: usize,
    saw_self_recur: bool,
}

impl<'a> Lowerer<'a> {
    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn bind(&mut self, name: &str) -> String {
        let slot = local_symbol(name);
        if let Some(scope) = self.scopes.last_mut() {
            scope.retain(|(existing, _)| existing != name);
            scope.push((name.to_string(), slot.clone()));
        }
        slot
    }

    fn lookup_local(&self, name: &str) -> Option<String> {
        for scope in self.scopes.iter().rev() {
            if let Some((_, slot)) = scope.iter().rev().find(|(existing, _)| existing == name) {
                return Some(slot.clone());
            }
        }
        None
    }

    fn error(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Which enclosing locals a closure body reads.
    ///
    /// Only slots that are actually in scope outside the closure count; a name
    /// the body binds for itself is not a capture.
    fn captures_of(&self, body: &Block) -> Vec<String> {
        let mut referenced = HashSet::new();
        collect_locals_block(body, &mut referenced);
        let mut captures: Vec<String> = self
            .scopes
            .iter()
            .flat_map(|scope| scope.iter())
            .filter(|(_, slot)| referenced.contains(slot))
            .map(|(_, slot)| slot.clone())
            .collect();
        captures.sort();
        captures.dedup();
        captures
    }

    fn module(&mut self, module: &ast::Module) -> Module {
        self.module = module.name.clone();
        let mut types = Vec::new();
        let mut functions = Vec::new();
        let mut foreign = Vec::new();
        let mut consts = Vec::new();
        let mut impls = Vec::new();
        let mut protocols = Vec::new();

        for item in &module.items {
            match item {
                ast::Item::Type(decl) => types.push(self.type_def(decl)),
                ast::Item::Fn(decl) => {
                    functions.push(self.function(decl, &global_symbol(&module.name, &decl.name)))
                }
                ast::Item::Const { name, value, span, .. } => {
                    self.push_scope();
                    let value = self.expr(value);
                    self.pop_scope();
                    consts.push(Const {
                        name: name.clone(),
                        symbol: global_symbol(&module.name, name),
                        value,
                        span: *span,
                    });
                }
                ast::Item::Protocol(decl) => protocols.push(ProtocolDef {
                    name: decl.name.clone(),
                    methods: decl.methods.iter().map(|method| method.name.clone()).collect(),
                }),
                ast::Item::Impl(decl) => {
                    let mut methods = Vec::new();
                    for method in &decl.methods {
                        let symbol = global_symbol(
                            &module.name,
                            &format!("{}#{}#{}", decl.protocol, decl.type_name, method.name),
                        );
                        functions.push(self.function(method, &symbol));
                        methods.push((method.name.clone(), symbol));
                    }
                    impls.push(ImplDef {
                        protocol: decl.protocol.clone(),
                        type_name: decl.type_name.clone(),
                        methods,
                    });
                }
                ast::Item::Foreign(decl) => foreign.push(ForeignFn {
                    name: decl.name.clone(),
                    symbol: global_symbol(&module.name, &decl.name),
                    library: decl.library.clone(),
                    c_symbol: decl.symbol.clone(),
                    params: decl
                        .c_params
                        .iter()
                        .filter_map(|name| korben_runtime::ffi::CType::parse(name))
                        .collect(),
                    ret: korben_runtime::ffi::CType::parse(&decl.c_ret)
                        .unwrap_or(korben_runtime::ffi::CType::Void),
                    span: decl.span,
                }),
                // Tests belong to the test runner, and macros are already expanded.
                ast::Item::Test(_) | ast::Item::Macro(_) | ast::Item::Derive(_) => {}
            }
        }

        Module { name: module.name.clone(), types, functions, foreign, consts, impls, protocols }
    }

    fn type_def(&mut self, decl: &ast::TypeDecl) -> TypeDef {
        let kind = match &decl.body {
            ast::TypeBody::Record(fields) => TypeKind::Record {
                fields: fields.iter().map(|(name, _, _)| name.clone()).collect(),
            },
            ast::TypeBody::Newtype(_) => TypeKind::Record { fields: vec!["value".to_string()] },
            ast::TypeBody::Enum(variants) => TypeKind::Enum {
                variants: variants
                    .iter()
                    .map(|variant| Variant {
                        name: variant.name.clone(),
                        symbol: global_symbol(&self.module, &variant.name),
                        fields: variant.fields.iter().map(|(name, _, _)| name.clone()).collect(),
                    })
                    .collect(),
            },
            ast::TypeBody::Alias(_) => TypeKind::Alias,
        };
        TypeDef {
            name: decl.name.clone(),
            symbol: global_symbol(&self.module, &decl.name),
            kind,
            span: decl.span,
        }
    }

    fn function(&mut self, decl: &ast::FnDecl, symbol: &str) -> Function {
        self.push_scope();
        let outer_loop = std::mem::replace(&mut self.in_loop, 0);
        let outer_recur = std::mem::replace(&mut self.saw_self_recur, false);
        let params = self.params(&decl.params);
        let body = self.block(&decl.body);
        let self_recursive = self.saw_self_recur;
        self.in_loop = outer_loop;
        self.saw_self_recur = outer_recur;
        self.pop_scope();
        Function {
            name: decl.name.clone(),
            symbol: symbol.to_string(),
            module: self.module.clone(),
            params,
            body,
            effects: decl.declared_effects,
            is_public: decl.is_public,
            is_async: decl.is_async,
            self_recursive,
            span: decl.span,
        }
    }

    fn params(&mut self, params: &[ast::Param]) -> Vec<Param> {
        // Defaults are evaluated in the caller's absence, before the parameter
        // itself is in scope, so they are lowered first.
        let mut lowered = Vec::with_capacity(params.len());
        for param in params {
            let default = param.default.as_ref().map(|expr| self.expr(expr));
            let slot = self.bind(&param.name);
            lowered.push(Param {
                slot,
                name: param.name.clone(),
                keyword: param.keyword.clone(),
                default,
                span: param.span,
            });
        }
        lowered
    }

    fn block(&mut self, body: &ast::Body) -> Block {
        self.push_scope();
        let mut stmts = Vec::with_capacity(body.stmts.len());
        let mut has_defer = false;
        for stmt in &body.stmts {
            match stmt {
                ast::Stmt::Let { pattern, value, span, .. } => {
                    let value = self.expr(value);
                    let pattern = self.pattern(pattern);
                    stmts.push(Stmt::Let { pattern, value, span: *span });
                }
                ast::Stmt::Var { name, value, span, .. } => {
                    let value = self.expr(value);
                    let slot = self.bind(name);
                    stmts.push(Stmt::Var { slot, value, span: *span });
                }
                ast::Stmt::Defer { body, span } => {
                    has_defer = true;
                    let body = self.block(body);
                    stmts.push(Stmt::Defer { body, span: *span });
                }
                ast::Stmt::Expr(expr) => stmts.push(Stmt::Expr(self.expr(expr))),
            }
        }
        self.pop_scope();
        Block { stmts, has_defer, span: body.span }
    }

    fn pattern(&mut self, pattern: &ast::Pattern) -> Pattern {
        match pattern {
            ast::Pattern::Wildcard(span) => Pattern::Wildcard(*span),
            ast::Pattern::Binding(name, span) => Pattern::Bind(self.bind(name), *span),
            ast::Pattern::Nil(span) => Pattern::Nil(*span),
            ast::Pattern::Bool(value, span) => Pattern::Bool(*value, *span),
            ast::Pattern::Int(value, span) => Pattern::Int(*value, *span),
            ast::Pattern::Float(value, span) => Pattern::Float(*value, *span),
            ast::Pattern::Str(value, span) => Pattern::Str(value.clone(), *span),
            ast::Pattern::Keyword(value, span) => Pattern::Keyword(value.clone(), *span),
            ast::Pattern::Typed { inner, .. } => self.pattern(inner),
            ast::Pattern::Variant { name, positional, named, span } => Pattern::Variant {
                name: name.clone(),
                positional: positional.iter().map(|sub| self.pattern(sub)).collect(),
                named: named
                    .iter()
                    .map(|(field, sub)| (field.clone(), self.pattern(sub)))
                    .collect(),
                span: *span,
            },
            ast::Pattern::Vector { items, rest, span } => Pattern::Vector {
                items: items.iter().map(|item| self.pattern(item)).collect(),
                rest: rest.as_ref().map(|name| name.as_ref().map(|name| self.bind(name))),
                span: *span,
            },
            ast::Pattern::Map { entries, span }
            | ast::Pattern::Record { fields: entries, span } => Pattern::Members {
                entries: entries
                    .iter()
                    .map(|(key, sub)| (key.clone(), self.pattern(sub)))
                    .collect(),
                span: *span,
            },
        }
    }

    fn expr(&mut self, expr: &ast::Expr) -> Expr {
        match expr {
            ast::Expr::Nil(span) => Expr::Nil(*span),
            ast::Expr::Bool(value, span) => Expr::Bool(*value, *span),
            ast::Expr::Int(value, span) => Expr::Int(*value, *span),
            ast::Expr::Float(value, span) => Expr::Float(*value, *span),
            ast::Expr::Str(value, span) => Expr::Str(value.clone(), *span),
            ast::Expr::Keyword(name, span) => Expr::Keyword(name.clone(), *span),
            ast::Expr::Interp(parts, span) => {
                let mut pieces = Vec::with_capacity(parts.len());
                for part in parts {
                    pieces.push(match part {
                        ast::InterpPart::Text(text) => Expr::Str(text.clone(), *span),
                        ast::InterpPart::Expr(expr) => self.expr(expr),
                    });
                }
                Expr::Concat(pieces, *span)
            }
            ast::Expr::Var(name, span) => match self.resolve(name) {
                Some(reference) => Expr::Ref(reference, *span),
                None => {
                    self.unresolved(name, *span);
                    Expr::Nil(*span)
                }
            },
            ast::Expr::Path { module, name, span } => match self.resolve_path(module, name) {
                Some(reference) => Expr::Ref(reference, *span),
                None => {
                    self.error(
                        Diagnostic::error(format!("`{module}/{name}` is not defined"))
                            .with_code("unbound-name")
                            .at(*span, "not found in that module"),
                    );
                    Expr::Nil(*span)
                }
            },
            ast::Expr::Vector(items, span) => {
                Expr::Vector(items.iter().map(|item| self.expr(item)).collect(), *span)
            }
            ast::Expr::Set(items, span) => {
                Expr::Set(items.iter().map(|item| self.expr(item)).collect(), *span)
            }
            ast::Expr::Map(entries, span) => Expr::Map(
                entries.iter().map(|(key, value)| (self.expr(key), self.expr(value))).collect(),
                *span,
            ),
            ast::Expr::Record { type_name, fields, span } => Expr::Record {
                type_name: type_name.clone(),
                fields: fields
                    .iter()
                    .map(|(name, value, _)| (name.clone(), self.expr(value)))
                    .collect(),
                span: *span,
            },
            ast::Expr::If { cond, then, els, span } => Expr::If {
                cond: Box::new(self.expr(cond)),
                then: Box::new(self.expr(then)),
                els: els.as_ref().map(|els| Box::new(self.expr(els))),
                span: *span,
            },
            ast::Expr::And(operands, span) => {
                Expr::And(operands.iter().map(|operand| self.expr(operand)).collect(), *span)
            }
            ast::Expr::Or(operands, span) => {
                Expr::Or(operands.iter().map(|operand| self.expr(operand)).collect(), *span)
            }
            ast::Expr::Do(body, span) => Expr::Do(Box::new(self.block(body)), *span),
            ast::Expr::Lambda(decl, span) => {
                self.push_scope();
                let outer_loop = std::mem::replace(&mut self.in_loop, 0);
                let outer_recur = std::mem::replace(&mut self.saw_self_recur, false);
                let params = self.params(&decl.params);
                let body = self.block(&decl.body);
                let self_recursive = self.saw_self_recur;
                self.in_loop = outer_loop;
                self.saw_self_recur = outer_recur;
                self.pop_scope();
                let captures = self.captures_of(&body);
                Expr::Lambda(
                    Box::new(Lambda {
                        name: decl.name.clone(),
                        params,
                        body,
                        captures,
                        self_recursive,
                        span: *span,
                    }),
                    *span,
                )
            }
            ast::Expr::Call { callee, args, span } => {
                let callee = self.expr(callee);
                let args = args
                    .iter()
                    .map(|arg| Arg {
                        keyword: arg.keyword.clone(),
                        value: self.expr(&arg.value),
                        span: arg.span,
                    })
                    .collect();
                Expr::Call { callee: Box::new(callee), args, span: *span }
            }
            ast::Expr::Field { target, name, span } => {
                // `alias.name` is a module member unless `alias` is a binding.
                if let ast::Expr::Var(root, _) = &**target {
                    if self.lookup_local(root).is_none() && self.resolve(root).is_none() {
                        if let Some(reference) = self.resolve_path(root, name) {
                            return Expr::Ref(reference, *span);
                        }
                    }
                }
                Expr::Field { target: Box::new(self.expr(target)), name: name.clone(), span: *span }
            }
            ast::Expr::Match { scrutinee, arms, span } => {
                let scrutinee = Box::new(self.expr(scrutinee));
                let arms = arms
                    .iter()
                    .map(|arm| {
                        self.push_scope();
                        let pattern = self.pattern(&arm.pattern);
                        let guard = arm.guard.as_ref().map(|guard| self.expr(guard));
                        let body = self.expr(&arm.body);
                        self.pop_scope();
                        MatchArm { pattern, guard, body, span: arm.span }
                    })
                    .collect();
                Expr::Match { scrutinee, arms, span: *span }
            }
            ast::Expr::Loop { bindings, body, span } => {
                let values: Vec<Expr> =
                    bindings.iter().map(|(_, value)| self.expr(value)).collect();
                self.push_scope();
                let slots: Vec<String> = bindings.iter().map(|(name, _)| self.bind(name)).collect();
                self.in_loop += 1;
                let body = self.block(body);
                self.in_loop -= 1;
                self.pop_scope();
                Expr::Loop {
                    bindings: slots.into_iter().zip(values).collect(),
                    body: Box::new(body),
                    span: *span,
                }
            }
            ast::Expr::Recur(args, span) => {
                if self.in_loop == 0 {
                    self.saw_self_recur = true;
                }
                Expr::Recur(args.iter().map(|arg| self.expr(arg)).collect(), *span)
            }
            ast::Expr::Assign { name, value, span } => {
                let value = Box::new(self.expr(value));
                match self.lookup_local(name) {
                    Some(slot) => Expr::Assign { slot, value, span: *span },
                    None => {
                        self.error(
                            Diagnostic::error(format!("cannot assign to `{name}`"))
                                .with_code("unbound-assign")
                                .at(*span, "no binding with this name is in scope")
                                .help(format!("declare it with `(var {name} ...)` first")),
                        );
                        Expr::Nil(*span)
                    }
                }
            }
            ast::Expr::Propagate(inner, span) => Expr::Propagate(Box::new(self.expr(inner)), *span),
            ast::Expr::Throw(inner, span) => Expr::Throw(Box::new(self.expr(inner)), *span),
            ast::Expr::Try { body, catches, finally, span } => {
                let body = Box::new(self.block(body));
                let catches = catches
                    .iter()
                    .map(|arm| {
                        self.push_scope();
                        let slot = self.bind(&arm.binding);
                        let body = self.block(&arm.body);
                        self.pop_scope();
                        CatchArm { condition: arm.condition.clone(), slot, body, span: arm.span }
                    })
                    .collect();
                let finally = finally.as_ref().map(|body| Box::new(self.block(body)));
                Expr::Try { body, catches, finally, span: *span }
            }
            ast::Expr::With { name, value, body, span } => {
                let value = Box::new(self.expr(value));
                self.push_scope();
                let slot = self.bind(name);
                let body = Box::new(self.block(body));
                self.pop_scope();
                Expr::With { slot, value, body, span: *span }
            }
            ast::Expr::Unsafe(body, span) => Expr::Unsafe(Box::new(self.block(body)), *span),
            ast::Expr::Await(inner, span) => Expr::Await(Box::new(self.expr(inner)), *span),
            ast::Expr::Spawn { scope, thunk, span } => Expr::Spawn {
                scope: Box::new(self.expr(scope)),
                thunk: Box::new(self.expr(thunk)),
                span: *span,
            },
            ast::Expr::TaskScope { name, body, span } => {
                self.push_scope();
                let slot = self.bind(name);
                let body = self.block(body);
                self.pop_scope();
                Expr::TaskScope { slot, body: Box::new(body), span: *span }
            }
            ast::Expr::Quote(syntax, span) => Expr::Quote(crate::eval::quote_value(syntax), *span),
            ast::Expr::SyntaxQuote(_, span) => {
                self.error(
                    Diagnostic::error("syntax objects cannot be constructed at run time")
                        .with_code("syntax-quote-runtime")
                        .at(*span, "`` ` `` is only available at expansion time")
                        .help("move this into a `macro` definition"),
                );
                Expr::Nil(*span)
            }
        }
    }

    fn unresolved(&mut self, name: &str, span: Span) {
        self.error(
            Diagnostic::error(format!("`{name}` is not defined"))
                .with_code("unbound-name")
                .at(span, "no binding with this name is in scope"),
        );
    }

    /// Resolve a bare name the way the interpreter would.
    fn resolve(&mut self, name: &str) -> Option<Ref> {
        if let Some(slot) = self.lookup_local(name) {
            return Some(Ref::Local(slot));
        }
        let current = self.interp.modules.borrow().get(&self.module)?.clone();
        if let Some(value) = current.globals.borrow().get(name) {
            return Some(self.classify(&self.module.clone(), name, value));
        }
        let imported = current.imported.borrow().get(name).cloned();
        if let Some((source, original)) = imported {
            return self.resolve_in(&source, &original);
        }
        self.resolve_in(crate::builtins::PRELUDE, name)
    }

    fn resolve_path(&mut self, alias: &str, name: &str) -> Option<Ref> {
        let target = self
            .interp
            .modules
            .borrow()
            .get(&self.module)
            .and_then(|current| current.aliases.borrow().get(alias).cloned())
            .or_else(|| {
                self.interp.modules.borrow().contains_key(alias).then(|| alias.to_string())
            })?;
        self.resolve_in(&target, name)
    }

    fn resolve_in(&mut self, module: &str, name: &str) -> Option<Ref> {
        let runtime = self.interp.modules.borrow().get(module)?.clone();
        let value = runtime
            .exports
            .borrow()
            .get(name)
            .cloned()
            .or_else(|| runtime.globals.borrow().get(name).cloned())?;
        Some(self.classify(module, name, &value))
    }

    /// Decide what kind of reference a runtime value represents.
    fn classify(&mut self, module: &str, name: &str, value: &crate::value::Value) -> Ref {
        use crate::value::{Body, Value};
        match value {
            Value::Fn(function) => match &function.body {
                Body::Native(_) => {
                    let key = format!("{module}/{name}");
                    self.builtins.insert(key.clone());
                    Ref::Builtin(key)
                }
                Body::Ctor { type_name, variant, fields } => Ref::Ctor {
                    type_name: type_name.to_string(),
                    variant: variant.as_ref().map(ToString::to_string),
                    fields: fields.iter().map(ToString::to_string).collect(),
                },
                Body::Method { protocol, name } => {
                    Ref::Method { protocol: protocol.to_string(), name: name.to_string() }
                }
                Body::Host(_) | Body::Rust(_) => Ref::Global(global_symbol(module, name)),
            },
            Value::Variant(variant) if variant.fields.is_empty() => Ref::Unit {
                type_name: variant.type_name.to_string(),
                variant: variant.variant.to_string(),
            },
            _ => Ref::Global(global_symbol(module, name)),
        }
    }
}

/// Collect every local slot a block reads.
fn collect_locals_block(block: &Block, out: &mut HashSet<String>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { value, .. } | Stmt::Var { value, .. } => collect_locals(value, out),
            Stmt::Defer { body, .. } => collect_locals_block(body, out),
            Stmt::Expr(expr) => collect_locals(expr, out),
        }
    }
}

fn collect_locals(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Ref(Ref::Local(slot), _) => {
            out.insert(slot.clone());
        }
        Expr::Assign { slot, value, .. } => {
            out.insert(slot.clone());
            collect_locals(value, out);
        }
        Expr::Vector(items, _)
        | Expr::Set(items, _)
        | Expr::Recur(items, _)
        | Expr::Concat(items, _)
        | Expr::And(items, _)
        | Expr::Or(items, _) => {
            for item in items {
                collect_locals(item, out);
            }
        }
        Expr::Map(entries, _) => {
            for (key, value) in entries {
                collect_locals(key, out);
                collect_locals(value, out);
            }
        }
        Expr::Record { fields, .. } => {
            for (_, value) in fields {
                collect_locals(value, out);
            }
        }
        Expr::If { cond, then, els, .. } => {
            collect_locals(cond, out);
            collect_locals(then, out);
            if let Some(els) = els {
                collect_locals(els, out);
            }
        }
        Expr::Do(block, _) | Expr::Unsafe(block, _) | Expr::TaskScope { body: block, .. } => {
            collect_locals_block(block, out)
        }
        // A nested closure's captures are reads of this closure's scope too.
        Expr::Lambda(lambda, _) => {
            collect_locals_block(&lambda.body, out);
        }
        Expr::Call { callee, args, .. } => {
            collect_locals(callee, out);
            for arg in args {
                collect_locals(&arg.value, out);
            }
        }
        Expr::Field { target, .. } => collect_locals(target, out),
        Expr::Match { scrutinee, arms, .. } => {
            collect_locals(scrutinee, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_locals(guard, out);
                }
                collect_locals(&arm.body, out);
            }
        }
        Expr::Loop { bindings, body, .. } => {
            for (_, value) in bindings {
                collect_locals(value, out);
            }
            collect_locals_block(body, out);
        }
        Expr::Propagate(inner, _) | Expr::Throw(inner, _) | Expr::Await(inner, _) => {
            collect_locals(inner, out)
        }
        Expr::Spawn { scope, thunk, .. } => {
            collect_locals(scope, out);
            collect_locals(thunk, out);
        }
        Expr::Try { body, catches, finally, .. } => {
            collect_locals_block(body, out);
            for arm in catches {
                collect_locals_block(&arm.body, out);
            }
            if let Some(finally) = finally {
                collect_locals_block(finally, out);
            }
        }
        Expr::With { value, body, .. } => {
            collect_locals(value, out);
            collect_locals_block(body, out);
        }
        _ => {}
    }
}

// ------------------------------------------------------------------ printing

/// Render the IR as readable text. This is what `korben build --emit ir` prints.
pub fn render(program: &Program) -> String {
    let mut out = String::new();
    if let Some(entry) = &program.entry {
        let _ = writeln!(out, ";; entry: {entry}");
    }
    if !program.builtins.is_empty() {
        let _ = writeln!(out, ";; runtime builtins: {}", program.builtins.len());
        for name in &program.builtins {
            let _ = writeln!(out, ";;   {name}");
        }
    }
    for module in &program.modules {
        let _ = writeln!(out, "\n(module {}", module.name);
        for def in &module.types {
            match &def.kind {
                TypeKind::Record { fields } => {
                    let _ = writeln!(out, "  (record {} [{}])", def.name, fields.join(" "));
                }
                TypeKind::Enum { variants } => {
                    let _ = writeln!(out, "  (enum {}", def.name);
                    for variant in variants {
                        let _ =
                            writeln!(out, "    ({} [{}])", variant.name, variant.fields.join(" "));
                    }
                    let _ = writeln!(out, "  )");
                }
                TypeKind::Alias => {
                    let _ = writeln!(out, "  (alias {})", def.name);
                }
            }
        }
        for protocol in &module.protocols {
            let _ =
                writeln!(out, "  (protocol {} [{}])", protocol.name, protocol.methods.join(" "));
        }
        for def in &module.impls {
            let _ = writeln!(out, "  (impl {} {}", def.protocol, def.type_name);
            for (name, symbol) in &def.methods {
                let _ = writeln!(out, "    ({name} -> {symbol})");
            }
            let _ = writeln!(out, "  )");
        }
        for def in &module.consts {
            let _ = writeln!(out, "  (const {} {}", def.symbol, render_expr(&def.value, 2));
            let _ = writeln!(out, "  )");
        }
        for foreign in &module.foreign {
            let params: Vec<String> = foreign.params.iter().map(|ty| format!("{ty:?}")).collect();
            let _ = writeln!(
                out,
                "  (foreign {} \"{}\" \"{}\" [{}] -> {:?})",
                foreign.symbol,
                foreign.library,
                foreign.c_symbol,
                params.join(" "),
                foreign.ret
            );
        }
        for function in &module.functions {
            let params: Vec<String> = function
                .params
                .iter()
                .map(|param| match &param.keyword {
                    Some(keyword) => format!(":{keyword} {}", param.slot),
                    None => param.slot.clone(),
                })
                .collect();
            let mut flags = Vec::new();
            if function.is_public {
                flags.push("pub".to_string());
            }
            if function.self_recursive {
                flags.push("tail-recursive".to_string());
            }
            if !function.effects.is_empty() {
                flags.push(function.effects.render());
            }
            let suffix =
                if flags.is_empty() { String::new() } else { format!(" ; {}", flags.join(" ")) };
            let _ = writeln!(out, "  (fn {} [{}]{suffix}", function.symbol, params.join(" "));
            let _ = write!(out, "{}", render_block(&function.body, 2));
            let _ = writeln!(out, "  )");
        }
        let _ = writeln!(out, ")");
    }
    out
}

use std::fmt::Write as _;

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn render_block(block: &Block, depth: usize) -> String {
    let mut out = String::new();
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { pattern, value, .. } => {
                let _ = writeln!(
                    out,
                    "{}(let {} {})",
                    indent(depth),
                    render_pattern(pattern),
                    render_expr(value, depth + 1)
                );
            }
            Stmt::Var { slot, value, .. } => {
                let _ = writeln!(
                    out,
                    "{}(var {slot} {})",
                    indent(depth),
                    render_expr(value, depth + 1)
                );
            }
            Stmt::Defer { body, .. } => {
                let _ = writeln!(out, "{}(defer", indent(depth));
                let _ = write!(out, "{}", render_block(body, depth + 1));
                let _ = writeln!(out, "{})", indent(depth));
            }
            Stmt::Expr(expr) => {
                let _ = writeln!(out, "{}{}", indent(depth), render_expr(expr, depth));
            }
        }
    }
    out
}

fn render_reference(reference: &Ref) -> String {
    match reference {
        Ref::Local(slot) => format!("local:{slot}"),
        Ref::Global(symbol) => format!("global:{symbol}"),
        Ref::Builtin(name) => format!("builtin:{name}"),
        Ref::Ctor { type_name, variant: Some(variant), fields } => {
            format!("ctor:{type_name}/{variant}[{}]", fields.join(" "))
        }
        Ref::Ctor { type_name, fields, .. } => {
            format!("ctor:{type_name}[{}]", fields.join(" "))
        }
        Ref::Method { protocol, name } => format!("method:{protocol}/{name}"),
        Ref::Unit { type_name, variant } => format!("unit:{type_name}/{variant}"),
    }
}

fn render_pattern(pattern: &Pattern) -> String {
    match pattern {
        Pattern::Wildcard(_) => "_".to_string(),
        Pattern::Bind(slot, _) => slot.clone(),
        Pattern::Nil(_) => "nil".to_string(),
        Pattern::Bool(value, _) => value.to_string(),
        Pattern::Int(value, _) => value.to_string(),
        Pattern::Float(value, _) => korben_syntax::format_float(*value),
        Pattern::Str(value, _) => korben_syntax::diag::json_string(value),
        Pattern::Keyword(name, _) => format!(":{name}"),
        Pattern::Variant { name, positional, named, .. } => {
            let mut parts: Vec<String> = positional.iter().map(render_pattern).collect();
            for (field, sub) in named {
                parts.push(format!("{field}: {}", render_pattern(sub)));
            }
            if parts.is_empty() {
                format!("({name})")
            } else {
                format!("({name} {})", parts.join(" "))
            }
        }
        Pattern::Vector { items, rest, .. } => {
            let mut parts: Vec<String> = items.iter().map(render_pattern).collect();
            match rest {
                Some(Some(slot)) => parts.push(format!("...{slot}")),
                Some(None) => parts.push("...".to_string()),
                None => {}
            }
            format!("[{}]", parts.join(" "))
        }
        Pattern::Members { entries, .. } => {
            let parts: Vec<String> =
                entries.iter().map(|(key, sub)| format!("{key} {}", render_pattern(sub))).collect();
            format!("{{{}}}", parts.join(" "))
        }
    }
}

fn render_expr(expr: &Expr, depth: usize) -> String {
    match expr {
        Expr::Nil(_) => "nil".to_string(),
        Expr::Bool(value, _) => value.to_string(),
        Expr::Int(value, _) => value.to_string(),
        Expr::Float(value, _) => korben_syntax::format_float(*value),
        Expr::Str(value, _) => korben_syntax::diag::json_string(value),
        Expr::Keyword(name, _) => format!(":{name}"),
        Expr::Symbol(name, _) => format!("'{name}"),
        Expr::Ref(reference, _) => render_reference(reference),
        Expr::Vector(items, _) => {
            format!("[{}]", render_list(items, depth))
        }
        Expr::Set(items, _) => format!("#{{{}}}", render_list(items, depth)),
        Expr::Map(entries, _) => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(key, value)| {
                    format!("{} {}", render_expr(key, depth), render_expr(value, depth))
                })
                .collect();
            format!("{{{}}}", parts.join(" "))
        }
        Expr::Record { type_name, fields, .. } => {
            let parts: Vec<String> = fields
                .iter()
                .map(|(name, value)| format!("{name} {}", render_expr(value, depth)))
                .collect();
            match type_name {
                Some(name) => format!("({name} {{{}}})", parts.join(" ")),
                None => format!("{{{}}}", parts.join(" ")),
            }
        }
        Expr::Concat(parts, _) => format!("(concat {})", render_list(parts, depth)),
        Expr::If { cond, then, els, .. } => match els {
            Some(els) => format!(
                "(if {} {} {})",
                render_expr(cond, depth),
                render_expr(then, depth),
                render_expr(els, depth)
            ),
            None => format!("(if {} {})", render_expr(cond, depth), render_expr(then, depth)),
        },
        Expr::And(operands, _) => format!("(and {})", render_list(operands, depth)),
        Expr::Or(operands, _) => format!("(or {})", render_list(operands, depth)),
        Expr::Do(block, _) => {
            format!("(do\n{}{})", render_block(block, depth + 1), indent(depth))
        }
        Expr::Lambda(lambda, _) => {
            let params: Vec<String> =
                lambda.params.iter().map(|param| param.slot.clone()).collect();
            format!(
                "(fn [{}]\n{}{})",
                params.join(" "),
                render_block(&lambda.body, depth + 1),
                indent(depth)
            )
        }
        Expr::Call { callee, args, .. } => {
            let mut parts = vec![render_expr(callee, depth)];
            for arg in args {
                match &arg.keyword {
                    Some(keyword) => {
                        parts.push(format!(":{keyword}"));
                        parts.push(render_expr(&arg.value, depth));
                    }
                    None => parts.push(render_expr(&arg.value, depth)),
                }
            }
            format!("({})", parts.join(" "))
        }
        Expr::Field { target, name, .. } => {
            format!("(field {} {name})", render_expr(target, depth))
        }
        Expr::Match { scrutinee, arms, .. } => {
            let mut out = format!("(match {}\n", render_expr(scrutinee, depth));
            for arm in arms {
                let guard = match &arm.guard {
                    Some(guard) => format!(" :when {}", render_expr(guard, depth + 1)),
                    None => String::new(),
                };
                let _ = writeln!(
                    out,
                    "{}{}{guard} {}",
                    indent(depth + 1),
                    render_pattern(&arm.pattern),
                    render_expr(&arm.body, depth + 2)
                );
            }
            let _ = write!(out, "{})", indent(depth));
            out
        }
        Expr::Loop { bindings, body, .. } => {
            let parts: Vec<String> = bindings
                .iter()
                .map(|(slot, value)| format!("{slot} {}", render_expr(value, depth)))
                .collect();
            format!(
                "(loop [{}]\n{}{})",
                parts.join(" "),
                render_block(body, depth + 1),
                indent(depth)
            )
        }
        Expr::Recur(args, _) => format!("(recur {})", render_list(args, depth)),
        Expr::Assign { slot, value, .. } => {
            format!("(set! {slot} {})", render_expr(value, depth))
        }
        Expr::Propagate(inner, _) => format!("(propagate {})", render_expr(inner, depth)),
        Expr::Throw(inner, _) => format!("(throw {})", render_expr(inner, depth)),
        Expr::Try { body, catches, finally, .. } => {
            let mut out = format!("(try\n{}", render_block(body, depth + 1));
            for arm in catches {
                let _ = writeln!(out, "{}(catch {} {}", indent(depth + 1), arm.condition, arm.slot);
                let _ = write!(out, "{}", render_block(&arm.body, depth + 2));
                let _ = writeln!(out, "{})", indent(depth + 1));
            }
            if let Some(finally) = finally {
                let _ = writeln!(out, "{}(finally", indent(depth + 1));
                let _ = write!(out, "{}", render_block(finally, depth + 2));
                let _ = writeln!(out, "{})", indent(depth + 1));
            }
            let _ = write!(out, "{})", indent(depth));
            out
        }
        Expr::With { slot, value, body, .. } => format!(
            "(with {slot} {}\n{}{})",
            render_expr(value, depth),
            render_block(body, depth + 1),
            indent(depth)
        ),
        Expr::Unsafe(block, _) => {
            format!("(unsafe\n{}{})", render_block(block, depth + 1), indent(depth))
        }
        Expr::TaskScope { slot, body, .. } => {
            format!("(task-scope {slot}\n{}{})", render_block(body, depth + 1), indent(depth))
        }
        Expr::Spawn { scope, thunk, .. } => {
            format!("(spawn {} {})", render_expr(scope, depth), render_expr(thunk, depth))
        }
        Expr::Await(inner, _) => format!("(await {})", render_expr(inner, depth)),
        Expr::Quote(value, _) => format!("(quote {value})"),
    }
}

fn render_list(items: &[Expr], depth: usize) -> String {
    items.iter().map(|item| render_expr(item, depth)).collect::<Vec<_>>().join(" ")
}
