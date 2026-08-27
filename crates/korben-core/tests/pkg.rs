//! Versions, resolution, and the lockfile.

use korben_core::manifest::Manifest;
use korben_core::pkg::{
    package_checksum, resolve, Lockfile, Requirement, Resolution, Source, Version,
};
use std::path::{Path, PathBuf};

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!(
            "korben-pkg-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = PathBuf::from(path.to_string_lossy().replace(['(', ')', ' '], ""));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch");
        Scratch(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Write a package with the given manifest body and one source module.
fn package(root: &Path, name: &str, version: &str, extra: &str) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("korben.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"{version}\"\nlicense = \"MIT\"\nmain = \"{name}\"\n{extra}"
        ),
    )
    .unwrap();
    std::fs::write(
        root.join(format!("src/{name}.kb")),
        format!("(module {name})\n\n(pub fn tag [] -> String \"{name} {version}\")\n"),
    )
    .unwrap();
}

fn version(text: &str) -> Version {
    Version::parse(text).expect(text)
}

// ------------------------------------------------------------------ versions

#[test]
fn versions_parse_and_order() {
    assert_eq!(version("1.2.3").to_string(), "1.2.3");
    assert_eq!(version("0.1").to_string(), "0.1.0");
    assert_eq!(version("2").to_string(), "2.0.0");
    assert_eq!(version("1.0.0-alpha").to_string(), "1.0.0-alpha");
    assert!(version("1.0.0") > version("0.9.9"));
    assert!(version("1.2.10") > version("1.2.9"));
    // A pre-release sorts below the release it precedes.
    assert!(version("1.0.0-alpha") < version("1.0.0"));
    assert!(Version::parse("1.x.3").is_err());
    assert!(Version::parse("1.2.3.4").is_err());
}

#[test]
fn caret_allows_compatible_changes_only() {
    let requirement = Requirement::parse("^1.2.3").unwrap();
    assert!(requirement.matches(&version("1.2.3")));
    assert!(requirement.matches(&version("1.9.0")));
    assert!(!requirement.matches(&version("1.2.2")));
    assert!(!requirement.matches(&version("2.0.0")));

    // Below 1.0 the minor version is the compatibility boundary.
    let zero = Requirement::parse("^0.2.3").unwrap();
    assert!(zero.matches(&version("0.2.9")));
    assert!(!zero.matches(&version("0.3.0")));
    assert!(!zero.matches(&version("0.2.2")));

    let patch = Requirement::parse("^0.0.3").unwrap();
    assert!(patch.matches(&version("0.0.3")));
    assert!(!patch.matches(&version("0.0.4")));
}

#[test]
fn other_operators_behave() {
    assert!(Requirement::parse("~1.2.3").unwrap().matches(&version("1.2.9")));
    assert!(!Requirement::parse("~1.2.3").unwrap().matches(&version("1.3.0")));
    assert!(Requirement::parse("=1.2.3").unwrap().matches(&version("1.2.3")));
    assert!(!Requirement::parse("=1.2.3").unwrap().matches(&version("1.2.4")));
    assert!(Requirement::parse("*").unwrap().matches(&version("9.9.9")));
    assert!(Requirement::parse(">=1.0, <2.0").unwrap().matches(&version("1.5.0")));
    assert!(!Requirement::parse(">=1.0, <2.0").unwrap().matches(&version("2.0.0")));
    // A bare version means the same as caret.
    assert!(Requirement::parse("1.2.3").unwrap().matches(&version("1.9.0")));
    assert!(!Requirement::parse("1.2.3").unwrap().matches(&version("2.0.0")));
}

// --------------------------------------------------------------- resolution

#[test]
fn a_path_dependency_resolves_to_the_directory_named() {
    let scratch = Scratch::new("path");
    package(&scratch.path().join("json"), "json", "0.3.1", "");
    package(
        &scratch.path().join("app"),
        "app",
        "0.1.0",
        "\n[dependencies.json]\npath = \"../json\"\n",
    );

    let root = scratch.path().join("app");
    let manifest = Manifest::load(&root.join("korben.toml")).unwrap();
    let resolution = resolve(&root, &manifest).expect("resolve");
    assert_eq!(resolution.packages.len(), 1);
    let package = &resolution.packages[0];
    assert_eq!(package.name, "json");
    assert_eq!(package.version.to_string(), "0.3.1");
    assert_eq!(package.source, Source::Path("../json".to_string()));
    assert!(package.checksum.starts_with("sha256:"));
}

#[test]
fn a_registry_dependency_picks_the_highest_compatible_version() {
    let scratch = Scratch::new("registry");
    let registry = scratch.path().join("registry");
    for version in ["0.1.0", "0.2.0", "0.2.7", "0.3.0"] {
        package(&registry.join("text").join(version), "text", version, "");
    }
    package(
        &scratch.path().join("app"),
        "app",
        "0.1.0",
        &format!(
            "\n[registry]\npath = \"{}\"\n\n[dependencies]\ntext = \"^0.2\"\n",
            registry.display()
        ),
    );

    let root = scratch.path().join("app");
    let manifest = Manifest::load(&root.join("korben.toml")).unwrap();
    let resolution = resolve(&root, &manifest).expect("resolve");
    assert_eq!(resolution.packages.len(), 1);
    // 0.3.0 exists but is not compatible with `^0.2`.
    assert_eq!(resolution.packages[0].version.to_string(), "0.2.7");
}

#[test]
fn transitive_requirements_are_intersected() {
    let scratch = Scratch::new("transitive");
    let registry = scratch.path().join("registry");
    for version in ["1.0.0", "1.4.0", "1.9.0", "2.0.0"] {
        package(&registry.join("core").join(version), "core", version, "");
    }
    package(
        &registry.join("mid").join("0.1.0"),
        "mid",
        "0.1.0",
        "\n[dependencies]\ncore = \">=1.0, <1.5\"\n",
    );
    package(
        &scratch.path().join("app"),
        "app",
        "0.1.0",
        &format!(
            "\n[registry]\npath = \"{}\"\n\n[dependencies]\nmid = \"^0.1\"\ncore = \"^1.0\"\n",
            registry.display()
        ),
    );

    let root = scratch.path().join("app");
    let manifest = Manifest::load(&root.join("korben.toml")).unwrap();
    let resolution = resolve(&root, &manifest).expect("resolve");
    let core = resolution.get("core").expect("core resolved");
    // `^1.0` alone would take 1.9.0; `mid` narrows it to below 1.5.
    assert_eq!(core.version.to_string(), "1.4.0");
}

#[test]
fn a_conflict_names_every_requirement_and_who_made_it() {
    let scratch = Scratch::new("conflict");
    let registry = scratch.path().join("registry");
    for version in ["1.0.0", "2.0.0"] {
        package(&registry.join("core").join(version), "core", version, "");
    }
    package(
        &registry.join("mid").join("0.1.0"),
        "mid",
        "0.1.0",
        "\n[dependencies]\ncore = \"^1.0\"\n",
    );
    package(
        &scratch.path().join("app"),
        "app",
        "0.1.0",
        &format!(
            "\n[registry]\npath = \"{}\"\n\n[dependencies]\nmid = \"^0.1\"\ncore = \"^2.0\"\n",
            registry.display()
        ),
    );

    let root = scratch.path().join("app");
    let manifest = Manifest::load(&root.join("korben.toml")).unwrap();
    let error = resolve(&root, &manifest).expect_err("expected a conflict");
    assert!(error.contains("no version of `core`"), "{error}");
    assert!(error.contains("requires ^2.0"), "{error}");
    assert!(error.contains("requires ^1.0"), "{error}");
    assert!(error.contains("available: 1.0.0, 2.0.0"), "{error}");
}

// ----------------------------------------------------------------- checksums

#[test]
fn a_checksum_covers_the_manifest_and_every_source_file() {
    let scratch = Scratch::new("checksum");
    let root = scratch.path().join("lib");
    package(&root, "lib", "0.1.0", "");
    let original = package_checksum(&root);

    // Touching a source file changes the digest.
    std::fs::write(root.join("src/lib.kb"), "(module lib)\n").unwrap();
    let edited = package_checksum(&root);
    assert_ne!(original, edited);

    // So does touching the manifest.
    let manifest = std::fs::read_to_string(root.join("korben.toml")).unwrap();
    std::fs::write(root.join("korben.toml"), format!("{manifest}# comment\n")).unwrap();
    assert_ne!(edited, package_checksum(&root));

    // And it is stable when nothing changes.
    assert_eq!(package_checksum(&root), package_checksum(&root));
}

// ------------------------------------------------------------------ lockfile

#[test]
fn a_lockfile_round_trips() {
    let scratch = Scratch::new("lock");
    package(&scratch.path().join("json"), "json", "0.3.1", "");
    package(
        &scratch.path().join("app"),
        "app",
        "0.1.0",
        "\n[dependencies.json]\npath = \"../json\"\n",
    );
    let root = scratch.path().join("app");
    let manifest = Manifest::load(&root.join("korben.toml")).unwrap();
    let resolution = resolve(&root, &manifest).unwrap();

    let lock = Lockfile::from_resolution(&manifest, &resolution);
    let text = lock.render();
    assert!(text.contains("[package.json]"), "{text}");
    assert!(text.contains("source = \"path+../json\""), "{text}");

    let parsed = Lockfile::parse(&text).expect("parse");
    assert_eq!(parsed.root, "app");
    assert_eq!(parsed.packages.len(), 1);
    assert_eq!(parsed.packages[0].version.to_string(), "0.3.1");
    assert_eq!(parsed.packages[0].checksum, resolution.packages[0].checksum);
    assert!(parsed.matches(&manifest));
}

#[test]
fn a_lockfile_detects_a_changed_dependency() {
    let scratch = Scratch::new("tamper");
    let library = scratch.path().join("json");
    package(&library, "json", "0.3.1", "");
    package(
        &scratch.path().join("app"),
        "app",
        "0.1.0",
        "\n[dependencies.json]\npath = \"../json\"\n",
    );
    let root = scratch.path().join("app");
    let manifest = Manifest::load(&root.join("korben.toml")).unwrap();
    let lock = Lockfile::from_resolution(&manifest, &resolve(&root, &manifest).unwrap());

    // Reproducing an unchanged tree succeeds.
    lock.materialize(&root, &manifest).expect("unchanged");

    // Changing the dependency is an error rather than a silent difference.
    std::fs::write(library.join("src/json.kb"), "(module json)\n").unwrap();
    let error = lock.materialize(&root, &manifest).expect_err("expected a mismatch");
    assert!(error.contains("has changed since it was locked"), "{error}");
    assert!(error.contains("korben update"), "{error}");
}

#[test]
fn a_lockfile_notices_when_the_manifest_moved_on() {
    let scratch = Scratch::new("stale");
    package(&scratch.path().join("json"), "json", "0.3.1", "");
    package(
        &scratch.path().join("app"),
        "app",
        "0.1.0",
        "\n[dependencies.json]\npath = \"../json\"\n",
    );
    let root = scratch.path().join("app");
    let manifest = Manifest::load(&root.join("korben.toml")).unwrap();
    let lock = Lockfile::from_resolution(&manifest, &resolve(&root, &manifest).unwrap());
    assert!(lock.matches(&manifest));

    let mut changed = manifest.clone();
    changed.dependencies.clear();
    assert!(!lock.matches(&changed), "dropping a dependency must invalidate the lock");
}

#[test]
fn an_unsupported_lock_version_is_refused() {
    let error = Lockfile::parse("version = 99\nroot = \"app\"\n").expect_err("refused");
    assert!(error.contains("format version 99"), "{error}");
}

#[test]
fn an_empty_resolution_locks_nothing() {
    let resolution = Resolution::default();
    assert!(resolution.is_empty());
    let manifest = Manifest::default_for("app");
    let lock = Lockfile::from_resolution(&manifest, &resolution);
    assert!(lock.packages.is_empty());
}

// -------------------------------------------------------------- supply chain

#[test]
fn install_scripts_are_refused_outright() {
    for body in [
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\ninstall = \"curl | sh\"\n",
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n\n[scripts]\npostinstall = \"x\"\n",
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n\n[build]\nscript = \"build.sh\"\n",
    ] {
        let error = Manifest::parse(body, None).expect_err("must be refused");
        assert!(error.contains("install time"), "{error}");
        assert!(error.contains("21.3"), "{error}");
    }
}

#[test]
fn an_ordinary_manifest_is_still_accepted() {
    let manifest = Manifest::parse(
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n\n[dependencies]\njson = \"^0.1\"\n",
        None,
    )
    .expect("accepted");
    assert_eq!(manifest.dependencies.len(), 1);
    assert_eq!(manifest.dependencies[0].requirement, "^0.1");
}

/// The toolchain carries the runtime's source so generated projects build
/// offline. A new runtime module that is not carried would only fail later, in
/// someone else's `korben build`, so it is checked here.
#[test]
fn every_runtime_source_file_is_vendored_into_generated_projects() {
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory")
        .join("korben-runtime/src");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(&runtime).expect("read runtime sources") {
        let path = entry.expect("entry").path();
        if path.extension().map(|extension| extension != "rs").unwrap_or(true) {
            continue;
        }
        let name = format!("src/{}", path.file_name().unwrap().to_string_lossy());
        if !korben_core::codegen::RUNTIME_FILES.iter().any(|(carried, _)| *carried == name) {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "these runtime files are not carried into generated projects: {missing:?}\n\
         add them to RUNTIME_FILES in codegen.rs"
    );
}
