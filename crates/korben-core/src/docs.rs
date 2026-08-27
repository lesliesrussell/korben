//! Documentation generation.
//!
//! Output is derived from module declarations, `;;;` comments, signatures,
//! effects, and visibility — the sources the specification names in section 24.

// korben-6bc

use crate::ast::{Effects, FnDecl, Item, Module, TypeBody, TypeExpr};
use korben_syntax::diag::json_string;

pub fn render_module(module: &Module) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", module.name));
    if let Some(doc) = &module.doc {
        out.push_str(doc);
        out.push_str("\n\n");
    }
    if !module.imports.is_empty() {
        out.push_str("## Imports\n\n");
        for import in &module.imports {
            match &import.names {
                Some(names) => {
                    out.push_str(&format!("- `{}` — {}\n", import.path, names.join(", ")))
                }
                None => out.push_str(&format!("- `{}` as `{}`\n", import.path, import.alias)),
            }
        }
        out.push('\n');
    }

    let public: Vec<&Item> = module.items.iter().filter(|item| item.is_public()).collect();
    let types: Vec<&Item> =
        public.iter().copied().filter(|item| matches!(item, Item::Type(_))).collect();
    if !types.is_empty() {
        out.push_str("## Types\n\n");
        for item in types {
            let Item::Type(decl) = item else { continue };
            out.push_str(&format!("### `{}`\n\n", decl.name));
            if let Some(doc) = &decl.doc {
                out.push_str(doc);
                out.push_str("\n\n");
            }
            match &decl.body {
                TypeBody::Record(fields) => {
                    out.push_str("| field | type |\n| --- | --- |\n");
                    for (name, ty, _) in fields {
                        out.push_str(&format!("| `{name}` | `{}` |\n", render_type(ty)));
                    }
                }
                TypeBody::Enum(variants) => {
                    for variant in variants {
                        let fields: Vec<String> = variant
                            .fields
                            .iter()
                            .map(|(name, ty, _)| format!("{name}: {}", render_type(ty)))
                            .collect();
                        if fields.is_empty() {
                            out.push_str(&format!("- `({})`\n", variant.name));
                        } else {
                            out.push_str(&format!("- `({} {})`\n", variant.name, fields.join(" ")));
                        }
                    }
                }
                TypeBody::Newtype(inner) => {
                    out.push_str(&format!("A newtype over `{}`.\n", render_type(inner)));
                }
                TypeBody::Alias(inner) => {
                    out.push_str(&format!("An alias for `{}`.\n", render_type(inner)));
                }
            }
            out.push('\n');
        }
    }

    let functions: Vec<&FnDecl> = public
        .iter()
        .filter_map(|item| match item {
            Item::Fn(decl) => Some(&**decl),
            _ => None,
        })
        .collect();
    if !functions.is_empty() {
        out.push_str("## Functions\n\n");
        for decl in functions {
            out.push_str(&format!("### `{}`\n\n", decl.name));
            out.push_str(&format!("```lisp\n{}\n```\n\n", signature(decl)));
            if let Some(doc) = &decl.doc {
                out.push_str(doc);
                out.push('\n');
            }
            if decl.is_unsafe {
                out.push_str("\n> **Unsafe.** Callers must establish the safety contract.\n");
            }
            if !decl.declared_effects.is_empty() {
                out.push_str(&format!("\nEffects: `{}`\n", decl.declared_effects.render()));
            }
            out.push('\n');
        }
    }

    let protocols: Vec<&Item> =
        public.iter().copied().filter(|item| matches!(item, Item::Protocol(_))).collect();
    if !protocols.is_empty() {
        out.push_str("## Protocols\n\n");
        for item in protocols {
            let Item::Protocol(decl) = item else { continue };
            out.push_str(&format!("### `{}`\n\n", decl.name));
            if let Some(doc) = &decl.doc {
                out.push_str(doc);
                out.push_str("\n\n");
            }
            for method in &decl.methods {
                let params: Vec<String> =
                    method.params.iter().map(|param| param.name.clone()).collect();
                let ret = method
                    .ret
                    .as_ref()
                    .map(|ty| format!(" -> {}", render_type(ty)))
                    .unwrap_or_default();
                out.push_str(&format!("- `({} [{}]{ret})`\n", method.name, params.join(" ")));
            }
            out.push('\n');
        }
    }
    out
}

pub fn signature(decl: &FnDecl) -> String {
    let params: Vec<String> = decl
        .params
        .iter()
        .map(|param| {
            let keyword =
                param.keyword.as_ref().map(|name| format!(":{name} ")).unwrap_or_default();
            let ty =
                param.ty.as_ref().map(|ty| format!(": {}", render_type(ty))).unwrap_or_default();
            format!("{keyword}{}{ty}", param.name)
        })
        .collect();
    let ret = decl.ret.as_ref().map(|ty| format!(" -> {}", render_type(ty))).unwrap_or_default();
    let effects = if decl.declared_effects.is_empty() {
        String::new()
    } else {
        format!(" {}", decl.declared_effects.render())
    };
    let keyword = if decl.is_async { "async fn" } else { "fn" };
    format!("({keyword} {} [{}]{ret}{effects})", decl.name, params.join(" "))
}

pub fn render_type(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Name(name, args, _) if args.is_empty() => name.clone(),
        TypeExpr::Name(name, args, _) => {
            let rendered: Vec<String> = args
                .iter()
                .map(|arg| match arg {
                    TypeExpr::Name(_, inner, _) if !inner.is_empty() => {
                        format!("({})", render_type(arg))
                    }
                    _ => render_type(arg),
                })
                .collect();
            format!("{name} {}", rendered.join(" "))
        }
        TypeExpr::Record(fields, _) => {
            let rendered: Vec<String> =
                fields.iter().map(|(name, ty)| format!("{name}: {}", render_type(ty))).collect();
            format!("{{ {} }}", rendered.join(" "))
        }
        TypeExpr::Tuple(items, _) => {
            let rendered: Vec<String> = items.iter().map(render_type).collect();
            format!("[{}]", rendered.join(" "))
        }
        TypeExpr::Fn(params, ret, effects, _) => {
            let rendered: Vec<String> = params.iter().map(render_type).collect();
            let effects =
                if effects.is_empty() { String::new() } else { format!(" {}", effects.render()) };
            format!("(-> [{}] {}{effects})", rendered.join(" "), render_type(ret))
        }
    }
}

/// Machine-readable API description, for editors and registries.
pub fn render_api_json(modules: &[Module]) -> String {
    let mut entries = Vec::new();
    for module in modules {
        let mut items = Vec::new();
        for item in module.items.iter().filter(|item| item.is_public()) {
            let (kind, signature_text, effects) = match item {
                Item::Fn(decl) => ("fn", signature(decl), decl.declared_effects),
                Item::Type(decl) => ("type", decl.name.clone(), Effects::NONE),
                Item::Protocol(decl) => ("protocol", decl.name.clone(), Effects::NONE),
                Item::Macro(decl) => ("macro", decl.name.clone(), Effects::NONE),
                Item::Const { name, .. } => ("def", name.clone(), Effects::NONE),
                _ => continue,
            };
            items.push(format!(
                "{{\"kind\":\"{kind}\",\"name\":{},\"signature\":{},\"effects\":{}}}",
                json_string(item.name()),
                json_string(&signature_text),
                json_string(&effects.render())
            ));
        }
        entries.push(format!(
            "{{\"module\":{},\"items\":[{}]}}",
            json_string(&module.name),
            items.join(",")
        ));
    }
    format!("{{\"api\":[{}]}}", entries.join(","))
}
