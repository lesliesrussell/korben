//! Shared helpers for evaluating and checking Korben source in tests.
//!
//! Each integration test binary pulls this module in and uses a subset of it.
#![allow(dead_code)]

// korben-6bc

use korben_core::eval::Output;
use korben_core::project::Session;
use korben_syntax::span::Span;
use std::path::PathBuf;

/// Evaluation recurses over the AST, so tests run it on a large stack the way
/// the `korben` executable does, with `max_depth` as the real recursion bound.
const STACK_SIZE: usize = 256 * 1024 * 1024;
const MAX_DEPTH: usize = 4_096;

fn on_big_stack<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(work)
        .expect("spawn test worker")
        .join()
        .expect("test worker panicked")
}

pub struct Run {
    pub output: String,
    /// The rendered result of calling `main`, when it returned normally.
    pub value: Option<String>,
    pub diagnostics: Vec<String>,
}

/// Load `source` as a module and call its `main`, capturing stdout.
pub fn run(source: &str) -> Run {
    let source = source.to_string();
    on_big_stack(move || run_inner(&source))
}

fn run_inner(source: &str) -> Run {
    let mut session = Session::bare(PathBuf::from("."));
    session.interp.max_depth.set(MAX_DEPTH);
    session.interp.out.replace(Output::Captured(String::new()));
    let loaded = session.load_text("test", source);
    let mut diagnostics: Vec<String> = session
        .diagnostics
        .items
        .iter()
        .filter(|item| item.is_error())
        .map(|item| item.code.clone().unwrap_or_else(|| item.message.clone()))
        .collect();

    let mut value = None;
    if diagnostics.is_empty() {
        if let Ok(runtime) = loaded {
            let main = runtime.globals.borrow().get("main").cloned();
            if let Some(main) = main {
                *session.interp.current.borrow_mut() = runtime;
                match session.interp.apply(main, Vec::new(), Span::synthetic()) {
                    Ok(result) => value = Some(result.to_string()),
                    Err(flow) => {
                        let diagnostic =
                            korben_core::project::flow_diagnostic(flow, Span::synthetic());
                        diagnostics.push(
                            diagnostic.code.clone().unwrap_or_else(|| diagnostic.message.clone()),
                        );
                    }
                }
            }
        }
    }

    let output = match session.interp.out.replace(Output::Stdout) {
        Output::Captured(text) => text,
        Output::Stdout => String::new(),
    };
    Run { output, value, diagnostics }
}

/// Evaluate `expr` as the body of `main` and return what it printed.
pub fn eval(expr: &str) -> String {
    let source = format!("(fn main [] (println {expr}))");
    let result = run(&source);
    assert!(
        result.diagnostics.is_empty(),
        "unexpected errors evaluating `{expr}`: {:?}",
        result.diagnostics
    );
    result.output.trim_end().to_string()
}

fn error_codes(session: &Session) -> Vec<String> {
    session
        .diagnostics
        .items
        .iter()
        .filter(|item| item.is_error())
        .map(|item| item.code.clone().unwrap_or_else(|| item.message.clone()))
        .collect()
}

/// Type-check `source` and return the diagnostic codes it produces.
pub fn check(source: &str) -> Vec<String> {
    check_with(source, false)
}

/// Type-check in `--strict-api` mode.
pub fn check_strict(source: &str) -> Vec<String> {
    check_with(source, true)
}

// korben-4io
/// Type-check `source` and return the rendered error messages, help included.
pub fn check_messages(source: &str) -> Vec<String> {
    let mut session = Session::bare(PathBuf::from("."));
    let _ = session.load_text("test", source);
    korben_core::infer::check_session(&mut session, false);
    session
        .diagnostics
        .items
        .iter()
        .filter(|item| item.is_error())
        .map(|item| format!("{} -- {}", item.message, item.help.join("; ")))
        .collect()
}

fn check_with(source: &str, strict: bool) -> Vec<String> {
    let mut session = Session::bare(PathBuf::from("."));
    let _ = session.load_text("test", source);
    korben_core::infer::check_session(&mut session, strict);
    error_codes(&session)
}

/// Run the lint rules and return their codes.
pub fn lint(source: &str) -> Vec<String> {
    let mut session = Session::bare(PathBuf::from("."));
    let _ = session.load_text("test", source);
    korben_core::infer::lint_session(&session)
        .items
        .iter()
        .map(|item| item.code.clone().unwrap_or_else(|| item.message.clone()))
        .collect()
}
