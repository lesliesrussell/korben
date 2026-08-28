//! Driving the server the way an editor does: framed messages in, messages out.

// korben-efd

use std::path::{Path, PathBuf};

use korben_lsp::json::{parse, Json};
use korben_lsp::server::{uri_of, Server};

/// A scratch project that cleans itself up.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let unique =
            format!("korben-lsp-{label}-{}-{:?}", std::process::id(), std::thread::current().id());
        let path = std::env::temp_dir().join(unique.replace(['(', ')', ' '], ""));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("src")).expect("create scratch project");
        std::fs::write(
            path.join("korben.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nlicense = \"MIT\"\nmain = \"main\"\n",
        )
        .expect("write manifest");
        Scratch(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create directory");
        }
        std::fs::write(&path, text).expect("write source");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Frame a request the way the protocol requires.
fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{body}", body.len())
}

fn request(id: i64, method: &str, params: &str) -> String {
    frame(&format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#))
}

fn notification(method: &str, params: &str) -> String {
    frame(&format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{params}}}"#))
}

/// Run a conversation and split the replies apart.
fn converse(root: &Path, messages: &[String]) -> Vec<Json> {
    let input = messages.concat();
    let mut reader = std::io::BufReader::new(input.as_bytes());
    let mut output: Vec<u8> = Vec::new();
    Server::new(root.to_path_buf()).serve(&mut reader, &mut output).expect("serve");
    split(&String::from_utf8(output).expect("utf-8 output"))
}

/// Split framed output into messages, checking each header as it goes.
fn split(text: &str) -> Vec<Json> {
    let mut messages = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let header_end = rest.find("\r\n\r\n").expect("a framed message");
        let header = &rest[..header_end];
        let length: usize = header
            .split(':')
            .nth(1)
            .expect("a Content-Length value")
            .trim()
            .parse()
            .expect("a numeric length");
        let body_start = header_end + 4;
        let body = &rest[body_start..body_start + length];
        messages.push(parse(body).expect("valid JSON"));
        rest = &rest[body_start + length..];
    }
    messages
}

/// The result of the response with this id.
fn result_of(messages: &[Json], id: i64) -> Option<&Json> {
    messages
        .iter()
        .find(|message| message.get("id").and_then(Json::as_i64) == Some(id))
        .and_then(|message| message.get("result"))
}

/// Every `publishDiagnostics` notification, in order.
fn published(messages: &[Json]) -> Vec<&Json> {
    messages
        .iter()
        .filter(|message| {
            message.get("method").and_then(Json::as_str) == Some("textDocument/publishDiagnostics")
        })
        .filter_map(|message| message.get("params"))
        .collect()
}

fn open(path: &Path, text: &str) -> String {
    let uri = uri_of(path);
    let escaped = Json::string(text).render();
    notification(
        "textDocument/didOpen",
        &format!(
            r#"{{"textDocument":{{"uri":"{uri}","languageId":"korben","version":1,"text":{escaped}}}}}"#
        ),
    )
}

fn at(path: &Path, line: i64, character: i64) -> String {
    let uri = uri_of(path);
    format!(
        r#"{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{character}}}}}"#
    )
}

const GREETING: &str = r#"(module main
  (use std.string :as string))

;;; Greet someone by name.
(pub fn greet [name: String] -> String
  (format "Hello, {name}!"))

(pub fn main [] -> Unit !io
  (let message (greet "Ada"))
  (println message))
"#;

#[test]
fn initialize_advertises_what_the_server_can_do() {
    let scratch = Scratch::new("initialize");
    let messages = converse(scratch.path(), &[request(1, "initialize", "{}")]);
    let capabilities = result_of(&messages, 1).and_then(|result| result.get("capabilities"));
    let capabilities = capabilities.expect("capabilities");
    assert_eq!(capabilities.get("hoverProvider"), Some(&Json::Bool(true)));
    assert_eq!(capabilities.get("definitionProvider"), Some(&Json::Bool(true)));
    assert_eq!(capabilities.get("documentFormattingProvider"), Some(&Json::Bool(true)));
    assert!(capabilities.get("completionProvider").is_some());
    // Full sync, which is what the change handler is written for.
    assert_eq!(capabilities.get("textDocumentSync"), Some(&Json::Int(1)));
}

#[test]
fn opening_a_clean_file_publishes_no_errors() {
    let scratch = Scratch::new("clean");
    let path = scratch.write("src/main.kb", GREETING);
    let messages = converse(scratch.path(), &[open(&path, GREETING)]);
    let diagnostics = published(&messages);
    for params in diagnostics {
        let items = params.get("diagnostics").and_then(Json::as_array).expect("a list");
        let errors: Vec<&Json> = items
            .iter()
            .filter(|item| item.get("severity").and_then(Json::as_i64) == Some(1))
            .collect();
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
    }
}

#[test]
fn an_error_is_published_for_the_file_it_is_in() {
    let scratch = Scratch::new("errors");
    let broken = "(module main)\n\n(pub fn main [] -> Unit !io (println (missng \"x\")))\n";
    let path = scratch.write("src/main.kb", broken);
    let messages = converse(scratch.path(), &[open(&path, broken)]);
    let uri = uri_of(&path);
    let params = published(&messages)
        .into_iter()
        .find(|params| params.get("uri").and_then(Json::as_str) == Some(uri.as_str()))
        .expect("diagnostics for the open file");
    let items = params.get("diagnostics").and_then(Json::as_array).expect("a list");
    let item =
        items.iter().find(|item| item.get("code").and_then(Json::as_str) == Some("unbound-name"));
    let item = item.expect("the unbound name");
    assert_eq!(item.get("severity"), Some(&Json::Int(1)));
    assert_eq!(item.get("source"), Some(&Json::string("korben")));
    // The range must point at the name, on the line the name is written.
    assert_eq!(item.path(&["range", "start", "line"]).and_then(Json::as_i64), Some(2));
}

#[test]
fn an_unsaved_buffer_is_what_gets_checked() {
    let scratch = Scratch::new("overlay");
    // On disk the file is fine; the buffer the editor holds is not.
    let path = scratch.write("src/main.kb", GREETING);
    let edited = GREETING.replace("(greet \"Ada\")", "(greet-typo \"Ada\")");
    let messages = converse(scratch.path(), &[open(&path, &edited)]);
    let uri = uri_of(&path);
    let params = published(&messages)
        .into_iter()
        .find(|params| params.get("uri").and_then(Json::as_str) == Some(uri.as_str()))
        .expect("diagnostics for the open file");
    let items = params.get("diagnostics").and_then(Json::as_array).expect("a list");
    assert!(
        items.iter().any(|item| item
            .get("message")
            .and_then(Json::as_str)
            .map(|message| message.contains("greet-typo"))
            .unwrap_or(false)),
        "the buffer was not what got checked: {items:?}"
    );
}

#[test]
fn fixing_a_file_clears_the_diagnostics_it_had() {
    let scratch = Scratch::new("clearing");
    let broken = "(module main)\n\n(pub fn main [] -> Unit !io (println (missng \"x\")))\n";
    let path = scratch.write("src/main.kb", broken);
    let uri = uri_of(&path);
    // Fully clean, lints included, so an empty publish is the only right answer.
    let fixed = Json::string(
        "(module main)\n\n;;; Say hello.\n(pub fn main [] -> Unit !io (println \"x\"))\n",
    );
    let messages = converse(
        scratch.path(),
        &[
            open(&path, broken),
            notification(
                "textDocument/didChange",
                &format!(
                    r#"{{"textDocument":{{"uri":"{uri}","version":2}},"contentChanges":[{{"text":{}}}]}}"#,
                    fixed.render()
                ),
            ),
        ],
    );
    let last = published(&messages)
        .into_iter()
        .rfind(|params| params.get("uri").and_then(Json::as_str) == Some(uri.as_str()))
        .expect("a final publish for the file");
    let items = last.get("diagnostics").and_then(Json::as_array).expect("a list");
    assert!(items.is_empty(), "diagnostics were not cleared: {items:?}");
}

#[test]
fn hover_on_a_declaration_shows_its_signature_and_documentation() {
    let scratch = Scratch::new("hover-decl");
    let path = scratch.write("src/main.kb", GREETING);
    // Line 8 is `(let message (greet "Ada"))`; the cursor sits on `greet`.
    let messages = converse(
        scratch.path(),
        &[open(&path, GREETING), request(1, "textDocument/hover", &at(&path, 8, 17))],
    );
    let value = result_of(&messages, 1)
        .and_then(|result| result.path(&["contents", "value"]))
        .and_then(Json::as_str)
        .expect("hover contents");
    assert!(value.contains("greet"), "{value}");
    assert!(value.contains("String"), "{value}");
    assert!(value.contains("Greet someone by name."), "{value}");
}

#[test]
fn hover_on_a_local_shows_the_type_inference_gave_it() {
    let scratch = Scratch::new("hover-local");
    let path = scratch.write("src/main.kb", GREETING);
    // Line 9 is `(println message)`, where `message` is a local binding.
    let messages = converse(
        scratch.path(),
        &[open(&path, GREETING), request(1, "textDocument/hover", &at(&path, 9, 13))],
    );
    let value = result_of(&messages, 1)
        .and_then(|result| result.path(&["contents", "value"]))
        .and_then(Json::as_str)
        .expect("hover contents");
    assert!(value.contains("message: String"), "{value}");
}

#[test]
fn go_to_definition_finds_the_declaration() {
    let scratch = Scratch::new("definition");
    let path = scratch.write("src/main.kb", GREETING);
    let messages = converse(
        scratch.path(),
        &[open(&path, GREETING), request(1, "textDocument/definition", &at(&path, 8, 17))],
    );
    let result = result_of(&messages, 1).expect("a location");
    assert_eq!(result.get("uri").and_then(Json::as_str), Some(uri_of(&path).as_str()));
    // `(pub fn greet ...)` starts on line 4.
    assert_eq!(result.path(&["range", "start", "line"]).and_then(Json::as_i64), Some(4));
}

#[test]
fn completion_after_an_alias_offers_that_modules_members() {
    let scratch = Scratch::new("completion-alias");
    let source =
        "(module main\n  (use std.string :as string))\n\n(pub fn f [] -> String (string.))\n";
    let path = scratch.write("src/main.kb", source);
    // The cursor sits just after `string.` on line 3.
    let messages = converse(
        scratch.path(),
        &[open(&path, source), request(1, "textDocument/completion", &at(&path, 3, 31))],
    );
    let items = result_of(&messages, 1)
        .and_then(|result| result.get("items"))
        .and_then(Json::as_array)
        .expect("completion items");
    let labels: Vec<&str> = items.iter().filter_map(|item| item.get("label")?.as_str()).collect();
    assert!(labels.contains(&"lower"), "{labels:?}");
    assert!(labels.contains(&"split"), "{labels:?}");
    // Only that module's members, so a name from elsewhere must not appear.
    assert!(!labels.contains(&"println"), "{labels:?}");
}

#[test]
fn completion_elsewhere_offers_declarations_and_builtins() {
    let scratch = Scratch::new("completion-open");
    let path = scratch.write("src/main.kb", GREETING);
    let messages = converse(
        scratch.path(),
        &[open(&path, GREETING), request(1, "textDocument/completion", &at(&path, 9, 3))],
    );
    let items = result_of(&messages, 1)
        .and_then(|result| result.get("items"))
        .and_then(Json::as_array)
        .expect("completion items");
    let labels: Vec<&str> = items.iter().filter_map(|item| item.get("label")?.as_str()).collect();
    assert!(labels.contains(&"greet"), "the file's own declaration is missing: {labels:?}");
    assert!(labels.contains(&"println"), "a prelude builtin is missing: {labels:?}");
}

#[test]
fn document_symbols_outline_the_file() {
    let scratch = Scratch::new("symbols");
    let path = scratch.write("src/main.kb", GREETING);
    let uri = uri_of(&path);
    let messages = converse(
        scratch.path(),
        &[
            open(&path, GREETING),
            request(
                1,
                "textDocument/documentSymbol",
                &format!(r#"{{"textDocument":{{"uri":"{uri}"}}}}"#),
            ),
        ],
    );
    let symbols = result_of(&messages, 1).and_then(Json::as_array).expect("symbols");
    let names: Vec<&str> = symbols.iter().filter_map(|item| item.get("name")?.as_str()).collect();
    assert_eq!(names, vec!["greet", "main"]);
}

#[test]
fn formatting_returns_one_edit_for_the_whole_document() {
    let scratch = Scratch::new("formatting");
    let unformatted = "(module main)\n\n(pub fn  f  [] -> Int    (+ 1    2))\n";
    let path = scratch.write("src/main.kb", unformatted);
    let uri = uri_of(&path);
    let messages = converse(
        scratch.path(),
        &[
            open(&path, unformatted),
            request(
                1,
                "textDocument/formatting",
                &format!(r#"{{"textDocument":{{"uri":"{uri}"}},"options":{{}}}}"#),
            ),
        ],
    );
    let edits = result_of(&messages, 1).and_then(Json::as_array).expect("edits");
    assert_eq!(edits.len(), 1, "{edits:?}");
    let text = edits[0].get("newText").and_then(Json::as_str).expect("replacement text");
    // The canonical formatter always breaks a declaration onto its own lines.
    assert_eq!(text, "(module main)\n\n(pub fn f [] -> Int\n  (+ 1 2))\n");
}

#[test]
fn formatting_a_file_that_does_not_parse_changes_nothing() {
    let scratch = Scratch::new("formatting-broken");
    let broken = "(module main)\n\n(pub fn f [] -> Int (+ 1 2)\n";
    let path = scratch.write("src/main.kb", broken);
    let uri = uri_of(&path);
    let messages = converse(
        scratch.path(),
        &[
            open(&path, broken),
            request(
                1,
                "textDocument/formatting",
                &format!(r#"{{"textDocument":{{"uri":"{uri}"}},"options":{{}}}}"#),
            ),
        ],
    );
    let edits = result_of(&messages, 1).and_then(Json::as_array).expect("edits");
    assert!(edits.is_empty(), "an unparseable file was rewritten: {edits:?}");
}

#[test]
fn an_unimplemented_request_is_refused_rather_than_ignored() {
    let scratch = Scratch::new("unimplemented");
    let messages = converse(scratch.path(), &[request(7, "textDocument/rename", "{}")]);
    let error = messages
        .iter()
        .find(|message| message.get("id").and_then(Json::as_i64) == Some(7))
        .and_then(|message| message.get("error"))
        .expect("an error response");
    assert_eq!(error.get("code"), Some(&Json::Int(-32601)));
}

#[test]
fn shutdown_is_answered_and_exit_ends_the_session() {
    let scratch = Scratch::new("lifecycle");
    let messages = converse(
        scratch.path(),
        &[
            request(1, "shutdown", "null"),
            frame(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            // Anything after `exit` must not be served.
            request(2, "initialize", "{}"),
        ],
    );
    assert!(result_of(&messages, 1).is_some(), "shutdown went unanswered");
    assert!(result_of(&messages, 2).is_none(), "the server kept going past exit");
}
