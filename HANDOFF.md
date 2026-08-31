# Handoff

Written 2026-08-31, at the end of the session that closed korbead and every
open P0 and P1.

Read this with `bd ready`. The beads carry the detail; this carries the shape.

## Where korben stands against its review

`review.md` names three things that would move korben from 8 to 9. Two are
done as far as they usefully go. The third has not been started and has no
bead, which is the single biggest gap in the tracker.

| | |
|---|---|
| 1. Resumable task state machines | **Done.** Tasks have their own stacks and genuinely suspend; sockets wake parked tasks through a reactor; a computing task yields so it cannot hold the server. |
| 2. Type-directed unboxing | **Two slices done**, measured 2.8x then 4.8x on arithmetic-heavy code. Values are still tagged: this removed dispatch, not boxing. |
| 3. One production-quality deployment target | **Not started, not filed.** |

Item 3 is the one to think about first. A fair amount of what shipped
already points at it without being organised as it: structured logging with
levels and RFC3339 timestamps, SIGINT/SIGTERM handling, TLS, `fs/rename`,
offline reproducible builds. What is missing is the decision about *which*
target, and then making build, deploy, logging, configuration, signals and
failure behaviour excellent for that one thing rather than adequate for
several. That is a design conversation before it is code.

## What is open

Five P2s, no P0 and no P1.

- `korben-baq` — no TLS server, and TLS reads block the runtime. The biggest
  of the five, and more tractable than when it was filed: the reactor from
  `korben-8h8` is the machinery a non-blocking TLS read needs.
- `korben-7nj` — a parametric record cannot be annotated. Its literal infers a
  structural record that does not unify with the applied type. Finishes what
  `korben-msz` started; the fix is the same instantiate-then-unify shape,
  applied at the construction end.
- `korben-8t6` — no string substring or slice, so a buffer cannot be split at
  an offset.
- `korben-1f6` — `File` is write-only; no read from an open handle.
- `korben-kd1` — the stdlib is not canonically formatted by korben's own
  formatter. Cheap, and embarrassing in a way that matters: the formatter
  being canonical rather than preference-driven is a marketed property.

## The pattern worth hunting

Five bugs this session were the same shape: **one part of korben not knowing
what another part already did.** Not missing features — missing wiring.

- `korben test` did not call the checker that existed, so a green suite said
  nothing about whether the code compiled.
- `chart_session` re-ran inference instead of reading what `check_session` had
  already computed, and got a second opinion.
- `construct` did not apply the keyword rule `bind_args` already documented.
- `through` did not consult the `globals` map that existed.
- The checker rejected named record construction that `apply::construct` had
  always supported.

A deliberate pass looking for capabilities one layer offers and another
refuses may be worth more than the next individual bead. The parts of this
codebase are good; the seams are where the defects live.

## Traps this session actually hit

Not theory. Each of these cost real time or nearly shipped a wrong result.

**The generated crate is a second implementation that only the differential
tests can see.** Three separate times: a new runtime module missing from
codegen's vendored file list; the generated `Host` impl living inside a string
constant, so a workspace-wide signature search missed it; and the generated
entry point not installing a task host. Any change to the runtime's calling
convention has a second implementation somewhere in `codegen.rs`.

**A test that passes both with and without your change is not testing your
change.** The reactor and the preemption work were each verified in both
directions — `KORBEN_NO_POLL=1` and a raised yield budget — and both turned a
plausible pass into a real one. For anything about fairness or scheduling,
do this.

**Check the bead before believing it.** `korben-6nt` blamed heterogeneous
maps; that case no longer reproduced, and the real causes were two others.
`korben-8fl.8` was filed as not independently actionable, which was true when
written and false by the time it was picked up.

**Measure the measurement.** During korbead, a control endpoint exercised the
same quadratic path it was supposed to control for, and a corpus synthesizer
stripped the field that made the path quadratic at all. Either would have
produced a confident false conclusion.

**Reading types out of the checker's tables requires instantiating them
first.** A bound variable read raw into an inference context aliases a real
inference variable, so a field would quietly agree with whatever the
surrounding code wanted — worse than returning `Unknown`, which is at least
honest.

**A `RefCell` guard is a temporary.** `map.borrow().get(k)?` borrows something
already dropped. Every lookup that used to hand back a reference now clones.

**`Session` and the type `Checker` have fields named `modules` and `types`
too.** A mechanical rewrite keyed on a field name alone edits the wrong
struct.

**Commit `Cargo.lock` with a dependency change.** It went stale for several
commits because a `git add` was scoped to `crates/`. CI regenerates the lock,
so it stayed green and hid it.

## How work gets done here

Bead first, always — no code without one. Branch named exactly for the bead,
merge `--no-ff`, delete the branch, close the bead with what was learned
rather than what was done. The closing notes are the memory; several of them
saved time later in this same session.

Gates before every merge: `cargo fmt --all`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace`. Currently 385 tests
across 33 suites, zero failures. Then push and **watch CI** — one commit sat
red for twenty-four minutes because it was pushed and forgotten.

## Two costs taken on purpose

Both are worth knowing before someone reads them as mistakes.

**`corosensei` is not optional the way TLS is.** Async is a language feature,
and a build flag that changed its semantics would be two concurrency models
wearing one name. So every generated project now carries a dependency where it
previously carried none. The offline-build guarantee still holds; the
zero-dependency property no longer does.

**Closing the type-parameter hole broke real code, and will break more.**
`(if-let n (Some 42) n :none)` had branches of type `Int` and `Keyword`,
which korben's own `if` rule has always forbidden — it only ever passed
because the payload was unknown. Code that leaned on generics being unchecked
now has to agree with itself. That is the point, but it will look like a
regression to anyone who did not expect it.
