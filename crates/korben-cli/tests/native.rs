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
