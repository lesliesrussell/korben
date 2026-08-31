//! Command dispatch for the `korben` executable.

// korben-6bc

use crate::ui;
use korben_core::eval::{Interp, Output};
use korben_core::project::{self, Session};
use korben_core::value::{Env, Flow, Value};
use korben_syntax::diag::{Diagnostic, Diagnostics};
use korben_syntax::span::Span;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Call depth allowed by the toolchain, sized for `main`'s large worker stack.
pub const MAX_CALL_DEPTH: usize = 8_192;

pub fn run(args: &[String]) -> ExitCode {
    let Some(command) = args.first().map(String::as_str) else {
        print_help();
        return ExitCode::SUCCESS;
    };
    let rest = &args[1..];
    match command {
        "help" | "--help" | "-h" => {
            print_help();
            ExitCode::SUCCESS
        }
        "version" | "--version" | "-V" => {
            println!("korben {VERSION}");
            ExitCode::SUCCESS
        }
        "new" => cmd_new(rest),
        "init" => cmd_init(rest),
        "run" => cmd_run(rest),
        "check" => cmd_check(rest),
        "fmt" => cmd_fmt(rest),
        "lint" => cmd_lint(rest),
        "test" => cmd_test(rest),
        "expand" => cmd_expand(rest),
        "repl" => crate::repl::run(rest),
        "doc" => cmd_doc(rest),
        "build" => cmd_build(rest),
        // korben-blf
        "publish" => cmd_publish(rest),
        // korben-poj
        "install" => cmd_install(rest),
        "doctor" => cmd_doctor(rest),
        "inspect" => cmd_inspect(rest),
        "dev" => cmd_dev(rest),
        "ffi" => cmd_ffi(rest),
        "add" => cmd_add(rest),
        "remove" => cmd_remove(rest),
        "update" => cmd_update(rest),
        "audit" => cmd_audit(rest),
        "lsp" => cmd_lsp(),
        other => {
            if let Some(milestone) = planned_command(other) {
                eprintln!(
                    "{} `korben {other}` is planned for {milestone}.",
                    ui::yellow("not yet implemented:")
                );
                eprintln!("  Run `korben help` to see what v{VERSION} supports.");
                return ExitCode::from(2);
            }
            eprintln!("{} unknown command `{other}`", ui::red("error:"));
            eprintln!("  Run `korben help` for the list of commands.");
            ExitCode::from(2)
        }
    }
}

/// Commands the specification defines but that land in a later milestone.
fn planned_command(name: &str) -> Option<&'static str> {
    Some(match name {
        "bench" => "Milestone D (benchmark harness)",
        _ => return None,
    })
}

fn print_help() {
    println!("{}", ui::bold(&format!("korben {VERSION}")));
    println!("A compiled, statically typed, ownership-safe Lisp for native software.\n");
    println!("{}", ui::bold("USAGE"));
    println!("  korben <command> [options]\n");
    println!("{}", ui::bold("PROJECT"));
    println!("  new <name> [--template cli|lib|service]   create a new project");
    println!("  init                                      add a manifest to this directory");
    println!("  doctor                                    report toolchain and project health\n");
    println!("{}", ui::bold("DEPENDENCIES"));
    println!("  add <name> [--version <req>] [--path <dir>] [--dev]");
    println!("  remove <name>                             drop a dependency");
    println!("  update                                    re-resolve and rewrite the lockfile");
    println!("  audit                                     verify the lockfile and checksums");
    println!("  publish [--registry <dir>]                copy this package into a registry");
    println!(
        "  install                                   fetch the git registry this project reads\n"
    );
    println!("{}", ui::bold("DEVELOP"));
    println!("  run [entry] [--profile] [-- args...]      run a project, member, or file");
    println!("  dev                                       check, test, then run");
    println!("  check [--json] [--strict-api]             type, effect, and ownership analysis");
    println!("  test [filter] [--json]                    run tests and property tests");
    println!("  fmt [--check] [paths...]                  canonical formatting");
    println!("  lint [--json]                             built-in lint rules");
    println!("  repl                                      project-aware interactive session");
    println!("  expand <file>                             show macro expansion");
    println!("  doc [--out <dir>]                         generate documentation");
    println!("  inspect                                   show the resolved project model");
    println!("  ffi [c <header>] [rust <file.rs>]         list or generate foreign bindings");
    println!(
        "  build [--release] [--target <triple>]     compile to a native executable\n\
         \x20       [--emit ir|rust]"
    );
    println!("  lsp                                       language server, on stdin and stdout\n");
    println!("{}", ui::bold("OTHER"));
    println!("  version                                   print the toolchain version");
    println!("  help                                      print this message");
}

// korben-mic
/// The entry module `run` and `build` should act on.
///
/// A workspace has more than one package, so which program is meant becomes a
/// real question. `--package` answers it; otherwise the sole member declaring
/// an entry point does, and anything else is an error that lists the choices
/// rather than a guess.
fn entry_module(session: &mut Session, flags: &Flags) -> Result<String, String> {
    let requested = flags.value("package");
    let chosen = match &session.workspace {
        Some(workspace) => {
            let member = workspace.program(requested)?;
            Some((member.name.clone(), member.manifest.main.clone()))
        }
        None => {
            if let Some(name) = requested {
                if name != session.manifest.name {
                    return Err(format!(
                        "this project is not a workspace, and its only package is `{}`",
                        session.manifest.name
                    ));
                }
            }
            None
        }
    };
    match chosen {
        // The session was opened on a stand-in member; point it at the one
        // actually being built, so the artifact is named and placed correctly.
        Some((package, entry)) => {
            session.focus(&package);
            Ok(entry)
        }
        None => Ok(session.manifest.main.clone()),
    }
}

// korben-efd
// -------------------------------------------------------------------- lsp

fn cmd_lsp() -> ExitCode {
    // The protocol owns stdout, so nothing else may print to it. A failure has
    // to go to stderr, where an editor's log will show it.
    match korben_lsp::serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{} {error}", ui::red("lsp:"));
            ExitCode::FAILURE
        }
    }
}

// ------------------------------------------------------------------ new/init

fn cmd_new(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let Some(name) = flags.positional.first() else {
        eprintln!("{} `korben new` needs a project name", ui::red("error:"));
        return ExitCode::from(2);
    };
    let template = flags.value("template").unwrap_or("cli").to_string();
    let dir = PathBuf::from(name);
    if dir.exists() {
        eprintln!("{} `{name}` already exists", ui::red("error:"));
        return ExitCode::from(2);
    }
    match scaffold(&dir, name, &template) {
        Ok(()) => {
            println!("{} project `{name}` ({template} template)", ui::green("created"));
            println!("\n  cd {name}");
            println!("  korben dev");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{} {error}", ui::red("error:"));
            ExitCode::FAILURE
        }
    }
}

fn cmd_init(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let name = flags
        .positional
        .first()
        .cloned()
        .or_else(|| dir.file_name().map(|name| name.to_string_lossy().to_string()))
        .unwrap_or_else(|| "app".to_string());
    if dir.join(project::MANIFEST_NAME).exists() {
        eprintln!("{} {} already exists", ui::red("error:"), project::MANIFEST_NAME);
        return ExitCode::from(2);
    }
    let template = flags.value("template").unwrap_or("cli").to_string();
    match scaffold(&dir, &name, &template) {
        Ok(()) => {
            println!("{} project `{name}` in this directory", ui::green("initialized"));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{} {error}", ui::red("error:"));
            ExitCode::FAILURE
        }
    }
}

fn scaffold(dir: &Path, name: &str, template: &str) -> Result<(), String> {
    let write = |path: PathBuf, contents: String| -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        std::fs::write(&path, contents)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    };

    let mut manifest = korben_core::manifest::Manifest::default_for(name);
    manifest.description = Some(format!("A Korben {template}"));
    manifest.license = Some("MIT".to_string());
    write(dir.join(project::MANIFEST_NAME), manifest.render())?;
    write(dir.join(".gitignore"), "/target\n".to_string())?;

    let (main_source, test_source) = match template {
        "lib" => (templates::LIB_MAIN, templates::LIB_TEST),
        "service" => (templates::SERVICE_MAIN, templates::SERVICE_TEST),
        _ => (templates::CLI_MAIN, templates::CLI_TEST),
    };
    write(dir.join("src/main.kb"), main_source.replace("{{name}}", name))?;
    write(dir.join("tests/main_test.kb"), test_source.replace("{{name}}", name))?;
    write(
        dir.join("README.md"),
        format!(
            "# {name}\n\nA Korben project.\n\n```sh\nkorben dev      # check, test, run\nkorben test\nkorben fmt\nkorben repl\n```\n"
        ),
    )?;
    Ok(())
}

mod templates {
    /// A command-line program: records, pattern matching, and process arguments.
    pub const CLI_MAIN: &str = r#"(module main
  (use std.io :as io)
  (use std.process :as process))

(pub type Greeting
  {message: String})

;;; Build the greeting shown to the user.
(pub fn greeting [name: String] -> Greeting
  (Greeting {message (format "Hello, {name}!")}))

(pub fn main [] -> Unit !io
  (let args (process.args))
  (let name (match args [] "world" [first ...rest] first))
  (io.println (greeting name).message))
"#;

    pub const CLI_TEST: &str = r#"(module main_test
  (use main [greeting]))

(test "greets the given name"
  (assert-eq "Hello, Ada!" (greeting "Ada").message))

(test "greets the world by default"
  (assert-eq "Hello, world!" (greeting "world").message))
"#;

    /// A reusable function plus a small driver.
    pub const LIB_MAIN: &str = r#"(module main
  (use std.string :as string))

;;; Turn a title into a URL-safe slug.
(pub fn slugify [input: String] -> String
  (let lowered (string.lower input))
  (let words (filter (string.split lowered " ") (fn [word] (not (empty? word)))))
  (string.join words "-"))

(pub fn main [] -> Unit !io
  (println (slugify "Korben Is Fast")))
"#;

    pub const LIB_TEST: &str = r#"(module main_test
  (use main [slugify]))

(test "slugifies a title"
  (assert-eq "korben-is-fast" (slugify "Korben Is Fast")))

(test "collapses repeated spaces"
  (assert-eq "a-b" (slugify "a  b")))
"#;

    // korben-qrt
    /// An HTTP service over `std.http`: routing by pattern match, and a `serve`
    /// argument that puts the same handler on a socket.
    pub const SERVICE_MAIN: &str = r#"(module main
  (use std.http :as http)
  (use std.json :as json)
  (use std.log :as log)
  (use std.process :as process))

(pub enum AppError
  (BadRequest message: String))

;;; Build the greeting for a name.
(pub fn greeting-for [name: String] -> Result String AppError
  (if (empty? name) (Err (BadRequest "a name is required")) (Ok (format "Hello, {name}!"))))

;;; Route a request to a response.
;;;
;;; The request is an ordinary record, so the method, path, and query string
;;; are ordinary fields to match on.
(pub fn handle [request: http.Request] -> http.Response
  (match request
    {:method :get :path "/health"} (http.text 200 "ok")

    {:method :get :path "/greeting" :query {"name" name}}
      (match (greeting-for name)
        (Ok message) (http.json 200 (json.encode {message message}))
        (Err (BadRequest reason)) (http.json 400 (json.encode {error reason})))

    {:method :get :path "/greeting"} (http.json 200 (json.encode {message "Hello, world!"}))
    ; A write endpoint reads the request body.
    {:method :post :path "/greeting"}
      (do
        (let name request.body)
        (match (greeting-for name)
          (Ok message) (http.json 201 (json.encode {message message}))
          (Err (BadRequest reason)) (http.json 400 (json.encode {error reason}))))

    _ (http.not-found)))

(pub fn main [] -> Unit !io
  (match (process.args)
    ; `serve` puts the handler on a socket. Requests are handled one at a time.
    ["serve"]
      (do
        (log.info "listening" {port 3000})
        (match (http.serve "127.0.0.1:3000" handle)
          (Ok _) nil
          (Err error) (println "server stopped:" (http.describe error))))
    ; Without a socket, the handler is an ordinary function to call.
    _ (println (handle (http.test-request :get "/greeting?name=Ada")).body)))
"#;

    pub const SERVICE_TEST: &str = r#"(module main_test
  (use main [handle greeting-for BadRequest])
  (use std.http :as http))

(test "health endpoint is available"
  (let response (handle (http.test-request :get "/health")))
  (assert-eq 200 response.status)
  (assert-eq "ok" response.body))

(test "the greeting reads the query string"
  (let response (handle (http.test-request :get "/greeting?name=Ada")))
  (assert-eq 200 response.status)
  (assert (contains? response.body "Hello, Ada!") "expected the name in the body"))

(test "an empty name is a bad request"
  (assert-eq (Err (BadRequest "a name is required")) (greeting-for "")))

(test "unknown paths are not found"
  (assert-eq 404 (handle (http.test-request :get "/nope")).status))

; korben-ajx: `:body` lets a write endpoint be tested without a socket.
(test "posting a name greets it"
  (let response (handle (http.test-request :post "/greeting" :body "Ada")))
  (assert-eq 201 response.status)
  (assert (contains? response.body "Hello, Ada!") "expected the name in the body"))

(test "posting an empty name is a bad request"
  (assert-eq 400 (handle (http.test-request :post "/greeting" :body "")).status))
"#;
}

// -------------------------------------------------------------------- check

fn cmd_check(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let json = flags.has("json");
    let strict = flags.has("strict-api");
    let mut session = match open_session(&flags) {
        Ok(session) => session,
        Err(code) => return code,
    };
    load_all(&mut session, &flags);
    korben_core::infer::check_session(&mut session, strict);

    let failed = ui::report(&session.diagnostics, &session.sources, json);
    if !json {
        let summary = ui::summarize(&session.diagnostics);
        if failed {
            eprintln!("{} {summary}", ui::red("check failed:"));
        } else {
            println!(
                "{} {} module{} — {summary}",
                ui::green("checked"),
                session.modules.len(),
                ui::plural(session.modules.len())
            );
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

// ---------------------------------------------------------------------- run

fn cmd_run(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let mut session = match open_session(&flags) {
        Ok(session) => session,
        Err(code) => return code,
    };

    // `korben run path/to/file.kb` runs a single file; otherwise the entry module.
    let entry = flags.positional.first().map(PathBuf::from);
    let loaded = match &entry {
        Some(path) if path.exists() => session.load_file(path, None),
        Some(name) => session.load_module(&name.to_string_lossy(), Span::synthetic()),
        None => match entry_module(&mut session, &flags) {
            Ok(main) => session.load_module(&main, Span::synthetic()),
            Err(error) => {
                eprintln!("{} {error}", ui::red("error:"));
                return ExitCode::FAILURE;
            }
        },
    };

    // Korben is statically typed, so running checks first: the same programs
    // are accepted whether they are interpreted or compiled.
    if loaded.is_ok() {
        korben_core::infer::check_session(&mut session, false);
    }
    if ui::report(&session.diagnostics, &session.sources, false) || loaded.is_err() {
        eprintln!("{} {}", ui::red("run failed:"), ui::summarize(&session.diagnostics));
        return ExitCode::FAILURE;
    }
    let Ok(runtime) = loaded else { return ExitCode::FAILURE };

    let Some(main) = runtime.globals.borrow().get("main").cloned() else {
        eprintln!("{} module `{}` defines no `main`", ui::red("error:"), runtime.name);
        eprintln!("  Add `(pub fn main [] -> Unit !io ...)`.");
        return ExitCode::FAILURE;
    };

    *session.interp.current.borrow_mut() = runtime;
    korben_runtime::std::set_program_args(flags.passthrough.clone());
    // korben-ycd
    if flags.has("profile") {
        korben_runtime::profile::start();
    }
    let outcome = match session.interp.apply(main, Vec::new(), Span::synthetic()) {
        Ok(value) => finish_main(&value),
        Err(flow) => {
            let diagnostic = project::flow_diagnostic(flow, Span::synthetic());
            eprint!("{}", diagnostic.render(&session.sources, ui::use_color()));
            ExitCode::FAILURE
        }
    };
    // Reported even when the program failed: where it spent its time is often
    // the question being asked about a program that did not finish.
    if flags.has("profile") {
        report_profile();
    }
    outcome
}

// korben-ycd
/// Print what the profiler recorded, longest-running body first.
fn report_profile() {
    let rows = korben_runtime::profile::report();
    if rows.is_empty() {
        eprintln!("\n{} nothing ran", ui::dim("profile:"));
        return;
    }
    let total: u128 = rows.iter().map(|(_, entry)| entry.self_nanos).sum();
    eprintln!();
    eprintln!("{}", ui::bold("PROFILE"));
    eprintln!(
        "  {} is time in that function's own body, with everything it called",
        ui::dim("self")
    );
    eprintln!("  {}", ui::dim("subtracted, so a recursive call is not counted twice."));
    eprintln!();
    eprintln!("  {:>10}  {:>10}  {:>6}  function", "calls", "self", "share");
    for (name, entry) in rows.iter().take(20) {
        let share = if total == 0 { 0.0 } else { entry.self_nanos as f64 * 100.0 / total as f64 };
        eprintln!(
            "  {:>10}  {:>10}  {:>5.1}%  {name}",
            entry.calls,
            human_nanos(entry.self_nanos),
            share
        );
    }
    if rows.len() > 20 {
        eprintln!("  {}", ui::dim(&format!("and {} more", rows.len() - 20)));
    }
}

// korben-ycd
fn human_nanos(nanos: u128) -> String {
    if nanos >= 1_000_000_000 {
        format!("{:.2}s", nanos as f64 / 1_000_000_000.0)
    } else if nanos >= 1_000_000 {
        format!("{:.1}ms", nanos as f64 / 1_000_000.0)
    } else if nanos >= 1_000 {
        format!("{:.1}µs", nanos as f64 / 1_000.0)
    } else {
        format!("{nanos}ns")
    }
}

/// `main` returning `Err` is a failed process exit, per the CLI conventions.
fn finish_main(value: &Value) -> ExitCode {
    if let Value::Variant(variant) = value {
        if &*variant.variant == "Err" {
            let payload = variant
                .fields
                .first()
                .map(|(_, value)| korben_core::value::display(value))
                .unwrap_or_default();
            eprintln!("{} {payload}", ui::red("error:"));
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn cmd_dev(args: &[String]) -> ExitCode {
    println!("{}", ui::bold("check"));
    let check = cmd_check(args);
    if check != ExitCode::SUCCESS {
        return check;
    }
    println!("\n{}", ui::bold("test"));
    let test = cmd_test(args);
    if test != ExitCode::SUCCESS {
        return test;
    }
    println!("\n{}", ui::bold("run"));
    cmd_run(args)
}

// --------------------------------------------------------------------- test

fn cmd_test(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let json = flags.has("json");
    let filter = flags.positional.first().cloned();
    let mut session = match open_session(&flags) {
        Ok(session) => session,
        Err(code) => return code,
    };
    load_all(&mut session, &flags);
    // korben-yjw
    // Tests run against checked code. Without this the runner reports a green
    // suite for a program that does not typecheck, so passing tests say
    // nothing about whether the code compiles.
    korben_core::infer::check_session(&mut session, false);

    if session.diagnostics.has_errors() {
        ui::report(&session.diagnostics, &session.sources, json);
        eprintln!("{} {}", ui::red("test failed:"), ui::summarize(&session.diagnostics));
        return ExitCode::FAILURE;
    }

    let tests = session.interp.tests.take();
    let mut passed = 0usize;
    let mut failures: Vec<(String, String, Diagnostic)> = Vec::new();
    let mut results = Vec::new();

    for (module_name, name, decl, runtime) in tests {
        if let Some(filter) = &filter {
            if !name.contains(filter.as_str()) && !module_name.contains(filter.as_str()) {
                continue;
            }
        }
        *session.interp.current.borrow_mut() = runtime;
        session.interp.out.replace(Output::Captured(String::new()));
        let env = Env::root();

        // Property tests bind each generator once per case.
        let cases = if decl.generators.is_empty() { 1 } else { 32 };
        let mut outcome: Result<(), Diagnostic> = Ok(());
        for _ in 0..cases {
            let scope = env.child();
            let mut generated = Ok(());
            for (binding, generator) in &decl.generators {
                match session.interp.eval(generator, &scope) {
                    Ok(value) => {
                        let sample = match sample_from(&session.interp, value, decl.span) {
                            Ok(sample) => sample,
                            Err(flow) => {
                                generated = Err(project::flow_diagnostic(flow, decl.span));
                                break;
                            }
                        };
                        scope.define(std::rc::Rc::from(binding.as_str()), sample);
                    }
                    Err(flow) => {
                        generated = Err(project::flow_diagnostic(flow, decl.span));
                        break;
                    }
                }
            }
            if let Err(diagnostic) = generated {
                outcome = Err(diagnostic);
                break;
            }
            if let Err(flow) = session.interp.eval_body(&decl.body, &scope) {
                outcome = Err(project::flow_diagnostic(flow, decl.span));
                break;
            }
        }

        let captured = match session.interp.out.replace(Output::Stdout) {
            Output::Captured(text) => text,
            Output::Stdout => String::new(),
        };
        match outcome {
            Ok(()) => {
                passed += 1;
                results.push((module_name.clone(), name.clone(), true));
                if !json {
                    println!("  {} {name}", ui::green("ok"));
                }
            }
            Err(diagnostic) => {
                results.push((module_name.clone(), name.clone(), false));
                if !json {
                    println!("  {} {name}", ui::red("FAIL"));
                }
                if !captured.is_empty() && !json {
                    for line in captured.lines() {
                        println!("       {}", ui::dim(line));
                    }
                }
                failures.push((module_name, name, diagnostic));
            }
        }
    }

    if json {
        let body: Vec<String> = results
            .iter()
            .map(|(module, name, ok)| {
                format!(
                    "{{\"module\":{},\"name\":{},\"passed\":{ok}}}",
                    korben_syntax::diag::json_string(module),
                    korben_syntax::diag::json_string(name)
                )
            })
            .collect();
        println!(
            "{{\"passed\":{passed},\"failed\":{},\"tests\":[{}]}}",
            failures.len(),
            body.join(",")
        );
        return if failures.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE };
    }

    if !failures.is_empty() {
        println!();
        for (module, name, diagnostic) in &failures {
            println!("{} {module}/{name}", ui::red("failure:"));
            eprint!("{}", diagnostic.render(&session.sources, ui::use_color()));
        }
    }
    let total = passed + failures.len();
    if failures.is_empty() {
        println!("\n{} {passed}/{total} test{} passed", ui::green("success:"), ui::plural(total));
        ExitCode::SUCCESS
    } else {
        println!(
            "\n{} {}/{total} test{} failed",
            ui::red("failed:"),
            failures.len(),
            ui::plural(total)
        );
        ExitCode::FAILURE
    }
}

/// Draw one sample from a generator: a function is called, a vector is indexed.
fn sample_from(interp: &Interp, generator: Value, span: Span) -> Result<Value, Flow> {
    match generator {
        Value::Fn(_) => interp.apply(generator, Vec::new(), span),
        other => Ok(other),
    }
}

// ---------------------------------------------------------------------- fmt

fn cmd_fmt(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let check_only = flags.has("check");
    let paths = if flags.positional.is_empty() {
        let root = project::find_manifest(&std::env::current_dir().unwrap_or_default())
            .and_then(|manifest| manifest.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        project::source_files(&root)
    } else {
        flags
            .positional
            .iter()
            .flat_map(|path| {
                let path = PathBuf::from(path);
                if path.is_dir() {
                    project::source_files(&path)
                } else {
                    vec![path]
                }
            })
            .collect()
    };

    if paths.is_empty() {
        println!("{} no `.kb` files found", ui::yellow("nothing to format:"));
        return ExitCode::SUCCESS;
    }

    let mut sources = korben_syntax::SourceMap::new();
    let mut diagnostics = Diagnostics::new();
    let mut changed = Vec::new();
    for path in &paths {
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("{} cannot read {}", ui::red("error:"), path.display());
            continue;
        };
        let file = sources.add_file(path, text.clone());
        let (formatted, errors) = korben_syntax::fmt::format_source(file, &text);
        for error in errors {
            diagnostics.push(error);
        }
        if formatted == text {
            continue;
        }
        changed.push(path.clone());
        if !check_only {
            if let Err(error) = std::fs::write(path, formatted) {
                eprintln!("{} cannot write {}: {error}", ui::red("error:"), path.display());
                return ExitCode::FAILURE;
            }
        }
    }

    if ui::report(&diagnostics, &sources, false) {
        return ExitCode::FAILURE;
    }
    if check_only {
        if changed.is_empty() {
            println!(
                "{} {} file{} already formatted",
                ui::green("ok:"),
                paths.len(),
                ui::plural(paths.len())
            );
            return ExitCode::SUCCESS;
        }
        for path in &changed {
            println!("{} {}", ui::yellow("needs formatting:"), path.display());
        }
        return ExitCode::FAILURE;
    }
    println!(
        "{} {} of {} file{}",
        ui::green("formatted"),
        changed.len(),
        paths.len(),
        ui::plural(paths.len())
    );
    ExitCode::SUCCESS
}

// --------------------------------------------------------------------- lint

fn cmd_lint(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let json = flags.has("json");
    let mut session = match open_session(&flags) {
        Ok(session) => session,
        Err(code) => return code,
    };
    load_all(&mut session, &flags);
    let lints = korben_core::infer::lint_session(&session);
    let mut all = session.diagnostics.clone();
    all.extend(lints);

    let failed = ui::report(&all, &session.sources, json);
    if !json {
        println!("{} {}", ui::bold("lint:"), ui::summarize(&all));
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

// ------------------------------------------------------------------- expand

fn cmd_expand(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let Some(target) = flags.positional.first() else {
        eprintln!("{} `korben expand` needs a file", ui::red("error:"));
        return ExitCode::from(2);
    };
    let path = PathBuf::from(target);
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("{} cannot read {}", ui::red("error:"), path.display());
        return ExitCode::FAILURE;
    };

    let mut session = match open_session(&flags) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let file = session.sources.add_file(&path, text.clone());
    let (forms, errors) = korben_syntax::read_all(file, &text, korben_syntax::Comments::Skip);
    for error in errors {
        session.diagnostics.push(error);
    }
    let mut diagnostics = Diagnostics::new();
    let expanded = korben_core::expand::expand_module(&session.interp, &forms, &mut diagnostics);
    session.diagnostics.extend(diagnostics);

    if ui::report(&session.diagnostics, &session.sources, false) {
        return ExitCode::FAILURE;
    }
    // Expansion output is printed through the canonical formatter.
    print!("{}", korben_syntax::fmt::format_forms(&expanded));
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------- doc

fn cmd_doc(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let out = PathBuf::from(flags.value("out").unwrap_or("target/doc"));
    let mut session = match open_session(&flags) {
        Ok(session) => session,
        Err(code) => return code,
    };
    load_all(&mut session, &flags);
    if ui::report(&session.diagnostics, &session.sources, false) {
        return ExitCode::FAILURE;
    }
    if let Err(error) = std::fs::create_dir_all(&out) {
        eprintln!("{} cannot create {}: {error}", ui::red("error:"), out.display());
        return ExitCode::FAILURE;
    }

    let mut index = String::from("# API documentation\n\n");
    for module in &session.modules {
        let markdown = korben_core::docs::render_module(module);
        let file = out.join(format!("{}.md", module.name));
        if let Err(error) = std::fs::write(&file, markdown) {
            eprintln!("{} cannot write {}: {error}", ui::red("error:"), file.display());
            return ExitCode::FAILURE;
        }
        index.push_str(&format!("- [{}]({}.md)\n", module.name, module.name));
    }
    let json = korben_core::docs::render_api_json(&session.modules);
    let _ = std::fs::write(out.join("api.json"), json);
    if let Err(error) = std::fs::write(out.join("index.md"), index) {
        eprintln!("{} cannot write index: {error}", ui::red("error:"));
        return ExitCode::FAILURE;
    }
    println!(
        "{} {} module{} to {}",
        ui::green("documented"),
        session.modules.len(),
        ui::plural(session.modules.len()),
        out.display()
    );
    ExitCode::SUCCESS
}

// -------------------------------------------------------------------- build

fn cmd_build(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let release = flags.has("release");
    let emit = flags.value("emit").unwrap_or("").to_string();
    // korben-8cg
    let target = flags.value("target").map(str::to_string);
    let mut session = match open_session(&flags) {
        Ok(session) => session,
        Err(code) => return code,
    };
    load_all(&mut session, &flags);
    korben_core::infer::check_session(&mut session, flags.has("strict-api"));
    if ui::report(&session.diagnostics, &session.sources, false) {
        eprintln!("{} {}", ui::red("build failed:"), ui::summarize(&session.diagnostics));
        return ExitCode::FAILURE;
    }

    // Lowering to core IR resolves every name, so it also catches references
    // the interpreter would only have failed on at run time.
    let entry = match flags.positional.first().map(PathBuf::from) {
        // Building a single file makes that file the entry point.
        Some(path) if path.is_file() => session
            .modules
            .last()
            .map(|module| module.name.clone())
            .unwrap_or_else(|| session.manifest.main.clone()),
        _ => match entry_module(&mut session, &flags) {
            Ok(main) => main,
            Err(error) => {
                eprintln!("{} {error}", ui::red("error:"));
                return ExitCode::FAILURE;
            }
        },
    };
    let program = match korben_core::ir::lower_session(&session, &entry) {
        Ok(program) => program,
        Err(diagnostics) => return report_all(&diagnostics, &session),
    };

    if emit == "ir" {
        print!("{}", korben_core::ir::render(&program));
        return ExitCode::SUCCESS;
    }
    if program.entry.is_none() {
        eprintln!("{} module `{entry}` defines no `main`", ui::red("build failed:"));
        eprintln!("  Add `(pub fn main [] -> Unit !io ...)`.");
        return ExitCode::FAILURE;
    }

    let generated = match korben_core::codegen::generate(&program, &session.sources) {
        Ok(generated) => generated,
        Err(diagnostics) => return report_all(&diagnostics, &session),
    };
    if emit == "rust" {
        print!("{}", generated.main);
        return ExitCode::SUCCESS;
    }

    // korben-8cg
    if let Some(triple) = &target {
        if let Err(code) = verify_target(triple) {
            return code;
        }
    }

    let profile = if release { "release" } else { "debug" };
    // korben-8cg
    // Cross-built artifacts sit under their triple, the way cargo arranges
    // them, so building for two targets does not overwrite one with the other.
    let out = match &target {
        Some(triple) => session.root.join("target").join(triple).join(profile),
        None => session.root.join("target").join(profile),
    };
    // The generated Rust does not depend on the target, so every target shares
    // one generated crate and cargo keeps their build directories apart.
    let crate_dir = session.root.join("target").join("codegen").join(profile);
    if let Err(error) = std::fs::create_dir_all(&out) {
        eprintln!("{} cannot create {}: {error}", ui::red("error:"), out.display());
        return ExitCode::FAILURE;
    }
    if let Err(error) = korben_core::codegen::write_project(
        &crate_dir,
        &session.manifest.name,
        &session.manifest.version,
        release,
        &generated,
    ) {
        eprintln!("{} cannot write generated project: {error}", ui::red("error:"));
        return ExitCode::FAILURE;
    }

    // Bootstrapping strategy from specification 18.3: lower to Rust and hand
    // the result to an isolated Rust compilation pipeline.
    let mut command = std::process::Command::new("cargo");
    command.arg("build").current_dir(&crate_dir).env("RUSTFLAGS", "");
    if release {
        command.arg("--release");
    }
    // korben-8cg
    if let Some(triple) = &target {
        command.args(["--target", triple]);
    }
    let status = match command.status() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("{} cannot run `cargo`: {error}", ui::red("build failed:"));
            eprintln!(
                "  The native backend compiles generated Rust; install a Rust toolchain, or use"
            );
            eprintln!("  `korben run` and `korben build --emit rust`.");
            return ExitCode::FAILURE;
        }
    };
    if !status.success() {
        eprintln!("{} the generated crate did not compile", ui::red("build failed:"));
        eprintln!("  Inspect it at {}", crate_dir.display());
        // korben-8cg
        // Generated Rust that compiles for the host and not for a cross target
        // is the target's story -- a missing linker, or a platform the runtime
        // does not reach -- so do not send the user to report a compiler bug.
        match &target {
            Some(triple) => {
                eprintln!("  It compiles for the host, so `{triple}` is the difference:");
                eprintln!("  check that its linker is installed and that the runtime supports it.");
            }
            None => eprintln!("  and please report this as a compiler bug."),
        }
        return ExitCode::FAILURE;
    }

    // korben-8cg
    let artifact = artifact_name(&session.manifest.name, target.as_deref());
    let mut built = crate_dir.join("target");
    if let Some(triple) = &target {
        built = built.join(triple);
    }
    let built = built.join(profile).join(&artifact);
    let destination = out.join(&artifact);
    if let Err(error) = std::fs::copy(&built, &destination) {
        eprintln!("{} cannot copy the executable: {error}", ui::red("error:"));
        return ExitCode::FAILURE;
    }

    let size = std::fs::metadata(&destination).map(|meta| meta.len()).unwrap_or(0);
    // korben-8cg
    let described = match &target {
        Some(triple) => format!("{profile}, {triple}"),
        None => profile.to_string(),
    };
    println!(
        "{} {} ({described}, {})",
        ui::green("built"),
        destination.display(),
        human_size(size)
    );
    println!("  {}", ui::dim(&format!("generated crate: {}", crate_dir.display())));
    ExitCode::SUCCESS
}

// korben-8cg
/// Reject a target triple before cargo does, because cargo reports a missing
/// target as a missing `std` crate -- which says nothing about what to install.
fn verify_target(triple: &str) -> Result<(), ExitCode> {
    let Some(known) = tool_output("rustc", &["--print", "target-list"]) else {
        // No rustc to ask. The cargo invocation is about to say so itself.
        return Ok(());
    };
    if !known.lines().any(|line| line.trim() == triple) {
        eprintln!("{} `{triple}` is not a target rustc knows", ui::red("build failed:"));
        if let Some(near) = nearest_target(triple, &known) {
            eprintln!("  Did you mean `{near}`?");
        }
        eprintln!("  `rustc --print target-list` lists every target.");
        return Err(ExitCode::FAILURE);
    }
    // rustup owns installed standard libraries. Another Rust installation may
    // have the target without it, so its absence is not evidence of anything.
    let Some(installed) = tool_output("rustup", &["target", "list", "--installed"]) else {
        return Ok(());
    };
    if !installed.lines().any(|line| line.trim() == triple) {
        eprintln!(
            "{} the standard library for `{triple}` is not installed",
            ui::red("build failed:")
        );
        eprintln!("  Install it with `rustup target add {triple}`.");
        return Err(ExitCode::FAILURE);
    }
    Ok(())
}

// korben-8cg
/// Standard output of a toolchain query, or `None` when it cannot be asked.
fn tool_output(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

// korben-8cg
/// The closest real triple to a misspelled one. Triples are long enough that a
/// typo stays close, and different enough that a near miss is rarely wrong.
fn nearest_target(triple: &str, known: &str) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    for candidate in known.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let distance = korben_core::eval::edit_distance(triple, candidate);
        if distance <= 3 && best.map(|(best, _)| distance < best).unwrap_or(true) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate.to_string())
}

// korben-8cg
/// What cargo names the executable it produces for `triple`.
fn artifact_name(name: &str, triple: Option<&str>) -> String {
    match triple {
        Some(triple) if triple.contains("windows") => format!("{name}.exe"),
        Some(triple) if triple.starts_with("wasm") => format!("{name}.wasm"),
        Some(_) => name.to_string(),
        None if cfg!(windows) => format!("{name}.exe"),
        None => name.to_string(),
    }
}

fn report_all(diagnostics: &[Diagnostic], session: &Session) -> ExitCode {
    for diagnostic in diagnostics {
        eprint!("{}", diagnostic.render(&session.sources, ui::use_color()));
    }
    eprintln!("{} {} error(s)", ui::red("build failed:"), diagnostics.len());
    ExitCode::FAILURE
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

// -------------------------------------------------------------- dependencies

/// Locate the manifest of the project the command was run in.
fn manifest_path() -> Result<PathBuf, ExitCode> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match project::find_manifest(&cwd) {
        Some(path) => Ok(path),
        None => {
            eprintln!(
                "{} no {} found here or in any parent",
                ui::red("error:"),
                project::MANIFEST_NAME
            );
            Err(ExitCode::from(2))
        }
    }
}

fn cmd_add(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let Some(name) = flags.positional.first() else {
        eprintln!("{} `korben add` needs a package name", ui::red("error:"));
        eprintln!("  korben add <name> [--version <req>] [--path <dir>] [--dev]");
        return ExitCode::from(2);
    };
    let path = match manifest_path() {
        Ok(path) => path,
        Err(code) => return code,
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("{} cannot read {}", ui::red("error:"), path.display());
        return ExitCode::FAILURE;
    };
    let dev = flags.has("dev");
    let section = if dev { "dev-dependencies" } else { "dependencies" };
    let requirement = flags.value("version").unwrap_or("*").to_string();
    let directory = flags.value("path").map(str::to_string);

    let updated = match add_dependency(&text, section, name, &requirement, directory.as_deref()) {
        Ok(updated) => updated,
        Err(error) => {
            eprintln!("{} {error}", ui::red("error:"));
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = std::fs::write(&path, updated) {
        eprintln!("{} cannot write {}: {error}", ui::red("error:"), path.display());
        return ExitCode::FAILURE;
    }
    match directory {
        Some(directory) => println!("{} {name} (path {directory})", ui::green("added")),
        None => println!("{} {name} {requirement}", ui::green("added")),
    }
    relock(&flags)
}

fn cmd_remove(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let Some(name) = flags.positional.first() else {
        eprintln!("{} `korben remove` needs a package name", ui::red("error:"));
        return ExitCode::from(2);
    };
    let path = match manifest_path() {
        Ok(path) => path,
        Err(code) => return code,
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("{} cannot read {}", ui::red("error:"), path.display());
        return ExitCode::FAILURE;
    };
    let (updated, removed) = remove_dependency(&text, name);
    if !removed {
        eprintln!("{} `{name}` is not a declared dependency", ui::yellow("note:"));
        return ExitCode::SUCCESS;
    }
    if let Err(error) = std::fs::write(&path, updated) {
        eprintln!("{} cannot write {}: {error}", ui::red("error:"), path.display());
        return ExitCode::FAILURE;
    }
    println!("{} {name}", ui::green("removed"));
    relock(&flags)
}

fn cmd_update(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let path = match manifest_path() {
        Ok(path) => path,
        Err(code) => return code,
    };
    let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    // Removing the lock forces a fresh resolution.
    let _ = std::fs::remove_file(root.join(korben_core::pkg::LOCK_NAME));
    relock(&flags)
}

/// Re-resolve and report what the lockfile now pins.
fn relock(flags: &Flags) -> ExitCode {
    let session = match open_session(flags) {
        Ok(session) => session,
        Err(code) => return code,
    };
    if session.resolution.is_empty() {
        println!("{} no dependencies to lock", ui::dim("note:"));
        return ExitCode::SUCCESS;
    }
    println!("\n{}", ui::bold("locked"));
    for package in &session.resolution.packages {
        println!("  {} {}  {}", package.name, package.version, ui::dim(&package.source.identity()));
    }
    ExitCode::SUCCESS
}

fn cmd_audit(_args: &[String]) -> ExitCode {
    let path = match manifest_path() {
        Ok(path) => path,
        Err(code) => return code,
    };
    let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let manifest = match korben_core::manifest::Manifest::load(&path) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!("{} {error}", ui::red("error:"));
            return ExitCode::FAILURE;
        }
    };
    let lock_path = root.join(korben_core::pkg::LOCK_NAME);
    let declared = manifest.dependencies.len() + manifest.dev_dependencies.len();

    println!("{}", ui::bold("package"));
    println!("  name         {}", manifest.name);
    println!("  version      {}", manifest.version);
    let mut findings: Vec<String> = Vec::new();
    if manifest.license.is_none() {
        findings.push("no license is declared".to_string());
    }
    if manifest.description.is_none() {
        findings.push("no description is declared".to_string());
    }

    println!("\n{}", ui::bold("dependencies"));
    if declared == 0 {
        println!("  {}", ui::dim("none declared; there is nothing to lock"));
    } else if !lock_path.is_file() {
        println!("  {} no lockfile; run `korben update`", ui::red("missing:"));
        findings.push("the build is not reproducible without a lockfile".to_string());
    } else {
        match korben_core::pkg::Lockfile::load(&lock_path) {
            Ok(lock) => {
                if !lock.matches(&manifest) {
                    println!(
                        "  {} the lockfile does not describe this manifest",
                        ui::red("stale:")
                    );
                    findings.push("run `korben update` to re-resolve".to_string());
                }
                for locked in &lock.packages {
                    let resolved = korben_core::pkg::Lockfile {
                        root: lock.root.clone(),
                        manifest_digest: lock.manifest_digest.clone(),
                        packages: vec![locked.clone()],
                    }
                    .materialize(&root, &manifest);
                    match resolved {
                        Ok(_) => println!(
                            "  {} {} {}  {}",
                            ui::green("ok"),
                            locked.name,
                            locked.version,
                            ui::dim(&locked.checksum)
                        ),
                        Err(error) => {
                            println!("  {} {} {}", ui::red("FAIL"), locked.name, locked.version);
                            for line in error.lines() {
                                println!("       {line}");
                            }
                            findings.push(format!("`{}` failed verification", locked.name));
                        }
                    }
                    if locked.source.is_local() {
                        findings.push(format!(
                            "`{}` comes from a local path, so the lock is not portable",
                            locked.name
                        ));
                    }
                }
            }
            Err(error) => {
                println!("  {} {error}", ui::red("unreadable:"));
                findings.push("the lockfile could not be parsed".to_string());
            }
        }
    }

    println!("\n{}", ui::bold("supply chain"));
    println!("  {} install scripts are prohibited and never executed", ui::green("ok"));
    if std::env::var_os(korben_core::pkg::SKIP_CHECKSUMS).is_some() {
        println!(
            "  {} {} is set, so checksums are not verified",
            ui::red("weakened:"),
            korben_core::pkg::SKIP_CHECKSUMS
        );
        findings.push("checksum verification is disabled".to_string());
    } else {
        println!("  {} checksums are verified on every build", ui::green("ok"));
    }
    if !manifest.ffi_c.is_empty() {
        println!("  {} links native libraries: {}", ui::yellow("note:"), manifest.ffi_c.join(", "));
    }
    // korben-868
    // Specification 21.3 asks that weakened verification be reported, and a
    // registry nobody signs is exactly that.
    if let Some(url) = &manifest.registry_git {
        if manifest.registry_signed {
            println!("  {} the registry must carry a signature git verifies", ui::green("ok"));
        } else {
            println!(
                "  {} `{url}` is fetched without checking any signature",
                ui::yellow("weakened:")
            );
            println!("       Set `signed = true` under `[registry]` to require one.");
            findings.push("the registry's signature is not required".to_string());
        }
    }

    if findings.is_empty() {
        println!("\n{} nothing to report", ui::green("audit:"));
        return ExitCode::SUCCESS;
    }
    println!("\n{}", ui::bold("findings"));
    for finding in &findings {
        println!("  - {finding}");
    }
    ExitCode::SUCCESS
}

/// Insert a dependency line, creating the section if it is missing.
fn add_dependency(
    text: &str,
    section: &str,
    name: &str,
    requirement: &str,
    path: Option<&str>,
) -> Result<String, String> {
    let (without, _) = remove_dependency(text, name);
    let mut out = without.trim_end().to_string();
    match path {
        // A path dependency needs the long form.
        Some(path) => {
            out.push_str(&format!("\n\n[{section}.{name}]\n"));
            if requirement != "*" {
                out.push_str(&format!("version = \"{requirement}\"\n"));
            }
            out.push_str(&format!("path = \"{path}\"\n"));
        }
        None => {
            let header = format!("[{section}]");
            match out.find(&header) {
                Some(index) => {
                    let line_end = out[index..]
                        .find('\n')
                        .map(|offset| index + offset + 1)
                        .unwrap_or(out.len());
                    out.insert_str(line_end, &format!("{name} = \"{requirement}\"\n"));
                }
                None => {
                    out.push_str(&format!("\n\n{header}\n{name} = \"{requirement}\"\n"));
                }
            }
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// Drop every declaration of `name`, in either form.
fn remove_dependency(text: &str, name: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut removed = false;
    let mut skipping_section = false;
    let short = format!("{name} =");
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            skipping_section = trimmed == format!("[dependencies.{name}]")
                || trimmed == format!("[dev-dependencies.{name}]");
            if skipping_section {
                removed = true;
                continue;
            }
        }
        if skipping_section {
            continue;
        }
        if trimmed.starts_with(&short) || trimmed.starts_with(&format!("\"{name}\" =")) {
            removed = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    (out, removed)
}

// ---------------------------------------------------------------------- ffi

fn cmd_ffi(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    match flags.positional.first().map(String::as_str) {
        Some("c") => cmd_ffi_c(&flags),
        // korben-rm1
        Some("rust") => cmd_ffi_rust(&flags),
        Some(other) => {
            eprintln!("{} unknown `korben ffi` subcommand `{other}`", ui::red("error:"));
            eprintln!(
                "  Use `korben ffi` to list bindings, `korben ffi c <header>`, or \
                 `korben ffi rust <file.rs>`."
            );
            ExitCode::from(2)
        }
        None => cmd_ffi_list(&flags),
    }
}

/// Show every foreign declaration the project makes, and what it links.
fn cmd_ffi_list(flags: &Flags) -> ExitCode {
    let mut session = match open_session(flags) {
        Ok(session) => session,
        Err(code) => return code,
    };
    load_all(&mut session, flags);
    if ui::report(&session.diagnostics, &session.sources, false) {
        return ExitCode::FAILURE;
    }

    if !session.manifest.ffi_c.is_empty() {
        println!("{} {}", ui::bold("linked C libraries:"), session.manifest.ffi_c.join(", "));
    }
    if !session.manifest.ffi_rust.is_empty() {
        println!(
            "{} {}",
            ui::yellow("declared Rust adapters:"),
            session.manifest.ffi_rust.join(", ")
        );
        // korben-rm1
        println!("  {}", ui::dim("generate bindings with `korben ffi rust <file.rs>`"));
    }

    let mut total = 0usize;
    for module in &session.modules {
        let foreign: Vec<&korben_core::ast::ForeignDecl> = module
            .items
            .iter()
            .filter_map(|item| match item {
                korben_core::ast::Item::Foreign(decl) => Some(&**decl),
                _ => None,
            })
            .collect();
        if foreign.is_empty() {
            continue;
        }
        println!("\n{}", ui::bold(&module.name));
        for decl in foreign {
            total += 1;
            let visibility = if decl.is_public { "pub " } else { "" };
            println!(
                "  {visibility}{} {}",
                decl.name,
                ui::dim(&format!("-> {} [{}]", decl.c_ret, decl.library))
            );
            println!("      {}", ui::dim(&korben_core::docs::foreign_signature(decl)));
        }
    }
    if total == 0 {
        println!("{} this project declares no foreign functions", ui::dim("note:"));
    } else {
        println!("\n{} {total} foreign declaration{}", ui::green("total:"), ui::plural(total));
    }
    ExitCode::SUCCESS
}

/// Generate a binding module from a C header.
fn cmd_ffi_c(flags: &Flags) -> ExitCode {
    let Some(header) = flags.positional.get(1) else {
        eprintln!("{} `korben ffi c` needs a header file", ui::red("error:"));
        eprintln!("  korben ffi c <header.h> --library <name> [--module <name>] [--out <file>]");
        return ExitCode::from(2);
    };
    let path = PathBuf::from(header);
    let Ok(source) = std::fs::read_to_string(&path) else {
        eprintln!("{} cannot read {}", ui::red("error:"), path.display());
        return ExitCode::FAILURE;
    };
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "bindings".to_string());
    let library = flags.value("library").unwrap_or(&stem).to_string();
    let module = flags.value("module").unwrap_or(&stem).to_string();

    let extracted = korben_core::cheader::extract(&source);
    let rendered =
        korben_core::cheader::render(&module, &library, &path.display().to_string(), &extracted);

    match flags.value("out") {
        Some(out) => {
            if let Err(error) = std::fs::write(out, &rendered) {
                eprintln!("{} cannot write {out}: {error}", ui::red("error:"));
                return ExitCode::FAILURE;
            }
            println!(
                "{} {} binding{} to {out}",
                ui::green("generated"),
                extracted.bindings.len(),
                ui::plural(extracted.bindings.len())
            );
        }
        None => print!("{rendered}"),
    }
    if !extracted.skipped.is_empty() {
        eprintln!(
            "{} {} declaration{} could not be typed and were skipped",
            ui::yellow("note:"),
            extracted.skipped.len(),
            ui::plural(extracted.skipped.len())
        );
    }
    ExitCode::SUCCESS
}

// korben-rm1
/// Generate a binding module from a Rust adapter's source.
fn cmd_ffi_rust(flags: &Flags) -> ExitCode {
    let Some(source_path) = flags.positional.get(1) else {
        eprintln!("{} `korben ffi rust` needs a Rust source file", ui::red("error:"));
        eprintln!("  korben ffi rust <lib.rs> --library <name> [--module <name>] [--out <file>]");
        return ExitCode::from(2);
    };
    let path = PathBuf::from(source_path);
    let Ok(source) = std::fs::read_to_string(&path) else {
        eprintln!("{} cannot read {}", ui::red("error:"), path.display());
        return ExitCode::FAILURE;
    };
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "adapter".to_string());
    // A crate's entry point is `lib.rs`, whose name says nothing about the
    // library it builds, so ask rather than generate a module called `lib`.
    let default = if stem == "lib" || stem == "mod" { "adapter" } else { &stem };
    let library = flags.value("library").unwrap_or(default).to_string();
    let module = flags.value("module").unwrap_or(default).to_string();

    let extracted = korben_adapter::extract(&source);
    if extracted.exported.is_empty() && extracted.skipped.is_empty() {
        eprintln!("{} {} has no `#[korben_export]` functions", ui::yellow("note:"), path.display());
        return ExitCode::SUCCESS;
    }
    let rendered =
        korben_adapter::korben::render(&module, &library, &path.display().to_string(), &extracted);
    // Generated source lands in someone's project, where `korben fmt --check`
    // runs over it, so it leaves here in canonical form. Formatting it rather
    // than hand-tuning the rendering keeps it canonical if the formatter
    // changes; a file the reader cannot parse comes back unchanged, and the
    // check below is what would catch that.
    let mut sources = korben_syntax::SourceMap::new();
    let file = sources.add_file(&path, rendered.clone());
    let (rendered, errors) = korben_syntax::fmt::format_source(file, &rendered);
    if errors.iter().any(korben_syntax::Diagnostic::is_error) {
        eprintln!("{} the generated module does not parse", ui::red("error:"));
        eprintln!("  Please report this as a compiler bug.");
        return ExitCode::FAILURE;
    }

    match flags.value("out") {
        Some(out) => {
            if let Err(error) = std::fs::write(out, &rendered) {
                eprintln!("{} cannot write {out}: {error}", ui::red("error:"));
                return ExitCode::FAILURE;
            }
            println!(
                "{} {} binding{} to {out}",
                ui::green("generated"),
                extracted.exported.len(),
                ui::plural(extracted.exported.len())
            );
        }
        None => print!("{rendered}"),
    }
    if !extracted.skipped.is_empty() {
        eprintln!(
            "{} skipped {} function{} that could not cross the adapter boundary",
            ui::yellow("note:"),
            extracted.skipped.len(),
            ui::plural(extracted.skipped.len())
        );
    }
    ExitCode::SUCCESS
}

// korben-blf
/// Copy this package into a registry, as `<registry>/<name>/<version>/`.
///
/// This refuses more than it accepts, because everything it writes is something
/// another project will pin.
fn cmd_publish(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let mut session = match open_session(&flags) {
        Ok(session) => session,
        Err(code) => return code,
    };

    // A package that does not check is not one to hand to anybody else.
    load_all(&mut session, &flags);
    korben_core::infer::check_session(&mut session, flags.has("strict-api"));
    if ui::report(&session.diagnostics, &session.sources, false) {
        eprintln!("{} {}", ui::red("publish failed:"), ui::summarize(&session.diagnostics));
        return ExitCode::FAILURE;
    }

    // A path dependency names a directory on this machine. Nobody installing
    // the package will have it, so it could never be built from what is
    // published.
    let local: Vec<String> = session
        .manifest
        .dependencies
        .iter()
        .chain(session.manifest.dev_dependencies.iter())
        .filter(|dependency| dependency.path.is_some())
        .map(|dependency| dependency.name.clone())
        .collect();
    if !local.is_empty() {
        eprintln!(
            "{} `{}` depends on a path: {}",
            ui::red("publish failed:"),
            session.manifest.name,
            local.join(", ")
        );
        eprintln!("  A path names a directory on this machine, which nobody installing");
        eprintln!("  this package will have. Publish those packages and depend on their");
        eprintln!("  versions instead.");
        return ExitCode::FAILURE;
    }

    let Some(registry) = flags
        .value("registry")
        .map(PathBuf::from)
        .or_else(|| korben_core::pkg::registry_root(&session.manifest))
    else {
        eprintln!("{} no registry to publish into", ui::red("publish failed:"));
        eprintln!("  Pass `--registry <dir>`, set `registry` in the manifest, or set HOME.");
        return ExitCode::FAILURE;
    };

    let destination = registry.join(&session.manifest.name).join(&session.manifest.version);
    // A published version is what a lockfile pins. If it could be replaced, a
    // build that reproduced yesterday would stop reproducing today.
    if destination.exists() {
        eprintln!(
            "{} `{}` {} is already published",
            ui::red("publish failed:"),
            session.manifest.name,
            session.manifest.version
        );
        eprintln!("  At {}", destination.display());
        eprintln!("  A published version never changes, because lockfiles pin it.");
        eprintln!("  Raise the version in `korben.toml` and publish that.");
        return ExitCode::FAILURE;
    }

    let sources = korben_core::project::source_files(&session.root.join("src"));
    if sources.is_empty() {
        eprintln!(
            "{} `{}` has no sources in src/",
            ui::red("publish failed:"),
            session.manifest.name
        );
        return ExitCode::FAILURE;
    }

    if let Err(error) = copy_package(&session.root, &destination, &sources) {
        eprintln!("{} cannot write {}: {error}", ui::red("publish failed:"), destination.display());
        // Half a package in a registry is worse than none: resolution would
        // find it and try to build it.
        let _ = std::fs::remove_dir_all(&destination);
        return ExitCode::FAILURE;
    }

    let checksum = korben_core::pkg::package_checksum(&destination);
    println!(
        "{} {} {} to {}",
        ui::green("published"),
        session.manifest.name,
        session.manifest.version,
        destination.display()
    );
    println!("  {} {checksum}", ui::dim("checksum"));
    println!(
        "  {} {} source file{}",
        ui::dim("contents"),
        sources.len(),
        ui::plural(sources.len())
    );
    ExitCode::SUCCESS
}

// korben-poj
/// Clone or update the git registry this project reads from.
///
/// This is the only command that touches a network. Resolution reads the clone
/// and nothing else, so a build is offline whether or not the clone is current
/// -- which is what specification 21 means by an offline build.
fn cmd_install(args: &[String]) -> ExitCode {
    let _ = Flags::parse(args);
    let manifest_path = match manifest_path() {
        Ok(path) => path,
        Err(code) => return code,
    };
    let manifest = match korben_core::manifest::Manifest::load(&manifest_path) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!("{} {error}", ui::red("error:"));
            return ExitCode::FAILURE;
        }
    };
    let Some(url) = manifest.registry_git.clone() else {
        eprintln!("{} this project has no git registry", ui::red("error:"));
        eprintln!("  Add one to `korben.toml`:");
        eprintln!("    [registry]");
        eprintln!("    git = \"https://example.test/registry.git\"");
        eprintln!("  A registry is a repository laid out as <name>/<version>/.");
        return ExitCode::from(2);
    };
    let Some(cache) = korben_core::pkg::git_cache(&url) else {
        eprintln!("{} no HOME to cache the registry under", ui::red("error:"));
        return ExitCode::FAILURE;
    };

    if git(&["--version"], None).is_none() {
        eprintln!("{} `git` is not on PATH", ui::red("install failed:"));
        eprintln!("  A git registry is fetched with git, the way the native backend");
        eprintln!("  compiles with cargo. Install git, or use `[registry] path`.");
        return ExitCode::FAILURE;
    }

    let existing = cache.join(".git").is_dir();
    let outcome = if existing {
        git(&["pull", "--ff-only"], Some(&cache))
    } else {
        if let Some(parent) = cache.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!("{} cannot create {}: {error}", ui::red("error:"), parent.display());
                return ExitCode::FAILURE;
            }
        }
        git(&["clone", "--quiet", &url, &cache.display().to_string()], None)
    };

    match outcome {
        Some(_) => {
            // korben-868
            // Provenance comes from the repository's own signature, checked by
            // git. Specification 21.3 wants verifiable signed artifacts, 22.4
            // warns against custom cryptographic constructions, and the
            // toolchain takes no dependencies -- so the verification is
            // delegated to the tool already doing the fetching.
            match signature(&cache) {
                Signature::Good(signer) => {
                    println!("{} {url}", ui::green("verified"));
                    println!("  {} {signer}", ui::dim("signed by"));
                }
                other if manifest.registry_signed => {
                    eprintln!(
                        "{} the registry's latest commit {}",
                        ui::red("install failed:"),
                        other.describe()
                    );
                    eprintln!("  `[registry] signed = true` asks for a signature git can verify.");
                    eprintln!("  The clone has been removed rather than left unverified.");
                    let _ = std::fs::remove_dir_all(&cache);
                    return ExitCode::FAILURE;
                }
                other => {
                    let verb = if existing { "updated" } else { "cloned" };
                    println!("{} {url}", ui::green(verb));
                    println!("  {} {}", ui::yellow("unverified:"), other.describe());
                }
            }
            println!("  {} {}", ui::dim("at"), cache.display());
            let packages = korben_core::pkg::registry_names(&cache);
            println!(
                "  {} {} package{}",
                ui::dim("offering"),
                packages.len(),
                ui::plural(packages.len())
            );
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("{} git could not fetch {url}", ui::red("install failed:"));
            if existing {
                eprintln!("  The clone at {} could not be fast-forwarded.", cache.display());
                eprintln!("  Delete it to start again.");
            }
            ExitCode::FAILURE
        }
    }
}

// korben-868
/// What git makes of the signature on a registry's latest commit.
enum Signature {
    Good(String),
    Untrusted(String),
    Bad,
    Missing,
    None,
}

impl Signature {
    fn describe(&self) -> String {
        match self {
            Signature::Good(signer) => format!("is signed by {signer}"),
            Signature::Untrusted(signer) => {
                format!("is signed by {signer}, whose key is not trusted here")
            }
            Signature::Bad => "carries a signature that does not verify".to_string(),
            Signature::Missing => "is signed by a key this machine does not have".to_string(),
            Signature::None => "carries no signature".to_string(),
        }
    }
}

// korben-868
/// Ask git about the signature on the latest commit of a clone.
fn signature(cache: &Path) -> Signature {
    let Some(output) = git(&["log", "-1", "--format=%G?%n%GS"], Some(cache)) else {
        return Signature::None;
    };
    let mut lines = output.lines();
    let status = lines.next().unwrap_or("N").trim().to_string();
    let signer = lines.next().unwrap_or("").trim().to_string();
    let signer = if signer.is_empty() { "an unnamed key".to_string() } else { signer };
    match status.as_str() {
        "G" => Signature::Good(signer),
        "U" => Signature::Untrusted(signer),
        "B" => Signature::Bad,
        "X" | "Y" | "R" | "E" => Signature::Missing,
        _ => Signature::None,
    }
}

// korben-poj
/// Run git, returning its output when it succeeded.
fn git(args: &[&str], directory: Option<&Path>) -> Option<String> {
    let mut command = std::process::Command::new("git");
    command.args(args);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let output = command.output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

// korben-blf
/// Copy the manifest and every source file into a published package.
fn copy_package(root: &Path, destination: &Path, sources: &[PathBuf]) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    std::fs::copy(
        root.join(korben_core::project::MANIFEST_NAME),
        destination.join(korben_core::project::MANIFEST_NAME),
    )?;
    for source in sources {
        let relative = source.strip_prefix(root).unwrap_or(source);
        let target = destination.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(source, &target)?;
    }
    Ok(())
}

// ----------------------------------------------------------- doctor/inspect

fn cmd_doctor(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    println!("{}", ui::bold("toolchain"));
    println!("  korben       {VERSION}");
    println!(
        "  executable   {}",
        std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    );
    println!("  platform     {} {}", std::env::consts::OS, std::env::consts::ARCH);

    let cwd = std::env::current_dir().unwrap_or_default();
    match project::find_manifest(&cwd) {
        Some(manifest_path) => {
            println!("\n{}", ui::bold("project"));
            println!("  manifest     {}", manifest_path.display());
            match korben_core::manifest::Manifest::load(&manifest_path) {
                Ok(manifest) => {
                    println!("  name         {}", manifest.name);
                    println!("  version      {}", manifest.version);
                    println!("  edition      {}", manifest.edition);
                    println!("  entry        {}", manifest.main);
                    if manifest.edition != "2026" {
                        println!(
                            "  {} edition `{}` is not supported by this toolchain",
                            ui::yellow("warning:"),
                            manifest.edition
                        );
                    }
                    let lock =
                        manifest_path.parent().map(|root| root.join(korben_core::pkg::LOCK_NAME));
                    let declared = manifest.dependencies.len() + manifest.dev_dependencies.len();
                    println!("  dependencies {declared}");
                    match lock {
                        Some(lock) if lock.is_file() => {
                            println!("  lockfile     {}", lock.display())
                        }
                        _ if declared > 0 => println!(
                            "  {} no lockfile; run `korben update`",
                            ui::yellow("lockfile:")
                        ),
                        _ => {}
                    }
                    if std::env::var_os(korben_core::pkg::SKIP_CHECKSUMS).is_some() {
                        println!(
                            "  {} {} is set; dependency checksums are not verified",
                            ui::red("warning:"),
                            korben_core::pkg::SKIP_CHECKSUMS
                        );
                    }
                    if !manifest.ffi_c.is_empty() {
                        println!("  links        {}", manifest.ffi_c.join(", "));
                    }
                }
                Err(error) => println!("  {} {error}", ui::red("error:")),
            }
            let mut session = match open_session(&flags) {
                Ok(session) => session,
                Err(code) => return code,
            };
            load_all(&mut session, &flags);
            println!("  modules      {}", session.modules.len());
            println!("  tests        {}", session.interp.tests.borrow().len());
            println!("\n{}", ui::bold("health"));
            println!("  {}", ui::summarize(&session.diagnostics));
        }
        None => {
            println!("\n{} no project found in this directory", ui::yellow("note:"));
            println!("  Run `korben new <name>` or `korben init`.");
        }
    }
    ExitCode::SUCCESS
}

fn cmd_inspect(args: &[String]) -> ExitCode {
    let flags = Flags::parse(args);
    let mut session = match open_session(&flags) {
        Ok(session) => session,
        Err(code) => return code,
    };
    load_all(&mut session, &flags);
    if ui::report(&session.diagnostics, &session.sources, false) {
        return ExitCode::FAILURE;
    }
    // korben-mic
    if let Some(workspace) = &session.workspace {
        println!("{}", ui::bold("workspace"));
        println!("  root         {}", workspace.root.display());
        for member in &workspace.members {
            let kind = if member.has_program() { "program" } else { "library" };
            println!("  {} {} ({kind})", ui::green("member"), member.name);
        }
        println!();
    }
    for module in &session.modules {
        println!("{}", ui::bold(&module.name));
        for import in &module.imports {
            let detail = match &import.names {
                Some(names) => format!(" [{}]", names.join(" ")),
                None => format!(" :as {}", import.alias),
            };
            println!("  use {}{detail}", import.path);
        }
        for item in &module.items {
            let kind = match item {
                korben_core::ast::Item::Fn(decl) => {
                    if decl.is_async {
                        "async fn"
                    } else {
                        "fn"
                    }
                }
                korben_core::ast::Item::Type(_) => "type",
                korben_core::ast::Item::Protocol(_) => "protocol",
                korben_core::ast::Item::Impl(_) => "impl",
                korben_core::ast::Item::Macro(_) => "macro",
                korben_core::ast::Item::Test(_) => "test",
                korben_core::ast::Item::Derive(_) => "derive",
                korben_core::ast::Item::Foreign(_) => "ffi/c-fn",
                korben_core::ast::Item::Const { .. } => "def",
            };
            let visibility = match item {
                korben_core::ast::Item::Test(_)
                | korben_core::ast::Item::Impl(_)
                | korben_core::ast::Item::Derive(_) => "",
                item if item.is_public() => "pub ",
                _ => "",
            };
            println!("  {visibility}{kind} {}", item.name());
        }
        println!();
    }
    ExitCode::SUCCESS
}

// ------------------------------------------------------------------ helpers

pub fn open_session(flags: &Flags) -> Result<Session, ExitCode> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // A file argument lets commands work outside a project.
    let standalone =
        flags.positional.first().map(|path| Path::new(path).is_file()).unwrap_or(false);
    match Session::open(&cwd) {
        Ok(session) => {
            session.interp.max_depth.set(MAX_CALL_DEPTH);
            Ok(session)
        }
        Err(error) => {
            if standalone {
                let session = Session::bare(cwd);
                session.interp.max_depth.set(MAX_CALL_DEPTH);
                return Ok(session);
            }
            eprintln!("{} {error}", ui::red("error:"));
            // Only suggest creating a project when there genuinely is none;
            // a failure inside an existing project needs a different fix.
            if project::find_manifest(&cwd).is_none() {
                eprintln!("  Run `korben new <name>` to create a project, or pass a `.kb` file.");
            }
            Err(ExitCode::from(2))
        }
    }
}

/// Load every module reachable from `src/` plus every test file.
pub fn load_all(session: &mut Session, flags: &Flags) {
    if let Some(path) = flags.positional.first().map(PathBuf::from) {
        if path.is_file() {
            let _ = session.load_file(&path, None);
            return;
        }
    }
    // korben-mic
    // In a workspace every member is loaded, so `check`, `test`, and `lint` at
    // the root say something about the whole repository rather than about
    // whichever member the caller happened to be standing in.
    let roots: Vec<PathBuf> = match &session.workspace {
        Some(workspace) => workspace.members.iter().map(|member| member.root.clone()).collect(),
        None => vec![session.root.clone()],
    };
    for root in roots {
        let src = root.join("src");
        for path in project::source_files(&src) {
            let name = module_name_for(&src, &path);
            let _ = session.load_module(&name, Span::synthetic());
        }
        for path in project::source_files(&root.join("tests")) {
            let _ = session.load_file(&path, None);
        }
    }
}

fn module_name_for(src: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(src).unwrap_or(path);
    let mut parts: Vec<String> =
        relative.components().map(|part| part.as_os_str().to_string_lossy().to_string()).collect();
    if let Some(last) = parts.last_mut() {
        let trimmed = last.trim_end_matches(".kb").to_string();
        *last = trimmed;
    }
    if parts.last().map(|part| part == "mod").unwrap_or(false) {
        parts.pop();
    }
    parts.join(".")
}

/// Minimal flag parsing: `--name value`, `--name=value`, `--flag`, and positionals.
pub struct Flags {
    pub positional: Vec<String>,
    pub options: Vec<(String, Option<String>)>,
    /// Everything after a bare `--`, passed through to the program.
    pub passthrough: Vec<String>,
}

impl Flags {
    pub fn parse(args: &[String]) -> Flags {
        let mut positional = Vec::new();
        let mut options = Vec::new();
        let mut passthrough = Vec::new();
        let mut index = 0usize;
        while index < args.len() {
            let arg = &args[index];
            if arg == "--" {
                passthrough.extend(args[index + 1..].iter().cloned());
                break;
            }
            if let Some(rest) = arg.strip_prefix("--") {
                match rest.split_once('=') {
                    Some((name, value)) => {
                        options.push((name.to_string(), Some(value.to_string())))
                    }
                    None => {
                        // A following non-flag token is this option's value.
                        let value =
                            args.get(index + 1).filter(|next| !next.starts_with('-')).cloned();
                        if value.is_some() && takes_value(rest) {
                            index += 1;
                            options.push((rest.to_string(), value));
                        } else {
                            options.push((rest.to_string(), None));
                        }
                    }
                }
                index += 1;
                continue;
            }
            positional.push(arg.clone());
            index += 1;
        }
        Flags { positional, options, passthrough }
    }

    pub fn has(&self, name: &str) -> bool {
        self.options.iter().any(|(option, _)| option == name)
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|(option, _)| option == name)
            .and_then(|(_, value)| value.as_deref())
    }
}

fn takes_value(name: &str) -> bool {
    matches!(
        name,
        "template"
            | "out"
            | "target"
            | "emit"
            | "filter"
            | "library"
            | "module"
            | "path"
            | "version"
            | "package"
            | "registry"
    )
}
