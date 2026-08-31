# korbead: proving korben on a real HTTP service

**Status:** approved design, not yet implemented
**Epic:** `korben-8fl`
**Date:** 2026-08-30

## Why

`review.md` scores korben 8/10 and names three things that would move it to 9:
resumable task state machines, type-directed runtime specialization, and one
production-quality deployment target. Those are inferences drawn from reading a
specification. Two of them are expensive, and reading the code shows at least
one is mis-aimed.

The goal is to make korben usable for real work. So rather than execute the
review's list, build the thing and let it say what is actually missing.

### What reading the code already changed

Four findings, none of which appear in the review:

1. **`net.rs:375-584` provides a connection pool** — `pool_wait`, `pool_read`,
   `pool_evict` — doing readiness multiplexing below the language. A korben HTTP
   server already serves many concurrent connections without task suspension.
   The async limitation bites CPU-bound handlers, fairness, and preemptive
   cancellation, not the concurrent-I/O case the review implies.

2. **`MapValue` (`value.rs:41`) is a linear scan**, and `stdlib/http.kb:228-240`
   keeps `pending` as a map keyed by connection id, `dissoc`-ing inside a
   `reduce`. Connection bookkeeping in the HTTP server is quadratic in
   concurrent connections. This is on the request path today.

3. **Type-directed unboxing has an unnoticed prerequisite.**
   `infer::check_session` returns nothing; it emits diagnostics and discards the
   inference results. `ir.rs` carries no inferred types, and
   `codegen::generate` never sees the checker. There is no type information at
   codegen time to direct anything.

4. **Nothing handles signals.** `stdlib/http.kb:223` `serve` is an unbounded
   `loop`/`recur` with no exit path. A deployed service can only be killed.

## Hard constraint

**No changes to korben during this project.** Not the runtime, not the stdlib,
not the compiler. Every wall hit is filed as a bead and left alone.

The friction log is the deliverable. "Production-quality deployment target"
becomes *here is exactly what stands between korben and a production service,
with evidence* — more honest and more useful than patching around the gaps
while building.

## What gets built

`korbead` — an HTTP/JSON service written in korben, serving this repository's
beads data, with its own write-side annotation store.

### Read and write split

Beads' architecture makes `.beads/issues.jsonl` a passive export; mutations go
through Dolt. Korben has no process spawn and no MySQL driver, so the service
cannot legitimately write beads state. Therefore:

- **Read side** — a snapshot from `bd list --all --json` (54 issues, ~20 fields
  each), loaded at startup with `fs.read-text` and `json.decode`.
- **Write side** — per-issue notes the service owns outright, persisted to its
  own file. Real write traffic, no conflict with beads' sync model.

### Endpoints

| Method | Path | Notes |
|---|---|---|
| GET | `/health` | |
| GET | `/issues` | filter by status, type, priority, text |
| GET | `/issues/{id}` | oracle: `bd show {id} --json` |
| GET | `/ready` | oracle: `bd ready --json` |
| GET | `/stats` | oracle: `bd stats` |
| POST | `/issues/{id}/notes` | write path |
| GET | `/issues/{id}/notes` | |

Routing is pattern matching over the request record, as `examples/http.kb` does.

### Correctness oracle

`/stats`, `/ready`, and `/issues/{id}` must diff clean against `bd`. That turns
"does korben work" from a judgment call into a diff.

### Location

Its own directory **outside the korben repository**, driven by a real
`cargo install --path crates/korben-cli`. This forces the actual user path
(install, `korben new`, `korben build`, a package that is not a member of the
compiler workspace), and makes the no-changes constraint physical rather than a
promise. `korben` is not on PATH today, so that path has never been exercised.

### Out of scope

Auth, TLS, pagination, write-through to beads, hosted deployment.

## Known gaps, deliberately not fixed

Each is filed and labelled `runtime-gap`, deferred by decision:

- No signal handling, so no graceful drain. The service will be killed, and the
  README will say so.
- No `std.fs/rename`, so the notes overlay is rewritten whole and
  non-atomically. The corruption window is documented, not worked around.
- `File` is write-only; the snapshot is read with `fs.read-text` instead.

## Instrumentation

Three falsifiable hypotheses. Each confirms or kills one review item, and a
negative result is as useful as a positive one.

- **H1 — maps dominate.** Predicts *superlinear* latency growth from 64 to 256
  concurrent connections. If true, a real hash map is the highest-value runtime
  fix, and the review never mentions it.
- **H2 — unboxing is irrelevant here.** This workload is string, map, and JSON
  work with almost no arithmetic. Predicts unboxed `Int`/`Float` barely moves
  the profile. If true, review item 2 is deprioritized for service workloads and
  its expensive `infer -> IR` prerequisite is deferred with it.
- **H3 — resumable tasks are not the bottleneck.** `net.pool` multiplexes below
  the language. Predicts throughput flat regardless of handler await structure.
  If true, review item 1 is a semantics problem, not a performance one.

Method: `oha` at 1/8/64/256 concurrency; per-request latency from
`std.time/now-millis` in the log line; the same service run interpreted and
native to measure the asserted 2x ratio rather than assert it; `samply` on the
native binary for a flame graph.

If 54 issues proves too small to stress the data path, synthesize a larger
corpus rather than pretend it is enough.

## Sequence

1. Install korben and scaffold `korbead` outside the repo (`korben-8fl` child).
2. Snapshot loader and issue model.
3. Read endpoints, with korben's own test runner covering pure logic —
   filtering and `/ready` dependency resolution.
4. Notes overlay write path.
5. Oracle diff harness against `bd`.
6. Load and profile run; test H1, H2, H3.
7. Findings write-up; set the runtime-gap beads' priorities from measurements
   rather than from the review's guesses.

Steps 1-5 are the artifact you keep. Steps 6-7 settle the roadmap.

## Risks

- **The dataset is small.** 54 issues stresses the connection path but not the
  data path. Mitigation: synthesize issues for the load run.
- **The write path is thin.** A notes API is modest. If it proves too thin to
  generate meaningful write traffic, widen it rather than inflate the numbers.
- **The project may find the compiler was fine.** That is a valid outcome and
  should be reported as one.
