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
    /// `[dependencies]` and `[dev-dependencies]` as name to version requirement.
    pub dependencies: BTreeMap<String, String>,
    pub dev_dependencies: BTreeMap<String, String>,
    /// Capabilities build scripts and macros are granted.
    pub build_capabilities: Vec<String>,
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
            dependencies: BTreeMap::new(),
            dev_dependencies: BTreeMap::new(),
            build_capabilities: Vec::new(),
            path: None,
        }
    }

    /// Parse a manifest. Unknown keys are ignored so that newer manifests keep
    /// loading in older toolchains, which is what the edition policy expects.
    pub fn parse(text: &str, path: Option<PathBuf>) -> Result<Manifest, String> {
        let tables = parse_tables(text)?;
        let package = tables.get("package");
        let name = package
            .and_then(|table| table.get("name"))
            .and_then(TomlValue::as_str)
            .ok_or_else(|| "manifest is missing `[package] name`".to_string())?
            .to_string();
        let mut manifest = Manifest::default_for(&name);
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
        for (section, target) in [
            ("dependencies", &mut manifest.dependencies),
            ("dev-dependencies", &mut manifest.dev_dependencies),
        ] {
            if let Some(table) = tables.get(section) {
                for (key, value) in table {
                    if let Some(text) = value.as_str() {
                        target.insert(key.clone(), text.to_string());
                    }
                }
            }
        }
        Ok(manifest)
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
        for (name, requirement) in &self.dependencies {
            out.push_str(&format!("{name} = \"{requirement}\"\n"));
        }
        out.push_str("\n[dev-dependencies]\n");
        for (name, requirement) in &self.dev_dependencies {
            out.push_str(&format!("{name} = \"{requirement}\"\n"));
        }
        out.push_str("\n[build]\n");
        out.push_str(&format!("target = \"{}\"\n", self.target));
        out.push_str(&format!("opt-level = {}\n", self.opt_level));
        out
    }
}

type Table = BTreeMap<String, TomlValue>;

fn parse_tables(text: &str) -> Result<BTreeMap<String, Table>, String> {
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
