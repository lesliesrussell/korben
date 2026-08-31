//! Type, effect, exhaustiveness, and lint analysis.

// korben-6bc

mod common;
use common::{check, check_messages, check_strict, lint};

#[test]
fn well_typed_code_produces_no_errors() {
    assert!(check(
        r#"(type User { id: Int name: String })
           (fn rename [user: User new: String] -> User (assoc user :name new))
           (fn add [a: Int b: Int] -> Int (+ a b))
           (fn main [] -> Unit (println (add 1 2)))"#
    )
    .is_empty());
}

#[test]
fn argument_type_mismatches_are_reported() {
    assert_eq!(
        check("(fn add [a: Int b: Int] -> Int (+ a b))\n(fn f [] (add 1 \"two\"))"),
        vec!["type-mismatch"]
    );
}

#[test]
fn return_type_mismatches_are_reported() {
    assert_eq!(check("(fn f [] -> Int \"text\")"), vec!["type-mismatch"]);
}

#[test]
fn a_uniform_vector_literal_is_a_vec() {
    assert!(check("(fn f [] -> Vec Int [1 2 3])").is_empty());
    assert_eq!(check("(fn f [] -> Vec Int [1 2 \"three\"])"), vec!["type-mismatch"]);
}

#[test]
fn a_mixed_vector_literal_is_a_tuple() {
    // Specification 9.5: the language distinguishes fixed-length heterogeneous
    // tuples from homogeneous vectors by inferred context.
    assert!(check("(fn f [] [1 \"two\" true])").is_empty());
    assert!(check("(fn f [] -> [Int String] [1 \"two\"])").is_empty());
}

#[test]
fn if_branches_must_agree() {
    assert_eq!(check("(fn f [c] (if c 1 \"two\"))"), vec!["type-mismatch"]);
}

#[test]
fn a_nil_branch_does_not_constrain_the_other() {
    // This is the shape `when`, `cond`, and `when-let` expand into.
    assert!(check("(fn f [c] (if c :yes nil))").is_empty());
}

#[test]
fn non_exhaustive_matches_name_the_missing_cases() {
    let codes = check(
        r#"(enum Color (Red) (Green) (Blue))
           (fn name-of [c: Color] -> String (match c (Red) "red" (Green) "green"))"#,
    );
    assert_eq!(codes, vec!["non-exhaustive"]);
}

#[test]
fn a_wildcard_arm_makes_a_match_exhaustive() {
    assert!(check(
        r#"(enum Color (Red) (Green) (Blue))
           (fn name-of [c: Color] -> String (match c (Red) "red" _ "other"))"#
    )
    .is_empty());
}

#[test]
fn unreachable_arms_are_a_warning_not_an_error() {
    let source = r#"(enum Color (Red) (Green))
                    (fn f [c: Color] (match c _ 1 (Red) 2))"#;
    assert!(check(source).is_empty());
    let mut session = korben_core::project::Session::bare(std::path::PathBuf::from("."));
    let _ = session.load_text("t", source);
    korben_core::infer::check_session(&mut session, false);
    assert!(session
        .diagnostics
        .items
        .iter()
        .any(|item| item.code.as_deref() == Some("unreachable")));
}

#[test]
fn unknown_fields_are_reported() {
    assert_eq!(
        check(
            r#"(type User { id: Int })
               (fn f [u: User] -> Int u.missing)"#
        ),
        vec!["unknown-field"]
    );
}

#[test]
fn record_construction_checks_field_names_and_types() {
    assert_eq!(
        check("(type User { id: Int name: String })\n(fn f [] (User { id \"seven\" name \"x\" }))"),
        vec!["type-mismatch"]
    );
    assert_eq!(
        check("(type User { id: Int })\n(fn f [] (User { id 1 extra 2 }))"),
        vec!["unknown-field"]
    );
    assert_eq!(
        check("(type User { id: Int name: String })\n(fn f [] (User { id 1 }))"),
        vec!["missing-field"]
    );
}

#[test]
fn question_mark_requires_a_result_or_option() {
    assert_eq!(check("(fn f [] -> Int (let x (+ 1 2) ?) x)"), vec!["propagate-type"]);
}

#[test]
fn undeclared_effects_are_reported() {
    assert_eq!(
        check("(fn f [] -> Unit (println \"hi\"))"),
        Vec::<String>::new(),
        "a private function with no effect annotation is inferred, not rejected"
    );
    assert_eq!(
        check("(pub fn f [] -> Unit (println \"hi\"))"),
        vec!["undeclared-effect"],
        "a public function must declare the effects it performs"
    );
    assert!(check("(pub fn f [] -> Unit !io (println \"hi\"))").is_empty());
}

#[test]
fn strict_api_requires_complete_public_signatures() {
    let source = "(pub fn f [x] x)";
    assert!(check(source).is_empty());
    let strict = check_strict(source);
    assert_eq!(strict.len(), 2, "{strict:?}");
    assert!(strict.iter().all(|code| code == "strict-api"));
}

#[test]
fn arity_is_checked_at_the_call_site() {
    assert_eq!(check("(fn f [a b] a)\n(fn g [] (f 1))"), vec!["arity"]);
}

#[test]
fn variadic_prelude_functions_accept_extra_arguments() {
    assert!(check("(fn f [] (println \"a\" \"b\" \"c\"))").is_empty());
}

#[test]
fn keyword_parameters_do_not_count_toward_arity() {
    assert!(check(
        "(fn connect [host: String :port port: Int = 1] -> String host)\n(fn f [] (connect \"h\"))"
    )
    .is_empty());
}

#[test]
fn unknown_types_are_flagged_as_warnings() {
    let mut session = korben_core::project::Session::bare(std::path::PathBuf::from("."));
    let _ = session.load_text("t", "(fn f [x: Nonexistent] -> Int 1)");
    korben_core::infer::check_session(&mut session, false);
    assert!(session
        .diagnostics
        .items
        .iter()
        .any(|item| item.code.as_deref() == Some("unknown-type")));
    assert!(!session.diagnostics.has_errors());
}

#[test]
fn lints_catch_unused_bindings() {
    let codes = lint("(fn f [used unused] (+ used 1))");
    assert!(codes.contains(&"unused-binding".to_string()), "{codes:?}");

    let clean = lint("(fn f [used _ignored] (+ used 1))");
    assert!(!clean.contains(&"unused-binding".to_string()), "{clean:?}");
}

#[test]
fn lints_ask_for_documentation_on_public_functions() {
    let codes = lint("(pub fn f [] 1)");
    assert!(codes.contains(&"missing-docs".to_string()), "{codes:?}");
}

#[test]
fn lints_flag_unsafe_boundaries() {
    let codes = lint("(unsafe fn poke [] 1)");
    assert!(codes.contains(&"unsafe-boundary".to_string()), "{codes:?}");
}

// ------------------------------------------------------------- name resolution

// korben-4io
#[test]
fn an_undefined_name_is_reported_by_the_checker() {
    // `korben check` never runs the evaluator, so if the checker stays quiet
    // about this the mistake reaches a run before anything reports it.
    assert_eq!(check("(fn f [] -> Int (totally-undefined-fn 1))"), vec!["unbound-name"]);
}

#[test]
fn an_undefined_name_inside_a_test_is_reported() {
    assert_eq!(
        check("(fn f [] -> Int 1)\n(test \"t\" (assert-eq 1 (missing-helper)))"),
        vec!["unbound-name"]
    );
}

#[test]
fn a_near_miss_suggests_the_name_it_missed() {
    let messages = check_messages("(fn slug [] -> Int 1)\n(fn f [] -> Int (slugg))");
    assert_eq!(messages, vec!["`slugg` is not defined -- did you mean `slug`?"]);
}

#[test]
fn a_misspelled_builtin_suggests_the_builtin() {
    let messages = check_messages("(fn f [] -> Unit !io (printn \"x\"))");
    assert_eq!(messages, vec!["`printn` is not defined -- did you mean `println`?"]);
}

#[test]
fn a_distant_name_gets_no_suggestion() {
    // A suggestion further away than a third of the name is noise, not help.
    let messages = check_messages("(fn f [] -> Int (zzzzzzzzzz))");
    assert_eq!(messages, vec!["`zzzzzzzzzz` is not defined -- "]);
}

#[test]
fn a_member_a_module_does_not_have_is_reported() {
    let messages = check_messages(
        "(module m (use std.string :as string))\n(fn f [] -> String (string.lowr \"X\"))",
    );
    assert_eq!(messages, vec!["`std.string` has no member `lowr` -- did you mean `lower`?"]);
}

#[test]
fn names_that_do_resolve_stay_quiet() {
    // Locals, module declarations, prelude builtins, module members, and a type
    // addressed like a module all resolve -- none of them may be reported.
    assert!(check(
        r#"(module m (use std.string :as string))
           (fn helper [text: String] -> String (string.upper text))
           (fn f [] -> Unit !io
             (let cell (Cell.new 1))
             (let value (helper "x"))
             (println value (cell.get)))"#
    )
    .is_empty());
}

// ----------------------------------------------------- duplicate declarations

// korben-707
#[test]
fn a_name_declared_twice_is_reported() {
    // The two execution modes disagreed about this: the interpreter kept the
    // later definition, and the native backend refused to compile.
    assert_eq!(check("(fn f [] -> Int 1)\n(fn f [] -> Int 2)"), vec!["duplicate-definition"]);
}

#[test]
fn a_duplicate_points_at_both_declarations() {
    let messages = check_messages("(fn f [] -> Int 1)\n(fn f [] -> Int 2)");
    assert_eq!(
        messages,
        vec!["`f` is declared twice in this module -- rename one of them, or remove the one that is not wanted"]
    );
}

#[test]
fn duplicates_are_reported_for_every_kind_of_declaration() {
    for source in [
        "(type T {a: Int})\n(type T {b: Int})",
        "(enum E (A x: Int) (A y: Int))",
        // A constructor and a function share the value namespace.
        "(enum E (F x: Int))\n(fn F [] -> Int 1)",
        // As do a record's constructor and a function.
        "(type R {a: Int})\n(fn R [] -> Int 1)",
        // And two protocols declaring the same method.
        "(protocol P (go [self] -> Int))\n(protocol Q (go [self] -> Int))",
    ] {
        assert_eq!(check(source), vec!["duplicate-definition"], "not reported for:\n{source}");
    }
}

#[test]
fn a_duplicate_protocol_reports_its_name_and_its_methods() {
    // Two mistakes, not one: the protocol's name collides, and so does every
    // method it declares.
    assert_eq!(
        check("(protocol P (go [self] -> Int))\n(protocol P (go [self] -> Int))"),
        vec!["duplicate-definition", "duplicate-definition"]
    );
}

#[test]
fn a_type_and_its_constructor_are_not_a_duplicate() {
    // `(type Point ...)` declares the type `Point` and the constructor
    // `Point`. That is one declaration in each namespace, not a collision.
    assert!(check("(type Point {x: Int y: Int})\n(fn f [] -> Point (Point {x 1 y 2}))").is_empty());
}

#[test]
fn an_enum_and_a_type_of_the_same_name_in_different_modules_are_fine() {
    // Modules have their own namespaces, so the same name in two of them is
    // exactly what per-module scoping is for.
    let mut session = korben_core::project::Session::bare(std::path::PathBuf::from("."));
    let _ = session.load_text("one", "(module one)\n(pub fn handle [] -> Int 1)\n");
    let _ = session.load_text("two", "(module two)\n(pub fn handle [] -> Int 2)\n");
    korben_core::infer::check_session(&mut session, false);
    let codes: Vec<String> = session
        .diagnostics
        .items
        .iter()
        .filter(|item| item.is_error())
        .filter_map(|item| item.code.clone())
        .collect();
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn three_declarations_of_a_name_report_twice() {
    // Each redeclaration is its own mistake to fix.
    assert_eq!(
        check("(fn f [] -> Int 1)\n(fn f [] -> Int 2)\n(fn f [] -> Int 3)"),
        vec!["duplicate-definition", "duplicate-definition"]
    );
}

// korben-wzh
#[test]
fn a_def_annotation_is_checked_against_its_value() {
    assert_eq!(check("(def limit: Int \"forty-two\")\n(fn main [] limit)"), vec!["type-mismatch"]);
    assert!(check("(def limit: Int 42)\n(fn main [] limit)").is_empty());
}

// korben-95f
#[test]
fn a_function_a_macro_makes_unreachable_is_reported() {
    // Expansion runs before evaluation and a call site cannot tell the two
    // apart, so the macro always wins and the function is dead code.
    assert_eq!(
        lint("(fn twice [n: Int] -> Int (* 2 n))\n(macro twice [form] `(do ~form ~form))"),
        vec!["shadowed-by-macro"]
    );
    // The order they are written in does not change which one wins.
    assert_eq!(
        lint("(macro twice [form] `(do ~form ~form))\n(fn twice [n: Int] -> Int (* 2 n))"),
        vec!["shadowed-by-macro"]
    );
}

// korben-95f
#[test]
fn a_constant_a_macro_makes_unreachable_is_reported() {
    assert_eq!(lint("(def limit 42)\n(macro limit [] `1)"), vec!["shadowed-by-macro"]);
}

// korben-95f
#[test]
fn a_macro_or_a_function_on_its_own_is_not_reported() {
    assert!(lint("(macro twice [form] `(do ~form ~form))").is_empty());
    assert!(lint("(fn twice [n: Int] -> Int (* 2 n))").is_empty());
    // A different name is not a collision, however similar.
    assert!(lint("(fn twice-over [n: Int] -> Int (* 2 n))\n(macro twice [form] `~form)").is_empty());
}

// korben-3cb
#[test]
fn a_key_the_map_cannot_hold_is_reported() {
    // A String-keyed map indexed with a Keyword, which is the shape that
    // arises when JSON data (Keyword-keyed) and an HTTP request's query
    // (String-keyed) are handled in the same function.
    assert_eq!(
        check(r#"(fn f [m: Map String String] -> String (get m :status "default"))"#),
        vec!["map-key-type"]
    );
    // And the other direction.
    assert_eq!(
        check(r#"(fn f [m: Map Keyword Int] -> Int (get m "status" 0))"#),
        vec!["map-key-type"]
    );
}

// korben-3cb
#[test]
fn a_key_the_map_can_hold_is_accepted() {
    assert!(check(r#"(fn f [m: Map String String] -> String (get m "status" "d"))"#).is_empty());
    assert!(check(r#"(fn f [m: Map Keyword Int] -> Int (get m :status 0))"#).is_empty());
}

// korben-3cb
#[test]
fn a_map_key_check_stays_quiet_where_the_answer_is_not_settled() {
    // Not a map: `get` is overloaded across Vec and Record too, and neither
    // takes the map's idea of a key.
    assert!(check(r#"(fn f [v: Vec String] -> String (get v 0 "d"))"#).is_empty());
    // A map whose key type is still open must not be guessed at.
    assert!(check(r#"(fn f [m] (get m :anything "d"))"#).is_empty());
}

// korben-3cb
#[test]
fn a_local_binding_named_get_is_not_the_builtin() {
    // Shadowing must not be mistaken for the builtin and reported against.
    assert!(check(
        r#"(fn f [m: Map String String] -> String
             (let get (fn [a b c] "shadowed"))
             (get m :status "d"))"#
    )
    .is_empty());
}

// korben-gs0
#[test]
fn a_runtime_builtin_checks_its_arguments() {
    // Every builtin used to infer as `Unknown`, so none of this was caught.
    assert_eq!(
        check(
            r#"(module m (use std.string :as string))
                 (fn f [v: Vec String] -> String (string.upper v))"#
        ),
        vec!["type-mismatch"]
    );
    assert_eq!(
        check(
            r#"(module m (use std.math :as math))
                 (fn f [] -> Int (math.abs "not a number"))"#
        ),
        vec!["type-mismatch"]
    );
}

// korben-gs0
#[test]
fn a_runtime_builtin_checks_its_result() {
    assert_eq!(
        check(
            r#"(module m (use std.string :as string))
                 (fn f [s: String] -> Int (string.upper s))"#
        ),
        vec!["type-mismatch"]
    );
    // `parse-int` answers with a Result, not a bare Int.
    assert_eq!(
        check(
            r#"(module m (use std.string :as string))
                 (fn f [s: String] -> Int (string.parse-int s))"#
        ),
        vec!["type-mismatch"]
    );
}

// korben-gs0
#[test]
fn a_runtime_builtin_carries_its_effects() {
    // Reading a file is `!io`. Effects are inferred privately and required at
    // a public boundary, so this is reported on a `pub fn` and not on a
    // private one.
    assert_eq!(
        check(
            r#"(module m (use std.fs :as fs))
                 (pub fn f [p: String] -> Result String IoError (fs.read-text p))"#
        ),
        vec!["undeclared-effect"]
    );
    assert!(check(
        r#"(module m (use std.fs :as fs))
           (pub fn f [p: String] -> Result String IoError !io (fs.read-text p))"#
    )
    .is_empty());
}

// korben-gs0
#[test]
fn correct_uses_of_typed_builtins_stay_quiet() {
    assert!(check(
        r#"(module m (use std.string :as string) (use std.math :as math))
           (fn f [s: String] -> String (string.upper (string.trim s)))
           (fn g [] -> Int (math.abs -3))
           (fn h [s: String] -> Vec String (string.split s ","))"#
    )
    .is_empty());
}

// korben-gs0
#[test]
fn an_overloaded_builtin_is_left_untyped() {
    // `get` and friends dispatch on the collection, so they have no single
    // signature. They must stay unchecked rather than be given a wrong one.
    assert!(check(r#"(fn f [v: Vec String] -> String (get v 0 "d"))"#).is_empty());
    assert!(check(r#"(fn f [m: Map String Int] -> Int (get m "k" 0))"#).is_empty());
}

// korben-vok
/// Inference used to be run and discarded. What it concluded is now kept on
/// the session, which is what lets anything downstream ask for a type instead
/// of running inference again and risking a different answer.
#[test]
fn the_checker_keeps_what_it_concluded() {
    let source = r#"(fn f [n: Int] -> String
                      (let doubled (* n 2))
                      (let label "count")
                      label)"#;
    let mut session = korben_core::project::Session::bare(std::path::PathBuf::from("."));
    let _ = session.load_text("t", source);
    korben_core::infer::check_session(&mut session, false);
    assert!(session.diagnostics.items.iter().all(
        |item| item.code.is_none() || !matches!(item.severity, korben_syntax::Severity::Error)
    ));

    // Every expression in the source is charted, and the types are the ones a
    // reader would expect rather than unresolved variables.
    let charted: Vec<String> = session.types.values().map(|ty| ty.to_string()).collect();
    assert!(!charted.is_empty(), "nothing was recorded");
    assert!(charted.iter().any(|ty| ty == "Int"), "the arithmetic should be Int: {charted:?}");
    assert!(charted.iter().any(|ty| ty == "String"), "the label should be String: {charted:?}");
}

// korben-vok
/// A type only settled by a later constraint must be recorded settled, not as
/// the variable it was when the expression was first walked.
#[test]
fn a_type_learned_later_is_recorded_resolved() {
    // `x` has no annotation; it is only pinned to Int by the call below it.
    let source = r#"(fn takes-int [n: Int] -> Int n)
                    (fn f [] -> Int
                      (let x 1)
                      (takes-int x))"#;
    let mut session = korben_core::project::Session::bare(std::path::PathBuf::from("."));
    let _ = session.load_text("t", source);
    korben_core::infer::check_session(&mut session, false);

    let charted: Vec<String> = session.types.values().map(|ty| ty.to_string()).collect();
    assert!(
        !charted.iter().any(|ty| ty.starts_with('?') || ty == "_"),
        "an unresolved variable was recorded: {charted:?}"
    );
}

// korben-vok
/// The chart is only useful to the code generator if the spans it is keyed by
/// survive lowering. If they did not, `type_of` would answer `None` for
/// everything and the whole thing would be an expensive no-op.
#[test]
fn the_lowered_program_can_be_asked_for_a_type() {
    let source = r#"(fn f [n: Int] -> Int
                      (let doubled (* n 2))
                      doubled)
                    (fn main [] -> Unit !io (println (f 21)))"#;
    let mut session = korben_core::project::Session::bare(std::path::PathBuf::from("."));
    let _ = session.load_text("t", source);
    korben_core::infer::check_session(&mut session, false);
    let program = korben_core::ir::lower_session(&session, "t").expect("lowering");

    assert!(!program.types.is_empty(), "the program carried no types");

    // Walk the lowered functions and ask about the spans they actually hold.
    // At least some must answer, or the spans did not survive.
    let mut answered = Vec::new();
    for module in &program.modules {
        for function in &module.functions {
            collect_answers(&function.body, &program, &mut answered);
        }
    }
    assert!(
        answered.iter().any(|ty| ty == "Int"),
        "no lowered expression could be asked its type: {answered:?}"
    );
}

#[cfg(test)]
fn collect_answers(
    block: &korben_core::ir::Block,
    program: &korben_core::ir::Program,
    out: &mut Vec<String>,
) {
    use korben_core::ir::Stmt;
    for statement in &block.stmts {
        let expr = match statement {
            Stmt::Let { value, .. } | Stmt::Var { value, .. } => value,
            Stmt::Expr(expr) => expr,
            Stmt::Defer { .. } => continue,
        };
        if let Some(ty) = program.type_of(expr.span()) {
            out.push(ty.to_string());
        }
    }
}
