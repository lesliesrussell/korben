//! Build artifacts.
//!
//! `korben build` emits a `.kbx` bundle: every module the program needs,
//! concatenated with their module headers and a manifest stanza. It is a
//! reproducible, single-file artifact that the Korben runtime executes.
//!
//! Native code generation — lowering typed IR through Rust or Cranelift — is
//! Milestone C. Keeping the artifact format explicit now means the build
//! command has real, inspectable output rather than a placeholder.

// korben-6bc

use crate::project::Session;

pub const BUNDLE_HEADER: &str = ";; korben bundle v1";
pub const BUNDLE_EXTENSION: &str = "kbx";
const MODULE_MARKER: &str = ";; --- module ";

/// True when this text is a build artifact rather than a source file.
pub fn is_bundle(text: &str) -> bool {
    text.starts_with(BUNDLE_HEADER)
}

/// Split a bundle back into its modules, in the order they were written.
pub fn read_bundle(text: &str) -> Vec<(String, String)> {
    let mut modules: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(MODULE_MARKER) {
            if let Some(module) = current.take() {
                modules.push(module);
            }
            let name = rest.trim_end_matches(" ---").trim().to_string();
            current = Some((name, String::new()));
            continue;
        }
        if let Some((_, source)) = current.as_mut() {
            source.push_str(line);
            source.push('\n');
        }
    }
    if let Some(module) = current {
        modules.push(module);
    }
    modules
}

/// The entry module recorded in a bundle header.
pub fn bundle_entry(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix(";; entry: "))
        .map(|entry| entry.trim().to_string())
}

pub fn write_bundle(session: &Session) -> String {
    let mut out = String::new();
    out.push_str(BUNDLE_HEADER);
    out.push('\n');
    out.push_str(&format!(";; package: {}\n", session.manifest.name));
    out.push_str(&format!(";; version: {}\n", session.manifest.version));
    out.push_str(&format!(";; edition: {}\n", session.manifest.edition));
    out.push_str(&format!(";; entry: {}\n", session.manifest.main));
    out.push_str(&format!(";; modules: {}\n\n", session.modules.len()));

    for module in &session.modules {
        let source = session.sources.source(module.file);
        if source.is_empty() {
            continue;
        }
        out.push_str(&format!("{MODULE_MARKER}{} ---\n", module.name));
        out.push_str(source.trim_end());
        out.push_str("\n\n");
    }
    out
}
