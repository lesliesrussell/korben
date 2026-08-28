//! Several packages in one repository, resolved and locked together.
//!
//! A workspace exists so that members cannot disagree. One resolution pass
//! covers every member's dependencies, and one `korben.lock` at the root pins
//! the result, so two members that share a dependency share the version of it
//! -- which is the whole reason to put them in one repository rather than two.

// korben-mic

use std::path::{Path, PathBuf};

use crate::manifest::{Dependency, Manifest};
use crate::project::MANIFEST_NAME;

/// One package in a workspace.
#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub root: PathBuf,
    pub manifest: Manifest,
}

impl Member {
    /// Whether this member is a program rather than a library.
    ///
    /// The manifest's `main` is the name of the entry *module*, and a library's
    /// entry module is simply its own source, so the file existing proves
    /// nothing. What settles it is whether that module declares a `main`
    /// function, which is what `korben run` would go looking for.
    pub fn has_program(&self) -> bool {
        let main = self.manifest.main.replace('.', "/");
        let src = self.root.join("src");
        let candidates = [src.join(format!("{main}.kb")), src.join(&main).join("mod.kb")];
        let Some(path) = candidates.into_iter().find(|path| path.is_file()) else {
            return false;
        };
        let Ok(text) = std::fs::read_to_string(&path) else { return false };
        declares_main(&text)
    }
}

/// Whether a source declares a top-level `main` function.
fn declares_main(text: &str) -> bool {
    let (forms, _) = korben_syntax::read_all(u32::MAX, text, korben_syntax::Comments::Skip);
    forms.iter().any(|form| {
        let Some(items) = form.as_list() else { return false };
        // `(fn main ...)` and `(pub fn main ...)` are one flat form apiece.
        let mut parts = items.iter().filter_map(korben_syntax::Syntax::as_symbol);
        let first = parts.next();
        let rest: Vec<&str> = parts.collect();
        match first {
            Some("fn") => rest.first() == Some(&"main"),
            Some("pub") => rest.first() == Some(&"fn") && rest.get(1) == Some(&"main"),
            _ => false,
        }
    })
}

/// A workspace root and the packages it gathers.
#[derive(Clone, Debug)]
pub struct Workspace {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub members: Vec<Member>,
}

impl Workspace {
    /// The workspace containing `start`, if there is one.
    ///
    /// Walking up stops at the first manifest that declares `[workspace]` and
    /// actually lists the package below it. A manifest that declares members it
    /// does not include is a workspace for those members and not for this one,
    /// which keeps an unrelated parent directory from capturing a project.
    ///
    /// A workspace root that cannot be loaded is an error rather than a `None`.
    /// Treating a broken root as "no workspace here" would silently check one
    /// package and report success for a repository that does not build.
    pub fn find(start: &Path) -> Result<Option<Workspace>, String> {
        let Some(nearest) = crate::project::find_manifest(start) else { return Ok(None) };
        let Some(nearest_root) = nearest.parent().map(Path::to_path_buf) else {
            return Ok(None);
        };

        // The nearest manifest may itself be the root.
        let manifest = Manifest::load(&nearest)?;
        if manifest.is_workspace() {
            return Workspace::open(&nearest_root, manifest).map(Some);
        }

        let Some(mut current) = nearest_root.parent().map(Path::to_path_buf) else {
            return Ok(None);
        };
        loop {
            let candidate = current.join(MANIFEST_NAME);
            if candidate.is_file() {
                // A manifest above this one that fails to parse is not this
                // package's problem, so keep walking rather than failing here.
                if let Ok(manifest) = Manifest::load(&candidate) {
                    if manifest.is_workspace() {
                        let workspace = Workspace::open(&current, manifest)?;
                        if workspace.member_at(&nearest_root).is_some() {
                            return Ok(Some(workspace));
                        }
                    }
                }
            }
            if !current.pop() {
                return Ok(None);
            }
        }
    }

    /// Load the members a root manifest lists.
    pub fn open(root: &Path, manifest: Manifest) -> Result<Workspace, String> {
        let mut members = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for relative in &manifest.members {
            let directory = root.join(relative);
            let path = directory.join(MANIFEST_NAME);
            if !path.is_file() {
                return Err(format!(
                    "workspace member `{relative}` has no {MANIFEST_NAME}\n  looked in {}",
                    directory.display()
                ));
            }
            let member = Manifest::load(&path)
                .map_err(|error| format!("workspace member `{relative}`: {error}"))?;
            if member.is_virtual {
                return Err(format!(
                    "workspace member `{relative}` has no `[package] name`\n  a member is a package; only the root may be a bare workspace"
                ));
            }
            if seen.contains(&member.name) {
                return Err(format!(
                    "two workspace members are both called `{}`\n  every member needs its own name, since dependencies are resolved by name",
                    member.name
                ));
            }
            seen.push(member.name.clone());
            members.push(Member { name: member.name.clone(), root: directory, manifest: member });
        }
        // A root that is also a package is a member of its own workspace, so a
        // command run there builds the thing the root declares.
        if !manifest.is_virtual {
            members.insert(
                0,
                Member {
                    name: manifest.name.clone(),
                    root: root.to_path_buf(),
                    manifest: manifest.clone(),
                },
            );
        }
        Ok(Workspace { root: root.to_path_buf(), manifest, members })
    }

    pub fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|member| member.name == name)
    }

    /// The member rooted at a directory.
    pub fn member_at(&self, root: &Path) -> Option<&Member> {
        self.members.iter().find(|member| member.root == root)
    }

    /// Every member's dependencies, as resolution's starting demands.
    pub fn demands(&self) -> Vec<(String, PathBuf, Vec<Dependency>)> {
        self.members
            .iter()
            .map(|member| {
                let dependencies = member
                    .manifest
                    .dependencies
                    .iter()
                    .chain(&member.manifest.dev_dependencies)
                    .cloned()
                    .collect();
                (member.name.clone(), member.root.clone(), dependencies)
            })
            .collect()
    }

    /// The member a `run` or `build` should act on.
    ///
    /// A workspace has no single program by default, so this refuses to guess:
    /// either the caller named one, or exactly one member declares an entry
    /// point, or the answer is an error that lists the choices.
    pub fn program(&self, requested: Option<&str>) -> Result<&Member, String> {
        if let Some(name) = requested {
            return self.member(name).ok_or_else(|| {
                format!("no workspace member is called `{name}`\n  members: {}", self.names())
            });
        }
        let with_main: Vec<&Member> =
            self.members.iter().filter(|member| member.has_program()).collect();
        match with_main.as_slice() {
            [only] => Ok(only),
            [] => Err(format!(
                "no workspace member declares a program\n  members: {}\n  pass `--package <name>` to choose one",
                self.names()
            )),
            _ => Err(format!(
                "this workspace has more than one program\n  members: {}\n  pass `--package <name>` to choose one",
                with_main.iter().map(|member| member.name.as_str()).collect::<Vec<_>>().join(", ")
            )),
        }
    }

    fn names(&self) -> String {
        self.members.iter().map(|member| member.name.as_str()).collect::<Vec<_>>().join(", ")
    }
}
