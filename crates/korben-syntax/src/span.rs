//! Source files, byte spans, and line/column mapping.
//!
//! Every syntax object carries a [`Span`]. Spans survive macro expansion so that
//! diagnostics, the formatter, and the language server can always point back at
//! the text the user actually wrote.

// korben-6bc

use std::path::{Path, PathBuf};

/// Index of a source file inside a [`SourceMap`].
pub type FileId = u32;

/// A half-open byte range `[start, end)` inside a single source file.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(file: FileId, start: u32, end: u32) -> Span {
        Span { file, start, end }
    }

    /// A zero-width span, used for compiler-synthesized syntax.
    pub fn synthetic() -> Span {
        Span { file: u32::MAX, start: 0, end: 0 }
    }

    pub fn is_synthetic(&self) -> bool {
        self.file == u32::MAX
    }

    /// The smallest span covering both inputs. Synthetic spans are absorbed.
    pub fn to(self, other: Span) -> Span {
        if self.is_synthetic() {
            return other;
        }
        if other.is_synthetic() || other.file != self.file {
            return self;
        }
        Span { file: self.file, start: self.start.min(other.start), end: self.end.max(other.end) }
    }

    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// One loaded source file: its display name, optional path on disk, text, and a
/// precomputed table of line start offsets.
pub struct SourceFile {
    pub name: String,
    pub path: Option<PathBuf>,
    pub text: String,
    line_starts: Vec<u32>,
}

impl SourceFile {
    fn new(name: String, path: Option<PathBuf>, text: String) -> SourceFile {
        let mut line_starts = vec![0u32];
        for (offset, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(offset as u32 + 1);
            }
        }
        SourceFile { name, path, text, line_starts }
    }

    /// Zero-based line index containing `offset`.
    pub fn line_index(&self, offset: u32) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(index) => index,
            Err(index) => index - 1,
        }
    }

    pub fn line_start(&self, line: usize) -> u32 {
        self.line_starts.get(line).copied().unwrap_or(self.text.len() as u32)
    }

    pub fn line_text(&self, line: usize) -> &str {
        let start = self.line_start(line) as usize;
        let end =
            self.line_starts.get(line + 1).map(|value| *value as usize).unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches(['\n', '\r'])
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// One-based line and column (counted in characters, not bytes).
    pub fn line_col(&self, offset: u32) -> (usize, usize) {
        let line = self.line_index(offset);
        let start = self.line_start(line) as usize;
        let clamped = (offset as usize).min(self.text.len());
        let column = self.text[start..clamped].chars().count() + 1;
        (line + 1, column)
    }
}

/// The collection of every source file the compiler has read this session.
#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> SourceMap {
        SourceMap { files: Vec::new() }
    }

    pub fn add(&mut self, name: impl Into<String>, text: impl Into<String>) -> FileId {
        let id = self.files.len() as FileId;
        self.files.push(SourceFile::new(name.into(), None, text.into()));
        id
    }

    pub fn add_file(&mut self, path: &Path, text: impl Into<String>) -> FileId {
        let id = self.files.len() as FileId;
        self.files.push(SourceFile::new(
            path.display().to_string(),
            Some(path.to_path_buf()),
            text.into(),
        ));
        id
    }

    pub fn get(&self, file: FileId) -> Option<&SourceFile> {
        if file == u32::MAX {
            return None;
        }
        self.files.get(file as usize)
    }

    /// Look up an already-loaded file by its path on disk.
    pub fn find_by_path(&self, path: &Path) -> Option<FileId> {
        self.files
            .iter()
            .position(|file| file.path.as_deref() == Some(path))
            .map(|index| index as FileId)
    }

    pub fn source(&self, file: FileId) -> &str {
        self.get(file).map(|file| file.text.as_str()).unwrap_or("")
    }

    pub fn name(&self, file: FileId) -> &str {
        self.get(file).map(|file| file.name.as_str()).unwrap_or("<synthetic>")
    }

    /// The text covered by `span`, or `""` for synthetic spans.
    pub fn snippet(&self, span: Span) -> &str {
        match self.get(span.file) {
            Some(file) => {
                let start = (span.start as usize).min(file.text.len());
                let end = (span.end as usize).min(file.text.len()).max(start);
                &file.text[start..end]
            }
            None => "",
        }
    }

    /// `path:line:col` for a span, suitable for terminal hyperlinking.
    pub fn location(&self, span: Span) -> String {
        match self.get(span.file) {
            Some(file) => {
                let (line, column) = file.line_col(span.start);
                format!("{}:{}:{}", file.name, line, column)
            }
            None => "<synthetic>".to_string(),
        }
    }
}
