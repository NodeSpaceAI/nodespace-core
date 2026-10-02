# NodeSpace Development Agent Guide

## CRITICAL: Pre-Release Development - NO BACKWARD COMPATIBILITY

**NodeSpace has ZERO users, NO production deployment, and NO releases.**

- ❌ **NO backward compatibility code** - Delete old patterns immediately when replaced
- ❌ **NO migration strategies** - We can reset the database anytime. This covers **internal data-shape changes**, not just product/version compatibility: a startup backfill, a lazy on-read format upgrade, or any code that carries existing rows from an old shape to a new one is exactly what this prohibits. **Instead: change the format and reset the database.**
- ❌ **NO gradual rollouts** - Implement new architecture directly, delete old code
- ❌ **NO transition periods** - No dual-mode support, no feature flags for compatibility
- ❌ **NO version support** - Don't maintain multiple versions of any API/method
- ❌ **NO "soak periods"** - No waiting weeks between changes
- ❌ **NO phased migrations** - Unless coordinating across multiple active worktrees
- ❌ **NO `#[deprecated]` attributes** - Delete old code, don't deprecate it
- ❌ **NO `#[allow(dead_code)]`** - Delete unused code, don't suppress warnings

Make breaking changes without hesitation. Fix breakage immediately in the same session. Implement final architecture directly — skip intermediate steps. If you find yourself writing "for backward compatibility" or "during the transition period" — **STOP. This is greenfield development.**

If you catch yourself asking *"how do existing databases get the new shape?"* — that question is the signal, not the task. The answer is always: change the shape, reset the database. Writing a backfill to answer it is the prohibited thing, even when the change feels purely internal and no user-facing version is involved.

**One exception, which shares the word but is not the same thing:** `packages/core/src/db/schema.rs` is the **SQL schema bootstrap** — the DDL that creates a database's tables at all. A fresh database still needs its schema created, so that file is legitimate and stays. The rule above bans **data** migrations (moving existing rows between shapes), not **DDL** (creating the tables).

There is **no migration ladder and no schema version**: `create_schema` defines the current shape directly and is the only thing that builds a database. When the shape changes, edit that DDL and reset the database — never add a versioned migration step, a `user_version` check, or anything that carries an older on-disk shape forward.

## Project Overview

NodeSpace is an AI-native knowledge management system: Rust backend, Svelte 5 frontend, Tauri 2.0 desktop. Stack: libsql/SQLite (local store — `packages/core/src/db/`), async/await trait-based Rust, $state/$derived/$effect runes. UI-first approach — build interfaces with mock data before storage integration.

**Before starting any task, read:**
- [`overview.md`](../nodespace-docs/development/overview.md) - Complete development process
- [`startup-sequence.md`](../nodespace-docs/development/startup-sequence.md) - Mandatory pre-implementation steps

## Mandatory Startup Sequence — NEW TASK

> **EXCEPTION: If continuing from a WIP commit, skip to the next section.**

1. **Check git status on the primary checkout**: `git status` — commit any pending changes first
2. **Pull latest `main`**: `git fetch origin && git pull origin main`
3. **Enter an isolated worktree**: `EnterWorktree({name: "issue-<number>-brief-desc"})`
   - The tool owns the location and branch name — do not try to place it elsewhere. It creates the worktree under `.claude/worktrees/` on a branch named `worktree-<name>`, branched from `origin/main`. `EnterWorktree` only accepts paths inside that directory; a path anywhere else is rejected.
   - All subsequent commands run **inside the worktree**; primary `main` stays untouched
   - Naming: terse, no `feature/` prefix, e.g. `issue-1122-agent-tools`
   - Because the local branch carries a `worktree-` prefix and the remote branch should not, push with an explicit refspec: `git push origin HEAD:issue-<number>-brief-desc`
   - Continuing parent-issue work on a shared branch: `EnterWorktree({path: "<repo>/.claude/worktrees/<existing>"})`
4. **Install dependencies**: `bun install`
5. **Run test baseline**: `bun run test` — frontend only (Rust tests require warm cache)
   - WAIT for complete output — look for "Test Files X passed" summary and "Duration" line
6. **Document baseline**: `bun run gh:comment <number> "Frontend: X passed"`

   > ⚠️ **All `bun run gh:*` commands MUST run from the worktree root, NOT from subdirectories. Do NOT pipe to gh:comment (it doesn't read stdin).**

7. **Assign issue**: `bun run gh:assign <number> "@me"`
8. **Update project status**: `bun run gh:status <number> "In Progress"`
9. **Select subagent**, read issue requirements, plan self-contained implementation

## Mandatory Startup Sequence — CONTINUING FROM WIP

1. **Enter the existing worktree**: `EnterWorktree({path: "<repo>/.claude/worktrees/issue-<N>-brief-desc"})`
   - Find it with `git worktree list` rather than assuming the path
   - If removed: `EnterWorktree({name: "issue-<N>-brief-desc"})` to recreate, then `git checkout <branch>` inside it
2. **Check git status**: confirm you're on the right branch
3. **Pull latest**: `git fetch origin && git pull origin <branch-name>`
4. **Sync dependencies if needed**: `bun install` — only if WIP commit mentions new packages
5. **Review WIP commit message**: understand completed work and remaining tasks
6. **Resume** from the "Remaining Work" section

**DO NOT** re-run baseline, re-assign the issue, re-update status, or create a new worktree.

## Critical Process Violations

If you start implementation without completing the startup sequence: STOP, complete it, restart.

**Common mistakes:**
- Skipping `git pull` on main before EnterWorktree — worktree branches from stale local `main`
- Skipping test baseline — leads to undetected regressions
- Running `bun run gh:*` from a subdirectory — fails with "Script not found"
- Reading/editing files before EnterWorktree — edits land in wrong checkout
- Skipping EnterWorktree entirely and working on `main` — blocks parallel work
- Fighting `EnterWorktree` over where it puts the worktree or what it names the branch — it owns both; "correcting" them wastes calls and fixes nothing
- `cd`-ing into a package directory — the Bash working directory persists across calls, so a later `bun run gh:*` fails with "Script not found". Use `bun run --cwd <pkg>` / `cargo -p <pkg>` instead
- Using TodoWrite without startup sequence as the first item

## Finding Tasks

```bash
bun run gh:list
bun run gh:view <issue-number>
bun run gh:edit <issue-number> --title "New Title"
bun run gh:edit <issue-number> --body "Updated description"
bun run gh:edit <issue-number> --labels "foundation,ui"
bun run gh:edit <issue-number> --state "closed"
```

When creating or modifying issues, follow the [Issue Workflow Guide](../nodespace-docs/development/issue-workflow.md).

**Never assign an issue at creation time.** An assignee means *work is in progress* — it is set in the startup sequence (`bun run gh:assign <N> "@me"`), at the moment work actually begins, and nowhere else. Do not pass `--assignees` to `gh:create`, and do not assign someone to a newly filed issue to indicate intent, ownership, or triage. A backlog issue is unassigned; that is how the backlog stays readable. (`gh:create` does not assign by default — this only happens when the flag is passed explicitly.)

**Pro and sync issues belong in nodespace-sync** (ADR-081): anything about Pro UI, Pro commands, tenants, membership invites or admission, Supabase, or the Pro installer or update channel. Don't file them here. A generic extension point that the Pro app uses is core work and is filed here.

Issue priority: `foundation` (highest) > `design-system` > `ui` > `backend`

## Architecture & Docs

> Do not infer architecture from existing code comments — they may be stale.

- Node storage / data models / DB queries: read [`data-layer.md`](../nodespace-docs/architecture/data-layer.md)
- Frontend state / persistence: read [`frontend-state-and-persistence.md`](../nodespace-docs/architecture/frontend-state-and-persistence.md)
- Full architecture: `../nodespace-docs/architecture/system-overview.md`, `technology-stack.md`

## Pro / Sync Boundary (CRITICAL)

Core is the complete free product, with no sync, collaboration or access-control semantics. NodeSpace Pro is a separate app that nodespace-sync builds on top of core; it replaces the installed app. Read [ADR-081](../nodespace-docs/decisions/081-pro-is-a-separate-app-core-ships-no-pro-code.md) to [ADR-085](../nodespace-docs/decisions/085-ai-chat-sync-policy-in-pro.md) before any Pro-boundary work.

- **Core holds no Pro code.** No Pro UI (components, stores, settings sections, modals), Pro Tauri commands or their client, Pro proto, tenant / invite / admission logic, cloud identity, Pro build config, installer or update channel, and no edition branching: no build flag, socket or launchd name, or code path that depends on being or talking to the Pro product. Pro features and Pro bugs, fixes included, are filed on and built in nodespace-sync ([ADR-081](../nodespace-docs/decisions/081-pro-is-a-separate-app-core-ships-no-pro-code.md) §5). A generic extension point the Pro app uses is core work ([ADR-082](../nodespace-docs/decisions/082-core-extension-points-for-the-pro-app.md)).
- **Pro state lives on Pro types, never on core's** ([ADR-083](../nodespace-docs/decisions/083-pro-owned-state-leaves-the-core-data-model.md)). Pro adds ADR-078 `extends` subtypes of core types (`pro-database-settings`, `pro-collection`, `pro-person`), each with its own typed behaviour, plus Pro fields on edges such as `member_of`. Core's schemas declare no sync, restriction or AI-chat privacy fields, and core never reads them.
- **Core refuses Pro databases.** Binding a database to sync sets `required_extensions: ["pro"]` on its settings node; core's daemon refuses to open a database listing an extension it doesn't support, with "This database needs NodeSpace Pro" and a download link. This is a compatibility guard, not security.
- **Core may contain only what ADR-081 lists:** generic extension points, the Labs "Team synchronization" contact card (the default of the `collaboration.entry` slot), the Pro-database refusal, and local features with local consumers (echo suppression, event batching, OCC and the conflict journal, collections and `member_of` as grouping, the person model). Name an extension point for what it does, not for who uses it: no "Pro daemon", Supabase, tenant or nodespace-sync wording in new core code or comments.
- **`scripts/check-pro-boundary.ts` is a hard ban** (ADR-081 §8). It scans `packages/`, `scripts/` and `README.md` for Pro markers and fails on any hit: no baselines, no raise classes, no exemptions. It runs in the push check, in the merge gate's lint section before it takes the machine slot, and in `bun run test:scripts` (every merge gate). A false positive gets a narrow, tested fix to the marker's pattern in the checker.
- **Its `ALLOWLIST` holds at most one file** (ADR-081 §8): the module of core's display names for known extension ids, which maps `pro` to "NodeSpace Pro" for the Pro-database refusal and is added with it. An entry exempts only its exact string; every other marker in that file still counts. Installer and CLI coexistence checks compare the installed product with `community` and name no other product, so they need no entry.
- **A test that asserts Pro code is absent builds its needle from fragments** (`["pro", "tier"].join("_")`, `["bound_ten", "ant"].join("")`, `concat!("--edi", "tion")`), so it does not contain the marker it looks for. Fragments belong only in absence tests; in product code they hide Pro code, and review rejects them.
- `bun run test:changed` skips the scripts tier for frontend-only and `.md`-only diffs, so the push check runs the ban.

## Node Type System & Schema Architecture (CRITICAL)

**Any change that adds or changes a node type, field, relationship, structural rule or subtype MUST follow the [node type sequence](../nodespace-docs/development/node-type-sequence.md), the same way the startup sequence is followed.** No exceptions, not even for a single field or enum value. If you started without it, STOP, follow it, and restart.

Read first: [ADR-086](../nodespace-docs/decisions/086-core-node-types-defined-once-typed-by-category.md) (defined once, registry, categories), [ADR-087](../nodespace-docs/decisions/087-lifecycle-status-is-governance-and-one-participation-rule.md) (lifecycle is governance), [ADR-089](../nodespace-docs/decisions/089-structural-rules-children-and-parent.md) (structural rules), [ADR-088](../nodespace-docs/decisions/088-ai-chat-subtypes-and-message-nodes.md) for AI chats, and [`node-types.md`](../nodespace-docs/components/node-types.md). `node-types.md` is the one per-type reference, and it is updated in the same change: `bun run node-types:check` (the merge gate and `test:changed`'s Rust tier) fails when its type list or a type's field table differs from the registry and the seeded core schemas. It finds the docs repository beside the primary checkout, or at `NODESPACE_DOCS_DIR`, and skips with a warning on a machine without one. Recipes are in [`creating-node-types.md`](../nodespace-docs/components/creating-node-types.md).

**Decision tree:**

```
Shipping a new kind of node?
  No fields (data is content)           → primitive core type
  Scalar fields                         → flat core type
  Nested fields                         → structured core type
  Specializes a shipped type            → core subtype (extends it; inherits all its rules)
  Shared base never created directly    → abstract base (abstract: true)
Adding state to a core type?
  It belongs to the type                → declare a field in its (closed) schema
  It belongs to a specialization        → a subtype's field, bare name, own bucket
  It links nodes                        → a declared relationship, not a list of ids
  It's "hidden / retired"               → lifecycle_status via governance; never a type field
User extending a core type?             → custom:/org:/plugin: prefix (ADR-063)
User-defined type's field?              → bare name
Another build's data?                   → its own extends subtype (ADR-082/083); never on a core type
```

**Rules:**
- ✅ One definition per wire type, in `nodespace-types`. TypeScript is generated (ts-rs), never hand-copied: after changing a wire type, the registry or the promoted fields, run `bun run gen:types` and commit `packages/desktop-app/src/lib/types/generated/`. Never edit those files. `bun run test:changed` and the merge gate fail when they differ from Rust (`bun run types:check`). Hand-written TypeScript adds only guards, default-filling converters and helpers around the generated shapes
- ✅ Every field is declared. The schema `default` is the only default. Field names are snake_case
- ✅ Rules resolve through the `extends` chain via the registry. Subtypes add rules; they never relax them
- ✅ Ids are UUIDs, except `date`, `schema` and `database-settings-singleton`. Seeded nodes use fixed literal UUIDs
- ✅ Stored names are user-visible keys. They appear in `titleTemplate` tokens, CEL selectors, query filters and frontend lookups, so changing one breaks every call site
- ✅ Avoid user-defined field names that shadow core properties (`status`, `priority`, `due_date`, …). `create_schema` warns but doesn't rename the field
- ❌ No `node_type == "<literal>"` comparisons (Rust, SQL or TypeScript). Resolve the type through the registry: `type_is_a` / `CoreNodeType::nearest` in Rust, `is_a_sql` in SQL, `isA` in TypeScript; `is_exactly` / `isExactly` only where a subtype must not match. `scripts/check-node-type-literals.ts` fails the push check and `bun run test:scripts` on a literal comparison outside tests
- ❌ No reads of `lifecycle_status` outside the governance participation check, and no type-specific meaning for `archived`. Ask `crate::governance`: `participates` / `is_visible` in Rust, `participates_sql` / `default_query_conditions` in SQL; `include_archived` is the only opt-in. `scripts/check-lifecycle-reads.ts` fails the push check and `bun run test:scripts` on a comparison, a match or a lookup of the field by name outside tests
- ❌ No undeclared property keys, wrapper structs, `default_metadata`, hierarchy flags or compatibility shims
- ❌ No unprefixed user properties on core types. No deleting core fields from schemas without the sequence and an ADR

## Component Architecture (CRITICAL)

Naming conventions (follow exactly):
- `*Node` — individual node components wrapping BaseNode
- `*NodeViewer` — page-level viewers wrapping BaseNodeViewer

**Correct hierarchy:**
- `BaseNode` (`src/lib/design/components/base-node.svelte`) — abstract core, NEVER use directly
- `BaseNodeViewer` (`src/lib/design/components/base-node-viewer.svelte`) — node collection manager
- `TextNode`, `TaskNode`, `DateNode` — concrete node wrappers
- `DateNodeViewer` — date page viewer

**Do NOT create:** `TextNodeViewer`, `DatePageViewer`, or direct BaseNode usage in app code.

Full docs: [`component-architecture.md`](../nodespace-docs/components/component-architecture.md), [`frontend-architecture.md`](../nodespace-docs/architecture/frontend-architecture.md)

When building components: read the architecture guide first, determine type (Node vs Viewer), follow naming, use provided templates, register in plugin system with correct lazy loading paths.

A build injects extensions with `NODESPACE_EXTENSIONS=<module path>` (a relative path resolves against `packages/desktop-app`); core ships none, its release workflow refuses the variable, and core's own test runs must not set it (ADR-082). Changing the host API (`@nodespace/extension-api`), the registry types, slot hosts or hook timing needs an `EXTENSION_API_VERSION` bump and a re-recorded surface snapshot (ADR-082).

## Sub-Agent Commissioning

When commissioning a specialized sub-agent, you MUST include these instructions verbatim:

```
IMPORTANT SUB-AGENT INSTRUCTIONS:
- DO NOT repeat the startup sequence (git status, branch creation, issue assignment, etc.) - the main agent has already completed this
- You are working on an EXISTING feature branch with the issue already assigned and in progress
- Focus ONLY on the specific technical implementation task assigned to you
- DO NOT commit changes or create pull requests - the main agent will handle all git operations and PR creation
- DO NOT run project management commands (bun run gh:status, bun run gh:pr, etc.) - main agent manages project status
- Follow all project standards (no lint suppression, use Bun only, etc.) but skip the administrative steps
- Continue with the existing implementation approach and maintain consistency with established patterns
- Return control to main agent when your technical work is complete
```

## Implementation Workflow

1. **Pick an Issue & Assign Yourself** (from worktree root after startup sequence)
   ```bash
   bun run gh:list
   bun run gh:view <number>
   bun run gh:assign <number> "@me"
   bun run gh:status <number> "In Progress"
   ```

2. **Implement with Self-Contained Approach**
   - Use mock data/services temporarily for independent development
   - Vertical slicing: complete features end-to-end, not horizontal layers
   - Check off each `- [ ]` acceptance criterion as you complete it

3. **Testing**
   ```bash
   bun run test:changed      # The tiers your working diff reaches — run before committing
   bun run test              # Fast unit tests, Happy-DOM — use during development
   bun run test:unit         # Same as above
   bun run test:watch        # TDD watch mode
   bun run test:browser      # Real browser tests (focus/blur, Playwright/Chromium)
   bun run test:browser:watch
   bun run test:all          # Unit + scripts + skill + Rust (nextest)
   bun run test:all:coverage # Same, with coverage instrumentation (reporting only)
   bun run test:db           # Full SQLite integration (before merging critical changes)
   bun run test:perf         # Performance benchmarks — run at release time, never in the gates
   bun run test:coverage
   ```

   - **Tests are your job, not the push's.** A push only lints; the merge gate is the single full run. Run `bun run test` or `bun run test:changed` while you work and before you commit — a failure found at merge time costs a full slot in the one-at-a-time merge queue. `test:changed` runs the tiers `scripts/gate-scope.ts` says your diff against `origin/main` reaches (a change it doesn't recognize, or to the gate itself, runs every tier; `NODESPACE_TEST_ALL=1` forces that). Its Rust tier waits while a merge gate is running and holds the machine slot while it runs; the other tiers take no lock and run at low priority.
   - **Happy-DOM** (`bun run test`): 99% of tests — logic, services, utilities
   - **Browser mode** (`bun run test:browser`): only for real focus/blur or browser-specific DOM APIs; requires `bunx playwright install chromium` (the gate installs it when missing)
   - **Performance**: `src/tests/performance/**` is excluded from the unit run; wall-clock thresholds fail under machine load for reasons unrelated to the change. `bun run release` runs `test:perf` on the releaser's quiet machine before it creates a release (`--skip-perf` overrides a run known to be noisy). Don't add timing assertions to correctness tests
   - **Database mode**: full integration validation before merging critical changes

4. **Quality Checks & PR**
   ```bash
   bun run quality:fix       # MANDATORY — fix all lint/format issues
   git add . && git commit -m "Fix linting and formatting"
   git push origin HEAD:issue-<number>-brief-desc
   bun run gh:pr <number>    # Creates PR, updates status to "In Review"
   ```

   > Do **not** run `bun run test:all` by hand here — `bun run test:changed` covers what your
   > change reaches, and the merge gate runs everything. `quality:fix` stays manual because it
   > rewrites files, which you want done before you commit.

   > ⚠️ **`bun run gh:pr` infers the head branch from the LOCAL branch name**, which `EnterWorktree` prefixes with `worktree-`. It therefore fails with `Validation Failed: {"field":"head","code":"invalid"}` against a remote branch pushed without that prefix. Create the PR directly instead, then set status:
   > ```bash
   > gh pr create --repo NodeSpaceAI/nodespace-core --base main \
   >   --head issue-<number>-brief-desc --title "..." --body "..."
   > bun run gh:status <number> "In Review"
   > ```
   >
   > The Status field accepts exactly six values — `Backlog`, `Todo`, `In Progress`, `In Review`, `Done`, `Blocked` — each mapped to a single-select option ID on the project board. Anything else is rejected; there is no "Ready for Review".

   > ⚠️ **The gate has two modes (`scripts/test-gate.ts`, ADR-047).**
   > - **`git push` runs lint only** — scripts lint/typecheck, the design-token check, the app-version drift check, the Pro-boundary hard ban, the literal node-type comparison check and the `lifecycle_status` read check. Seconds, no lock, no tests. A push needs no long timeout.
   > - **`bun run merge <PR#>` queues the PR, and the merge queue runs the full pyramid once** on exactly what lands: the queued PRs stacked onto current main. That covers every unit tier plus the daemon build, the SKILL.md, generated-TypeScript and `node-types.md` drift checks, e2e and Tauri-seam tests, and the PRs are then squash-merged. It is the only automated test run, and how PRs are merged (see step 6).
   >
   > The merge gate holds the machine-wide **machine slot** from its first compile until it exits, so no other Rust build or Rust test run (a `test:changed` Rust tier) shares the machine with it; a merge's ticket queues ahead of those. Neither ever runs without the slot: a merge gate that can't get it within two hours fails, and a `test:changed` Rust tier that can't get it within 30 minutes stops and asks you to re-run. Every stage has a timeout, printed when it starts (`▶ rust:test … (timeout 20m)`), sized to catch hangs, not slowness: a stage that exceeds it is killed with its whole process tree and fails the gate, releasing the slot. A waiting run prints a status line every minute naming the holder's pid and worktree and how long it has held — a wait with fresh status lines is a queue, not a hang. The gate prints one line per stage; full output goes to a log file whose path it prints (and the tail, on failure). The merge gate refuses to start with under 20 GB free — every worktree's `target/` holds its own build output, so remove finished worktrees rather than letting them accumulate. Always run `bun run merge` in the background and wait for it: a queued merge can wait hours, and a command that times out looks identical to a real failure but isn't one. `bun install` installs pinned cargo-nextest and sccache into the repo's `.tools/` (`scripts/setup-rust-tooling.ts`), shared by every worktree through a `.tools` link — nothing is installed or configured outside the repository. It also writes each checkout a gitignored `.cargo/config.toml` that routes every development cargo build (bare `cargo check` included) through sccache on the gate's shared cache, with a server per checkout, so a new worktree reuses llama.cpp's C/C++ build instead of compiling it cold (Rust crates only hit within one target dir; a moved worktree needs `bun install` again); sidecar binaries never need staging or copying in. Each crate's integration tests are one test binary (`tests/it/main.rs`, one module per file): add a test file as a module there, and run one file's tests with `.tools/bin/cargo-nextest nextest run -p <crate> --test it <module>::` (nextest, not plain `cargo test`, which runs the whole binary's tests in one process). `--no-verify` is reserved for WIP Handoff Commits and non-executable diffs.

5. **Code Review** — run `/pragmatic-code-review` on every PR before merge. NEVER merge without it. **Always follow it with `/address-review`, unconditionally — even when the review comes back APPROVE with zero findings.** Do not pre-judge from the review text whether anything is "just nits" or "nothing to address" and skip the step on that basis; `/address-review` itself owns that triage and the "is a re-review needed?" decision. Repeat review → address-review until `/address-review` reports no re-review needed. Then STOP — merging is the user's call, not automatic.

6. **Merge & Clean Up** — only after the user says to merge:
   ```bash
   # Step 1: Full gate, then merge (from any checkout; everything pushed first)
   bun run merge <PR#>
   ```
   It puts the PR in the **team-wide merge queue** (git refs on origin, `scripts/merge-queue.ts`) and waits until the PR lands or leaves the queue. One gate runs at a time across every machine. Whichever `bun run merge` takes the queue's lock runs the next round, for every queued PR, yours or a teammate's:
   - It stacks the PRs as pushed onto current main, in the persistent gate checkout (`.claude/worktrees/_gate`, kept warm so only changed crates recompile).
   - It runs the full pyramid once on the stack.
   - On a pass, it squash-merges the PRs in order. Before each one it checks that the PR's replay onto main reproduces the tested tree, and that it still holds the queue's lock. `--match-head-commit` pins the merge to that commit. The PR's remote branch is then deleted. A PR GitHub refuses to merge (a draft, say) is ejected.
   - On a failure, it retests the first half of the batch, down to the PR that broke it.
   - A PR that conflicts or fails the gate is ejected, with the reason in a comment on the PR; the waiting `bun run merge` then exits 1.

   Run `bun run merge` from an up-to-date checkout, such as the pulled primary checkout. It runs that checkout's own `scripts/merge-pr.ts`, and a stale one merges the old way, around the queue. main moves only through the queue, so a running gate is never invalidated by another merge. `main` has no branch protection: a direct push (a release's version bump) bypasses the queue, and the queue stops landing when it sees one and retests. A PR leaves the queue when it lands, when it's ejected, or when its `bun run merge` stops waiting (Ctrl-C or 3 hours), so run it in the background and wait. The lock heartbeats every minute, and a lock unchanged for 10 minutes is taken over from a dead holder. Your PR worktree is never touched; unpushed commits are refused, not silently skipped. `bun run merge <PR#> --dry-run` gates that one PR on current main in the gate checkout, with no queue, push or merge. **Merge with `bun run merge`, not `gh pr merge` or the GitHub button**: a push only lints, so `bun run merge` is where a change gets its one full test run.
   ```
   # Step 2: Leave the worktree
   ExitWorktree({action: "remove", discard_changes: true})
   ```
   ```bash
   # Step 3: From the primary checkout
   git pull origin main
   bun run gh:status <issue#> "Done"
   ```
   `discard_changes: true` is safe — the squash merge supersedes local branch commits.

**TodoWrite — NEW tasks:** First item must be the full startup sequence as a single step. Last items: "Run test:changed", "Run quality:fix and commit", "Push (lint only)", "Create PR", "bun run merge + ExitWorktree".

**TodoWrite — WIP continuation:** First item: "WIP continuation sequence: git status, pull branch, review WIP commit, resume from Remaining Work". Last items same as above.

## Plan Mode — CRITICAL CONTEXT PRESERVATION

The context window clears between planning and implementation. The implementation agent sees ONLY the plan.

Every plan MUST include:

1. **Step 0 — Startup sequence:**
   > `git status` and `git pull origin main` on primary checkout, `EnterWorktree({name: "issue-<N>-brief-desc"})` (the tool owns the location and branch name — accept them), then inside the worktree: `bun install`, `bun run test` (baseline), `bun run gh:comment <N> "..."`, `bun run gh:assign <N> "@me"`, `bun run gh:status <N> "In Progress"`

2. **Final steps:**
   > `bun run test:changed`, `bun run quality:fix` + commit, `git push origin HEAD:issue-<N>-brief-desc` (lint only — don't run `test:all` by hand), then `gh pr create --head issue-<N>-brief-desc` (not `bun run gh:pr` — it fails on the `worktree-` branch prefix). After the user approves the merge: `bun run merge <PR#>` (full pyramid on the rebased PR in the warm gate checkout, then squash-merge), then `ExitWorktree({action: "remove", discard_changes: true})`.

3. **Inline standards** the implementation agent needs: e.g. "use `createLogger` not `console.log`", "mock Tauri with `vi.mock('@tauri-apps/api/core')`", "use `bun run test` not `bun test`".

## Development Standards

**Linting:** NO lint suppression — fix issues properly. No `any` types. No `{@html}`. Full docs: [`code-quality.md`](../nodespace-docs/development/standards/code-quality.md)

**Logging:** NO raw `console.log/debug/info/warn/error` in production code.
```ts
import { createLogger } from '$lib/utils/logger';
const log = createLogger('ServiceName');
log.debug() / log.info() / log.warn() / log.error()
```
Test files and DeveloperInspector are exempt.

**Runtime:** Bun-only. `npm`/`yarn`/`pnpm` are blocked. Use `bun install`, `bun run dev`, `bun run test`, `bunx` for one-off tools.

**`bun.lock`:** The `"configVersion": 0` line in its header is written by Bun 1.3+ and is intentional — never remove it or revert it as stray churn, and never change it to `1` (`0` keeps the hoisted linker; `1` switches the workspace to isolated installs). Removing it makes every `bun install` re-add it, dirtying every fresh worktree.

**Tooling scripts:** Tooling in `scripts/` is TypeScript (`scripts/*.ts`, run via `bun run`). A `.sh` file is acceptable only where shell is genuinely required — e.g. an uninstaller that must run without Bun present, or a signing/packaging pipeline driving platform tools. Prefer deleting a one-off script once its purpose is served rather than leaving it in `scripts/`.

**Testing — NEVER use `bun test`** — it bypasses the Happy-DOM vitest config and breaks DOM tests. Always use `bun run test` or another `bun run test:*` command.

The one exception is `bun run test:scripts` (`bun test scripts/`), which covers standalone tooling tests under `scripts/`. Those files import `bun:test` and sit outside every Vitest project glob, so Vitest cannot run them. **Tests under `scripts/` must therefore stay DOM-free** — a DOM test placed there would be silently misrouted around Happy-DOM, which is precisely what the rule above exists to prevent.

**Git:** Branch per issue, name `issue-<number>-brief-desc`. Link commits: `git commit -m "Add TextNode component (closes #4)"`. Include Claude Code attribution.

**Documentation:** All specs/design/architecture/ADRs live in `../nodespace-docs/` — this repo carries only root `CLAUDE.md` + `README.md`, both pointing there. Never add a `docs/` directory, nested READMEs, or other `.md` docs to this repo. Do not embed GitHub issue numbers in documentation content or code comments — describe the behavior/constraint directly, and reference decisions by ADR. (GitHub process mechanics like commit/PR/branch conventions above are unaffected.) Full rule: [`documentation.md`](../nodespace-docs/development/standards/documentation.md)

## WIP Handoff Commits

Create when: implementation spans multiple sessions, approaching context limits, at a natural breakpoint, or before risky changes. Push immediately after creating.

**Commit template:**
```
WIP: [Brief description of what was accomplished]

## Completed in This Session
- [x] Phase 1: [accomplishment]

## Remaining Work
- [ ] Phase 2: [what's next]

## Current State
- Files modified: [key files]
- Tests status: [Passing/Failing/Not yet written]
- Known issues: [blockers or concerns]
- Dependencies: [what this depends on]

## Context for Next Session
[2-3 sentences: overall approach, key decisions, what to focus on next]

## Acceptance Criteria Status
From issue #[number]:
- [x] [completed]
- [ ] [remaining]

Co-Authored-By: Claude <noreply@anthropic.com>
```

After pushing, update the issue comment with a handoff summary and commit link. Do NOT use WIP commits for normal development — only intentional session handoffs.

## Documentation Search

Documentation lives in `../nodespace-docs/` and is searchable via the NodeSpace CLI/daemon (skills-based interface, not HTTP MCP). Read the docs directly from the filesystem when you need architecture or component references — the `../nodespace-docs/` directory is always available.

To import or refresh docs into NodeSpace: `nodespace import dir ../nodespace-docs --auto-collection-routing --exclude archived`


## Repository Structure

> Documentation lives in [`../nodespace-docs/`](../nodespace-docs/) — a separate repo.

```
nodespace-core/
├── packages/
│   ├── desktop-app/              # Tauri desktop shell (thin command bindings)
│   │   ├── src/                  # Frontend source (Svelte 5)
│   │   │   ├── lib/design/       # Design system: components, tokens.ts, theme.ts
│   │   │   └── app.css           # Semantic color tokens (light + dark)
│   │   ├── app-lib/              # Tauri app library (`nodespace-app-lib`): commands, services, entry point
│   │   ├── app-build/            # Build-script helpers for an app crate (`nodespace-app-build`): unstaged bundle entries, stale sidecars
│   │   ├── src-tauri/            # Tauri app crate (`nodespace-app`): main.rs, build.rs, config, icons, sidecars
│   │   └── [configs]             # App-specific configurations
│   ├── nodespace-types/          # Shared wire types (core + Tauri command layer)
│   ├── core/                     # Knowledge graph data layer (NodeService, ops/)
│   ├── nlp-engine/               # LLM inference and embedding (llama.cpp)
│   ├── agent/                    # AI agent orchestration (local ReAct loop, PTY external-agent orchestration)
│   ├── proto/                    # Generated gRPC proto types (client stubs only)
│   ├── daemon/                   # gRPC daemon (nodespaced) — service definitions
│   ├── cli/                      # `nodespace` CLI — a gRPC client for nodespaced
│   ├── skill/                    # Skill package: installs NodeSpace tools into PTY agents
│   └── dev-tools/                # Bun dev-proxy
├── scripts/                      # Build and GitHub utilities (TypeScript, run via `bun run`)
├── assets/                       # Static assets (screenshots)
├── CLAUDE.md                     # Agent guide (this file)
├── README.md                     # Project overview
├── package.json                  # Bun workspace root
└── Cargo.toml                    # Rust workspace
```
