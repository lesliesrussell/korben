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
    // korben-blf: `publish` used to be one of these and now exists, so this
    // asks about one that is still ahead.
    let output = korben(scratch.path(), &["install"]);
    assert!(!output.status.success());
    assert!(combined(&output).contains("Milestone"), "{}", combined(&output));
}

// korben-mic
/// Write a workspace: a virtual root, a library, and a program that uses it.
fn workspace(scratch: &Scratch) {
    let root = scratch.path();
    std::fs::write(root.join("korben.toml"), "[workspace]\nmembers = [\"lib\", \"app\"]\n")
        .expect("write root manifest");

    std::fs::create_dir_all(root.join("lib").join("src")).expect("create lib");
    std::fs::write(
        root.join("lib").join("korben.toml"),
        "[package]\nname = \"lib\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\nmain = \"lib\"\n",
    )
    .expect("write lib manifest");
    std::fs::write(
        root.join("lib").join("src").join("lib.kb"),
        "(module lib)\n\n;;; Greet someone.\n(pub fn hello [name: String] -> String\n  (format \"Hello, {name}!\"))\n",
    )
    .expect("write lib source");

    std::fs::create_dir_all(root.join("app").join("src")).expect("create app");
    std::fs::write(
        root.join("app").join("korben.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nlicense = \"MIT\"\nmain = \"main\"\n\n[dependencies]\nlib = \"^1.0\"\n",
    )
    .expect("write app manifest");
    std::fs::write(
        root.join("app").join("src").join("main.kb"),
        "(module main\n  (use lib :as lib))\n\n;;; Entry.\n(pub fn main [] -> Unit !io (println (lib.hello \"Ada\")))\n",
    )
    .expect("write app source");
}

#[test]
fn a_workspace_checks_every_member_and_locks_them_together() {
    let scratch = Scratch::new("workspace-check");
    workspace(&scratch);

    let check = korben(scratch.path(), &["check"]);
    assert!(check.status.success(), "{}", combined(&check));
    // Both members, not just whichever one the root stands in for.
    assert!(stdout(&check).contains("2 modules"), "{}", stdout(&check));

    // One lock, at the workspace root.
    assert!(scratch.path().join("korben.lock").is_file(), "no lock at the root");
    assert!(!scratch.path().join("app").join("korben.lock").is_file(), "a member wrote a lock");
}

#[test]
fn a_workspace_runs_and_builds_the_member_that_has_the_program() {
    let scratch = Scratch::new("workspace-build");
    workspace(&scratch);

    let run = korben(scratch.path(), &["run"]);
    assert!(run.status.success(), "{}", combined(&run));
    assert!(stdout(&run).contains("Hello, Ada!"), "{}", stdout(&run));

    // The artifact is named and placed for the member being built, not for
    // whichever member the session happened to open on.
    let build = korben(scratch.path(), &["build"]);
    assert!(build.status.success(), "{}", combined(&build));
    assert!(
        scratch.path().join("app").join("target").join("debug").join("app").is_file(),
        "the executable is not where `app` should have put it:\n{}",
        combined(&build)
    );
}

#[test]
fn a_workspace_with_two_programs_asks_which_one() {
    let scratch = Scratch::new("workspace-ambiguous");
    workspace(&scratch);
    // Give `lib` a `main` too, so neither member is the obvious choice.
    std::fs::write(
        scratch.path().join("lib").join("src").join("lib.kb"),
        "(module lib)\n\n;;; Entry.\n(pub fn main [] -> Unit !io (println \"lib\"))\n",
    )
    .expect("write");

    let run = korben(scratch.path(), &["run"]);
    assert!(!run.status.success(), "it guessed instead of asking");
    assert!(combined(&run).contains("--package"), "{}", combined(&run));

    let chosen = korben(scratch.path(), &["run", "--package", "lib"]);
    assert!(chosen.status.success(), "{}", combined(&chosen));
    assert!(stdout(&chosen).contains("lib"), "{}", stdout(&chosen));
}

// korben-efd
#[test]
fn the_language_server_speaks_the_protocol_on_stdin_and_stdout() {
    let scratch = Scratch::new("lsp");
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let exit = r#"{"jsonrpc":"2.0","method":"exit"}"#;
    let input = format!(
        "Content-Length: {}\r\n\r\n{body}Content-Length: {}\r\n\r\n{exit}",
        body.len(),
        exit.len()
    );
    let mut child = Command::new(EXE)
        .arg("lsp")
        .current_dir(scratch.path())
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the language server");
    use std::io::Write;
    child.stdin.take().expect("stdin").write_all(input.as_bytes()).expect("write");
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "{}", combined(&output));
    let text = stdout(&output);
    assert!(text.starts_with("Content-Length: "), "unframed output: {text}");
    assert!(text.contains("\"hoverProvider\":true"), "{text}");
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

// korben-rm1
const ADAPTER: &str = r#"use korben_export::korben_export;

#[korben_export]
pub fn slugify(input: &str) -> String {
    input.trim().to_lowercase().replace(' ', "-")
}

#[korben_export]
pub fn add(left: i64, right: i64) -> i64 {
    left + right
}

#[korben_export]
pub fn scale(value: f64, factor: f64) -> f64 {
    value * factor
}

#[korben_export]
pub fn toggle(flag: bool) -> bool {
    !flag
}

#[korben_export]
pub fn parse_port(text: String) -> Result<i64, String> {
    text.parse::<i64>().map_err(|error| format!("{text}: {error}"))
}

#[korben_export]
pub fn log_line(text: &str) {
    println!("{text}");
}

/// Not annotated, so not the generator's business.
pub fn helper(value: i64) -> i64 {
    value
}
"#;

// korben-rm1
#[test]
fn a_generated_adapter_module_checks_and_is_already_formatted() {
    let scratch = Scratch::new("ffirust");
    let source = scratch.path().join("lib.rs");
    std::fs::write(&source, ADAPTER).unwrap();
    let out = scratch.path().join("slug.kb");

    let generated = korben(
        scratch.path(),
        &[
            "ffi",
            "rust",
            source.to_str().unwrap(),
            "--library",
            "slug",
            "--module",
            "slug",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert!(generated.status.success(), "{}", combined(&generated));
    assert!(stdout(&generated).contains("6 bindings"), "{}", stdout(&generated));

    let text = std::fs::read_to_string(&out).unwrap();
    // Both layers are there: the foreign contract and the wrapper over it.
    assert!(
        text.contains("(ffi/c-fn raw-slugify \"korben_export_slugify\" [input: CStr] -> CStr)"),
        "{text}"
    );
    assert!(text.contains("(pub fn slugify [input: String] -> Result String String"), "{text}");
    // The error channel is declared once, however many functions there are.
    assert_eq!(text.matches("\"korben_export_last_error\"").count(), 1);
    // A name that is not annotated is not the generator's business.
    assert!(!text.contains("helper"), "{text}");

    // The generated module is real Korben: it type-checks, and it is already
    // in the canonical form `korben fmt --check` demands of a project.
    let checked = korben(scratch.path(), &["check", out.to_str().unwrap()]);
    assert!(checked.status.success(), "{}", combined(&checked));
    let formatted = korben(scratch.path(), &["fmt", "--check", out.to_str().unwrap()]);
    assert!(formatted.status.success(), "{}", combined(&formatted));
}

// korben-rm1
#[test]
fn a_signature_that_cannot_cross_is_listed_rather_than_guessed_at() {
    let scratch = Scratch::new("ffirustskip");
    let source = scratch.path().join("lib.rs");
    std::fs::write(
        &source,
        "use korben_export::korben_export;\n\n\
         #[korben_export]\npub fn tally(rows: Vec<i64>) -> i64 { rows.len() as i64 }\n\n\
         #[korben_export]\npub fn scale(text: &str, factor: f64) -> f64 { factor }\n",
    )
    .unwrap();

    let generated = korben(scratch.path(), &["ffi", "rust", source.to_str().unwrap()]);
    assert!(generated.status.success(), "{}", combined(&generated));
    let text = combined(&generated);
    assert!(text.contains("Not exported:"), "{text}");
    assert!(text.contains("Vec<i64>"), "{text}");
    assert!(text.contains("mixes floating-point and other parameters"), "{text}");
    assert!(text.contains("skipped 2 functions"), "{text}");
}

// korben-rm1
#[test]
fn a_file_with_nothing_annotated_says_so() {
    let scratch = Scratch::new("ffirustempty");
    let source = scratch.path().join("lib.rs");
    std::fs::write(&source, "pub fn ordinary(value: i64) -> i64 { value }\n").unwrap();

    let generated = korben(scratch.path(), &["ffi", "rust", source.to_str().unwrap()]);
    assert!(generated.status.success(), "{}", combined(&generated));
    assert!(
        combined(&generated).contains("no `#[korben_export]` functions"),
        "{}",
        combined(&generated)
    );
}

// korben-ym2
/// Send a request on a fresh connection and return the status line.
fn http_status(port: u16, timeout: std::time::Duration) -> Option<String> {
    use std::io::{Read, Write};
    let mut socket = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    socket.set_read_timeout(Some(timeout)).ok()?;
    socket.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n\r\n").ok()?;
    let mut buffer = [0u8; 128];
    let count = socket.read(&mut buffer).ok()?;
    let text = String::from_utf8_lossy(&buffer[..count]).to_string();
    text.lines().next().map(|line| line.trim().to_string())
}

// korben-ym2
#[test]
fn a_silent_client_does_not_hold_up_the_server() {
    // The case that decides the design. A client that connects and never sends
    // used to stop every later client: the accepting task was underneath its
    // read and could not get back to accepting.
    let scratch = Scratch::new("httpconcurrent");
    assert!(korben(scratch.path(), &["new", "server", "--template", "service"]).status.success());
    let project = scratch.path().join("server");
    let port = free_port();
    std::fs::write(
        project.join("src/main.kb"),
        format!(
            r#"(module main
  (use std.http :as http))

(fn handle [request: http.Request] -> http.Response
  (http.text 200 "ok"))

(pub fn main [] -> Unit !io
  (match (http.serve "127.0.0.1:{port}" handle)
    (Ok _) nil
    (Err error) (println "server stopped:" (http.describe error))))
"#
        ),
    )
    .unwrap();
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();

    let mut server = Command::new(EXE)
        .arg("run")
        .current_dir(&project)
        .env("NO_COLOR", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start the server");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(std::time::Instant::now() < deadline, "the server never bound {port}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let short = std::time::Duration::from_secs(5);
    let first = http_status(port, short);

    // Hold a connection open and say nothing on it, ever.
    let silent = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect silently");
    std::thread::sleep(std::time::Duration::from_millis(200));
    let during_silence = http_status(port, short);

    // And one that sends a request it never finishes.
    let mut partial = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect partially");
    {
        use std::io::Write;
        partial
            .write_all(b"POST /x HTTP/1.1\r\nContent-Length: 100\r\n\r\nhalf")
            .expect("send half a request");
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    let during_partial = http_status(port, short);

    let _ = server.kill();
    let _ = server.wait();
    drop(silent);
    drop(partial);

    assert_eq!(first.as_deref(), Some("HTTP/1.1 200 OK"), "the server answered nothing at all");
    assert_eq!(
        during_silence.as_deref(),
        Some("HTTP/1.1 200 OK"),
        "a client that says nothing held up the server"
    );
    assert_eq!(
        during_partial.as_deref(),
        Some("HTTP/1.1 200 OK"),
        "a half-sent request held up the server"
    );
}

// korben-c6k
#[test]
fn a_request_that_never_ends_is_refused_rather_than_buffered() {
    use std::io::{Read, Write};
    let scratch = Scratch::new("httptoolarge");
    assert!(korben(scratch.path(), &["new", "server", "--template", "service"]).status.success());
    let project = scratch.path().join("server");
    let port = free_port();
    std::fs::write(
        project.join("src/main.kb"),
        format!(
            r#"(module main
  (use std.http :as http))

(fn handle [request: http.Request] -> http.Response
  (http.text 200 "ok"))

(pub fn main [] -> Unit !io
  (match (http.serve "127.0.0.1:{port}" handle)
    (Ok _) nil
    (Err error) (println "server stopped:" (http.describe error))))
"#
        ),
    )
    .unwrap();
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();

    let mut server = Command::new(EXE)
        .arg("run")
        .current_dir(&project)
        .env("NO_COLOR", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start the server");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(std::time::Instant::now() < deadline, "the server never bound {port}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // Promise a body far larger than the cap, then send past the cap without
    // ever finishing it.
    let mut greedy = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    greedy.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    greedy
        .write_all(b"POST /x HTTP/1.1\r\nHost: x\r\nContent-Length: 400000\r\n\r\n")
        .expect("send the head");
    let chunk = vec![b'x'; 8192];
    for _ in 0..16 {
        // The server answers and hangs up once the cap is passed, so a write
        // failing here is the expected end of this loop, not a test failure.
        if greedy.write_all(&chunk).is_err() {
            break;
        }
    }
    let mut buffer = [0u8; 64];
    let status = match greedy.read(&mut buffer) {
        Ok(count) if count > 0 => String::from_utf8_lossy(&buffer[..count])
            .lines()
            .next()
            .map(|line| line.trim().to_string()),
        _ => None,
    };

    let _ = server.kill();
    let _ = server.wait();
    assert_eq!(
        status.as_deref(),
        Some("HTTP/1.1 413 Request Too Large"),
        "the server buffered a request that never ends"
    );
}

// korben-c6k
#[test]
fn a_connection_that_does_nothing_is_evicted() {
    let scratch = Scratch::new("httpevict");
    assert!(korben(scratch.path(), &["new", "watcher"]).status.success());
    let project = scratch.path().join("watcher");
    let port = free_port();
    // The pool is what knows a silent connection exists, so the eviction it
    // offers is tested directly rather than through the server's own timeout.
    std::fs::write(
        project.join("src/main.kb"),
        format!(
            r#"(module main
  (use std.net :as net)
  (use std.time :as time))

(pub fn main [] -> Unit !io
  (match (net.pool "127.0.0.1:{port}")
    (Err _) (println "no pool")
    (Ok opened)
      (with server opened
        ;; Accept whatever has arrived; a silent connection is registered here
        ;; but never reported as ready.
        (match (server.wait 3000)
          (Err _) nil
          (Ok _) nil)
        (time.sleep-millis 200)
        (match (server.evict 100)
          (Err _) (println "evict failed")
          (Ok dropped) (println (len dropped))))))
"#
        ),
    )
    .unwrap();
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();

    let server = Command::new(EXE)
        .arg("run")
        .current_dir(&project)
        .env("NO_COLOR", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start the program");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let silent = loop {
        if let Ok(socket) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            break socket;
        }
        assert!(std::time::Instant::now() < deadline, "never bound {port}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };

    let output = server.wait_with_output().expect("wait for the program");
    drop(silent);
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    assert_eq!(text.trim(), "1", "expected one connection to be evicted:\n{text}");
}

// korben-ycd
#[test]
fn profiling_reports_where_the_time_went_without_changing_the_program() {
    let scratch = Scratch::new("profile");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    std::fs::write(
        project.join("src/main.kb"),
        r#"(module main)

;;; Deliberately the expensive one.
(fn work [n: Int] -> Int
  (reduce (range n) 0 +))

(fn quick [] -> Int 1)

(pub fn main [] -> Unit !io
  (println (work 20000))
  (println (quick)))
"#,
    )
    .unwrap();
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();

    let plain = korben(&project, &["run"]);
    assert!(plain.status.success(), "{}", combined(&plain));

    let profiled = korben(&project, &["run", "--profile"]);
    assert!(profiled.status.success(), "{}", combined(&profiled));

    // Specification 23: profiling changes nothing about the program itself.
    assert_eq!(stdout(&plain), stdout(&profiled), "the flag changed the program's output");

    let report = String::from_utf8_lossy(&profiled.stderr).to_string();
    assert!(report.contains("PROFILE"), "{report}");
    assert!(report.contains("work"), "the expensive function is missing:\n{report}");
    assert!(report.contains("calls"), "{report}");
    // The report goes to stderr, so a profiled run still pipes cleanly.
    assert!(!stdout(&profiled).contains("PROFILE"), "{}", stdout(&profiled));
}

// korben-blf
#[test]
fn a_published_package_is_resolvable_and_immutable() {
    let scratch = Scratch::new("publish");
    let registry = scratch.path().join("registry");
    let registry_arg = registry.to_str().unwrap().to_string();

    assert!(korben(scratch.path(), &["new", "greeting", "--template", "lib"]).status.success());
    let library = scratch.path().join("greeting");

    let published = korben(&library, &["publish", "--registry", &registry_arg]);
    assert!(published.status.success(), "{}", combined(&published));
    let report = stdout(&published);
    assert!(report.contains("published greeting 0.1.0"), "{report}");
    let checksum = report
        .lines()
        .find_map(|line| line.trim().strip_prefix("checksum "))
        .expect("the checksum is reported")
        .to_string();
    assert!(registry.join("greeting/0.1.0/korben.toml").is_file());

    // A published version is what a lockfile pins, so it must never change.
    let again = korben(&library, &["publish", "--registry", &registry_arg]);
    assert!(!again.status.success());
    assert!(combined(&again).contains("already published"), "{}", combined(&again));

    // And another project can depend on exactly that.
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let app = scratch.path().join("app");
    let added = Command::new(EXE)
        .args(["add", "greeting", "--version", "^0.1"])
        .current_dir(&app)
        .env("NO_COLOR", "1")
        .env("KORBEN_REGISTRY", &registry)
        .output()
        .expect("run korben");
    assert!(added.status.success(), "{}", combined(&added));

    let lock = std::fs::read_to_string(app.join("korben.lock")).expect("a lockfile");
    assert!(lock.contains(&checksum), "the lockfile pins a different package:\n{lock}");

    let checked = Command::new(EXE)
        .arg("check")
        .current_dir(&app)
        .env("NO_COLOR", "1")
        .env("KORBEN_REGISTRY", &registry)
        .output()
        .expect("run korben");
    assert!(checked.status.success(), "{}", combined(&checked));
}

// korben-blf
#[test]
fn a_package_depending_on_a_path_is_not_publishable() {
    let scratch = Scratch::new("publishpath");
    let registry = scratch.path().join("registry");
    assert!(korben(scratch.path(), &["new", "helper", "--template", "lib"]).status.success());
    assert!(korben(scratch.path(), &["new", "app", "--template", "lib"]).status.success());
    let app = scratch.path().join("app");
    assert!(korben(&app, &["add", "helper", "--path", "../helper"]).status.success());

    let published = korben(&app, &["publish", "--registry", registry.to_str().unwrap()]);
    assert!(!published.status.success());
    let text = combined(&published);
    assert!(text.contains("depends on a path"), "{text}");
    assert!(text.contains("helper"), "{text}");
    // Nothing half-written is left behind for resolution to find.
    assert!(!registry.join("app").exists());
}
