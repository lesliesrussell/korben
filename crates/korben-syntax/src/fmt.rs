//! The canonical formatter.
//!
//! `korben fmt` is stable and macro-aware: a source file has exactly one
//! preferred rendering per compiler version. The formatter works on syntax
//! objects read in [`Comments::Keep`] mode so comments and author-intended
//! blank lines survive, and it never asks the user to hand-format anything.

// korben-6bc

use crate::diag::Diagnostic;
use crate::reader::{read_all, CommentKind, Comments, Datum, Syntax};
use crate::span::FileId;

/// Preferred maximum line width. Forms that fit are printed on one line.
pub const MAX_WIDTH: usize = 96;
const INDENT: usize = 2;

/// Format a whole source file. Returns the formatted text plus any reader
/// diagnostics; when reading fails the original text is returned unchanged so
/// that `korben fmt` never destroys a file it cannot parse.
pub fn format_source(file: FileId, text: &str) -> (String, Vec<Diagnostic>) {
    let (forms, diagnostics) = read_all(file, text, Comments::Keep);
    if diagnostics.iter().any(Diagnostic::is_error) {
        return (text.to_string(), diagnostics);
    }
    (format_forms(&forms), diagnostics)
}

/// Format a sequence of top-level forms.
pub fn format_forms(forms: &[Syntax]) -> String {
    let mut out = String::new();
    for (index, form) in forms.iter().enumerate() {
        if index > 0 {
            out.push('\n');
            // A doc comment binds tightly to the declaration beneath it.
            if form.blank_before && !is_doc_comment(&forms[index - 1]) {
                out.push('\n');
            }
        }
        out.push_str(&render(form, 0));
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn is_doc_comment(form: &Syntax) -> bool {
    matches!(&form.datum, Datum::Comment(CommentKind::Doc, _))
}

/// A postfix token the reader leaves separate: `?` and `.field`.
fn is_postfix(form: &Syntax) -> bool {
    match form.as_symbol() {
        Some("?") => true,
        // `...rest` is a rest pattern, not a field access.
        Some(name) => name.len() > 1 && name.starts_with('.') && !name.starts_with("..."),
        None => false,
    }
}

/// Group a sequence into units of one form plus its trailing postfix tokens,
/// so `(fs.read path) ?` prints as `(fs.read path)?`.
fn units(items: &[Syntax]) -> Vec<&[Syntax]> {
    let mut groups: Vec<&[Syntax]> = Vec::new();
    let mut start = 0usize;
    for (index, item) in items.iter().enumerate() {
        if index > start && !is_postfix(item) {
            groups.push(&items[start..index]);
            start = index;
        }
    }
    if start < items.len() {
        groups.push(&items[start..]);
    }
    groups
}

fn flat_unit(unit: &[Syntax]) -> Option<String> {
    let mut out = flat(&unit[0])?;
    for postfix in &unit[1..] {
        out.push_str(&atom(postfix));
    }
    Some(out)
}

fn render_unit(unit: &[Syntax], col: usize) -> String {
    let mut out = render(&unit[0], col);
    for postfix in &unit[1..] {
        out.push_str(&atom(postfix));
    }
    out
}

/// Declaration forms always put their body on its own line, which keeps the
/// shape of a module readable regardless of how short a definition happens to be.
fn always_breaks(head: &str) -> bool {
    matches!(
        head,
        "module"
            | "type"
            | "enum"
            | "protocol"
            | "impl"
            | "fn"
            | "async-fn"
            | "unsafe-fn"
            | "macro"
            | "test"
            | "property"
            | "match"
            | "try"
            | "restart-case"
    )
}

/// Render `form` assuming it starts at column `col`.
pub fn render(form: &Syntax, col: usize) -> String {
    if let Some(flat) = flat(form) {
        if col + flat.chars().count() <= MAX_WIDTH && !forces_break(form) {
            return flat;
        }
    }
    match &form.datum {
        Datum::List(items) => render_list(items, col),
        Datum::Vector(items) => render_seq(items, col, "[", "]"),
        Datum::Set(items) => render_seq(items, col, "#{", "}"),
        Datum::Map(items) => render_map(items, col),
        Datum::Tagged(tag, inner) => {
            format!("#{tag} {}", render(inner, col + tag.chars().count() + 2))
        }
        _ => flat(form).unwrap_or_else(|| atom(form)),
    }
}

/// Single-line rendering, or `None` when the form contains a comment and must break.
fn flat(form: &Syntax) -> Option<String> {
    match &form.datum {
        Datum::Comment(..) => None,
        Datum::List(items) => flat_seq(items, "(", ")"),
        Datum::Vector(items) => flat_seq(items, "[", "]"),
        Datum::Set(items) => flat_seq(items, "#{", "}"),
        Datum::Map(items) => flat_map(items),
        Datum::Tagged(tag, inner) => Some(format!("#{tag} {}", flat(inner)?)),
        _ => Some(atom(form)),
    }
}

fn flat_seq(items: &[Syntax], open: &str, close: &str) -> Option<String> {
    // Reader shortcuts print in their sugared form.
    if let Some(sugar) = flat_reader_sugar(items) {
        return Some(sugar);
    }
    let mut parts = Vec::with_capacity(items.len());
    for unit in units(items) {
        parts.push(flat_unit(unit)?);
    }
    Some(format!("{open}{}{close}", parts.join(" ")))
}

/// True when a form must break even though it would fit on one line.
fn forces_break(form: &Syntax) -> bool {
    let Datum::List(items) = &form.datum else { return false };
    let Some(head) = items.first().and_then(Syntax::as_symbol) else { return false };
    let (head, rest) = if head == "pub" {
        match items.get(1).and_then(Syntax::as_symbol) {
            Some(inner) => (inner, &items[2..]),
            None => return false,
        }
    } else {
        (head, &items[1..])
    };
    if !always_breaks(head) {
        return false;
    }
    // A declaration with no body has nothing to break onto the next line.
    let keep = body_head_args(head, rest).unwrap_or(0);
    rest.len() > keep
}

fn flat_map(items: &[Syntax]) -> Option<String> {
    let mut parts = Vec::new();
    for pair in items.chunks(2) {
        match pair {
            [key, value] => parts.push(format!("{} {}", flat(key)?, flat(value)?)),
            [key] => parts.push(flat(key)?),
            _ => unreachable!(),
        }
    }
    Some(format!("{{{}}}", parts.join(" ")))
}

/// Render `(quote x)` as `'x`, `(unquote x)` as `~x`, and so on.
fn flat_reader_sugar(items: &[Syntax]) -> Option<String> {
    if items.len() != 2 {
        return None;
    }
    let prefix = match items[0].as_symbol()? {
        "quote" => "'",
        "syntax-quote" => "`",
        "unquote" => "~",
        "unquote-splice" => "~@",
        "var-ref" => "#'",
        _ => return None,
    };
    Some(format!("{prefix}{}", flat(&items[1])?))
}

fn atom(form: &Syntax) -> String {
    match &form.datum {
        Datum::Nil => "nil".to_string(),
        Datum::Bool(value) => value.to_string(),
        Datum::Int(value) => value.to_string(),
        Datum::Float(value) => crate::format_float(*value),
        Datum::Str(value) => crate::diag::json_string(value),
        Datum::Keyword(name) => format!(":{name}"),
        Datum::Symbol(name) => name.clone(),
        Datum::Comment(CommentKind::Doc, text) => format!(";;; {text}").trim_end().to_string(),
        Datum::Comment(CommentKind::Line, text) => {
            if text.trim().is_empty() {
                ";".to_string()
            } else {
                format!("; {}", text.trim())
            }
        }
        Datum::Comment(CommentKind::Block, text) => format!("#|{text}|#"),
        _ => form.to_string(),
    }
}

fn indent_of(col: usize) -> String {
    " ".repeat(col)
}

/// Render a broken list form, choosing a layout based on its head symbol.
fn render_list(items: &[Syntax], col: usize) -> String {
    if items.is_empty() {
        return "()".to_string();
    }
    if let Some(rendered) = render_reader_sugar(items, col) {
        return rendered;
    }

    let groups = units(items);
    let head = &items[0];
    let args = &items[1..];

    let head_text = render(head, col + 1);
    let head_width = head_text.chars().count();

    if let Some(name) = head.as_symbol() {
        if name == "match" {
            return render_match(&head_text, args, col);
        }
        if let Some(keep) = body_head_args(name, args) {
            return render_body_form(&head_text, args, keep, col);
        }
    }

    // Ordinary call: subsequent arguments align under the first argument.
    let align = col + 1 + head_width + 1;
    if head.is_comment() || align > MAX_WIDTH.saturating_sub(8) {
        return render_body_form(&head_text, args, 0, col);
    }
    let mut out = format!("({head_text}");
    for (index, unit) in groups.iter().skip(1).enumerate() {
        if index == 0 {
            out.push(' ');
        } else {
            out.push('\n');
            out.push_str(&indent_of(align));
        }
        out.push_str(&render_unit(unit, align));
    }
    out.push(')');
    out
}

/// `(head a b\n  body...)` — `keep` arguments stay on the opening line.
fn render_body_form(head_text: &str, args: &[Syntax], keep: usize, col: usize) -> String {
    let body_col = col + INDENT;
    let mut out = format!("({head_text}");
    let keep = keep.min(args.len());
    for arg in &args[..keep] {
        out.push(' ');
        out.push_str(&render(arg, col + 1 + out.chars().count()));
    }
    for unit in units(&args[keep..]) {
        out.push('\n');
        if unit[0].blank_before {
            out.push('\n');
        }
        out.push_str(&indent_of(body_col));
        out.push_str(&render_unit(unit, body_col));
    }
    out.push(')');
    out
}

/// `match` renders the scrutinee on the head line, then one clause per group,
/// keeping a pattern and its body together when they fit.
fn render_match(head_text: &str, args: &[Syntax], col: usize) -> String {
    if args.is_empty() {
        return format!("({head_text})");
    }
    let body_col = col + INDENT;
    let mut out = format!("({head_text} {}", render(&args[0], col + 2 + head_text.chars().count()));

    let mut index = 1usize;
    let mut first_clause = true;
    while index < args.len() {
        if args[index].is_comment() {
            out.push('\n');
            out.push_str(&indent_of(body_col));
            out.push_str(&render(&args[index], body_col));
            index += 1;
            continue;
        }

        // A clause is `pattern [:when guard] body`.
        let pattern = &args[index];
        index += 1;
        let mut guard = None;
        if index + 1 < args.len() && args[index].as_keyword() == Some("when") {
            guard = Some((&args[index], &args[index + 1]));
            index += 2;
        }
        let body = args.get(index);
        if body.is_some() {
            index += 1;
        }

        out.push('\n');
        if !first_clause && pattern.blank_before {
            out.push('\n');
        }
        first_clause = false;
        out.push_str(&indent_of(body_col));

        let mut clause = render(pattern, body_col);
        if let Some((when_kw, guard_expr)) = guard {
            clause.push(' ');
            clause.push_str(&render(when_kw, body_col));
            clause.push(' ');
            clause.push_str(&render(guard_expr, body_col));
        }

        match body {
            Some(body) => {
                let inline = flat(body);
                let clause_width = clause.chars().count();
                match inline {
                    Some(text)
                        if !clause.contains('\n')
                            && body_col + clause_width + 1 + text.chars().count() <= MAX_WIDTH =>
                    {
                        out.push_str(&clause);
                        out.push(' ');
                        out.push_str(&text);
                    }
                    _ => {
                        out.push_str(&clause);
                        out.push('\n');
                        out.push_str(&indent_of(body_col + INDENT));
                        out.push_str(&render(body, body_col + INDENT));
                    }
                }
            }
            None => out.push_str(&clause),
        }
    }
    out.push(')');
    out
}

fn render_reader_sugar(items: &[Syntax], col: usize) -> Option<String> {
    if items.len() != 2 || items[1].is_comment() {
        return None;
    }
    let prefix = match items[0].as_symbol()? {
        "quote" => "'",
        "syntax-quote" => "`",
        "unquote" => "~",
        "unquote-splice" => "~@",
        "var-ref" => "#'",
        _ => return None,
    };
    Some(format!("{prefix}{}", render(&items[1], col + prefix.len())))
}

fn render_seq(items: &[Syntax], col: usize, open: &str, close: &str) -> String {
    if items.is_empty() {
        return format!("{open}{close}");
    }
    let align = col + open.chars().count();
    let mut out = String::from(open);
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push('\n');
            out.push_str(&indent_of(align));
        }
        out.push_str(&render(item, align));
    }
    out.push_str(close);
    out
}

/// Maps break one key/value pair per line, values aligned after their key.
fn render_map(items: &[Syntax], col: usize) -> String {
    if items.is_empty() {
        return "{}".to_string();
    }
    let align = col + 1;
    let mut out = String::from("{");
    let mut first = true;
    let mut index = 0usize;
    while index < items.len() {
        if !first {
            out.push('\n');
            out.push_str(&indent_of(align));
        }
        first = false;
        if items[index].is_comment() {
            out.push_str(&render(&items[index], align));
            index += 1;
            continue;
        }
        let key = render(&items[index], align);
        out.push_str(&key);
        index += 1;
        if index < items.len() && !items[index].is_comment() {
            out.push(' ');
            out.push_str(&render(&items[index], align + key.chars().count() + 1));
            index += 1;
        }
    }
    out.push('}');
    out
}

/// For forms with a body, how many arguments stay on the opening line.
/// `None` means the form is an ordinary call and uses argument alignment.
fn body_head_args(head: &str, args: &[Syntax]) -> Option<usize> {
    // `(pub fn name ...)` is one flat form; keep the inner form's layout.
    if head == "pub" {
        let inner = args.first()?.as_symbol()?;
        return Some(1 + body_head_args(inner, &args[1..])?);
    }
    let count = match head {
        // Signature forms: name, parameter vector, and any return/effect annotation.
        "fn" | "async-fn" | "macro" | "unsafe-fn" => signature_head_args(args),
        "async" | "comptime" => {
            // `(async fn name [..] -> T ..)` nests a signature form.
            if args.first().and_then(Syntax::as_symbol) == Some("fn") {
                1 + signature_head_args(&args[1..])
            } else {
                0
            }
        }
        "module" | "loop" | "task-scope" | "restart-case" | "test" | "when" | "unless"
        | "while" | "if" | "if-let" | "when-let" | "for" | "with-span" | "case" | "cond" => 1,
        "property" | "impl" | "with" | "def" => 2,
        "let" | "var" => 1 + annotation_run(args, 1),
        "type" | "enum" | "protocol" | "derive" => 1 + type_params_run(args, 1),
        "do" | "try" | "unsafe" | "quote" => 0,
        "set!" | "recur" | "throw" | "await" | "use" | "defer" | "catch" | "finally" => 0,
        _ => return None,
    };
    Some(count.min(args.len()))
}

/// `fn name [params] -> Ret !effects` — everything before the body.
fn signature_head_args(args: &[Syntax]) -> usize {
    let mut count = 0usize;
    // Function name (may be absent for anonymous `fn`).
    if matches!(args.first().map(|arg| &arg.datum), Some(Datum::Symbol(_))) {
        count += 1;
    }
    // Parameter vector.
    if matches!(args.get(count).map(|arg| &arg.datum), Some(Datum::Vector(_))) {
        count += 1;
    }
    // Return annotation: `-> T ...` runs until the first non-atom form.
    if args.get(count).and_then(Syntax::as_symbol) == Some("->") {
        count += 1;
        count += annotation_run(args, count);
    }
    count
}

/// Count the run of bare atoms starting at `from`, which is how Korben writes
/// type applications such as `Result UInt16 ConfigError !io`.
fn annotation_run(args: &[Syntax], from: usize) -> usize {
    let mut count = 0usize;
    while let Some(arg) = args.get(from + count) {
        match &arg.datum {
            Datum::Symbol(_) | Datum::Keyword(_) => count += 1,
            _ => break,
        }
    }
    count
}

/// `(type Cache K V { .. })` keeps the generic parameters on the head line.
fn type_params_run(args: &[Syntax], from: usize) -> usize {
    let mut count = 0usize;
    while let Some(arg) = args.get(from + count) {
        match &arg.datum {
            Datum::Symbol(name) if starts_uppercase(name) => count += 1,
            _ => break,
        }
    }
    count
}

fn starts_uppercase(name: &str) -> bool {
    name.chars().next().map(char::is_uppercase).unwrap_or(false)
}
