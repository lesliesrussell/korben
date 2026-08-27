# Korben: Language and Platform Specification

**Status:** Design specification

**Tagline:** A compiled, statically typed, ownership-safe Lisp for native software, with a one-command modern developer experience.

## 1. Product definition

Korben is a Lisp-family programming language and integrated development platform implemented in Rust. It targets command-line tools, network services, automation, local-first software, native extensions, game and creative tooling, and backend components for desktop applications.

Korben combines:

- S-expression syntax, macros, REPL-driven development, and code-as-data from Lisp.
- Static typing, inference-first APIs, structural records, discriminated unions, and developer-oriented diagnostics comparable in spirit to TypeScript.
- Deterministic resource management, ownership, borrowing, explicit mutation, and explicit unsafe boundaries inspired by Rust.
- Native executable delivery, simple concurrency, and operational directness associated with Go.
- A coherent single-binary workflow—build, run, test, format, lint, package, document, and inspect—akin to the experience goal of Bun.

Korben is not a Common Lisp implementation, a Rust syntax alternative, a JavaScript replacement for browsers, or a compatibility layer for another language runtime.

## 2. Goals

### 2.1 Primary goals

1. Compile a Korben project into small, fast-starting native executables.
2. Make the safe, idiomatic path concise, readable, and heavily inferred.
3. Support expressive compile-time metaprogramming without sacrificing source maps, formatting, navigation, or diagnostics.
4. Make typed native interoperation a first-class language capability.
5. Ship developer tooling as part of the language distribution rather than as a fragmented ecosystem.
6. Support interactive development through a project-aware REPL and fast incremental checking.
7. Provide safe structured concurrency and predictable resource cleanup.
8. Be practical for modern working developers who value strong typing, simple deployment, and excellent editor support.

### 2.2 Non-goals for initial releases

- Browser DOM APIs or a browser runtime.
- Full Common Lisp compatibility.
- Full Rust syntax, trait-system compatibility, macro compatibility, or crate compatibility.
- Automatic direct consumption of arbitrary Rust crates without binding metadata or an adapter.
- A general-purpose tracing garbage collector as the default memory model.
- A self-hosting compiler before the semantics, runtime ABI, and tooling are stable.
- Distributed actors, a database, UI toolkit, ORM, or web framework in the language core.

## 3. User experience

Korben is installed as a single executable named `korben`.

```sh
korben new hello-service
cd hello-service
korben dev
korben test
korben check
korben fmt
korben doc
korben build --release
```

A project is usable with a directory, source files, and `korben.toml`. No separate formatter, package manager, test framework, language server installation, runtime installation, or system-wide project generator is required for the standard workflow.

### 3.1 Design principles

- **Native by default:** Release builds are native binaries. Runtime deployment does not require a Korben installation.
- **Inference first:** Annotations describe module interfaces, public APIs, FFI boundaries, and complex intent. Local code relies on inference.
- **Explicit effects at boundaries:** I/O, asynchronous work, foreign calls, mutation, allocation-sensitive paths, and unsafe behavior are visible in signatures or forms.
- **Simple defaults, explicit escape hatches:** The default is safe and idiomatic; advanced control exists without becoming mandatory.
- **Tooling is language semantics:** Formatter stability, diagnostics, source spans, expansion traces, project resolution, and LSP behavior are release-critical.
- **No ambient magic:** Imports, dependency resolution, mutation, error propagation, unsafe operations, and resource ownership remain inspectable.

## 4. Repository layout

A project uses this structure:

```text
my-app/
  korben.toml
  korben.lock
  src/
    main.kb
    app.kb
  tests/
    app_test.kb
  benches/
    parser_bench.kb
  assets/
  target/
```

### 4.1 Manifest

```toml
[package]
name = "hello-service"
version = "0.1.0"
edition = "2026"
description = "Example Korben service"
license = "MIT"

[dependencies]
http = "^0.1"
json = "^0.1"

[dev-dependencies]
testkit = "^0.1"

[build]
target = "native"
opt-level = 2

[ffi]
rust = ["crates/adapter"]
```

The lockfile pins the fully resolved dependency graph and source checksums.

## 5. Lexical structure and reader

Korben uses UTF-8 source files. Newlines are whitespace except where retained in string literals. The reader produces syntax objects: data structures carrying source span, lexical context, comments where needed by tooling, and expansion provenance.

### 5.1 Delimiters

- Lists: `(form ...)`
- Vectors: `[item ...]`
- Maps / records literals: `{key value ...}`
- Sets: `#{item ...}`
- Strings: `"text"`
- Raw strings: `r"text"`, with hash-delimited variants such as `r#"text"#`
- Keywords: `:name`, `:http/port`
- Symbols: `name`, `http.get`, `my-module/value`

### 5.2 Comments

```lisp
; line comment

#| block comment
   block comments may nest
|#

;;; documentation comment attached to the following declaration
```

### 5.3 Literals

```lisp
42
-17
3.14159
true
false
nil
"hello\nworld"
:ready
#uuid "550e8400-e29b-41d4-a716-446655440000"
#date "2026-08-27"
#duration "250ms"
```

`nil` is a unit-like value. Absence is represented by `Option T`, not by unchecked nullable references.

### 5.4 Reader forms

```lisp
'form            ; quote
`form            ; syntax quote
~form            ; unquote inside syntax quote
~@forms          ; splice inside syntax quote
#'name           ; function reference
#(...)           ; anonymous function shorthand
```

Reader shortcuts are expanded into canonical syntax objects before macro expansion. Reader syntax must remain intentionally small. Features that alter semantics belong in macros or core forms, not an ever-growing reader.

## 6. Modules and imports

A source module begins with a module declaration.

```lisp
(module app.main
  (use std.io)
  (use std.result :only [Result Ok Err])
  (use http :as http)
  (use json [encode decode]))
```

Module names map to paths: `app.main` resolves to `src/app/main.kb` or `src/app/main/mod.kb` according to deterministic resolver rules.

### 6.1 Visibility

Declarations are private by default. `pub` exposes a declaration from its module.

```lisp
(pub type User { id: Uuid name: String })

(pub fn find-user [id: Uuid] -> Result User UserError
  ...)
```

A module’s public API must have complete, stable types after inference. Public API annotations are recommended and may be required in strict package publishing mode.

### 6.2 Imports

```lisp
(use std.fs)
(use std.fs :only [read-text write-text])
(use net.http :as http)
(use app.models [User Role])
```

Imports are explicit and statically resolved. There are no ambient global namespaces.

## 7. Core forms

The compiler recognizes a compact set of special forms. Everything else is a function call or macro expansion.

```lisp
if
let
var
set!
fn
async-fn
match
loop
recur
quote
syntax-quote
try
throw
with
defer
unsafe
module
use
type
enum
protocol
impl
macro
pub
```

The exact surface may grow, but special forms require a strong semantic reason and toolchain support.

## 8. Functions and calls

### 8.1 Function definition

```lisp
(fn add [left: Int right: Int] -> Int
  (+ left right))

(fn greeting [name]
  (format "Hello, {name}"))
```

Functions are first-class. Parameter annotations and return annotations are optional for private functions when inference can determine a principal type.

### 8.2 Named arguments

Korben supports named arguments for declared keyword parameters, not arbitrary dynamic keyword maps.

```lisp
(fn connect [host: String :port port: Int = 5432 :tls? tls?: Bool = true]
  ...)

(connect "db.local" :port 6543 :tls? false)
```

### 8.3 Anonymous functions

```lisp
(map users (fn [user] (user.name)))
(map users #(.name %))
```

Anonymous shorthand is intentionally restricted to straightforward expressions so it stays legible and easy for tooling to rewrite.

### 8.4 Tail calls

Korben guarantees proper tail calls for direct self-recursion and `recur`. The compiler may optimize other tail calls, but only guaranteed cases form the portable semantic contract.

```lisp
(fn sum [values]
  (loop [remaining values total 0]
    (match remaining
      [] total
      [head ...tail] (recur tail (+ total head)))))
```

## 9. Data types

### 9.1 Primitive types

```text
Bool
Char
String
Bytes
Int
Int8 Int16 Int32 Int64 Int128
UInt UInt8 UInt16 UInt32 UInt64 UInt128
Float32 Float64
Unit
Never
Symbol
Keyword
```

`Int` and `UInt` are target-word-sized signed and unsigned integers. Numeric literals infer the narrowest appropriate type compatible with use sites, defaulting to `Int` or `Float64` when unconstrained.

### 9.2 Parametric types

```lisp
Option T
Result T E
Vec T
Map K V
Set T
Box T
Rc T
Arc T
Weak T
Channel T
Task T
Stream T E
```

### 9.3 Records

```lisp
(type User
  { id: Uuid
    name: String
    email: Option String
    admin?: Bool })

(let user
  { id id
    name "Mack"
    email (Some "mack@example.test")
    admin? false })
```

Records are immutable by default. Fields are accessed with `user.name` or `(get user :name)`. The compiler may use structural inference internally, while named exported records establish stable nominal boundaries.

### 9.4 Enums / tagged unions

```lisp
(enum LoginResult
  (Success user: User)
  (InvalidCredentials)
  (Locked retry-after: Duration))

(enum Option T
  (Some value: T)
  (None))
```

Enums are nominal, algebraic data types. Pattern matching must be exhaustive unless a wildcard branch is explicitly supplied.

### 9.5 Tuples and vectors

```lisp
(let point [10 20])
(let names ["Ada" "Grace" "Linus"])
```

The language distinguishes fixed-length heterogeneous tuples from homogeneous vectors based on inferred context. Users may declare explicit tuple types such as `[Int String Bool]` and vector types such as `Vec String`.

### 9.6 Maps and sets

```lisp
{:host "localhost" :port 8080}
#{:read :write :admin}
```

Map and set keys must satisfy `Hash` and `Eq`. Deterministic ordered variants live in the standard library.

### 9.7 Newtypes

```lisp
(type UserId (newtype Uuid))
(type Milliseconds (newtype Int64))
```

Newtypes prevent accidental interchange of semantically distinct values while retaining low-level representation efficiency.

## 10. Type system

Korben uses static inference with explicit annotations available wherever useful.

### 10.1 Core model

The type system includes:

- Hindley–Milner-style local type inference.
- Let-polymorphism.
- Algebraic data types and parametric polymorphism.
- Exhaustive pattern checking.
- Structural record typing for internal composition, with nominal types at explicit module and FFI boundaries.
- Protocol-constrained polymorphism.
- Effect annotations for capabilities that affect calling conventions or safety.
- Ownership and borrowing qualifiers.

The compiler must render inferred types using source-level names and avoid leaking implementation-level IR terminology into ordinary diagnostics.

### 10.2 Type annotations

```lisp
(fn parse-port [text: String] -> Result UInt16 ParseError
  ...)

(let items: Vec String (read-lines path))

(type Cache K V
  { entries: Map K V
    capacity: Int })
```

Annotations are mandatory for exported functions in `--strict-api` mode, all FFI declarations, and ambiguous recursive generic definitions where inference cannot establish a stable public interface.

### 10.3 Option and Result

There is no implicit `null` / `undefined` equivalent.

```lisp
(fn middle-name [user: User] -> Option String
  user.middle-name)

(fn load-config [path: Path] -> Result Config ConfigError
  ...)
```

The postfix `?` operator is permitted only in functions whose return type can propagate the corresponding error or absence:

```lisp
(fn load-port [path: Path] -> Result UInt16 ConfigError
  (let text (fs.read-text path)? )
  (parse-port text)? )
```

The formatter normalizes spacing and layout.

### 10.4 Protocols

Protocols are typeclass-like behavior contracts with explicit implementations.

```lisp
(protocol Renderable
  (render [self] -> String))

(impl Renderable User
  (fn render [user]
    (format "User({user.id}, {user.name})")))
```

Coherence is enforced: an implementation must be defined either by the protocol’s defining package or the implementing type’s defining package. This prevents conflicting dependency implementations.

### 10.5 Effects

Korben does not force a monad syntax for routine programming. Instead, function signatures track selected effects needed for safety, optimization, and API clarity:

```lisp
(fn read-config [path: Path] -> Result Config IoError !io
  ...)

(async fn fetch-user [id: UserId] -> Result User HttpError !async !io
  ...)
```

Initial effect kinds are `!io`, `!async`, `!alloc`, `!ffi`, and `!unsafe`. Effects can be inferred privately and must be explicit at public API boundaries in strict mode. Pure functions have no effect marker.

## 11. Pattern matching

```lisp
(match result
  (Ok user) (render user)
  (Err error) (render-error error))

(match request
  {:method :get :path "/health"} (response.ok "ok")
  {:method :post :body body} (create body)
  _ (response.not-found))
```

Patterns support literals, bindings, enum constructors, tuple/vector patterns, record patterns, typed patterns, rest patterns where collection semantics permit them, and guards.

```lisp
(match token
  (Number n) :when (> n 0) (positive n)
  (Number _) :zero
  (Identifier name) (resolve name))
```

The compiler reports unreachable branches and non-exhaustive matches with suggested missing cases.

## 12. Ownership and memory safety

Korben’s memory model is designed to make native resources safe without imposing lifetime syntax on ordinary code.

### 12.1 Categories

Values are classified by their operational behavior:

- `Copy`: inexpensive bitwise-copyable values such as booleans and fixed-width numbers.
- `Value`: immutable, safely shareable values with implementation-selected representation.
- `Owned T`: uniquely owned value; moving transfers responsibility.
- `Borrow T`: immutable scoped reference.
- `BorrowMut T`: exclusive scoped mutable reference.
- `Shared T`: explicit reference-counted sharing where identity or shared ownership is needed.
- `Managed T`: opt-in garbage-collected graph allocation, available only with an enabled managed-runtime feature.

The language presents these concepts through ordinary code and diagnostics, not raw Rust syntax.

### 12.2 Moves and copies

Resource-bearing values move on assignment, argument passing, and return unless borrowed or explicitly cloned.

```lisp
(let file (fs.open path)?)
(process file)
; file cannot be used here if process consumed it
```

Copyable values remain usable after passing:

```lisp
(let retries 3)
(schedule retries)
(log retries)
```

`clone` is explicit when it may allocate or duplicate meaningful state:

```lisp
(send channel (clone request))
```

### 12.3 Borrowing

The compiler infers short lexical borrows in ordinary calls.

```lisp
(fn title [document]
  document.title)
```

When retaining a reference, users declare it in the type:

```lisp
(fn first-line [text: Borrow String] -> Borrow String
  ...)
```

Borrowed values cannot outlive their owner, cannot be returned as unrestricted owned data, and cannot cross async task boundaries unless backed by an explicitly safe shared representation.

### 12.4 Mutation

Data is immutable by default. Mutable state uses a `var` binding or an explicit cell/container.

```lisp
(var total 0)
(set! total (+ total 1))

(let counter (Cell.new 0))
(counter.update (fn [n] (+ n 1)))
```

A mutable borrow is exclusive. The compiler prohibits aliases that would permit simultaneous mutable and immutable access inconsistent with the ownership contract.

### 12.5 Deterministic cleanup

```lisp
(with file (fs.create path)?
  (file.write text)? )

(defer (metrics.flush))
```

`with` guarantees cleanup in normal return, error propagation, and panic unwinding paths. `defer` runs when the surrounding lexical scope exits, in last-in-first-out order.

Resource types may implement the `Drop` protocol. Cleanup must not silently throw; errors during cleanup are attached to the primary failure or reported through explicit close APIs.

### 12.6 Cycles and managed memory

The default core runtime does not rely on a tracing garbage collector. Graphs with cycles require intentional representation:

```lisp
(let parent (Arc.new ...))
(let child (Arc.new ...))
; use Weak edges to break ownership cycles
```

An optional `managed` capability may provide a moving or non-moving GC heap for selected object graphs, plugin hosts, and dynamic language embeddings. Managed values cannot directly own native resources without a finalization-safe wrapper and explicit close semantics.

### 12.7 Unsafe

Unsafe operations are lexically marked:

```lisp
(unsafe
  (ffi.pointer-read pointer Int32))
```

Unsafe code cannot be called from safe code without an explicitly declared `unsafe fn` boundary. The compiler, formatter, linter, and generated documentation surface unsafe regions prominently.

## 13. Errors, conditions, and recovery

Korben distinguishes expected failures from exceptional conditions.

### 13.1 Expected failures

Use `Result T E` and `Option T` for recoverable outcomes callers are expected to handle.

```lisp
(fn decode-user [bytes: Bytes] -> Result User DecodeError
  ...)
```

### 13.2 Conditions

Conditions represent exceptional situations: invariant violations, cancellation, unavailable resources, programmer errors, and process-level failures.

```lisp
(try
  (run-job job)
  (catch IoCondition condition
    (log.error condition)
    :retry)
  (finally
    (cleanup)))
```

Korben may support a Common Lisp-inspired condition and restart system, but the initial specification limits it to typed conditions, handlers, and explicitly named restart points. Conditions must not become an untyped alternate error channel.

```lisp
(restart-case
  (open-cache path)
  (use-memory-cache [] memory-cache)
  (retry [] (open-cache path)))
```

A package must document the conditions and restarts in its public APIs.

## 14. Macros and metaprogramming

Macros operate on syntax objects, not raw lists. Syntax objects preserve lexical scope, source location, comments as required by tooling, and an expansion chain.

### 14.1 Hygienic macros

```lisp
(macro unless [condition ...body]
  `(if (not ~condition)
     (do ~@body)
     nil))
```

Identifiers introduced by a macro are hygienically scoped by default. Deliberate capture requires explicit APIs such as `datum->syntax` and is reported by lint rules.

### 14.2 Macro phases

- Reader phase: minimal syntactic sugar only.
- Expansion phase: hygienic macros run in a sandboxed compile-time environment.
- Type phase: typed macros may inspect declared or inferred interfaces only through stable compiler APIs.
- Code generation phase: procedural macros may generate declarations but cannot mutate compiler state or project files unless invoked by an explicit build script capability.

### 14.3 Macro safety and tooling

- Macro expansion has configurable time, memory, and capability limits.
- Expanded output is type checked exactly like handwritten source.
- `korben expand file.kb` prints formatted expansion with source provenance.
- Errors display a macro invocation chain and point to the relevant macro definition and call site.
- The formatter formats source, never requires users to hand-format expansion output.
- The language server can show expanded forms and generated documentation.

### 14.4 Compile-time evaluation

Compile-time functions are declared explicitly:

```lisp
(comptime fn field-names [record-type]
  ...)
```

They run in a restricted deterministic environment by default. Filesystem, network, clock, environment, and process access require declared build capabilities in `korben.toml`.

## 15. Concurrency and async

Korben provides structured concurrency. Tasks belong to a lexical task scope unless deliberately detached.

### 15.1 Async functions

```lisp
(async fn fetch-profile [id: UserId] -> Result Profile HttpError !async !io
  (let response (await (http.get (url "/profiles/{id}"))))
  (response.json Profile))
```

`await` is valid only inside `async fn`, `async`, or explicitly asynchronous blocks.

### 15.2 Task scopes

```lisp
(async fn load-dashboard [ids: Vec UserId] -> Result (Vec Profile) DashboardError
  (task-scope scope
    (let tasks (map ids (fn [id] (scope.spawn (fetch-profile id)))))
    (join-all tasks)?))
```

When a scope exits, its child tasks complete, are cancelled, or are explicitly transferred to a supervisor. A task must not silently outlive the operation that created it.

### 15.3 Channels and cancellation

```lisp
(let [sender receiver] (channel.bounded 100))
(await (sender.send event))
(let event (await (receiver.recv)))
```

Channels are typed and support bounded/unbounded modes. Cancellation is cooperative, typed, and propagated through task trees. Blocking calls are isolated through runtime adapters.

### 15.4 Shared state

Shared mutable state is explicit via `Mutex`, `RwLock`, atom-like transactional cells, actors, or channels. The compiler rejects non-safe references crossing task boundaries.

## 16. Standard library

The standard library is versioned with the compiler and maintains a narrow, reliable base.

### 16.1 Core modules

```text
std.core       Basic functions, equality, ordering, conversion
std.option     Option helpers
std.result     Result helpers and propagation utilities
std.collections Vec, Map, Set, iterators, persistent structures
std.string     Unicode-aware strings and parsing
std.bytes      Byte buffers, codecs, binary utilities
std.fs         Filesystem and paths
std.io         Readers, writers, streams
std.net        TCP, UDP, addresses, DNS
std.http       HTTP client and server primitives
std.json       JSON encoding and decoding
std.time       Instants, durations, clocks, scheduling
std.process    Subprocesses, signals, environment
std.async      Tasks, channels, cancellation, streams
std.crypto     Approved cryptographic primitives and secure randomness
std.log        Structured logging and tracing interfaces
std.test       Test declarations, assertions, property tests
std.ffi        Safe FFI primitives and ABI declarations
```

### 16.2 Collections

Collections prioritize predictable semantics and avoid accidentally quadratic convenience APIs. The standard library documents allocation and copying behavior for every operation.

Persistent immutable collections are preferred in high-level APIs. Mutable builders and buffers exist for performance-critical work and are explicitly named.

### 16.3 Serialization

Types may derive codecs through macros or compiler-supported derives:

```lisp
(derive User [Json Encode Decode Eq Hash])
```

Generated codecs fail with typed, path-rich errors. Serialization formats are libraries, not privileged language syntax.

## 17. FFI

FFI is a primary Korben feature, but foreign unsafety must remain contained.

### 17.1 FFI design goals

- Typed declarations.
- ABI-aware layouts.
- Explicit ownership transfer rules.
- Generated bindings where metadata is available.
- Safe wrapper APIs as the ordinary user-facing form.
- `unsafe` required for raw pointers, unchecked layout assumptions, and untrusted foreign contracts.

### 17.2 C FFI

```lisp
(ffi/c-library "sqlite3")

(ffi/c-fn sqlite3-open
  [filename: CStr out-db: (Ptr (Ptr SqliteDb))]
  -> CInt)
```

The binding generator can consume C headers through libclang-compatible tooling. Generated raw bindings are isolated in a module; a safe Korben wrapper exposes `Result`, owned handles, and deterministic cleanup.

### 17.3 Rust FFI

Rust libraries expose Korben-compatible APIs using a `korben-export` adapter crate and generated metadata.

```rust
#[korben_export]
pub fn slugify(input: String) -> Result<String, KorbenError> {
    // implementation
}
```

```lisp
(use rust.slugify [slugify])

(slugify "Korben Is Fast")
```

The Rust adapter defines ABI-safe representations, error conversion, ownership transfer, callbacks, async interoperability, and version compatibility. Korben does not claim direct safe calling of arbitrary Rust APIs merely because both tools are implemented in Rust.

### 17.4 WASM and dynamic libraries

Korben supports WASM modules and dynamic libraries as capability-scoped plugin or embedding boundaries. Their imports, exports, memory limits, host calls, and version requirements are declared in manifests.

### 17.5 FFI restrictions

Borrowed Korben references cannot cross a foreign boundary unless the binding proves duration and thread safety. Foreign callbacks must declare threading and reentrancy behavior. Dynamic values crossing FFI have runtime tags and validation where necessary.

## 18. Compilation and runtime

### 18.1 Compiler pipeline

```text
Source text
  -> Reader and concrete syntax tree
  -> Syntax objects and module resolution
  -> Hygienic macro expansion
  -> Parsed abstract syntax tree
  -> Name resolution
  -> Type, effect, and ownership analysis
  -> Typed core IR
  -> Optimization and monomorphization
  -> Backend lowering
  -> Native executable, library, WASM module, or bytecode artifact
```

Every phase preserves source provenance sufficiently for diagnostics, debugging metadata, macro trace reporting, and editor features.

### 18.2 Execution modes

Korben has two compatible execution modes:

- **Development bytecode:** Fast startup, REPL evaluation, debugger hooks, tests, macro evaluation, and iterative workflows.
- **Native release compilation:** Optimized machine code for distributable executables, libraries, and performance-sensitive services.

Both modes share reader semantics, macro behavior, type checking, effect checks, ownership rules, and observable language semantics.

### 18.3 Initial backend strategy

The first native backend may lower typed Korben IR to generated Rust and invoke an isolated Rust compilation pipeline. Generated source is internal and reproducible with a diagnostic flag.

This is a bootstrapping strategy, not an eternal user-facing dependency. Later versions may use Cranelift for fast native compilation and/or LLVM for maximum optimization. Backend replacement must preserve the language ABI and observable behavior.

### 18.4 Runtime scope

The runtime is intentionally small:

- Memory allocation support for language values.
- Async scheduler and I/O reactor.
- Panic/condition boundary handling.
- String, collection, reflection-metadata, and FFI support.
- Structured logging/tracing hooks.

The runtime must not impose a large framework, mandatory garbage collector, hidden web server, or ambient global execution model.

### 18.5 Build modes

```sh
korben run src/main.kb
korben build
korben build --release
korben build --target wasm32-wasi
korben build --emit ir
korben build --emit rust
```

Default development builds optimize for incrementality and diagnostics. Release builds optimize for executable speed and size according to explicit profiles.

## 19. REPL and inspector

`korben repl` starts a project-aware interactive environment.

```text
$ korben repl
Korben 0.1.0
project: hello-service

kb> (type-of (map [1 2 3] inc))
Vec Int

kb> (use app.models [User])
kb> (expand unless)
...
kb> :reload
kb> :tests app.handlers
```

### 19.1 REPL requirements

- Load the current project and resolve normal dependencies.
- Preserve definitions across evaluations where sound.
- Recompile changed modules incrementally.
- Display inferred types, docs, source locations, and macro expansions.
- Render structured values with depth and size controls.
- Support commands for reload, test selection, package inspection, task inspection, and runtime metrics.
- Require explicit confirmation before evaluating forms marked as process-spawning, destructive filesystem, or unsafe operations when safe REPL mode is enabled.

## 20. Toolchain

The `korben` binary provides the standard workflow.

### 20.1 Commands

```text
korben new [template]
korben init
korben run [entry]
korben dev
korben build
korben check
korben test
korben bench
korben fmt
korben lint
korben doc
korben repl
korben add
korben remove
korben update
korben publish
korben install
korben expand
korben inspect
korben doctor
korben lsp
korben ffi
```

### 20.2 Formatter

`korben fmt` is canonical and stable. A source file has one preferred formatting result for each compiler version. The formatter is macro-aware, preserves comments, treats reader forms correctly, and formats all project source by default.

Formatting changes are versioned and documented. The formatter never requires a separate runtime or third-party editor plugin.

### 20.3 Diagnostics

Compiler errors include:

- A concise human explanation.
- Source spans and relevant secondary spans.
- Expected versus received types using source-level naming.
- Macro expansion backtraces where applicable.
- Ownership origin, move, and borrow paths where applicable.
- Actionable suggestions only when they are confidence-safe.
- Stable machine-readable JSON diagnostics for editors and CI.

### 20.4 Language server

`korben lsp` implements the Language Server Protocol and includes:

- Completion and signature help.
- Go to definition, references, implementation, and type definition.
- Hover for inferred types, effects, ownership mode, docs, and expansion origin.
- Rename with macro hygiene awareness.
- Code actions for imports, match branches, type annotations, error propagation, and safe ownership repairs.
- Incremental workspace diagnostics.
- Semantic tokens and inlay hints.
- Formatter integration.

### 20.5 Testing

```lisp
(test "parses a port"
  (assert-eq (Ok 8080) (parse-port "8080")))

(property "encoding round trips" [user gen-user]
  (assert-eq (Ok user) (decode-json User (encode-json user))))
```

The runner supports unit tests, property tests, table-driven tests, snapshots, async tests, timeouts, parallel execution with deterministic reporting, focused tests, coverage instrumentation, and JSON output.

### 20.6 Linting and security checks

The built-in linter includes rules for unused bindings, unreachable branches, accidental allocation, ignored results, unsafe escape scope, dynamic FFI boundaries, blocking work in async functions, nondeterministic tests, and public API documentation.

`korben audit` checks dependency advisories, lockfile integrity, licenses, package provenance, and known compromised releases where registry data is available.

## 21. Package management

Packages are immutable versioned archives containing source, manifest, lock metadata, checksums, signatures where available, and generated API metadata.

### 21.1 Dependency resolution

- Semantic version ranges in manifests.
- Deterministic global resolution rules.
- Fully pinned lockfile.
- Content-addressed local cache.
- Offline build support after dependencies are fetched.
- Workspace support for monorepos.
- Registry, Git, path, and signed archive dependencies.

### 21.2 Package quality expectations

Published packages should provide documentation, license information, supported compiler range, public API metadata, effect/unsafe declarations, and reproducible build settings.

Packages may expose macros, native artifacts, build scripts, and FFI modules only through explicit manifest capabilities.

### 21.3 Supply chain protections

- Checksums are mandatory.
- Lockfiles retain source identity.
- Install scripts are prohibited by default.
- Build scripts are sandboxed and capability-declared.
- Dependency native code is compiled locally or fetched only from verifiable signed artifacts according to user policy.
- `korben doctor` reports weakened verification settings.

## 22. Security model

### 22.1 Capability-oriented APIs

Sensitive operations require explicit capability-bearing objects or configured grants, rather than universal ambient access.

```lisp
(fn import-file [fs-cap: FsRead path: Path] -> Result Bytes IoError !io
  (fs.read fs-cap path))
```

Applications may choose a convenience prelude that supplies process-level capabilities, but libraries should receive capabilities explicitly.

### 22.2 Build and macro sandboxing

Macros and build scripts run with no network, filesystem, process, environment, clock, or random capability by default. Required capabilities are declared in the manifest and visible during install/build.

### 22.3 Unsafe containment

Unsafe operations are lexical, typed, lint-visible, documented in public APIs, and transitive in dependency metadata. Safe packages cannot silently make a caller’s memory safety depend on an undocumented unsafe implementation.

### 22.4 Cryptography

The standard library exposes vetted, high-level protocols and algorithms rather than encouraging custom cryptographic constructions. Unsafe or low-level primitives remain separate and documented with misuse risks.

## 23. Observability and debugging

Korben supports structured logging, tracing spans, metrics interfaces, panic reports, and debugger metadata from the first stable release.

```lisp
(with-span "request"
  {:request-id request.id}
  (log.info "handling request")
  (handler request))
```

The compiler emits source maps, native debug symbols, and macro expansion provenance. The inspector can display task trees, active spans, allocation counters where enabled, owned-resource leaks detected at shutdown, and typed stack traces.

Profiling integrations should support CPU, allocation, async wait, and lock-contention analysis without changing ordinary source code.

## 24. Documentation

Documentation is generated from module declarations, `;;;` documentation comments, signatures, examples, protocol implementations, conditions, effects, ownership modes, and package metadata.

```lisp
;;; Reads an entire UTF-8 file.
;;; Returns `Err InvalidUtf8` when the bytes are not valid UTF-8.
(pub fn read-text [path: Path] -> Result String IoError !io
  ...)
```

`korben doc` produces static documentation and machine-readable API descriptions. Doctests compile and run as part of tests unless explicitly marked otherwise.

## 25. Versioning and compatibility

### 25.1 Language editions

Language changes that could alter valid program meaning require a named edition. Projects select an edition in `korben.toml`.

```toml
[package]
edition = "2026"
```

The compiler supports a documented migration path and automated fix tool where feasible.

### 25.2 Semantic versioning

- The compiler, standard library, runtime ABI, package metadata, and tooling protocol each have documented compatibility versions.
- Stable public Korben package APIs follow semantic versioning.
- Experimental features require explicit feature gates.
- Native FFI ABI stability is opt-in and separately versioned.

### 25.3 Stability policy

The compiler may evolve rapidly before 1.0, but source-breaking changes must be accompanied by migration notes and, where realistic, automated code transformations. Formatter output is considered part of the developer experience contract.

## 26. Performance requirements

Performance claims are measured, not assumed.

### 26.1 Baseline targets

- Fast incremental `korben check` for edited modules.
- Near-instant startup for ordinary command-line programs.
- Competitive throughput for native network and data-processing workloads.
- Predictable allocation behavior documented in standard-library APIs.
- No mandatory tracing GC pauses in the default runtime.
- Low-overhead async tasks suitable for many concurrent I/O operations.

### 26.2 Benchmarking

The repository maintains benchmarks for:

- Compiler cold and incremental builds.
- REPL evaluation and module reload latency.
- CLI startup.
- HTTP throughput and tail latency.
- JSON encoding/decoding.
- Collection operations.
- Async scheduling.
- FFI call overhead.
- Binary size.

Benchmarks are published with target hardware, compiler version, build flags, workload source, and comparison methodology.

## 27. Initial roadmap

### Milestone A: Usable core

- Reader, parser, source spans, module resolver.
- Basic evaluator or bytecode VM.
- Functions, records, enums, pattern matching, immutable collections.
- Local inference and errors.
- Canonical formatter.
- Project-aware REPL.
- `new`, `run`, `check`, `fmt`, and `test`.

### Milestone B: Practical typed applications

- Result/Option propagation.
- Protocols and derives.
- Files, process, JSON, HTTP, logging, and testing libraries.
- LSP MVP.
- Documentation generator.
- Package manifests, lockfiles, local workspaces.
- Deterministic resource scopes.

### Milestone C: Native and safe systems boundary

- Ownership analysis for resource types.
- Borrow checking for retained references and mutation.
- C FFI generation and safe wrappers.
- Rust adapter ABI and binding generator.
- Native code generation via Rust lowering or Cranelift.
- Release artifacts and cross compilation.

### Milestone D: Production platform

- Structured concurrency and async runtime.
- Auditing and package provenance.
- Profiler/debugger integrations.
- WASM/plugin host.
- Registry publishing.
- Stable package and runtime ABI policy.

## 28. Acceptance criteria for v0.1

Korben v0.1 is ready only when a new user can:

1. Install one executable.
2. Create a project with `korben new`.
3. Write a typed HTTP or CLI program using records, enums, pattern matching, `Result`, modules, and tests.
4. Run it through `korben dev` with immediate diagnostics.
5. Format and test it using built-in commands.
6. Explore it in a REPL that knows project dependencies and displays inferred types.
7. Produce a native release binary with `korben build --release`.
8. Consume one small C or Rust library through a generated, typed binding and safe wrapper.
9. Receive human-readable errors for ordinary type mismatches, macro expansion failures, non-exhaustive matches, and ownership violations.
10. Reproduce the build from a lockfile without executing undeclared install scripts.

## 29. Reference example

```lisp
(module app.main
  (use std.result [Result Ok Err])
  (use std.json)
  (use std.log)
  (use http))

(type Greeting
  { message: String })

(type AppError
  (enum
    (BadRequest message: String)
    (Internal cause: String)))

(fn greeting-handler [request: http.Request]
  -> Result http.Response AppError !io
  (match request
    {:method :get :path "/health"}
    (Ok (http.text 200 "ok"))

    {:method :get :path "/greeting" :query {:name name}}
    (Ok (http.json 200 { message (format "Hello, {name}") }))

    {:method :get :path "/greeting"}
    (Ok (http.json 200 { message "Hello, world" }))

    _
    (Ok (http.json 404 { error "not found" }))))

(async fn main [] -> Result Unit AppError !async !io
  (log.info "starting server" {:port 3000})
  (http.serve {:port 3000} greeting-handler)
  (Ok ()))

(test "health endpoint is available"
  (let response (greeting-handler (http.test-request :get "/health"))?)
  (assert-eq 200 response.status)
  (assert-eq "ok" response.body))
```

## 30. Final definition

Korben is successful when a programmer can use Lisp as their everyday language for shipping a safe, fast native program—and when the surrounding experience is more coherent than assembling Rust, Cargo, a formatter, an LSP, a task runtime, bindings, a test framework, and deployment tooling by hand.

Its differentiator is not any isolated feature. It is the integration of Lisp abstraction, static guarantees, ownership-aware systems programming, native execution, and exceptionally opinionated tooling into one language platform.

