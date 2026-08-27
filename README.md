# Korben

A compiled, statically typed, ownership-safe Lisp for native software, with a
one-command modern developer experience.

`spec.md` is the full language and platform specification. This repository is the
Rust implementation of it. **This tree implements Milestone A ("Usable core") of
the roadmap in specification section 27, plus parts of Milestone B.** See
[Status](#status) for exactly what does and does not work yet.

```sh
cargo build --release
./target/release/korben new hello-service
cd hello-service
./target/release/korben dev
```

## The toolchain

One executable covers the standard workflow. Every command works on a project
(`korben.toml` plus `src/`) or on a single `.kb` file.

| Command | What it does |
| --- | --- |
| `korben new <name> [--template cli\|lib\|service]` | Create a project |
| `korben init` | Add a manifest to the current directory |
| `korben run [entry] [-- args...]` | Run the project, a module, a file, or a build artifact |
| `korben dev` | Check, test, then run |
| `korben check [--json] [--strict-api]` | Type, effect, and exhaustiveness analysis |
| `korben test [filter] [--json]` | Unit and property tests |
| `korben fmt [--check] [paths...]` | Canonical formatting |
| `korben lint [--json]` | Built-in lint rules |
| `korben repl` | Project-aware REPL |
| `korben expand <file>` | Print macro expansion through the formatter |
| `korben doc [--out <dir>]` | Markdown docs plus a machine-readable `api.json` |
| `korben inspect` | The resolved project model |
| `korben doctor` | Toolchain and project health |
| `korben build [--release]` | Produce a runnable artifact |

Diagnostics carry a concise explanation, source spans, expected-versus-found
types in source-level names, and confidence-safe suggestions. `--json` emits the
stable machine-readable form for editors and CI.

```
error[type-mismatch]: type mismatch in an argument
  --> src/main.kb:10:27
   |
10 | (fn oops [] -> Int (add 1 "two"))
   |                           ^^^^^ expected `Int`, found `String`
   |
  note: expected: Int
  note:    found: String
```

## The language today

`examples/tour.kb` exercises everything below; `examples/greeting.kb` is the
specification's reference example adapted to what exists.

- S-expression reader with vectors, maps, sets, keywords, raw strings, nesting
  block comments, tagged literals (`#uuid`, `#date`, `#duration`), and the
  reader shortcuts `'`, `` ` ``, `~`, `~@`, `#'`, `#(...)`.
- Modules with explicit imports (`:as`, `:only`, `[names]`), `pub` visibility,
  and deterministic path resolution.
- Functions with optional annotations, named arguments with defaults, closures,
  `#(.field %)` shorthand, and guaranteed tail calls through `loop`/`recur` and
  self-recursion.
- Records, enums, newtypes, tuples, vectors, maps, and sets. Collections are
  immutable; `var` and `Cell` make mutation explicit.
- Pattern matching over literals, constructors, vectors with rest patterns,
  maps, records, and guards — with exhaustiveness and unreachability checking.
- `Option` and `Result` with the postfix `?` propagation operator. No `null`.
- Typed conditions with `try`/`catch`/`finally`, `throw`, deterministic cleanup
  through `with`, and `defer` in last-in-first-out order.
- Protocols with explicit implementations and dispatch on the receiver.
- Hygienic macros. Macros are functions from syntax objects to syntax objects,
  run at compile time; identifiers a template binds cannot capture a caller's.
  The prelude (`when`, `unless`, `and`, `or`, `cond`, `if-let`, `when-let`) is
  written in Korben, so `korben expand` shows exactly what your code becomes.
- Hindley–Milner type inference with let-polymorphism, structural records that
  unify with the nominal types they match, effect inference (`!io`, `!async`,
  `!alloc`, `!ffi`, `!unsafe`), and `--strict-api` for complete public
  signatures.
- Standard library: `std.core`, `std.string`, `std.math`, `std.io`, `std.fs`,
  `std.json`, `std.log`, `std.time`, `std.process`, `std.test`, `std.syntax`.

Inference is deliberately conservative. Where the checker cannot reach a sound
conclusion it produces an unconstrained type rather than a guess, so a reported
error is a real one.

## Status

Implemented (Milestone A, and the parts of B that do not need a package
registry):

- Reader, spans, module resolver, hygienic macro expansion.
- Type, effect, and exhaustiveness analysis.
- A direct interpreter over the typed AST.
- Canonical formatter, linter, documentation generator, project-aware REPL.
- `new`, `init`, `run`, `dev`, `check`, `test`, `fmt`, `lint`, `repl`, `expand`,
  `doc`, `inspect`, `doctor`, `build`.

Not yet implemented, and reported as such rather than stubbed silently:

- **Native code generation** (Milestone C). `korben build` emits a `.kbx`
  bundle — every module of the program in one reproducible file — plus a
  launcher script. The bundle runs on the Korben runtime; it is not yet a
  standalone native binary.
- **Async runtime and structured concurrency** (Milestone D). `async`, `await`,
  and `task-scope` parse and type-check, and run eagerly on the calling task.
  Channels, task scopes, and cancellation are not implemented.
- **Ownership and borrow analysis** (Milestone C). `Owned`, `Borrow`,
  `BorrowMut`, `Shared`, and `Managed` are not enforced yet; `unsafe` is
  lexically tracked and surfaced by the linter and documentation.
- **FFI** (Milestone C). `korben ffi` reports the milestone it lands in.
- **Package management** (Milestone D). Manifests parse dependencies and the
  lockfile format is specified, but `add`, `remove`, `update`, `publish`,
  `install`, and `audit` report the milestone they land in.
- **Language server** (Milestone B). `korben lsp` reports the milestone it lands
  in; the JSON diagnostics and `api.json` it will build on already exist.
- **Restart-case conditions.** Typed conditions and handlers work; named restart
  points do not.

The interpreter bounds recursion with `Interp::max_depth` and reports exceeding
it as a diagnostic rather than aborting. The executable runs on a large worker
stack so ordinary recursive code has room.

## Repository layout

```text
crates/
  korben-syntax/   source maps, spans, diagnostics, lexer, reader, formatter
  korben-core/     lowering, macro expansion, types, inference, evaluator,
                   standard library, project loading, docs, build artifacts
  korben-cli/      the `korben` executable
examples/          runnable programs, covered by the test suite
spec.md            the language and platform specification
```

The implementation has no dependencies outside the Rust standard library, so the
toolchain stays a single self-contained binary.

## Development

```sh
cargo test              # 98 tests: reader, formatter, evaluator, checker, CLI
cargo clippy --workspace --all-targets
cargo fmt
```

The CLI tests drive the real executable through the whole documented workflow —
`new`, `check`, `test`, `fmt`, `run`, `doc`, `build`, and running the resulting
artifact — for every project template.

## License

MIT.
