//! Diagnostics: the human-readable and machine-readable error surface.
//!
//! Per the specification every compiler error carries a concise explanation, a
//! primary span, optional secondary spans, and confidence-safe suggestions.
//! Rendering is available as annotated terminal text or as stable JSON.

// korben-6bc

use crate::span::{SourceMap, Span};
use std::fmt::Write as _;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }

    fn color(self) -> &'static str {
        match self {
            Severity::Error => "\u{1b}[1;31m",
            Severity::Warning => "\u{1b}[1;33m",
            Severity::Note => "\u{1b}[1;36m",
        }
    }
}

/// A span with an attached explanation.
#[derive(Clone, Debug)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

impl Label {
    pub fn new(span: Span, message: impl Into<String>) -> Label {
        Label { span, message: message.into() }
    }
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable machine-readable code such as `type-mismatch`.
    pub code: Option<String>,
    pub message: String,
    pub primary: Option<Label>,
    pub secondary: Vec<Label>,
    pub notes: Vec<String>,
    pub help: Vec<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            severity,
            code: None,
            message: message.into(),
            primary: None,
            secondary: Vec::new(),
            notes: Vec::new(),
            help: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Error, message)
    }

    pub fn warning(message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Warning, message)
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Diagnostic {
        self.code = Some(code.into());
        self
    }

    pub fn at(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.primary = Some(Label::new(span, message));
        self
    }

    /// Attach a primary span with no inline label.
    pub fn span(mut self, span: Span) -> Diagnostic {
        self.primary = Some(Label::new(span, ""));
        self
    }

    pub fn secondary(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.secondary.push(Label::new(span, message));
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Diagnostic {
        self.notes.push(note.into());
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Diagnostic {
        self.help.push(help.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Render as annotated source text.
    pub fn render(&self, sources: &SourceMap, color: bool) -> String {
        let (bold, dim, reset, accent) = if color {
            ("\u{1b}[1m", "\u{1b}[2m", "\u{1b}[0m", self.severity.color())
        } else {
            ("", "", "", "")
        };

        let mut out = String::new();
        let code = match &self.code {
            Some(code) => format!("[{code}]"),
            None => String::new(),
        };
        let _ = writeln!(
            out,
            "{accent}{}{code}{reset}{bold}: {}{reset}",
            self.severity.label(),
            self.message
        );

        let mut labels: Vec<(&Label, bool)> = Vec::new();
        if let Some(primary) = &self.primary {
            labels.push((primary, true));
        }
        for label in &self.secondary {
            labels.push((label, false));
        }

        for (label, is_primary) in labels {
            let Some(file) = sources.get(label.span.file) else {
                continue;
            };
            let (line, column) = file.line_col(label.span.start);
            let gutter_width = line.to_string().len().max(2);
            let pad = " ".repeat(gutter_width);
            let arrow = if is_primary { "-->" } else { "..." };
            let _ = writeln!(out, "{pad}{dim}{arrow}{reset} {}:{}:{}", file.name, line, column);
            let _ = writeln!(out, "{pad} {dim}|{reset}");

            let text = file.line_text(line - 1);
            let _ = writeln!(out, "{dim}{line:>gutter_width$} |{reset} {text}");

            // Underline the span, clamped to the first line it touches.
            let line_start = file.line_start(line - 1);
            let prefix_bytes = (label.span.start.saturating_sub(line_start)) as usize;
            let prefix = &text[..prefix_bytes.min(text.len())];
            let lead = display_width(prefix);
            let span_bytes = label.span.len().max(1) as usize;
            let visible = &text[prefix_bytes.min(text.len())
                ..(prefix_bytes + span_bytes).min(text.len()).max(prefix_bytes.min(text.len()))];
            let width = display_width(visible).max(1);
            let caret = if is_primary { "^" } else { "-" };
            let marker_color = if is_primary { accent } else { dim };
            let _ = write!(
                out,
                "{pad} {dim}|{reset} {}{marker_color}{}{reset}",
                " ".repeat(lead),
                caret.repeat(width)
            );
            if label.message.is_empty() {
                out.push('\n');
            } else {
                let _ = writeln!(out, " {marker_color}{}{reset}", label.message);
            }
            let _ = writeln!(out, "{pad} {dim}|{reset}");
        }

        for note in &self.notes {
            let _ = writeln!(out, "  {dim}note:{reset} {note}");
        }
        for help in &self.help {
            let _ = writeln!(out, "  {bold}help:{reset} {help}");
        }
        out
    }

    /// Stable machine-readable form for editors and CI.
    pub fn to_json(&self, sources: &SourceMap) -> String {
        let mut out = String::from("{");
        let _ = write!(out, "\"severity\":\"{}\"", self.severity.label());
        if let Some(code) = &self.code {
            let _ = write!(out, ",\"code\":{}", json_string(code));
        }
        let _ = write!(out, ",\"message\":{}", json_string(&self.message));
        if let Some(primary) = &self.primary {
            let _ = write!(out, ",\"primary\":{}", label_json(primary, sources));
        }
        let _ = write!(out, ",\"secondary\":[");
        for (index, label) in self.secondary.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&label_json(label, sources));
        }
        out.push(']');
        let _ = write!(out, ",\"notes\":[");
        for (index, note) in self.notes.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&json_string(note));
        }
        out.push(']');
        let _ = write!(out, ",\"help\":[");
        for (index, help) in self.help.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&json_string(help));
        }
        out.push(']');
        out.push('}');
        out
    }
}

fn label_json(label: &Label, sources: &SourceMap) -> String {
    let (file, line, column) = match sources.get(label.span.file) {
        Some(file) => {
            let (line, column) = file.line_col(label.span.start);
            (file.name.clone(), line, column)
        }
        None => ("<synthetic>".to_string(), 0, 0),
    };
    format!(
        "{{\"file\":{},\"line\":{},\"column\":{},\"start\":{},\"end\":{},\"message\":{}}}",
        json_string(&file),
        line,
        column,
        label.span.start,
        label.span.end,
        json_string(&label.message)
    )
}

/// Escape a Rust string as a JSON string literal.
pub fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Terminal columns a string occupies, treating tabs as advancing to 4 columns.
fn display_width(text: &str) -> usize {
    text.chars().map(|ch| if ch == '\t' { 4 } else { 1 }).sum()
}

/// A collection of diagnostics produced by one compilation.
#[derive(Default, Debug, Clone)]
pub struct Diagnostics {
    pub items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Diagnostics {
        Diagnostics { items: Vec::new() }
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.items.push(diagnostic);
    }

    pub fn extend(&mut self, other: Diagnostics) {
        self.items.extend(other.items);
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(Diagnostic::is_error)
    }

    pub fn error_count(&self) -> usize {
        self.items.iter().filter(|item| item.is_error()).count()
    }

    pub fn warning_count(&self) -> usize {
        self.items.iter().filter(|item| item.severity == Severity::Warning).count()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn render(&self, sources: &SourceMap, color: bool) -> String {
        self.items.iter().map(|item| item.render(sources, color)).collect::<Vec<_>>().join("\n")
    }

    pub fn to_json(&self, sources: &SourceMap) -> String {
        let body =
            self.items.iter().map(|item| item.to_json(sources)).collect::<Vec<_>>().join(",");
        format!("{{\"diagnostics\":[{body}]}}")
    }
}
