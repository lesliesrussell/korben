# Korben

A compiled, statically typed, ownership-safe Lisp for native software, with a
one-command modern developer experience.

`spec.md` is the full language and platform specification. This repository is the
Rust implementation of it. **This tree implements Milestone A ("Usable core"),
the native backend and ownership analysis from Milestone C, and parts of
Milestone B.** See
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
| `korben ffi [c <header>]` | List or generate foreign bindings |
| `korben doctor` | Toolchain and project health |
| `korben build [--release] [--emit ir\|rust]` | Compile to a native executable |

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

## Two execution modes

Korben runs the same program two ways, and the specification requires them to
have identical observable semantics.

```sh
korben run                  # development mode: direct interpretation
korben build --release      # native mode: a standalone executable
```

Both link the same `korben-runtime` crate — one value representation, one call
dispatch, one standard library — so they agree by construction rather than by
convention. A differential test suite compiles a corpus covering records, enums,
guards, protocols, macros, `?`, `loop`/`recur`, named arguments, cells, JSON,
`try`/`catch`/`finally`, and `defer` both ways and requires byte-identical
output, runtime fault reports included.

The native backend lowers typed core IR to Rust and hands it to an isolated
cargo build, the bootstrapping strategy specification section 18.3 describes.
Both stages are inspectable:

```sh
korben build --emit ir      # the name-resolved core IR
korben build --emit rust    # the generated Rust
```

A release binary of the language tour is 587 KB and starts in about 7 ms.
Values are still dynamically typed at run time, so compute-bound code is only
about twice the interpreter's speed; using the inferred types to unbox is the
next optimization, not a redesign.

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
- `and` and `or` short-circuit on truthiness. They are core forms rather than
  macros because their result may be any of their operands' types, which no
  expansion into `if` can express in a typed language.
- Records, enums, newtypes, tuples, vectors, maps, and sets. Collections are
  immutable; `var` and `Cell` make mutation explicit.
- Pattern matching over literals, constructors, vectors with rest patterns,
  maps, records, and guards — with exhaustiveness and unreachability checking.
- `Option` and `Result` with the postfix `?` propagation operator. No `null`.
- Typed conditions with `try`/`catch`/`finally`, `throw`, deterministic cleanup
  through `with`, and `defer` in last-in-first-out order.
- **Ownership and borrowing.** Only resource-bearing values move, so ordinary
  code never sees an ownership diagnostic. A type owns a resource when it
  implements `Drop`, is written `Owned T`, is a native handle such as `File`, or
  contains one. Borrows are taken implicitly at call sites, as specification
  12.3 requires, and `unsafe` functions may only be called from `unsafe` code.
  `examples/ownership.kb` walks through it.
- Protocols with explicit implementations and dispatch on the receiver.
- Hygienic macros. Macros are functions from syntax objects to syntax objects,
  run at compile time; identifiers a template binds cannot capture a caller's.
  The prelude (`when`, `unless`, `and`, `or`, `cond`, `if-let`, `when-let`) is
  written in Korben, so `korben expand` shows exactly what your code becomes.
- Hindley–Milner type inference with let-polymorphism, structural records that
  unify with the nominal types they match, tuple inference for heterogeneous
  vector literals, effect inference (`!io`, `!async`, `!alloc`, `!ffi`,
  `!unsafe`), and `--strict-api` for complete public signatures.
- **C interoperation.** `(ffi/c-library ...)` and `(ffi/c-fn ...)` declare typed
  foreign functions; `korben ffi c <header>` generates them from C prototypes.
  A declaration asserts a contract the compiler cannot verify, so it is an
  `unsafe fn` carrying `!ffi` and `!unsafe` — safe Korben wrappers are the
  ordinary user-facing form. A null `CStr` or pointer marshals to `None`, so
  foreign null never becomes a Korben value. `examples/ffi.kb` calls libc.
- Standard library: `std.core`, `std.string`, `std.math`, `std.io`, `std.fs`,
  `std.json`, `std.log`, `std.time`, `std.process`, `std.test`, `std.syntax`.
  `fs.open` and `fs.create` return a `File`, a real resource that `with` closes
  on every exit path.

Inference is deliberately conservative. Where the checker cannot reach a sound
conclusion it produces an unconstrained type rather than a guess, so a reported
error is a real one. Ownership follows the same principle from the other end:
it says nothing at all about values that cannot leak.

```
error[use-after-move]: `file` was moved and cannot be used again
  --> src/main.kb:10:12
   |
10 |   (consume file))
   |            ^^^^ used after the move
   |
  ... src/main.kb:9:12
   |
 9 |   (consume file)
   |            ---- moved here
   |
  note: an owned resource is released exactly once, so moving it transfers responsibility
  help: pass it by `Borrow`, or restructure so the value is used before it moves
```

## Status

Implemented:

- Reader, spans, module resolver, hygienic macro expansion.
- Type, effect, and exhaustiveness analysis.
- A direct interpreter over the typed AST.
- Core IR, and a native backend that produces standalone executables.
- Ownership, move, and borrow analysis with `Drop`-based resource types.
- C FFI: typed declarations, a binding generator, and dynamic library loading
  shared by both execution modes.
- Canonical formatter, linter, documentation generator, project-aware REPL.
- `new`, `init`, `run`, `dev`, `check`, `test`, `fmt`, `lint`, `repl`, `expand`,
  `doc`, `inspect`, `doctor`, `build`.

Not yet implemented, and reported as such rather than stubbed silently:

- **Async runtime and structured concurrency** (Milestone D). `async`, `await`,
  and `task-scope` parse and type-check, and run eagerly on the calling task.
  Channels, task scopes, and cancellation are not implemented.
- **Lifetime inference.** Ownership tracks moves flow-sensitively and reports
  use-after-move, possible moves across branches, moves inside a loop, cloning a
  resource, exclusive-borrow aliasing, borrows crossing a task boundary, and
  escapes from a `with` scope. What it does not do is follow a borrow's owner
  across a function boundary, so returning a borrow is checked by types alone.
- **`Shared` and `Managed`.** Both are recognized as ownership categories, but
  neither has a runtime representation yet, so there is no way to construct one.
- **The Rust adapter ABI** (specification 17.3). `[ffi] rust = [...]` parses and
  `korben ffi` reports it, but the `korben-export` adapter is not implemented.
  C interoperation is complete enough to consume a Rust library through a
  `#[no_mangle] extern "C"` surface.
- **Foreign callbacks and structs by value.** A pointer is carried opaquely,
  never dereferenced, and never freed on Korben's behalf; specification 17.1's
  ownership-transfer annotations are not implemented. Function pointers and
  structs passed or returned by value are declined by the generator rather than
  guessed at.
- **Package management** (Milestone D). Manifests parse dependencies and the
  lockfile format is specified, but `add`, `remove`, `update`, `publish`,
  `install`, and `audit` report the milestone they land in.
- **Language server** (Milestone B). `korben lsp` reports the milestone it lands
  in; the JSON diagnostics and `api.json` it will build on already exist.
- **Restart-case conditions.** Typed conditions and handlers work; named restart
  points do not.

Sharp edges worth knowing:

- Building natively needs `cargo` on `PATH`, because the backend compiles
  generated Rust. `korben run` needs nothing.
- Foreign calls are Unix-only, and a signature must be all-integer (up to eight
  parameters) or all-floating (up to four). The C ABI passes the two classes in
  different registers, so a mixed signature is rejected with an explanation
  rather than called incorrectly.
- `korben ffi c` reads the prototype subset that appears in ordinary headers.
  Extraction through libclang, which would follow typedefs and macros, is future
  work; declarations it cannot type are listed rather than guessed at.
- A string literal inside a `{...}` interpolation hole must be escaped:
  `(format "{(str \"a\" \"b\")}")`. Write `{{` and `}}` for literal braces.

The interpreter bounds recursion with `Interp::max_depth` and reports exceeding
it as a diagnostic rather than aborting. The executable runs on a large worker
stack so ordinary recursive code has room.

## Repository layout

```text
crates/
  korben-syntax/   source maps, spans, diagnostics, lexer, reader, formatter
  korben-runtime/  values, call dispatch, the standard library -- shared by the
                   interpreter and by generated native code
  korben-core/     lowering, macro expansion, types, inference, evaluator,
                   core IR, the native backend, project loading, docs
  korben-cli/      the `korben` executable
examples/          runnable programs, covered by the test suite
spec.md            the language and platform specification
```

The implementation has no dependencies outside the Rust standard library, so the
toolchain stays a single self-contained binary. `korben-runtime`'s source is
embedded in it and written into each generated project, which is what keeps
native builds reproducible and offline.

## Development

```sh
cargo test              # 148 tests: reader, formatter, evaluator, checker,
                        # ownership, FFI, CLI, and interpreter-vs-native
                        # differential tests
cargo clippy --workspace --all-targets
cargo fmt
```

The CLI tests drive the real executable through the whole documented workflow —
`new`, `check`, `test`, `fmt`, `run`, `doc`, `build` — for every project
template. The differential tests additionally compile programs to native
executables and compare their output against the interpreter; they skip
themselves when `cargo` is unavailable.

## License

MIT.
