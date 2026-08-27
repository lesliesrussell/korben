//! Project loading: manifests, module resolution, and the compilation session.
//!
//! A session owns the source map, the interpreter, and the analyzed AST for
//! every module it has loaded, which is what `check`, `run`, `test`, `repl`,
//! and `doc` all operate on.
#![allow(clippy::result_unit_err)]

// korben-6bc

use crate::ast::{Item, Module};
use crate::eval::{Interp, TypeInfo};
use crate::expand::expand_module;
use crate::lower::lower_module;
use crate::manifest::Manifest;
use crate::value::{Builtin, Closure, Env, Flow, ModuleRuntime, Sym, Value};
use korben_syntax::diag::{Diagnostic, Diagnostics};
use korben_syntax::reader::{Comments, Datum, Syntax};
use korben_syntax::span::Span;
use korben_syntax::SourceMap;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub const SOURCE_EXTENSION: &str = "kb";
pub const MANIFEST_NAME: &str = "korben.toml";

const PRELUDE_SOURCE: &str = include_str!("prelude.kb");

/// Protocols the compiler knows how to derive.
pub const DERIVABLE: &[&str] = &["Eq", "Hash", "Ord", "Json", "Encode", "Decode", "Show", "Clone"];

/// Loading reports its failures through the session's diagnostics rather than
/// through an error value, so the unit error type here is deliberate.
pub type Loaded = Result<Rc<ModuleRuntime>, ()>;

pub struct Session {
    pub sources: SourceMap,
    pub interp: Interp,
    pub diagnostics: Diagnostics,
    /// Analyzed modules in load order.
    pub modules: Vec<Module>,
    pub root: PathBuf,
    pub manifest: Manifest,
    loaded: HashSet<String>,
    loading: Vec<String>,
}

impl Session {
    /// A session with no project on disk, used by the REPL and by scripts.
    pub fn bare(root: PathBuf) -> Session {
        let manifest = Manifest::default_for("scratch");
        let mut session = Session {
            sources: SourceMap::new(),
            interp: Interp::new(),
            diagnostics: Diagnostics::new(),
            modules: Vec::new(),
            root,
            manifest,
            loaded: HashSet::new(),
            loading: Vec::new(),
        };
        session.load_prelude();
        session
    }

    /// Open the project containing `start`, walking up to find `korben.toml`.
    pub fn open(start: &Path) -> Result<Session, String> {
        let manifest_path = find_manifest(start).ok_or_else(|| {
            format!("no {MANIFEST_NAME} found in {} or any parent", start.display())
        })?;
        let manifest = Manifest::load(&manifest_path)?;
        let root = manifest_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut session = Session {
            sources: SourceMap::new(),
            interp: Interp::new(),
            diagnostics: Diagnostics::new(),
            modules: Vec::new(),
            root,
            manifest,
            loaded: HashSet::new(),
            loading: Vec::new(),
        };
        session.load_prelude();
        Ok(session)
    }

    fn load_prelude(&mut self) {
        let file = self.sources.add("<prelude>", PRELUDE_SOURCE);
        let (forms, errors) = korben_syntax::read_all(file, PRELUDE_SOURCE, Comments::Skip);
        for error in errors {
            self.diagnostics.push(error);
        }
        let module = self.interp.module(crate::builtins::PRELUDE);
        let previous = std::mem::replace(&mut self.interp.current, module);
        let mut diagnostics = Diagnostics::new();
        let remaining = expand_module(&mut self.interp, &forms, &mut diagnostics);
        self.diagnostics.extend(diagnostics);
        for form in remaining.iter().filter(|form| !form.is_comment()) {
            self.diagnostics.push(
                Diagnostic::error("the prelude may only define macros")
                    .with_code("prelude")
                    .at(form.span, format!("found {}", form.describe())),
            );
        }
        self.interp.current = previous;
    }

    pub fn src_dir(&self) -> PathBuf {
        self.root.join("src")
    }

    /// Map a module path such as `app.main` to a file on disk.
    pub fn module_path(&self, name: &str) -> Option<PathBuf> {
        let relative = name.replace('.', "/");
        [
            self.src_dir().join(format!("{relative}.{SOURCE_EXTENSION}")),
            self.src_dir().join(&relative).join(format!("mod.{SOURCE_EXTENSION}")),
        ]
        .into_iter()
        .find(|candidate| candidate.is_file())
    }

    /// Load a module by name, loading its imports first.
    pub fn load_module(&mut self, name: &str, span: Span) -> Loaded {
        if self.loaded.contains(name) {
            return Ok(self.interp.module(name));
        }
        // Standard library modules are provided natively, not read from disk.
        if self.interp.modules.contains_key(name) {
            return Ok(self.interp.module(name));
        }
        if self.loading.iter().any(|existing| existing == name) {
            let cycle = self.loading.join(" -> ");
            self.diagnostics.push(
                Diagnostic::error(format!("circular module dependency involving `{name}`"))
                    .with_code("module-cycle")
                    .at(span, "this import closes a cycle")
                    .note(format!("cycle: {cycle} -> {name}")),
            );
            return Err(());
        }
        let Some(path) = self.module_path(name) else {
            self.diagnostics.push(
                Diagnostic::error(format!("cannot find module `{name}`"))
                    .with_code("module-not-found")
                    .at(span, "no source file for this module")
                    .help(format!("expected `src/{}.{SOURCE_EXTENSION}`", name.replace('.', "/"))),
            );
            return Err(());
        };
        self.load_file(&path, Some(name.to_string()))
    }

    /// Read, expand, lower, and register a single source file. A `.kbx` build
    /// artifact is unpacked into the modules it contains.
    pub fn load_file(&mut self, path: &Path, module_name: Option<String>) -> Loaded {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                self.diagnostics.push(
                    Diagnostic::error(format!("cannot read {}: {error}", path.display()))
                        .with_code("io"),
                );
                return Err(());
            }
        };
        if crate::bundle::is_bundle(&text) {
            return self.load_bundle(path, &text);
        }
        let file = self.sources.add_file(path, text.clone());
        let default_name = module_name.clone().unwrap_or_else(|| default_module_name(path));
        self.load_source(file, &text, default_name, module_name)
    }

    /// Load source text that is already in the source map.
    pub fn load_source(
        &mut self,
        file: korben_syntax::FileId,
        text: &str,
        default_name: String,
        module_name: Option<String>,
    ) -> Loaded {
        let (forms, errors) = korben_syntax::read_all(file, text, Comments::Skip);
        for error in errors {
            self.diagnostics.push(error);
        }

        // The declared module name wins over the file path.
        let declared = declared_module_name(&forms).unwrap_or(default_name);
        let name = module_name.unwrap_or(declared);

        self.loading.push(name.clone());
        // Imports are resolved before expansion so imported macros are visible.
        for import in scan_imports(&forms) {
            let _ = self.load_module(&import.0, import.2);
        }

        let runtime = self.interp.module(&name);
        let previous = std::mem::replace(&mut self.interp.current, runtime.clone());

        let mut diagnostics = Diagnostics::new();
        let expanded = expand_module(&mut self.interp, &forms, &mut diagnostics);
        let module = lower_module(file, &name, &expanded, &mut diagnostics);
        self.diagnostics.extend(diagnostics);

        self.wire_imports(&module, &runtime);
        self.register_items(&module, &runtime);

        self.interp.current = previous;
        self.loading.pop();
        self.loaded.insert(name.clone());
        self.modules.push(module);
        Ok(runtime)
    }

    /// Load source text from memory. Convenience for tests and embedders.
    pub fn load_text(&mut self, name: &str, text: &str) -> Loaded {
        let file = self.sources.add(name.to_string(), text.to_string());
        self.load_source(file, text, name.to_string(), None)
    }

    fn wire_imports(&mut self, module: &Module, runtime: &Rc<ModuleRuntime>) {
        for import in &module.imports {
            if self.load_module(&import.path, import.span).is_err() {
                continue;
            }
            runtime.aliases.borrow_mut().insert(import.alias.clone(), import.path.clone());
            let Some(names) = &import.names else { continue };
            let Some(source) = self.interp.modules.get(&import.path) else { continue };
            for name in names {
                if !source.exports.borrow().contains_key(name) {
                    let private = source.globals.borrow().contains_key(name);
                    let help = if private {
                        format!("`{name}` is private; mark it `(pub ...)` to export it")
                    } else {
                        "only `pub` declarations are visible to importers".to_string()
                    };
                    self.diagnostics.push(
                        Diagnostic::error(format!("`{}` does not export `{name}`", import.path))
                            .with_code("unknown-export")
                            .at(import.span, "not found in that module")
                            .help(help),
                    );
                    continue;
                }
                runtime
                    .imported
                    .borrow_mut()
                    .insert(name.clone(), (import.path.clone(), name.clone()));
            }
        }
    }

    fn register_items(&mut self, module: &Module, runtime: &Rc<ModuleRuntime>) {
        // Types first, so functions can construct them regardless of order.
        for item in &module.items {
            if let Item::Type(decl) = item {
                self.register_type(decl, runtime);
            }
        }
        for item in &module.items {
            match item {
                Item::Protocol(decl) => {
                    let methods: Vec<String> =
                        decl.methods.iter().map(|method| method.name.clone()).collect();
                    for method in &methods {
                        self.interp.method_owner.insert(method.clone(), decl.name.clone());
                        let dispatcher = Value::Builtin(Rc::new(Builtin::Method {
                            protocol: Rc::from(decl.name.as_str()),
                            name: Rc::from(method.as_str()),
                        }));
                        define(runtime, method, dispatcher, decl.is_public);
                    }
                    self.interp.protocols.insert(decl.name.clone(), methods);
                }
                Item::Fn(decl) => {
                    let value = Value::Closure(Rc::new(Closure {
                        decl: decl.clone(),
                        env: Env::root(),
                        module: runtime.name.clone(),
                    }));
                    define(runtime, &decl.name, value, decl.is_public);
                }
                Item::Test(decl) => {
                    self.interp.tests.push((
                        module.name.clone(),
                        decl.name.clone(),
                        decl.clone(),
                        runtime.clone(),
                    ));
                }
                Item::Derive(decl) => {
                    for protocol in &decl.protocols {
                        if !DERIVABLE.contains(&protocol.as_str()) {
                            self.diagnostics.push(
                                Diagnostic::error(format!("cannot derive `{protocol}`"))
                                    .with_code("derive-unknown")
                                    .at(decl.span, "not a derivable protocol")
                                    .help(format!("derivable protocols: {}", DERIVABLE.join(", "))),
                            );
                            continue;
                        }
                        self.interp
                            .impls
                            .entry((protocol.clone(), decl.type_name.clone()))
                            .or_default();
                    }
                }
                _ => {}
            }
        }
        // Implementations need their protocol registered first.
        for item in &module.items {
            if let Item::Impl(decl) = item {
                if !self.interp.protocols.contains_key(&decl.protocol) {
                    self.diagnostics.push(
                        Diagnostic::error(format!("unknown protocol `{}`", decl.protocol))
                            .with_code("unknown-protocol")
                            .at(decl.span, "no protocol with this name is in scope"),
                    );
                    continue;
                }
                let mut methods = HashMap::new();
                for method in &decl.methods {
                    let value = Value::Closure(Rc::new(Closure {
                        decl: Rc::new(method.clone()),
                        env: Env::root(),
                        module: runtime.name.clone(),
                    }));
                    methods.insert(method.name.clone(), value);
                }
                if let Some(expected) = self.interp.protocols.get(&decl.protocol) {
                    for name in expected {
                        if !methods.contains_key(name) {
                            self.diagnostics.push(
                                Diagnostic::error(format!(
                                    "implementation of `{}` for `{}` is missing `{name}`",
                                    decl.protocol, decl.type_name
                                ))
                                .with_code("incomplete-impl")
                                .at(decl.span, "required method not implemented"),
                            );
                        }
                    }
                }
                self.interp.impls.insert((decl.protocol.clone(), decl.type_name.clone()), methods);
            }
        }
        // Constants run last: their initializers may call anything above.
        let previous = std::mem::replace(&mut self.interp.current, runtime.clone());
        for item in &module.items {
            if let Item::Const { name, value, is_public, span, .. } = item {
                let env = Env::root();
                match self.interp.eval(value, &env) {
                    Ok(value) => define(runtime, name, value, *is_public),
                    Err(flow) => self.report_flow(flow, *span),
                }
            }
        }
        self.interp.current = previous;
    }

    fn register_type(&mut self, decl: &Rc<crate::ast::TypeDecl>, runtime: &Rc<ModuleRuntime>) {
        use crate::ast::TypeBody;
        let type_name: Sym = Rc::from(decl.name.as_str());
        match &decl.body {
            TypeBody::Record(fields) => {
                let names: Vec<Sym> =
                    fields.iter().map(|(name, _, _)| Rc::from(name.as_str())).collect();
                self.interp.types.insert(
                    decl.name.clone(),
                    Rc::new(TypeInfo {
                        name: decl.name.clone(),
                        fields: names.iter().map(|name| name.to_string()).collect(),
                        variants: Vec::new(),
                        is_enum: false,
                    }),
                );
                let ctor = Value::Builtin(Rc::new(Builtin::Ctor {
                    type_name: type_name.clone(),
                    variant: None,
                    fields: names,
                }));
                define(runtime, &decl.name, ctor, decl.is_public);
            }
            TypeBody::Enum(variants) => {
                let mut infos = Vec::new();
                for variant in variants {
                    let names: Vec<Sym> =
                        variant.fields.iter().map(|(name, _, _)| Rc::from(name.as_str())).collect();
                    infos.push((
                        variant.name.clone(),
                        names.iter().map(|name| name.to_string()).collect::<Vec<_>>(),
                    ));
                    self.interp.variant_owner.insert(variant.name.clone(), decl.name.clone());
                    let value = if names.is_empty() {
                        // A payload-free variant is a value, so `None` needs no call.
                        Value::Variant(Rc::new(crate::value::VariantValue {
                            type_name: type_name.clone(),
                            variant: Rc::from(variant.name.as_str()),
                            fields: Vec::new(),
                        }))
                    } else {
                        Value::Builtin(Rc::new(Builtin::Ctor {
                            type_name: type_name.clone(),
                            variant: Some(Rc::from(variant.name.as_str())),
                            fields: names,
                        }))
                    };
                    define(runtime, &variant.name, value, decl.is_public);
                }
                self.interp.types.insert(
                    decl.name.clone(),
                    Rc::new(TypeInfo {
                        name: decl.name.clone(),
                        fields: Vec::new(),
                        variants: infos,
                        is_enum: true,
                    }),
                );
            }
            TypeBody::Newtype(_) => {
                self.interp.types.insert(
                    decl.name.clone(),
                    Rc::new(TypeInfo {
                        name: decl.name.clone(),
                        fields: vec!["value".to_string()],
                        variants: Vec::new(),
                        is_enum: false,
                    }),
                );
                let ctor = Value::Builtin(Rc::new(Builtin::Ctor {
                    type_name,
                    variant: None,
                    fields: vec![Rc::from("value")],
                }));
                define(runtime, &decl.name, ctor, decl.is_public);
            }
            TypeBody::Alias(_) => {
                self.interp.types.insert(
                    decl.name.clone(),
                    Rc::new(TypeInfo {
                        name: decl.name.clone(),
                        fields: Vec::new(),
                        variants: Vec::new(),
                        is_enum: false,
                    }),
                );
            }
        }
    }

    /// Load every module in a build artifact and return its entry module.
    fn load_bundle(&mut self, path: &Path, text: &str) -> Loaded {
        let entry = crate::bundle::bundle_entry(text).unwrap_or_else(|| "main".to_string());
        let mut last = None;
        for (name, source) in crate::bundle::read_bundle(text) {
            let display = format!("{}#{name}", path.display());
            let file = self.sources.add(display, source.clone());
            let (forms, errors) = korben_syntax::read_all(file, &source, Comments::Skip);
            for error in errors {
                self.diagnostics.push(error);
            }
            let runtime = self.interp.module(&name);
            let previous = std::mem::replace(&mut self.interp.current, runtime.clone());
            let mut diagnostics = Diagnostics::new();
            let expanded = expand_module(&mut self.interp, &forms, &mut diagnostics);
            let module = lower_module(file, &name, &expanded, &mut diagnostics);
            self.diagnostics.extend(diagnostics);
            self.wire_imports(&module, &runtime);
            self.register_items(&module, &runtime);
            self.interp.current = previous;
            self.loaded.insert(name.clone());
            self.modules.push(module);
            if name == entry {
                last = Some(runtime.clone());
            }
            if last.is_none() {
                last = Some(runtime);
            }
        }
        last.ok_or(())
    }

    /// Register a module's declarations into an already-loaded runtime module.
    /// The REPL uses this so definitions accumulate across evaluations.
    pub fn declare(&mut self, module: Module) {
        let runtime = self.interp.module(&module.name);
        let previous = std::mem::replace(&mut self.interp.current, runtime.clone());
        self.wire_imports(&module, &runtime);
        self.register_items(&module, &runtime);
        self.interp.current = previous;
        // Replace any earlier version of the same module so `check` sees one copy.
        self.modules.retain(|existing| existing.name != module.name);
        self.modules.push(module);
    }

    /// Turn a runtime control-flow escape into a reported diagnostic.
    pub fn report_flow(&mut self, flow: Flow, span: Span) {
        self.diagnostics.push(flow_diagnostic(flow, span));
    }
}

/// Convert non-local control flow that escaped to the top into a diagnostic.
pub fn flow_diagnostic(flow: Flow, span: Span) -> Diagnostic {
    match flow {
        Flow::Panic(diagnostic) => *diagnostic,
        Flow::Condition(value, span) => Diagnostic::error("unhandled condition")
            .with_code("condition")
            .at(span, format!("{value}"))
            .help("wrap the call in `(try ... (catch ...))`"),
        Flow::Propagate(value) => Diagnostic::error("unhandled error propagated to the top level")
            .with_code("propagate")
            .at(span, format!("{value}")),
        Flow::Recur(_) => Diagnostic::error("`recur` outside a loop or function")
            .with_code("recur-scope")
            .at(span, "`recur` must appear in tail position of a `loop` or `fn`"),
    }
}

fn define(runtime: &Rc<ModuleRuntime>, name: &str, value: Value, is_public: bool) {
    runtime.globals.borrow_mut().insert(name.to_string(), value.clone());
    if is_public {
        runtime.exports.borrow_mut().insert(name.to_string(), value);
    }
}

pub fn find_manifest(start: &Path) -> Option<PathBuf> {
    let mut current =
        if start.is_dir() { start.to_path_buf() } else { start.parent()?.to_path_buf() };
    loop {
        let candidate = current.join(MANIFEST_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn default_module_name(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "main".to_string())
}

fn declared_module_name(forms: &[Syntax]) -> Option<String> {
    for form in forms {
        if form.head_symbol() == Some("module") {
            return form.as_list()?.get(1)?.as_symbol().map(str::to_string);
        }
    }
    None
}

/// Find `use` forms before expansion so imported macros are available.
fn scan_imports(forms: &[Syntax]) -> Vec<(String, Option<String>, Span)> {
    let mut imports = Vec::new();
    let mut visit = |form: &Syntax| {
        if form.head_symbol() != Some("use") {
            return;
        }
        let Some(items) = form.as_list() else { return };
        if let Some(path) = items.get(1).and_then(Syntax::as_symbol) {
            imports.push((path.to_string(), None, form.span));
        }
    };
    for form in forms {
        if form.head_symbol() == Some("module") {
            if let Some(items) = form.as_list() {
                for inner in &items[2..] {
                    visit(inner);
                }
            }
            continue;
        }
        visit(form);
    }
    imports
}

/// Every `.kb` file under a directory, sorted for deterministic ordering.
pub fn source_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_sources(dir, &mut found);
    found.sort();
    found
}

fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if path.is_dir() {
            let skip = path
                .file_name()
                .map(|name| name == "target" || name.to_string_lossy().starts_with('.'))
                .unwrap_or(false);
            if !skip {
                collect_sources(&path, out);
            }
            continue;
        }
        if path.extension().map(|extension| extension == SOURCE_EXTENSION).unwrap_or(false) {
            out.push(path);
        }
    }
}

/// True when a form sequence contains only comments.
pub fn is_blank(forms: &[Syntax]) -> bool {
    forms.iter().all(|form| matches!(form.datum, Datum::Comment(..)))
}
