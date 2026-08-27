//! The abstract syntax tree produced after reading and macro expansion.
//!
//! Every node keeps the span of the source text it came from, so diagnostics,
//! the formatter, and the language server can always point back at what the
//! user wrote — including through macro expansion.

// korben-6bc

use korben_syntax::span::Span;
use korben_syntax::Syntax;
use std::rc::Rc;

/// Effects tracked in signatures, per specification section 10.5.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Effects(pub u8);

pub const EFFECT_IO: u8 = 1 << 0;
pub const EFFECT_ASYNC: u8 = 1 << 1;
pub const EFFECT_ALLOC: u8 = 1 << 2;
pub const EFFECT_FFI: u8 = 1 << 3;
pub const EFFECT_UNSAFE: u8 = 1 << 4;

impl Effects {
    pub const NONE: Effects = Effects(0);

    pub fn from_name(name: &str) -> Option<Effects> {
        match name {
            "!io" => Some(Effects(EFFECT_IO)),
            "!async" => Some(Effects(EFFECT_ASYNC)),
            "!alloc" => Some(Effects(EFFECT_ALLOC)),
            "!ffi" => Some(Effects(EFFECT_FFI)),
            "!unsafe" => Some(Effects(EFFECT_UNSAFE)),
            _ => None,
        }
    }

    pub fn union(self, other: Effects) -> Effects {
        Effects(self.0 | other.0)
    }

    pub fn contains(self, other: Effects) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn missing(self, required: Effects) -> Effects {
        Effects(required.0 & !self.0)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn names(self) -> Vec<&'static str> {
        let mut names = Vec::new();
        for (bit, name) in [
            (EFFECT_IO, "!io"),
            (EFFECT_ASYNC, "!async"),
            (EFFECT_ALLOC, "!alloc"),
            (EFFECT_FFI, "!ffi"),
            (EFFECT_UNSAFE, "!unsafe"),
        ] {
            if self.0 & bit != 0 {
                names.push(name);
            }
        }
        names
    }

    pub fn render(self) -> String {
        self.names().join(" ")
    }
}

/// Surface syntax for a type annotation.
#[derive(Clone, Debug)]
pub enum TypeExpr {
    /// `Int`, `Vec T`, `Result T E`, `app.models/User`.
    Name(String, Vec<TypeExpr>, Span),
    /// `{ id: Uuid name: String }`
    Record(Vec<(String, TypeExpr)>, Span),
    /// `[Int String Bool]`
    Tuple(Vec<TypeExpr>, Span),
    /// `(-> [Int Int] Int)`
    Fn(Vec<TypeExpr>, Box<TypeExpr>, Effects, Span),
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Name(_, _, span)
            | TypeExpr::Record(_, span)
            | TypeExpr::Tuple(_, span)
            | TypeExpr::Fn(_, _, _, span) => *span,
        }
    }
}

/// A binding position: `name`, `name: Type`, or a destructuring pattern.
#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: Option<TypeExpr>,
    /// `Some(":port")` when the caller passes this argument by keyword.
    pub keyword: Option<String>,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    /// Effects written in the signature. Inference computes the actual set.
    pub declared_effects: Effects,
    pub body: Body,
    pub is_async: bool,
    pub is_public: bool,
    pub is_unsafe: bool,
    pub doc: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TypeDecl {
    pub name: String,
    pub params: Vec<String>,
    pub body: TypeBody,
    pub is_public: bool,
    pub doc: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TypeBody {
    Record(Vec<(String, TypeExpr, Span)>),
    Newtype(TypeExpr),
    Alias(TypeExpr),
    Enum(Vec<VariantDecl>),
}

#[derive(Clone, Debug)]
pub struct VariantDecl {
    pub name: String,
    /// Named payload fields, e.g. `(Success user: User)`.
    pub fields: Vec<(String, TypeExpr, Span)>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ProtocolDecl {
    pub name: String,
    pub methods: Vec<ProtocolMethod>,
    pub is_public: bool,
    pub doc: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ProtocolMethod {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub effects: Effects,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ImplDecl {
    pub protocol: String,
    pub type_name: String,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct MacroDecl {
    pub name: String,
    pub params: Vec<String>,
    /// Name bound to the remaining arguments, from a `...rest` parameter.
    pub rest: Option<String>,
    pub body: Vec<Syntax>,
    pub is_public: bool,
    pub doc: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TestDecl {
    pub name: String,
    /// Property tests bind generators before running the body.
    pub generators: Vec<(String, Expr)>,
    pub body: Body,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct DeriveDecl {
    pub type_name: String,
    pub protocols: Vec<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Item {
    Fn(Rc<FnDecl>),
    Type(Rc<TypeDecl>),
    Protocol(Rc<ProtocolDecl>),
    Impl(Rc<ImplDecl>),
    Macro(Rc<MacroDecl>),
    Test(Rc<TestDecl>),
    Derive(DeriveDecl),
    /// `(def name value)` — a module-level constant.
    Const {
        name: String,
        ty: Option<TypeExpr>,
        value: Expr,
        is_public: bool,
        doc: Option<String>,
        span: Span,
    },
}

impl Item {
    pub fn name(&self) -> &str {
        match self {
            Item::Fn(decl) => &decl.name,
            Item::Type(decl) => &decl.name,
            Item::Protocol(decl) => &decl.name,
            Item::Impl(decl) => &decl.type_name,
            Item::Macro(decl) => &decl.name,
            Item::Test(decl) => &decl.name,
            Item::Derive(decl) => &decl.type_name,
            Item::Const { name, .. } => name,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Item::Fn(decl) => decl.span,
            Item::Type(decl) => decl.span,
            Item::Protocol(decl) => decl.span,
            Item::Impl(decl) => decl.span,
            Item::Macro(decl) => decl.span,
            Item::Test(decl) => decl.span,
            Item::Derive(decl) => decl.span,
            Item::Const { span, .. } => *span,
        }
    }

    pub fn is_public(&self) -> bool {
        match self {
            Item::Fn(decl) => decl.is_public,
            Item::Type(decl) => decl.is_public,
            Item::Protocol(decl) => decl.is_public,
            Item::Macro(decl) => decl.is_public,
            Item::Const { is_public, .. } => *is_public,
            Item::Impl(_) | Item::Test(_) | Item::Derive(_) => true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Import {
    pub path: String,
    /// Local alias; defaults to the last path segment.
    pub alias: String,
    /// `:only [..]` or `[..]` — names pulled directly into scope.
    pub names: Option<Vec<String>>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Module {
    pub name: String,
    pub file: korben_syntax::FileId,
    pub imports: Vec<Import>,
    pub items: Vec<Item>,
    pub doc: Option<String>,
    pub span: Span,
}

/// A sequence of statements. `let` and `var` scope over the statements that
/// follow them, matching the specification's function-body examples.
#[derive(Clone, Debug)]
pub struct Body {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

impl Body {
    pub fn empty(span: Span) -> Body {
        Body { stmts: Vec::new(), span }
    }
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Let { pattern: Pattern, ty: Option<TypeExpr>, value: Expr, span: Span },
    Var { name: String, ty: Option<TypeExpr>, value: Expr, span: Span },
    Defer { body: Body, span: Span },
    Expr(Expr),
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let { span, .. } | Stmt::Var { span, .. } | Stmt::Defer { span, .. } => *span,
            Stmt::Expr(expr) => expr.span(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct CatchArm {
    pub condition: String,
    pub binding: String,
    pub body: Body,
    pub span: Span,
}

/// One argument at a call site.
#[derive(Clone, Debug)]
pub struct Arg {
    /// `Some("port")` for `:port 6543`.
    pub keyword: Option<String>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Nil(Span),
    Bool(bool, Span),
    Int(i64, Span),
    Float(f64, Span),
    Str(String, Span),
    /// A `"...{expr}..."` interpolated string, already split into segments.
    Interp(Vec<InterpPart>, Span),
    Keyword(String, Span),
    Var(String, Span),
    Vector(Vec<Expr>, Span),
    Map(Vec<(Expr, Expr)>, Span),
    Set(Vec<Expr>, Span),
    /// `{ name value ... }` with symbol keys, optionally with a nominal type.
    Record {
        type_name: Option<String>,
        fields: Vec<(String, Expr, Span)>,
        span: Span,
    },
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Option<Box<Expr>>,
        span: Span,
    },
    /// Short-circuiting conjunction: the last value, or the first falsey one.
    And(Vec<Expr>, Span),
    /// Short-circuiting disjunction: the first truthy value, or the last one.
    Or(Vec<Expr>, Span),
    Do(Box<Body>, Span),
    Lambda(Rc<FnDecl>, Span),
    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
        span: Span,
    },
    /// `target.field`
    Field {
        target: Box<Expr>,
        name: String,
        span: Span,
    },
    /// `module/name` or `module.name` where `module` is an import alias.
    Path {
        module: String,
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
        body: Box<Body>,
        span: Span,
    },
    Recur(Vec<Expr>, Span),
    Assign {
        name: String,
        value: Box<Expr>,
        span: Span,
    },
    /// Postfix `?` — propagate `Err`/`None` to the caller.
    Propagate(Box<Expr>, Span),
    Try {
        body: Box<Body>,
        catches: Vec<CatchArm>,
        finally: Option<Box<Body>>,
        span: Span,
    },
    Throw(Box<Expr>, Span),
    With {
        name: String,
        value: Box<Expr>,
        body: Box<Body>,
        span: Span,
    },
    Unsafe(Box<Body>, Span),
    Await(Box<Expr>, Span),
    /// `(async ...)` block or `(task-scope name ...)`.
    TaskScope {
        name: String,
        body: Box<Body>,
        span: Span,
    },
    Quote(Rc<Syntax>, Span),
    /// Compile-time constructed syntax with `~` / `~@` holes filled in.
    SyntaxQuote(Rc<Template>, Span),
}

/// A syntax-quote template. Literal parts are copied verbatim; unquoted parts
/// are evaluated in the macro's compile-time environment.
#[derive(Clone, Debug)]
pub enum Template {
    Literal(Syntax),
    /// `~expr`
    Unquote(Expr),
    /// `~@expr`
    Splice(Expr),
    List(Vec<Template>, Span),
    Vector(Vec<Template>, Span),
    Map(Vec<Template>, Span),
    Set(Vec<Template>, Span),
}

#[derive(Clone, Debug)]
pub enum InterpPart {
    Text(String),
    Expr(Expr),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Nil(span)
            | Expr::Bool(_, span)
            | Expr::Int(_, span)
            | Expr::Float(_, span)
            | Expr::Str(_, span)
            | Expr::Interp(_, span)
            | Expr::Keyword(_, span)
            | Expr::Var(_, span)
            | Expr::Vector(_, span)
            | Expr::Map(_, span)
            | Expr::Set(_, span)
            | Expr::Record { span, .. }
            | Expr::If { span, .. }
            | Expr::And(_, span)
            | Expr::Or(_, span)
            | Expr::Do(_, span)
            | Expr::Lambda(_, span)
            | Expr::Call { span, .. }
            | Expr::Field { span, .. }
            | Expr::Path { span, .. }
            | Expr::Match { span, .. }
            | Expr::Loop { span, .. }
            | Expr::Recur(_, span)
            | Expr::Assign { span, .. }
            | Expr::Propagate(_, span)
            | Expr::Try { span, .. }
            | Expr::Throw(_, span)
            | Expr::With { span, .. }
            | Expr::Unsafe(_, span)
            | Expr::Await(_, span)
            | Expr::TaskScope { span, .. }
            | Expr::Quote(_, span)
            | Expr::SyntaxQuote(_, span) => *span,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Pattern {
    Wildcard(Span),
    Binding(String, Span),
    Nil(Span),
    Bool(bool, Span),
    Int(i64, Span),
    Float(f64, Span),
    Str(String, Span),
    Keyword(String, Span),
    /// `(Ok user)` / `(Success user: pattern)` / `(None)`
    Variant {
        name: String,
        positional: Vec<Pattern>,
        named: Vec<(String, Pattern)>,
        span: Span,
    },
    /// `[head ...tail]`
    Vector {
        items: Vec<Pattern>,
        rest: Option<Option<String>>,
        span: Span,
    },
    /// `{:method :get}` — keyword-keyed map pattern.
    Map {
        entries: Vec<(String, Pattern)>,
        span: Span,
    },
    /// `{name pattern}` — symbol-keyed record pattern.
    Record {
        fields: Vec<(String, Pattern)>,
        span: Span,
    },
    Typed {
        inner: Box<Pattern>,
        ty: TypeExpr,
        span: Span,
    },
}

impl Pattern {
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wildcard(span)
            | Pattern::Binding(_, span)
            | Pattern::Nil(span)
            | Pattern::Bool(_, span)
            | Pattern::Int(_, span)
            | Pattern::Float(_, span)
            | Pattern::Str(_, span)
            | Pattern::Keyword(_, span)
            | Pattern::Variant { span, .. }
            | Pattern::Vector { span, .. }
            | Pattern::Map { span, .. }
            | Pattern::Record { span, .. }
            | Pattern::Typed { span, .. } => *span,
        }
    }

    /// Every name this pattern binds, in binding order.
    pub fn bindings(&self, out: &mut Vec<(String, Span)>) {
        match self {
            Pattern::Binding(name, span) => out.push((name.clone(), *span)),
            Pattern::Variant { positional, named, .. } => {
                for pattern in positional {
                    pattern.bindings(out);
                }
                for (_, pattern) in named {
                    pattern.bindings(out);
                }
            }
            Pattern::Vector { items, rest, .. } => {
                for item in items {
                    item.bindings(out);
                }
                if let Some(Some(name)) = rest {
                    out.push((name.clone(), self.span()));
                }
            }
            Pattern::Map { entries, .. } => {
                for (_, pattern) in entries {
                    pattern.bindings(out);
                }
            }
            Pattern::Record { fields, .. } => {
                for (_, pattern) in fields {
                    pattern.bindings(out);
                }
            }
            Pattern::Typed { inner, .. } => inner.bindings(out),
            _ => {}
        }
    }

    /// True when the pattern always matches, which makes later arms unreachable.
    pub fn is_irrefutable(&self) -> bool {
        match self {
            Pattern::Wildcard(_) | Pattern::Binding(_, _) => true,
            Pattern::Typed { inner, .. } => inner.is_irrefutable(),
            _ => false,
        }
    }
}
