//! Lowering: syntax objects to the abstract syntax tree.
//!
//! Runs after macro expansion. Everything the compiler treats as a special form
//! is recognized here; anything else becomes a call. Errors are recovered from
//! aggressively so that one bad form does not hide the rest of the file.

// korben-6bc

use crate::ast::*;
use korben_syntax::diag::{Diagnostic, Diagnostics};
use korben_syntax::reader::{CommentKind, Datum, Syntax};
use korben_syntax::span::{FileId, Span};
use std::rc::Rc;

/// Special forms recognized directly by the compiler (specification section 7).
pub const SPECIAL_FORMS: &[&str] = &[
    "if",
    "let",
    "var",
    "set!",
    "fn",
    "async-fn",
    "match",
    "loop",
    "recur",
    "quote",
    "syntax-quote",
    "try",
    "throw",
    "with",
    "defer",
    "unsafe",
    "module",
    "use",
    "type",
    "enum",
    "protocol",
    "impl",
    "macro",
    "pub",
    "do",
    "async",
    "await",
    "task-scope",
    "test",
    "property",
    "derive",
    "def",
    "comptime",
    "propagate",
    "fn-shorthand",
];

pub struct Lowerer<'a> {
    pub diagnostics: &'a mut Diagnostics,
    file: FileId,
    lambda_counter: usize,
}

/// Lower a whole file into a module.
pub fn lower_module(
    file: FileId,
    default_name: &str,
    forms: &[Syntax],
    diagnostics: &mut Diagnostics,
) -> Module {
    let mut lowerer = Lowerer { diagnostics, file, lambda_counter: 0 };
    lowerer.module(default_name, forms)
}

impl<'a> Lowerer<'a> {
    fn error(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    fn module(&mut self, default_name: &str, forms: &[Syntax]) -> Module {
        let span = forms.first().map(|form| form.span).unwrap_or(Span::new(self.file, 0, 0));
        let mut module = Module {
            name: default_name.to_string(),
            file: self.file,
            imports: Vec::new(),
            items: Vec::new(),
            doc: None,
            span,
        };

        let mut pending_doc: Option<String> = None;
        for form in forms {
            if let Datum::Comment(kind, text) = &form.datum {
                if *kind == CommentKind::Doc {
                    let doc = pending_doc.get_or_insert_with(String::new);
                    if !doc.is_empty() {
                        doc.push('\n');
                    }
                    doc.push_str(text);
                }
                continue;
            }
            let doc = pending_doc.take();
            match form.head_symbol() {
                Some("module") => {
                    let items = form.as_list().unwrap();
                    match items.get(1).and_then(Syntax::as_symbol) {
                        Some(name) => module.name = name.to_string(),
                        None => self.error(
                            Diagnostic::error("module declaration needs a name")
                                .with_code("module-name")
                                .at(form.span, "expected `(module app.main ...)`"),
                        ),
                    }
                    module.doc = doc;
                    for inner in &items[2..] {
                        if inner.head_symbol() == Some("use") {
                            if let Some(import) = self.import(inner) {
                                module.imports.push(import);
                            }
                        } else {
                            self.error(
                                Diagnostic::error("only `use` forms may appear in a module header")
                                    .with_code("module-header")
                                    .at(inner.span, format!("found {}", inner.describe()))
                                    .help("move this declaration below the `(module ...)` form"),
                            );
                        }
                    }
                }
                Some("use") => {
                    if let Some(import) = self.import(form) {
                        module.imports.push(import);
                    }
                }
                _ => {
                    if let Some(item) = self.item(form, false, doc) {
                        module.items.push(item);
                    }
                }
            }
        }
        module
    }

    fn import(&mut self, form: &Syntax) -> Option<Import> {
        let items = form.as_list()?;
        let Some(path) = items.get(1).and_then(Syntax::as_symbol) else {
            self.error(
                Diagnostic::error("`use` needs a module path")
                    .with_code("import-path")
                    .at(form.span, "expected `(use std.io)`"),
            );
            return None;
        };
        let mut alias = path.rsplit('.').next().unwrap_or(path).to_string();
        let mut names = None;
        let mut index = 2usize;
        while index < items.len() {
            match &items[index].datum {
                Datum::Keyword(keyword) if keyword == "as" => {
                    match items.get(index + 1).and_then(Syntax::as_symbol) {
                        Some(name) => alias = name.to_string(),
                        None => self.error(
                            Diagnostic::error("`:as` needs an alias")
                                .with_code("import-alias")
                                .at(items[index].span, "expected `:as name`"),
                        ),
                    }
                    index += 2;
                }
                Datum::Keyword(keyword) if keyword == "only" => {
                    names = Some(self.name_list(items.get(index + 1)));
                    index += 2;
                }
                Datum::Vector(_) => {
                    names = Some(self.name_list(Some(&items[index])));
                    index += 1;
                }
                _ => {
                    self.error(
                        Diagnostic::error("unexpected form in `use`")
                            .with_code("import-form")
                            .at(items[index].span, format!("found {}", items[index].describe()))
                            .help("`use` accepts `:as name`, `:only [..]`, or `[..]`"),
                    );
                    index += 1;
                }
            }
        }
        Some(Import { path: path.to_string(), alias, names, span: form.span })
    }

    fn name_list(&mut self, form: Option<&Syntax>) -> Vec<String> {
        let Some(form) = form else { return Vec::new() };
        let Some(items) = form.as_vector() else {
            self.error(
                Diagnostic::error("expected a vector of names")
                    .with_code("import-names")
                    .at(form.span, format!("found {}", form.describe())),
            );
            return Vec::new();
        };
        let mut names = Vec::new();
        for item in items {
            match item.as_symbol() {
                Some(name) => names.push(name.to_string()),
                None => self.error(
                    Diagnostic::error("expected a name")
                        .with_code("import-names")
                        .at(item.span, format!("found {}", item.describe())),
                ),
            }
        }
        names
    }

    // ---------------------------------------------------------------- items

    fn item(&mut self, form: &Syntax, is_public: bool, doc: Option<String>) -> Option<Item> {
        let head = form.head_symbol()?.to_string();
        let items = form.as_list().unwrap();
        match head.as_str() {
            "pub" => {
                // `(pub fn name ...)` is one flat form, not a nested one.
                if items.len() < 2 {
                    self.error(
                        Diagnostic::error("`pub` needs a declaration")
                            .with_code("pub-form")
                            .at(form.span, "expected `(pub fn ...)`"),
                    );
                    return None;
                }
                let inner = Syntax::list(items[1..].to_vec(), form.span);
                self.item(&inner, true, doc)
            }
            "fn" | "async-fn" => {
                let decl = self.fn_decl(form, items, head == "async-fn", is_public, false, doc)?;
                Some(Item::Fn(Rc::new(decl)))
            }
            "async" if items.get(1).and_then(Syntax::as_symbol) == Some("fn") => {
                let inner: Vec<Syntax> = items[1..].to_vec();
                let decl = self.fn_decl(form, &inner, true, is_public, false, doc)?;
                Some(Item::Fn(Rc::new(decl)))
            }
            "unsafe" if items.get(1).and_then(Syntax::as_symbol) == Some("fn") => {
                let inner: Vec<Syntax> = items[1..].to_vec();
                let decl = self.fn_decl(form, &inner, false, is_public, true, doc)?;
                Some(Item::Fn(Rc::new(decl)))
            }
            "type" => {
                self.type_decl(form, items, is_public, doc).map(|decl| Item::Type(Rc::new(decl)))
            }
            "enum" => {
                self.enum_decl(form, items, is_public, doc).map(|decl| Item::Type(Rc::new(decl)))
            }
            "protocol" => self
                .protocol_decl(form, items, is_public, doc)
                .map(|decl| Item::Protocol(Rc::new(decl))),
            "impl" => self.impl_decl(form, items).map(|decl| Item::Impl(Rc::new(decl))),
            "macro" => {
                self.macro_decl(form, items, is_public, doc).map(|decl| Item::Macro(Rc::new(decl)))
            }
            "test" => self.test_decl(form, items, false).map(|decl| Item::Test(Rc::new(decl))),
            "property" => self.test_decl(form, items, true).map(|decl| Item::Test(Rc::new(decl))),
            "derive" => self.derive_decl(form, items).map(Item::Derive),
            "def" => {
                let Some(name) = items.get(1).and_then(Syntax::as_symbol) else {
                    self.error(
                        Diagnostic::error("`def` needs a name")
                            .with_code("def-name")
                            .at(form.span, "expected `(def name value)`"),
                    );
                    return None;
                };
                let (name, ty, rest) = self.split_annotation(name, &items[2..], form.span);
                let value = match rest.first() {
                    Some(value) => self.expr(value),
                    None => {
                        self.error(
                            Diagnostic::error("`def` needs a value")
                                .with_code("def-value")
                                .at(form.span, "expected `(def name value)`"),
                        );
                        Expr::Nil(form.span)
                    }
                };
                Some(Item::Const { name, ty, value, is_public, doc, span: form.span })
            }
            other => {
                self.error(
                    Diagnostic::error(format!("`{other}` is not a top-level declaration"))
                        .with_code("top-level-form")
                        .at(form.span, "expected a declaration")
                        .help("top level accepts fn, type, enum, protocol, impl, macro, def, derive, test, and use"),
                );
                None
            }
        }
    }

    fn fn_decl(
        &mut self,
        form: &Syntax,
        items: &[Syntax],
        is_async: bool,
        is_public: bool,
        is_unsafe: bool,
        doc: Option<String>,
    ) -> Option<FnDecl> {
        let Some(name) = items.get(1).and_then(Syntax::as_symbol) else {
            self.error(
                Diagnostic::error("function declaration needs a name")
                    .with_code("fn-name")
                    .at(form.span, "expected `(fn name [params] body)`"),
            );
            return None;
        };
        let mut index = 2usize;
        let params = match items.get(index) {
            Some(vector) if matches!(vector.datum, Datum::Vector(_)) => {
                index += 1;
                self.params(vector)
            }
            _ => {
                self.error(
                    Diagnostic::error("function declaration needs a parameter vector")
                        .with_code("fn-params")
                        .at(form.span, format!("`{name}` has no `[...]` parameter list")),
                );
                Vec::new()
            }
        };
        let (ret, effects) = self.return_annotation(items, &mut index);
        let body = self.body(&items[index..], form.span);
        Some(FnDecl {
            name: name.to_string(),
            params,
            ret,
            declared_effects: effects,
            body,
            is_async,
            is_public,
            is_unsafe,
            doc,
            span: form.span,
        })
    }

    /// Parse `-> Type !effect ...` if present.
    fn return_annotation(
        &mut self,
        items: &[Syntax],
        index: &mut usize,
    ) -> (Option<TypeExpr>, Effects) {
        let mut effects = Effects::NONE;
        let mut ret = None;
        if items.get(*index).and_then(Syntax::as_symbol) == Some("->") {
            *index += 1;
            ret = self.type_run(items, index);
            if ret.is_none() {
                let span = items.get(*index).map(|item| item.span).unwrap_or(Span::synthetic());
                self.error(
                    Diagnostic::error("`->` needs a return type")
                        .with_code("fn-return")
                        .at(span, "expected a type after `->`"),
                );
            }
        }
        while let Some(name) = items.get(*index).and_then(Syntax::as_symbol) {
            if !name.starts_with('!') {
                break;
            }
            match Effects::from_name(name) {
                Some(effect) => effects = effects.union(effect),
                None => {
                    let span = items[*index].span;
                    self.error(
                        Diagnostic::error(format!("unknown effect `{name}`"))
                            .with_code("unknown-effect")
                            .at(span, "not a recognized effect")
                            .help("effects are !io, !async, !alloc, !ffi, and !unsafe"),
                    );
                }
            }
            *index += 1;
        }
        (ret, effects)
    }

    fn params(&mut self, vector: &Syntax) -> Vec<Param> {
        let Some(items) = vector.as_vector() else { return Vec::new() };
        let items = fold_postfix(items);
        let mut params = Vec::new();
        let mut index = 0usize;
        let mut pending_keyword: Option<String> = None;
        while index < items.len() {
            let item = &items[index];
            if let Datum::Keyword(keyword) = &item.datum {
                pending_keyword = Some(keyword.clone());
                index += 1;
                continue;
            }
            let Some(raw) = item.as_symbol() else {
                self.error(
                    Diagnostic::error("expected a parameter name")
                        .with_code("param-name")
                        .at(item.span, format!("found {}", item.describe())),
                );
                index += 1;
                continue;
            };
            let span = item.span;
            index += 1;
            let (name, ty) = match raw.strip_suffix(':') {
                Some(name) => {
                    let annotated = self.type_run(&items, &mut index);
                    if annotated.is_none() {
                        self.error(
                            Diagnostic::error(format!("`{name}:` has no type"))
                                .with_code("param-type")
                                .at(span, "expected a type after the annotation colon"),
                        );
                    }
                    (name.to_string(), annotated)
                }
                None => (raw.to_string(), None),
            };
            let mut default = None;
            if items.get(index).and_then(Syntax::as_symbol) == Some("=") {
                index += 1;
                match items.get(index) {
                    Some(value) => {
                        default = Some(self.expr(value));
                        index += 1;
                    }
                    None => self.error(
                        Diagnostic::error("`=` needs a default value")
                            .with_code("param-default")
                            .at(span, "expected a value after `=`"),
                    ),
                }
            }
            params.push(Param { name, ty, keyword: pending_keyword.take(), default, span });
        }
        params
    }

    // ---------------------------------------------------------- type syntax

    /// Greedily read a type application such as `Result UInt16 ConfigError`.
    fn type_run(&mut self, items: &[Syntax], index: &mut usize) -> Option<TypeExpr> {
        let head = items.get(*index)?;
        if !is_type_atom(head) && !is_type_group(head) {
            return None;
        }
        let mut ty = self.type_single(head)?;
        *index += 1;
        // Only a bare name can take arguments by juxtaposition.
        if let TypeExpr::Name(name, args, span) = &mut ty {
            let mut end = *span;
            while let Some(next) = items.get(*index) {
                if !is_type_arg(next) {
                    break;
                }
                // A parenthesized form in final position is the body, not a
                // type argument: `(fn f [] -> Greeting (Greeting { .. }))`.
                if !matches!(next.datum, Datum::Symbol(_)) && *index + 1 >= items.len() {
                    break;
                }
                let Some(arg) = self.type_single(next) else { break };
                end = end.to(next.span);
                args.push(arg);
                *index += 1;
            }
            let _ = name;
            *span = end;
        }
        Some(ty)
    }

    fn type_single(&mut self, form: &Syntax) -> Option<TypeExpr> {
        match &form.datum {
            Datum::Symbol(name) => Some(TypeExpr::Name(name.clone(), Vec::new(), form.span)),
            Datum::Vector(items) => {
                let mut parts = Vec::new();
                for item in items {
                    parts.push(self.type_single(item)?);
                }
                Some(TypeExpr::Tuple(parts, form.span))
            }
            Datum::Map(items) => {
                let mut fields = Vec::new();
                let entries = fold_postfix(items);
                let mut index = 0usize;
                while index < entries.len() {
                    let key = &entries[index];
                    index += 1;
                    let Some(raw) = key.as_symbol() else {
                        self.error(
                            Diagnostic::error("expected a field name")
                                .with_code("record-field")
                                .at(key.span, format!("found {}", key.describe()))
                                .help("write `{ name: Type ... }`"),
                        );
                        continue;
                    };
                    let name = raw.strip_suffix(':').unwrap_or(raw).to_string();
                    match self.type_run(&entries, &mut index) {
                        Some(ty) => fields.push((name, ty)),
                        None => self.error(
                            Diagnostic::error(format!("field `{name}` has no type"))
                                .with_code("record-field")
                                .at(key.span, "expected a type after this field name"),
                        ),
                    }
                }
                Some(TypeExpr::Record(fields, form.span))
            }
            Datum::List(items) => {
                if items.first().and_then(Syntax::as_symbol) == Some("->") {
                    // `(-> [A B] C !io)`
                    let params = match items.get(1).and_then(Syntax::as_vector) {
                        Some(vector) => vector
                            .iter()
                            .filter_map(|item| self.type_single(item))
                            .collect::<Vec<_>>(),
                        None => Vec::new(),
                    };
                    let mut index = 2usize;
                    let ret = self.type_run(items, &mut index).unwrap_or(TypeExpr::Name(
                        "Unit".to_string(),
                        Vec::new(),
                        form.span,
                    ));
                    let mut effects = Effects::NONE;
                    while let Some(name) = items.get(index).and_then(Syntax::as_symbol) {
                        if let Some(effect) = Effects::from_name(name) {
                            effects = effects.union(effect);
                        }
                        index += 1;
                    }
                    return Some(TypeExpr::Fn(params, Box::new(ret), effects, form.span));
                }
                let mut index = 0usize;
                self.type_run(items, &mut index)
            }
            _ => {
                self.error(
                    Diagnostic::error("expected a type")
                        .with_code("type-syntax")
                        .at(form.span, format!("found {}", form.describe())),
                );
                None
            }
        }
    }

    fn type_decl(
        &mut self,
        form: &Syntax,
        items: &[Syntax],
        is_public: bool,
        doc: Option<String>,
    ) -> Option<TypeDecl> {
        let Some(name) = items.get(1).and_then(Syntax::as_symbol) else {
            self.error(
                Diagnostic::error("`type` needs a name")
                    .with_code("type-name")
                    .at(form.span, "expected `(type Name ...)`"),
            );
            return None;
        };
        let mut index = 2usize;
        let mut params = Vec::new();
        while let Some(param) = items.get(index).and_then(Syntax::as_symbol) {
            if !is_type_parameter(param) {
                break;
            }
            params.push(param.to_string());
            index += 1;
        }
        let Some(body_form) = items.get(index) else {
            self.error(
                Diagnostic::error(format!("type `{name}` has no definition"))
                    .with_code("type-body")
                    .at(form.span, "expected a record, enum, newtype, or alias body"),
            );
            return None;
        };
        let body = match &body_form.datum {
            Datum::Map(_) => match self.type_single(body_form)? {
                TypeExpr::Record(fields, span) => TypeBody::Record(
                    fields.into_iter().map(|(name, ty)| (name, ty, span)).collect(),
                ),
                other => TypeBody::Alias(other),
            },
            Datum::List(inner) if inner.first().and_then(Syntax::as_symbol) == Some("newtype") => {
                let mut index = 1usize;
                let ty = self.type_run(inner, &mut index)?;
                TypeBody::Newtype(ty)
            }
            Datum::List(inner) if inner.first().and_then(Syntax::as_symbol) == Some("enum") => {
                TypeBody::Enum(self.variants(&inner[1..]))
            }
            _ => TypeBody::Alias(self.type_single(body_form)?),
        };
        Some(TypeDecl { name: name.to_string(), params, body, is_public, doc, span: form.span })
    }

    fn enum_decl(
        &mut self,
        form: &Syntax,
        items: &[Syntax],
        is_public: bool,
        doc: Option<String>,
    ) -> Option<TypeDecl> {
        let Some(name) = items.get(1).and_then(Syntax::as_symbol) else {
            self.error(
                Diagnostic::error("`enum` needs a name")
                    .with_code("enum-name")
                    .at(form.span, "expected `(enum Name (Variant ...) ...)`"),
            );
            return None;
        };
        let mut index = 2usize;
        let mut params = Vec::new();
        while let Some(param) = items.get(index).and_then(Syntax::as_symbol) {
            if !is_type_parameter(param) {
                break;
            }
            params.push(param.to_string());
            index += 1;
        }
        let variants = self.variants(&items[index..]);
        Some(TypeDecl {
            name: name.to_string(),
            params,
            body: TypeBody::Enum(variants),
            is_public,
            doc,
            span: form.span,
        })
    }

    fn variants(&mut self, forms: &[Syntax]) -> Vec<VariantDecl> {
        let mut variants = Vec::new();
        for form in forms {
            let Some(items) = form.as_list() else {
                self.error(
                    Diagnostic::error("expected a variant")
                        .with_code("enum-variant")
                        .at(form.span, format!("found {}", form.describe()))
                        .help("write `(Variant field: Type ...)`"),
                );
                continue;
            };
            let Some(name) = items.first().and_then(Syntax::as_symbol) else {
                self.error(
                    Diagnostic::error("variant needs a name")
                        .with_code("enum-variant")
                        .at(form.span, "expected a constructor name"),
                );
                continue;
            };
            let items = fold_postfix(items);
            let mut fields = Vec::new();
            let mut index = 1usize;
            while index < items.len() {
                let key = &items[index];
                index += 1;
                let Some(raw) = key.as_symbol() else {
                    self.error(
                        Diagnostic::error("expected a field name")
                            .with_code("enum-variant")
                            .at(key.span, format!("found {}", key.describe())),
                    );
                    continue;
                };
                let field = raw.strip_suffix(':').unwrap_or(raw).to_string();
                match self.type_run(&items, &mut index) {
                    Some(ty) => fields.push((field, ty, key.span)),
                    None => self.error(
                        Diagnostic::error(format!("variant field `{field}` has no type"))
                            .with_code("enum-variant")
                            .at(key.span, "expected a type after this field name"),
                    ),
                }
            }
            variants.push(VariantDecl { name: name.to_string(), fields, span: form.span });
        }
        variants
    }

    fn protocol_decl(
        &mut self,
        form: &Syntax,
        items: &[Syntax],
        is_public: bool,
        doc: Option<String>,
    ) -> Option<ProtocolDecl> {
        let Some(name) = items.get(1).and_then(Syntax::as_symbol) else {
            self.error(
                Diagnostic::error("`protocol` needs a name")
                    .with_code("protocol-name")
                    .at(form.span, "expected `(protocol Name (method [self] -> T))`"),
            );
            return None;
        };
        let mut methods = Vec::new();
        for method_form in &items[2..] {
            let Some(parts) = method_form.as_list() else {
                self.error(
                    Diagnostic::error("expected a method signature")
                        .with_code("protocol-method")
                        .at(method_form.span, format!("found {}", method_form.describe())),
                );
                continue;
            };
            let Some(method_name) = parts.first().and_then(Syntax::as_symbol) else {
                self.error(
                    Diagnostic::error("method needs a name")
                        .with_code("protocol-method")
                        .at(method_form.span, "expected a method name"),
                );
                continue;
            };
            let params = match parts.get(1) {
                Some(vector) if matches!(vector.datum, Datum::Vector(_)) => self.params(vector),
                _ => {
                    self.error(
                        Diagnostic::error(format!(
                            "method `{method_name}` needs a parameter vector"
                        ))
                        .with_code("protocol-method")
                        .at(method_form.span, "expected `[self ...]`"),
                    );
                    Vec::new()
                }
            };
            let mut index = 2usize;
            let (ret, effects) = self.return_annotation(parts, &mut index);
            methods.push(ProtocolMethod {
                name: method_name.to_string(),
                params,
                ret,
                effects,
                span: method_form.span,
            });
        }
        Some(ProtocolDecl { name: name.to_string(), methods, is_public, doc, span: form.span })
    }

    fn impl_decl(&mut self, form: &Syntax, items: &[Syntax]) -> Option<ImplDecl> {
        let protocol = items.get(1).and_then(Syntax::as_symbol);
        let type_name = items.get(2).and_then(Syntax::as_symbol);
        let (Some(protocol), Some(type_name)) = (protocol, type_name) else {
            self.error(
                Diagnostic::error("`impl` needs a protocol and a type")
                    .with_code("impl-header")
                    .at(form.span, "expected `(impl Protocol Type (fn method [..] ..))`"),
            );
            return None;
        };
        let mut methods = Vec::new();
        for method_form in &items[3..] {
            let Some(parts) = method_form.as_list() else {
                self.error(
                    Diagnostic::error("expected a method definition")
                        .with_code("impl-method")
                        .at(method_form.span, format!("found {}", method_form.describe())),
                );
                continue;
            };
            if parts.first().and_then(Syntax::as_symbol) != Some("fn") {
                self.error(
                    Diagnostic::error("expected a `fn` definition")
                        .with_code("impl-method")
                        .at(method_form.span, "protocol implementations contain `fn` forms"),
                );
                continue;
            }
            // A method's public surface is the protocol's, not its own.
            if let Some(decl) = self.fn_decl(method_form, parts, false, false, false, None) {
                methods.push(decl);
            }
        }
        Some(ImplDecl {
            protocol: protocol.to_string(),
            type_name: type_name.to_string(),
            methods,
            span: form.span,
        })
    }

    fn macro_decl(
        &mut self,
        form: &Syntax,
        items: &[Syntax],
        is_public: bool,
        doc: Option<String>,
    ) -> Option<MacroDecl> {
        let Some(name) = items.get(1).and_then(Syntax::as_symbol) else {
            self.error(
                Diagnostic::error("`macro` needs a name")
                    .with_code("macro-name")
                    .at(form.span, "expected `(macro name [params] body)`"),
            );
            return None;
        };
        let Some(param_forms) = items.get(2).and_then(Syntax::as_vector) else {
            self.error(
                Diagnostic::error(format!("macro `{name}` needs a parameter vector"))
                    .with_code("macro-params")
                    .at(form.span, "expected `[params]`"),
            );
            return None;
        };
        let mut params = Vec::new();
        let mut rest = None;
        for param in param_forms {
            let Some(text) = param.as_symbol() else {
                self.error(
                    Diagnostic::error("expected a macro parameter name")
                        .with_code("macro-params")
                        .at(param.span, format!("found {}", param.describe())),
                );
                continue;
            };
            match text.strip_prefix("...") {
                Some(name) => rest = Some(name.to_string()),
                None => params.push(text.to_string()),
            }
        }
        Some(MacroDecl {
            name: name.to_string(),
            params,
            rest,
            body: items[3..].to_vec(),
            is_public,
            doc,
            span: form.span,
        })
    }

    fn test_decl(
        &mut self,
        form: &Syntax,
        items: &[Syntax],
        is_property: bool,
    ) -> Option<TestDecl> {
        let Some(name) = items.get(1).and_then(Syntax::as_str) else {
            self.error(
                Diagnostic::error("a test needs a name string")
                    .with_code("test-name")
                    .at(form.span, "expected `(test \"description\" ...)`"),
            );
            return None;
        };
        let mut index = 2usize;
        let mut generators = Vec::new();
        if is_property {
            match items.get(index).and_then(Syntax::as_vector) {
                Some(bindings) => {
                    index += 1;
                    for pair in bindings.chunks(2) {
                        match pair {
                            [name_form, generator] => {
                                if let Some(binding) = name_form.as_symbol() {
                                    generators.push((binding.to_string(), self.expr(generator)));
                                }
                            }
                            [name_form] => self.error(
                                Diagnostic::error("property generator is missing")
                                    .with_code("property-generators")
                                    .at(name_form.span, "expected `[name generator ...]`"),
                            ),
                            _ => unreachable!(),
                        }
                    }
                }
                None => self.error(
                    Diagnostic::error("a property test needs a generator vector")
                        .with_code("property-generators")
                        .at(form.span, "expected `(property \"...\" [value gen] ...)`"),
                ),
            }
        }
        let body = self.body(&items[index..], form.span);
        Some(TestDecl { name: name.to_string(), generators, body, span: form.span })
    }

    fn derive_decl(&mut self, form: &Syntax, items: &[Syntax]) -> Option<DeriveDecl> {
        let Some(type_name) = items.get(1).and_then(Syntax::as_symbol) else {
            self.error(
                Diagnostic::error("`derive` needs a type name")
                    .with_code("derive-type")
                    .at(form.span, "expected `(derive User [Eq Hash])`"),
            );
            return None;
        };
        let protocols = self.name_list(items.get(2));
        Some(DeriveDecl { type_name: type_name.to_string(), protocols, span: form.span })
    }

    // ---------------------------------------------------------------- bodies

    fn body(&mut self, forms: &[Syntax], span: Span) -> Body {
        let forms = fold_postfix(forms);
        let mut stmts = Vec::new();
        for form in &forms {
            if form.is_comment() {
                continue;
            }
            match form.head_symbol() {
                Some("let") => stmts.push(self.let_stmt(form)),
                Some("var") => stmts.push(self.var_stmt(form)),
                Some("defer") => {
                    let items = form.as_list().unwrap();
                    let inner = self.body(&items[1..], form.span);
                    stmts.push(Stmt::Defer { body: inner, span: form.span });
                }
                _ => stmts.push(Stmt::Expr(self.expr(form))),
            }
        }
        Body { stmts, span }
    }

    fn let_stmt(&mut self, form: &Syntax) -> Stmt {
        let items = form.as_list().unwrap();
        let items = fold_postfix(items);
        let Some(target) = items.get(1) else {
            self.error(
                Diagnostic::error("`let` needs a binding")
                    .with_code("let-form")
                    .at(form.span, "expected `(let name value)`"),
            );
            return Stmt::Expr(Expr::Nil(form.span));
        };
        // `(let name: Type value)` or `(let pattern value)`.
        if let Some(raw) = target.as_symbol() {
            let mut index = 2usize;
            let (name, ty, _) = {
                let (name, ty, _) = self.split_annotation(raw, &[], form.span);
                if raw.ends_with(':') {
                    let annotated = self.type_run(&items, &mut index);
                    (name, annotated, ())
                } else {
                    (name, ty, ())
                }
            };
            let value = match items.get(index) {
                Some(value) => self.expr(value),
                None => {
                    self.error(
                        Diagnostic::error("`let` needs a value")
                            .with_code("let-form")
                            .at(form.span, format!("`{name}` is not given a value")),
                    );
                    Expr::Nil(form.span)
                }
            };
            return Stmt::Let {
                pattern: Pattern::Binding(name, target.span),
                ty,
                value,
                span: form.span,
            };
        }
        let pattern = self.pattern(target);
        let value = match items.get(2) {
            Some(value) => self.expr(value),
            None => {
                self.error(
                    Diagnostic::error("`let` needs a value")
                        .with_code("let-form")
                        .at(form.span, "expected `(let pattern value)`"),
                );
                Expr::Nil(form.span)
            }
        };
        Stmt::Let { pattern, ty: None, value, span: form.span }
    }

    fn var_stmt(&mut self, form: &Syntax) -> Stmt {
        let items = form.as_list().unwrap();
        let items = fold_postfix(items);
        let Some(raw) = items.get(1).and_then(Syntax::as_symbol) else {
            self.error(
                Diagnostic::error("`var` needs a name")
                    .with_code("var-form")
                    .at(form.span, "expected `(var name value)`"),
            );
            return Stmt::Expr(Expr::Nil(form.span));
        };
        let mut index = 2usize;
        let name = raw.strip_suffix(':').unwrap_or(raw).to_string();
        let ty = if raw.ends_with(':') { self.type_run(&items, &mut index) } else { None };
        let value = match items.get(index) {
            Some(value) => self.expr(value),
            None => {
                self.error(
                    Diagnostic::error("`var` needs an initial value")
                        .with_code("var-form")
                        .at(form.span, format!("`{name}` is not given a value")),
                );
                Expr::Nil(form.span)
            }
        };
        Stmt::Var { name, ty, value, span: form.span }
    }

    fn split_annotation(
        &mut self,
        raw: &str,
        _rest: &[Syntax],
        _span: Span,
    ) -> (String, Option<TypeExpr>, Vec<Syntax>) {
        (raw.strip_suffix(':').unwrap_or(raw).to_string(), None, Vec::new())
    }

    // ----------------------------------------------------------- expressions

    pub fn expr(&mut self, form: &Syntax) -> Expr {
        let span = form.span;
        match &form.datum {
            Datum::Nil => Expr::Nil(span),
            Datum::Bool(value) => Expr::Bool(*value, span),
            Datum::Int(value) => Expr::Int(*value, span),
            Datum::Float(value) => Expr::Float(*value, span),
            Datum::Str(value) => Expr::Str(value.clone(), span),
            Datum::Keyword(name) => Expr::Keyword(name.clone(), span),
            Datum::Symbol(name) => self.symbol_expr(name, span),
            Datum::Vector(items) => {
                let items = fold_postfix(items);
                Expr::Vector(items.iter().map(|item| self.expr(item)).collect(), span)
            }
            Datum::Set(items) => {
                let items = fold_postfix(items);
                Expr::Set(items.iter().map(|item| self.expr(item)).collect(), span)
            }
            Datum::Map(items) => self.map_or_record(items, span),
            Datum::Tagged(tag, inner) => {
                // `#uuid "..."` becomes a call to the tag's constructor.
                Expr::Call {
                    callee: Box::new(Expr::Var(format!("{tag}/parse"), span)),
                    args: vec![Arg { keyword: None, value: self.expr(inner), span: inner.span }],
                    span,
                }
            }
            Datum::Comment(..) => Expr::Nil(span),
            Datum::List(items) => self.list_expr(items, span),
        }
    }

    fn symbol_expr(&mut self, name: &str, span: Span) -> Expr {
        // `module/name` is a qualified reference; a bare `/` is the division
        // function, and `a/` or `/b` are ordinary symbols.
        if let Some((module, rest)) = name.split_once('/') {
            if !module.is_empty() && !rest.is_empty() {
                return Expr::Path { module: module.to_string(), name: rest.to_string(), span };
            }
        }
        if name.contains('.') && !name.starts_with('.') {
            let mut parts = name.split('.');
            let root = parts.next().unwrap();
            let mut expr = Expr::Var(root.to_string(), span);
            for part in parts {
                expr = Expr::Field { target: Box::new(expr), name: part.to_string(), span };
            }
            return expr;
        }
        Expr::Var(name.to_string(), span)
    }

    /// `{...}` is a map when its keys are keywords and a record when they are symbols.
    fn map_or_record(&mut self, items: &[Syntax], span: Span) -> Expr {
        let items = fold_postfix(items);
        if !items.len().is_multiple_of(2) {
            self.error(
                Diagnostic::error("map literal has an odd number of forms")
                    .with_code("map-literal")
                    .at(span, "every key needs a value"),
            );
        }
        let is_record = items
            .chunks(2)
            .filter_map(|pair| pair.first())
            .all(|key| matches!(key.datum, Datum::Symbol(_)));
        if is_record && !items.is_empty() {
            let mut fields = Vec::new();
            for pair in items.chunks(2) {
                let [key, value] = pair else { continue };
                let raw = key.as_symbol().unwrap();
                let name = raw.strip_suffix(':').unwrap_or(raw).to_string();
                fields.push((name, self.expr(value), key.span));
            }
            return Expr::Record { type_name: None, fields, span };
        }
        let mut entries = Vec::new();
        for pair in items.chunks(2) {
            let [key, value] = pair else { continue };
            entries.push((self.expr(key), self.expr(value)));
        }
        Expr::Map(entries, span)
    }

    fn list_expr(&mut self, raw_items: &[Syntax], span: Span) -> Expr {
        if raw_items.is_empty() {
            return Expr::Nil(span);
        }
        let items = fold_postfix(raw_items);
        let head = items[0].as_symbol().map(str::to_string);
        if let Some(head) = head.as_deref() {
            if let Some(expr) = self.special_form(head, &items, span) {
                return expr;
            }
            // `(.field target args...)` is field access or a method call.
            // A dotted name such as `.a.b` chains field accesses.
            if let Some(path) = head.strip_prefix('.') {
                if items.len() >= 2 && !path.is_empty() {
                    let mut access = self.expr(&items[1]);
                    for field in path.split('.') {
                        access =
                            Expr::Field { target: Box::new(access), name: field.to_string(), span };
                    }
                    if items.len() == 2 {
                        return access;
                    }
                    let args = self.args(&items[2..]);
                    return Expr::Call { callee: Box::new(access), args, span };
                }
            }
        }

        let callee = self.expr(&items[0]);
        // `(user.name)` and `(process.args)` are both zero-argument calls; the
        // evaluator decides whether the target is a field or a module member.
        let args = self.args(&items[1..]);
        Expr::Call { callee: Box::new(callee), args, span }
    }

    /// Collect call arguments, recording which were written as `:name value`.
    fn args(&mut self, forms: &[Syntax]) -> Vec<Arg> {
        let mut args = Vec::new();
        let mut index = 0usize;
        while index < forms.len() {
            let form = &forms[index];
            if let Datum::Keyword(keyword) = &form.datum {
                if index + 1 < forms.len() {
                    let value = self.expr(&forms[index + 1]);
                    args.push(Arg {
                        keyword: Some(keyword.clone()),
                        value,
                        span: form.span.to(forms[index + 1].span),
                    });
                    index += 2;
                    continue;
                }
            }
            args.push(Arg { keyword: None, value: self.expr(form), span: form.span });
            index += 1;
        }
        args
    }

    fn special_form(&mut self, head: &str, items: &[Syntax], span: Span) -> Option<Expr> {
        match head {
            "if" => {
                let cond = self.expr(items.get(1)?);
                let then = match items.get(2) {
                    Some(form) => self.expr(form),
                    None => {
                        self.error(
                            Diagnostic::error("`if` needs a then-branch")
                                .with_code("if-form")
                                .at(span, "expected `(if condition then else)`"),
                        );
                        Expr::Nil(span)
                    }
                };
                let els = items.get(3).map(|form| Box::new(self.expr(form)));
                if items.len() > 4 {
                    self.error(
                        Diagnostic::error("`if` takes at most three forms")
                            .with_code("if-form")
                            .at(items[4].span, "unexpected extra form")
                            .help("wrap multiple expressions in `(do ...)`"),
                    );
                }
                Some(Expr::If { cond: Box::new(cond), then: Box::new(then), els, span })
            }
            "do" => Some(Expr::Do(Box::new(self.body(&items[1..], span)), span)),
            // `and` and `or` short-circuit on truthiness and may yield operands
            // of different types, which no macro expanding to `if` can express.
            "and" => Some(Expr::And(items[1..].iter().map(|item| self.expr(item)).collect(), span)),
            "or" => Some(Expr::Or(items[1..].iter().map(|item| self.expr(item)).collect(), span)),
            "fn" | "async-fn" => {
                let is_async = head == "async-fn";
                // Anonymous when the first form is a parameter vector.
                if matches!(items.get(1).map(|item| &item.datum), Some(Datum::Vector(_))) {
                    self.lambda_counter += 1;
                    let name = format!("fn#{}", self.lambda_counter);
                    let params = self.params(&items[1]);
                    let mut index = 2usize;
                    let (ret, effects) = self.return_annotation(items, &mut index);
                    let body = self.body(&items[index..], span);
                    return Some(Expr::Lambda(
                        Rc::new(FnDecl {
                            name,
                            params,
                            ret,
                            declared_effects: effects,
                            body,
                            is_async,
                            is_public: false,
                            is_unsafe: false,
                            doc: None,
                            span,
                        }),
                        span,
                    ));
                }
                let decl = self.fn_decl(
                    &Syntax::list(items.to_vec(), span),
                    items,
                    is_async,
                    false,
                    false,
                    None,
                )?;
                Some(Expr::Lambda(Rc::new(decl), span))
            }
            "fn-shorthand" => Some(self.shorthand_lambda(&items[1..], span)),
            "match" => {
                let scrutinee = self.expr(items.get(1)?);
                let mut arms = Vec::new();
                let mut index = 2usize;
                while index < items.len() {
                    let pattern = self.pattern(&items[index]);
                    index += 1;
                    let mut guard = None;
                    if items.get(index).and_then(Syntax::as_keyword) == Some("when") {
                        match items.get(index + 1) {
                            Some(form) => {
                                guard = Some(self.expr(form));
                                index += 2;
                            }
                            None => {
                                self.error(
                                    Diagnostic::error("`:when` needs a guard expression")
                                        .with_code("match-guard")
                                        .at(
                                            items[index].span,
                                            "expected an expression after `:when`",
                                        ),
                                );
                                index += 1;
                            }
                        }
                    }
                    let arm_span = pattern.span();
                    match items.get(index) {
                        Some(form) => {
                            let body = self.expr(form);
                            index += 1;
                            arms.push(MatchArm { pattern, guard, body, span: arm_span });
                        }
                        None => {
                            self.error(
                                Diagnostic::error("match arm has no result expression")
                                    .with_code("match-arm")
                                    .at(arm_span, "this pattern has no body"),
                            );
                        }
                    }
                }
                Some(Expr::Match { scrutinee: Box::new(scrutinee), arms, span })
            }
            "loop" => {
                let Some(binding_forms) = items.get(1).and_then(Syntax::as_vector) else {
                    self.error(
                        Diagnostic::error("`loop` needs a binding vector")
                            .with_code("loop-form")
                            .at(span, "expected `(loop [name value ...] body)`"),
                    );
                    return Some(Expr::Nil(span));
                };
                let binding_forms = fold_postfix(binding_forms);
                let mut bindings = Vec::new();
                for pair in binding_forms.chunks(2) {
                    match pair {
                        [name_form, value] => match name_form.as_symbol() {
                            Some(name) => bindings.push((name.to_string(), self.expr(value))),
                            None => self.error(
                                Diagnostic::error("expected a loop binding name")
                                    .with_code("loop-form")
                                    .at(name_form.span, format!("found {}", name_form.describe())),
                            ),
                        },
                        [name_form] => self.error(
                            Diagnostic::error("loop binding has no value")
                                .with_code("loop-form")
                                .at(name_form.span, "expected a value"),
                        ),
                        _ => unreachable!(),
                    }
                }
                let body = self.body(&items[2..], span);
                Some(Expr::Loop { bindings, body: Box::new(body), span })
            }
            "recur" => {
                let args = items[1..].iter().map(|item| self.expr(item)).collect();
                Some(Expr::Recur(args, span))
            }
            "set!" => {
                let Some(name) = items.get(1).and_then(Syntax::as_symbol) else {
                    self.error(
                        Diagnostic::error("`set!` needs a variable name")
                            .with_code("set-form")
                            .at(span, "expected `(set! name value)`"),
                    );
                    return Some(Expr::Nil(span));
                };
                let value = match items.get(2) {
                    Some(form) => self.expr(form),
                    None => {
                        self.error(
                            Diagnostic::error("`set!` needs a value")
                                .with_code("set-form")
                                .at(span, "expected `(set! name value)`"),
                        );
                        Expr::Nil(span)
                    }
                };
                Some(Expr::Assign { name: name.to_string(), value: Box::new(value), span })
            }
            "propagate" => Some(Expr::Propagate(Box::new(self.expr(items.get(1)?)), span)),
            "throw" => Some(Expr::Throw(Box::new(self.expr(items.get(1)?)), span)),
            "await" => Some(Expr::Await(Box::new(self.expr(items.get(1)?)), span)),
            "unsafe" => Some(Expr::Unsafe(Box::new(self.body(&items[1..], span)), span)),
            "async" => Some(Expr::TaskScope {
                name: "async".to_string(),
                body: Box::new(self.body(&items[1..], span)),
                span,
            }),
            "task-scope" => {
                let name = items.get(1).and_then(Syntax::as_symbol).unwrap_or("scope").to_string();
                let body = self.body(&items[2..], span);
                Some(Expr::TaskScope { name, body: Box::new(body), span })
            }
            "with" => {
                let Some(name) = items.get(1).and_then(Syntax::as_symbol) else {
                    self.error(
                        Diagnostic::error("`with` needs a binding name")
                            .with_code("with-form")
                            .at(span, "expected `(with name resource body)`"),
                    );
                    return Some(Expr::Nil(span));
                };
                let value = match items.get(2) {
                    Some(form) => self.expr(form),
                    None => {
                        self.error(
                            Diagnostic::error("`with` needs a resource expression")
                                .with_code("with-form")
                                .at(span, "expected `(with name resource body)`"),
                        );
                        Expr::Nil(span)
                    }
                };
                let body = self.body(&items[3..], span);
                Some(Expr::With {
                    name: name.to_string(),
                    value: Box::new(value),
                    body: Box::new(body),
                    span,
                })
            }
            "try" => {
                let mut body_forms = Vec::new();
                let mut catches = Vec::new();
                let mut finally = None;
                for form in &items[1..] {
                    match form.head_symbol() {
                        Some("catch") => {
                            let parts = form.as_list().unwrap();
                            let condition = parts
                                .get(1)
                                .and_then(Syntax::as_symbol)
                                .unwrap_or("Condition")
                                .to_string();
                            let binding = parts
                                .get(2)
                                .and_then(Syntax::as_symbol)
                                .unwrap_or("condition")
                                .to_string();
                            let body = self.body(&parts[3..], form.span);
                            catches.push(CatchArm { condition, binding, body, span: form.span });
                        }
                        Some("finally") => {
                            let parts = form.as_list().unwrap();
                            finally = Some(Box::new(self.body(&parts[1..], form.span)));
                        }
                        _ => body_forms.push(form.clone()),
                    }
                }
                let body = self.body(&body_forms, span);
                Some(Expr::Try { body: Box::new(body), catches, finally, span })
            }
            "quote" => Some(Expr::Quote(Rc::new(items.get(1)?.clone()), span)),
            "syntax-quote" => {
                let template = self.template(items.get(1)?);
                Some(Expr::SyntaxQuote(Rc::new(template), span))
            }
            "format" => {
                // `(format "hello {name}")` splits into interpolation segments.
                let template = items.get(1).and_then(Syntax::as_str)?;
                let parts = self.interpolate(template, items[1].span);
                if items.len() > 2 {
                    // Extra arguments mean this is a positional format call instead.
                    return None;
                }
                Some(Expr::Interp(parts, span))
            }
            "let" | "var" | "defer" => {
                // A `let`/`var`/`defer` used in expression position wraps in `do`.
                let body =
                    self.body(std::slice::from_ref(&Syntax::list(items.to_vec(), span)), span);
                Some(Expr::Do(Box::new(body), span))
            }
            _ => None,
        }
    }

    /// Lower a syntax-quote template, pre-lowering `~` and `~@` holes.
    fn template(&mut self, form: &Syntax) -> Template {
        let span = form.span;
        match &form.datum {
            Datum::List(items) => {
                if items.len() == 2 {
                    match items[0].as_symbol() {
                        Some("unquote") => return Template::Unquote(self.expr(&items[1])),
                        Some("unquote-splice") => return Template::Splice(self.expr(&items[1])),
                        _ => {}
                    }
                }
                Template::List(items.iter().map(|item| self.template(item)).collect(), span)
            }
            Datum::Vector(items) => {
                Template::Vector(items.iter().map(|item| self.template(item)).collect(), span)
            }
            Datum::Map(items) => {
                Template::Map(items.iter().map(|item| self.template(item)).collect(), span)
            }
            Datum::Set(items) => {
                Template::Set(items.iter().map(|item| self.template(item)).collect(), span)
            }
            _ => Template::Literal(form.clone()),
        }
    }

    /// `#(...)` — collect `%`, `%1`, `%2` into a parameter list.
    fn shorthand_lambda(&mut self, items: &[Syntax], span: Span) -> Expr {
        let mut arity = 0usize;
        collect_shorthand_arity(items, &mut arity);
        let params: Vec<Param> = (1..=arity)
            .map(|index| Param {
                name: if index == 1 { "%".to_string() } else { format!("%{index}") },
                ty: None,
                keyword: None,
                default: None,
                span,
            })
            .collect();
        let body = self.body(&[Syntax::list(items.to_vec(), span)], span);
        self.lambda_counter += 1;
        Expr::Lambda(
            Rc::new(FnDecl {
                name: format!("fn#{}", self.lambda_counter),
                params,
                ret: None,
                declared_effects: Effects::NONE,
                body,
                is_async: false,
                is_public: false,
                is_unsafe: false,
                doc: None,
                span,
            }),
            span,
        )
    }

    /// Split `"Hello, {name}"` into literal text and embedded expressions.
    fn interpolate(&mut self, template: &str, span: Span) -> Vec<InterpPart> {
        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut chars = template.char_indices().peekable();
        while let Some((offset, ch)) = chars.next() {
            // `{{` and `}}` are literal braces, as in a Rust format string.
            if ch == '}' && chars.peek().map(|(_, next)| *next) == Some('}') {
                chars.next();
                literal.push('}');
                continue;
            }
            if ch != '{' {
                literal.push(ch);
                continue;
            }
            if chars.peek().map(|(_, next)| *next) == Some('{') {
                chars.next();
                literal.push('{');
                continue;
            }
            let mut depth = 1usize;
            let mut source = String::new();
            let mut closed = false;
            for (_, inner) in chars.by_ref() {
                match inner {
                    '{' => {
                        depth += 1;
                        source.push(inner);
                    }
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            closed = true;
                            break;
                        }
                        source.push(inner);
                    }
                    _ => source.push(inner),
                }
            }
            if !closed {
                self.error(
                    Diagnostic::error("unclosed interpolation in string")
                        .with_code("format-string")
                        .at(span, format!("`{{` at offset {offset} is never closed"))
                        .help("write `\\{` for a literal brace"),
                );
                literal.push('{');
                literal.push_str(&source);
                continue;
            }
            if !literal.is_empty() {
                parts.push(InterpPart::Text(std::mem::take(&mut literal)));
            }
            let (forms, errors) =
                korben_syntax::read_all(span.file, &source, korben_syntax::Comments::Skip);
            for error in errors {
                self.error(error);
            }
            match forms.first() {
                Some(form) => {
                    // Spans inside an interpolation point at the whole literal.
                    let mut rewritten = form.clone();
                    retarget_spans(&mut rewritten, span);
                    parts.push(InterpPart::Expr(self.expr(&rewritten)));
                }
                None => self.error(
                    Diagnostic::error("empty interpolation in string")
                        .with_code("format-string")
                        .at(span, "`{}` needs an expression"),
                ),
            }
        }
        if !literal.is_empty() {
            parts.push(InterpPart::Text(literal));
        }
        parts
    }

    // -------------------------------------------------------------- patterns

    pub fn pattern(&mut self, form: &Syntax) -> Pattern {
        let span = form.span;
        match &form.datum {
            Datum::Nil => Pattern::Nil(span),
            Datum::Bool(value) => Pattern::Bool(*value, span),
            Datum::Int(value) => Pattern::Int(*value, span),
            Datum::Float(value) => Pattern::Float(*value, span),
            Datum::Str(value) => Pattern::Str(value.clone(), span),
            Datum::Keyword(name) => Pattern::Keyword(name.clone(), span),
            Datum::Symbol(name) => {
                if name == "_" {
                    return Pattern::Wildcard(span);
                }
                if is_constructor_name(name) {
                    return Pattern::Variant {
                        name: name.clone(),
                        positional: Vec::new(),
                        named: Vec::new(),
                        span,
                    };
                }
                Pattern::Binding(name.clone(), span)
            }
            Datum::Vector(items) => {
                let mut patterns = Vec::new();
                let mut rest = None;
                for item in items {
                    if let Some(name) = item.as_symbol().and_then(|name| name.strip_prefix("...")) {
                        rest = Some(if name.is_empty() || name == "_" {
                            None
                        } else {
                            Some(name.to_string())
                        });
                        continue;
                    }
                    patterns.push(self.pattern(item));
                }
                Pattern::Vector { items: patterns, rest, span }
            }
            Datum::Map(items) => {
                let keyword_keys = items
                    .chunks(2)
                    .filter_map(|pair| pair.first())
                    .all(|key| matches!(key.datum, Datum::Keyword(_)));
                let mut entries = Vec::new();
                for pair in items.chunks(2) {
                    let [key, value] = pair else { continue };
                    let name = match &key.datum {
                        Datum::Keyword(name) | Datum::Str(name) => name.clone(),
                        Datum::Symbol(name) => name.strip_suffix(':').unwrap_or(name).to_string(),
                        _ => {
                            self.error(
                                Diagnostic::error("expected a field name in a pattern")
                                    .with_code("pattern-map")
                                    .at(key.span, format!("found {}", key.describe()))
                                    .help("map and record patterns take keyword, string, or symbol keys"),
                            );
                            continue;
                        }
                    };
                    entries.push((name, self.pattern(value)));
                }
                if keyword_keys {
                    Pattern::Map { entries, span }
                } else {
                    Pattern::Record { fields: entries, span }
                }
            }
            Datum::List(items) => {
                let Some(name) = items.first().and_then(Syntax::as_symbol) else {
                    self.error(
                        Diagnostic::error("expected a constructor pattern")
                            .with_code("pattern-form")
                            .at(span, "expected `(Constructor ...)`"),
                    );
                    return Pattern::Wildcard(span);
                };
                let mut positional = Vec::new();
                let mut named = Vec::new();
                let mut index = 1usize;
                while index < items.len() {
                    let item = &items[index];
                    if let Some(field) = item.as_symbol().and_then(|text| text.strip_suffix(':')) {
                        match items.get(index + 1) {
                            Some(value) => {
                                named.push((field.to_string(), self.pattern(value)));
                                index += 2;
                                continue;
                            }
                            None => {
                                self.error(
                                    Diagnostic::error(format!("field `{field}` has no pattern"))
                                        .with_code("pattern-form")
                                        .at(item.span, "expected a pattern after this field name"),
                                );
                                index += 1;
                                continue;
                            }
                        }
                    }
                    positional.push(self.pattern(item));
                    index += 1;
                }
                Pattern::Variant { name: name.to_string(), positional, named, span }
            }
            Datum::Set(_) | Datum::Tagged(..) | Datum::Comment(..) => {
                self.error(
                    Diagnostic::error("unsupported pattern")
                        .with_code("pattern-form")
                        .at(span, format!("{} cannot be matched", form.describe())),
                );
                Pattern::Wildcard(span)
            }
        }
    }
}

/// Lower a single expression outside a module, used by the REPL.
pub fn lower_expr(file: FileId, form: &Syntax, diagnostics: &mut Diagnostics) -> Expr {
    let mut lowerer = Lowerer { diagnostics, file, lambda_counter: 0 };
    lowerer.expr(form)
}

/// Lower a sequence of forms into a body, used by the REPL and macro bodies.
pub fn lower_body(
    file: FileId,
    forms: &[Syntax],
    span: Span,
    diagnostics: &mut Diagnostics,
) -> Body {
    let mut lowerer = Lowerer { diagnostics, file, lambda_counter: 0 };
    lowerer.body(forms, span)
}

/// Fold postfix operators that the reader leaves as separate tokens:
/// `X ?` becomes `(propagate X)` and `X .field` becomes `(.field X)`.
pub fn fold_postfix(items: &[Syntax]) -> Vec<Syntax> {
    let mut out: Vec<Syntax> = Vec::with_capacity(items.len());
    for item in items {
        match item.as_symbol() {
            Some("?") => {
                if let Some(previous) = out.pop() {
                    let span = previous.span.to(item.span);
                    out.push(Syntax::list(
                        vec![Syntax::symbol("propagate", item.span), previous],
                        span,
                    ));
                    continue;
                }
            }
            // `(f x).field` — a `.name` token directly after a form. A `...rest`
            // token is a rest pattern and must not be folded.
            Some(name)
                if name.len() > 1
                    && name.starts_with('.')
                    && !name.starts_with("...")
                    && !out.is_empty() =>
            {
                let previous = out.pop().unwrap();
                let span = previous.span.to(item.span);
                out.push(Syntax::list(vec![Syntax::symbol(name, item.span), previous], span));
                continue;
            }
            _ => {}
        }
        out.push(item.clone());
    }
    out
}

fn collect_shorthand_arity(items: &[Syntax], arity: &mut usize) {
    for item in items {
        match &item.datum {
            Datum::Symbol(name) => {
                if name == "%" {
                    *arity = (*arity).max(1);
                } else if let Some(digits) = name.strip_prefix('%') {
                    if let Ok(index) = digits.parse::<usize>() {
                        *arity = (*arity).max(index);
                    }
                }
            }
            Datum::List(inner) | Datum::Vector(inner) | Datum::Map(inner) | Datum::Set(inner) => {
                collect_shorthand_arity(inner, arity)
            }
            Datum::Tagged(_, inner) => collect_shorthand_arity(std::slice::from_ref(inner), arity),
            _ => {}
        }
    }
}

fn retarget_spans(form: &mut Syntax, span: Span) {
    form.span = span;
    match &mut form.datum {
        Datum::List(items) | Datum::Vector(items) | Datum::Map(items) | Datum::Set(items) => {
            for item in items {
                retarget_spans(item, span);
            }
        }
        Datum::Tagged(_, inner) => retarget_spans(inner, span),
        _ => {}
    }
}

/// Constructor names begin with an uppercase letter, as do type names.
pub fn is_constructor_name(name: &str) -> bool {
    name.rsplit(['.', '/'])
        .next()
        .and_then(|segment| segment.chars().next())
        .map(char::is_uppercase)
        .unwrap_or(false)
}

fn is_type_parameter(name: &str) -> bool {
    name.len() <= 2 && name.chars().next().map(char::is_uppercase).unwrap_or(false)
}

/// A bare symbol usable as the head of a type application.
fn is_type_atom(form: &Syntax) -> bool {
    match form.as_symbol() {
        Some(name) => !name.ends_with(':') && name != "=" && name != "->" && !name.starts_with('!'),
        None => false,
    }
}

/// A symbol or group usable as a juxtaposed type argument.
///
/// Juxtaposition makes `Result (Vec Profile) DashboardError` read naturally,
/// but a function body follows the return type in the same list, so a group is
/// only treated as a type when it is shaped like one: a constructor-cased head
/// with type-shaped elements, a tuple of types, or a record whose keys carry
/// annotation colons.
fn is_type_arg(form: &Syntax) -> bool {
    match &form.datum {
        Datum::Symbol(name) => is_type_atom(form) && is_constructor_name(name),
        _ => looks_like_type(form),
    }
}

fn looks_like_type(form: &Syntax) -> bool {
    match &form.datum {
        Datum::Symbol(name) => is_type_atom(form) && is_constructor_name(name),
        Datum::List(items) => match items.first() {
            Some(head) => {
                head.as_symbol().map(is_constructor_name).unwrap_or(false)
                    && items[1..].iter().all(looks_like_type)
            }
            None => false,
        },
        Datum::Vector(items) => !items.is_empty() && items.iter().all(looks_like_type),
        Datum::Map(items) => {
            !items.is_empty()
                && items
                    .chunks(2)
                    .filter_map(|pair| pair.first())
                    .all(|key| key.as_symbol().map(|name| name.ends_with(':')).unwrap_or(false))
        }
        _ => false,
    }
}

fn is_type_group(form: &Syntax) -> bool {
    matches!(form.datum, Datum::List(_) | Datum::Vector(_) | Datum::Map(_))
}
