//! Module namespaces.
//!
//! Names are private to their module unless exported and imported, so two
//! modules may each declare a `handle` or a `BadRequest` without meaning the
//! same thing. This builds the mapping every analysis needs: for each module,
//! which local name refers to which qualified declaration, and which module an
//! import alias stands for.
//!
//! Qualified names are written `module/name`. Diagnostics show only the part
//! after the slash, so the qualification stays out of the reader's way.

// korben-7zt

use crate::ast::{Item, Module, TypeBody};
use korben_syntax::{Diagnostic, Diagnostics, Span};
use std::collections::HashMap;

#[derive(Default)]
pub struct Namespace {
    /// module -> local value name -> qualified name
    values: HashMap<String, HashMap<String, String>>,
    /// module -> local type name -> qualified name
    types: HashMap<String, HashMap<String, String>>,
    /// module -> import alias -> module
    aliases: HashMap<String, HashMap<String, String>>,
}

/// `module/name`, the form every table is keyed by.
pub fn qualify(module: &str, name: &str) -> String {
    format!("{module}/{name}")
}

/// The part of a qualified name a reader cares about.
pub fn short(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

impl Namespace {
    pub fn build(modules: &[Module]) -> Namespace {
        let mut namespace = Namespace::default();
        // First pass: what each module declares.
        for module in modules {
            let values = namespace.values.entry(module.name.clone()).or_default();
            let mut types: Vec<(String, String)> = Vec::new();
            for item in &module.items {
                match item {
                    Item::Type(decl) => {
                        types.push((decl.name.clone(), qualify(&module.name, &decl.name)));
                        // A record or newtype also declares a constructor.
                        match &decl.body {
                            TypeBody::Enum(variants) => {
                                for variant in variants {
                                    values.insert(
                                        variant.name.clone(),
                                        qualify(&module.name, &variant.name),
                                    );
                                }
                            }
                            _ => {
                                values.insert(decl.name.clone(), qualify(&module.name, &decl.name));
                            }
                        }
                    }
                    Item::Protocol(decl) => {
                        types.push((decl.name.clone(), qualify(&module.name, &decl.name)));
                        for method in &decl.methods {
                            values.insert(method.name.clone(), qualify(&module.name, &method.name));
                        }
                    }
                    Item::Fn(decl) => {
                        values.insert(decl.name.clone(), qualify(&module.name, &decl.name));
                    }
                    Item::Foreign(decl) => {
                        values.insert(decl.name.clone(), qualify(&module.name, &decl.name));
                    }
                    Item::Const { name, .. } => {
                        values.insert(name.clone(), qualify(&module.name, name));
                    }
                    _ => {}
                }
            }
            let table = namespace.types.entry(module.name.clone()).or_default();
            for (name, qualified) in types {
                table.insert(name, qualified);
            }
        }

        // Second pass: what each module imports.
        for module in modules {
            for import in &module.imports {
                namespace
                    .aliases
                    .entry(module.name.clone())
                    .or_default()
                    .insert(import.alias.clone(), import.path.clone());

                let Some(names) = &import.names else { continue };
                for name in names {
                    // A name may be a value, a type, or both.
                    if let Some(qualified) =
                        namespace.values.get(&import.path).and_then(|table| table.get(name))
                    {
                        let qualified = qualified.clone();
                        namespace
                            .values
                            .entry(module.name.clone())
                            .or_default()
                            .insert(name.clone(), qualified);
                    }
                    if let Some(qualified) =
                        namespace.types.get(&import.path).and_then(|table| table.get(name))
                    {
                        let qualified = qualified.clone();
                        namespace
                            .types
                            .entry(module.name.clone())
                            .or_default()
                            .insert(name.clone(), qualified);
                    }
                }
            }
        }
        namespace
    }

    /// The declaration a value name refers to, seen from `module`.
    pub fn value(&self, module: &str, name: &str) -> Option<&str> {
        self.values.get(module)?.get(name).map(String::as_str)
    }

    // korben-4io
    /// Every value name `module` can write bare, for `did you mean` help.
    pub fn visible_values(&self, module: &str) -> impl Iterator<Item = &str> {
        self.values.get(module).into_iter().flat_map(|table| table.keys().map(String::as_str))
    }

    /// The declaration a type name refers to, seen from `module`.
    pub fn ty(&self, module: &str, name: &str) -> Option<&str> {
        self.types.get(module)?.get(name).map(String::as_str)
    }

    /// The module an import alias stands for.
    pub fn module_of(&self, module: &str, alias: &str) -> Option<&str> {
        self.aliases.get(module)?.get(alias).map(String::as_str)
    }

    /// A value in another module, addressed as `alias/name` or `alias.name`.
    pub fn through(&self, module: &str, alias: &str, name: &str) -> Option<&str> {
        let target = self.module_of(module, alias)?;
        self.values.get(target)?.get(name).map(String::as_str)
    }

    /// A type in another module.
    pub fn type_through(&self, module: &str, alias: &str, name: &str) -> Option<&str> {
        let target = self.module_of(module, alias)?;
        self.types.get(target)?.get(name).map(String::as_str)
    }
}

// korben-707
/// Report a name declared twice in the same module.
///
/// The evaluator and the native backend disagree about which of two
/// definitions wins -- one keeps the last, the other refuses to compile -- so
/// this has to be an error rather than a preference. Modules are independent,
/// and the two namespaces are independent of each other: a record `Point`
/// declares the type `Point` and the constructor `Point`, which is one
/// declaration in each namespace rather than a collision.
pub fn duplicate_declarations(modules: &[Module]) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    for module in modules {
        // A macro shares the value namespace: a call site cannot tell a macro
        // from a function, and expansion runs first, so a function with a
        // macro's name is unreachable rather than merely shadowed.
        let mut values: HashMap<&str, Span> = HashMap::new();
        let mut types: HashMap<&str, Span> = HashMap::new();

        for item in &module.items {
            match item {
                Item::Fn(decl) => {
                    report(&mut diagnostics, &mut values, &decl.name, decl.span);
                }
                Item::Foreign(decl) => {
                    report(&mut diagnostics, &mut values, &decl.name, decl.span);
                }
                // Macro forms are consumed by the expander and do not reach
                // here today. The arm is written for the value namespace they
                // would occupy, since a call site cannot tell a macro from a
                // function.
                Item::Macro(decl) => {
                    report(&mut diagnostics, &mut values, &decl.name, decl.span);
                }
                Item::Const { name, span, .. } => {
                    report(&mut diagnostics, &mut values, name, *span);
                }
                Item::Type(decl) => {
                    let reported = report(&mut diagnostics, &mut types, &decl.name, decl.span);
                    match &decl.body {
                        TypeBody::Enum(variants) => {
                            for variant in variants {
                                report(&mut diagnostics, &mut values, &variant.name, variant.span);
                            }
                        }
                        // A record or newtype also declares its constructor. A
                        // duplicate collides in both namespaces at once, but it
                        // is one mistake and deserves one diagnostic.
                        _ if reported => {}
                        _ => {
                            report(&mut diagnostics, &mut values, &decl.name, decl.span);
                        }
                    }
                }
                Item::Protocol(decl) => {
                    report(&mut diagnostics, &mut types, &decl.name, decl.span);
                    for method in &decl.methods {
                        report(&mut diagnostics, &mut values, &method.name, method.span);
                    }
                }
                Item::Impl(_) | Item::Derive(_) | Item::Test(_) => {}
            }
        }
    }
    diagnostics
}

/// Record a declaration, reporting it when the name is already taken.
///
/// Returns whether a diagnostic was pushed, so a declaration that occupies two
/// namespaces is reported once rather than once per namespace.
fn report<'a>(
    diagnostics: &mut Diagnostics,
    seen: &mut HashMap<&'a str, Span>,
    name: &'a str,
    span: Span,
) -> bool {
    let Some(first) = seen.get(name).copied() else {
        seen.insert(name, span);
        return false;
    };
    diagnostics.push(
        Diagnostic::error(format!("`{name}` is declared twice in this module"))
            .with_code("duplicate-definition")
            .at(span, "redeclared here")
            .secondary(first, "first declared here")
            .help("rename one of them, or remove the one that is not wanted"),
    );
    true
}
