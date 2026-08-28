//! The request loop: documents in, answers out.

// korben-efd

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use korben_syntax::Severity;

use crate::analysis::{Analysis, Range};
use crate::json::Json;
use crate::position::{to_offset, Position};
use crate::rpc::{self, Message};

/// What the server knows between messages.
pub struct Server {
    root: PathBuf,
    /// Open buffers, which take precedence over the same file on disk.
    documents: BTreeMap<PathBuf, String>,
    /// Files this server has published diagnostics for, so a file that becomes
    /// clean is cleared rather than left showing its last error.
    published: BTreeMap<PathBuf, usize>,
    shutdown: bool,
}

impl Server {
    pub fn new(root: PathBuf) -> Server {
        Server { root, documents: BTreeMap::new(), published: BTreeMap::new(), shutdown: false }
    }

    /// Serve until the client closes the connection or says `exit`.
    pub fn serve(
        &mut self,
        input: &mut impl BufRead,
        output: &mut impl Write,
    ) -> Result<(), String> {
        while let Some(message) = rpc::read(input)? {
            if message.method == "exit" {
                return Ok(());
            }
            self.handle(message, output)?;
        }
        Ok(())
    }

    fn handle(&mut self, message: Message, output: &mut impl Write) -> Result<(), String> {
        match message.method.as_str() {
            "initialize" => {
                if let Some(root) = root_of(&message.params) {
                    self.root = root;
                }
                let id = message.id.clone().unwrap_or(Json::Null);
                rpc::respond(output, id, capabilities())
            }
            "initialized" | "$/setTrace" | "workspace/didChangeConfiguration" => Ok(()),
            "shutdown" => {
                self.shutdown = true;
                let id = message.id.clone().unwrap_or(Json::Null);
                rpc::respond(output, id, Json::Null)
            }
            "textDocument/didOpen" => {
                if let (Some(path), Some(text)) = (
                    message.params.path(&["textDocument", "uri"]).and_then(path_of),
                    message.params.path(&["textDocument", "text"]).and_then(Json::as_str),
                ) {
                    self.documents.insert(path, text.to_string());
                    self.publish(output)?;
                }
                Ok(())
            }
            "textDocument/didChange" => {
                if let Some(path) = message.params.path(&["textDocument", "uri"]).and_then(path_of)
                {
                    // Full sync: the last change carries the whole document,
                    // which is what `capabilities` asks the client to send.
                    if let Some(text) = message
                        .params
                        .get("contentChanges")
                        .and_then(Json::as_array)
                        .and_then(|changes| changes.last())
                        .and_then(|change| change.get("text"))
                        .and_then(Json::as_str)
                    {
                        self.documents.insert(path, text.to_string());
                        self.publish(output)?;
                    }
                }
                Ok(())
            }
            "textDocument/didSave" => self.publish(output),
            "textDocument/didClose" => {
                if let Some(path) = message.params.path(&["textDocument", "uri"]).and_then(path_of)
                {
                    self.documents.remove(&path);
                    self.publish(output)?;
                }
                Ok(())
            }
            "textDocument/hover" => self.answer(message, output, Server::hover),
            "textDocument/definition" => self.answer(message, output, Server::definition),
            "textDocument/completion" => self.answer(message, output, Server::completion),
            "textDocument/documentSymbol" => self.answer(message, output, Server::symbols),
            "textDocument/formatting" => self.answer(message, output, Server::formatting),
            // A notification the server does not implement is ignored; a
            // request needs an answer, even a refusal, or the client waits.
            _ => match message.id {
                Some(id) if !message.method.is_empty() => rpc::respond_error(
                    output,
                    id,
                    rpc::METHOD_NOT_FOUND,
                    &format!("`{}` is not implemented", message.method),
                ),
                _ => Ok(()),
            },
        }
    }

    /// Run one query and answer it, or answer `null` when it has no result.
    fn answer(
        &mut self,
        message: Message,
        output: &mut impl Write,
        query: fn(&Server, &Analysis, &Path, usize, &Json) -> Json,
    ) -> Result<(), String> {
        let id = message.id.clone().unwrap_or(Json::Null);
        if !message.is_request() {
            return Ok(());
        }
        let Some(path) = message.params.path(&["textDocument", "uri"]).and_then(path_of) else {
            return rpc::respond(output, id, Json::Null);
        };
        let analysis = self.analyze();
        let offset = match position_of(&message.params) {
            Some(position) => {
                let text = self
                    .documents
                    .get(&path)
                    .map(String::as_str)
                    .or_else(|| analysis.file_of(&path).map(|file| analysis.text_of(file)))
                    .unwrap_or("");
                to_offset(text, position)
            }
            None => 0,
        };
        let result = query(self, &analysis, &path, offset, &message.params);
        rpc::respond(output, id, result)
    }

    fn analyze(&self) -> Analysis {
        Analysis::build(&self.root, &self.documents)
    }

    fn hover(&self, analysis: &Analysis, path: &Path, offset: usize, _params: &Json) -> Json {
        let Some(file) = analysis.file_of(path) else { return Json::Null };
        let Some((markdown, range)) = analysis.hover(file, offset) else { return Json::Null };
        Json::object([
            (
                "contents",
                Json::object([
                    ("kind", Json::string("markdown")),
                    ("value", Json::string(markdown)),
                ]),
            ),
            ("range", range_json(range)),
        ])
    }

    fn definition(&self, analysis: &Analysis, path: &Path, offset: usize, _params: &Json) -> Json {
        let Some(file) = analysis.file_of(path) else { return Json::Null };
        let Some(location) = analysis.definition(file, offset) else { return Json::Null };
        Json::object([
            ("uri", Json::string(uri_of(&location.path))),
            ("range", range_json(location.range)),
        ])
    }

    fn completion(&self, analysis: &Analysis, path: &Path, offset: usize, _params: &Json) -> Json {
        let Some(file) = analysis.file_of(path) else { return Json::Null };
        let items = analysis.completions(file, offset).into_iter().map(|item| {
            let mut fields =
                vec![("label", Json::string(item.label)), ("kind", Json::Int(item.kind))];
            if let Some(detail) = item.detail {
                fields.push(("detail", Json::string(detail)));
            }
            Json::object(fields)
        });
        Json::object([("isIncomplete", Json::Bool(false)), ("items", Json::array(items))])
    }

    fn symbols(&self, analysis: &Analysis, path: &Path, _offset: usize, _params: &Json) -> Json {
        let Some(file) = analysis.file_of(path) else { return Json::array([]) };
        Json::array(analysis.symbols(file).into_iter().map(|symbol| {
            Json::object([
                ("name", Json::string(symbol.name)),
                ("kind", Json::Int(symbol.kind)),
                ("detail", Json::string(symbol.detail)),
                ("range", range_json(symbol.range)),
                ("selectionRange", range_json(symbol.range)),
            ])
        }))
    }

    fn formatting(&self, analysis: &Analysis, path: &Path, _offset: usize, _params: &Json) -> Json {
        let Some(file) = analysis.file_of(path) else { return Json::array([]) };
        let text =
            self.documents.get(path).cloned().unwrap_or_else(|| analysis.text_of(file).to_string());
        let (formatted, errors) = korben_syntax::fmt::format_source(file, &text);
        // A file that does not parse cannot be formatted, and rewriting it on a
        // guess would destroy work in progress.
        if errors.iter().any(|error| error.is_error()) {
            return Json::array([]);
        }
        if formatted == text {
            return Json::array([]);
        }
        // One edit replacing the whole document: the formatter is defined on
        // whole files, so a narrower diff would be an invention.
        let end = crate::position::to_position(&text, text.len());
        Json::array([Json::object([
            ("range", range_json(Range { start: Position::default(), end })),
            ("newText", Json::string(formatted)),
        ])])
    }

    /// Publish diagnostics for every file that has any, and clear the rest.
    fn publish(&mut self, output: &mut impl Write) -> Result<(), String> {
        let analysis = self.analyze();
        let grouped = analysis.diagnostics();
        let mut counts: BTreeMap<PathBuf, usize> = BTreeMap::new();
        for (file, items) in &grouped {
            let Some(path) = analysis.session.sources.get(*file).and_then(|f| f.path.clone())
            else {
                continue;
            };
            counts.insert(path.clone(), items.len());
            let rendered = items.iter().map(|item| diagnostic_json(&analysis, item));
            rpc::notify(
                output,
                "textDocument/publishDiagnostics",
                Json::object([
                    ("uri", Json::string(uri_of(&path))),
                    ("diagnostics", Json::array(rendered)),
                ]),
            )?;
        }
        // Anything that had diagnostics last time and has none now must be told
        // so, or the editor keeps showing errors the code no longer has.
        let stale: Vec<PathBuf> =
            self.published.keys().filter(|path| !counts.contains_key(*path)).cloned().collect();
        for path in stale {
            rpc::notify(
                output,
                "textDocument/publishDiagnostics",
                Json::object([
                    ("uri", Json::string(uri_of(&path))),
                    ("diagnostics", Json::array([])),
                ]),
            )?;
        }
        self.published = counts;
        Ok(())
    }
}

fn diagnostic_json(analysis: &Analysis, item: &korben_syntax::Diagnostic) -> Json {
    let range = item
        .primary
        .as_ref()
        .map(|label| analysis.range_of(label.span))
        .unwrap_or(Range { start: Position::default(), end: Position::default() });
    let mut message = item.message.clone();
    for note in &item.notes {
        message.push('\n');
        message.push_str(note);
    }
    for help in &item.help {
        message.push_str("\nhelp: ");
        message.push_str(help);
    }
    let mut fields = vec![
        ("range", range_json(range)),
        ("severity", Json::Int(severity_of(item.severity))),
        ("message", Json::string(message)),
        ("source", Json::string("korben")),
    ];
    if let Some(code) = &item.code {
        fields.push(("code", Json::string(code.clone())));
    }
    Json::object(fields)
}

fn severity_of(severity: Severity) -> i64 {
    match severity {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Note => 3,
    }
}

fn range_json(range: Range) -> Json {
    Json::object([("start", position_json(range.start)), ("end", position_json(range.end))])
}

fn position_json(position: Position) -> Json {
    Json::object([
        ("line", Json::Int(position.line as i64)),
        ("character", Json::Int(position.character as i64)),
    ])
}

fn position_of(params: &Json) -> Option<Position> {
    let position = params.get("position")?;
    Some(Position {
        line: position.get("line")?.as_i64()? as u32,
        character: position.get("character")?.as_i64()? as u32,
    })
}

/// The workspace root an `initialize` request names.
fn root_of(params: &Json) -> Option<PathBuf> {
    if let Some(folders) = params.get("workspaceFolders").and_then(Json::as_array) {
        if let Some(path) = folders.first().and_then(|folder| folder.get("uri")).and_then(path_of) {
            return Some(path);
        }
    }
    if let Some(path) = params.get("rootUri").and_then(path_of) {
        return Some(path);
    }
    params.get("rootPath").and_then(Json::as_str).map(PathBuf::from)
}

/// The path a `file:` URI names.
fn path_of(value: &Json) -> Option<PathBuf> {
    let uri = value.as_str()?;
    let rest = uri.strip_prefix("file://")?;
    // `file:///path` on Unix; a host, if any, is not a path this server can read.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    Some(PathBuf::from(percent_decode(rest)))
}

/// A `file:` URI for a path.
pub fn uri_of(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&text[index + 1..index + 3], 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What this server can do. Anything absent, a client will not ask for.
fn capabilities() -> Json {
    Json::object([
        (
            "capabilities",
            Json::object([
                // Full sync: a whole-document rewrite per change. Incremental
                // sync would be faster, and is not implemented rather than
                // being half-implemented.
                ("textDocumentSync", Json::Int(1)),
                ("hoverProvider", Json::Bool(true)),
                ("definitionProvider", Json::Bool(true)),
                ("documentSymbolProvider", Json::Bool(true)),
                ("documentFormattingProvider", Json::Bool(true)),
                (
                    "completionProvider",
                    Json::object([(
                        "triggerCharacters",
                        Json::array([Json::string("."), Json::string(":")]),
                    )]),
                ),
            ]),
        ),
        (
            "serverInfo",
            Json::object([
                ("name", Json::string("korben-lsp")),
                ("version", Json::string(env!("CARGO_PKG_VERSION"))),
            ]),
        ),
    ])
}
