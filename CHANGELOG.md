# Changelog

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
