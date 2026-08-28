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
use crate::value::{closure_value, span_of, Closure, Env, Flow, ModuleRuntime, Sym, Value};
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

/// Standard-library modules written in Korben and carried inside the toolchain.
///
/// They are loaded on demand, from memory, so they need no files on disk and
/// stay in step with the compiler that ships them.
const EMBEDDED_MODULES: &[(&str, &str)] = &[("std.http", include_str!("stdlib/http.kb"))];

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
    /// The pinned dependency graph this session is building against.
    pub resolution: crate::pkg::Resolution,
    /// True when the lockfile was regenerated rather than reproduced.
    pub lock_written: bool,
    /// Which packages each package may import from.
    visibility: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    /// Which package each loaded module came from.
    module_package: HashMap<String, String>,
    loaded: HashSet<String>,
    loading: Vec<String>,
    /// Modules already reported as missing, so one bad import is one error.
    missing: HashSet<String>,
    /// Unsaved editor buffers, keyed by path, read in place of the file.
    overlay: HashMap<PathBuf, String>,
    /// The workspace this package belongs to, when it belongs to one.
    pub workspace: Option<crate::workspace::Workspace>,
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
            resolution: crate::pkg::Resolution::default(),
            lock_written: false,
            visibility: std::collections::BTreeMap::new(),
            module_package: HashMap::new(),
            loaded: HashSet::new(),
            loading: Vec::new(),
            missing: HashSet::new(),
            overlay: HashMap::new(),
            workspace: None,
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
        // korben-mic
        // A workspace changes where the lockfile lives and what resolution
        // covers, but not which package this session is for: `root` and
        // `manifest` stay the member the caller is standing in.
        let workspace = crate::workspace::Workspace::find(start)?;
        let (root, manifest) = match &workspace {
            Some(workspace) if manifest.is_virtual => {
                // Standing at a bare workspace root, there is no one package.
                // The first member stands in so that a session still has a
                // name; commands that need a specific program ask for one.
                match workspace.members.first() {
                    Some(member) => (member.root.clone(), member.manifest.clone()),
                    None => (root, manifest),
                }
            }
            _ => (root, manifest),
        };
        let mut session = Session {
            sources: SourceMap::new(),
            interp: Interp::new(),
            diagnostics: Diagnostics::new(),
            modules: Vec::new(),
            root,
            manifest,
            resolution: crate::pkg::Resolution::default(),
            lock_written: false,
            visibility: std::collections::BTreeMap::new(),
            module_package: HashMap::new(),
            loaded: HashSet::new(),
            loading: Vec::new(),
            missing: HashSet::new(),
            overlay: HashMap::new(),
            workspace,
        };
        session.load_prelude();
        session.prepare_dependencies()?;
        Ok(session)
    }

    /// Pin the dependency graph.
    ///
    /// When `korben.lock` is present and still describes the manifest, it is
    /// used verbatim and every checksum is verified: that is the reproducible
    /// path, and resolution does not run. Otherwise the graph is resolved and
    /// the lock is written.
    fn prepare_dependencies(&mut self) -> Result<(), String> {
        // korben-mic
        // A workspace resolves once, at its root: members that share a
        // dependency must share the version of it, which is only true if one
        // pass sees every member's requirements together.
        if self.workspace.is_some() {
            return self.prepare_workspace_dependencies();
        }
        use crate::pkg::{resolve, Lockfile, LOCK_NAME};
        let declared =
            !self.manifest.dependencies.is_empty() || !self.manifest.dev_dependencies.is_empty();
        let lock_path = self.root.join(LOCK_NAME);

        if lock_path.is_file() {
            let lock = Lockfile::load(&lock_path)?;
            if lock.matches(&self.manifest) {
                self.resolution = lock.materialize(&self.root, &self.manifest)?;
                self.visibility = crate::pkg::visibility(&self.manifest, &self.resolution);
                return Ok(());
            }
        }
        if !declared {
            // Nothing to pin, so no lockfile is written.
            self.visibility = crate::pkg::visibility(&self.manifest, &self.resolution);
            return Ok(());
        }

        self.resolution = resolve(&self.root, &self.manifest)?;
        let lock = Lockfile::from_resolution(&self.manifest, &self.resolution);
        std::fs::write(&lock_path, lock.render())
            .map_err(|error| format!("cannot write {}: {error}", lock_path.display()))?;
        self.lock_written = true;
        self.visibility = crate::pkg::visibility(&self.manifest, &self.resolution);
        Ok(())
    }

    // korben-mic
    /// Resolve and lock a whole workspace at its root.
    fn prepare_workspace_dependencies(&mut self) -> Result<(), String> {
        use crate::pkg::{resolve_all, Lockfile, LOCK_NAME};
        let workspace = self.workspace.clone().expect("a workspace");
        let manifests: Vec<&Manifest> =
            workspace.members.iter().map(|member| &member.manifest).collect();
        let lock_path = workspace.root.join(LOCK_NAME);
        let declared = workspace.members.iter().any(|member| {
            !member.manifest.dependencies.is_empty() || !member.manifest.dev_dependencies.is_empty()
        });

        if lock_path.is_file() {
            let lock = Lockfile::load(&lock_path)?;
            if lock.matches_workspace(&manifests) {
                self.resolution = lock.materialize(&workspace.root, &workspace.manifest)?;
                self.visibility = self.workspace_visibility(&workspace);
                return Ok(());
            }
        }
        if !declared {
            self.visibility = self.workspace_visibility(&workspace);
            return Ok(());
        }

        let members: Vec<(String, PathBuf)> =
            workspace.members.iter().map(|m| (m.name.clone(), m.root.clone())).collect();
        self.resolution = resolve_all(&workspace.manifest, &workspace.demands(), &members)?;
        let lock = Lockfile::from_workspace(&workspace.manifest.name, &manifests, &self.resolution);
        std::fs::write(&lock_path, lock.render())
            .map_err(|error| format!("cannot write {}: {error}", lock_path.display()))?;
        self.lock_written = true;
        self.visibility = self.workspace_visibility(&workspace);
        Ok(())
    }

    /// What each member may import: its own dependencies, plus itself.
    fn workspace_visibility(
        &self,
        workspace: &crate::workspace::Workspace,
    ) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
        let mut map = std::collections::BTreeMap::new();
        for member in &workspace.members {
            let mut visible = crate::pkg::visibility(&member.manifest, &self.resolution);
            if let Some(entry) = visible.remove(&member.name) {
                map.insert(member.name.clone(), entry);
            }
            for (name, entry) in visible {
                map.entry(name).or_insert(entry);
            }
        }
        map
    }

    /// The package a loaded module belongs to.
    pub fn package_of(&self, module: &str) -> &str {
        self.module_package.get(module).map(String::as_str).unwrap_or(&self.manifest.name)
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
    ///
    /// The root package is searched first, then each resolved dependency, so a
    /// dependency's modules are importable under the names they declare.
    pub fn module_path(&self, name: &str) -> Option<(PathBuf, String)> {
        let relative = name.replace('.', "/");
        let mut roots: Vec<(PathBuf, String)> = vec![(self.src_dir(), self.manifest.name.clone())];
        // korben-mic
        // Every member's sources are reachable, so a workspace can be checked
        // as a unit. Whether one member may import another is a separate
        // question, answered by the visibility table.
        if let Some(workspace) = &self.workspace {
            for member in &workspace.members {
                roots.push((member.root.join("src"), member.name.clone()));
            }
        }
        for package in &self.resolution.packages {
            roots.push((package.root.join("src"), package.name.clone()));
        }
        for (src, package) in roots {
            for candidate in [
                src.join(format!("{relative}.{SOURCE_EXTENSION}")),
                src.join(&relative).join(format!("mod.{SOURCE_EXTENSION}")),
            ] {
                if candidate.is_file() {
                    return Some((candidate, package));
                }
            }
        }
        None
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
        // Some of the standard library is written in Korben and embedded.
        if let Some((_, source)) = EMBEDDED_MODULES.iter().find(|(module, _)| *module == name) {
            self.loading.push(name.to_string());
            let file = self.sources.add(format!("<{name}>"), source.to_string());
            let source = source.to_string();
            let loaded = self.load_source(file, &source, name.to_string(), Some(name.to_string()));
            self.loading.pop();
            self.module_package.insert(name.to_string(), "std".to_string());
            return loaded;
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
        let Some((path, package)) = self.module_path(name) else {
            // One bad import is one error, however many times it is reached.
            if self.missing.insert(name.to_string()) {
                // A dotless name looks like a package, so suggest declaring it.
                let help = if name.contains('.') {
                    format!("expected `src/{}.{SOURCE_EXTENSION}`", name.replace('.', "/"))
                } else {
                    format!(
                        "expected `src/{name}.{SOURCE_EXTENSION}`, or declare the package with `korben add {name}`"
                    )
                };
                self.diagnostics.push(
                    Diagnostic::error(format!("cannot find module `{name}`"))
                        .with_code("module-not-found")
                        .at(span, "no source file for this module")
                        .help(help),
                );
            }
            return Err(());
        };
        self.module_package.insert(name.to_string(), package);
        self.load_file(&path, Some(name.to_string()))
    }

    /// Read, expand, lower, and register a single source file. A `.kbx` build
    /// artifact is unpacked into the modules it contains.
    pub fn load_file(&mut self, path: &Path, module_name: Option<String>) -> Loaded {
        // korben-efd
        // An editor's unsaved buffer is the truth about that file. Checking it
        // here means every path into the loader sees the same text, including
        // an import that reaches a file the editor happens to have open.
        let text = match self.overlay.get(path) {
            Some(text) => text.clone(),
            None => match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) => {
                    self.diagnostics.push(
                        Diagnostic::error(format!("cannot read {}: {error}", path.display()))
                            .with_code("io"),
                    );
                    return Err(());
                }
            },
        };
        if crate::bundle::is_bundle(&text) {
            return self.load_bundle(path, &text);
        }
        let file = self.sources.add_file(path, text.clone());
        let default_name = module_name.clone().unwrap_or_else(|| default_module_name(path));
        self.load_source(file, &text, default_name, module_name)
    }

    // korben-mic
    /// Point this session at one workspace member.
    ///
    /// A session opened in a workspace carries a stand-in package so that it
    /// has a name at all. `build` needs the real one: the artifact is named
    /// after it and written under its directory, so building `app` must not
    /// produce something called after whichever member came first.
    pub fn focus(&mut self, name: &str) {
        let Some(workspace) = &self.workspace else { return };
        let Some(member) = workspace.member(name) else { return };
        self.root = member.root.clone();
        self.manifest = member.manifest.clone();
    }

    // korben-efd
    /// Read `path` from `text` rather than from disk, for an unsaved buffer.
    pub fn set_overlay(&mut self, path: PathBuf, text: String) {
        self.overlay.insert(path, text);
    }

    /// Go back to reading `path` from disk.
    pub fn clear_overlay(&mut self, path: &Path) {
        self.overlay.remove(path);
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

        // A module declaration must agree with where the file lives, so that
        // module paths resolve deterministically.
        let declared = declared_module_name(&forms);
        let name = match (&module_name, &declared) {
            (Some(expected), Some(declared)) if expected != declared => {
                let span = forms
                    .iter()
                    .find(|form| form.head_symbol() == Some("module"))
                    .map(|form| form.span)
                    .unwrap_or(Span::new(file, 0, 0));
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "module is declared as `{declared}` but resolves as `{expected}`"
                    ))
                    .with_code("module-name-mismatch")
                    .at(span, format!("expected `(module {expected} ...)`"))
                    .help("a module's name must match its path under `src/`"),
                );
                expected.clone()
            }
            _ => module_name.clone().unwrap_or_else(|| declared.clone().unwrap_or(default_name)),
        };

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
            if !self.import_is_declared(&module.name, &import.path) {
                let importer = self.package_of(&module.name).to_string();
                let provider = self.package_of(&import.path).to_string();
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "`{importer}` does not declare a dependency on `{provider}`"
                    ))
                    .with_code("undeclared-dependency")
                    .at(
                        import.span,
                        format!("module `{}` comes from package `{provider}`", import.path),
                    )
                    .help(format!("add it with `korben add {provider}`")),
                );
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
                        let dispatcher = Value::method(&decl.name, method);
                        define(runtime, method, dispatcher, decl.is_public);
                    }
                    self.interp.protocols.insert(decl.name.clone(), methods);
                }
                Item::Fn(decl) => {
                    let value = closure_value(Rc::new(Closure {
                        decl: decl.clone(),
                        env: Env::root(),
                        module: runtime.name.clone(),
                    }));
                    define(runtime, &decl.name, value, decl.is_public);
                }
                // A foreign declaration becomes an ordinary callable whose body
                // marshals into the C ABI.
                Item::Foreign(decl) => {
                    let signature = korben_runtime::ffi::CSignature {
                        library: decl.library.clone(),
                        symbol: decl.symbol.clone(),
                        params: decl
                            .c_params
                            .iter()
                            .filter_map(|name| korben_runtime::ffi::CType::parse(name))
                            .collect(),
                        ret: korben_runtime::ffi::CType::parse(&decl.c_ret)
                            .unwrap_or(korben_runtime::ffi::CType::Void),
                    };
                    let name = decl.name.clone();
                    let params = decl
                        .params
                        .iter()
                        .map(|param| crate::value::Param {
                            name: Rc::from(param.name.as_str()),
                            keyword: None,
                            has_default: false,
                        })
                        .collect();
                    let value = Value::Fn(Rc::new(crate::value::Function {
                        name: name.clone(),
                        params,
                        is_async: false,
                        body: crate::value::Body::Rust(Box::new(move |_, args, loc| {
                            let values = args.into_iter().map(|arg| arg.value).collect();
                            korben_runtime::ffi::call(&signature, values, &name, loc)
                        })),
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
                    let value = closure_value(Rc::new(Closure {
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
                let field_names: Vec<&str> = names.iter().map(|name| &**name).collect();
                let ctor = Value::ctor(&decl.name, None, &field_names);
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
                        Value::variant(&decl.name, &variant.name, Vec::new())
                    } else {
                        let field_names: Vec<&str> = names.iter().map(|name| &**name).collect();
                        Value::ctor(&decl.name, Some(&variant.name), &field_names)
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
                let ctor = Value::ctor(&decl.name, None, &["value"]);
                let _ = &type_name;
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

    /// Whether the importing module's package may import from the provider.
    fn import_is_declared(&self, importer_module: &str, imported: &str) -> bool {
        // The standard library is always available.
        if imported.starts_with("std.") || !self.module_package.contains_key(imported) {
            return true;
        }
        let importer = self.package_of(importer_module);
        let provider = self.package_of(imported);
        if importer == provider {
            return true;
        }
        self.visibility.get(importer).map(|allowed| allowed.contains(provider)).unwrap_or(false)
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
        Flow::Panic(fault) => fault_diagnostic(*fault, span),
        Flow::Condition(value, loc) => Diagnostic::error("unhandled condition")
            .with_code("condition")
            .at(span_of(loc), format!("{value}"))
            .help("wrap the call in `(try ... (catch ...))`"),
        Flow::Propagate(value) => Diagnostic::error("unhandled error propagated to the top level")
            .with_code("propagate")
            .at(span, format!("{value}")),
        Flow::Recur(_) => Diagnostic::error("`recur` outside a loop or function")
            .with_code("recur-scope")
            .at(span, "`recur` must appear in tail position of a `loop` or `fn`"),
    }
}

/// Render a runtime fault as a compiler diagnostic, so both execution modes
/// report the same failure in the same shape.
pub fn fault_diagnostic(fault: crate::value::Fault, fallback: Span) -> Diagnostic {
    let span = if fault.loc.is_none() { fallback } else { span_of(fault.loc) };
    let mut diagnostic =
        Diagnostic::error(fault.message).with_code(fault.code).at(span, fault.label);
    for note in fault.notes {
        diagnostic = diagnostic.note(note);
    }
    for help in fault.help {
        diagnostic = diagnostic.help(help);
    }
    diagnostic
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
