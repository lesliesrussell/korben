//! Reader and diagnostic tests.

// korben-6bc

use korben_syntax::diag::Diagnostic;
use korben_syntax::reader::{CommentKind, Comments, Datum};
use korben_syntax::{read_all, SourceMap};

fn read(text: &str) -> (Vec<korben_syntax::Syntax>, Vec<Diagnostic>) {
    read_all(0, text, Comments::Skip)
}

fn render(text: &str) -> String {
    let (forms, errors) = read(text);
    assert!(errors.is_empty(), "unexpected reader errors in `{text}`: {:?}", errors[0].message);
    forms.iter().map(ToString::to_string).collect::<Vec<_>>().join(" ")
}

#[test]
fn reads_literals() {
    assert_eq!(render("42 -17 3.5 true false nil"), "42 -17 3.5 true false nil");
    assert_eq!(render(r#""hello\nworld""#), "\"hello\\nworld\"");
    assert_eq!(render(":ready :http/port"), ":ready :http/port");
    assert_eq!(render("0xff 0b1010 1_000"), "255 10 1000");
}

#[test]
fn reads_collections() {
    assert_eq!(render("(a b c)"), "(a b c)");
    assert_eq!(render("[1 2 3]"), "[1 2 3]");
    assert_eq!(render("{:host \"local\" :port 80}"), "{:host \"local\" :port 80}");
    assert_eq!(render("#{:read :write}"), "#{:read :write}");
    // Commas are whitespace.
    assert_eq!(render("[1, 2, 3]"), "[1 2 3]");
}

#[test]
fn reads_reader_shortcuts() {
    assert_eq!(render("'form"), "(quote form)");
    assert_eq!(render("`(a ~b ~@c)"), "(syntax-quote (a (unquote b) (unquote-splice c)))");
    assert_eq!(render("#'name"), "(var-ref name)");
    assert_eq!(render("#(+ % 1)"), "(fn-shorthand + % 1)");
}

#[test]
fn reads_raw_and_escaped_strings() {
    assert_eq!(render(r##"r#"a"b"#"##), r#""a\"b""#);
    let (forms, _) = read(r#""\u{1f600}""#);
    assert!(matches!(&forms[0].datum, Datum::Str(text) if text.chars().count() == 1));
}

#[test]
fn nested_block_comments_are_skipped() {
    let (forms, errors) = read("#| outer #| inner |# outer |# 1");
    assert!(errors.is_empty());
    assert_eq!(forms.len(), 1);
    assert!(matches!(forms[0].datum, Datum::Int(1)));
}

#[test]
fn doc_comments_survive_at_top_level_only() {
    let (forms, _) = read(";;; docs\n(fn f [] ; inline\n  1)");
    assert_eq!(forms.len(), 2);
    assert!(matches!(&forms[0].datum, Datum::Comment(CommentKind::Doc, text) if text == "docs"));
    // The inline comment inside the list is dropped.
    assert_eq!(forms[1].to_string(), "(fn f [] 1)");
}

#[test]
fn spans_point_at_the_source() {
    let mut sources = SourceMap::new();
    let text = "(fn add [a b]\n  (+ a b))";
    let file = sources.add_file(std::path::Path::new("t.kb"), text);
    let (forms, _) = read_all(file, text, Comments::Skip);
    let body = forms[0].as_list().unwrap().last().unwrap();
    assert_eq!(sources.snippet(body.span), "(+ a b)");
    assert_eq!(sources.location(body.span), "t.kb:2:3");
}

#[test]
fn unclosed_delimiter_is_reported_once() {
    let (_, errors) = read("(fn f [] 1");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_deref(), Some("reader-delimiter"));
}

#[test]
fn unterminated_string_is_reported() {
    let (_, errors) = read("\"oops");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_deref(), Some("reader-string"));
}

#[test]
fn diagnostics_render_with_a_caret() {
    let mut sources = SourceMap::new();
    let text = "(fn f [] 1)";
    let file = sources.add_file(std::path::Path::new("t.kb"), text);
    let span = korben_syntax::Span::new(file, 4, 5);
    let rendered = Diagnostic::error("bad name")
        .with_code("test")
        .at(span, "here")
        .help("rename it")
        .render(&sources, false);
    assert!(rendered.contains("error[test]: bad name"));
    assert!(rendered.contains("t.kb:1:5"));
    assert!(rendered.contains("^ here"));
    assert!(rendered.contains("help: rename it"));
}

#[test]
fn diagnostics_render_as_json() {
    let mut sources = SourceMap::new();
    let file = sources.add("t.kb", "(fn f [] 1)");
    let span = korben_syntax::Span::new(file, 4, 5);
    let json = Diagnostic::error("bad \"name\"").at(span, "here").to_json(&sources);
    assert!(json.contains(r#""severity":"error""#));
    assert!(json.contains(r#""message":"bad \"name\"""#));
    assert!(json.contains(r#""line":1"#));
}
