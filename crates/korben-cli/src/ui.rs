//! Terminal presentation helpers shared by every command.

// korben-6bc

use korben_syntax::diag::Diagnostics;
use korben_syntax::SourceMap;

/// Whether to emit ANSI colors: off when stdout is redirected or `NO_COLOR` is set.
pub fn use_color() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var("TERM").map(|term| term == "dumb").unwrap_or(false) {
        return false;
    }
    std::io::IsTerminal::is_terminal(&std::io::stderr())
}

pub fn bold(text: &str) -> String {
    if use_color() {
        format!("\u{1b}[1m{text}\u{1b}[0m")
    } else {
        text.to_string()
    }
}

pub fn green(text: &str) -> String {
    if use_color() {
        format!("\u{1b}[32m{text}\u{1b}[0m")
    } else {
        text.to_string()
    }
}

pub fn red(text: &str) -> String {
    if use_color() {
        format!("\u{1b}[31m{text}\u{1b}[0m")
    } else {
        text.to_string()
    }
}

pub fn yellow(text: &str) -> String {
    if use_color() {
        format!("\u{1b}[33m{text}\u{1b}[0m")
    } else {
        text.to_string()
    }
}

pub fn dim(text: &str) -> String {
    if use_color() {
        format!("\u{1b}[2m{text}\u{1b}[0m")
    } else {
        text.to_string()
    }
}

/// Print diagnostics and return true when any of them is an error.
pub fn report(diagnostics: &Diagnostics, sources: &SourceMap, json: bool) -> bool {
    if json {
        println!("{}", diagnostics.to_json(sources));
        return diagnostics.has_errors();
    }
    for diagnostic in &diagnostics.items {
        eprint!("{}", diagnostic.render(sources, use_color()));
        eprintln!();
    }
    diagnostics.has_errors()
}

/// `3 errors` / `1 error, 2 warnings`
pub fn summarize(diagnostics: &Diagnostics) -> String {
    let errors = diagnostics.error_count();
    let warnings = diagnostics.warning_count();
    let mut parts = Vec::new();
    if errors > 0 {
        parts.push(format!("{errors} error{}", plural(errors)));
    }
    if warnings > 0 {
        parts.push(format!("{warnings} warning{}", plural(warnings)));
    }
    if parts.is_empty() {
        "no problems".to_string()
    } else {
        parts.join(", ")
    }
}

pub fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}
