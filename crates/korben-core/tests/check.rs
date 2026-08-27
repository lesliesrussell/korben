//! Type, effect, exhaustiveness, and lint analysis.

// korben-6bc

mod common;
use common::{check, check_strict, lint};

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
fn vector_elements_must_agree() {
    assert_eq!(check("(fn f [] [1 \"two\"])"), vec!["type-mismatch"]);
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
