# Several packages in one repository

A workspace is a root that owns its members. The root manifest declares no
package of its own — `members` is the whole of it:

```toml
[workspace]
members = ["toolkit", "report"]
```

- `toolkit/` is a library. Nothing about it says it is part of a workspace.
- `report/` depends on it the way it would depend on anything else, by name and
  version requirement, with **no path**: the root already knows where its
  members are.

```toml
[dependencies]
toolkit = "^1.0"
```

## What the workspace changes

```sh
korben check     # every member, not just one
korben test      # every member
korben fmt       # every member
korben run       # the member that has a program, chosen for you
```

Resolution runs once across all of it and produces **one `korben.lock` at the
root**, so two members that share a dependency get the same version of it rather
than two independent answers. A member is recorded as `member+<name>` rather
than as a path or a registry.

Members are deliberately not verified against a checksum on every build. A
checksum pins a dependency so that what is built is what was reviewed; a member
is source in this repository being edited, and checking it would stop the build
on every keystroke to demand `korben update`. Everything from outside the
workspace is still pinned.

With more than one member defining a program, `run` and `build` refuse to guess
and ask for `--package <name>`.
