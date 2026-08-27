//! Formatter tests. The formatter must be stable, idempotent, and
//! comment-preserving.

// korben-6bc

use korben_syntax::fmt::format_source;

fn format(text: &str) -> String {
    let (formatted, errors) = format_source(0, text);
    assert!(errors.is_empty(), "unexpected errors: {:?}", errors.first().map(|e| &e.message));
    formatted
}

fn assert_stable(input: &str, expected: &str) {
    let once = format(input);
    assert_eq!(once, expected, "\n--- got ---\n{once}\n--- want ---\n{expected}");
    let twice = format(&once);
    assert_eq!(twice, once, "formatting is not idempotent");
}

#[test]
fn short_calls_stay_on_one_line() {
    assert_stable("(+   1   2)", "(+ 1 2)\n");
    assert_stable("[1,2,3]", "[1 2 3]\n");
}

#[test]
fn declarations_break_their_body() {
    assert_stable(
        "(fn add [left: Int right: Int] -> Int (+ left right))",
        "(fn add [left: Int right: Int] -> Int\n  (+ left right))\n",
    );
}

#[test]
fn module_headers_break() {
    assert_stable(
        "(module app.main (use std.io) (use json [encode decode]))",
        "(module app.main\n  (use std.io)\n  (use json [encode decode]))\n",
    );
}

#[test]
fn match_clauses_get_one_line_each() {
    assert_stable(
        "(fn f [r] (match r (Ok user) (render user) (Err e) (fail e)))",
        "(fn f [r]\n  (match r\n    (Ok user) (render user)\n    (Err e) (fail e)))\n",
    );
}

#[test]
fn guards_stay_with_their_pattern() {
    let formatted = format("(fn f [t] (match t (Number n) :when (> n 0) (positive n) _ nil))");
    assert!(formatted.contains("(Number n) :when (> n 0) (positive n)"), "{formatted}");
}

#[test]
fn postfix_operators_attach() {
    let formatted =
        format("(fn f [p] -> Result Int E (let text (fs.read-text p) ?) (parse text) ?)");
    assert!(formatted.contains("(fs.read-text p)?"), "{formatted}");
    assert!(formatted.contains("(parse text)?"), "{formatted}");
}

#[test]
fn rest_patterns_are_not_treated_as_field_access() {
    let formatted = format("(fn f [v] (match v [head ...tail] head [] nil))");
    assert!(formatted.contains("[head ...tail]"), "{formatted}");
}

#[test]
fn reader_shortcuts_keep_their_sugar() {
    assert_stable("(quote a)", "'a\n");
    assert_stable("(macro m [x] (syntax-quote (do (unquote x))))", "(macro m [x]\n  `(do ~x))\n");
}

#[test]
fn comments_are_preserved() {
    let input = ";;; docs\n(fn f []\n  ; explain\n  1)\n";
    let formatted = format(input);
    assert!(formatted.contains(";;; docs"), "{formatted}");
    assert!(formatted.contains("; explain"), "{formatted}");
}

#[test]
fn blank_lines_between_declarations_collapse_to_one() {
    let formatted = format("(fn a [] 1)\n\n\n\n(fn b [] 2)\n");
    assert_eq!(formatted, "(fn a []\n  1)\n\n(fn b []\n  2)\n");
}

#[test]
fn a_doc_comment_stays_attached_to_its_declaration() {
    let formatted = format("(fn a [] 1)\n\n;;; docs\n\n(fn b [] 2)\n");
    assert!(formatted.contains(";;; docs\n(fn b []"), "{formatted}");
}

#[test]
fn long_calls_align_under_the_first_argument() {
    let long = format(&format!("(some-function {} )", "\"argument-value\" ".repeat(8)));
    let lines: Vec<&str> = long.lines().collect();
    assert!(lines.len() > 1, "expected a break: {long}");
    let indent = lines[1].len() - lines[1].trim_start().len();
    assert_eq!(indent, "(some-function ".len());
}

#[test]
fn unparsable_input_is_left_alone() {
    let text = "(fn f [] 1";
    let (formatted, errors) = format_source(0, text);
    assert_eq!(formatted, text);
    assert!(!errors.is_empty());
}
