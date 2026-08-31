//! Hygienic macro expansion.
//!
//! Macros are ordinary functions from syntax objects to syntax objects, run in
//! a compile-time environment. Because every syntax object keeps the span of
//! the text it came from, the expander can tell template-introduced identifiers
//! from ones the caller passed in, and renames the former so a macro can never
//! capture a caller's binding.

// korben-6bc

use crate::ast::MacroDecl;
use crate::eval::{value_to_syntax, Interp};
use crate::lower::{self, fold_postfix};
use crate::value::{closure_value, span_of, syntax_value, Closure, Env, Flow, Value};
use korben_syntax::diag::{Diagnostic, Diagnostics};
use korben_syntax::reader::{Datum, Syntax};
use korben_syntax::span::Span;
use std::collections::HashSet;
use std::rc::Rc;

/// Upper bound on expansion steps, per specification section 14.3.
const MAX_STEPS: usize = 100_000;

/// A macro definition together with its compiled compile-time function.
pub struct MacroEntry {
    pub decl: Rc<MacroDecl>,
    pub function: Value,
}

pub struct Expander<'a> {
    interp: &'a mut Interp,
    diagnostics: &'a mut Diagnostics,
    steps: usize,
    expansions: usize,
    /// Macro names currently being expanded, to report recursion cycles.
    stack: Vec<(String, Span)>,
}

/// Expand every top-level form in a file, registering macros as they appear so
/// that later forms can use them.
pub fn expand_module(
    interp: &mut Interp,
    forms: &[Syntax],
    diagnostics: &mut Diagnostics,
) -> Vec<Syntax> {
    let mut expander = Expander { interp, diagnostics, steps: 0, expansions: 0, stack: Vec::new() };
    let mut out = Vec::new();
    for form in forms {
        let expanded = expander.expand(form);
        if let Some(decl) = expander.macro_decl(&expanded) {
            expander.register(decl);
            continue;
        }
        out.push(expanded);
    }
    out
}

impl<'a> Expander<'a> {
    /// Recognize `(macro ...)` and `(pub macro ...)`.
    fn macro_decl(&mut self, form: &Syntax) -> Option<Rc<MacroDecl>> {
        let (items, is_public) = match form.head_symbol() {
            Some("macro") => (form.as_list()?, false),
            Some("pub") => {
                let outer = form.as_list()?;
                if outer.get(1)?.as_symbol() != Some("macro") {
                    return None;
                }
                (&outer[1..], true)
            }
            _ => return None,
        };
        let mut diagnostics = Diagnostics::new();
        let module = lower::lower_module(
            form.span.file,
            "macro",
            &[Syntax::list(items.to_vec(), form.span)],
            &mut diagnostics,
        );
        self.diagnostics.extend(diagnostics);
        for item in module.items {
            if let crate::ast::Item::Macro(decl) = item {
                if is_public {
                    let mut decl = (*decl).clone();
                    decl.is_public = true;
                    return Some(Rc::new(decl));
                }
                return Some(decl);
            }
        }
        None
    }

    fn register(&mut self, decl: Rc<MacroDecl>) {
        // A macro is a function from syntax to syntax; build its closure once.
        let mut params: Vec<crate::ast::Param> = decl
            .params
            .iter()
            .map(|name| crate::ast::Param {
                name: name.clone(),
                ty: None,
                keyword: None,
                default: None,
                span: decl.span,
            })
            .collect();
        if let Some(rest) = &decl.rest {
            params.push(crate::ast::Param {
                name: rest.clone(),
                ty: None,
                keyword: None,
                default: None,
                span: decl.span,
            });
        }
        let mut diagnostics = Diagnostics::new();
        let body = lower::lower_body(decl.span.file, &decl.body, decl.span, &mut diagnostics);
        self.diagnostics.extend(diagnostics);
        let fn_decl = Rc::new(crate::ast::FnDecl {
            name: decl.name.clone(),
            params,
            ret: None,
            declared_effects: crate::ast::Effects::NONE,
            body,
            is_async: false,
            is_public: decl.is_public,
            is_unsafe: false,
            doc: decl.doc.clone(),
            span: decl.span,
        });
        let function = closure_value(Rc::new(Closure {
            decl: fn_decl,
            env: Env::root(),
            module: self.interp.current.borrow().name.clone(),
        }));
        self.interp.macros.insert(decl.name.clone(), Rc::new(MacroEntry { decl, function }));
    }

    /// Fully expand a form and everything inside it.
    pub fn expand(&mut self, form: &Syntax) -> Syntax {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            if self.steps == MAX_STEPS + 1 {
                self.diagnostics.push(
                    Diagnostic::error("macro expansion limit reached")
                        .with_code("macro-limit")
                        .at(form.span, "expansion did not terminate")
                        .help("check for a macro that expands into a call to itself"),
                );
            }
            return form.clone();
        }

        match &form.datum {
            Datum::List(items) if !items.is_empty() => {
                if let Some(head) = items[0].as_symbol() {
                    // `quote` is opaque; syntax-quote expands only its unquotes.
                    if head == "quote" {
                        return form.clone();
                    }
                    if head == "syntax-quote" {
                        let mut expanded = items.to_vec();
                        if let Some(template) = expanded.get_mut(1) {
                            *template = self.expand_template(template);
                        }
                        return Syntax::new(Datum::List(expanded), form.span);
                    }
                    if self.interp.macros.contains_key(head) {
                        return self.expand_call(head.to_string(), form);
                    }
                }
                let expanded: Vec<Syntax> = items.iter().map(|item| self.expand(item)).collect();
                let mut result = Syntax::new(Datum::List(expanded), form.span);
                result.blank_before = form.blank_before;
                result
            }
            Datum::Vector(items) => {
                let mut result = Syntax::new(
                    Datum::Vector(items.iter().map(|item| self.expand(item)).collect()),
                    form.span,
                );
                result.blank_before = form.blank_before;
                result
            }
            Datum::Map(items) => Syntax::new(
                Datum::Map(items.iter().map(|item| self.expand(item)).collect()),
                form.span,
            ),
            Datum::Set(items) => Syntax::new(
                Datum::Set(items.iter().map(|item| self.expand(item)).collect()),
                form.span,
            ),
            _ => form.clone(),
        }
    }

    /// Inside a syntax-quote, only `~` and `~@` holes are expanded.
    fn expand_template(&mut self, form: &Syntax) -> Syntax {
        match &form.datum {
            Datum::List(items) => {
                if items.len() == 2
                    && matches!(items[0].as_symbol(), Some("unquote") | Some("unquote-splice"))
                {
                    let expanded = vec![items[0].clone(), self.expand(&items[1])];
                    return Syntax::new(Datum::List(expanded), form.span);
                }
                Syntax::new(
                    Datum::List(items.iter().map(|item| self.expand_template(item)).collect()),
                    form.span,
                )
            }
            Datum::Vector(items) => Syntax::new(
                Datum::Vector(items.iter().map(|item| self.expand_template(item)).collect()),
                form.span,
            ),
            Datum::Map(items) => Syntax::new(
                Datum::Map(items.iter().map(|item| self.expand_template(item)).collect()),
                form.span,
            ),
            Datum::Set(items) => Syntax::new(
                Datum::Set(items.iter().map(|item| self.expand_template(item)).collect()),
                form.span,
            ),
            _ => form.clone(),
        }
    }

    fn expand_call(&mut self, name: String, form: &Syntax) -> Syntax {
        let items = fold_postfix(form.as_list().unwrap());
        let entry = self.interp.macros[&name].clone();
        let (decl, function) = (entry.decl.clone(), entry.function.clone());

        if self.stack.iter().any(|(existing, _)| *existing == name) && self.stack.len() > 64 {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "macro `{name}` expanded recursively without terminating"
                ))
                .with_code("macro-recursion")
                .at(form.span, "expansion cycle detected")
                .secondary(decl.span, "macro defined here"),
            );
            return form.clone();
        }

        let arguments = &items[1..];
        let required = decl.params.len();
        if arguments.len() < required || (decl.rest.is_none() && arguments.len() > required) {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "macro `{name}` expects {required} argument(s) but got {}",
                    arguments.len()
                ))
                .with_code("macro-arity")
                .at(form.span, "wrong number of arguments")
                .secondary(decl.span, "macro defined here"),
            );
            return form.clone();
        }

        let mut args: Vec<(Option<String>, Value)> = arguments[..required]
            .iter()
            .map(|item| (None, syntax_value(Rc::new(item.clone()))))
            .collect();
        if decl.rest.is_some() {
            let rest: Vec<Value> = arguments[required..]
                .iter()
                .map(|item| syntax_value(Rc::new(item.clone())))
                .collect();
            args.push((None, Value::vector(rest)));
        }

        self.stack.push((name.clone(), form.span));
        let result = self.interp.apply(function, args, form.span);
        self.stack.pop();

        let expanded = match result {
            Ok(value) => value_to_syntax(&value, form.span),
            Err(Flow::Panic(fault)) => {
                // The expansion chain is what makes a macro failure debuggable.
                let mut diagnostic = crate::project::fault_diagnostic(*fault, form.span);
                diagnostic.notes.push(format!("while expanding macro `{name}`"));
                diagnostic
                    .secondary
                    .push(korben_syntax::diag::Label::new(decl.span, "macro defined here"));
                self.diagnostics.push(diagnostic);
                return Syntax::new(Datum::Nil, form.span);
            }
            Err(Flow::Condition(value, loc)) => {
                self.diagnostics.push(
                    Diagnostic::error(format!("macro `{name}` raised a condition"))
                        .with_code("macro-condition")
                        .at(span_of(loc), format!("{value}"))
                        .secondary(form.span, "expanded here"),
                );
                return Syntax::new(Datum::Nil, form.span);
            }
            Err(_) => {
                self.diagnostics.push(
                    Diagnostic::error(format!("macro `{name}` used non-local control flow"))
                        .with_code("macro-control-flow")
                        .at(form.span, "`recur` and `?` are not valid at expansion time"),
                );
                return Syntax::new(Datum::Nil, form.span);
            }
        };

        self.expansions += 1;
        let renamed = hygienize(&expanded, decl.span, self.expansions);
        self.expand(&renamed)
    }
}

/// Rename identifiers the macro template introduced as bindings.
///
/// A symbol is template-introduced when its span lies inside the macro's own
/// definition; arguments spliced in by the caller keep call-site spans. Only
/// names the template actually *binds* are renamed, so references to globals
/// and to caller-supplied names still resolve normally.
fn hygienize(form: &Syntax, macro_span: Span, expansion: usize) -> Syntax {
    let mut introduced = HashSet::new();
    collect_binders(form, macro_span, &mut introduced);
    if introduced.is_empty() {
        return form.clone();
    }
    rename(form, macro_span, &introduced, expansion)
}

fn within(span: Span, outer: Span) -> bool {
    span.file == outer.file && span.start >= outer.start && span.end <= outer.end
}

fn collect_binders(form: &Syntax, macro_span: Span, out: &mut HashSet<String>) {
    if let Datum::List(items) = &form.datum {
        match items.first().and_then(Syntax::as_symbol) {
            Some("let") | Some("var") => {
                if let Some(target) = items.get(1) {
                    add_binder(target, macro_span, out);
                }
            }
            Some("loop") => {
                if let Some(bindings) = items.get(1).and_then(Syntax::as_vector) {
                    for pair in bindings.chunks(2) {
                        if let Some(name) = pair.first() {
                            add_binder(name, macro_span, out);
                        }
                    }
                }
            }
            Some("fn") | Some("async-fn") => {
                for item in items.iter().take(3) {
                    if let Some(params) = item.as_vector() {
                        for param in params {
                            add_binder(param, macro_span, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    match &form.datum {
        Datum::List(items) | Datum::Vector(items) | Datum::Map(items) | Datum::Set(items) => {
            for item in items {
                collect_binders(item, macro_span, out);
            }
        }
        _ => {}
    }
}

fn add_binder(form: &Syntax, macro_span: Span, out: &mut HashSet<String>) {
    if !within(form.span, macro_span) {
        return;
    }
    if let Some(name) = form.as_symbol() {
        let name = name.strip_suffix(':').unwrap_or(name);
        if name != "_" && !name.starts_with('%') {
            out.insert(name.to_string());
        }
    }
}

fn rename(
    form: &Syntax,
    macro_span: Span,
    introduced: &HashSet<String>,
    expansion: usize,
) -> Syntax {
    let mut result = match &form.datum {
        Datum::Symbol(name) if within(form.span, macro_span) => {
            let (base, suffix) = match name.strip_suffix(':') {
                Some(base) => (base, ":"),
                None => (name.as_str(), ""),
            };
            if introduced.contains(base) {
                Syntax::new(Datum::Symbol(format!("{base}__m{expansion}{suffix}")), form.span)
            } else {
                form.clone()
            }
        }
        Datum::List(items) => Syntax::new(
            Datum::List(
                items.iter().map(|item| rename(item, macro_span, introduced, expansion)).collect(),
            ),
            form.span,
        ),
        Datum::Vector(items) => Syntax::new(
            Datum::Vector(
                items.iter().map(|item| rename(item, macro_span, introduced, expansion)).collect(),
            ),
            form.span,
        ),
        Datum::Map(items) => Syntax::new(
            Datum::Map(
                items.iter().map(|item| rename(item, macro_span, introduced, expansion)).collect(),
            ),
            form.span,
        ),
        Datum::Set(items) => Syntax::new(
            Datum::Set(
                items.iter().map(|item| rename(item, macro_span, introduced, expansion)).collect(),
            ),
            form.span,
        ),
        _ => form.clone(),
    };
    result.blank_before = form.blank_before;
    result
}
