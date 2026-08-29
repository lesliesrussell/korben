//! `korben.toml` parsing.
//!
//! A deliberately small TOML subset: tables, string/integer/boolean values, and
//! arrays of strings. That covers the manifest shape the specification defines
//! without pulling a dependency into a single-binary toolchain.

// korben-6bc

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub enum TomlValue {
    Str(String),
    Int(i64),
    Bool(bool),
    Array(Vec<TomlValue>),
}

impl TomlValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            TomlValue::Str(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            TomlValue::Int(value) => Some(*value),
            _ => None,
        }
    }
}

/// One declared dependency.
#[derive(Clone, Debug)]
pub struct Dependency {
    pub name: String,
    /// The version requirement as written, e.g. `^0.1`.
    pub requirement: String,
    /// A directory relative to this manifest, for a path dependency.
    pub path: Option<String>,
    pub dev: bool,
}

/// Manifest keys that would run code at install time. Specification 21.3
/// prohibits install scripts, so these are rejected rather than ignored.
const FORBIDDEN_KEYS: &[&str] =
    &["install", "preinstall", "postinstall", "script", "scripts", "prepare"];

#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub edition: String,
    pub description: Option<String>,
    pub license: Option<String>,
    /// Module path of the program entry point.
    pub main: String,
    pub opt_level: i64,
    pub target: String,
    pub dependencies: Vec<Dependency>,
    pub dev_dependencies: Vec<Dependency>,
    /// `[registry] path = "..."` — where registry dependencies are looked up.
    pub registry: Option<String>,
    // korben-poj
    /// `[registry] git = "..."` — a repository laid out as a registry, cloned
    /// into a local cache by `korben install`.
    pub registry_git: Option<String>,
    /// Capabilities build scripts and macros are granted.
    pub build_capabilities: Vec<String>,
    /// `[ffi] c = [...]` — C libraries this package links against.
    pub ffi_c: Vec<String>,
    /// `[ffi] rust = [...]` — Rust adapter crates.
    pub ffi_rust: Vec<String>,
    /// `[workspace] members = [...]` — directories this manifest gathers.
    pub members: Vec<String>,
    /// True when the manifest declares `[workspace]` but no `[package]`: a root
    /// that gathers members without being a package itself.
    pub is_virtual: bool,
    pub path: Option<PathBuf>,
}

impl Manifest {
    pub fn default_for(name: &str) -> Manifest {
        Manifest {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            edition: "2026".to_string(),
            description: None,
            license: None,
            main: "main".to_string(),
            opt_level: 2,
            target: "native".to_string(),
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            registry: None,
            registry_git: None,
            build_capabilities: Vec::new(),
            ffi_c: Vec::new(),
            ffi_rust: Vec::new(),
            members: Vec::new(),
            is_virtual: false,
            path: None,
        }
    }

    /// Parse a manifest. Unknown keys are ignored so that newer manifests keep
    /// loading in older toolchains, which is what the edition policy expects.
    pub fn parse(text: &str, path: Option<PathBuf>) -> Result<Manifest, String> {
        let tables = parse_tables(text)?;
        let package = tables.get("package");
        let workspace = tables.get("workspace");
        // korben-mic
        // A workspace root may gather members without being a package itself,
        // so `[package]` is required only when there is no `[workspace]`.
        let declared = package.and_then(|table| table.get("name")).and_then(TomlValue::as_str);
        let name = match (declared, workspace) {
            (Some(name), _) => name.to_string(),
            (None, Some(_)) => root_name(path.as_deref()),
            (None, None) => return Err("manifest is missing `[package] name`".to_string()),
        };
        let mut manifest = Manifest::default_for(&name);
        manifest.is_virtual = declared.is_none();
        if let Some(workspace) = workspace {
            if let Some(TomlValue::Array(items)) = workspace.get("members") {
                manifest.members =
                    items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect();
            }
        }
        manifest.path = path;
        if let Some(package) = package {
            if let Some(value) = package.get("version").and_then(TomlValue::as_str) {
                manifest.version = value.to_string();
            }
            if let Some(value) = package.get("edition").and_then(TomlValue::as_str) {
                manifest.edition = value.to_string();
            }
            manifest.description =
                package.get("description").and_then(TomlValue::as_str).map(str::to_string);
            manifest.license =
                package.get("license").and_then(TomlValue::as_str).map(str::to_string);
            if let Some(value) = package.get("main").and_then(TomlValue::as_str) {
                manifest.main = value.to_string();
            }
        }
        if let Some(build) = tables.get("build") {
            if let Some(value) = build.get("target").and_then(TomlValue::as_str) {
                manifest.target = value.to_string();
            }
            if let Some(value) = build.get("opt-level").or_else(|| build.get("opt_level")) {
                if let Some(value) = value.as_int() {
                    manifest.opt_level = value;
                }
            }
            if let Some(TomlValue::Array(items)) = build.get("capabilities") {
                manifest.build_capabilities =
                    items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect();
            }
        }
        if let Some(ffi) = tables.get("ffi") {
            for (key, target) in [("c", &mut manifest.ffi_c), ("rust", &mut manifest.ffi_rust)] {
                if let Some(TomlValue::Array(items)) = ffi.get(key) {
                    *target =
                        items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect();
                }
            }
        }
        if let Some(registry) = tables.get("registry") {
            manifest.registry =
                registry.get("path").and_then(TomlValue::as_str).map(str::to_string);
            // korben-poj
            manifest.registry_git =
                registry.get("git").and_then(TomlValue::as_str).map(str::to_string);
        }

        // Install scripts are prohibited, so a manifest that declares one is
        // rejected outright rather than having the key quietly ignored.
        for (section, table) in &tables {
            for key in table.keys() {
                if FORBIDDEN_KEYS.contains(&key.as_str()) {
                    let where_ = if section.is_empty() {
                        String::new()
                    } else {
                        format!(" in `[{section}]`")
                    };
                    return Err(format!(
                        "`{key}`{where_} would run code at install time, which is not allowed\n  specification 21.3: install scripts are prohibited by default"
                    ));
                }
            }
            if FORBIDDEN_KEYS.contains(&section.as_str()) {
                return Err(format!(
                    "`[{section}]` would run code at install time, which is not allowed\n  specification 21.3: install scripts are prohibited by default"
                ));
            }
        }

        for (section, dev) in [("dependencies", false), ("dev-dependencies", true)] {
            // The short form: `name = "^0.1"`.
            if let Some(table) = tables.get(section) {
                for (key, value) in table {
                    if let Some(text) = value.as_str() {
                        manifest.dependency_mut(dev).push(Dependency {
                            name: key.clone(),
                            requirement: text.to_string(),
                            path: None,
                            dev,
                        });
                    }
                }
            }
            // The long form: `[dependencies.name]` with `version` and `path`.
            let prefix = format!("{section}.");
            for (heading, table) in &tables {
                let Some(name) = heading.strip_prefix(&prefix) else { continue };
                let path = table.get("path").and_then(TomlValue::as_str).map(str::to_string);
                let requirement = table
                    .get("version")
                    .and_then(TomlValue::as_str)
                    .map(str::to_string)
                    // A path dependency without a version accepts whatever is there.
                    .unwrap_or_else(|| "*".to_string());
                manifest.dependency_mut(dev).push(Dependency {
                    name: name.to_string(),
                    requirement,
                    path,
                    dev,
                });
            }
        }
        manifest.dependencies.sort_by(|left, right| left.name.cmp(&right.name));
        manifest.dev_dependencies.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(manifest)
    }

    // korben-mic
    /// True when this manifest gathers members.
    pub fn is_workspace(&self) -> bool {
        !self.members.is_empty()
    }

    fn dependency_mut(&mut self, dev: bool) -> &mut Vec<Dependency> {
        if dev {
            &mut self.dev_dependencies
        } else {
            &mut self.dependencies
        }
    }

    /// Look up a declared dependency by name.
    pub fn dependency(&self, name: &str) -> Option<&Dependency> {
        self.dependencies
            .iter()
            .chain(&self.dev_dependencies)
            .find(|dependency| dependency.name == name)
    }

    pub fn load(path: &Path) -> Result<Manifest, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        Manifest::parse(&text, Some(path.to_path_buf()))
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("[package]\n");
        out.push_str(&format!("name = \"{}\"\n", self.name));
        out.push_str(&format!("version = \"{}\"\n", self.version));
        out.push_str(&format!("edition = \"{}\"\n", self.edition));
        if let Some(description) = &self.description {
            out.push_str(&format!("description = \"{description}\"\n"));
        }
        if let Some(license) = &self.license {
            out.push_str(&format!("license = \"{license}\"\n"));
        }
        out.push_str(&format!("main = \"{}\"\n", self.main));
        out.push_str("\n[dependencies]\n");
        out.push_str(&render_dependencies(&self.dependencies));
        out.push_str("\n[dev-dependencies]\n");
        out.push_str(&render_dependencies(&self.dev_dependencies));
        if !self.ffi_c.is_empty() || !self.ffi_rust.is_empty() {
            out.push_str("\n[ffi]\n");
            if !self.ffi_c.is_empty() {
                out.push_str(&format!("c = [{}]\n", render_list(&self.ffi_c)));
            }
            if !self.ffi_rust.is_empty() {
                out.push_str(&format!("rust = [{}]\n", render_list(&self.ffi_rust)));
            }
        }
        out.push_str("\n[build]\n");
        out.push_str(&format!("target = \"{}\"\n", self.target));
        out.push_str(&format!("opt-level = {}\n", self.opt_level));
        out
    }
}

// korben-mic
/// The name a virtual workspace root goes by: its directory, so diagnostics and
/// the lockfile have something to say rather than an empty string.
fn root_name(path: Option<&Path>) -> String {
    path.and_then(Path::parent)
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string())
}

/// Render dependencies, using the long form only where it is needed.
fn render_dependencies(dependencies: &[Dependency]) -> String {
    let mut out = String::new();
    let mut long = Vec::new();
    for dependency in dependencies {
        match &dependency.path {
            None => {
                out.push_str(&format!("{} = \"{}\"\n", dependency.name, dependency.requirement))
            }
            Some(_) => long.push(dependency),
        }
    }
    let section = if dependencies.first().map(|entry| entry.dev).unwrap_or(false) {
        "dev-dependencies"
    } else {
        "dependencies"
    };
    for dependency in long {
        out.push_str(&format!("\n[{section}.{}]\n", dependency.name));
        if dependency.requirement != "*" {
            out.push_str(&format!("version = \"{}\"\n", dependency.requirement));
        }
        if let Some(path) = &dependency.path {
            out.push_str(&format!("path = \"{path}\"\n"));
        }
    }
    out
}

fn render_list(items: &[String]) -> String {
    items.iter().map(|item| format!("\"{item}\"")).collect::<Vec<_>>().join(", ")
}

pub type Table = BTreeMap<String, TomlValue>;

/// Parse a TOML subset into its tables, keyed by dotted section name.
pub fn parse_tables(text: &str) -> Result<BTreeMap<String, Table>, String> {
    let mut tables: BTreeMap<String, Table> = BTreeMap::new();
    let mut current = String::new();
    tables.insert(current.clone(), Table::new());

    for (index, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            current = header.trim().trim_matches('"').to_string();
            tables.entry(current.clone()).or_default();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected `key = value`", index + 1));
        };
        let key = key.trim().trim_matches('"').to_string();
        let value = parse_value(value.trim())
            .ok_or_else(|| format!("line {}: cannot parse value `{}`", index + 1, value.trim()))?;
        tables.entry(current.clone()).or_default().insert(key, value);
    }
    Ok(tables)
}

/// Remove a trailing `#` comment, honoring quoted strings.
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    for (index, ch) in line.char_indices() {
        match ch {
            '"' => in_string = !in_string,
            '#' if !in_string => return &line[..index],
            _ => {}
        }
    }
    line
}

fn parse_value(raw: &str) -> Option<TomlValue> {
    if let Some(inner) = raw.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) {
        return Some(TomlValue::Str(inner.to_string()));
    }
    if raw == "true" {
        return Some(TomlValue::Bool(true));
    }
    if raw == "false" {
        return Some(TomlValue::Bool(false));
    }
    if let Some(inner) = raw.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
        let mut items = Vec::new();
        for part in split_array(inner) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            items.push(parse_value(part)?);
        }
        return Some(TomlValue::Array(items));
    }
    raw.parse::<i64>().ok().map(TomlValue::Int)
}

fn split_array(inner: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_string = false;
    for (index, ch) in inner.char_indices() {
        match ch {
            '"' => in_string = !in_string,
            ',' if !in_string => {
                parts.push(&inner[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);
    parts
}
