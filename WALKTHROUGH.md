# A walkthrough of the toolchain

Every command below was run, and every output is what it printed. Paths are
shortened; nothing else is edited.

The language has examples of its own in `examples/`. This is about the tools
around it: publishing a package, fetching one, building for another machine, and
finding out where a program spends its time.

## Start something

```console
$ korben new greeter --template lib
created project `greeter` (lib template)

  cd greeter
  korben dev
```

`korben dev` is check, test, and run in one command. Three templates: `cli`,
`lib`, and `service`.

## Publish it

A registry is a directory laid out as `<name>/<version>/`. Publishing copies the
package into one.

```console
$ korben publish --registry ../registry
published greeter 0.1.0 to ../registry/greeter/0.1.0
  checksum sha256:240af11255d508d0093965347a3d6c8e3edbc16fb35832820abf965668a03a75
  contents 1 source file
```

That checksum is what a lockfile will pin, so publishing refuses anything it
would be lying about: a package that does not type-check, a version that already
exists — every lockfile that pinned it would start lying — and a package that
depends on a local path, which nobody installing it will have.

## Share it

Make the registry a git repository and it can be fetched. Nothing about the
layout changes; a registry is a repository laid out like a local one.

```console
$ korben install
cloned ../registry
  unverified: carries no signature
  at ~/.korben/registries/1c267594e24dfb2b
  offering 1 package
```

`install` is the only command that touches a network. Resolution reads the local
clone and nothing else, so builds are offline whether or not the clone is
current.

Provenance is git's: if the registry's latest commit is signed with a key you
trust, `install` says `verified` and names the signer. Setting `signed = true`
under `[registry]` refuses a registry that is not, and removes the clone rather
than leaving unverified content behind. `korben audit` reports the weaker
setting:

```console
$ korben audit
supply chain
  ok install scripts are prohibited and never executed
  ok checksums are verified on every build
  weakened: ../registry is fetched without checking any signature
       Set `signed = true` under `[registry]` to require one.
```

## Depend on it

```console
$ korben add greeter --version "^0.1"
locked
  greeter 0.1.0  registry+git+../registry
```

The lockfile names the registry by URL rather than by the directory this machine
cloned it into, so it is the same file on every machine that shares it.

## Build it for somewhere else

```console
$ korben build --release
built target/release/service (release, 701 KB)
  generated crate: target/codegen/release
```

The backend lowers to Rust and hands it to cargo, so a build can target any
triple that toolchain has. The triple is checked before cargo runs, because
cargo reports a missing target as a missing `std` crate, which explains nothing:

```console
$ korben build --target x86_64-unknown-linux-gnuu
build failed: `x86_64-unknown-linux-gnuu` is not a target rustc knows
  Did you mean `x86_64-unknown-linux-gnu`?
  `rustc --print target-list` lists every target.

$ korben build --target x86_64-unknown-linux-gnu
build failed: the standard library for `x86_64-unknown-linux-gnu` is not installed
  Install it with `rustup target add x86_64-unknown-linux-gnu`.
```

With the target installed it builds, and the artifact lands under its triple so
two targets never overwrite each other:

```console
$ korben build --target wasm32-unknown-unknown
built target/wasm32-unknown-unknown/debug/service.wasm (debug, wasm32-unknown-unknown, 7.2 MB)
```

## Find out where the time went

```console
$ korben run --profile
Hello, world!

PROFILE
  self is time in that function's own body, with everything it called
  subtracted, so a recursive call is not counted twice.

       calls        self   share  function
           1     207.0µs   56.0%  args
           1     156.0µs   42.2%  main
           1       3.6µs    1.0%  println
           1       1.9µs    0.5%  greeting
           1       917ns    0.2%  Greeting
```

Nothing in the program changes to be profiled: every call goes through one place
in the runtime, so user functions, builtins, protocol methods, and constructors
are all covered.

The number is *self* time — the time inside a function's own body, with
everything it called subtracted. Inclusive time is the more familiar figure, but
it double-counts a recursive call and has to be explained every time it is read.
Self time says plainly which body the program is sitting in. On a program that
sums a range, the table reads `reduce 59%`, `+ 39%`, which is the true answer.

The report goes to stderr, so a profiled run still pipes cleanly, and it prints
even when the program failed — where the time went is often exactly the question
being asked about a program that did not finish.
