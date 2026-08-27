//! Ownership, move, and borrow analysis.

mod common;
use common::check;

/// Ordinary immutable data is never move-checked, so ordinary code sees no
/// ownership diagnostics at all. This is the property that keeps the analysis
/// from being noise.
#[test]
fn ordinary_data_never_moves() {
    assert!(check(
        r#"(fn take [text: String] -> Int (len text))
           (fn f [] -> Int
             (let name "Ada")
             (let sum (take name))
             (+ sum (take name)))"#
    )
    .is_empty());

    assert!(check(
        r#"(type User { id: Int name: String })
           (fn show [user: User] -> String user.name)
           (fn f [] -> String
             (let user (User { id 1 name "Ada" }))
             (str (show user) (show user)))"#
    )
    .is_empty());
}

#[test]
fn a_resource_is_moved_by_passing_it() {
    let codes = check(
        r#"(module m (use std.fs :as fs))
           (fn consume [file: File] -> Unit !io (file.close))
           (fn f [] -> Unit !io
             (let file (fs.create "/tmp/x")?)
             (consume file)
             (consume file))"#,
    );
    assert_eq!(codes, vec!["use-after-move"]);
}

#[test]
fn a_borrow_leaves_the_value_usable() {
    assert!(check(
        r#"(module m (use std.fs :as fs))
           (fn peek [file: Borrow File] -> Bool !io (file.closed?))
           (fn consume [file: File] -> Unit !io (file.close))
           (fn f [] -> Unit !io
             (let file (fs.create "/tmp/x")?)
             (println (peek file))
             (println (peek file))
             (consume file))"#
    )
    .is_empty());
}

#[test]
fn a_move_on_one_branch_is_reported_as_a_possible_move() {
    let codes = check(
        r#"(module m (use std.fs :as fs))
           (fn consume [file: File] -> Unit !io (file.close))
           (fn f [flag: Bool] -> Unit !io
             (let file (fs.create "/tmp/x")?)
             (if flag (consume file) nil)
             (consume file))"#,
    );
    assert_eq!(codes, vec!["maybe-moved"]);
}

#[test]
fn a_move_on_every_branch_is_accepted_and_then_final() {
    // Moving on both paths is fine; the value is simply gone afterwards.
    assert!(check(
        r#"(module m (use std.fs :as fs))
           (fn consume [file: File] -> Unit !io (file.close))
           (fn f [flag: Bool] -> Unit !io
             (let file (fs.create "/tmp/x")?)
             (if flag (consume file) (consume file)))"#
    )
    .is_empty());

    let codes = check(
        r#"(module m (use std.fs :as fs))
           (fn consume [file: File] -> Unit !io (file.close))
           (fn f [flag: Bool] -> Unit !io
             (let file (fs.create "/tmp/x")?)
             (if flag (consume file) (consume file))
             (consume file))"#,
    );
    assert_eq!(codes, vec!["use-after-move"]);
}

#[test]
fn moving_inside_a_loop_is_reported() {
    let codes = check(
        r#"(module m (use std.fs :as fs))
           (fn consume [file: File] -> Unit !io (file.close))
           (fn f [] -> Unit !io
             (let file (fs.create "/tmp/x")?)
             (loop [n 0]
               (if (< n 3) (do (consume file) (recur (inc n))) nil)))"#,
    );
    assert_eq!(codes, vec!["move-in-loop"]);
}

#[test]
fn borrowing_inside_a_loop_is_fine() {
    assert!(check(
        r#"(module m (use std.fs :as fs))
           (fn peek [file: Borrow File] -> Bool !io (file.closed?))
           (fn f [] -> Unit !io
             (let file (fs.create "/tmp/x")?)
             (loop [n 0]
               (if (< n 3) (do (println (peek file)) (recur (inc n))) nil)))"#
    )
    .is_empty());
}

#[test]
fn a_resource_cannot_be_cloned() {
    let codes = check(
        r#"(module m (use std.fs :as fs))
           (fn consume [file: File] -> Unit !io (file.close))
           (fn f [] -> Unit !io
             (let file (fs.create "/tmp/x")?)
             (consume (clone file)))"#,
    );
    assert_eq!(codes, vec!["clone-resource"]);
}

#[test]
fn a_scoped_resource_cannot_escape_its_scope() {
    let codes = check(
        r#"(module m (use std.fs :as fs))
           (fn f [path: String] -> File !io
             (with file (fs.create path)?
               file))"#,
    );
    assert_eq!(codes, vec!["borrow-escape"]);
}

#[test]
fn implementing_drop_makes_a_type_resource_bearing() {
    let codes = check(
        r#"(type Connection { name: String })
           (impl Drop Connection (fn drop [c] (println c.name)))
           (fn consume [c: Connection] -> String c.name)
           (fn f [] -> String
             (let c (Connection { name "db" }))
             (str (consume c) (consume c)))"#,
    );
    assert_eq!(codes, vec!["use-after-move"]);
}

#[test]
fn a_type_containing_a_resource_is_itself_resource_bearing() {
    let codes = check(
        r#"(module m (use std.fs :as fs))
           (type Session { handle: File name: String })
           (fn consume [s: Session] -> String s.name)
           (fn f [] -> String !io
             (let s (Session { handle (fs.create "/tmp/x")? name "one" }))
             (str (consume s) (consume s)))"#,
    );
    assert_eq!(codes, vec!["use-after-move"]);
}

#[test]
fn an_exclusive_borrow_cannot_alias_another_argument() {
    let codes = check(
        r#"(type Buffer { text: String })
           (fn append [target: BorrowMut Buffer source: Borrow Buffer] -> Unit
             (println target.text source.text))
           (fn f [] -> Unit
             (let buffer (Buffer { text "x" }))
             (append buffer buffer))"#,
    );
    assert_eq!(codes, vec!["exclusive-borrow"]);
}

#[test]
fn distinct_bindings_may_be_borrowed_together() {
    assert!(check(
        r#"(type Buffer { text: String })
           (fn append [target: BorrowMut Buffer source: Borrow Buffer] -> Unit
             (println target.text source.text))
           (fn f [] -> Unit
             (let a (Buffer { text "a" }))
             (let b (Buffer { text "b" }))
             (append a b))"#
    )
    .is_empty());
}

#[test]
fn unsafe_functions_are_contained() {
    let source = "(unsafe fn poke [] -> Int 42)\n";
    assert_eq!(
        check(&format!("{source}(fn caller [] -> Int (poke))")),
        vec!["unsafe-call"],
        "safe code must not call an unsafe function"
    );
    assert!(
        check(&format!("{source}(fn caller [] -> Int (unsafe (poke)))")).is_empty(),
        "an unsafe block is an explicit opt-in"
    );
    assert!(
        check(&format!("{source}(unsafe fn caller [] -> Int (poke))")).is_empty(),
        "an unsafe function may call unsafe code directly"
    );
}

#[test]
fn a_borrow_may_not_cross_a_task_boundary() {
    let codes = check(
        r#"(module m (use std.fs :as fs))
           (fn peek [file: Borrow File] -> Bool !io (file.closed?))
           (fn f [file: Borrow File] -> Unit !async !io
             (task-scope scope
               (println (peek file))))"#,
    );
    assert_eq!(codes, vec!["borrow-across-task"]);
}

#[test]
fn reassignment_makes_a_moved_binding_usable_again() {
    assert!(check(
        r#"(module m (use std.fs :as fs))
           (fn consume [file: File] -> Unit !io (file.close))
           (fn f [] -> Unit !io
             (var file (fs.create "/tmp/a")?)
             (consume file)
             (set! file (fs.create "/tmp/b")?)
             (consume file))"#
    )
    .is_empty());
}

#[test]
fn diagnostics_point_at_both_the_move_and_the_use() {
    let mut session = korben_core::project::Session::bare(std::path::PathBuf::from("."));
    let _ = session.load_text(
        "t",
        r#"(module t (use std.fs :as fs))
           (fn consume [file: File] -> Unit !io (file.close))
           (fn f [] -> Unit !io
             (let file (fs.create "/tmp/x")?)
             (consume file)
             (consume file))"#,
    );
    korben_core::infer::check_session(&mut session, false);
    let diagnostic = session
        .diagnostics
        .items
        .iter()
        .find(|item| item.code.as_deref() == Some("use-after-move"))
        .expect("expected a use-after-move diagnostic");

    assert!(diagnostic.message.contains("`file` was moved"), "{}", diagnostic.message);
    assert!(diagnostic.primary.is_some(), "the use must be the primary span");
    assert!(
        diagnostic.secondary.iter().any(|label| label.message.contains("moved here")),
        "the move site must be shown"
    );
    assert!(
        diagnostic.secondary.iter().any(|label| label.message.contains("owned resource")),
        "the binding's category must be explained"
    );
    assert!(!diagnostic.help.is_empty(), "a fix must be suggested");
}
