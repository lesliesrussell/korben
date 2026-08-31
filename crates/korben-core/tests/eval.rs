//! End-to-end language semantics.

// korben-6bc

mod common;
use common::{eval, run};

#[test]
fn arithmetic_and_comparison() {
    assert_eq!(eval("(+ 1 2 3)"), "6");
    assert_eq!(eval("(- 10 3)"), "7");
    assert_eq!(eval("(* 2 3 4)"), "24");
    assert_eq!(eval("(/ 10 2)"), "5");
    assert_eq!(eval("(mod -7 3)"), "2");
    assert_eq!(eval("(< 1 2 3)"), "true");
    assert_eq!(eval("(> 3 3)"), "false");
    assert_eq!(eval("(= [1 2] [1 2])"), "true");
}

#[test]
fn division_by_zero_is_a_fault() {
    let result = run("(fn main [] (/ 1 0))");
    assert_eq!(result.diagnostics, vec!["divide-by-zero"]);
}

#[test]
fn integer_overflow_is_reported() {
    let result = run("(fn main [] (* 9223372036854775807 2))");
    assert_eq!(result.diagnostics, vec!["overflow"]);
}

#[test]
fn strings_interpolate() {
    assert_eq!(eval(r#"(format "sum {(+ 1 2)}")"#), "sum 3");
    let result = run(r#"(fn main [] (let name "Ada") (println (format "Hello, {name}!")))"#);
    assert_eq!(result.output, "Hello, Ada!\n");
}

#[test]
fn collections_are_immutable() {
    let result = run(r#"(fn main []
             (let a [1 2 3])
             (let b (conj a 4))
             (println a)
             (println b))"#);
    assert_eq!(result.output, "[1 2 3]\n[1 2 3 4]\n");
}

#[test]
fn higher_order_functions() {
    assert_eq!(eval("(map [1 2 3] inc)"), "[2 3 4]");
    assert_eq!(eval("(filter [1 2 3 4] (fn [n] (= 0 (mod n 2))))"), "[2 4]");
    assert_eq!(eval("(reduce [1 2 3 4] 0 +)"), "10");
    assert_eq!(eval("(map [1 2 3] #(* % 10))"), "[10 20 30]");
    assert_eq!(eval("(sort [3 1 2])"), "[1 2 3]");
}

#[test]
fn records_and_field_access() {
    let result = run(r#"(type User { id: Int name: String })
           (fn main []
             (let user (User { id 7 name "Mack" }))
             (println user.name)
             (println (User { id 7 name "Mack" }).id)
             (println (get user :id)))"#);
    assert_eq!(result.output, "Mack\n7\n7\n");
}

#[test]
fn enums_and_pattern_matching() {
    let result = run(r#"(enum Shape (Circle radius: Int) (Square side: Int) (Empty))
           (fn area [s]
             (match s
               (Circle radius) (* 3 radius radius)
               (Square side) (* side side)
               (Empty) 0))
           (fn main []
             (println (area (Circle 2)))
             (println (area (Square 3)))
             (println (area (Empty))))"#);
    assert_eq!(result.output, "12\n9\n0\n");
}

#[test]
fn match_supports_guards_maps_and_rest_patterns() {
    let result = run(r#"(fn classify [n]
             (match n
               0 :zero
               v :when (> v 0) :positive
               _ :negative))
           (fn route [request]
             (match request
               {:method :get :path "/health"} "ok"
               {:method :post :body body} body
               _ "not found"))
           (fn head-of [v]
             (match v
               [] :empty
               [head ...tail] [head tail]))
           (fn main []
             (println [(classify 0) (classify 5) (classify -5)])
             (println (route {:method :get :path "/health"}))
             (println (route {:method :post :body "created"}))
             (println (route {:method :put}))
             (println (head-of [1 2 3]))
             (println (head-of [])))"#);
    assert_eq!(
        result.output,
        "[:zero :positive :negative]\nok\ncreated\nnot found\n[1 [2 3]]\n:empty\n"
    );
}

#[test]
fn a_failed_match_is_a_fault() {
    let result = run("(fn main [] (match 5 1 :one))");
    assert_eq!(result.diagnostics, vec!["match-failure"]);
}

#[test]
fn result_and_option_propagate_with_question_mark() {
    let result = run(r#"(fn parse [text] -> Result Int String
             (match text
               "1" (Ok 1)
               _ (Err "bad")))
           (fn double [text] -> Result Int String
             (let value (parse text) ?)
             (Ok (* 2 value)))
           (fn main []
             (println (double "1"))
             (println (double "x")))"#);
    assert_eq!(result.output, "(Ok 2)\n(Err \"bad\")\n");
}

#[test]
fn loop_and_recur_do_not_grow_the_stack() {
    let result = run(r#"(fn count-to [limit]
             (loop [n 0 total 0]
               (if (= n limit) total (recur (inc n) (+ total n)))))
           (fn main [] (println (count-to 100000)))"#);
    assert_eq!(result.output, "4999950000\n");
}

#[test]
fn self_recursion_in_tail_position_uses_recur() {
    let result = run(r#"(fn countdown [n]
             (if (= n 0) :done (recur (dec n))))
           (fn main [] (println (countdown 50000)))"#);
    assert_eq!(result.output, ":done\n");
}

#[test]
fn unbounded_non_tail_recursion_reports_a_stack_overflow() {
    let result = run("(fn f [n] (+ 1 (f (inc n))))\n(fn main [] (f 0))");
    assert_eq!(result.diagnostics, vec!["stack-overflow"]);
}

#[test]
fn named_arguments_and_defaults() {
    let result =
        run(r#"(fn connect [host: String :port port: Int = 5432 :tls? tls?: Bool = true] -> String
             (format "{host}:{port}/{tls?}"))
           (fn main []
             (println (connect "db"))
             (println (connect "db" :port 6543))
             (println (connect "db" :tls? false)))"#);
    assert_eq!(result.output, "db:5432/true\ndb:6543/true\ndb:5432/false\n");
}

#[test]
fn keywords_still_pass_positionally_to_functions_without_keyword_params() {
    let result = run(r#"(fn f [a b] [a b])
                        (fn main [] (println (f :key 1)))"#);
    assert_eq!(result.output, "[:key 1]\n");
}

#[test]
fn wrong_arity_is_reported_with_the_definition() {
    let result = run("(fn f [a b] a)\n(fn main [] (f 1))");
    assert_eq!(result.diagnostics, vec!["arity"]);
}

#[test]
fn mutation_requires_var() {
    let result = run(r#"(fn main []
             (var total 0)
             (set! total (+ total 5))
             (println total))"#);
    assert_eq!(result.output, "5\n");

    // `let` bindings are immutable; only `var` accepts `set!`.
    let immutable = run("(fn main [] (let x 1) (set! x 2))");
    assert_eq!(immutable.diagnostics, vec!["immutable-assign"]);

    let unbound = run("(fn main [] (set! nowhere 2))");
    assert_eq!(unbound.diagnostics, vec!["unbound-assign"]);
}

#[test]
fn cells_hold_explicit_mutable_state() {
    let result = run(r#"(fn main []
             (let counter (Cell.new 0))
             (counter.update inc)
             (counter.update inc)
             (println (counter.get)))"#);
    assert_eq!(result.output, "2\n");
}

#[test]
fn defer_runs_last_in_first_out() {
    let result = run(r#"(fn main []
             (defer (println "first deferred"))
             (defer (println "second deferred"))
             (println "body"))"#);
    assert_eq!(result.output, "body\nsecond deferred\nfirst deferred\n");
}

#[test]
fn conditions_are_caught_and_finally_always_runs() {
    let result = run(r#"(fn main []
             (try
               (throw "boom")
               (catch Condition c (println "caught" c))
               (finally (println "cleanup"))))"#);
    assert_eq!(result.output, "caught boom\ncleanup\n");
}

#[test]
fn an_uncaught_condition_escapes_as_a_diagnostic() {
    let result = run(r#"(fn main [] (throw "boom"))"#);
    assert_eq!(result.diagnostics, vec!["condition"]);
}

#[test]
fn protocols_dispatch_on_the_receiver() {
    let result = run(r#"(type User { name: String })
           (type Robot { serial: Int })
           (protocol Renderable (render [self] -> String))
           (impl Renderable User (fn render [u] (format "user {u.name}")))
           (impl Renderable Robot (fn render [r] (format "robot {r.serial}")))
           (fn main []
             (println (render (User { name "Ada" })))
             (println (render (Robot { serial 5 }))))"#);
    assert_eq!(result.output, "user Ada\nrobot 5\n");
}

#[test]
fn a_missing_implementation_is_reported() {
    let result = run(r#"(protocol Renderable (render [self] -> String))
           (fn main [] (render 42))"#);
    assert_eq!(result.diagnostics, vec!["missing-impl"]);
}

#[test]
fn prelude_macros_short_circuit() {
    let result = run(r#"(fn boom [] (throw "evaluated"))
           (fn main []
             (println (and false (boom)))
             (println (or true (boom)))
             (println (and 1 2 3))
             (println (cond false :a :else :fallback))
             (println (when true :yes))
             (println (unless true :no)))"#);
    assert_eq!(result.output, "false\ntrue\n3\n:fallback\n:yes\nnil\n");
}

#[test]
fn macros_are_hygienic() {
    let result = run(r#"(macro capture-attempt [form] `(do (let value 1) (+ ~form value)))
           (fn main []
             (let value 100)
             (println (capture-attempt value)))"#);
    // `value` inside the macro must not capture the caller's binding.
    assert_eq!(result.output, "101\n");
}

#[test]
fn macros_can_recurse_and_splice() {
    let result = run(r#"(macro my-list [...items]
             (if (empty? items) `[] `(conj (my-list ~@(rest items)) ~(get items 0))))
           (fn main [] (println (my-list 1 2 3)))"#);
    assert_eq!(result.output, "[3 2 1]\n");
}

#[test]
fn json_round_trips() {
    let result = run(r#"(module t (use std.json :as json))
           (fn main []
             (let encoded (json.encode {:name "Ada" :tags ["x" "y"] :n 3}))
             (println encoded)
             (println (json.decode encoded)))"#);
    assert_eq!(
        result.output,
        "{\"name\":\"Ada\",\"tags\":[\"x\",\"y\"],\"n\":3}\n(Ok {:name \"Ada\" :tags [\"x\" \"y\"] :n 3})\n"
    );
}

#[test]
fn string_library_basics() {
    let result = run(r#"(module t (use std.string :as string))
           (fn main []
             (println (string.split "a,b,c" ","))
             (println (string.join ["a" "b"] "-"))
             (println (string.upper "hey"))
             (println (string.parse-int "42"))
             (println (string.parse-int "x")))"#);
    let lines: Vec<&str> = result.output.lines().collect();
    assert_eq!(lines[0], "[\"a\" \"b\" \"c\"]");
    assert_eq!(lines[1], "a-b");
    assert_eq!(lines[2], "HEY");
    assert_eq!(lines[3], "(Ok 42)");
    assert!(lines[4].starts_with("(Err"));
}

#[test]
fn unknown_names_suggest_a_correction() {
    let mut session = korben_core::project::Session::bare(std::path::PathBuf::from("."));
    let _ = session.load_text("t", "(fn spam [] 1)\n(fn main [] (span))");
    let runtime = session.interp.module("t");
    let main = runtime.globals.borrow().get("main").cloned().unwrap();
    session.interp.current = runtime;
    let error = session
        .interp
        .apply(main, Vec::new(), korben_syntax::span::Span::synthetic())
        .err()
        .expect("expected an unbound-name error");
    let diagnostic =
        korben_core::project::flow_diagnostic(error, korben_syntax::span::Span::synthetic());
    assert_eq!(diagnostic.code.as_deref(), Some("unbound-name"));
    assert!(diagnostic.help.iter().any(|help| help.contains("spam")), "{:?}", diagnostic.help);
}

#[test]
fn private_declarations_are_not_importable() {
    let mut session = korben_core::project::Session::bare(std::path::PathBuf::from("."));
    let _ = session.load_text("helper", "(fn secret [] 1)\n(pub fn open [] 2)");
    let _ = session.load_text("consumer", "(module consumer (use helper [secret]))");
    let codes: Vec<String> =
        session.diagnostics.items.iter().filter_map(|item| item.code.clone()).collect();
    assert!(codes.contains(&"unknown-export".to_string()), "{codes:?}");
}

// korben-wzh
#[test]
fn a_top_level_def_binds_a_constant() {
    let result = run("(def limit 42)\n(fn main [] (println limit))");
    assert_eq!(result.diagnostics, Vec::<String>::new());
    assert_eq!(result.output, "42\n");
}

// korben-wzh
#[test]
fn a_def_may_carry_a_type_annotation() {
    let result = run("(def limit: Int 42)\n(fn main [] (println limit))");
    assert_eq!(result.diagnostics, Vec::<String>::new());
    assert_eq!(result.output, "42\n");
}

// korben-wzh
#[test]
fn a_def_without_a_value_is_reported() {
    let result = run("(def limit)\n(fn main [] 1)");
    assert_eq!(result.diagnostics, vec!["def-value"]);
}

// korben-0mo
/// Renaming within a filesystem is atomic, which is what makes
/// write-then-rename safe. Without it a program has no way to replace a file
/// without a window in which it is truncated.
#[test]
fn a_file_can_be_replaced_atomically() {
    let dir = std::env::temp_dir().join(format!("korben-rename-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("state.json");
    std::fs::write(&target, "old").unwrap();

    let result = run(&format!(
        r#"(module m (use std.fs :as fs))
           (fn main [] -> Unit !io
             (let target "{}")
             (let temp "{}")
             (match (fs.write-text temp "new")
               (Err e) (println "write failed:" e)
               (Ok _)
                 (match (fs.rename temp target)
                   (Err e) (println "rename failed:" e)
                   (Ok _) (println "replaced"))))"#,
        target.display(),
        dir.join("state.json.tmp").display()
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.output.trim_end(), "replaced");

    // The new content is in place and nothing was left beside it.
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    assert!(!dir.join("state.json.tmp").exists(), "the temporary file should be gone");

    let _ = std::fs::remove_dir_all(&dir);
}

// korben-0mo
#[test]
fn renaming_something_absent_reports_a_failure() {
    let dir = std::env::temp_dir().join(format!("korben-rename-miss-{}", std::process::id()));
    let result = run(&format!(
        r#"(module m (use std.fs :as fs))
           (fn main [] -> Unit !io
             (match (fs.rename "{}" "{}")
               (Ok _) (println "succeeded")
               (Err _) (println "failed")))"#,
        dir.join("not-there").display(),
        dir.join("destination").display()
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    // A rename that cannot happen must say so rather than report success.
    assert_eq!(result.output.trim_end(), "failed");
}
