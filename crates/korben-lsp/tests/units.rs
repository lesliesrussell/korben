//! The JSON codec, position arithmetic, and message framing.

// korben-efd

use korben_lsp::json::{parse, Json};
use korben_lsp::position::{to_offset, to_position, word_at, Position};
use korben_lsp::rpc;

// ---------------------------------------------------------------------- json

#[test]
fn json_round_trips_through_text() {
    let source = r#"{"a":[1,-2,3.5],"b":{"c":true,"d":null},"e":"x\ny"}"#;
    let value = parse(source).expect("parse");
    assert_eq!(parse(&value.render()).expect("reparse"), value);
}

#[test]
fn strings_escape_what_json_requires() {
    let rendered = Json::string("quote \" backslash \\ newline \n bell \u{7}").render();
    // The control character has no literal form in JSON, so it is escaped.
    assert_eq!(rendered, "\"quote \\\" backslash \\\\ newline \\n bell \\u0007\"");
    assert_eq!(
        parse(&rendered).expect("parse").as_str(),
        Some("quote \" backslash \\ newline \n bell \u{7}")
    );
}

#[test]
fn a_surrogate_pair_decodes_to_one_character() {
    // An emoji arrives from an editor as a `\uXXXX\uXXXX` pair.
    let value = parse(r#""😀""#).expect("parse");
    assert_eq!(value.as_str(), Some("😀"));
}

#[test]
fn a_lone_leading_surrogate_is_refused() {
    assert!(parse(r#""\ud83d""#).is_err());
}

#[test]
fn malformed_input_is_an_error_rather_than_a_panic() {
    for source in ["{", "[1,", r#"{"a"}"#, r#"{a:1}"#, "tru", "", "{} extra"] {
        assert!(parse(source).is_err(), "`{source}` should not parse");
    }
}

#[test]
fn a_nested_field_is_reachable_by_path() {
    let value = parse(r#"{"textDocument":{"uri":"file:///a.kb"}}"#).expect("parse");
    assert_eq!(value.path(&["textDocument", "uri"]).and_then(Json::as_str), Some("file:///a.kb"));
    assert!(value.path(&["textDocument", "missing"]).is_none());
    assert!(value.path(&["nope", "uri"]).is_none());
}

// ------------------------------------------------------------------ position

#[test]
fn a_position_is_counted_in_utf16_code_units() {
    // `é` is two bytes and one code unit; `😀` is four bytes and two units.
    let text = "aé😀b";
    assert_eq!(to_position(text, 0), Position { line: 0, character: 0 });
    assert_eq!(to_position(text, 1), Position { line: 0, character: 1 });
    assert_eq!(to_position(text, 3), Position { line: 0, character: 2 });
    assert_eq!(to_position(text, 7), Position { line: 0, character: 4 });
}

#[test]
fn positions_and_offsets_are_inverses() {
    let text = "one\ntwo é\n😀 four\n";
    for offset in 0..=text.len() {
        if !text.is_char_boundary(offset) {
            continue;
        }
        assert_eq!(to_offset(text, to_position(text, offset)), offset, "at byte {offset}");
    }
}

#[test]
fn a_position_past_the_end_clamps_rather_than_panicking() {
    let text = "one\ntwo\n";
    assert_eq!(to_offset(text, Position { line: 99, character: 0 }), text.len());
    // Past the end of a line lands on that line's end, not the next line.
    assert_eq!(to_offset(text, Position { line: 0, character: 99 }), 3);
}

#[test]
fn a_word_is_the_whole_korben_symbol() {
    let text = "(string.split-once text \"?\")";
    let (start, end) = word_at(text, 3).expect("a word");
    assert_eq!(&text[start..end], "string.split-once");
    let (start, end) = word_at(text, 19).expect("a word");
    assert_eq!(&text[start..end], "text");
}

#[test]
fn a_cursor_just_past_a_name_still_means_that_name() {
    let text = "(empty? xs)";
    // Offset 7 is the space after `empty?`, where an editor leaves the cursor.
    let (start, end) = word_at(text, 7).expect("a word");
    assert_eq!(&text[start..end], "empty?");
}

#[test]
fn punctuation_is_not_a_word() {
    assert!(word_at("(  )", 2).is_none());
    assert!(word_at("", 0).is_none());
}

// ----------------------------------------------------------------- framing

#[test]
fn a_framed_message_reads_back() {
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"x":1}}"#;
    let framed = format!("Content-Length: {}\r\n\r\n{body}", body.len());
    let mut input = std::io::BufReader::new(framed.as_bytes());
    let message = rpc::read(&mut input).expect("read").expect("a message");
    assert_eq!(message.method, "initialize");
    assert!(message.is_request());
    assert_eq!(message.params.get("x").and_then(Json::as_i64), Some(1));
    assert!(rpc::read(&mut input).expect("read").is_none());
}

#[test]
fn a_body_length_is_counted_in_bytes_not_characters() {
    // The body holds a character that is one char and four bytes; a length
    // counted in characters would cut the message short.
    let body = r#"{"jsonrpc":"2.0","method":"x","params":{"t":"😀"}}"#;
    let framed = format!("Content-Length: {}\r\n\r\n{body}", body.len());
    let mut input = std::io::BufReader::new(framed.as_bytes());
    let message = rpc::read(&mut input).expect("read").expect("a message");
    assert_eq!(message.params.path(&["t"]).and_then(Json::as_str), Some("😀"));
}

#[test]
fn headers_are_matched_without_regard_to_case() {
    let body = r#"{"jsonrpc":"2.0","method":"x"}"#;
    let framed = format!(
        "content-length: {}\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{body}",
        body.len()
    );
    let mut input = std::io::BufReader::new(framed.as_bytes());
    let message = rpc::read(&mut input).expect("read").expect("a message");
    assert_eq!(message.method, "x");
}

#[test]
fn a_truncated_message_is_an_error_rather_than_a_hang() {
    let mut input = std::io::BufReader::new(&b"Content-Length: 40\r\n\r\n{\"a\":1}"[..]);
    assert!(rpc::read(&mut input).is_err());
    let mut input = std::io::BufReader::new(&b"Content-Length: 10\r\n"[..]);
    assert!(rpc::read(&mut input).is_err());
}

#[test]
fn a_message_without_a_length_is_refused() {
    let mut input = std::io::BufReader::new(&b"Content-Type: text/plain\r\n\r\n{}"[..]);
    assert!(rpc::read(&mut input).is_err());
}

#[test]
fn a_response_carries_the_id_it_answers() {
    let mut out: Vec<u8> = Vec::new();
    rpc::respond(&mut out, Json::Int(7), Json::string("ok")).expect("respond");
    let text = String::from_utf8(out).expect("utf-8");
    let (header, body) = text.split_once("\r\n\r\n").expect("framing");
    assert_eq!(header, format!("Content-Length: {}", body.len()));
    let value = parse(body).expect("parse");
    assert_eq!(value.get("id").and_then(Json::as_i64), Some(7));
    assert_eq!(value.get("result").and_then(Json::as_str), Some("ok"));
}
