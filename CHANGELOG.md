# Changelog

## 0.2.0 — Native backend

`korben build` now produces a standalone native executable.

### Added

- **Core IR** with full name resolution, printed by `korben build --emit ir`.
  Every reference is classified as a local, a module global, a constructor, a
  protocol method, or a runtime builtin.
- **`korben-runtime`**: the value representation, call dispatch, argument
  binding, construction, protocol dispatch, JSON, and the standard library. The
  interpreter and generated code both use it, so the two execution modes share
  observable semantics by construction.
- **Native code generation**: typed core IR lowered to Rust and compiled by an
  isolated cargo pipeline, per specification 18.3. `korben build --emit rust`
  prints the generated source. A release binary of the language tour is 587 KB.
- **Differential tests** that compile a corpus both ways and require
  byte-identical output, runtime fault reports included.
- Native binaries embed a source table, so a runtime fault reports against the
  Korben source that caused it.

### Changed

- `and` and `or` are core forms rather than prelude macros. Their result may be
  any of their operands' types, which an expansion into `if` cannot express.
- `korben run` type-checks before running, so both execution modes accept the
  same programs.
- `--strict-api` is its own flag; `--release` no longer implies it.
- A heterogeneous vector literal infers as a tuple, per specification 9.5.
- A module declaration that disagrees with its path is now an error.
- Map literals widen instead of rejecting heterogeneous entries.
- `korben build` emits a native executable instead of a `.kbx` bundle.

### Fixed

- Comparison and arithmetic accept the chains they always evaluated: `(< 1 2 3)`
  and `(+ 1 2 3)` no longer fail the arity check.
- `{{` and `}}` are literal braces in an interpolated string.
- Protocol implementations are no longer treated as public API by
  `--strict-api`.

## 0.1.0 — Milestone A

First implementation of the Korben specification: a usable core plus the
standard toolchain.

### Language

- Reader producing syntax objects with source spans, hygiene scopes, and
  comments: lists, vectors, maps, sets, keywords, raw strings, nesting block
  comments, tagged literals, and the `'` `` ` `` `~` `~@` `#'` `#(...)`
  shortcuts.
- Modules, explicit imports, and `pub` visibility with deterministic resolution.
- Functions with inference-first annotations, named arguments with defaults,
  closures, and guaranteed tail calls via `loop`/`recur` and self-recursion.
- Records, enums, newtypes, tuples, and immutable collections; `var` and `Cell`
  for explicit mutation.
- Pattern matching with literals, constructors, rest patterns, map and record
  patterns, and guards.
- `Option`, `Result`, and the postfix `?` propagation operator.
- Typed conditions (`try`/`catch`/`finally`, `throw`), `with` for deterministic
  cleanup, and `defer` in last-in-first-out order.
- Protocols with explicit implementations and receiver dispatch.
- Hygienic compile-time macros; the control-flow prelude is written in Korben.

### Analysis

- Hindley–Milner inference with let-polymorphism and structural records that
  unify with the nominal types they match.
- Effect inference and checking for `!io`, `!async`, `!alloc`, `!ffi`, `!unsafe`.
- Exhaustiveness and unreachable-arm checking for enum matches.
- `--strict-api` mode requiring complete types on exported functions.
- Lints for unused bindings, undocumented public functions, and unsafe
  boundaries.

### Toolchain

- `new`, `init`, `run`, `dev`, `check`, `test`, `fmt`, `lint`, `repl`, `expand`,
  `doc`, `inspect`, `doctor`, and `build`.
- Annotated terminal diagnostics and stable JSON diagnostics.
- A canonical, idempotent, comment-preserving formatter.
- A project-aware REPL with `:type`, `:expand`, `:reload`, `:tests`, `:doc`.
- Markdown documentation plus a machine-readable `api.json`.
- `.kbx` build artifacts that round-trip through `korben run`.

### Not in this release

Native code generation, the async runtime, ownership and borrow analysis, FFI,
package management, and the language server. Commands for those report the
milestone they land in rather than failing obscurely.
