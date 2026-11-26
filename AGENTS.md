# NodeSpace Development Agent Guide

## CRITICAL: Pre-Release Development - NO BACKWARD COMPATIBILITY

**NodeSpace has ZERO users and NO production deployment.** Releases are published, but nobody outside the team has installed one, so an installed older release is not a compatibility target either: an update may refuse it or reset its data.

- ❌ **NO backward compatibility code** - Delete old patterns immediately when replaced
- ❌ **NO migration strategies** - We can reset the database anytime. This covers **internal data-shape changes**, not just product/version compatibility: a startup backfill, a lazy on-read format upgrade, or any code that carries existing rows from an old shape to a new one is exactly what this prohibits. **Instead: change the format and reset the database.**
- ❌ **NO gradual rollouts** - Implement new architecture directly, delete old code
- ❌ **NO transition periods** - No dual-mode support, no feature flags for compatibility
- ❌ **NO version support** - Don't maintain multiple versions of any API/method
- ❌ **NO "soak periods"** - No waiting weeks between changes
- ❌ **NO phased migrations** - Unless coordinating across multiple active worktrees
- ❌ **NO `#[deprecated]` attributes** - Delete old code, don't deprecate it
- ❌ **NO `#[allow(dead_code)]`** - Delete unused code, don't suppress warnings

This is a pre-release standard of this project, not a general principle: it ends at the first release real users install, and a post-release upgrade method then replaces it deliberately, not by accident.

Make breaking changes without hesitation. Fix breakage immediately in the same session. Implement final architecture directly — skip intermediate steps. If you find yourself writing "for backward compatibility" or "during the transition period" — **STOP. This is greenfield development.**

If you catch yourself asking *"how do existing databases get the new shape?"* — that question is the signal, not the task. The answer is always: change the shape, reset the database. Writing a backfill to answer it is the prohibited thing, even when the change feels purely internal and no user-facing version is involved.

**One exception, which shares the word but is not the same thing:** `packages/core/src/db/schema.rs` is the **SQL schema bootstrap** — the DDL that creates a database's tables at all. A fresh database still needs its schema created, so that file is legitimate and stays. The rule above bans **data** migrations (moving existing rows between shapes), not **DDL** (creating the tables).

There is **no migration ladder and no schema version**: `create_schema` defines the current shape directly and is the only thing that builds a database. When the shape changes, edit that DDL and reset the database — never add a versioned migration step, a `user_version` check, or anything that carries an older on-disk shape forward.

## Project Overview

NodeSpace is an AI-native knowledge management system: Rust backend, Svelte 5 frontend, Tauri 2.0 desktop. Stack: libsql/SQLite (local store — `packages/core/src/db/`), async/await trait-based Rust, $state/$derived/$effect runes. UI-first approach — build interfaces with mock data before storage integration.

**Design lives in NodeSpace, not in this repository.** The design documents, the decision records and the process skills are nodes in the NodeSpace workspace; the skills are attached to the `nodespace-core` project node (`54751b77-462a-44e9-af89-409245df92d9`). Reach them by title with the CLI: `nodespace search "<title>"`, then `nodespace node get <id>`. Before starting any task, read the skills "Working an Issue in nodespace-core" and "Setting Up nodespace-core and Finding Its Commands", and "Testing a Change in nodespace-core" before testing.

## Mandatory Startup Sequence

**Read the skill "Working an Issue in nodespace-core" before any task, and follow it step by step**, including the sequence for resuming from a WIP handoff commit. Find it by title: `nodespace search "Working an Issue in nodespace-core"` (or `nodespace node query --title-contains "Working an Issue"`), then `nodespace node get <id>`. If you started implementing without it, STOP, complete it, and restart.

What a session needs before it can reach NodeSpace: work in a worktree entered with `EnterWorktree({name: "issue-<number>-brief-desc"})` (never on the primary checkout's `main`, and only after `git fetch origin && git pull origin main` there), run `bun install` in it, and run every `bun run` script from the worktree root, never from a package directory.

**TodoWrite:** for a new task the first item is the full startup sequence as one step; when continuing from a WIP commit it is "WIP continuation sequence: git status, pull branch, review WIP commit, resume from Remaining Work". The last items are "Run test:changed", "Run quality:fix and commit", "Push (lint only)", "Create PR", "bun run merge + ExitWorktree".

## Finding and Tracking Work

**GitHub issues are the public intake only; the team's work is tracked in NodeSpace** (the decision "GitHub issues are the public intake only, and the team's work is tracked in NodeSpace").

- Work is a task or a spec of the `nodespace-core` project in the workspace. Find it with `nodespace search "<title>"` or `nodespace node query --title-contains "<title>"`; claim it by starting it in NodeSpace. Work you find yourself is created there, never filed on GitHub: follow the skills "Filing and Editing an Issue in nodespace-core" and "Handling Scope Found While Implementing".
- Do not create, edit, assign, or set the status of a GitHub issue for the team's work. The `bun run gh:*` scripts that did so are removed; `gh:view`, `gh:comment`, `gh:pr` and `gh:tracking-issue` remain (`bun run gh:help`) for the intake side, and a scheduled check that fails files its issue through `gh:tracking-issue`.
- A report from someone outside the team is read, shaped into a NodeSpace task whose `## Source` section holds the issue URL, answered on the issue, and closed. The skill `.claude/skills/github-intake-triage` carries the rule.
- A pull request description names its NodeSpace task. Use `Closes #N` only for an issue that came from outside, so the reporter is notified; the merge queue reads the pull request, not an issue number.
- Problems about NodeSpace Sync are built in nodespace-sync, not here (see Sync Boundary).

## Architecture & Docs

> Do not infer architecture from existing code comments — they may be stale.

Read the design document before working in an area. Search by title with `nodespace search`: "The data layer" (node storage, data models, DB queries), "Frontend state and persistence", "Frontend architecture", "NodeSpace system overview", "Technology stack", "Crate boundaries", "Node types".

## Sync Boundary (CRITICAL)

Core is the complete free product, with no sync, collaboration or access-control semantics. NodeSpace Sync is the paid offering: a synchronization service NodeSpace provides, added to the one app. It is a service, not an edition, and the word "Pro" is no longer used. Read the decisions about the Sync boundary before any sync-boundary work (search `nodespace search "extension points"`, "Sync keeps its state on its own types", "Every write is local; sync is added on top").

- **Core holds no Sync code.** No Sync UI (components, stores, settings sections, modals), Sync Tauri commands or their client, Sync proto, tenant / invite / admission logic, cloud identity, Sync build config, installer or update channel, and no edition branching: no build flag, socket or launchd name, or code path that depends on being or talking to the Sync product. Sync features and Sync bugs, fixes included, are filed on and built in nodespace-sync (ADR-081 §5). A generic extension point the Sync app uses is core work (ADR-082).
- **Sync state lives on Sync types, never on core's** (ADR-083). Sync adds ADR-078 `extends` subtypes of core types (`sync-database-settings`, `sync-collection`, `sync-person`), each with its own typed behaviour, plus Sync fields on edges such as `member_of`. Core's schemas declare no sync, restriction or AI-chat privacy fields, and core never reads them.
- **Core refuses Sync databases.** Binding a database to sync sets `required_extensions: ["sync"]` on its settings node; core's daemon refuses to open a database listing an extension it doesn't support, with "This database needs NodeSpace Sync" and a "Get NodeSpace Sync" link. This is a compatibility guard, not security.
- **Core may contain only what ADR-081 lists:** generic extension points, the Labs "Team synchronization" contact card (the default of the `collaboration.entry` slot), the Sync-database refusal, and local features with local consumers (echo suppression, event batching, OCC and the conflict journal, collections and `member_of` as grouping, the person model). Name an extension point for what it does, not for who uses it: no "Sync daemon", Supabase, tenant or nodespace-sync wording in new core code or comments.
- **`scripts/check-sync-boundary.ts` is a hard ban** (ADR-081 §8). It scans `packages/`, `scripts/` and `README.md` for Sync-boundary markers and fails on any hit: no baselines, no raise classes, no exemptions. It runs in the push check, in the merge gate's lint section before it takes the machine slot, and in `bun run test:scripts` (every merge gate). A false positive gets a narrow, tested fix to the marker's pattern in the checker.
- **Its `ALLOWLIST` holds at most one file** (ADR-081 §8): the module of core's display names for known extension ids, which maps `sync` to "NodeSpace Sync" for the Sync-database refusal and is added with it. An entry exempts only its exact string; every other marker in that file still counts, and "NodeSpace Pro" stays banned there too. Installer and CLI coexistence checks compare the installed product with `community` and name no other product, so they need no entry.
- **A test that asserts Sync code is absent builds its needle from fragments** (`["pro", "tier"].join("_")`, `["bound_ten", "ant"].join("")`, `concat!("--edi", "tion")`), so it does not contain the marker it looks for. Fragments belong only in absence tests; in product code they hide Sync code, and review rejects them.
- `bun run test:changed` skips the scripts tier for frontend-only and `.md`-only diffs, so the push check runs the ban.

## Node Type System & Schema Architecture (CRITICAL)

**Any change that adds or changes a node type, field, relationship, structural rule or subtype MUST follow the documents "Changing a Core Node Type" and "Adding a Core Node Type by Kind" in the NodeSpace workspace, the same way the startup sequence is followed.** No exceptions, not even for a single field or enum value. If you started without it, STOP, follow it, and restart.

Read first: ADR-086 (defined once, registry, categories), ADR-087 (lifecycle is governance), ADR-089 (structural rules), ADR-088 for AI chats. The per-type reference is `packages/core/shape-record/node-types.md`, generated from the registry and the seeded core schemas: after the change run `bun run node-types:gen` and commit the regenerated sheet in the same change. `bun run node-types:check` (the merge gate and `test:changed`'s Rust tier) fails when the committed sheet differs from what the code generates. It reads nothing outside this checkout. Recipes are in the documents "Creating node types" and "Adding a Core Node Type by Kind", and the design in "Node types" and the decision "Core node types are defined once, in one registry".

**Every change to the built-in shape ends with a change-log entry** (ADR-104, "Each release records its built-in shape"; design document "The built-in shape record"). A table, a core schema (fields, enum values, protection, `extends`, relationships, structural rules, context paths) or a shipped seed changes `packages/core/shape-record/current.json`. After the change run `bun run shape:gen`, commit the regenerated `current.json`, and add one entry per area to `packages/core/shape-record/change-log.json`: the class (`additive`, `reconcile`, `rebuild`, `migrate`, `refuse`), the snapshot paths it covers, whether a user could have produced the same state, the aspect and affected users for seeded content, and the affected data and transformation for `migrate`. `bun run shape:check` (the merge gate and `test:changed`'s Rust tier) fails on a difference with no entry, and on an entry for a path that did not change. It records and classifies only: it moves no data, and a `migrate` entry is a note for a later decision, not a licence to write a migration. The gate reads a hand-written class, so review reads it. `bun run release` runs `bun run shape:check` before anything else and refuses on a difference with no entry, then runs `bun run shape:release <version>` (three numbers; a suffixed pre-release gets the check but no snapshot) and commits `shape-record/snapshots/<version>.json` with the version bump, so the release's snapshot is the next baseline and is on origin/main before the tag.

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
User extending a core type?             → a field in the schema's custom_fields, stored under its own name (ADR-112)
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
- ❌ No property key on any type that no schema in its chain declares (`fields` or `custom_fields`, or a `_` bookkeeping key). No deleting core fields from schemas without the sequence and an ADR

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

Full docs: "Base node components", "Frontend architecture" and "Writing Reactive State in the Frontend".

When building components: read the architecture guide first, determine type (Node vs Viewer), follow naming, use provided templates, register in plugin system with correct lazy loading paths.

A build injects extensions with `NODESPACE_EXTENSIONS=<module path>` (a relative path resolves against `packages/desktop-app`); core ships none, its release workflow refuses the variable, and core's own test runs must not set it (ADR-082). Extension points carry no version and core makes no compatibility promise about them: core and the app built on it come from one pinned core revision, so a change reaches the consuming build through its core pin (ADR-082 §8). The fixture tests (the Rust fixtures, `scripts/fixtures/skill-extension/`, the test extension under `src/tests/fixtures/`) are tests, not a compatibility surface. The host API lists its exports explicitly (no `export *`) and core modules never import it.

## Sub-Agent Commissioning

When commissioning a specialized sub-agent, you MUST include these instructions verbatim:

```
IMPORTANT SUB-AGENT INSTRUCTIONS:
- DO NOT repeat the startup sequence (git status, branch creation, claiming the task, etc.) - the main agent has already completed this
- You are working on an EXISTING feature branch with the task already claimed and in progress
- Focus ONLY on the specific technical implementation task assigned to you
- DO NOT commit changes or create pull requests - the main agent will handle all git operations and PR creation
- DO NOT update the task's status or open or comment on pull requests - the main agent manages project status
- Follow all project standards (no lint suppression, use Bun only, etc.) but skip the administrative steps
- Continue with the existing implementation approach and maintain consistency with established patterns
- Return control to main agent when your technical work is complete
```

## Implementation Workflow

The procedure is the skill "Working an Issue in nodespace-core" (steps from implementation through review, merge and cleanup); the commands and tiers are in "Testing a Change in nodespace-core" and "Setting Up nodespace-core and Finding Its Commands"; the rules for quality, comments and documentation are in "Code Quality Standards" and "Writing Documentation and Code Comments". Find each by title with `nodespace search "<title>"` or `nodespace node query --title-contains "<title>"`. Read them rather than relying on memory of this guide.

## Development Standards

Linting, logging, runtime (Bun only), the test command rules and documentation rules are the skills "Code Quality Standards", "Testing a Change in nodespace-core" and "Writing Documentation and Code Comments"; read them by title (`nodespace search "<title>"`). What they do not hold:

**Rust toolchain:** `rust-toolchain.toml` at the repository root pins one exact Rust release for every machine, the merge gate and the release workflow (ADR-047); rustup installs it on the first cargo command. Never run `rustup override`, pass `cargo +<toolchain>`, or set `RUSTUP_TOOLCHAIN` to build or lint with another. The bump procedure, the release-workflow install and the rule for code that spawns cargo are in the skill "Setting Up nodespace-core and Finding Its Commands". The same skill holds the `bun.lock` `configVersion` rule.

**Windows app-lib tests:** the GNU build, the Common Controls manifest and the `WebView2Loader.dll` `PATH` fact are in the skill "Setting Up nodespace-core and Finding Its Commands". Beyond those: on a machine without `bun`, the nine `skill_setup` tests that run the skill installer (they need `bun` and a built `packages/skill/dist/install.js`, and one uses a unix fake-shell shim) are `ignore`d on Windows with that reason; `bundled_model::the_copy_ignores_nodespace_home` is ignored there too because `dirs` resolves the Windows home from the account, never from `HOME` (`the_home_ignores_nodespace_home` covers the `NODESPACE_HOME` half). The unix-only test helpers are `cfg(unix)`. The supported Windows subset is the app-lib `--lib` tests; the `tests/it` binaries also need a built `nodespaced` (`NODESPACED_TEST_BIN`), which needs libclang, and fail with a message naming that on a machine without one.

**Tooling scripts:** Tooling in `scripts/` is TypeScript (`scripts/*.ts`, run via `bun run`). A `.sh` file is acceptable only where shell is genuinely required — e.g. an uninstaller that must run without Bun present, or a signing/packaging pipeline driving platform tools. Prefer deleting a one-off script once its purpose is served rather than leaving it in `scripts/`.

**Git:** Branch per issue, name `issue-<number>-brief-desc`. Commits and pull request descriptions include Claude Code attribution.

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
From the task:
- [x] [completed]
- [ ] [remaining]

Co-Authored-By: Claude <noreply@anthropic.com>
```

After pushing, record a handoff summary and the commit link on the task in NodeSpace. Do NOT use WIP commits for normal development — only intentional session handoffs.

## Repository Structure

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
│   └── skill/                    # Skill package: installs NodeSpace tools into PTY agents
├── scripts/                      # Build and GitHub-intake utilities (TypeScript, run via `bun run`)
├── assets/                       # Static assets (screenshots)
├── AGENTS.md                     # Agent guide (this file)
├── CLAUDE.md                     # One line: imports AGENTS.md for Claude Code
├── README.md                     # Project overview
├── package.json                  # Bun workspace root
└── Cargo.toml                    # Rust workspace
```
