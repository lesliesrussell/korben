//! Dependency resolution and the lockfile.
//!
//! Specification 21 asks for semantic version ranges, deterministic resolution,
//! a fully pinned lockfile, mandatory checksums, and offline builds. This is
//! that, over the two sources that need no network: a local path, and a package
//! directory in a local registry.
//!
//! The property that matters is in acceptance criterion 10: a build reproduces
//! from the lockfile. When `korben.lock` is present and agrees with the
//! manifest, resolution does not run at all — the locked versions are used
//! verbatim and their checksums are verified, so a dependency that changed
//! underneath you is an error rather than a silent difference.

// korben-sdg

use crate::hash::{checksum, hex, Sha256};
use crate::manifest::{Dependency, Manifest};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

pub const LOCK_NAME: &str = "korben.lock";
/// The lockfile format version, so a future change can be detected.
pub const LOCK_VERSION: i64 = 1;

/// Set to skip checksum verification. Reported by `doctor` and `audit`.
pub const SKIP_CHECKSUMS: &str = "KORBEN_SKIP_CHECKSUMS";
/// Overrides where registry packages are looked up.
pub const REGISTRY_ENV: &str = "KORBEN_REGISTRY";

// ------------------------------------------------------------------ versions

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// A pre-release tag sorts below the same version without one.
    pub pre: Option<String>,
}

impl Version {
    pub fn parse(text: &str) -> Result<Version, String> {
        let text = text.trim();
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre.to_string())),
            None => (text, None),
        };
        let mut parts = core.split('.');
        let mut number = |what: &str| -> Result<u64, String> {
            match parts.next() {
                Some(part) => {
                    part.parse::<u64>().map_err(|_| format!("`{text}` has a non-numeric {what}"))
                }
                None => Ok(0),
            }
        };
        let major = number("major version")?;
        let minor = number("minor version")?;
        let patch = number("patch version")?;
        if parts.next().is_some() {
            return Err(format!("`{text}` has too many version components"));
        }
        Ok(Version { major, minor, patch, pre })
    }
}

/// Semantic version ordering: a pre-release sorts *below* the release it
/// precedes, which is the opposite of how `Option` orders on its own.
impl Ord for Version {
    fn cmp(&self, other: &Version) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(left), Some(right)) => left.cmp(right),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}.{}.{}", self.major, self.minor, self.patch)?;
        match &self.pre {
            Some(pre) => write!(out, "-{pre}"),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Op {
    Caret,
    Tilde,
    Exact,
    GreaterEq,
    Greater,
    LessEq,
    Less,
    Any,
}

/// A comma-separated set of version constraints, all of which must hold.
#[derive(Clone, Debug)]
pub struct Requirement {
    text: String,
    terms: Vec<(Op, Version)>,
}

impl Requirement {
    pub fn parse(text: &str) -> Result<Requirement, String> {
        let mut terms = Vec::new();
        for part in text.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if part == "*" {
                terms.push((Op::Any, Version { major: 0, minor: 0, patch: 0, pre: None }));
                continue;
            }
            let (op, rest) = if let Some(rest) = part.strip_prefix(">=") {
                (Op::GreaterEq, rest)
            } else if let Some(rest) = part.strip_prefix("<=") {
                (Op::LessEq, rest)
            } else if let Some(rest) = part.strip_prefix('^') {
                (Op::Caret, rest)
            } else if let Some(rest) = part.strip_prefix('~') {
                (Op::Tilde, rest)
            } else if let Some(rest) = part.strip_prefix('=') {
                (Op::Exact, rest)
            } else if let Some(rest) = part.strip_prefix('>') {
                (Op::Greater, rest)
            } else if let Some(rest) = part.strip_prefix('<') {
                (Op::Less, rest)
            } else {
                // A bare version means the same as `^`, which is what a caller
                // almost always intends by writing `"1.2.3"`.
                (Op::Caret, part)
            };
            terms.push((op, Version::parse(rest)?));
        }
        if terms.is_empty() {
            return Err(format!("`{text}` is not a version requirement"));
        }
        Ok(Requirement { text: text.trim().to_string(), terms })
    }

    pub fn any() -> Requirement {
        Requirement {
            text: "*".to_string(),
            terms: vec![(Op::Any, Version { major: 0, minor: 0, patch: 0, pre: None })],
        }
    }

    pub fn matches(&self, version: &Version) -> bool {
        self.terms.iter().all(|(op, bound)| matches_term(*op, bound, version))
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(&self.text)
    }
}

fn matches_term(op: Op, bound: &Version, version: &Version) -> bool {
    match op {
        Op::Any => true,
        Op::Exact => version == bound,
        Op::Greater => version > bound,
        Op::GreaterEq => version >= bound,
        Op::Less => version < bound,
        Op::LessEq => version <= bound,
        Op::Tilde => {
            version >= bound && version.major == bound.major && version.minor == bound.minor
        }
        // Caret allows changes that do not modify the left-most non-zero
        // component, which is what semantic versioning calls compatible.
        Op::Caret => {
            if version < bound {
                return false;
            }
            if bound.major > 0 {
                version.major == bound.major
            } else if bound.minor > 0 {
                version.major == 0 && version.minor == bound.minor
            } else {
                version.major == 0 && version.minor == 0 && version.patch == bound.patch
            }
        }
    }
}

// ------------------------------------------------------------------- sources

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Source {
    /// A directory on this machine, relative to the manifest that named it.
    Path(String),
    /// A package directory inside a registry root.
    Registry(String),
    // korben-mic
    /// A sibling package in this workspace, relative to the workspace root.
    /// Distinct from `Path` because it travels with the repository rather than
    /// pointing outside it.
    Member(String),
}

impl Source {
    /// The identity recorded in the lockfile.
    pub fn identity(&self) -> String {
        match self {
            Source::Path(path) => format!("path+{path}"),
            Source::Registry(root) => format!("registry+{root}"),
            Source::Member(path) => format!("member+{path}"),
        }
    }

    pub fn parse(text: &str) -> Option<Source> {
        if let Some(rest) = text.strip_prefix("path+") {
            return Some(Source::Path(rest.to_string()));
        }
        if let Some(rest) = text.strip_prefix("member+") {
            return Some(Source::Member(rest.to_string()));
        }
        text.strip_prefix("registry+").map(|rest| Source::Registry(rest.to_string()))
    }

    /// True when this source cannot be reproduced on another machine.
    pub fn is_local(&self) -> bool {
        matches!(self, Source::Path(_))
    }
}

/// A package that resolution selected.
#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub version: Version,
    pub source: Source,
    /// Where the package's files are on this machine.
    pub root: PathBuf,
    pub checksum: String,
    /// Names this package depends on, sorted.
    pub dependencies: Vec<String>,
}

/// The resolved dependency graph.
#[derive(Clone, Debug, Default)]
pub struct Resolution {
    pub packages: Vec<Package>,
}

impl Resolution {
    pub fn get(&self, name: &str) -> Option<&Package> {
        self.packages.iter().find(|package| package.name == name)
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }
}

/// The registry directory packages are looked up in.
pub fn registry_root(manifest: &Manifest) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(REGISTRY_ENV) {
        return Some(PathBuf::from(path));
    }
    if let Some(path) = &manifest.registry {
        return Some(PathBuf::from(path));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".korben/registry"))
}

/// Every version of `name` the registry offers, newest last.
fn registry_versions(root: &Path, name: &str) -> Vec<(Version, PathBuf)> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(root.join(name)) else { return found };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if !path.join(crate::project::MANIFEST_NAME).is_file() {
            continue;
        }
        let Some(text) = path.file_name().map(|name| name.to_string_lossy().to_string()) else {
            continue;
        };
        if let Ok(version) = Version::parse(&text) {
            found.push((version, path));
        }
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found
}

/// The content checksum of a package: its manifest and every source file.
pub fn package_checksum(root: &Path) -> String {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let manifest = root.join(crate::project::MANIFEST_NAME);
    if manifest.is_file() {
        files.push((crate::project::MANIFEST_NAME.to_string(), manifest));
    }
    for path in crate::project::source_files(&root.join("src")) {
        let relative =
            path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        files.push((relative, path));
    }
    // Sorted, and each entry length-prefixed, so the digest cannot be confused
    // by a rename that concatenates to the same bytes.
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut hasher = Sha256::new();
    for (name, path) in files {
        let contents = std::fs::read(&path).unwrap_or_default();
        hasher.update(&(name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        hasher.update(&(contents.len() as u64).to_be_bytes());
        hasher.update(&contents);
    }
    format!("sha256:{}", hex(&hasher.finish()))
}

// ---------------------------------------------------------------- resolution

/// How many times the resolution walk may repeat before giving up.
const MAX_RESOLUTION_PASSES: usize = 32;

/// Who asked for a requirement, so a conflict can name them.
#[derive(Clone)]
struct Demand {
    requirement: Requirement,
    by: String,
}

/// Resolve a manifest's dependencies deterministically.
///
/// Every requirement on a name is collected, and the highest version satisfying
/// all of them is chosen. A name that cannot satisfy every requirement is a
/// conflict, reported with the requirements and who made them.
pub fn resolve(root: &Path, manifest: &Manifest) -> Result<Resolution, String> {
    let seed = vec![(
        manifest.name.clone(),
        root.to_path_buf(),
        manifest.dependencies.iter().chain(&manifest.dev_dependencies).cloned().collect(),
    )];
    resolve_all(manifest, &seed, &[])
}

// korben-mic
/// Resolve several packages at once, as a workspace does.
///
/// `seed` is what each package declares, and `members` are the packages the
/// workspace itself provides -- a member depended on by name resolves to the
/// directory in this repository rather than to a registry, which is what makes
/// members able to depend on each other without a path.
pub fn resolve_all(
    manifest: &Manifest,
    seed: &[(String, PathBuf, Vec<Dependency>)],
    members: &[(String, PathBuf)],
) -> Result<Resolution, String> {
    let registry = registry_root(manifest);
    // Sources are recorded relative to wherever the lockfile will sit, which is
    // the directory of the manifest resolution was started from.
    let lock_root = manifest.path.as_deref().and_then(Path::parent).unwrap_or(Path::new("."));
    let mut demands: BTreeMap<String, Vec<Demand>> = BTreeMap::new();

    // Resolution is a fixpoint. A requirement discovered deep in the graph can
    // invalidate a choice made earlier, so the walk repeats with the demands it
    // has accumulated until nothing changes. That makes the result independent
    // of the order dependencies happen to be declared in.
    for _ in 0..MAX_RESOLUTION_PASSES {
        let mut chosen: BTreeMap<String, Package> = BTreeMap::new();
        let mut changed = false;
        let mut queue: Vec<(String, PathBuf, Vec<Dependency>)> = seed.to_vec();

        while let Some((requirer, base, dependencies)) = queue.pop() {
            for dependency in dependencies {
                let requirement = Requirement::parse(&dependency.requirement)
                    .map_err(|error| format!("`{}`: {error}", dependency.name))?;
                let entry = demands.entry(dependency.name.clone()).or_default();
                if !entry.iter().any(|demand| {
                    demand.by == requirer && demand.requirement.text() == requirement.text()
                }) {
                    entry.push(Demand { requirement, by: requirer.clone() });
                    changed = true;
                }

                if chosen.contains_key(&dependency.name) {
                    continue;
                }

                // A path dependency has exactly one candidate: the directory named.
                let candidates: Vec<(Version, PathBuf, Source)> = match &dependency.path {
                    Some(relative) => {
                        let directory = normalize(&base.join(relative));
                        let found = Manifest::load(&directory.join(crate::project::MANIFEST_NAME))
                            .map_err(|error| {
                                format!("dependency `{}` at {}: {error}", dependency.name, relative)
                            })?;
                        let version = Version::parse(&found.version).map_err(|error| {
                            format!("dependency `{}`: {error}", dependency.name)
                        })?;
                        if found.name != dependency.name {
                            return Err(format!(
                                "dependency `{}` at {relative} calls itself `{}`",
                                dependency.name, found.name
                            ));
                        }
                        // korben-mic
                        // Recorded relative to the lockfile, not to the
                        // manifest that wrote it. The two coincide for a
                        // top-level dependency in a plain project, and diverge
                        // for a workspace member or a transitive path
                        // dependency -- where the old spelling resolved against
                        // the wrong base.
                        let recorded = relative_to(lock_root, &directory);
                        vec![(version, directory, Source::Path(recorded))]
                    }
                    // A sibling in the same workspace is right here, and
                    // reaching past it to a registry would be wrong even when
                    // the registry has a package by that name.
                    None if members.iter().any(|(name, _)| *name == dependency.name) => {
                        let directory = members
                            .iter()
                            .find(|(name, _)| *name == dependency.name)
                            .map(|(_, path)| path.clone())
                            .expect("the member just matched");
                        let found = Manifest::load(&directory.join(crate::project::MANIFEST_NAME))
                            .map_err(|error| {
                                format!("workspace member `{}`: {error}", dependency.name)
                            })?;
                        let version = Version::parse(&found.version).map_err(|error| {
                            format!("workspace member `{}`: {error}", dependency.name)
                        })?;
                        let recorded = relative_to(lock_root, &directory);
                        vec![(version, directory.clone(), Source::Member(recorded))]
                    }
                    None => {
                        let Some(registry) = &registry else {
                            return Err(format!(
                                "dependency `{}` needs a registry, and none is configured",
                                dependency.name
                            ));
                        };
                        registry_versions(registry, &dependency.name)
                            .into_iter()
                            .map(|(version, path)| {
                                (version, path, Source::Registry(registry.display().to_string()))
                            })
                            .collect()
                    }
                };

                let all = &demands[&dependency.name];
                // Candidates are sorted ascending, so the last match is the
                // highest version satisfying every requirement.
                let Some((version, directory, source)) =
                    candidates.into_iter().rfind(|(version, _, _)| {
                        all.iter().all(|demand| demand.requirement.matches(version))
                    })
                else {
                    return Err(conflict_message(&dependency.name, all, registry.as_deref()));
                };

                let found = Manifest::load(&directory.join(crate::project::MANIFEST_NAME))
                    .map_err(|error| format!("dependency `{}`: {error}", dependency.name))?;
                let mut names: Vec<String> =
                    found.dependencies.iter().map(|entry| entry.name.clone()).collect();
                names.sort();
                names.dedup();

                chosen.insert(
                    dependency.name.clone(),
                    Package {
                        name: dependency.name.clone(),
                        version,
                        source,
                        root: directory.clone(),
                        checksum: package_checksum(&directory),
                        dependencies: names,
                    },
                );
                // A dependency's own dependencies are resolved the same way.
                queue.push((dependency.name.clone(), directory, found.dependencies.clone()));
            }
        }

        if !changed {
            return Ok(Resolution { packages: chosen.into_values().collect() });
        }
    }
    Err("dependency resolution did not settle; the graph may be cyclic".to_string())
}

fn conflict_message(name: &str, demands: &[Demand], registry: Option<&Path>) -> String {
    let mut lines = vec![format!("no version of `{name}` satisfies every requirement")];
    for demand in demands {
        lines.push(format!("  `{}` requires {}", demand.by, demand.requirement));
    }
    match registry {
        Some(registry) => {
            let available = registry_versions(registry, name);
            if available.is_empty() {
                lines.push(format!("  no versions of `{name}` are in {}", registry.display()));
            } else {
                let versions: Vec<String> =
                    available.iter().map(|(version, _)| version.to_string()).collect();
                lines.push(format!("  available: {}", versions.join(", ")));
            }
        }
        None => lines.push("  no registry is configured".to_string()),
    }
    lines.join("\n")
}

// korben-mic
/// Express `target` relative to `base`, so a lockfile records a path that means
/// the same thing on another machine.
fn relative_to(base: &Path, target: &Path) -> String {
    let base = normalize(base);
    let target = normalize(target);
    if let Ok(rest) = target.strip_prefix(&base) {
        let text = rest.to_string_lossy().to_string();
        return if text.is_empty() { ".".to_string() } else { text };
    }
    // Outside the root: climb to the nearest shared ancestor, then descend.
    let base_parts: Vec<_> = base.components().collect();
    let target_parts: Vec<_> = target.components().collect();
    let shared =
        base_parts.iter().zip(&target_parts).take_while(|(left, right)| left == right).count();
    let mut parts: Vec<String> = vec!["..".to_string(); base_parts.len() - shared];
    parts.extend(
        target_parts[shared..].iter().map(|part| part.as_os_str().to_string_lossy().to_string()),
    );
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// Collapse `..` so a path dependency reads clearly in diagnostics.
fn normalize(path: &Path) -> PathBuf {
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                if matches!(parts.last().map(|part| part.as_os_str()), Some(last) if last != "..") {
                    parts.pop();
                } else {
                    parts.push("..".into());
                }
            }
            std::path::Component::CurDir => {}
            other => parts.push(other.as_os_str().to_os_string()),
        }
    }
    parts.iter().collect()
}

// ------------------------------------------------------------------ lockfile

/// A parsed `korben.lock`.
#[derive(Clone, Debug, Default)]
pub struct Lockfile {
    pub root: String,
    /// Digest of the manifest's dependency declarations when the lock was written.
    pub manifest_digest: String,
    pub packages: Vec<LockedPackage>,
}

#[derive(Clone, Debug)]
pub struct LockedPackage {
    pub name: String,
    pub version: Version,
    pub source: Source,
    pub checksum: String,
    pub dependencies: Vec<String>,
}

/// A digest over exactly the declarations that affect resolution, so an
/// unrelated manifest edit does not invalidate the lock.
pub fn manifest_digest(manifest: &Manifest) -> String {
    let mut lines: Vec<String> = Vec::new();
    for dependency in manifest.dependencies.iter().chain(&manifest.dev_dependencies) {
        lines.push(format!(
            "{}|{}|{}|{}",
            dependency.name,
            dependency.requirement,
            dependency.path.clone().unwrap_or_default(),
            dependency.dev
        ));
    }
    lines.sort();
    checksum(lines.join("\n").as_bytes())
}

// korben-mic
/// The digest a workspace's lock is keyed on: every member's declarations, so
/// adding a dependency to any member invalidates the lock rather than only the
/// member that changed.
pub fn workspace_digest(members: &[&Manifest]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for manifest in members {
        // A member's version is part of the digest because a sibling that
        // depends on it resolves against that version: bumping it has to
        // invalidate the lock the same way changing a requirement does.
        lines.push(format!(
            "{}\u{1f}{}\u{1f}{}",
            manifest.name,
            manifest.version,
            manifest_digest(manifest)
        ));
    }
    lines.sort();
    checksum(lines.join("\n").as_bytes())
}

impl Lockfile {
    pub fn from_resolution(manifest: &Manifest, resolution: &Resolution) -> Lockfile {
        Lockfile {
            root: manifest.name.clone(),
            manifest_digest: manifest_digest(manifest),
            packages: resolution
                .packages
                .iter()
                .map(|package| LockedPackage {
                    name: package.name.clone(),
                    version: package.version.clone(),
                    source: package.source.clone(),
                    checksum: package.checksum.clone(),
                    dependencies: package.dependencies.clone(),
                })
                .collect(),
        }
    }

    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        out.push_str("# korben.lock — generated by korben. Do not edit by hand.\n");
        out.push_str("# It pins every dependency so a build reproduces exactly.\n\n");
        let _ = writeln!(out, "version = {LOCK_VERSION}");
        let _ = writeln!(out, "root = \"{}\"", self.root);
        let _ = writeln!(out, "manifest = \"{}\"", self.manifest_digest);
        for package in &self.packages {
            let _ = writeln!(out, "\n[package.{}]", package.name);
            let _ = writeln!(out, "version = \"{}\"", package.version);
            let _ = writeln!(out, "source = \"{}\"", package.source.identity());
            let _ = writeln!(out, "checksum = \"{}\"", package.checksum);
            let names: Vec<String> =
                package.dependencies.iter().map(|name| format!("\"{name}\"")).collect();
            let _ = writeln!(out, "dependencies = [{}]", names.join(", "));
        }
        out
    }

    pub fn parse(text: &str) -> Result<Lockfile, String> {
        let tables = crate::manifest::parse_tables(text)?;
        let top = tables.get("").ok_or_else(|| "lockfile is empty".to_string())?;
        let version = top.get("version").and_then(|value| value.as_int()).unwrap_or(0);
        if version != LOCK_VERSION {
            return Err(format!(
                "lockfile format version {version} is not supported by this toolchain"
            ));
        }
        let mut lock = Lockfile {
            root: top.get("root").and_then(|value| value.as_str()).unwrap_or("").to_string(),
            manifest_digest: top
                .get("manifest")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            packages: Vec::new(),
        };
        for (section, table) in &tables {
            let Some(name) = section.strip_prefix("package.") else { continue };
            let version = table
                .get("version")
                .and_then(|value| value.as_str())
                .ok_or_else(|| format!("locked package `{name}` has no version"))?;
            let source = table
                .get("source")
                .and_then(|value| value.as_str())
                .and_then(Source::parse)
                .ok_or_else(|| format!("locked package `{name}` has no usable source"))?;
            let checksum = table
                .get("checksum")
                .and_then(|value| value.as_str())
                .ok_or_else(|| format!("locked package `{name}` has no checksum"))?;
            let dependencies = match table.get("dependencies") {
                Some(crate::manifest::TomlValue::Array(items)) => {
                    items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect()
                }
                _ => Vec::new(),
            };
            lock.packages.push(LockedPackage {
                name: name.to_string(),
                version: Version::parse(version)
                    .map_err(|error| format!("locked package `{name}`: {error}"))?,
                source,
                checksum: checksum.to_string(),
                dependencies,
            });
        }
        lock.packages.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(lock)
    }

    pub fn load(path: &Path) -> Result<Lockfile, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        Lockfile::parse(&text)
    }

    /// Turn locked entries back into packages, locating and verifying each one.
    ///
    /// This is the reproducible path: no resolution runs, and a package whose
    /// contents no longer match its checksum is an error.
    pub fn materialize(&self, root: &Path, manifest: &Manifest) -> Result<Resolution, String> {
        let registry = registry_root(manifest);
        let verify = std::env::var_os(SKIP_CHECKSUMS).is_none();
        let mut packages = Vec::with_capacity(self.packages.len());
        for locked in &self.packages {
            let directory = match &locked.source {
                Source::Path(relative) => normalize(&root.join(relative)),
                Source::Registry(recorded) => {
                    let base = registry.clone().unwrap_or_else(|| PathBuf::from(recorded));
                    base.join(&locked.name).join(locked.version.to_string())
                }
                // A member is relative to the workspace root, which is the root
                // the lockfile sits in.
                Source::Member(relative) => normalize(&root.join(relative)),
            };
            if !directory.join(crate::project::MANIFEST_NAME).is_file() {
                return Err(format!(
                    "locked dependency `{}` is not at {}\n  run `korben update` if it moved",
                    locked.name,
                    directory.display()
                ));
            }
            let actual = package_checksum(&directory);
            // korben-mic
            // A checksum pins a dependency so that what is built is what was
            // reviewed. A workspace member is not that: it is source in this
            // repository that the author is editing, and every keystroke would
            // otherwise stop the build demanding `korben update`. Its integrity
            // is the repository's business, not the lockfile's.
            let pinned = !matches!(locked.source, Source::Member(_));
            if verify && pinned && actual != locked.checksum {
                return Err(format!(
                    "dependency `{}` has changed since it was locked\n  \
                     locked:  {}\n  found:   {}\n  \
                     run `korben update` to accept the change",
                    locked.name, locked.checksum, actual
                ));
            }
            packages.push(Package {
                name: locked.name.clone(),
                version: locked.version.clone(),
                source: locked.source.clone(),
                root: directory,
                checksum: actual,
                dependencies: locked.dependencies.clone(),
            });
        }
        Ok(Resolution { packages })
    }

    /// True when the lock still describes this manifest's declarations.
    pub fn matches(&self, manifest: &Manifest) -> bool {
        self.manifest_digest == manifest_digest(manifest)
    }

    // korben-mic
    /// A lock for a whole workspace, keyed on every member's declarations.
    pub fn from_workspace(name: &str, members: &[&Manifest], resolution: &Resolution) -> Lockfile {
        Lockfile {
            root: name.to_string(),
            manifest_digest: workspace_digest(members),
            packages: resolution
                .packages
                .iter()
                .map(|package| LockedPackage {
                    name: package.name.clone(),
                    version: package.version.clone(),
                    source: package.source.clone(),
                    checksum: package.checksum.clone(),
                    dependencies: package.dependencies.clone(),
                })
                .collect(),
        }
    }

    /// True when the lock still describes every member's declarations.
    pub fn matches_workspace(&self, members: &[&Manifest]) -> bool {
        self.manifest_digest == workspace_digest(members)
    }
}

/// Names each package may import, including itself.
pub fn visibility(
    manifest: &Manifest,
    resolution: &Resolution,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let root = map.entry(manifest.name.clone()).or_default();
    root.insert(manifest.name.clone());
    for dependency in manifest.dependencies.iter().chain(&manifest.dev_dependencies) {
        root.insert(dependency.name.clone());
    }
    for package in &resolution.packages {
        let entry = map.entry(package.name.clone()).or_default();
        entry.insert(package.name.clone());
        for name in &package.dependencies {
            entry.insert(name.clone());
        }
    }
    map
}
