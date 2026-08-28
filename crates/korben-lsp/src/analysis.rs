//! Turning a checked session into the answers an editor asks for.
//!
//! Every query here runs against the same analysis `korben check` runs. An
//! editor that disagrees with the command line would be worse than no editor
//! support at all, so nothing in this module computes a second opinion.

// korben-efd

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use korben_core::ast::{Item, Module};
use korben_core::project::Session;
use korben_syntax::{Diagnostic, FileId, Span};

use crate::position::{to_position, word_at, Position};

/// A range within one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// What the server knows after checking the workspace once.
pub struct Analysis {
    pub session: Session,
    /// Inferred types by span, narrowest last, for hover.
    chart: Vec<(Span, String)>,
}

/// A place a definition lives.
pub struct Location {
    pub path: PathBuf,
    pub range: Range,
}

/// One entry offered for completion.
pub struct Completion {
    pub label: String,
    /// The protocol's `CompletionItemKind`.
    pub kind: i64,
    pub detail: Option<String>,
}

/// One entry in a document's outline.
pub struct Symbol {
    pub name: String,
    /// The protocol's `SymbolKind`.
    pub kind: i64,
    pub detail: String,
    pub range: Range,
}

const KIND_FUNCTION: i64 = 3;
const KIND_STRUCT: i64 = 23;
const KIND_INTERFACE: i64 = 11;
const KIND_CONSTANT: i64 = 14;
const KIND_MODULE: i64 = 2;
const KIND_EVENT: i64 = 24;

const COMPLETION_FUNCTION: i64 = 3;
const COMPLETION_CLASS: i64 = 7;
const COMPLETION_CONSTANT: i64 = 21;
const COMPLETION_MODULE: i64 = 9;

impl Analysis {
    /// Check the workspace, reading open buffers in place of their files.
    pub fn build(root: &Path, overlay: &BTreeMap<PathBuf, String>) -> Analysis {
        let mut session = match Session::open(root) {
            Ok(session) => session,
            // Outside a project every open buffer is still worth checking, so
            // fall back rather than going silent.
            Err(_) => Session::bare(root.to_path_buf()),
        };
        for (path, text) in overlay {
            session.set_overlay(path.clone(), text.clone());
        }
        load_workspace(&mut session, overlay);
        let chart = korben_core::infer::chart_session(&session);
        korben_core::infer::check_session(&mut session, false);
        let lints = korben_core::infer::lint_session(&session);
        session.diagnostics.extend(lints);
        Analysis { session, chart }
    }

    /// The file id the session gave `path`, if it read it at all.
    pub fn file_of(&self, path: &Path) -> Option<FileId> {
        self.session.sources.find_by_path(path)
    }

    pub fn text_of(&self, file: FileId) -> &str {
        self.session.sources.source(file)
    }

    /// Every diagnostic, grouped by the file it belongs to.
    pub fn diagnostics(&self) -> BTreeMap<FileId, Vec<&Diagnostic>> {
        let mut grouped: BTreeMap<FileId, Vec<&Diagnostic>> = BTreeMap::new();
        for item in &self.session.diagnostics.items {
            let Some(label) = &item.primary else { continue };
            grouped.entry(label.span.file).or_default().push(item);
        }
        grouped
    }

    /// The range a span covers, for the file it is in.
    pub fn range_of(&self, span: Span) -> Range {
        let text = self.session.sources.source(span.file);
        Range {
            start: to_position(text, span.start as usize),
            end: to_position(text, span.end as usize),
        }
    }

    /// What to show when the pointer rests on `offset`.
    pub fn hover(&self, file: FileId, offset: usize) -> Option<(String, Range)> {
        let text = self.session.sources.source(file);
        let (start, end) = word_at(text, offset)?;
        let name = &text[start..end];
        let range = Range { start: to_position(text, start), end: to_position(text, end) };

        // A declaration, wherever it is written, carries the most to say: its
        // signature and its documentation.
        if let Some((module, item)) = self.declaration(name) {
            let mut rendered = format!("```korben\n{}\n```", describe(item));
            let _ = module;
            if let Some(doc) = doc_of(item) {
                rendered.push_str("\n\n");
                rendered.push_str(doc.trim());
            }
            return Some((rendered, range));
        }

        // Otherwise report the type inference settled on for the narrowest
        // expression covering the cursor -- which is what a local binding, a
        // call, or a literal has to offer.
        let ty = self.narrowest(file, start, end)?;
        Some((format!("```korben\n{name}: {ty}\n```"), range))
    }

    /// Where the name under `offset` is declared.
    pub fn definition(&self, file: FileId, offset: usize) -> Option<Location> {
        let text = self.session.sources.source(file);
        let (start, end) = word_at(text, offset)?;
        let name = &text[start..end];
        // `alias.member` points at the member, and a bare name at itself.
        let target = name.rsplit(['.', '/']).next().unwrap_or(name);
        let (_, item) = self.declaration(target)?;
        let span = item.span();
        let path = self.session.sources.get(span.file)?.path.clone()?;
        Some(Location { path, range: self.range_of(span) })
    }

    /// What may be written at `offset`.
    pub fn completions(&self, file: FileId, offset: usize) -> Vec<Completion> {
        let text = self.session.sources.source(file);
        // After `alias.` the answer is that module's members and nothing else,
        // which is both shorter and more accurate than every name in scope.
        if let Some(alias) = alias_before(text, offset) {
            if let Some(members) = self.members_of(&alias) {
                return members;
            }
        }
        let mut items: Vec<Completion> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for module in &self.session.modules {
            let local = module.file == file;
            for item in &module.items {
                if !local && !item.is_public() {
                    continue;
                }
                if matches!(item, Item::Test(_) | Item::Derive(_) | Item::Impl(_)) {
                    continue;
                }
                if seen.insert(item.name().to_string()) {
                    items.push(Completion {
                        label: item.name().to_string(),
                        kind: completion_kind(item),
                        detail: Some(describe(item)),
                    });
                }
            }
        }
        for name in prelude_names() {
            if seen.insert(name.clone()) {
                items.push(Completion {
                    label: name,
                    kind: COMPLETION_FUNCTION,
                    detail: Some("std.core".to_string()),
                });
            }
        }
        for module in module_names() {
            if seen.insert(module.clone()) {
                items.push(Completion {
                    label: module.clone(),
                    kind: COMPLETION_MODULE,
                    detail: Some("module".to_string()),
                });
            }
        }
        items
    }

    /// The outline of one file.
    pub fn symbols(&self, file: FileId) -> Vec<Symbol> {
        let mut symbols = Vec::new();
        for module in self.session.modules.iter().filter(|module| module.file == file) {
            for item in &module.items {
                symbols.push(Symbol {
                    name: item.name().to_string(),
                    kind: symbol_kind(item),
                    detail: describe(item),
                    range: self.range_of(item.span()),
                });
            }
        }
        symbols
    }

    /// The declaration a name refers to, searched across the workspace.
    fn declaration(&self, name: &str) -> Option<(&Module, &Item)> {
        let mut fallback = None;
        for module in &self.session.modules {
            for item in &module.items {
                if item.name() != name {
                    continue;
                }
                if item.is_public() {
                    return Some((module, item));
                }
                fallback.get_or_insert((module, item));
            }
        }
        fallback
    }

    /// The narrowest charted type whose span covers the word.
    fn narrowest(&self, file: FileId, start: usize, end: usize) -> Option<&str> {
        let mut best: Option<(u32, &str)> = None;
        for (span, ty) in &self.chart {
            if span.file != file || span.start as usize > start || (span.end as usize) < end {
                continue;
            }
            let width = span.end.saturating_sub(span.start);
            if best.map(|(narrowest, _)| width < narrowest).unwrap_or(true) {
                best = Some((width, ty.as_str()));
            }
        }
        best.map(|(_, ty)| ty)
    }

    /// The members of a module named by an alias in some open file.
    fn members_of(&self, alias: &str) -> Option<Vec<Completion>> {
        let target = self
            .session
            .modules
            .iter()
            .flat_map(|module| &module.imports)
            .find(|import| import.alias == alias || import.path == alias)
            .map(|import| import.path.clone())
            .or_else(|| runtime_module(alias))?;

        let mut items: Vec<Completion> = Vec::new();
        for module in self.session.modules.iter().filter(|module| module.name == target) {
            for item in module.items.iter().filter(|item| item.is_public()) {
                items.push(Completion {
                    label: item.name().to_string(),
                    kind: completion_kind(item),
                    detail: Some(describe(item)),
                });
            }
        }
        let prefix = format!("{target}/");
        for name in korben_runtime::std::NAMES {
            if let Some(member) = name.strip_prefix(&prefix) {
                items.push(Completion {
                    label: member.to_string(),
                    kind: COMPLETION_FUNCTION,
                    detail: Some(target.clone()),
                });
            }
        }
        (!items.is_empty()).then_some(items)
    }
}

/// Load the project's sources, plus any open buffer the project does not cover.
fn load_workspace(session: &mut Session, overlay: &BTreeMap<PathBuf, String>) {
    let src = session.src_dir();
    for path in korben_core::project::source_files(&src) {
        let name = module_name_for(&src, &path);
        let _ = session.load_module(&name, Span::synthetic());
    }
    for path in korben_core::project::source_files(&session.root.join("tests")) {
        let _ = session.load_file(&path, None);
    }
    // A buffer outside `src/` and `tests/` -- a scratch file, or a file in a
    // directory the project does not claim -- still deserves diagnostics.
    let loaded: Vec<PathBuf> = overlay.keys().cloned().collect();
    for path in loaded {
        if session.sources.find_by_path(&path).is_none() {
            let _ = session.load_file(&path, None);
        }
    }
}

fn module_name_for(src: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(src).unwrap_or(path);
    let mut parts: Vec<String> =
        relative.components().map(|part| part.as_os_str().to_string_lossy().to_string()).collect();
    if let Some(last) = parts.last_mut() {
        *last = last.trim_end_matches(".kb").to_string();
    }
    if parts.last().map(|part| part == "mod").unwrap_or(false) {
        parts.pop();
    }
    parts.join(".")
}

/// The alias in `alias.` immediately before `offset`, if the cursor sits there.
fn alias_before(text: &str, offset: usize) -> Option<String> {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let head = before.trim_end_matches(|character: char| {
        character.is_alphanumeric() || "-_?!*+<>=/".contains(character)
    });
    let head = head.strip_suffix('.')?;
    let start = head
        .rfind(|character: char| character.is_whitespace() || "()[]{}\"';`,~@".contains(character))
        .map(|index| index + 1)
        .unwrap_or(0);
    let alias = &head[start..];
    (!alias.is_empty()).then(|| alias.to_string())
}

/// Whether a name addresses a runtime module, and what it is called.
fn runtime_module(name: &str) -> Option<String> {
    let prefix = format!("{name}/");
    korben_runtime::std::NAMES
        .iter()
        .any(|entry| entry.starts_with(&prefix))
        .then(|| name.to_string())
}

fn prelude_names() -> Vec<String> {
    korben_runtime::std::NAMES
        .iter()
        .filter_map(|name| name.strip_prefix("std.core/").map(str::to_string))
        .collect()
}

fn module_names() -> Vec<String> {
    let mut names: Vec<String> = korben_runtime::std::NAMES
        .iter()
        .filter_map(|name| name.split_once('/').map(|(module, _)| module.to_string()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// A one-line rendering of what a declaration is.
fn describe(item: &Item) -> String {
    match item {
        Item::Fn(decl) => korben_core::docs::signature(decl),
        Item::Foreign(decl) => korben_core::docs::foreign_signature(decl),
        Item::Type(decl) => format!("(type {})", decl.name),
        Item::Protocol(decl) => format!("(protocol {})", decl.name),
        Item::Impl(decl) => format!("(impl {})", decl.type_name),
        Item::Macro(decl) => format!("(macro {})", decl.name),
        Item::Test(decl) => format!("(test \"{}\")", decl.name),
        Item::Derive(decl) => format!("(derive {})", decl.type_name),
        Item::Const { name, .. } => format!("(def {name})"),
    }
}

fn doc_of(item: &Item) -> Option<&str> {
    match item {
        Item::Fn(decl) => decl.doc.as_deref(),
        Item::Type(decl) => decl.doc.as_deref(),
        Item::Protocol(decl) => decl.doc.as_deref(),
        Item::Macro(decl) => decl.doc.as_deref(),
        Item::Foreign(decl) => decl.doc.as_deref(),
        Item::Const { doc, .. } => doc.as_deref(),
        Item::Impl(_) | Item::Test(_) | Item::Derive(_) => None,
    }
}

fn symbol_kind(item: &Item) -> i64 {
    match item {
        Item::Fn(_) | Item::Foreign(_) => KIND_FUNCTION,
        Item::Type(_) => KIND_STRUCT,
        Item::Protocol(_) => KIND_INTERFACE,
        Item::Impl(_) | Item::Derive(_) => KIND_MODULE,
        Item::Macro(_) => KIND_EVENT,
        Item::Test(_) => KIND_FUNCTION,
        Item::Const { .. } => KIND_CONSTANT,
    }
}

fn completion_kind(item: &Item) -> i64 {
    match item {
        Item::Fn(_) | Item::Foreign(_) | Item::Macro(_) => COMPLETION_FUNCTION,
        Item::Type(_) | Item::Protocol(_) => COMPLETION_CLASS,
        Item::Const { .. } => COMPLETION_CONSTANT,
        _ => COMPLETION_FUNCTION,
    }
}
