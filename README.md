# Korben

A compiled, statically typed, ownership-safe Lisp for native software, with a
one-command modern developer experience.

`spec.md` is the full language and platform specification. This repository is the
Rust implementation of it. **Every acceptance criterion for v0.1 in
specification section 28 is met.** See [Status](#status) for what is
implemented and what is deliberately not.

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
| `korben run --profile` | Report where a program spent its time |
| `korben repl` | Project-aware REPL |
| `korben expand <file>` | Print macro expansion through the formatter |
| `korben doc [--out <dir>]` | Markdown docs plus a machine-readable `api.json` |
| `korben inspect` | The resolved project model |
| `korben ffi [c <header>] [rust <file.rs>]` | List or generate foreign bindings |
| `korben add <name> [--version <req>] [--path <dir>] [--dev]` | Declare a dependency |
| `korben remove <name>` | Drop a dependency |
| `korben update` | Re-resolve and rewrite the lockfile |
| `korben audit` | Verify the lockfile, checksums, and package metadata |
| `korben publish [--registry <dir>]` | Copy this package into a registry |
| `korben doctor` | Toolchain and project health |
| `korben build [--release] [--target <triple>] [--emit ir\|rust]` | Compile to a native executable |
| `korben lsp` | Language server, over stdin and stdout |

In a workspace, `check`, `test`, `fmt`, and `lint` cover every member, while
`run` and `build` take `--package <name>` to say which program is meant.

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

## Structured concurrency

Korben values are reference counted, so they belong to one thread. The scheduler
is therefore **cooperative and single-threaded: tasks are concurrent, not
simultaneous.**

Calling an `async fn` yields a task rather than running it. A scope owns every
task started under it, and on the way out joins them — or cancels them if the
body is already failing — so nothing silently outlives the operation that
created it, as specification 15.2 requires.

```lisp
(pub async fn load-dashboard [ids: Vec Int] -> Result (Vec Profile) DashboardError !async !io
  (task-scope scope
    (let tasks (map ids (fn [id] (spawn scope (fetch-profile id)))))
    (task.join-all tasks)))
```

An operation that would block — awaiting, receiving from an empty channel,
sending to a full one — *drives* other ready tasks and then tries again. That
makes producer and consumer patterns work without threads.

The cost of not having real suspension: a task that has already started cannot
be paused. If a task blocks on work that only a running task could produce,
that is a genuine cycle, and the scheduler says so instead of hanging:

```
error[channel-deadlock]: the channel is full and nothing can drain it
   |   (each [1 2 3] (fn [n] (sender.send n)))
   |                         ^^^^^^^^^^^^^^^^
  note: no other task is ready to receive
  help: give the channel more capacity, or receive before sending
```

`examples/async.kb` walks through spawning, joining, failure propagation,
cancellation, and channels.

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

Because the backend hands its output to cargo, a build can name any target that
toolchain has:

```sh
korben build --target wasm32-unknown-unknown
korben build --release --target aarch64-unknown-linux-gnu
```

The artifact lands in `target/<triple>/<profile>/`, so building for several
targets does not overwrite one with another, and it takes the extension the
triple implies. The triple is checked before cargo is invoked: one rustc does
not know is rejected with the nearest real one, and one whose standard library
is missing names the `rustup target add` command that installs it, rather than
surfacing as an error about a missing `std` crate.

A release binary of the language tour is 587 KB and starts in about 7 ms.
Values are still dynamically typed at run time, so compute-bound code is only
about twice the interpreter's speed; using the inferred types to unbox is the
next optimization, not a redesign.

## The language today

`examples/tour.kb` exercises everything below; `examples/greeting.kb` is the
specification's reference example adapted to what exists. `examples/packages/`
is a project rather than a file: two packages and a committed lockfile.

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
- **HTTP.** `std.http` is written in Korben over `std.net`: requests and
  responses are ordinary records, errors an ordinary enum, and routing is
  ordinary pattern matching. `examples/http.kb` is the specification's section
  29 reference program. Connections are served concurrently: the server owns its
  listener and every open connection, waits for readiness across all of them at
  once, and calls the handler only when a whole request has arrived. A client
  that connects and says nothing, or stops halfway, holds up only itself, and no
  task ever suspends — the handler always runs to completion.
- **C interoperation.** `(ffi/c-library ...)` and `(ffi/c-fn ...)` declare typed
  foreign functions; `korben ffi c <header>` generates them from C prototypes.
  A declaration asserts a contract the compiler cannot verify, so it is an
  `unsafe fn` carrying `!ffi` and `!unsafe` — safe Korben wrappers are the
  ordinary user-facing form. A null `CStr` or pointer marshals to `None`, so
  foreign null never becomes a Korben value. `examples/ffi.kb` calls libc.
- **The Rust adapter ABI.** `#[korben_export]` marks a function in a Rust
  library, adding an `extern "C"` shim beside it without changing the function;
  `korben ffi rust <file.rs>` writes the Korben half from the same signatures.
  Both halves are rendered from one reading of the signature, so they cannot
  disagree about a type or a symbol. Failure and panic share one channel, and
  every generated wrapper returns a `Result` even where the Rust function does
  not, because the boundary can fail where the function cannot.
  `examples/adapter/` is one library with both halves in it.
- **Structured concurrency.** `async fn`, `await`, `task-scope`, `spawn`,
  `join-all`, cooperative cancellation, and typed bounded or unbounded
  channels. `await` outside asynchronous code is a compile error.
- **Workspaces.** A `[workspace] members = [...]` root gathers several packages
  in one repository, resolved in a single pass and pinned by a single
  `korben.lock` at the root -- so two members that share a dependency share the
  version of it, which is the reason to keep them in one repository. A member
  may depend on a sibling by name, with no path. Sharing a workspace grants no
  access: an import still needs a declared dependency. Members are not
  checksummed, because they are source being edited rather than pinned
  artifacts; everything from outside the workspace still is.
- **Editor support.** `korben lsp` speaks the Language Server Protocol over
  stdin and stdout, with no dependency beyond the toolchain itself. Diagnostics
  republish as you type and read from the unsaved buffer, not the file on disk.
  Hover shows a declaration's signature and documentation, or the type inference
  gave a local -- from the same inference `korben check` runs, so the editor and
  the command line cannot disagree. Go to definition, completion (module members
  after an alias, declarations and builtins elsewhere), document symbols, and
  formatting through the canonical formatter round it out.
- Standard library: `std.core`, `std.string`, `std.math`, `std.io`, `std.fs`,
  `std.json`, `std.log`, `std.time`, `std.process`, `std.test`, `std.syntax`,
  `std.net`, `std.http`, `std.async`.
  `fs.open` and `fs.create` return a `File`, and `std.net` returns `Listener`
  and `Connection` — real resources that `with` closes on every exit path.

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

## Dependencies and reproducible builds

Dependencies are declared with semantic version requirements, resolved
deterministically, and pinned in `korben.lock`.

```sh
korben add json --version "^0.2"      # from a registry directory
korben add shout --path ../shout      # from a directory on this machine
korben update                          # re-resolve and re-pin
korben audit                           # verify the lock and its checksums
```

When the lockfile is present and still describes the manifest, **resolution
does not run**: the locked versions are used verbatim and every SHA-256
checksum is verified. A dependency that changed underneath the lock is an
error, not a silent difference.

```
error: dependency `shout` has changed since it was locked
  locked:  sha256:efcacd0439edcbb656ce6880e30e310c3973f5b8dd68db3477f94412faa0086f
  found:   sha256:a472672f20a3779d128b1e82168d49f8980aa693416eecf217e5fa1d84b44a39
  run `korben update` to accept the change
```

Resolution is a fixpoint, so the result does not depend on the order
dependencies happen to be declared in, and a conflict names every requirement
and who made it:

```
error: no version of `text` satisfies every requirement
  `app` requires ^0.3
  `greet` requires ^0.2
  available: 0.1.0, 0.2.0, 0.2.3, 0.3.0
```

A package may only import from packages it declares, so a transitive dependency
does not silently become part of your API surface.

**Install scripts are prohibited outright**, not merely discouraged: a manifest
declaring one is rejected rather than having the key quietly ignored.
`korben audit` and `korben doctor` report weakened settings, including
`KORBEN_SKIP_CHECKSUMS`.

## Status

Implemented:

- Reader, spans, module resolver, hygienic macro expansion.
- Type, effect, and exhaustiveness analysis.
- A direct interpreter over the typed AST.
- Core IR, and a native backend that produces standalone executables.
- Ownership, move, and borrow analysis with `Drop`-based resource types.
- `std.net` and `std.http`: a working HTTP/1.1 server and client.
- A cooperative async runtime: tasks, scopes, cancellation, and channels.
- Per-module namespaces, so two modules may declare the same name.
- C FFI: typed declarations, a binding generator, and dynamic library loading
  shared by both execution modes.
- Dependency resolution, a pinned lockfile with SHA-256 checksums, and
  reproducible builds over path and local-registry sources.
- Workspaces: several packages in one repository, resolved and locked together.
- A language server: diagnostics, hover, go to definition, completion, document
  symbols, and formatting.
- Canonical formatter, linter, documentation generator, project-aware REPL.
- `new`, `init`, `run`, `dev`, `check`, `test`, `fmt`, `lint`, `repl`, `expand`,
  `doc`, `inspect`, `doctor`, `build`, `lsp`.

Not yet implemented, and reported as such rather than stubbed silently:

- **Parallelism.** The scheduler is cooperative and single-threaded, and a
  started task cannot suspend. Making tasks run simultaneously means moving the
  value representation from reference counting to atomic sharing, which is a
  deliberate future change rather than an oversight.
- **Task suspension.** A started task still cannot suspend: its body runs on the
  native stack, so there is no resumable representation of a partly-finished
  task. The HTTP server does not need one — it waits for readiness across every
  socket at once and calls a handler only when a whole request has arrived — but
  real preemption, and fairness under load, do.
- **TLS.** `std.http` speaks `http://` only; `https://` needs `std.crypto`.
- **Lifetime inference.** Ownership tracks moves flow-sensitively and reports
  use-after-move, possible moves across branches, moves inside a loop, cloning a
  resource, exclusive-borrow aliasing, borrows crossing a task boundary, and
  escapes from a `with` scope. What it does not do is follow a borrow's owner
  across a function boundary, so returning a borrow is checked by types alone.
- **`Shared` and `Managed`.** Both are recognized as ownership categories, but
  neither has a runtime representation yet, so there is no way to construct one.
- **Foreign callbacks and structs by value.** A pointer is carried opaquely,
  never dereferenced, and never freed on Korben's behalf; specification 17.1's
  ownership-transfer annotations are not implemented. Function pointers and
  structs passed or returned by value are declined by the generator rather than
  guessed at.
- **A network registry** (Milestone D). Dependencies resolve from a directory on
  this machine — a path, or a registry root laid out as
  `<registry>/<name>/<version>/`, which `korben publish` writes into. Fetching
  one over a network is not implemented, and the reason is a decision rather
  than an omission: the toolchain has no HTTP client on the Rust side and no
  TLS, so a network registry means either shelling out to `git` for transport,
  or signing packages and taking integrity from the signature instead — which
  needs cryptography in the tree that specification 22.4 cautions against
  writing. `korben install` reports the milestone it lands in. Package signing,
  git dependencies, and sandboxed build scripts are not implemented.
- **The rest of the language server** (specification 20.4). What is implemented
  is listed above. Rename with macro-hygiene awareness, find references, code
  actions, signature help, semantic tokens, and inlay hints are not, and the
  server declines those requests rather than answering them badly. Document sync
  is whole-file: every keystroke re-checks the workspace, which is fast enough
  at the sizes Korben projects reach today and is the obvious thing to make
  incremental first.
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
- The Rust adapter carries `i64`, `f64`, `bool`, `&str`, and `String`, plus
  `Result<T, E>` for any printable `E`. Callbacks, structs by value, generics,
  and `async` are declined by name rather than guessed at, and a returned string
  is valid until the next call on that thread -- which is the contract Korben
  already relies on, since it copies a foreign string as it unmarshals it.
- A string literal inside a `{...}` interpolation hole must be escaped:
  `(format "{(str \"a\" \"b\")}")`. Write `{{` and `}}` for literal braces.

The interpreter bounds recursion with `Interp::max_depth` and reports exceeding
it as a diagnostic rather than aborting. The executable runs on a large worker
stack so ordinary recursive code has room.

## Acceptance

Specification section 28 lists what a new user must be able to do before v0.1
is ready. All ten hold today:

| # | Criterion | Where |
| --- | --- | --- |
| 1 | Install one executable | `cargo build --release` produces `korben`, 1.1 MB, no runtime deps |
| 2 | Create a project with `korben new` | three templates, each checked and tested in CI |
| 3 | Write a typed HTTP or CLI program | `examples/http.kb`, `examples/tour.kb` |
| 4 | Run it through `korben dev` with immediate diagnostics | check, test, run in one command |
| 5 | Format and test with built-in commands | `korben fmt`, `korben test` |
| 6 | Explore it in a project-aware REPL | `korben repl`, with `:type` |
| 7 | Produce a native release binary | `korben build --release` |
| 8 | Consume a C library through a generated binding and safe wrapper | `korben ffi c`, `examples/ffi.kb`, `examples/adapter/` |
| 9 | Readable errors for type, macro, exhaustiveness, and ownership faults | `examples/ownership.kb` |
| 10 | Reproduce the build from a lockfile without install scripts | `examples/packages/`, `korben audit` |

What that does *not* mean is that the specification is finished: parallelism, a
package registry, and TLS are all still ahead.

## Repository layout

```text
crates/
  korben-syntax/   source maps, spans, diagnostics, lexer, reader, formatter
  korben-runtime/  values, call dispatch, the standard library -- shared by the
                   interpreter and by generated native code
  korben-core/     lowering, macro expansion, types, inference, evaluator,
                   core IR, the native backend, project loading, docs
  korben-lsp/      the language server: JSON-RPC, positions, editor queries
  korben-cli/      the `korben` executable
  korben-adapter/  the signature subset the Rust adapter boundary carries
  korben-export/   what a Rust library depends on to be called from Korben
  korben-export-macro/  the `#[korben_export]` attribute
examples/          runnable programs, covered by the test suite;
                   `packages/` is a two-package project with a committed lock,
                   `adapter/` a Rust library with both halves of its boundary
spec.md            the language and platform specification
```

The implementation has no dependencies outside the Rust standard library, so the
toolchain stays a single self-contained binary. `korben-runtime`'s source is
embedded in it and written into each generated project, which is what keeps
native builds reproducible and offline.

## Development

```sh
cargo test              # 315 tests: reader, formatter, evaluator, checker,
                        # ownership, concurrency, FFI, the Rust adapter, HTTP,
                        # packaging, CLI, and interpreter-vs-native
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
