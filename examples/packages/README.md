# A build reproduced from a lockfile

Specification section 28's tenth acceptance criterion is that a user can
reproduce a build from a lockfile without executing undeclared install scripts.
The other examples are single files; this one needs two packages and a manifest,
so it is a directory.

- `greeting/` is a dependency: a `korben.toml`, a `src/`, and the `pub`
  declarations importers can see.
- `app/` depends on it by path and by version requirement, and commits the
  `korben.lock` that resolution produced.

## Run it

```console
$ cd app
$ korben run
Hello, Ada!
Hello, world!
```

## What the lockfile pins

`app/korben.lock` records the resolved version, where the package came from, and
a SHA-256 over its contents:

```toml
[package.greeting]
version = "1.2.0"
source = "path+../greeting"
checksum = "sha256:86d72aa9..."
```

Resolution runs to a fixpoint, so the version it settles on does not depend on
the order requirements are encountered. Given the same inputs it writes the same
lockfile, byte for byte.

## What it protects

Every build verifies the checksum before reading a line of the dependency. Edit
`greeting/src/greeting.kb` and the next build stops:

```console
$ korben run
error: dependency `greeting` has changed since it was locked
  locked:  sha256:86d72aa9...
  found:   sha256:49ea04bf...
  run `korben update` to accept the change
```

Accepting a change is a deliberate act — `korben update` rewrites the lock — and
never a side effect of building.

Install scripts are not sandboxed; they are refused. A manifest declaring
`install`, `[scripts]`, or `[build] script` fails to parse, so a package
carrying one is rejected before resolution can even consider it:

```console
$ korben update
error: dependency `greeting` at ../greeting: `postinstall` in `[scripts]` would
run code at install time, which is not allowed
  specification 21.3: install scripts are prohibited by default
```

There is no phase of the build in which a dependency runs arbitrary commands.
`korben audit` reports the whole picture:

```console
$ korben audit
dependencies
  ok greeting 1.2.0  sha256:86d72aa9...

supply chain
  ok install scripts are prohibited and never executed
  ok checksums are verified on every build

findings
  - `greeting` comes from a local path, so the lock is not portable
```

That last finding is honest rather than incidental: path and local-registry
sources work today, and a network registry is still ahead.
