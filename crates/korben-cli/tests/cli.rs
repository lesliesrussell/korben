//! End-to-end tests that drive the `korben` executable.

// korben-6bc

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EXE: &str = env!("CARGO_BIN_EXE_korben");

/// A scratch directory that cleans itself up.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let unique =
            format!("korben-test-{label}-{}-{:?}", std::process::id(), std::thread::current().id());
        let path = std::env::temp_dir().join(unique.replace(['(', ')', ' '], ""));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch directory");
        Scratch(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn korben(dir: &Path, args: &[&str]) -> Output {
    Command::new(EXE).args(args).current_dir(dir).env("NO_COLOR", "1").output().expect("run korben")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn combined(output: &Output) -> String {
    format!("{}{}", stdout(output), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn version_and_help_are_available() {
    let scratch = Scratch::new("help");
    let version = korben(scratch.path(), &["version"]);
    assert!(version.status.success());
    assert!(stdout(&version).starts_with("korben "));

    let help = korben(scratch.path(), &["help"]);
    assert!(help.status.success());
    let text = stdout(&help);
    for command in ["new", "run", "check", "test", "fmt", "repl", "expand", "doc"] {
        assert!(text.contains(command), "help is missing `{command}`:\n{text}");
    }
}

#[test]
fn unknown_commands_fail_with_guidance() {
    let scratch = Scratch::new("unknown");
    let output = korben(scratch.path(), &["frobnicate"]);
    assert!(!output.status.success());
    assert!(combined(&output).contains("unknown command"));
}

#[test]
fn planned_commands_say_which_milestone_they_land_in() {
    let scratch = Scratch::new("planned");
    let output = korben(scratch.path(), &["lsp"]);
    assert!(!output.status.success());
    assert!(combined(&output).contains("Milestone"));
}

#[test]
fn commands_outside_a_project_explain_what_to_do() {
    let scratch = Scratch::new("noproject");
    let output = korben(scratch.path(), &["check"]);
    assert!(!output.status.success());
    let text = combined(&output);
    assert!(text.contains("korben.toml"), "{text}");
    assert!(text.contains("korben new"), "{text}");
}

/// The whole documented workflow: new, check, test, fmt, run, doc, build.
#[test]
fn the_standard_workflow_works_for_every_template() {
    for template in ["cli", "lib", "service"] {
        let scratch = Scratch::new(&format!("workflow-{template}"));
        let created = korben(scratch.path(), &["new", "app", "--template", template]);
        assert!(created.status.success(), "new failed:\n{}", combined(&created));
        let project = scratch.path().join("app");
        assert!(project.join("korben.toml").is_file());
        assert!(project.join("src/main.kb").is_file());

        let check = korben(&project, &["check"]);
        assert!(check.status.success(), "{template} check failed:\n{}", combined(&check));

        let test = korben(&project, &["test"]);
        assert!(test.status.success(), "{template} test failed:\n{}", combined(&test));
        assert!(stdout(&test).contains("passed"));

        // A freshly generated project is already canonically formatted.
        let fmt = korben(&project, &["fmt", "--check"]);
        assert!(fmt.status.success(), "{template} is not formatted:\n{}", combined(&fmt));

        let run = korben(&project, &["run"]);
        assert!(run.status.success(), "{template} run failed:\n{}", combined(&run));
        assert!(!stdout(&run).is_empty());

        let doc = korben(&project, &["doc"]);
        assert!(doc.status.success(), "{template} doc failed:\n{}", combined(&doc));
        assert!(project.join("target/doc/main.md").is_file());
        assert!(project.join("target/doc/api.json").is_file());

        // Native code generation is covered in depth by the differential
        // tests; here we only check the artifact appears where documented.
        let build = korben(&project, &["build", "--emit", "rust"]);
        assert!(build.status.success(), "{template} codegen failed:\n{}", combined(&build));
        assert!(stdout(&build).contains("fn main()"));
    }
}

#[test]
fn check_reports_type_errors_and_exits_non_zero() {
    let scratch = Scratch::new("typeerror");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    std::fs::write(
        project.join("src/main.kb"),
        "(fn add [a: Int b: Int] -> Int (+ a b))\n(pub fn main [] -> Unit (add 1 \"two\"))\n",
    )
    .unwrap();
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();

    let output = korben(&project, &["check"]);
    assert!(!output.status.success());
    let text = combined(&output);
    assert!(text.contains("type-mismatch"), "{text}");
    assert!(text.contains("expected `Int`, found `String`"), "{text}");
}

#[test]
fn check_emits_machine_readable_json() {
    let scratch = Scratch::new("json");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    std::fs::write(project.join("src/main.kb"), "(pub fn main [] -> Int \"text\")\n").unwrap();
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();

    let output = korben(&project, &["check", "--json"]);
    assert!(!output.status.success());
    let text = stdout(&output);
    assert!(text.starts_with("{\"diagnostics\":["), "{text}");
    assert!(text.contains("\"code\":\"type-mismatch\""), "{text}");
    assert!(text.contains("\"line\":"), "{text}");
}

#[test]
fn test_failures_are_reported_and_exit_non_zero() {
    let scratch = Scratch::new("failingtest");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    std::fs::write(
        project.join("tests/main_test.kb"),
        "(module main_test)\n\n(test \"fails\"\n  (assert-eq 1 2))\n",
    )
    .unwrap();

    let output = korben(&project, &["test"]);
    assert!(!output.status.success());
    let text = combined(&output);
    assert!(text.contains("FAIL"), "{text}");
    assert!(text.contains("expected: 1"), "{text}");
}

#[test]
fn fmt_rewrites_files_and_check_mode_reports_without_writing() {
    let scratch = Scratch::new("fmt");
    let path = scratch.path().join("messy.kb");
    std::fs::write(&path, "(fn   add [a b]    (+   a   b))").unwrap();

    let check = korben(scratch.path(), &["fmt", "--check", "messy.kb"]);
    assert!(!check.status.success());
    assert!(combined(&check).contains("needs formatting"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "(fn   add [a b]    (+   a   b))");

    let write = korben(scratch.path(), &["fmt", "messy.kb"]);
    assert!(write.status.success(), "{}", combined(&write));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "(fn add [a b]\n  (+ a b))\n");

    let again = korben(scratch.path(), &["fmt", "--check", "messy.kb"]);
    assert!(again.status.success(), "{}", combined(&again));
}

#[test]
fn run_accepts_a_single_file_and_program_arguments() {
    let scratch = Scratch::new("script");
    let path = scratch.path().join("script.kb");
    std::fs::write(
        &path,
        r#"(module script (use std.process :as process))

(pub fn main [] -> Unit !io
  (println (process.args)))
"#,
    )
    .unwrap();
    let output = korben(scratch.path(), &["run", "script.kb", "--", "one", "two"]);
    assert!(output.status.success(), "{}", combined(&output));
    assert_eq!(stdout(&output), "[\"one\" \"two\"]\n");
}

#[test]
fn expand_shows_macro_output_through_the_formatter() {
    let scratch = Scratch::new("expand");
    let path = scratch.path().join("m.kb");
    std::fs::write(&path, "(fn f [x] (when (> x 0) x))\n").unwrap();
    let output = korben(scratch.path(), &["expand", "m.kb"]);
    assert!(output.status.success(), "{}", combined(&output));
    assert_eq!(stdout(&output), "(fn f [x]\n  (if (> x 0) (do x) nil))\n");
}

#[test]
fn the_repl_evaluates_and_reports_types() {
    let scratch = Scratch::new("repl");
    let output = Command::new(EXE)
        .arg("repl")
        .current_dir(scratch.path())
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.as_mut().unwrap().write_all(
                b"(+ 1 2)\n:type (map [1 2] inc)\n(fn double [n] (* n 2))\n(double 21)\n:quit\n",
            )?;
            child.wait_with_output()
        })
        .expect("run repl");
    let text = stdout(&output);
    assert!(text.contains("3 : Int"), "{text}");
    assert!(text.contains("Vec Int"), "{text}");
    assert!(text.contains("42"), "{text}");
}

#[test]
fn inspect_lists_the_resolved_project_model() {
    let scratch = Scratch::new("inspect");
    assert!(korben(scratch.path(), &["new", "app", "--template", "service"]).status.success());
    let output = korben(&scratch.path().join("app"), &["inspect"]);
    assert!(output.status.success(), "{}", combined(&output));
    let text = stdout(&output);
    assert!(text.contains("pub fn handle"), "{text}");
    assert!(text.contains("pub type AppError"), "{text}");
    assert!(text.contains("use std.json"), "{text}");
}

#[test]
fn doctor_reports_the_toolchain_and_project() {
    let scratch = Scratch::new("doctor");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let output = korben(&scratch.path().join("app"), &["doctor"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("korben "), "{text}");
    assert!(text.contains("edition      2026"), "{text}");
    assert!(text.contains("modules"), "{text}");
}

#[test]
fn new_refuses_to_overwrite_an_existing_directory() {
    let scratch = Scratch::new("overwrite");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let second = korben(scratch.path(), &["new", "app"]);
    assert!(!second.status.success());
    assert!(combined(&second).contains("already exists"));
}

#[test]
fn the_bundled_examples_run_check_and_format_cleanly() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples");
    let mut ran = 0usize;
    for entry in std::fs::read_dir(&examples).expect("read examples") {
        let path = entry.expect("example entry").path();
        if path.extension().map(|extension| extension != "kb").unwrap_or(true) {
            continue;
        }
        ran += 1;
        let file = path.to_str().unwrap();

        let check = korben(&examples, &["check", file]);
        assert!(check.status.success(), "check {file} failed:\n{}", combined(&check));

        let run = korben(&examples, &["run", file]);
        assert!(run.status.success(), "run {file} failed:\n{}", combined(&run));
        assert!(!stdout(&run).is_empty(), "{file} printed nothing");

        let fmt = korben(&examples, &["fmt", "--check", file]);
        assert!(fmt.status.success(), "{file} is not formatted:\n{}", combined(&fmt));
    }
    assert!(ran >= 2, "expected the examples directory to have examples");
}

#[test]
fn ffi_generates_bindings_from_a_header_and_lists_them() {
    let scratch = Scratch::new("ffi");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    std::fs::remove_file(project.join("src/main.kb")).unwrap();
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();
    std::fs::write(project.join("libc.h"), "size_t strlen(const char *s);\nint abs(int value);\n")
        .unwrap();

    // Generate a binding module from the header.
    let generated = korben(
        &project,
        &[
            "ffi",
            "c",
            "libc.h",
            "--library",
            "c",
            "--module",
            "bindings",
            "--out",
            "src/bindings.kb",
        ],
    );
    assert!(generated.status.success(), "{}", combined(&generated));
    assert!(stdout(&generated).contains("2 bindings"), "{}", stdout(&generated));

    let module = std::fs::read_to_string(project.join("src/bindings.kb")).unwrap();
    assert!(module.contains("(ffi/c-library \"c\")"), "{module}");
    assert!(module.contains("(pub ffi/c-fn strlen [s: CStr] -> CULong)"), "{module}");

    // Write a safe wrapper over the generated bindings and run it.
    std::fs::write(
        project.join("src/main.kb"),
        r#"(module main
  (use bindings [strlen abs]))

;;; Length of a string in bytes, as C counts it.
(pub fn byte-length [text: String] -> Int !ffi !unsafe
  (unsafe (strlen text)))

(pub fn main [] -> Unit !io !ffi !unsafe
  (println (byte-length "korben"))
  (println (unsafe (abs -9))))
"#,
    )
    .unwrap();

    let check = korben(&project, &["check"]);
    assert!(check.status.success(), "{}", combined(&check));

    let run = korben(&project, &["run"]);
    assert!(run.status.success(), "{}", combined(&run));
    assert_eq!(stdout(&run), "6\n9\n");

    // `korben ffi` reports what the project declares.
    let listed = korben(&project, &["ffi"]);
    assert!(listed.status.success(), "{}", combined(&listed));
    let text = stdout(&listed);
    assert!(text.contains("strlen"), "{text}");
    assert!(text.contains("(ffi/c-fn strlen [s: CStr] -> CULong)"), "{text}");
    assert!(text.contains("2 foreign declarations"), "{text}");
}

#[test]
fn calling_a_foreign_function_without_unsafe_is_rejected() {
    let scratch = Scratch::new("ffisafe");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();
    std::fs::write(
        project.join("src/main.kb"),
        "(module main)\n\n(ffi/c-library \"c\")\n(ffi/c-fn abs [value: CInt] -> CInt)\n\n(pub fn main [] -> Unit !io !ffi !unsafe (println (abs -1)))\n",
    )
    .unwrap();

    let check = korben(&project, &["check"]);
    assert!(!check.status.success());
    let text = combined(&check);
    assert!(text.contains("unsafe-call"), "{text}");
    assert!(text.contains("wrap this in `(unsafe ...)`"), "{text}");
}
