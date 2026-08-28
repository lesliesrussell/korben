//! Several packages in one repository.

// korben-mic

mod common;

use std::path::{Path, PathBuf};

use korben_core::manifest::Manifest;
use korben_core::pkg::{Lockfile, Source, LOCK_NAME};
use korben_core::project::Session;
use korben_core::workspace::Workspace;

/// A scratch directory that cleans itself up.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let unique =
            format!("korben-ws-{label}-{}-{:?}", std::process::id(), std::thread::current().id());
        let path = std::env::temp_dir().join(unique.replace(['(', ')', ' '], ""));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch");
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

/// Write a workspace root listing `members`.
fn root(path: &Path, members: &[&str], package: Option<&str>) {
    let listed = members.iter().map(|name| format!("\"{name}\"")).collect::<Vec<_>>().join(", ");
    let mut text = String::new();
    if let Some(name) = package {
        text.push_str(&format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nlicense = \"MIT\"\nmain = \"{name}\"\n\n"
        ));
    }
    text.push_str(&format!("[workspace]\nmembers = [{listed}]\n"));
    std::fs::write(path.join("korben.toml"), text).expect("write root manifest");
}

/// Write a member package, optionally with dependencies and a `main`.
fn member(base: &Path, name: &str, version: &str, deps: &str, program: bool) -> PathBuf {
    let path = base.join(name);
    std::fs::create_dir_all(path.join("src")).expect("create member");
    std::fs::write(
        path.join("korben.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"{version}\"\nlicense = \"MIT\"\nmain = \"{name}\"\n{deps}"
        ),
    )
    .expect("write member manifest");
    let body = if program {
        format!(
            "(module {name})\n\n;;; Entry.\n(pub fn main [] -> Unit !io (println \"{name}\"))\n"
        )
    } else {
        format!("(module {name})\n\n;;; Tag.\n(pub fn tag [] -> String \"{name}\")\n")
    };
    std::fs::write(path.join(format!("src/{name}.kb")), body).expect("write member source");
    path
}

// ------------------------------------------------------------------ discovery

#[test]
fn a_root_manifest_may_be_a_workspace_without_being_a_package() {
    let scratch = Scratch::new("virtual");
    root(scratch.path(), &["a", "b"], None);
    member(scratch.path(), "a", "1.0.0", "", false);
    member(scratch.path(), "b", "1.0.0", "", false);

    let manifest = Manifest::load(&scratch.path().join("korben.toml")).expect("load");
    assert!(manifest.is_virtual, "a root with no `[package]` is virtual");
    assert!(manifest.is_workspace());

    let workspace = Workspace::find(scratch.path()).expect("find").expect("a workspace");
    let names: Vec<&str> = workspace.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b"]);
}

#[test]
fn a_root_that_is_also_a_package_is_a_member_of_its_own_workspace() {
    let scratch = Scratch::new("root-package");
    root(scratch.path(), &["a"], Some("top"));
    std::fs::create_dir_all(scratch.path().join("src")).expect("src");
    std::fs::write(scratch.path().join("src/top.kb"), "(module top)\n").expect("write");
    member(scratch.path(), "a", "1.0.0", "", false);

    let workspace = Workspace::find(scratch.path()).expect("find").expect("a workspace");
    let names: Vec<&str> = workspace.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, vec!["top", "a"]);
}

#[test]
fn a_member_finds_the_workspace_above_it() {
    let scratch = Scratch::new("from-member");
    root(scratch.path(), &["a", "b"], None);
    let a = member(scratch.path(), "a", "1.0.0", "", false);
    member(scratch.path(), "b", "1.0.0", "", false);

    let workspace = Workspace::find(&a).expect("find").expect("a workspace");
    assert_eq!(workspace.root, scratch.path());
    assert!(workspace.member_at(&a).is_some());
}

#[test]
fn a_parent_that_does_not_list_this_package_is_not_its_workspace() {
    let scratch = Scratch::new("unrelated-parent");
    root(scratch.path(), &["a"], None);
    member(scratch.path(), "a", "1.0.0", "", false);
    // `stray` sits inside the directory tree but is not a listed member.
    let stray = member(scratch.path(), "stray", "1.0.0", "", false);

    assert!(
        Workspace::find(&stray).expect("find").is_none(),
        "an unlisted package must not be captured by the workspace above it"
    );
}

#[test]
fn a_plain_project_is_not_a_workspace() {
    let scratch = Scratch::new("plain");
    member(scratch.path(), "solo", "1.0.0", "", true);
    assert!(Workspace::find(&scratch.path().join("solo")).expect("find").is_none());
}

// -------------------------------------------------------------------- errors

#[test]
fn a_member_that_is_not_there_is_reported() {
    let scratch = Scratch::new("missing-member");
    root(scratch.path(), &["a", "gone"], None);
    member(scratch.path(), "a", "1.0.0", "", false);

    // Silently treating this as "no workspace" would check one package and
    // report success for a repository that does not build.
    let error = Workspace::find(scratch.path()).expect_err("must be an error");
    assert!(error.contains("`gone`"), "{error}");
    assert!(error.contains("korben.toml"), "{error}");
}

#[test]
fn two_members_with_the_same_name_are_reported() {
    let scratch = Scratch::new("duplicate");
    root(scratch.path(), &["first", "second"], None);
    member(scratch.path(), "first", "1.0.0", "", false);
    let second = scratch.path().join("second");
    std::fs::create_dir_all(second.join("src")).expect("create");
    std::fs::write(
        second.join("korben.toml"),
        "[package]\nname = \"first\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\nmain = \"first\"\n",
    )
    .expect("write");

    let error = Workspace::find(scratch.path()).expect_err("must be an error");
    assert!(error.contains("both called `first`"), "{error}");
}

#[test]
fn a_member_without_a_package_name_is_reported() {
    let scratch = Scratch::new("virtual-member");
    root(scratch.path(), &["inner"], None);
    let inner = scratch.path().join("inner");
    std::fs::create_dir_all(inner.join("src")).expect("create");
    std::fs::write(inner.join("korben.toml"), "[workspace]\nmembers = []\n").expect("write");

    let error = Workspace::find(scratch.path()).expect_err("must be an error");
    assert!(error.contains("`[package] name`"), "{error}");
}

// ---------------------------------------------------------------- resolution

#[test]
fn a_member_may_depend_on_a_sibling_by_name() {
    let scratch = Scratch::new("sibling");
    root(scratch.path(), &["lib", "app"], None);
    member(scratch.path(), "lib", "1.2.0", "", false);
    member(scratch.path(), "app", "0.1.0", "\n[dependencies]\nlib = \"^1.2\"\n", true);

    let session = Session::open(&scratch.path().join("app")).expect("open");
    let resolved = session.resolution.get("lib").expect("lib resolved");
    assert_eq!(resolved.version.to_string(), "1.2.0");
    // A sibling comes from the workspace, not from a registry or an outside path.
    assert_eq!(resolved.source, Source::Member("lib".to_string()));
}

#[test]
fn one_lockfile_covers_the_whole_workspace() {
    let scratch = Scratch::new("one-lock");
    root(scratch.path(), &["lib", "app"], None);
    member(scratch.path(), "lib", "1.2.0", "", false);
    member(scratch.path(), "app", "0.1.0", "\n[dependencies]\nlib = \"^1.2\"\n", true);

    Session::open(&scratch.path().join("app")).expect("open");
    // The lock belongs to the workspace, not to the member that triggered it.
    assert!(scratch.path().join(LOCK_NAME).is_file(), "no lock at the workspace root");
    assert!(!scratch.path().join("app").join(LOCK_NAME).is_file(), "a member wrote its own lock");
}

#[test]
fn the_workspace_lockfile_reproduces_byte_for_byte() {
    let scratch = Scratch::new("reproducible");
    root(scratch.path(), &["lib", "app"], None);
    member(scratch.path(), "lib", "1.2.0", "", false);
    member(scratch.path(), "app", "0.1.0", "\n[dependencies]\nlib = \"^1.2\"\n", true);

    Session::open(scratch.path()).expect("open");
    let first = std::fs::read_to_string(scratch.path().join(LOCK_NAME)).expect("read");
    std::fs::remove_file(scratch.path().join(LOCK_NAME)).expect("remove");
    Session::open(scratch.path()).expect("reopen");
    let second = std::fs::read_to_string(scratch.path().join(LOCK_NAME)).expect("read");
    assert_eq!(first, second);
}

#[test]
fn editing_a_members_source_does_not_stop_the_build() {
    let scratch = Scratch::new("editable");
    root(scratch.path(), &["lib", "app"], None);
    member(scratch.path(), "lib", "1.2.0", "", false);
    member(scratch.path(), "app", "0.1.0", "\n[dependencies]\nlib = \"^1.2\"\n", true);
    Session::open(scratch.path()).expect("open");

    // A member is source being written, not a pinned artifact. Verifying its
    // checksum would stop the build on every edit demanding `korben update`.
    std::fs::write(
        scratch.path().join("lib").join("src/lib.kb"),
        "(module lib)\n\n;;; Tag.\n(pub fn tag [] -> String \"edited\")\n",
    )
    .expect("edit the member");
    Session::open(scratch.path()).expect("an edited member must still open");
}

#[test]
fn an_outside_dependency_is_still_pinned() {
    let scratch = Scratch::new("pinned");
    // The dependency lives outside the workspace, so it is a path dependency
    // and the lock still guarantees what gets built.
    let outside = scratch.path().join("outside");
    std::fs::create_dir_all(outside.join("src")).expect("create");
    std::fs::write(
        outside.join("korben.toml"),
        "[package]\nname = \"outside\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\nmain = \"outside\"\n",
    )
    .expect("write");
    std::fs::write(outside.join("src/outside.kb"), "(module outside)\n").expect("write");

    let repo = scratch.path().join("repo");
    std::fs::create_dir_all(&repo).expect("create");
    root(&repo, &["app"], None);
    member(&repo, "app", "0.1.0", "\n[dependencies.outside]\npath = \"../../outside\"\n", true);
    Session::open(&repo).expect("open");

    std::fs::write(outside.join("src/outside.kb"), "(module outside)\n;; edited\n").expect("edit");
    let error = match Session::open(&repo) {
        Ok(_) => panic!("a changed dependency must stop the build"),
        Err(error) => error,
    };
    assert!(error.contains("has changed since it was locked"), "{error}");
}

#[test]
fn a_dependency_added_to_any_member_invalidates_the_lock() {
    let scratch = Scratch::new("invalidate");
    root(scratch.path(), &["lib", "app"], None);
    member(scratch.path(), "lib", "1.2.0", "", false);
    member(scratch.path(), "app", "0.1.0", "\n[dependencies]\nlib = \"^1.2\"\n", true);
    Session::open(scratch.path()).expect("open");

    let lock = Lockfile::load(&scratch.path().join(LOCK_NAME)).expect("load");
    let before: Vec<Manifest> = ["lib", "app"]
        .iter()
        .map(|name| Manifest::load(&scratch.path().join(name).join("korben.toml")).expect("load"))
        .collect();
    assert!(lock.matches_workspace(&before.iter().collect::<Vec<_>>()));

    // A change in one member must invalidate the lock for the whole workspace,
    // since the resolution it pins covered every member at once.
    member(scratch.path(), "lib", "1.3.0", "", false);
    let after: Vec<Manifest> = ["lib", "app"]
        .iter()
        .map(|name| Manifest::load(&scratch.path().join(name).join("korben.toml")).expect("load"))
        .collect();
    assert!(!lock.matches_workspace(&after.iter().collect::<Vec<_>>()));
}

// ------------------------------------------------------------------ programs

#[test]
fn the_sole_program_is_chosen_without_being_named() {
    let scratch = Scratch::new("one-program");
    root(scratch.path(), &["lib", "app"], None);
    member(scratch.path(), "lib", "1.0.0", "", false);
    member(scratch.path(), "app", "0.1.0", "", true);

    let workspace = Workspace::find(scratch.path()).expect("find").expect("a workspace");
    assert_eq!(workspace.program(None).expect("a program").name, "app");
}

#[test]
fn a_library_is_not_mistaken_for_a_program() {
    let scratch = Scratch::new("library");
    root(scratch.path(), &["lib"], None);
    member(scratch.path(), "lib", "1.0.0", "", false);

    let workspace = Workspace::find(scratch.path()).expect("find").expect("a workspace");
    // `lib`'s entry module exists -- it is the library's own source -- but it
    // declares no `main`, so it is not something to run.
    let error = workspace.program(None).expect_err("must refuse");
    assert!(error.contains("no workspace member declares a program"), "{error}");
}

#[test]
fn more_than_one_program_has_to_be_chosen_between() {
    let scratch = Scratch::new("two-programs");
    root(scratch.path(), &["one", "two"], None);
    member(scratch.path(), "one", "0.1.0", "", true);
    member(scratch.path(), "two", "0.1.0", "", true);

    let workspace = Workspace::find(scratch.path()).expect("find").expect("a workspace");
    let error = workspace.program(None).expect_err("must refuse to guess");
    assert!(error.contains("more than one program"), "{error}");
    assert!(error.contains("--package"), "{error}");

    assert_eq!(workspace.program(Some("two")).expect("named").name, "two");
    let error = workspace.program(Some("three")).expect_err("unknown member");
    assert!(error.contains("no workspace member is called `three`"), "{error}");
}

// ---------------------------------------------------------------- visibility

#[test]
fn sharing_a_workspace_does_not_grant_access() {
    let scratch = Scratch::new("visibility");
    root(scratch.path(), &["lib", "app"], None);
    member(scratch.path(), "lib", "1.0.0", "", false);
    let app = member(scratch.path(), "app", "0.1.0", "", true);
    // `app` imports `lib` without declaring it as a dependency.
    std::fs::write(
        app.join("src/app.kb"),
        "(module app\n  (use lib :as lib))\n\n;;; Entry.\n(pub fn main [] -> Unit !io (println (lib.tag)))\n",
    )
    .expect("write");

    let mut session = Session::open(&app).expect("open");
    let _ = session.load_module("app", korben_syntax::Span::synthetic());
    korben_core::infer::check_session(&mut session, false);
    let codes: Vec<String> = session
        .diagnostics
        .items
        .iter()
        .filter(|item| item.is_error())
        .filter_map(|item| item.code.clone())
        .collect();
    assert!(
        codes.iter().any(|code| code == "undeclared-dependency"),
        "an undeclared sibling import was allowed: {codes:?}"
    );
}
