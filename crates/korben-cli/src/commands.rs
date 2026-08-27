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
        "doctor" => cmd_doctor(rest),
        "inspect" => cmd_inspect(rest),
        "dev" => cmd_dev(rest),
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
        "add" | "remove" | "update" | "publish" | "install" | "audit" => {
            "Milestone D (registry and package management)"
        }
        "bench" => "Milestone D (benchmark harness)",
        "lsp" => "Milestone B (language server)",
        "ffi" => "Milestone C (C and Rust binding generation)",
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
    println!("{}", ui::bold("DEVELOP"));
    println!("  run [entry] [-- args...]                  run the project or a single file");
    println!("  dev                                       check, test, then run");
    println!("  check [--json] [--strict-api]             type, effect, and ownership analysis");
    println!("  test [filter] [--json]                    run tests and property tests");
    println!("  fmt [--check] [paths...]                  canonical formatting");
    println!("  lint [--json]                             built-in lint rules");
    println!("  repl                                      project-aware interactive session");
    println!("  expand <file>                             show macro expansion");
    println!("  doc [--out <dir>]                         generate documentation");
    println!("  inspect                                   show the resolved project model");
    println!("  build [--release] [--emit ir|rust]        produce a runnable artifact\n");
    println!("{}", ui::bold("OTHER"));
    println!("  version                                   print the toolchain version");
    println!("  help                                      print this message");
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

    /// A request handler: records, an error enum, `Result`, and map patterns.
    pub const SERVICE_MAIN: &str = r#"(module main
  (use std.json :as json)
  (use std.log :as log))

(pub type Request
  {method: Keyword path: String})

(pub enum AppError
  (BadRequest message: String)
  (NotFound path: String))

;;; Route a request to a JSON response body.
(pub fn handle [request: Request] -> Result String AppError
  (match request
    {:method :get :path "/health"} (Ok "ok")

    {:method :get :path "/version"} (Ok (json.encode {version "0.1.0"}))

    {:path path} (Err (NotFound path))))

(pub fn main [] -> Unit !io
  (log.info "starting" {port 3000})
  (match (handle (Request {method :get path "/health"}))
    (Ok body) (println body)
    (Err error) (println "error:" error)))
"#;

    pub const SERVICE_TEST: &str = r#"(module main_test
  (use main [handle Request]))

(test "health endpoint is available"
  (assert-eq (Ok "ok") (handle (Request {method :get path "/health"}))))

(test "unknown paths are not found"
  (match (handle (Request {method :get path "/nope"}))
    (Err (NotFound path)) (assert-eq "/nope" path)
    _ (assert false "expected a NotFound error")))
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
        None => {
            let main = session.manifest.main.clone();
            session.load_module(&main, Span::synthetic())
        }
    };

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

    session.interp.current = runtime;
    korben_runtime::std::set_program_args(flags.passthrough.clone());
    match session.interp.apply(main, Vec::new(), Span::synthetic()) {
        Ok(value) => finish_main(&value),
        Err(flow) => {
            let diagnostic = project::flow_diagnostic(flow, Span::synthetic());
            eprint!("{}", diagnostic.render(&session.sources, ui::use_color()));
            ExitCode::FAILURE
        }
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

    if session.diagnostics.has_errors() {
        ui::report(&session.diagnostics, &session.sources, json);
        eprintln!("{} {}", ui::red("test failed:"), ui::summarize(&session.diagnostics));
        return ExitCode::FAILURE;
    }

    let tests = std::mem::take(&mut session.interp.tests);
    let mut passed = 0usize;
    let mut failures: Vec<(String, String, Diagnostic)> = Vec::new();
    let mut results = Vec::new();

    for (module_name, name, decl, runtime) in tests {
        if let Some(filter) = &filter {
            if !name.contains(filter.as_str()) && !module_name.contains(filter.as_str()) {
                continue;
            }
        }
        session.interp.current = runtime;
        session.interp.out = Output::Captured(String::new());
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
                        let sample = match sample_from(&mut session.interp, value, decl.span) {
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

        let captured = match std::mem::replace(&mut session.interp.out, Output::Stdout) {
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
fn sample_from(interp: &mut Interp, generator: Value, span: Span) -> Result<Value, Flow> {
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
    let expanded =
        korben_core::expand::expand_module(&mut session.interp, &forms, &mut diagnostics);
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
    let mut session = match open_session(&flags) {
        Ok(session) => session,
        Err(code) => return code,
    };
    load_all(&mut session, &flags);
    korben_core::infer::check_session(&mut session, release);
    if ui::report(&session.diagnostics, &session.sources, false) {
        eprintln!("{} {}", ui::red("build failed:"), ui::summarize(&session.diagnostics));
        return ExitCode::FAILURE;
    }

    // Lowering to core IR resolves every name, so it also catches references
    // that the interpreter would only have failed on at run time.
    let entry = session.manifest.main.clone();
    let program = match korben_core::ir::lower_session(&session, &entry) {
        Ok(program) => program,
        Err(diagnostics) => {
            for diagnostic in &diagnostics {
                eprint!("{}", diagnostic.render(&session.sources, ui::use_color()));
            }
            eprintln!("{} {} error(s)", ui::red("build failed:"), diagnostics.len());
            return ExitCode::FAILURE;
        }
    };

    if emit == "ir" {
        print!("{}", korben_core::ir::render(&program));
        return ExitCode::SUCCESS;
    }

    let profile = if release { "release" } else { "debug" };
    let out = session.root.join("target").join(profile);
    if let Err(error) = std::fs::create_dir_all(&out) {
        eprintln!("{} cannot create {}: {error}", ui::red("error:"), out.display());
        return ExitCode::FAILURE;
    }
    let bundle = out.join(format!("{}.kbx", session.manifest.name));
    let contents = korben_core::bundle::write_bundle(&session);
    if let Err(error) = std::fs::write(&bundle, contents) {
        eprintln!("{} cannot write {}: {error}", ui::red("error:"), bundle.display());
        return ExitCode::FAILURE;
    }
    let launcher = out.join(&session.manifest.name);
    let script = format!(
        "#!/bin/sh\n# Generated by korben {VERSION}\nexec korben run \"$(dirname \"$0\")/{}.kbx\" \"$@\"\n",
        session.manifest.name
    );
    let _ = std::fs::write(&launcher, script);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755));
    }

    println!("{} {} ({profile})", ui::green("built"), bundle.display());
    println!("  {}", ui::dim(&format!("launcher: {}", launcher.display())));
    println!(
        "  {}",
        ui::dim(
            "the launcher requires `korben` on PATH; native code generation lands in Milestone C"
        )
    );
    ExitCode::SUCCESS
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
                    if !manifest.dependencies.is_empty() {
                        println!(
                            "  {} {} dependencies declared; the registry lands in Milestone D",
                            ui::yellow("note:"),
                            manifest.dependencies.len()
                        );
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
            println!("  tests        {}", session.interp.tests.len());
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
        Ok(mut session) => {
            session.interp.max_depth = MAX_CALL_DEPTH;
            Ok(session)
        }
        Err(error) => {
            if standalone {
                let mut session = Session::bare(cwd);
                session.interp.max_depth = MAX_CALL_DEPTH;
                return Ok(session);
            }
            eprintln!("{} {error}", ui::red("error:"));
            eprintln!("  Run `korben new <name>` to create a project, or pass a `.kb` file.");
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
    let src = session.src_dir();
    for path in project::source_files(&src) {
        let name = module_name_for(&src, &path);
        let _ = session.load_module(&name, Span::synthetic());
    }
    for path in project::source_files(&session.root.join("tests")) {
        let _ = session.load_file(&path, None);
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
    matches!(name, "template" | "out" | "target" | "emit" | "filter")
}
