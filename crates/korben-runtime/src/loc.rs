//! Source locations and the fault type.
//!
//! The runtime is shared by the interpreter and by generated native code, so it
//! cannot depend on the compiler front end. It carries the same byte spans the
//! front end produces and resolves them against a source table the program
//! installs at start-up, which is what lets a native binary report a runtime
//! fault against the Korben source the user wrote.

// korben-vtx

use std::cell::RefCell;
use std::fmt::Write as _;

/// A byte range in a source file, matching the front end's span layout.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Loc {
    pub file: u32,
    pub start: u32,
    pub end: u32,
}

impl Loc {
    pub const NONE: Loc = Loc { file: u32::MAX, start: 0, end: 0 };

    pub fn new(file: u32, start: u32, end: u32) -> Loc {
        Loc { file, start, end }
    }

    pub fn is_none(&self) -> bool {
        self.file == u32::MAX
    }
}

thread_local! {
    /// Name and text of every source file, indexed by file id.
    static SOURCES: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// Install the source table. Generated programs call this once from `main`.
pub fn install_sources(files: &[(&str, &str)]) {
    SOURCES.with(|sources| {
        *sources.borrow_mut() =
            files.iter().map(|(name, text)| (name.to_string(), text.to_string())).collect();
    });
}

/// `path:line:column` for a location, or `<unknown>` when it cannot be resolved.
pub fn describe(loc: Loc) -> String {
    SOURCES.with(|sources| {
        let sources = sources.borrow();
        let Some((name, text)) = sources.get(loc.file as usize) else {
            return "<unknown>".to_string();
        };
        let (line, column) = line_col(text, loc.start);
        format!("{name}:{line}:{column}")
    })
}

/// The text a location covers, for quoting in a fault report.
pub fn snippet_line(loc: Loc) -> Option<(String, usize, usize, usize)> {
    SOURCES.with(|sources| {
        let sources = sources.borrow();
        let (_, text) = sources.get(loc.file as usize)?;
        let (line, column) = line_col(text, loc.start);
        let start = text[..(loc.start as usize).min(text.len())]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        let end = text[start..].find('\n').map(|index| start + index).unwrap_or(text.len());
        let width = (loc.end.saturating_sub(loc.start) as usize).max(1);
        Some((text[start..end].to_string(), line, column, width))
    })
}

fn line_col(text: &str, offset: u32) -> (usize, usize) {
    let clamped = (offset as usize).min(text.len());
    let line = text[..clamped].bytes().filter(|byte| *byte == b'\n').count() + 1;
    let start = text[..clamped].rfind('\n').map(|index| index + 1).unwrap_or(0);
    let column = text[start..clamped].chars().count() + 1;
    (line, column)
}

/// An unrecoverable runtime error, reported the way the compiler reports one.
#[derive(Clone, Debug)]
pub struct Fault {
    pub code: String,
    pub message: String,
    pub loc: Loc,
    pub label: String,
    pub notes: Vec<String>,
    pub help: Vec<String>,
}

impl Fault {
    /// Start a fault with no location yet; `with_code` and `at` fill it in.
    pub fn error(message: impl Into<String>) -> Fault {
        Fault {
            code: "runtime".to_string(),
            message: message.into(),
            loc: Loc::NONE,
            label: String::new(),
            notes: Vec::new(),
            help: Vec::new(),
        }
    }

    pub fn with_code(mut self, code: &str) -> Fault {
        self.code = code.to_string();
        self
    }

    /// Attach the location this fault happened at, with an inline label.
    pub fn at(mut self, loc: Loc, label: impl Into<String>) -> Fault {
        self.loc = loc;
        self.label = label.into();
        self
    }

    pub fn new(code: &str, message: impl Into<String>, loc: Loc) -> Fault {
        Fault {
            code: code.to_string(),
            message: message.into(),
            loc,
            label: String::new(),
            notes: Vec::new(),
            help: Vec::new(),
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Fault {
        self.label = label.into();
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Fault {
        self.notes.push(note.into());
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Fault {
        self.help.push(help.into());
        self
    }

    /// Render the fault with its source line, as the compiler would.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "error[{}]: {}", self.code, self.message);
        if let Some((text, line, column, width)) = snippet_line(self.loc) {
            let gutter = line.to_string().len().max(2);
            let pad = " ".repeat(gutter);
            let _ = writeln!(out, "{pad}--> {}", describe(self.loc));
            let _ = writeln!(out, "{pad} |");
            let _ = writeln!(out, "{line:>gutter$} | {text}");
            let lead = " ".repeat(column.saturating_sub(1));
            let carets = "^".repeat(width);
            if self.label.is_empty() {
                let _ = writeln!(out, "{pad} | {lead}{carets}");
            } else {
                let _ = writeln!(out, "{pad} | {lead}{carets} {}", self.label);
            }
            let _ = writeln!(out, "{pad} |");
        } else if !self.loc.is_none() {
            let _ = writeln!(out, "  --> {}", describe(self.loc));
        }
        for note in &self.notes {
            let _ = writeln!(out, "  note: {note}");
        }
        for help in &self.help {
            let _ = writeln!(out, "  help: {help}");
        }
        out
    }
}
