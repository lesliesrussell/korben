# Changelog

## Unreleased

### Added

- The language tour declares and uses a constant. `(def name value)` was
  unusable until this release's first fix and appeared in no example after it,
  which is how it stayed broken: a form nothing demonstrates is a form nobody
  exercises.
- `examples/workspace/`, several packages under one root. The `[workspace]`
  form shipped in 0.8.0 with tests but nothing to copy: a reader wanting more
  than one package in a repository had `examples/packages/`, which is two
  projects linked by a path. This one shows a root that owns its members, a
  member depending on a sibling by name with no path, and the single lockfile
  at the root that makes two members share one answer about a shared
  dependency.
- The README says what the toolchain needs and where it runs, before a reader
  can trip over it: which commands reach for `cargo`, `rustc`, `rustup`, or
  `git`, that foreign calls are Unix-only, and that the HTTP server is fastest
  on Unix and merely slower elsewhere. These were discoverable only by failing.

### Fixed

- The HTTP server works again where `poll` is unavailable. The readiness loop
  that landed in 0.8.0 made `serve` fail outright off Unix, narrowing platform
  support without saying so; every test runs on Unix, so nothing caught it.
  Without `poll` the server now offers each open connection in turn, which costs
  a few milliseconds of latency and keeps the concurrency -- a silent client and
  a half-sent request still hold up nobody. `KORBEN_NO_POLL` forces that path so
  it is covered by a test on a machine that has `poll`.

## 0.9.0 — Distribution

A package can be handed to somebody else. `korben publish` writes one into a
registry, `korben install` fetches a registry held in a git repository, and git
verifies who published it — the cryptography is git's, because specification
21.3 wants signed artifacts, 22.4 warns against writing your own, and the
toolchain takes no dependencies. Alongside it, `korben run --profile` says where
a program spent its time.

### Added

- Registry provenance, from the registry's own repository. `korben install`
  asks git whether the latest commit carries a signature it can verify and says
  who signed it; `[registry] signed = true` refuses a registry that does not,
  and removes the clone rather than leaving it unverified. `korben audit`
  reports an unsigned registry as weakened verification, which specification
  21.3 asks for.
- The cryptography is git's. Specification 21.3 wants verifiable signed
  artifacts, 22.4 warns against custom constructions, and the zero-dependency
  rule rules out a vetted library — so the verification is delegated to the tool
  already doing the fetching, and no cryptography enters the tree.
- `korben install` fetches a registry held in a git repository. A registry is a
  repository laid out exactly like a local one, `<name>/<version>/`, so
  resolution, checksums, and the lockfile are unchanged by where it came from.
  Configure it with `[registry] git = "..."`.
- Transport is git because it is the only option that bends none of the rules
  the toolchain already keeps. There is no HTTP client on the Rust side and no
  TLS, and hand-writing either means cryptography that specification 22.4
  cautions against; meanwhile the native backend already requires `cargo`, and
  cross compilation `rustc` and `rustup`, so requiring `git` is consistent
  rather than new. It brings TLS, authentication, and mirroring from a tool that
  is already vetted.
- `install` is the only command that touches a network. Resolution reads the
  local clone and nothing else, so a build is offline whether or not the clone
  is up to date.
- `korben publish` copies a package into a registry, as
  `<registry>/<name>/<version>/` — the layout resolution already reads. It
  refuses more than it accepts: a package that does not check is not one to hand
  to anyone else, a version that already exists is never overwritten because
  every lockfile that pinned it would start lying, and a package depending on a
  local path could not be built by whoever installed it. The content checksum it
  prints is the one a lockfile goes on to pin.
- `korben run --profile` reports where a program spent its time: a table of
  functions by self time, with call counts and each one's share. Specification
  23 asks for profiling "without changing ordinary source code", and nothing in
  a program changes to be profiled -- `apply_now` is the one funnel every call
  passes through, so user functions, builtins, protocol methods, and
  constructors are all covered by one hook.
- Self time rather than inclusive time: the time in a function's own body, with
  everything it called subtracted. Inclusive time double-counts a recursive call
  and needs explaining every time it is read; self time says plainly which body
  the program is sitting in. The report goes to stderr, so a profiled run still
  pipes cleanly, and it is printed even when the program failed -- where the
  time went is often the question being asked about a program that did not
  finish.

## 0.8.0 — Milestone C

The native and safe systems boundary is complete. A Rust library can be called
through an adapter generated from one reading of its signatures, and a build can
target any triple the toolchain has. Alongside it: a language server, local
workspaces, and an HTTP server that serves connections concurrently — reached
without a task ever suspending, on the second design, after the first was built
and measured and found wanting.

### Added

- Bounds on what one connection can hold, now that the server keeps many open.
  A half-sent request may buffer 64 KiB before the server answers `413` and
  hangs up, and a connection that does nothing at all for 30 seconds is closed.
  A silent connection is invisible to `std.http` -- it is never ready, so
  nothing there could decide to give up on it -- so `Pool.evict` is what knows
  it exists. `Pool.write` carries a 30-second timeout, since it is deliberately
  the one blocking call in the loop.
- **Concurrent connection handling.** `std.http`'s server owns its listener and
  every open connection, waits for readiness across all of them at once, and
  calls the handler only when a whole request has arrived. A handler therefore
  always runs to completion, and a client that connects and says nothing, or
  stops halfway through a request, holds up only itself. `serve` keeps its
  signature and no task suspends.
- `std.net/pool`, the resource that makes it possible: one value owning a
  listener and its connections, addressed by id. A Korben collection could not
  hold them, because a resource-bearing value moves and a connection cannot be
  taken out of a vector and put back on every pass of a loop. Readiness across
  several sockets is the one thing the standard library cannot express, so
  `poll(2)` is declared as an `extern "C"`, the way `ffi.rs` already declares
  `dlopen` and `dlsym`.
- A socket operation that would block now runs another ready task before
  waiting, the way a channel already did. This is worth having on its own, but
  it is not what made the server concurrent: driving is re-entrant, so the
  accepting task sat underneath the connection it drove and could not get back
  to accepting while a silent client held it. That approach was built, measured,
  and replaced.
- **The Rust adapter ABI** (specification 17.3), the last of Milestone C.
  `#[korben_export]` marks a function in a Rust library and adds an
  `extern "C"` shim beside it, leaving the function itself ordinary Rust;
  `korben ffi rust <file.rs>` writes the Korben half, with the `raw-`
  declarations that are the foreign contract and a safe wrapper over each. Both
  halves are rendered from one reading of the signature, in the new
  `korben-adapter` crate, so they cannot disagree about a type, a symbol name,
  or a rejection. The boundary carries `i64`, `f64`, `bool`, `&str`, and
  `String`, plus `Result<T, E>` for any printable `E`; anything else is
  declined by name. Failure and panic share one channel, and a panic is caught
  before it crosses, because the release profile aborts on panic and an unwind
  across `extern "C"` is undefined behaviour. `examples/adapter/` is one
  library with both halves in it, and a differential test requires the two
  execution modes to produce identical output through it.
  `#[korben_export]` needs `proc_macro`, which the compiler ships, so the
  toolchain still has no dependency outside the standard library.
- **Cross compilation.** `korben build --target <triple>` passes the triple
  through to the cargo build the backend already drives, and the artifact lands
  in `target/<triple>/<profile>/` with the extension the triple implies. The
  triple is checked first: one rustc does not know is rejected with the nearest
  real one, and one whose standard library is not installed names `rustup
  target add`, where cargo would have reported a missing `std` crate. A cross
  build that fails to compile no longer asks the user to report a compiler bug
  -- the generated Rust compiles for the host, so the target is the difference.
- **Workspaces.** A `[workspace] members = [...]` root gathers several packages
  in one repository. One resolution pass covers every member and one
  `korben.lock` at the root pins the result, so members that share a dependency
  share the version of it. A member may depend on a sibling by name with no
  path. `check`, `test`, `fmt`, and `lint` cover every member; `run` and `build`
  take `--package <name>`, and refuse to guess when a workspace has more than
  one program.
- **A language server.** `korben lsp` speaks the Language Server Protocol over
  stdin and stdout: workspace diagnostics, hover, go to definition, completion,
  document symbols, and formatting. It has no dependency outside the toolchain
  -- the JSON-RPC codec and the protocol's UTF-16 position arithmetic are part
  of the new `korben-lsp` crate.
- Diagnostics read the editor's unsaved buffer rather than the file on disk, so
  an error appears while the code is being written rather than after it is
  saved. A file that becomes clean has its diagnostics cleared.
- Hover reports the type inference actually settled on, for locals as well as
  declarations. `korben_core::infer::chart_session` records a type per
  expression during the same pass `check_session` runs, so the editor and the
  command line cannot give different answers.
- `examples/packages/`, a two-package project with a committed `korben.lock`.
  Acceptance criterion 10 was the only one with no runnable example: the lock
  is verified reproducible by a test that regenerates it and compares bytes,
  and the same test confirms an edited dependency stops the build.

### Fixed

- A lockfile names a git registry by its URL rather than by the directory this
  machine cloned it into. The cache path contains `$HOME` and a digest of the
  URL, so recording it put one developer's home directory into a file everyone
  shares, and the fallback it left behind was wrong on every other machine.

- A response status the table does not name no longer claims to be `OK`: an
  unknown 4xx renders as `Client Error` and an unknown 5xx as `Server Error`.
  `413` is named outright, since the server now sends it.

- A path dependency is recorded in the lockfile relative to the lockfile rather
  than to the manifest that declared it. The two coincide for a top-level
  dependency in a single-package project, which is why this went unnoticed, and
  diverge for a workspace member or a transitive path dependency -- where the
  recorded path resolved against the wrong directory.
- `korben check` reports a name declared twice in one module. It had been
  caught only by the native backend, as a Rust compile failure the user was
  told to report as a compiler bug -- and the two execution modes disagreed
  about it, the interpreter keeping the later definition where the backend
  refused to compile. Values and types are separate namespaces, so a record
  declaring both a type and a constructor is still fine.
- `korben lint` reports a function, foreign function, or constant that a macro
  of the same name makes unreachable. Expansion runs before evaluation and a
  call site cannot tell a macro from a function, so the macro always won and the
  declaration under it was dead code that nothing mentioned. Both execution
  modes agree about which one wins, which is why this is a lint and not an
  error. `duplicate_declarations` had an arm for the case, but it never fired:
  the expander consumes macro forms before that pass sees the module.
- `(def name value)` works. `split_annotation` was a stub that discarded the
  forms after the name, so every top-level `def` failed with "`def` needs a
  value" and an annotation was never read. Nothing in the repository, prelude,
  or examples used `def`, which is why it went unnoticed. `let` and `var` now
  share the repaired helper rather than each reading annotations their own way.
- `korben check` reports unbound names. It had left them to the evaluator,
  which meant a module calling an undefined function checked clean and only
  failed once a run reached that line -- and `korben check` never runs the
  evaluator at all. Names reachable at run time but without a signature the
  checker can use are still accepted silently, so this reports mistakes rather
  than gaps in the checker's knowledge.
- A member a module does not have is reported too, whether the module is
  written in Korben or provided by the runtime. Both diagnostics suggest the
  nearest name, and withhold a suggestion that is not close enough to help.

### Changed

- The `service` template is written over `std.http` rather than hand-rolling a
  request record. `korben new --template service` now scaffolds a real handler,
  tests that exercise it without a socket, and a `serve` argument that puts it
  on a port.
- Documentation that the async runtime made stale: `std.net`, `std.http`, and
  the README each said concurrent connection handling needed an async runtime.
  It exists. The reason a server still handles one request at a time is that
  the sockets block and a started task cannot suspend, so a read stalls the
  scheduler. That needs non-blocking sockets, which is what the text says now.

## 0.7.0 — Structured concurrency

An async runtime, within the constraint that Korben values are reference
counted and therefore belong to one thread: the scheduler is cooperative and
single-threaded, so tasks are concurrent rather than simultaneous.

### Added

- **Tasks.** Calling an `async fn` yields a task instead of running it.
  `await` runs one, `join-all` runs many and short-circuits on the first `Err`,
  and a task carries its result, its failure, or its cancellation.
- **Task scopes.** `(task-scope name ...)` binds a scope; on the way out it
  joins the tasks started under it, or cancels them when the body is already
  failing. A child's failure reaches the code that started it, so nothing is
  silently dropped — specification 15.2's guarantee that a task never outlives
  the operation that created it.
- **`spawn`**, written `(spawn scope expr)` or `(scope.spawn expr)`. The
  expression is deferred rather than evaluated at the call.
- **Channels**, bounded and unbounded, over `std.async`. Sending to a full
  channel or receiving from an empty one drives other ready tasks and retries,
  so producer and consumer patterns work without threads. A genuine cycle is
  reported as a deadlock, naming what could not make progress, rather than
  hanging.
- **Cooperative cancellation**: `scope.cancel` stops work that has not started,
  and a running task can check `scope.cancelled?`.
- The checker types an `async fn` as returning `Task T`, unwraps it at `await`,
  and rejects `await` outside asynchronous code per specification 15.1.
- `examples/async.kb`.

### Fixed

- Generated closures captured their environment by move, so a value used after
  a closure was created failed to compile. The core IR now records which
  enclosing locals a closure reads, and generated code copies them — which is
  cheap, because values are reference counted.

### Not in this release

Parallelism, and a preemptible I/O reactor. A started task cannot suspend, so
the HTTP server still handles one request at a time.

## 0.6.0 — HTTP

A typed HTTP program can now be written, which closes the last open half of
v0.1 acceptance criterion 3. Every criterion in specification section 28 is
now met.

### Added

- **`std.net`**: blocking TCP. `Listener` and `Connection` are resource-bearing
  handles, so `with` releases them and the ownership analysis governs them.
  Socket operations are methods on the receiver, which borrows rather than
  moves it — that is what lets an accept loop keep its listener.
- **`std.http`**, written in Korben over `std.net` and carried inside the
  toolchain. Requests and responses are ordinary records, `HttpError` an
  ordinary enum, and routing ordinary pattern matching. Parses and renders
  HTTP/1.1, reads bodies by `content-length`, and provides a server, a client,
  and `test-request` for exercising a handler without a socket.
- **Embedded standard-library modules**: Korben source the toolchain carries and
  loads on demand, so it stays in step with the compiler that ships it.
- `std.string/split-once`, `std.string/byte-length`, `std.string/repeat`, and
  `std.core/keyword`.
- `examples/http.kb`, the specification's section 29 reference program.

### Fixed

- **Names were global across modules in the type checker**, so two modules
  declaring the same `handle` or `BadRequest` were conflated. Modules now have
  their own namespaces: a name resolves through the module's declarations and
  its imports, and types are keyed by `module/name` while diagnostics still show
  the short name.
- Unifying a type variable with itself failed the occurs check, which rejected
  correct programs whose inference happened to reach that state.
- The checker mismatched `:keyword value` arguments to functions that declare no
  such keyword. Function types now carry their keyword parameters, so the
  checker and the runtime agree on how an argument binds.
- A new runtime source file that was not carried into generated projects would
  only fail later, inside someone else's `korben build`. A test now checks the
  two lists agree.

### Not in this release

The async runtime, so a server handles one request at a time on the accepting
task. TLS: only `http://` is supported.

## 0.5.0 — Dependencies and reproducible builds

A build now reproduces from `korben.lock`, closing v0.1 acceptance
criterion 10.

### Added

- **Dependency declarations** with semantic version requirements, in short form
  (`json = "^0.1"`) or long form with a source
  (`[dependencies.json] path = "../json"`). Sources are a directory on this
  machine, or a package in a registry directory laid out as
  `<registry>/<name>/<version>/`.
- **Deterministic resolution.** Every requirement on a name is collected and the
  highest version satisfying all of them is chosen. Resolution is a fixpoint, so
  a requirement discovered deep in the graph can narrow a choice made earlier
  and the result does not depend on declaration order. A conflict names every
  requirement, who made it, and what versions exist.
- **`korben.lock`**, fully pinned: version, source identity, SHA-256 content
  checksum, and resolved edges. When the lock describes the manifest, resolution
  does not run — locked versions are used verbatim and checksums are verified,
  so a dependency that changed underneath the lock is an error.
- **SHA-256** in the toolchain, verified against the FIPS 180-4 vectors.
- **Cross-package modules.** A dependency's modules are importable under the
  names they declare, and only by packages that declare the dependency.
- **`korben add`, `remove`, `update`, and `audit`.** `audit` verifies lockfile
  integrity and checksums, and reports package metadata gaps, local-path
  dependencies that make a lock unportable, and weakened verification settings.
- An `[registry] path` manifest key and a `KORBEN_REGISTRY` override.

### Changed

- **Install scripts are prohibited outright.** A manifest declaring `install`,
  `preinstall`, `postinstall`, `prepare`, `script`, or a `[scripts]` table is
  rejected rather than having the key quietly ignored.
- `korben doctor` reports dependency count, lockfile presence, and whether
  `KORBEN_SKIP_CHECKSUMS` has disabled verification.
- A module that cannot be found and looks like a package name suggests
  `korben add`, and is reported once rather than once per attempt.

### Not in this release

A network registry, publishing, package signing, git dependencies, workspaces,
and sandboxed build scripts.

## 0.4.0 — C interoperation

`korben check` and `korben run` can now consume a C library through a
generated, typed binding and a safe wrapper, closing v0.1 acceptance
criterion 8.

### Added

- **Foreign declarations**: `(ffi/c-library "name")` selects a library and
  `(ffi/c-fn name ["symbol"] [params] -> CRet)` declares a function over the C
  types `CVoid`, `CBool`, `CChar`, `CInt`, `CUInt`, `CLong`, `CULong`,
  `CFloat`, `CDouble`, `CStr`, and `Ptr`.
- **Containment per specification 17.1 and 12.7.** A declaration asserts a
  contract the compiler cannot verify, so it is an `unsafe fn` carrying `!ffi`
  and `!unsafe`. Calling one from safe code is an error naming the fix. Safe
  Korben wrappers are the ordinary user-facing form, and their signatures still
  carry the effects, because 22.3 forbids hiding unsafe implementation from a
  caller.
- **No foreign null reaches Korben.** A `CStr` or `Ptr` return surfaces as an
  `Option`, so a null becomes `None` rather than something that can be
  dereferenced.
- **Dynamic loading and typed invocation** in `korben-runtime`, shared by both
  execution modes: a native executable and the interpreter call foreign code
  through the same code path.
- **`korben ffi c <header>`**: a binding generator over the C prototype subset
  that appears in ordinary headers. Prototypes it cannot type — variadics,
  function pointers, structs by value — are listed with a reason rather than
  guessed at.
- **`korben ffi`** lists a project's foreign declarations with their libraries
  and signatures.
- An `[ffi]` manifest section declaring linked C libraries and Rust adapters;
  `korben doctor` reports them.
- `examples/ffi.kb`.

### Not in this release

The Rust adapter ABI from specification 17.3, foreign callbacks, structs passed
by value, and ownership transfer across the foreign boundary. Foreign calls are
Unix-only, and a signature must be all-integer or all-floating: the C ABI passes
those classes in different registers, so a mixed signature is rejected with an
explanation rather than called incorrectly.

## 0.3.0 — Ownership and borrowing

`korben check` now reports ownership violations, closing the last part of v0.1
acceptance criterion 9.

### Added

- **Ownership analysis** over specification section 12. Only resource-bearing
  values move, so ordinary immutable data is never move-checked and ordinary
  code sees no ownership diagnostics. A type owns a resource when it implements
  `Drop`, is written `Owned T`, is a native handle, or contains one of those.
- Flow-sensitive move checking. Branches are analyzed from a common state and
  joined, so a value moved on one path is reported as *may have been moved*
  rather than being missed or over-reported. Reported: `use-after-move`,
  `maybe-moved`, `move-in-loop`, `clone-resource`, `exclusive-borrow`,
  `borrow-across-task`, `borrow-escape`, and `unsafe-call`.
- Every ownership diagnostic names the binding, points at both the move and the
  use, explains the category and type, and suggests a fix.
- Implicit borrows at call sites: a `T` satisfies a `Borrow T` parameter, per
  specification 12.3. The reverse does not hold, which is what stops a borrow
  escaping as owned data.
- `Drop` is a compiler-known protocol; implementing it makes a type
  resource-bearing and gives `with` something to call.
- `std.fs/open` and `std.fs/create` return a real `File` resource, with
  `write`, `read-text`, `close`, and `closed?`. `with` releases it on every exit
  path, including error propagation.
- `std.core/clone`, and a diagnostic when a resource is cloned.
- `examples/ownership.kb`.

### Changed

- Protocol implementations are checked like any other function body.

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
