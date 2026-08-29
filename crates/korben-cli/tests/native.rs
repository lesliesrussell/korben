//! Differential tests for the native backend.
//!
//! The specification requires the development and native execution modes to
//! share observable semantics. These tests assert that literally: the same
//! program, run through the interpreter and compiled to a native executable,
//! must produce byte-identical output.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EXE: &str = env!("CARGO_BIN_EXE_korben");

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let unique = format!("korben-native-{label}-{}", std::process::id());
        let path = std::env::temp_dir().join(unique);
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

/// The native backend shells out to cargo; skip rather than fail without it.
fn cargo_available() -> bool {
    Command::new("cargo").arg("--version").output().map(|out| out.status.success()).unwrap_or(false)
}

/// Build `source` both ways and return (interpreted, native) combined output.
fn both_ways(label: &str, module: &str, source: &str) -> (String, String) {
    let scratch = Scratch::new(label);
    let created = korben(scratch.path(), &["new", "app"]);
    assert!(created.status.success(), "new failed:\n{}", combined(&created));
    let project = scratch.path().join("app");
    std::fs::remove_file(project.join("src/main.kb")).unwrap();
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();
    std::fs::write(project.join(format!("src/{module}.kb")), source).unwrap();
    let manifest = std::fs::read_to_string(project.join("korben.toml")).unwrap();
    std::fs::write(
        project.join("korben.toml"),
        manifest.replace("main = \"main\"", &format!("main = \"{module}\"")),
    )
    .unwrap();

    let interpreted = korben(&project, &["run"]);
    let built = korben(&project, &["build"]);
    assert!(built.status.success(), "native build failed:\n{}", combined(&built));

    let executable = project.join("target/debug/app");
    let native = Command::new(&executable)
        .current_dir(&project)
        .output()
        .expect("run the native executable");

    (combined(&interpreted), combined(&native))
}

fn assert_same(label: &str, module: &str, source: &str) {
    if !cargo_available() {
        eprintln!("skipping {label}: no cargo on PATH");
        return;
    }
    let (interpreted, native) = both_ways(label, module, source);
    assert_eq!(
        interpreted, native,
        "\n{label}: the two execution modes disagree\n--- interpreted ---\n{interpreted}\n--- native ---\n{native}"
    );
    assert!(!interpreted.is_empty(), "{label} produced no output");
}

#[test]
fn values_and_collections_agree() {
    assert_same(
        "values",
        "values",
        r#"(module values)

(pub fn main [] -> Unit !io
  (println 42 -17 3.5 true false nil)
  (println "text" :keyword)
  (println [1 2 3] #{1 2 2 3} {:a 1 :b 2})
  (println (+ 1 2) (- 10 3) (* 2 3) (/ 10 2) (mod -7 3))
  (println (< 1 2 3) (= [1 2] [1 2]) (not= 1 2))
  (println (len [1 2 3]) (first [1 2]) (rest [1 2 3]) (reverse [1 2 3]))
  (println (map [1 2 3] inc) (filter [1 2 3 4] (fn [n] (= 0 (mod n 2)))))
  (println (reduce [1 2 3 4] 0 +) (sort [3 1 2]) (range 4))
  (println (conj [1] 2 3) (concat [1] [2]) (contains? [1 2] 2))
  (println (assoc {:a 1} :b 2) (keys {:a 1 :b 2}) (values {:a 1 :b 2})))
"#,
    );
}

#[test]
fn records_enums_and_matching_agree() {
    assert_same(
        "matching",
        "matching",
        r#"(module matching)

(pub type User { id: Int name: String })

(pub enum Shape
  (Circle radius: Int)
  (Square side: Int)
  (Nothing))

(fn area [s: Shape] -> Int
  (match s
    (Circle radius) (* 3 radius radius)
    (Square side) (* side side)
    (Nothing) 0))

(fn classify [n: Int] -> Keyword
  (match n
    0 :zero
    v :when (> v 0) :positive
    _ :negative))

(fn route [request] -> String
  (match request
    {:method :get :path "/health"} "ok"
    {:method :post :body body} body
    _ "not found"))

(fn head-and-rest [v: Vec Int] -> Vec Int
  (match v
    [] []
    [head ...tail] (conj tail head)))

(pub fn main [] -> Unit !io
  (let user (User { id 7 name "Mack" }))
  (println user user.name (get user :id))
  (println (area (Circle 2)) (area (Square 3)) (area (Nothing)))
  (println (classify 0) (classify 5) (classify -5))
  (println (route {:method :get :path "/health"}))
  (println (route {:method :post :body "created"}))
  (println (route {:method :put}))
  (println (head-and-rest [1 2 3]) (head-and-rest [])))
"#,
    );
}

#[test]
fn control_flow_and_errors_agree() {
    assert_same(
        "control",
        "control",
        r#"(module control
  (use std.string :as string))

(fn parse-port [text: String] -> Result Int String
  (let value (string.parse-int text)?)
  (if (and (> value 0) (< value 65536)) (Ok value) (Err "out of range")))

(fn sum [values] -> Int
  (loop [remaining values total 0]
    (match remaining
      [] total
      [head ...tail] (recur tail (+ total head)))))

(fn counters [] -> Vec Int
  (var total 0)
  (set! total (+ total 5))
  (let cell (Cell.new 1))
  (cell.update (fn [n] (* n 10)))
  [total (cell.get)])

(pub fn main [] -> Unit !io
  (println (parse-port "8080") (parse-port "nope") (parse-port "99999"))
  (println (sum [1 2 3 4 5]))
  (println (counters))
  (println (cond false :a :else :b) (when true :yes) (unless true :no))
  (println (and 1 2 3) (or false :fallback))
  (println (if-let n (Some 42) n :none))
  (try
    (throw "boom")
    (catch Condition c (println "caught" c))
    (finally (println "cleanup")))
  (defer (println "deferred last"))
  (println "body done"))
"#,
    );
}

#[test]
fn protocols_macros_and_json_agree() {
    assert_same(
        "protocols",
        "protocols",
        r#"(module protocols
  (use std.json :as json))

(pub type User { name: String })
(pub type Robot { serial: Int })

(pub protocol Renderable
  (render [self] -> String))

(impl Renderable User
  (fn render [u] (format "user {u.name}")))

(impl Renderable Robot
  (fn render [r] (format "robot {r.serial}")))

(macro twice [form]
  `(do ~form ~form))

(macro my-list [...items]
  (if (empty? items) `[] `(conj (my-list ~@(rest items)) ~(get items 0))))

(pub fn main [] -> Unit !io
  (println (render (User { name "Ada" })))
  (println (render (Robot { serial 5 })))
  (twice (println "twice"))
  (println (my-list 1 2 3))
  (println (json.encode {:name "Ada" :tags ["x" "y"] :n 3}))
  (println (json.decode "[1,2,3]"))
  (println (string-of)))

(fn string-of [] -> String
  (format "{(+ 1 2)} and {(str \"a\" \"b\")} and {{literal}}"))
"#,
    );
}

#[test]
fn runtime_faults_report_identically() {
    if !cargo_available() {
        eprintln!("skipping: no cargo on PATH");
        return;
    }
    // A fault must name the same code, message, and source line in both modes.
    let (interpreted, native) = both_ways(
        "faults",
        "faults",
        r#"(module faults)

(pub fn main [] -> Unit !io
  (println "before")
  (println (/ 1 0)))
"#,
    );
    assert_eq!(interpreted, native, "fault reports differ");
    assert!(interpreted.contains("error[divide-by-zero]"), "{interpreted}");
    assert!(interpreted.contains("faults.kb:5:12"), "{interpreted}");
}

#[test]
fn every_project_template_agrees() {
    if !cargo_available() {
        eprintln!("skipping: no cargo on PATH");
        return;
    }
    for template in ["cli", "lib", "service"] {
        let scratch = Scratch::new(&format!("template-{template}"));
        assert!(korben(scratch.path(), &["new", "app", "--template", template]).status.success());
        let project = scratch.path().join("app");

        let interpreted = korben(&project, &["run"]);
        let built = korben(&project, &["build"]);
        assert!(built.status.success(), "{template} build failed:\n{}", combined(&built));
        let native = Command::new(project.join("target/debug/app"))
            .current_dir(&project)
            .output()
            .expect("run native executable");
        assert_eq!(
            stdout(&interpreted),
            stdout(&native),
            "{template} differs between execution modes"
        );
    }
}

#[test]
fn emit_ir_and_emit_rust_produce_inspectable_output() {
    let scratch = Scratch::new("emit");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");

    let ir = korben(&project, &["build", "--emit", "ir"]);
    assert!(ir.status.success(), "{}", combined(&ir));
    let text = stdout(&ir);
    assert!(text.contains(";; entry:"), "{text}");
    assert!(text.contains("(fn m_main__greeting"), "{text}");
    assert!(text.contains("builtin:std.io/println"), "{text}");

    let rust = korben(&project, &["build", "--emit", "rust"]);
    assert!(rust.status.success(), "{}", combined(&rust));
    let text = stdout(&rust);
    assert!(text.contains("fn main()"), "{text}");
    assert!(text.contains("korben_runtime"), "{text}");
    assert!(text.contains("fn f_m_main__greeting"), "{text}");
}

#[test]
fn a_program_without_main_is_rejected_before_generating_code() {
    let scratch = Scratch::new("nomain");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");
    std::fs::write(project.join("src/main.kb"), "(module main)\n\n(pub fn helper [] -> Int 1)\n")
        .unwrap();
    std::fs::remove_file(project.join("tests/main_test.kb")).unwrap();

    let built = korben(&project, &["build"]);
    assert!(!built.status.success());
    assert!(combined(&built).contains("defines no `main`"), "{}", combined(&built));
}

#[test]
fn foreign_calls_agree_between_execution_modes() {
    assert_same(
        "ffi",
        "ffi",
        r#"(module ffi)

(ffi/c-library "c")
(ffi/c-fn strlen [text: CStr] -> CULong)
(ffi/c-fn abs [value: CInt] -> CInt)
(ffi/c-fn pow [base: CDouble exponent: CDouble] -> CDouble)
(ffi/c-fn getenv [name: CStr] -> CStr)

(pub fn byte-length [text: String] -> Int !ffi !unsafe
  (unsafe (strlen text)))

(pub fn main [] -> Unit !io !ffi !unsafe
  (println (byte-length "korben"))
  (println (unsafe (abs -42)))
  (println (unsafe (pow 2.0 10.0)))
  (println (unsafe (getenv "KORBEN_UNSET_VARIABLE")))
  (println (map ["one" "three"] byte-length)))
"#,
    );
}

#[test]
fn structured_concurrency_agrees_between_execution_modes() {
    assert_same(
        "async",
        "concurrency",
        r#"(module concurrency
  (use std.async :as task))

(async fn work [n: Int] -> Result Int String !async !io
  (println (format "running {n}"))
  (if (= n 9) (Err "nine is bad") (Ok (* n 2))))

(async fn produce [sender: Sender] -> Unit !async
  (each [1 2 3] (fn [n] (sender.send n)))
  (sender.close))

(pub fn drain [receiver: Receiver total: Int] -> Int !async
  (match (receiver.recv)
    (Some value) (recur receiver (+ total value))
    (None) total))

(pub fn main [] -> Unit !io !async
  (task-scope scope
    (let tasks (map [1 2 3] (fn [n] (spawn scope (work n)))))
    (println "spawned")
    (println (task.join-all tasks)))

  (task-scope scope
    (let tasks (map [1 9 3] (fn [n] (spawn scope (work n)))))
    (println (task.join-all tasks)))

  (task-scope scope
    (spawn scope (work 5))
    (println "the scope joins this one"))

  (task-scope scope
    (let pending (spawn scope (work 8)))
    (scope.cancel)
    (println (pending.state)))

  (async (println (await (work 4))))

  (task-scope scope
    (let ends (task.channel))
    (let sender (get ends 0))
    (let receiver (get ends 1))
    (spawn scope (produce sender))
    (println "channel total:" (drain receiver 0))))
"#,
    );
}

// korben-wzh
#[test]
fn top_level_constants_agree_between_execution_modes() {
    assert_same(
        "constants",
        "constants",
        r#"(module constants)

(def limit: Int 42)
(def greeting "hello")
(def sizes [1 2 3])

(pub fn main [] -> Unit !io
  (println limit greeting sizes)
  (println (+ limit 1) (len sizes)))
"#,
    );
}

// korben-8cg
/// Targets rustup reports as installed, or none when rustup is absent.
fn installed_targets() -> Vec<String> {
    let Ok(output) = Command::new("rustup").args(["target", "list", "--installed"]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout).lines().map(|line| line.trim().to_string()).collect()
}

// korben-8cg
#[test]
fn an_unknown_target_is_rejected_with_a_suggestion() {
    let scratch = Scratch::new("badtarget");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");

    let built = korben(&project, &["build", "--target", "x86_64-unknown-linux-gnuu"]);
    assert!(!built.status.success());
    let text = combined(&built);
    assert!(text.contains("is not a target rustc knows"), "{text}");
    assert!(text.contains("Did you mean `x86_64-unknown-linux-gnu`?"), "{text}");
}

// korben-8cg
#[test]
fn a_target_without_a_standard_library_names_the_command_that_installs_it() {
    let installed = installed_targets();
    if installed.is_empty() {
        eprintln!("skipping: no rustup to ask about installed targets");
        return;
    }
    // A real triple nobody is likely to have installed, and which this machine
    // demonstrably does not have.
    let candidates =
        ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-musl", "i686-pc-windows-msvc"];
    let Some(missing) = candidates.iter().find(|triple| !installed.contains(&triple.to_string()))
    else {
        eprintln!("skipping: every candidate target is installed here");
        return;
    };

    let scratch = Scratch::new("notarget");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");

    let built = korben(&project, &["build", "--target", missing]);
    assert!(!built.status.success());
    let text = combined(&built);
    assert!(text.contains("is not installed"), "{text}");
    assert!(text.contains(&format!("rustup target add {missing}")), "{text}");
}

// korben-8cg
#[test]
fn a_cross_build_lands_under_its_triple() {
    if !cargo_available() {
        eprintln!("skipping: no cargo on PATH");
        return;
    }
    let installed = installed_targets();
    // The host triple is always buildable, and naming it explicitly exercises
    // every part of the path a genuine cross build takes.
    let host = match String::from_utf8(
        Command::new("rustc").arg("-vV").output().expect("run rustc").stdout,
    ) {
        Ok(text) => text
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .map(|triple| triple.trim().to_string()),
        Err(_) => None,
    };
    let Some(host) = host else {
        eprintln!("skipping: cannot determine the host triple");
        return;
    };
    if !installed.is_empty() && !installed.contains(&host) {
        eprintln!("skipping: the host target is not installed");
        return;
    }

    let scratch = Scratch::new("crossbuild");
    assert!(korben(scratch.path(), &["new", "app"]).status.success());
    let project = scratch.path().join("app");

    let built = korben(&project, &["build", "--target", &host]);
    assert!(built.status.success(), "{}", combined(&built));
    assert!(stdout(&built).contains(&host), "{}", stdout(&built));
    let artifact = project.join("target").join(&host).join("debug").join("app");
    assert!(artifact.exists(), "no artifact at {}", artifact.display());
    // The default build keeps its own place, so the two do not collide.
    assert!(!project.join("target/debug/app").exists());
}

// korben-vdw
/// The environment variable a dynamic loader takes its search path from.
fn library_path_variable() -> &'static str {
    if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    }
}

// korben-vdw
#[test]
fn a_rust_adapter_agrees_between_execution_modes() {
    if !cargo_available() {
        eprintln!("skipping: no cargo on PATH");
        return;
    }
    if !cfg!(unix) {
        eprintln!("skipping: foreign calls are Unix-only");
        return;
    }

    let example = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples/adapter");
    let scratch = Scratch::new("adapter");
    let rust_target = scratch.path().join("rust");

    // korben-bve
    // Copy the crate out of the checkout before building it. Building it where
    // it lives lets cargo rewrite the committed `Cargo.lock` -- which it did,
    // silently, whenever the workspace version had moved -- and makes two
    // concurrent runs contend over that file. Only the path to korben-export
    // has to change, and the lockfile does not record it.
    let source = scratch.path().join("adapter");
    std::fs::create_dir_all(source.join("src")).unwrap();
    std::fs::copy(example.join("src/lib.rs"), source.join("src/lib.rs")).unwrap();
    std::fs::copy(example.join("Cargo.lock"), source.join("Cargo.lock")).unwrap();
    let export =
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("crates").join("korben-export");
    let manifest = std::fs::read_to_string(example.join("Cargo.toml")).unwrap();
    let manifest = manifest.replace(
        "korben-export = { path = \"../../crates/korben-export\" }",
        &format!("korben-export = {{ path = {:?} }}", export.display().to_string()),
    );
    assert!(manifest.contains("korben-export"), "the adapter manifest changed shape");
    std::fs::write(source.join("Cargo.toml"), manifest).unwrap();

    // `--locked` because the copy carries the committed lockfile: a lockfile
    // that has fallen behind the workspace version now fails here instead of
    // being quietly rewritten in place.
    let built = Command::new("cargo")
        .args(["build", "--locked", "--manifest-path"])
        .arg(source.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&rust_target)
        .output()
        .expect("build the adapter");
    assert!(built.status.success(), "adapter build failed:\n{}", combined(&built));
    let libraries = rust_target.join("debug");

    // Copy the Korben half out of the checkout, so neither execution mode
    // writes its build directory into the repository.
    let project = scratch.path().join("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::copy(example.join("korben.toml"), project.join("korben.toml")).unwrap();
    for module in ["main.kb", "slug.kb"] {
        std::fs::copy(example.join("src").join(module), project.join("src").join(module)).unwrap();
    }

    let run = |args: &[&str]| {
        Command::new(EXE)
            .args(args)
            .current_dir(&project)
            .env("NO_COLOR", "1")
            .env(library_path_variable(), &libraries)
            .output()
            .expect("run korben")
    };

    let interpreted = run(&["run"]);
    assert!(interpreted.status.success(), "interpreted run failed:\n{}", combined(&interpreted));
    // The adapter really ran: this is a string Rust built and handed back.
    assert!(stdout(&interpreted).contains("slugify: korben-is-fast"), "{}", stdout(&interpreted));
    // An error raised in Rust arrives as an `Err` carrying its message.
    assert!(
        stdout(&interpreted).contains("truncate failed: a limit of 0 leaves nothing to show"),
        "{}",
        stdout(&interpreted)
    );

    let compiled = run(&["build"]);
    assert!(compiled.status.success(), "native build failed:\n{}", combined(&compiled));
    let native = Command::new(project.join("target/debug/adapter"))
        .current_dir(&project)
        .env("NO_COLOR", "1")
        .env(library_path_variable(), &libraries)
        .output()
        .expect("run the native executable");

    assert_eq!(
        combined(&interpreted),
        combined(&native),
        "the two execution modes disagree about the adapter"
    );
}

// korben-str
#[test]
fn generated_code_compiles_without_warnings() {
    if !cargo_available() {
        eprintln!("skipping: no cargo on PATH");
        return;
    }
    // A wall of warnings about code the user did not write, and cannot fix,
    // reads as a compiler that does not compile cleanly. The service template
    // produced 41 of them before this was fixed.
    for template in ["cli", "lib", "service"] {
        let scratch = Scratch::new(&format!("warnfree-{template}"));
        let created = korben(scratch.path(), &["new", "app", "--template", template]);
        assert!(created.status.success(), "{}", combined(&created));
        let project = scratch.path().join("app");

        let built = korben(&project, &["build"]);
        assert!(built.status.success(), "{template} failed to build:\n{}", combined(&built));
        let output = combined(&built);
        let warnings: Vec<&str> =
            output.lines().filter(|line| line.starts_with("warning")).collect();
        assert!(
            warnings.is_empty(),
            "the {template} template's generated crate warns:\n{}",
            warnings.join("\n")
        );
    }
}
