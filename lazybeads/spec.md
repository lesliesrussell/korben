A CLI-first, Go-based operator layer over Beads, with a scriptable command surface and an optional terminal UI. The core is **not** a replacement issue tracker; it is a decision, navigation, and health layer that calls `bd` as the only authority for Beads data and mutations. [github](https://github.com/gastownhall/beads)

# LazyBeads Specification

## 1. Product definition

### Name

- **Project name:** LazyBeads
- **Repository name:** `lazybeads`
- **Binary name:** `lb`
- **Tagline:** “The terminal operator console for Beads.”
- **Primary invocation:** `lb`
- **Compatibility target:** Current stable Beads CLI, with explicit version probing and capability-based degradation.

### One-sentence value proposition

LazyBeads helps a human supervising one or more coding agents quickly discover the highest-leverage available Beads task, understand dependency and ownership state, safely mutate work, and diagnose workspace health.

### Core product promise

```text
Tell me what is worth doing next.
Tell me why.
Let me act on it safely without losing terminal composability.
```

### Primary user

A developer or technical lead using Beads in a local repository, remotely over SSH, inside a devcontainer, in tmux, or alongside coding agents.

This is deliberately terminal-first. Beads itself is positioned as a CLI installed system-wide, with JSON output, graph dependencies, atomic claiming, Dolt-backed persistence, and work-ready detection. LazyBeads layers on top rather than replacing those primitives. [github](https://github.com/gastownhall/beads)

### Non-goals

LazyBeads v1 must not:

- Implement a Beads-compatible database.
- Write directly to `.beads/`, `.beads/embeddeddolt/`, `.beads/dolt/`, or `issues.jsonl`.
- Issue direct SQL/Dolt mutations.
- Become a Git client, project-management SaaS, remote server, or collaboration backend.
- Require a graphical desktop environment.
- Require a network connection for local operations.
- Replace `bd` documentation or expose every upstream command on day one.
- Perform automatic claims, closes, syncs, or dependency edits without an explicit user action.
- Rank tasks through hidden “AI magic” that users cannot inspect or configure.

`.beads/issues.jsonl` is explicitly an export/interchange view rather than the database source of truth or a backup; embedded and server modes are managed by Beads/Dolt. [github](https://github.com/gastownhall/beads)

***

## 2. Design principles

### Beads remains authoritative

Every read comes from `bd` output; every write is performed through `bd`. LazyBeads may cache read data transiently but must never invent an alternate persistent issue state.

```text
LazyBeads UI/commands → typed Go services → argv-based bd adapter → Beads
```

### CLI is the product; TUI is a client

Every essential TUI action must correspond to a documented noninteractive `lb` command. The TUI cannot contain unique business logic.

For example:

```text
TUI: press c on selected issue
             ↓
Application: ClaimIssue(issueID)
             ↓
CLI equivalent: lb claim <issue-id>
             ↓
Beads call: bd update <issue-id> --claim --json
```

### Explain every recommendation

`lb next` must print the reasoning behind its ranking. If the user cannot understand why a bead was recommended, the ranking is a defect.

### Progressive disclosure

Default output should be compact and operational. Rich descriptions, dependency paths, event history, JSON structures, and raw Beads output should be available through explicit commands and flags.

### Human-safe by default

- Read operations never prompt.
- Mutations prompt when attached to an interactive terminal.
- Automation requires an explicit `--yes`.
- Destructive or shared-state-sensitive operations show precise targets and changes.
- Errors preserve upstream diagnostic details while adding actionable LazyBeads context.

### Local and remote terminal parity

Everything must work in:

- macOS Terminal, iTerm2, Kitty, Alacritty, WezTerm, and Ghostty.
- Linux terminals.
- SSH sessions.
- tmux and zellij.
- Devcontainers and CI.
- Windows Terminal where Beads is supported.

Beads supports macOS, Linux, Windows, and FreeBSD, and documents local/embedded as well as multi-writer server mode; LazyBeads should avoid assumptions about any one operating system or database mode. [github](https://github.com/gastownhall/beads)

***

## 3. User workflows

### Morning or session-start triage

```sh
cd ~/code/lazybeads
lb status
lb next
lb claim bd-a3f8.2
```

Expected outcome:

- Verify the workspace and `bd` integration.
- See concise health and workload metrics.
- Receive one recommendation with an explanation.
- Claim atomically through Beads.

### Resume work

```sh
lb focus
lb show bd-a3f8.2
```

Expected outcome:

- See work claimed by the current actor.
- See recently touched and stale work.
- Resume through a detailed issue view.

### Understand blocked work

```sh
lb why bd-a3f8.4
lb graph bd-a3f8.4
```

Expected outcome:

- Identify immediate blockers.
- Traverse transitive blockers.
- Find the nearest actionable root cause.
- View a human-readable tree without needing a graph canvas.

### Triage newly discovered work

```sh
lb create "Handle unsupported bd schema version" \
  --type bug \
  --priority 1 \
  --label compatibility \
  --description "Provide an explicit LazyBeads error and recovery guidance."

lb dep add bd-new bd-a3f8.1
```

Expected outcome:

- Create a Beads-backed task.
- Associate it with relevant work.
- Confirm mutations before execution.

### Agent-supervision pass

```sh
lb focus --all-actors
lb stale --since 4h
lb activity --since today
lb blocked --priority-max 1
```

Expected outcome:

- Identify agents or people holding work.
- Surface stale ownership.
- Find high-priority work still blocked.
- Understand changes since the user was last present.

### Script or agent usage

```sh
lb next --json | jq -r '.issue.id'
lb status --json | jq '.health.status'
lb why bd-a3f8.4 --json
lb claim bd-a3f8.2 --yes --json
```

Every substantive noninteractive command must support stable JSON output. This aligns with Beads’ documented pattern of JSON output across commands for agents and scripts. [steveyegge-beads-62.mintlify](https://steveyegge-beads-62.mintlify.app/cli/overview)

***

# 4. Functional requirements

## Workspace discovery

### Resolution order

LazyBeads resolves a Beads workspace in this order:

1. `--beads-dir <path>`
2. `BEADS_DIR`
3. `--project <path>`
4. Current working directory, walking upward according to the behavior supported by `bd`.
5. Cross-rig selection through `--rig <name>`.
6. Configured default workspace, if explicitly set by the user.

LazyBeads must not independently guess at Beads storage internals once `bd` can resolve them.

### Discovery command

```sh
lb status
lb status --project ~/code/example
lb status --beads-dir ~/code/example/.beads
lb status --rig company-platform
```

### Required discovery errors

| Condition | Exit | User-facing message |
|---|---:|---|
| `bd` absent from PATH | 3 | “Beads CLI (`bd`) was not found. Install it, or set `LB_BD_BIN`.” |
| `bd` cannot execute | 3 | “Found `bd`, but it could not execute.” |
| Not in a Beads workspace | 4 | “No Beads workspace was found from this directory.” |
| Workspace is unreadable | 4 | “Beads workspace exists but cannot be opened.” |
| Schema version mismatch | 5 | “Your `bd` binary cannot safely open this Beads database.” |
| Unsupported upstream capability | 6 | “This LazyBeads command requires a newer compatible Beads capability.” |
| Upstream mutation conflict | 7 | “Beads rejected the requested change; refresh and retry.” |

LazyBeads must include the raw `bd` stderr beneath a concise diagnosis when `--verbose` is used.

### Workspace metadata

LazyBeads should derive and expose:

```go
type Workspace struct {
    RootPath        string
    BeadsDir        string
    Rig             string
    BDBinary        string
    BDVersion       string
    StorageMode     StorageMode
    GitRoot         *string
    IsGitFree       bool
    IsStealth       bool
    IsRemoteBacked  bool
    LastRefreshedAt time.Time
}
```

`StorageMode` values:

```go
const (
    StorageUnknown  StorageMode = "unknown"
    StorageEmbedded StorageMode = "embedded"
    StorageServer   StorageMode = "server"
)
```

***

## Read commands

### `lb status`

Purpose: Provide a compact workspace-health and work-state overview.

```sh
lb status
lb status --json
lb status --watch 5s
```

Human output:

```text
lazybeads 0.1.0 · Beads 1.x · ~/code/lazybeads

Workspace
  Storage: embedded · sync: unknown
  Actor:   josh
  Updated: 18 seconds ago

Work
  Ready:       8
  In progress: 3
  Blocked:     11
  Open:        22
  Closed:      74

Attention
  ! 2 stale claims older than 4h
  ! 1 P1 task blocked by unclaimed ready work

Run `lb next` for a recommended task.
```

Required fields in JSON:

```json
{
  "schema_version": 1,
  "workspace": {},
  "counts": {
    "ready": 8,
    "open": 22,
    "in_progress": 3,
    "blocked": 11,
    "closed": 74
  },
  "attention": [],
  "health": {
    "status": "warning",
    "checks": []
  },
  "generated_at": "2026-08-27T15:11:00Z"
}
```

### `lb ready`

Purpose: Present unblocked work in a more human-efficient way than raw `bd ready`.

```sh
lb ready
lb ready --priority-max 1
lb ready --label backend
lb ready --parent bd-a3f8
lb ready --sort leverage
lb ready --limit 20
lb ready --json
```

Default sort:

1. Lower numeric priority first: P0 then P1, etc.
2. Higher downstream impact.
3. Higher age.
4. Stable lexical ID tie-breaker.

Human output:

```text
READY · 8 tasks

P1  bd-a3f8.2  Add typed bd adapter                  +3 downstream · 2h
P1  bd-f5c9    Define mutation error taxonomy       +2 downstream · 1d
P2  bd-a3f8.5  Add completion scripts               +0 downstream · 4h
P2  bd-k9ab    Build fixture workspace               +1 downstream · 3d
```

Legend:

- `+N downstream` means this issue has N transitive or direct dependent issues, according to the configured impact model.
- The tool must state whether this is direct or transitive in verbose and JSON output.

### `lb next`

Purpose: Recommend exactly one available issue and explain why.

```sh
lb next
lb next --limit 3
lb next --strategy priority
lb next --strategy leverage
lb next --strategy age
lb next --strategy balanced
lb next --json
```

Default human output:

```text
Recommended next task

P1  bd-a3f8.2  Add typed bd adapter
Status: ready · unclaimed · type: task
Reason: P1 priority; unlocks 3 open tasks; parent epic has active work.
Age: 2h 14m

Next:
  lb claim bd-a3f8.2
  lb show bd-a3f8.2
  lb why bd-a3f8.2
```

No-ready-work output:

```text
No claimable work is currently available.

Open tasks are blocked by 7 unresolved issues.
Run `lb blocked` to inspect them.
```

### `lb show`

Purpose: Render an issue in a readable, action-oriented format.

```sh
lb show <id>
lb show <id> --events
lb show <id> --raw
lb show <id> --json
```

Output must include:

- ID, title, type, priority, status.
- Assignee/claim information.
- Parent/epic context.
- Labels and custom metadata where available.
- Description.
- Immediate blockers.
- Direct dependents.
- Related links and relation type.
- Created and updated timestamps.
- Close reason when closed.
- Recent audit/history entries when requested.
- Suggested next commands contextual to state.

### `lb list`

Purpose: Expose a useful human filter interface while forwarding compatible Beads constraints.

```sh
lb list
lb list --status open
lb list --type bug
lb list --label ui --label performance
lb list --label-any ui --label-any performance
lb list --assignee me
lb list --query "schema"
lb list --created-after 2026-08-01
lb list --updated-before 2026-08-27
lb list --json
```

Rules:

- `--label` uses AND semantics.
- `--label-any` uses OR semantics.
- `--query` applies a local fuzzy/full-text filter only after obtaining candidate data from Beads unless Beads supports a stable native equivalent.
- `--assignee me` resolves the effective actor identity through configuration or environment.
- `--all` disables default safety limits.
- Human output uses a dense tabular layout.
- JSON returns all selected fields without terminal styling.

Beads documents common status, type, assignee, label, parent, metadata, and time filtering conventions. LazyBeads should preserve these semantics rather than creating contradictory flag meanings. [steveyegge-beads-62.mintlify](https://steveyegge-beads-62.mintlify.app/cli/overview)

### `lb focus`

Purpose: Show the work most relevant to the current operator.

```sh
lb focus
lb focus --all-actors
lb focus --actor claude-agent-1
lb focus --recent 24h
lb focus --json
```

Default sections:

```text
MY ACTIVE WORK
P1  bd-a3f8.2  Add typed bd adapter       claimed 26m ago

RECENTLY TOUCHED
P2  bd-a3f8.1  CLI architecture            updated 1h ago

RECENTLY UNBLOCKED
P1  bd-f5c9    Define error taxonomy       became ready 14m ago

NEEDS ATTENTION
P1  bd-z7q2    Add health checks           stale claim: 6h
```

“Current actor” resolution:

1. `--actor`
2. `LB_ACTOR`
3. `actor` in LazyBeads config.
4. Git user name/email only if intentionally enabled.
5. No assumption; show unconfigured state.

### `lb blocked`

Purpose: Identify blocked work and actionable root blockers.

```sh
lb blocked
lb blocked --priority-max 1
lb blocked --group-by blocker
lb blocked --group-by parent
lb blocked --json
```

Default output groups by the closest unresolved blocker:

```text
BLOCKED · 11 tasks

Blocked by bd-a3f8.2 — Add typed bd adapter
  P1 bd-a3f8.3  Implement `lb status`
  P1 bd-a3f8.4  Implement `lb ready`
  P2 bd-a3f8.5  Add shell completion

Blocked by bd-f5c9 — Define mutation error taxonomy
  P1 bd-h2x1    Add confirmation policy
```

### `lb why`

Purpose: Explain why an issue is not actionable, or why it is recommended.

```sh
lb why <id>
lb why <id> --depth 5
lb why <id> --all-paths
lb why <id> --json
```

If ready:

```text
bd-a3f8.2 is ready.

No open blockers were found.
It directly unlocks 3 open tasks.
```

If blocked:

```text
bd-a3f8.4 is blocked.

Immediate blocker
└── bd-a3f8.2 · Add typed bd adapter
    Status: in_progress · claimed by josh · updated 26m ago

Transitive impact
└── Completing bd-a3f8.2 unlocks:
    - bd-a3f8.3
    - bd-a3f8.4
    - bd-a3f8.5

Nearest currently actionable item
└── bd-a3f8.2
```

Required behavior:

- Detect cycles and print a nonfatal warning.
- Prevent unbounded traversal.
- Deduplicate shared nodes in multi-path graphs.
- Distinguish blocked-by, parent-child, discovered-from, relates-to, duplicates, supersedes, and replies-to relationships.
- Only blocking relationship types affect “ready” explanation.
- Include one or more actionable leaf nodes when possible.

Beads exposes dependency management and graph-link concepts including blocking, hierarchy, discovered-from, related, duplicates, supersedes, and replies-to relationships. [github](https://github.com/gastownhall/beads)

### `lb graph`

Purpose: Render a focused terminal-friendly issue graph.

```sh
lb graph <id>
lb graph <id> --direction blockers
lb graph <id> --direction dependents
lb graph <id> --depth 3
lb graph <id> --format tree
lb graph <id> --format dot
lb graph <id> --format json
```

Tree output:

```text
bd-a3f8.2 · Add typed bd adapter [in_progress]
├── blocks
│   ├── bd-a3f8.3 · Implement `lb status` [open]
│   ├── bd-a3f8.4 · Implement `lb ready` [open]
│   └── bd-a3f8.5 · Add shell completion [open]
└── parent
    └── bd-a3f8 · LazyBeads CLI [open]
```

DOT output is intended for piping to Graphviz:

```sh
lb graph bd-a3f8 --format dot | dot -Tsvg > lazybeads.svg
```

### `lb activity`

Purpose: Provide an event-oriented operational feed.

```sh
lb activity
lb activity --since 4h
lb activity --since today
lb activity --issue bd-a3f8.2
lb activity --actor claude-agent-1
lb activity --type claim
lb activity --json
```

Events should include, when exposed by Beads:

- Created.
- Updated.
- Claimed/unclaimed.
- Status changes.
- Dependency changes.
- Closed/reopened.
- Messages.
- Memory creation/retirement.
- Sync or workspace events, if observable.

### `lb search`

Purpose: Fast fuzzy discovery across currently available issue fields.

```sh
lb search adapter
lb search "schema mismatch"
lb search bd-a3f8
lb search --all "Dolt"
lb search --json "claim"
```

Rules:

- Search ID, title, description, labels, assignee, and parent context where retrieved.
- Default result count: 20.
- Search must be case-insensitive.
- Exact ID matches always rank first.
- Title prefix matches rank above description-only matches.
- If query is empty, exit 2 with usage guidance.

### `lb stale`

Purpose: Surface likely abandoned or insufficiently progressing work.

```sh
lb stale
lb stale --since 4h
lb stale --priority-max 1
lb stale --claimed-only
lb stale --json
```

Initial stale heuristic:

- `in_progress` issue.
- Has an assignee.
- Updated before `now - stale_after`.
- Not closed.
- Not explicitly deferred, where available.

Default `stale_after`: 8 hours. Configurable per workspace.

### `lb doctor`

Purpose: Diagnose tool, workspace, compatibility, storage, and optional sync health.

```sh
lb doctor
lb doctor --json
lb doctor --fix
lb doctor --verbose
```

Checks:

| Check | Severity if failed | Behavior |
|---|---|---|
| `bd` exists and executes | Error | Show path/install guidance |
| `bd version` readable | Error | Show raw error |
| Beads workspace discoverable | Error | Show root and resolution hints |
| Schema compatibility | Error | Surface upstream-safe recovery guidance |
| Required JSON parsing works | Error | Report unsupported output shape |
| LazyBeads config valid | Error | Identify file and location |
| Current actor configured | Warning | Explain how to set it |
| Pending stale claims | Warning | List task IDs |
| Dependency cycles | Warning | List graph path |
| Potential parent/status inconsistencies | Warning | Explain, do not mutate |
| Remote/sync visibility available | Info/Warning | Never invent “clean” without data |
| Upstream version outside tested range | Warning | Continue with capability gating |

`lb doctor --fix` may only create local LazyBeads configuration, install optional shell completion, or initialize a local cache directory. It must never upgrade/migrate Beads, run `bd migrate`, push/pull data, or alter issue state without a separately confirmed command.

Beads has an explicit schema-version guard and defined upgrade/migration workflow; LazyBeads must surface it rather than bypass it by default. [github](https://github.com/gastownhall/beads)

***

## Mutation commands

All mutation commands share:

```text
--yes            Skip interactive confirmation
--dry-run        Render intended bd argv without executing
--json           Emit a structured result
--project <path>
--beads-dir <path>
--rig <name>
--timeout <dur>
```

TTY policy:

- If standard input is a terminal and `--yes` is absent: prompt.
- If stdin is noninteractive and `--yes` is absent: refuse with exit code 2.
- `--dry-run` never prompts and never executes `bd`.
- `--json` with a mutation still requires `--yes` in noninteractive mode.
- All writes receive a per-workspace mutation lock.

### `lb create`

```sh
lb create "Add command adapter"
lb create "Fix JSON decoding" --type bug --priority 1
lb create "Plan TUI architecture" --type task --parent bd-a3f8
lb create "Record agent workflow rule" --description-file notes.md
lb create "..." --label cli --label mvp --metadata team=core
```

Required inputs:

- Title.
- Optional description or `--description-file`.
- Type.
- Priority.
- Labels.
- Assignee.
- Parent.
- Dependencies/relations.
- Due/defer dates where upstream supports them.
- Custom metadata fields where upstream supports them.
- Rig.

Title validation:

- Nonempty after trimming.
- Maximum size defined by upstream or 512 Unicode code points, whichever is lower.
- Refuse NUL bytes.
- Preserve Unicode exactly.

### `lb edit`

```sh
lb edit <id>
lb edit <id> --title "New title"
lb edit <id> --description "New description"
lb edit <id> --description-file ./issue.md
lb edit <id> --priority 1 --label add:cli --label remove:legacy
lb edit <id> --open-in-editor
```

`--open-in-editor` behavior:

1. Fetch issue.
2. Create a secure temporary editable representation.
3. Invoke `$VISUAL`, then `$EDITOR`.
4. Parse/validate changed fields.
5. Show semantic diff.
6. Ask confirmation.
7. Execute only valid supported updates.
8. Re-fetch and display confirmed state.

The editable format should be human-friendly YAML frontmatter plus Markdown:

```markdown
---
id: bd-a3f8.2
title: Add typed Beads adapter
type: task
priority: 1
status: open
assignee:
labels:
  - cli
  - mvp
---

Build a Go adapter that invokes `bd` through argv and decodes JSON output.
```

### `lb claim`

```sh
lb claim <id>
lb claim <id> --actor claude-agent-1
lb claim <id> --yes
```

Behavior:

- Fetch current issue state first.
- Refuse to claim closed work.
- Warn if already claimed by a different actor.
- Use canonical Beads atomic claim semantics rather than manually separately setting assignee and status.
- Re-read issue after command completion.
- Show successful confirmed claimant/status.

Beads documents `bd update <id> --claim` as atomic: it sets ownership and moves work into progress. [github](https://github.com/gastownhall/beads)

### `lb unclaim`

```sh
lb unclaim <id>
lb unclaim <id> --yes
```

Behavior:

- Use a supported Beads operation only.
- Do not guess at how Beads represents unclaiming.
- If upstream has no compatible stable capability, report that and offer the exact upstream command or required version—not a direct storage workaround.

### `lb close`

```sh
lb close <id> --reason "Implemented typed JSON adapter"
lb close <id> "Implemented typed JSON adapter"
lb close <id> --yes --json
```

Rules:

- Close reason required by LazyBeads even if upstream permits omission.
- Render affected dependents after successful close.
- After close, run a bounded ready refresh to tell the user what became available:

```text
Closed bd-a3f8.2.

Newly ready:
  P1 bd-a3f8.3 · Implement `lb status`
  P1 bd-a3f8.4 · Implement `lb ready`
  P2 bd-a3f8.5 · Add shell completion
```

The standard Beads workflow is ready → atomic claim → work → close, after which released blockers can create new ready work. [github](https://github.com/gastownhall/beads)

### `lb reopen`

```sh
lb reopen <id>
lb reopen <id> --reason "Regression discovered during integration"
```

Reopen must require a reason and refresh relevant graph state after confirmation.

### `lb dep`

Subcommands:

```sh
lb dep add <child> <parent>
lb dep add <child> <parent> --type blocks
lb dep add <child> <parent> --type parent-child
lb dep add <child> <parent> --type relates-to
lb dep remove <child> <parent>
lb dep list <id>
lb dep validate
```

Critical semantics:

- Arguments are never silently reordered.
- Confirmation must spell out direction in plain language:

```text
Make bd-a3f8.4 blocked by bd-a3f8.2?
```

- Before addition, detect direct self-reference and known cycle risk.
- LazyBeads may warn about a potential cycle but must defer final validity to Beads.
- `--type` defaults must be explicit in command output; no hidden assumption.
- `dep validate` is read-only and reports cycles, missing references, and suspicious relationship shapes where possible.

### `lb assign`

```sh
lb assign <id> <actor>
lb assign <id> --clear
```

This is distinct from claim:

- Assigning delegates or records responsibility.
- Claiming indicates active atomic work ownership.
- LazyBeads must avoid treating the two as interchangeable unless Beads does.

### `lb message`

Optional v1.1 command, contingent on stable upstream capability:

```sh
lb message send <recipient-or-issue> "Need review on adapter behavior"
lb message inbox
lb message thread <id>
```

Beads describes a message issue type with threading and mail delegation. The first LazyBeads release may only surface these as activity/detail content; send support should wait until the mapping is verified against the upstream CLI reference. [github](https://github.com/gastownhall/beads)

### `lb memory`

```sh
lb memory list
lb memory add "Use argv-based execution; never shell strings."
lb memory search "claims"
lb memory retire <id>
lb memory prime
```

Rules:

- `lb memory add` delegates to `bd remember`.
- `lb memory prime` renders or forwards the workflow context from `bd prime`.
- Treat memories as project-scoped durable operational context, not a second notes database.

Beads identifies `bd prime` and `bd remember` as its workflow-context and persistent-memory mechanisms. [github](https://github.com/gastownhall/beads)

### `lb sync`

Deferred from v1.0 writes, but read-only inspection may ship:

```sh
lb sync status
lb sync pull
lb sync push
```

Mutating sync actions must:

- Show the exact `bd dolt pull` or `bd dolt push` operation.
- Require confirmation.
- Be disabled in “safe mode.”
- Surface upstream output faithfully.
- Avoid claiming Git-like semantics if Beads/Dolt returns a different result.

Beads documents cross-machine synchronization through `bd dolt push` and `bd dolt pull`, using Dolt replication and a data reference on the remote. [github](https://github.com/gastownhall/beads)

***

# 5. Recommendation engine

## Purpose

The recommendation engine powers `lb next`, ready list ordering, dashboard ranking, and “nearest actionable blocker” selection.

It does not make autonomous changes. It ranks only tasks that Beads identifies as ready, unless the command explicitly asks for a blocked-work recommendation.

## Strategies

```text
priority   Strict priority then age
leverage   Dependency impact then priority
age        Oldest ready task first
balanced   Configurable weighted scoring; default
random     Deterministic daily shuffle for avoiding local maxima
```

Default: `balanced`.

## Eligibility

An issue is eligible for `lb next` only when:

- It is in the Beads ready set.
- It is not closed.
- It does not violate configured exclusion labels.
- It is not deferred, if returned by Beads.
- It is not excluded by current actor policy.
- It is within any requested parent/epic scope.
- It is not explicitly ignored by local user configuration.

## Default score

\[
S(i) =
W_pP(i) +
W_lL(i) +
W_aA(i) +
W_fF(i) +
W_rR(i) -
W_cC(i)
\]

Where:

| Symbol | Meaning | Default behavior |
|---|---|---|
| \(P(i)\) | Priority score | P0 strongly exceeds P1, etc. |
| \(L(i)\) | Leverage score | Count/weight of open work unlocked |
| \(A(i)\) | Age score | Increases gradually over time |
| \(F(i)\) | Focus score | Boosts selected parent, labels, or active epic |
| \(R(i)\) | Recency release score | Boosts work newly unblocked |
| \(C(i)\) | Cost/complexity penalty | Disabled unless explicit metadata exists |

Initial defaults:

```toml
[ranking]
strategy = "balanced"
priority_weight = 100
leverage_weight = 15
age_weight = 1
focus_weight = 25
recently_unblocked_weight = 10
complexity_penalty_weight = 0
```

### Priority normalization

```text
P0 = 5
P1 = 4
P2 = 3
P3 = 2
P4 = 1
Unknown = 0
```

### Leverage calculation

Initial version:

```text
direct_open_dependents × 1
+ transitive_open_dependents × 0.25
+ dependent_priority_weight
```

Requirements:

- Cap traversal depth and total visited nodes.
- Record whether an impact count is direct, transitive, or approximated.
- Do not rank relations such as `relates-to` as blocked/unlocked work.
- Make actual factor contributions visible in JSON and human verbose output.

### Explanation contract

Every recommendation contains:

```json
{
  "strategy": "balanced",
  "score": 468.4,
  "factors": [
    {
      "kind": "priority",
      "value": 4,
      "weight": 100,
      "contribution": 400,
      "explanation": "P1 priority"
    },
    {
      "kind": "leverage",
      "value": 3,
      "weight": 15,
      "contribution": 45,
      "explanation": "Directly unlocks 3 open issues"
    }
  ],
  "excluded": []
}
```

No factor may influence a recommendation without appearing in this explanation structure.

***

# 6. TUI specification

## Invocation

```sh
lb
lb tui
lb tui --project ~/code/lazybeads
lb tui --view ready
lb tui --issue bd-a3f8.2
```

`lb` with no arguments launches the TUI only if stdout and stdin are interactive terminals. Otherwise it prints concise help and exits 2.

## Implementation

- Go.
- Bubble Tea model-update-view architecture.
- Bubbles for reusable list, viewport, text input, paginator, spinner, help, and table primitives where suitable.
- Lip Gloss for terminal styling.
- No web runtime, embedded browser, Node dependency, or desktop shell.
- Minimum target terminal dimensions: 80×24.
- Degraded one-pane mode below 100 columns.
- Readable monochrome mode.
- Full functionality without mouse support.

## Layout

Default wide layout:

```text
┌─ lb · project-name ─ ready 8 · active 3 · blocked 11 · health ! ─────────────┐
│ [r] Ready  [f] Focus  [b] Blocked  [a] Activity  [m] Memory  [:] Commands   │
├──────────────────────────────┬───────────────────────────────────────────────┤
│ Ready · sort: balanced       │ bd-a3f8.2 · Add typed Beads adapter           │
│                              │                                               │
│ > P1  bd-a3f8.2  +3  2h      │ open · unclaimed · task · cli,mvp             │
│   P1  bd-f5c9    +2  1d      │                                               │
│   P2  bd-a3f8.5  +0  4h      │ Why this matters                              │
│                              │ P1; unlocks 3 tasks; part of focused epic    │
│ / filter                     │                                               │
│                              │ Description                                   │
│                              │ Build a typed `bd --json` command adapter... │
│                              │                                               │
│                              │ Blockers: none · Dependents: 3                │
├──────────────────────────────┴───────────────────────────────────────────────┤
│ enter inspect · c claim · e edit · d deps · / filter · : command · ? help   │
└───────────────────────────────────────────────────────────────────────────────┘
```

## Views

| View | Key | Purpose |
|---|---|---|
| Ready | `r` | Ranked claimable work |
| Focus | `f` | Current actor’s active/recent work |
| Blocked | `b` | Work grouped by actionable blocker |
| All issues | `i` | Filtered issue browser |
| Issue detail | `enter` | Full task inspector |
| Graph | `g` | Dependency tree for selected issue |
| Activity | `a` | Recent events |
| Memory | `m` | Project memory list/prime |
| Health | `h` | `lb status` / `lb doctor` details |
| Command palette | `:` | Fuzzy command runner |
| Help | `?` | Context-sensitive keybinding reference |

## Interaction model

### Navigation

- `j` / `k`: Move down/up.
- `ctrl-d` / `ctrl-u`: Half-page down/up.
- `g` / `G`: First/last list item when not in graph mode.
- `h` / `l`: Collapse/expand or previous/next panel depending on view.
- `tab` / `shift-tab`: Cycle focus zones.
- `enter`: Open issue detail or execute focused primary action.
- `esc`: Back, cancel modal, clear transient filtering.
- `q`: Quit only at top-level; otherwise go back.

### Core actions

- `c`: Claim selected issue.
- `u`: Unclaim selected issue where supported.
- `e`: Edit selected issue.
- `x`: Close selected issue.
- `o`: Open selected issue in `$EDITOR`.
- `d`: Dependency menu.
- `n`: Create issue.
- `R`: Refresh all relevant data.
- `y`: Copy issue ID; `Y` copy a command-ready reference.
- `/`: Filter current view.
- `:`: Command palette.

### Selection behavior

- One focused issue at a time.
- `space` toggles multi-select in lists that permit safe batch actions.
- v1 batch actions limited to non-destructive export/copy/filter operations.
- Batch claims, closes, and dependency mutations are deferred until confirmation UX and upstream behavior are fully specified.

### Confirmation modal

For all mutations:

```text
Claim bd-a3f8.2?

This will ask Beads to atomically claim:
  Issue:  bd-a3f8.2 — Add typed Beads adapter
  Actor:  josh
  Change: open → in_progress; assignee → josh

[y] Confirm  [n/esc] Cancel  [v] View raw command
```

The modal must never hide the issue ID, title, action, actor, or state transition.

## Keybinding configuration

Default bindings must be opinionated but configurable. Because the user uses Colemak-DH, support logical and physical key notation from the beginning.

Example config:

```toml
[keys]
layout = "logical"

[keys.global]
quit = ["q"]
command_palette = [":"]
help = ["?"]
refresh = ["R"]

[keys.list]
down = ["j", "down"]
up = ["k", "up"]
open = ["enter"]
claim = ["c"]
close = ["x"]
filter = ["/"]
```

Requirements:

- Detect collisions and report them at startup/doctor.
- Support multi-key chords in a later release; v1 supports single keys and modified keys.
- Display the currently active binding in help/footer.
- Never hardcode Vim navigation as the only method; arrows always work.

## Rendering and accessibility

- No color alone may communicate status.
- Use symbols plus text:
  - `● ready`
  - `! warning`
  - `× error`
  - `✓ closed`
- Honor `NO_COLOR`.
- Provide `--color auto|always|never`.
- Avoid animations by default; spinners only for active requests.
- Honor `--reduced-motion`.
- Truncate safely by terminal width.
- Ensure all internal string display uses Unicode-width-aware calculation.
- Use an ASCII fallback:

```sh
lb tui --ascii
```

***

# 7. CLI output contracts

## Global output modes

```sh
--format human
--format json
--format jsonl
--format yaml
--no-color
--quiet
--verbose
```

v1 required:

- `human`
- `json`
- `jsonl` for list-like/event-like commands

YAML may be introduced later.

## JSON envelope

Every JSON command returns:

```json
{
  "schema_version": 1,
  "command": "next",
  "workspace": {
    "root_path": "/Users/user/code/lazybeads",
    "rig": null
  },
  "data": {},
  "warnings": [],
  "generated_at": "2026-08-27T15:11:00Z"
}
```

Rules:

- `schema_version` is mandatory.
- Adding fields is backward-compatible.
- Renaming/removing a field requires a schema version increment.
- Human-readable warnings also appear structurally.
- Never include ANSI escape codes in JSON.
- Errors use a stable envelope to stdout only when `--json` is specified:

```json
{
  "schema_version": 1,
  "error": {
    "code": "bd_not_found",
    "message": "Beads CLI (`bd`) was not found.",
    "hint": "Install Beads or set LB_BD_BIN.",
    "upstream_stderr": null
  }
}
```

## Exit codes

| Code | Meaning |
|---:|---|
| 0 | Success |
| 1 | General runtime failure |
| 2 | Invalid CLI usage or unsafe noninteractive mutation |
| 3 | `bd` installation/execution failure |
| 4 | Workspace discovery/open failure |
| 5 | Schema/compatibility safety failure |
| 6 | Capability unavailable in installed Beads |
| 7 | Mutation rejected/conflicted |
| 8 | User declined confirmation |
| 9 | Timeout/cancellation |
| 10 | Internal LazyBeads invariant failure |

Beads itself documents 0 for success, 1 for errors/no results, and 2 for invalid arguments; LazyBeads may use its richer codes but must retain useful upstream context. [steveyegge-beads-62.mintlify](https://steveyegge-beads-62.mintlify.app/cli/overview)

***

# 8. Configuration

## File locations

Follow XDG on Linux and conventional platform paths elsewhere:

```text
Linux:   ~/.config/lazybeads/config.toml
macOS:   ~/Library/Application Support/lazybeads/config.toml
Windows: %AppData%\lazybeads\config.toml

Project: .lazybeads.toml
```

Precedence:

1. Command flags.
2. Environment variables.
3. Project configuration.
4. User configuration.
5. Built-in defaults.

## Example configuration

```toml
[general]
bd_binary = "bd"
actor = "josh"
color = "auto"
default_view = "ready"
confirm_mutations = true
stale_after = "8h"
refresh_interval = "30s"

[workspace]
focus_parent = "bd-a3f8"
exclude_labels = ["wontfix", "parking-lot"]

[ranking]
strategy = "balanced"
priority_weight = 100
leverage_weight = 15
age_weight = 1
focus_weight = 25
recently_unblocked_weight = 10
max_graph_depth = 8
max_graph_nodes = 500

[tui]
mouse = false
ascii = false
compact = false
show_descriptions = true

[aliases]
now = "next"
mine = "focus"
```

## Environment variables

```text
LB_BD_BIN
LB_CONFIG
LB_PROJECT
LB_BEADS_DIR
LB_RIG
LB_ACTOR
LB_COLOR
LB_NO_CONFIRM
LB_TIMEOUT
LB_LOG_LEVEL
```

Safety rule: `LB_NO_CONFIRM=1` must be explicitly documented and should emit a warning in interactive use. It must not be silently enabled by project configuration.

***

# 9. Beads adapter

## Adapter interface

```go
type Client interface {
    Version(ctx context.Context, opts Scope) (VersionInfo, error)
    Info(ctx context.Context, opts Scope) (Info, error)

    Ready(ctx context.Context, q ReadyQuery) ([]Issue, error)
    List(ctx context.Context, q ListQuery) ([]Issue, error)
    Show(ctx context.Context, id string, opts Scope) (IssueDetail, error)

    Create(ctx context.Context, input CreateIssueInput) (Issue, error)
    Update(ctx context.Context, id string, input UpdateIssueInput) (Issue, error)
    Claim(ctx context.Context, id string, input ClaimInput) (Issue, error)
    Close(ctx context.Context, id string, input CloseInput) (Issue, error)
    Reopen(ctx context.Context, id string, input ReopenInput) (Issue, error)

    AddDependency(ctx context.Context, input DependencyInput) error
    RemoveDependency(ctx context.Context, input DependencyInput) error
    ListDependencies(ctx context.Context, id string, opts Scope) ([]Dependency, error)

    Prime(ctx context.Context, opts Scope) (PrimeResult, error)
    Remember(ctx context.Context, input RememberInput) (Memory, error)

    Activity(ctx context.Context, q ActivityQuery) ([]Event, error)
    SyncStatus(ctx context.Context, opts Scope) (SyncStatus, error)
}
```

## Process execution rules

- Execute `bd` with `exec.CommandContext`.
- Use explicit argv only.
- Do not invoke `/bin/sh -c`, PowerShell command strings, or equivalent shell interpolation.
- Set working directory only after resolving the target workspace.
- Set `BEADS_DIR` only when explicitly selected through command/config/environment resolution.
- Capture stdout and stderr separately.
- Use a default timeout of 30 seconds.
- Permit `--timeout`.
- Limit captured output size to avoid runaway memory use.
- Preserve stdout bytes for JSON parsing.
- Parse stderr as diagnostic text; never assume it is JSON.
- Include an execution trace only with `--debug`, redacting sensitive environment values.

## Capability detection

At startup and once per workspace cache window:

1. Run a safe version/info command.
2. Determine installed version when possible.
3. Verify a minimal JSON read command.
4. Record supported optional capabilities.
5. Avoid version-string-only gating where feature probing is practical.

Example:

```go
type Capabilities struct {
    JSONOutput            bool
    AtomicClaim           bool
    DependencyRelations   bool
    EventHistory          bool
    MemoryCommands        bool
    CrossRig              bool
    SyncInspection        bool
    Reopen                bool
    Unclaim               bool
    CustomMetadata        bool
}
```

If a capability is absent:

- Hide it from default TUI actions.
- Keep it discoverable in help with an “Unavailable in current Beads” note.
- Return exit 6 in CLI use.
- Never substitute direct database access.

## Error mapping

```go
type ErrorKind string

const (
    ErrBDBinaryMissing     ErrorKind = "bd_not_found"
    ErrBDExecution         ErrorKind = "bd_execution_failed"
    ErrWorkspaceNotFound   ErrorKind = "workspace_not_found"
    ErrSchemaMismatch      ErrorKind = "schema_mismatch"
    ErrUnsupported         ErrorKind = "unsupported_capability"
    ErrNotFound            ErrorKind = "issue_not_found"
    ErrConflict            ErrorKind = "mutation_conflict"
    ErrValidation          ErrorKind = "validation_error"
    ErrTimeout             ErrorKind = "timeout"
    ErrCancelled           ErrorKind = "cancelled"
    ErrDecode              ErrorKind = "invalid_bd_json"
)
```

A mapped error must retain:

```go
type CommandError struct {
    Kind       ErrorKind
    Operation  string
    Args       []string
    ExitCode   int
    Stdout     string
    Stderr     string
    Cause      error
    Hint       string
}
```

Raw args must redact values marked as secrets. No secrets are expected from core `bd` commands, but the abstraction must be safe for future remote credentials/configuration.

***

# 10. Data model

## Core issue model

```go
type Issue struct {
    ID          string
    Title       string
    Description string

    Type        IssueType
    Status      IssueStatus
    Priority    Priority

    Assignee    *Actor
    Labels      []string
    Metadata    map[string]any

    ParentID    *string
    CreatedAt   *time.Time
    UpdatedAt   *time.Time
    DueAt       *time.Time
    DeferredAt  *time.Time
    ClosedAt    *time.Time
    CloseReason *string

    Raw         json.RawMessage
}
```

## Status and type handling

Never assume the upstream status/type set is permanently closed.

```go
type IssueStatus string
type IssueType string
type RelationType string
```

Known values should be displayed intelligently, but unknown values must be preserved and rendered as-is.

Known upstream types include bug, feature, task, epic, chore, and decision. [steveyegge-beads-62.mintlify](https://steveyegge-beads-62.mintlify.app/cli/overview)

## Graph model

```go
type GraphNode struct {
    Issue
    IsReady          bool
    IsBlocked        bool
    IsActionable     bool
    DirectDependents int
    TransitiveImpact int
}

type GraphEdge struct {
    FromID string
    ToID   string
    Type   RelationType
    Blocks bool
}

type IssueGraph struct {
    RootID      string
    Nodes       map[string]GraphNode
    Edges       []GraphEdge
    Cycles      [][]string
    Truncated   bool
    Truncation  *GraphTruncation
}
```

## Event model

```go
type Event struct {
    ID        string
    IssueID   *string
    Actor     *Actor
    Kind      EventKind
    Timestamp time.Time
    Summary   string
    Before    map[string]any
    After     map[string]any
    Raw       json.RawMessage
}
```

Unknown event kinds must remain visible rather than discarded.

***

# 11. Health system

## Health levels

```text
ok       No known blocking condition
info     Informational state
warning  Work is possible, but attention is useful
error    Safe operation is impaired or impossible
unknown  Insufficient evidence to assess
```

## Checks

### Tooling

- `bd` found.
- `bd` runs.
- Version/info readable.
- JSON output decodes.

### Workspace

- Workspace resolves.
- Storage mode visible if supported.
- Schema is compatible.
- Current path and selected rig are unambiguous.

### Work graph

- Ready count.
- Open/blocked counts.
- Dependency cycles.
- Closed issue with active children, if data supports detection.
- Self-dependencies.
- High-priority blocked work.
- Stale claims.

### Operator setup

- Actor identity configured.
- Configuration parses.
- Keybindings have no collisions.
- Optional editor exists for `lb edit --open-in-editor`.

### Sync

Only report sync conditions that are actually obtained from Beads. If unavailable:

```text
Sync: unknown (LazyBeads could not determine remote status)
```

Never output “clean” based merely on lack of error.

***

# 12. Security and safety

## Threat model

LazyBeads processes:

- Issue titles and descriptions controlled by collaborators/agents.
- Labels, metadata, IDs, event text, and memory content.
- Subprocess output from `bd`.
- Workspace file paths.
- User configuration.
- `$EDITOR` / `$VISUAL` paths.

Potential risks:

- Shell injection.
- Terminal escape sequence injection.
- Unsafe path traversal.
- Confused-deputy mutations targeting the wrong workspace.
- State changes caused by stale TUI data.
- Maliciously large output or pathological dependency graphs.
- Accidental sync/migration operations.
- Credential exposure in debug logs.

## Required protections

### No shell execution

All `bd` calls use argv vectors.

### Terminal sanitization

Before rendering data from issues/events/memory:

- Strip or escape control characters except permitted whitespace.
- Neutralize OSC sequences, especially clipboard and hyperlink sequences.
- Limit line lengths in list displays.
- Render raw content only through a safe pager/viewer mode.

### Workspace target visibility

Before every mutation, show:

```text
Workspace: /absolute/path
Rig:       <name or local>
Issue:     ID + title
```

### Freshness protection

For mutations:

1. Read current issue.
2. Show current state.
3. Prompt.
4. Run `bd`.
5. Re-fetch confirmed state.

If the issue materially changed between inspection and mutation, surface the returned conflict/state result rather than pretending success.

### Mutation serialization

Maintain an in-process lock keyed by canonical workspace identity:

```go
type WorkspaceLockManager interface {
    WithWriteLock(ctx context.Context, workspaceID string, fn func(context.Context) error) error
}
```

This does not replace Beads’ transactional/atomic protections; it prevents LazyBeads itself from issuing avoidable concurrent writes.

### Bounds

Default limits:

```text
Command timeout:        30 seconds
Maximum stdout capture: 10 MiB
Maximum stderr capture: 2 MiB
Graph traversal depth:  8
Graph node limit:       500
Search result limit:    1000 before local ranking
TUI event queue:        bounded, with refresh coalescing
```

### Sensitive information

- Never log complete environment variables.
- Redact configured credential-like values.
- Store no Beads database copies.
- Store only optional cached read data, never mutation queues, unless an explicit offline mode is designed in a later release.

***

# 13. Performance requirements

## Startup

On a normal initialized local workspace:

```text
`lb --help`: under 100 ms perceived startup
`lb status`: under 1 second target
`lb ready`: under 1 second target for ≤ 1,000 issues
TUI first paint: under 1.5 seconds target
```

Performance is measured excluding unusually slow external `bd`/Dolt behavior, but LazyBeads must expose where time was spent in debug mode.

## Responsiveness

- TUI keypress-to-render target: under 50 ms for local navigation.
- Filtering existing in-memory list: under 100 ms for 10,000 rows.
- Background refresh never blocks navigation.
- A mutation displays a progress state after 150 ms.
- Cancellable commands respond to Ctrl-C promptly.

## Caching

Permitted cache:

- In-memory only by default.
- Short TTL for workspace info and issue query results.
- Invalidate after every successful mutation.
- TUI refresh interval configurable; default 30 seconds.
- File watcher may trigger refresh hints, but it must not parse or write Beads internals.

Optional disk cache, if added later:

- Store in XDG cache.
- Clearable through `lb cache clear`.
- Never treated as authoritative.
- Never store secrets or unredacted error traces by default.

***

# 14. Repository architecture

```text
lazybeads/
├── cmd/
│   └── lb/
│       └── main.go
├── internal/
│   ├── app/
│   │   ├── service.go
│   │   ├── queries.go
│   │   ├── mutations.go
│   │   ├── recommendation.go
│   │   └── health.go
│   ├── beads/
│   │   ├── client.go
│   │   ├── command.go
│   │   ├── capabilities.go
│   │   ├── decode.go
│   │   ├── errors.go
│   │   └── fixtures/
│   ├── config/
│   │   ├── config.go
│   │   ├── load.go
│   │   └── validation.go
│   ├── domain/
│   │   ├── issue.go
│   │   ├── graph.go
│   │   ├── event.go
│   │   ├── memory.go
│   │   └── output.go
│   ├── workspace/
│   │   ├── resolver.go
│   │   ├── locks.go
│   │   ├── environment.go
│   │   └── watcher.go
│   ├── output/
│   │   ├── human.go
│   │   ├── json.go
│   │   ├── jsonl.go
│   │   ├── table.go
│   │   └── sanitize.go
│   ├── cli/
│   │   ├── root.go
│   │   ├── status.go
│   │   ├── ready.go
│   │   ├── next.go
│   │   ├── why.go
│   │   ├── show.go
│   │   ├── list.go
│   │   ├── focus.go
│   │   ├── blocked.go
│   │   ├── graph.go
│   │   ├── activity.go
│   │   ├── mutations.go
│   │   ├── doctor.go
│   │   └── completion.go
│   └── tui/
│       ├── model.go
│       ├── update.go
│       ├── view.go
│       ├── commands.go
│       ├── keymap.go
│       ├── views/
│       └── components/
├── test/
│   ├── integration/
│   ├── e2e/
│   └── golden/
├── docs/
│   ├── architecture.md
│   ├── compatibility.md
│   ├── configuration.md
│   ├── keybindings.md
│   └── security.md
├── scripts/
│   └── install.sh
├── .goreleaser.yml
├── go.mod
├── README.md
├── LICENSE
└── AGENTS.md
```

## Dependency policy

Prefer a small dependency graph:

- Cobra for CLI parsing.
- Bubble Tea, Bubbles, Lip Gloss for TUI.
- TOML parser.
- A fuzzy matcher.
- Standard library for subprocesses, JSON, paths, locking, context, and time.

Avoid:

- ORMs.
- Embedded databases.
- Web runtimes.
- Direct Dolt libraries.
- A plugin system in v1.
- Reflection-heavy command frameworks beyond what Cobra itself uses.

***

# 15. Testing strategy

## Unit tests

Required coverage:

- Configuration precedence and validation.
- Workspace resolution order.
- CLI argument formation.
- Error mapping.
- JSON decoding against fixture corpus.
- Recommendation scoring.
- Cycle detection.
- Graph traversal limits.
- Terminal sanitization.
- Output JSON schema.
- Confirmation policy.
- Keybinding collision detection.

## Fixture-driven adapter tests

Use captured, versioned `bd --json` fixture outputs:

```text
testdata/
├── bd-vX/
│   ├── ready.json
│   ├── list.json
│   ├── show-open.json
│   ├── show-closed.json
│   ├── schema-mismatch.stderr
│   ├── claim-success.json
│   └── malformed-output.txt
```

Every supported Beads version range needs a compatibility fixture set.

## Integration tests

Run against an ephemeral initialized Beads workspace when available:

1. Initialize workspace.
2. Create graph of issues.
3. Add dependencies.
4. Verify ready list.
5. Claim task.
6. Close task.
7. Verify newly ready dependents.
8. Validate `lb why`.
9. Test concurrent mutation behavior.
10. Test broken/missing/unsupported states.

Do not make CI depend solely on real upstream package network installation. Pin a test fixture or container image/version.

## End-to-end TUI tests

Use a terminal emulator test harness or Bubble Tea model tests to verify:

- First render.
- Navigation.
- Filtering.
- Modal confirmation.
- Successful/failed claim states.
- Resize behavior.
- No-color/ASCII rendering.
- Keybinding overrides.
- Error presentation.
- Focus returns correctly after refresh.

## Golden tests

Use golden files for:

- `lb status`
- `lb ready`
- `lb next`
- `lb why`
- `lb graph`
- `lb doctor`
- JSON envelopes
- Narrow terminal output
- ASCII and color-disabled output

Normalize timestamps and paths in test mode.

## Fuzzing

Fuzz:

- JSON decoder.
- Terminal sanitizer.
- Issue title/description rendering.
- Config parsing.
- Graph traversal with malformed cycles.
- Command argument generation.

***

# 16. Release plan

## Phase 0: foundation

Deliverables:

- Repository setup.
- Cobra root command.
- Config loader.
- Structured logging.
- `bd` binary discovery.
- Workspace resolution.
- Typed process runner.
- Capability probe.
- Fixture test harness.

Acceptance:

```sh
lb version
lb doctor
lb status
```

work against a supported local Beads workspace and fail clearly in unsupported environments.

## Phase 1: read-only operational CLI

Deliverables:

- `status`
- `ready`
- `show`
- `list`
- `search`
- `graph`
- `why`
- `blocked`
- JSON output contracts
- Recommendation engine v1

Acceptance:

```sh
lb next
```

provides one available Beads issue, a transparent reason, and valid JSON with `--json`.

## Phase 2: safe mutations

Deliverables:

- `create`
- `edit`
- `claim`
- `close`
- `reopen`
- dependency add/remove/list
- confirmation system
- dry-run system
- per-workspace mutation locking
- re-fetch/invalidation workflow

Acceptance:

- No mutation ever executes silently.
- Every success is re-read and displayed from confirmed Beads state.
- Noninteractive mutation without `--yes` fails safely.

## Phase 3: terminal UI

Deliverables:

- Ready view.
- Detail inspector.
- Filter.
- Claim/close/edit confirmations.
- Command palette.
- Help.
- Refresh/error states.
- User keymap support.
- Narrow/ASCII/no-color modes.

Acceptance:

A user can complete:

```text
launch → find ready task → inspect → claim → close → see released work
```

without a mouse.

## Phase 4: supervision and quality

Deliverables:

- Activity view.
- Focus/stale workflows.
- Memory view.
- Shell completion.
- Man page.
- Install paths.
- Version compatibility matrix.
- Release automation.
- Improved health diagnostics.

## Phase 5: optional advanced features

Candidates only after everyday use proves demand:

- Read-only sync status and confirmed push/pull actions.
- Formula/molecule visibility.
- Gate status.
- Cross-rig dashboard.
- Configurable saved views.
- Custom ranking plugins through external JSON hooks.
- `lb watch` event-like terminal dashboard.
- Editor integrations for Neovim and VS Code.
- A future graphical UI that consumes the same command/API contract.

***

# 17. MVP acceptance criteria

The first public version is ready when all of the following are true:

1. A user can install one Go binary and run it in a Beads project.
2. `lb doctor` correctly identifies missing `bd`, a missing workspace, and schema incompatibility.
3. `lb ready` presents unblocked work clearly and supports `--json`.
4. `lb next` selects only ready work and explains its ranking.
5. `lb why <id>` correctly explains immediate/transitive blockers on a fixture graph.
6. `lb show <id>` provides enough context to begin work without separately invoking `bd`.
7. `lb claim <id>` uses the Beads atomic claim operation and re-fetches confirmed state.
8. `lb close <id> --reason ...` prompts safely and lists newly ready work after success.
9. `lb dep add` clearly communicates relationship direction and requires confirmation.
10. `lb` launches a keyboard-only TUI with Ready, Detail, Filter, Claim, Close, and Help.
11. All mutation paths use argv-based subprocess execution with no shell interpolation.
12. All issue-derived terminal output is control-sequence safe.
13. Read commands function over SSH with no GUI requirements.
14. Output and error contracts are documented and covered by tests.

## Final positioning

LazyBeads should not compete with Beads. It should make Beads feel like a tool a technical human can live inside all day:

```text
bd = database, graph semantics, durable agent memory, atomic coordination
lb = attention management, explanation, navigation, operational confidence
```

That division is technically clean and product-legible. Beads already provides the key substrate—dependency-aware work, ready-task detection, `--json` machine interfaces, atomic claim behavior, persistent memory, and Dolt-backed synchronization—while LazyBeads gives the human operator a fast, inspectable terminal control plane. [github](https://github.com/gastownhall/beads)
