//! The project-aware REPL.
//!
//! Definitions persist across evaluations, the project's modules and
//! dependencies are in scope, and every result is reported with its inferred
//! type. Commands begin with `:`.

// korben-6bc

use crate::commands::{self, Flags, VERSION};
use crate::ui;
use korben_core::eval::Interp;
use korben_core::project::Session;
use korben_core::value::{Env, Value};
use korben_syntax::diag::Diagnostics;
use korben_syntax::lexer::{Lexer, TokenKind};
use korben_syntax::reader::Datum;
use korben_syntax::span::Span;
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

pub fn run(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut session = match Session::open(&cwd) {
        Ok(session) => session,
        Err(_) => Session::bare(cwd.clone()),
    };
    session.interp.max_depth.set(commands::MAX_CALL_DEPTH);
    let project_name = session.manifest.name.clone();
    load_project(&mut session, &flags);

    let interactive = std::io::stdin().is_terminal();
    if interactive {
        println!("{}", ui::bold(&format!("Korben {VERSION}")));
        println!("project: {project_name}");
        println!("{}", ui::dim("type :help for commands, :quit to exit"));
    }

    // The REPL evaluates in its own module so definitions accumulate safely.
    let runtime = session.interp.module("repl");
    scope_project_into_repl(&mut session);
    *session.interp.current.borrow_mut() = runtime.clone();
    let env = Env::root();
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut pending = String::new();

    loop {
        if interactive {
            let prompt = if pending.is_empty() { "kb> " } else { "...  " };
            print!("{prompt}");
            let _ = std::io::stdout().flush();
        }
        let Some(Ok(line)) = lines.next() else { break };
        if pending.is_empty() && line.trim().is_empty() {
            continue;
        }
        if pending.is_empty() {
            if let Some(command) = line.trim().strip_prefix(':') {
                match dispatch(command, &mut session, &env, &flags) {
                    Command::Continue => continue,
                    Command::Quit => break,
                }
            }
        }
        pending.push_str(&line);
        pending.push('\n');
        if !is_balanced(&pending) {
            continue;
        }
        let source = std::mem::take(&mut pending);
        evaluate(&mut session, &source, &env, interactive);
    }
    if interactive {
        println!();
    }
    ExitCode::SUCCESS
}

enum Command {
    Continue,
    Quit,
}

fn dispatch(command: &str, session: &mut Session, env: &Env, flags: &Flags) -> Command {
    let mut parts = command.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match name {
        "quit" | "q" | "exit" => return Command::Quit,
        "help" | "?" => {
            println!("{}", ui::bold("commands"));
            println!("  :help                 show this message");
            println!("  :quit                 leave the REPL");
            println!("  :reload               reload the project from disk");
            println!("  :type <expr>          show the inferred type of an expression");
            println!("  :expand <form>        show one round of macro expansion");
            println!("  :tests [filter]       run the project's tests");
            println!("  :modules              list loaded modules");
            println!("  :bindings             list definitions made in this session");
            println!("  :doc <name>           show documentation for a declaration");
        }
        "reload" => {
            let root = session.root.clone();
            let mut fresh = match Session::open(&root) {
                Ok(fresh) => fresh,
                Err(_) => Session::bare(root),
            };
            fresh.interp.max_depth.set(commands::MAX_CALL_DEPTH);
            load_project(&mut fresh, flags);
            let errors = fresh.diagnostics.error_count();
            report(&fresh.diagnostics, &fresh);
            *session = fresh;
            scope_project_into_repl(session);
            let repl_module = session.interp.module("repl");
            *session.interp.current.borrow_mut() = repl_module;
            if errors == 0 {
                println!("{} {} module(s)", ui::green("reloaded"), session.modules.len());
            }
        }
        "type" => {
            if rest.is_empty() {
                println!("{} :type needs an expression", ui::yellow("usage:"));
                return Command::Continue;
            }
            let file = session.sources.add("<repl:type>", rest.to_string());
            let (forms, errors) =
                korben_syntax::read_all(file, rest, korben_syntax::Comments::Skip);
            let mut diagnostics = Diagnostics::new();
            for error in errors {
                diagnostics.push(error);
            }
            let expanded =
                korben_core::expand::expand_module(&session.interp, &forms, &mut diagnostics);
            let Some(form) = expanded.first() else {
                println!("{} nothing to type", ui::yellow("note:"));
                return Command::Continue;
            };
            let expr = korben_core::lower::lower_expr(file, form, &mut diagnostics);
            if report(&diagnostics, session) {
                return Command::Continue;
            }
            println!("{}", korben_core::infer::type_of(session, &expr));
        }
        "expand" => {
            if rest.is_empty() {
                println!("{} :expand needs a form", ui::yellow("usage:"));
                return Command::Continue;
            }
            let file = session.sources.add("<repl:expand>", rest.to_string());
            let (forms, errors) =
                korben_syntax::read_all(file, rest, korben_syntax::Comments::Skip);
            let mut diagnostics = Diagnostics::new();
            for error in errors {
                diagnostics.push(error);
            }
            let expanded =
                korben_core::expand::expand_module(&session.interp, &forms, &mut diagnostics);
            if report(&diagnostics, session) {
                return Command::Continue;
            }
            print!("{}", korben_syntax::fmt::format_forms(&expanded));
        }
        "tests" => {
            let filter = if rest.is_empty() { None } else { Some(rest.to_string()) };
            run_tests(session, filter, env);
        }
        "modules" => {
            let mut names: Vec<String> = session.interp.modules.borrow().keys().cloned().collect();
            names.sort();
            for name in names {
                println!("  {name}");
            }
        }
        "bindings" => {
            let runtime = session.interp.module("repl");
            let globals = runtime.globals.borrow();
            let mut names: Vec<&str> = globals.keys().map(String::as_str).collect();
            names.sort();
            if names.is_empty() {
                println!("{}", ui::dim("  no definitions yet"));
            }
            for name in names {
                println!("  {name}");
            }
        }
        "doc" => {
            if rest.is_empty() {
                println!("{} :doc needs a name", ui::yellow("usage:"));
                return Command::Continue;
            }
            let mut found = false;
            for module in &session.modules {
                for item in &module.items {
                    if item.name() != rest {
                        continue;
                    }
                    found = true;
                    if let korben_core::ast::Item::Fn(decl) = item {
                        println!("{}", ui::bold(&korben_core::docs::signature(decl)));
                        match &decl.doc {
                            Some(doc) => println!("{doc}"),
                            None => println!("{}", ui::dim("no documentation")),
                        }
                    } else {
                        println!("{} in {}", ui::bold(item.name()), module.name);
                    }
                }
            }
            if !found {
                println!("{} no declaration named `{rest}`", ui::yellow("note:"));
            }
        }
        other => {
            println!("{} unknown command `:{other}` — try `:help`", ui::yellow("note:"));
        }
    }
    Command::Continue
}

fn evaluate(session: &mut Session, source: &str, env: &Env, interactive: bool) {
    let file = session.sources.add("<repl>", source.to_string());
    let (forms, errors) = korben_syntax::read_all(file, source, korben_syntax::Comments::Skip);
    let mut diagnostics = Diagnostics::new();
    for error in errors {
        diagnostics.push(error);
    }
    let expanded =
        korben_core::expand::expand_module(&session.interp, &forms, &mut diagnostics);
    if report(&diagnostics, session) {
        return;
    }

    for form in &expanded {
        // Declarations extend the session; anything else is an expression.
        if is_declaration(form) {
            let mut diagnostics = Diagnostics::new();
            let module = korben_core::lower::lower_module(
                file,
                "repl",
                std::slice::from_ref(form),
                &mut diagnostics,
            );
            if report(&diagnostics, session) {
                continue;
            }
            let names: Vec<String> =
                module.items.iter().map(|item| item.name().to_string()).collect();
            session.declare(module);
            if report(&session.diagnostics.clone(), session) {
                session.diagnostics = Diagnostics::new();
                continue;
            }
            session.diagnostics = Diagnostics::new();
            if interactive {
                for name in names {
                    println!("{} {name}", ui::dim("defined"));
                }
            }
            continue;
        }

        let mut diagnostics = Diagnostics::new();
        let expr = korben_core::lower::lower_expr(file, form, &mut diagnostics);
        if report(&diagnostics, session) {
            continue;
        }
        match session.interp.eval(&expr, env) {
            Ok(Value::Nil) if !interactive => {}
            Ok(value) => {
                let ty = korben_core::infer::type_of(session, &expr);
                if ty == "_" || ty == "Unit" {
                    println!("{value}");
                } else {
                    println!("{value} {}", ui::dim(&format!(": {ty}")));
                }
            }
            Err(flow) => {
                let diagnostic = korben_core::project::flow_diagnostic(flow, form.span);
                eprint!("{}", diagnostic.render(&session.sources, ui::use_color()));
            }
        }
    }
}

fn run_tests(session: &mut Session, filter: Option<String>, env: &Env) {
    let tests = session.interp.tests.borrow().clone();
    let mut passed = 0usize;
    let mut failed = 0usize;
    for (module_name, name, decl, runtime) in tests {
        if let Some(filter) = &filter {
            if !name.contains(filter.as_str()) && !module_name.contains(filter.as_str()) {
                continue;
            }
        }
        *session.interp.current.borrow_mut() = runtime;
        let scope = env.child();
        match session.interp.eval_body(&decl.body, &scope) {
            Ok(_) => {
                passed += 1;
                println!("  {} {name}", ui::green("ok"));
            }
            Err(flow) => {
                failed += 1;
                println!("  {} {name}", ui::red("FAIL"));
                let diagnostic = korben_core::project::flow_diagnostic(flow, decl.span);
                eprint!("{}", diagnostic.render(&session.sources, ui::use_color()));
            }
        }
    }
    let repl_module = session.interp.module("repl");
    *session.interp.current.borrow_mut() = repl_module;
    println!("{passed} passed, {failed} failed");
}

fn load_project(session: &mut Session, flags: &Flags) {
    commands::load_all(session, flags);
    report(&session.diagnostics.clone(), session);
    session.diagnostics = Diagnostics::new();
}

/// Make the project visible from the REPL: every loaded module is reachable by
/// its last path segment, and the entry module's exports are in scope directly.
fn scope_project_into_repl(session: &mut Session) {
    let runtime = session.interp.module("repl");
    let names: Vec<String> = session.modules.iter().map(|module| module.name.clone()).collect();
    for name in &names {
        let alias = name.rsplit('.').next().unwrap_or(name).to_string();
        runtime.aliases.borrow_mut().insert(alias, name.clone());
    }
    let entry = session.manifest.main.clone();
    let Some(module) = session.interp.modules.borrow().get(&entry).cloned() else { return };
    let exported: Vec<String> = module.exports.borrow().keys().cloned().collect();
    for name in exported {
        runtime.imported.borrow_mut().insert(name.clone(), (entry.clone(), name));
    }
}

fn report(diagnostics: &Diagnostics, session: &Session) -> bool {
    for diagnostic in &diagnostics.items {
        eprint!("{}", diagnostic.render(&session.sources, ui::use_color()));
    }
    diagnostics.has_errors()
}

fn is_declaration(form: &korben_syntax::Syntax) -> bool {
    let head = match form.head_symbol() {
        Some(head) => head,
        None => return false,
    };
    if head == "pub" {
        return form.as_list().and_then(|items| items.get(1)).map(is_declaration).unwrap_or(false);
    }
    matches!(
        head,
        "fn" | "async-fn"
            | "type"
            | "enum"
            | "protocol"
            | "impl"
            | "test"
            | "property"
            | "derive"
            | "def"
            | "use"
            | "module"
    ) && !matches!(form.datum, Datum::Symbol(_))
}

/// True when every delimiter in the buffer is closed, so the form is complete.
fn is_balanced(source: &str) -> bool {
    let (tokens, _) = Lexer::new(0, source).tokenize();
    let mut depth = 0i32;
    for token in tokens {
        match token.kind {
            TokenKind::LParen
            | TokenKind::LBracket
            | TokenKind::LBrace
            | TokenKind::HashBrace
            | TokenKind::HashParen => depth += 1,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => depth -= 1,
            _ => {}
        }
    }
    depth <= 0
}

/// Suppress an unused-import warning when the interpreter type is only named.
#[allow(dead_code)]
fn interpreter_type(interp: &Interp) -> usize {
    interp.modules.borrow().len()
}

#[allow(dead_code)]
fn unused_span() -> Span {
    Span::synthetic()
}
