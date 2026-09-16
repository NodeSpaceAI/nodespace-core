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

**One exception, which shares the word but is not the same thing:** `packages/core/src/db/migrations/` is the **SQL schema bootstrap** — the versioned DDL that creates a database's tables at all. A fresh database still needs its schema created, so that runner is legitimate and stays. The rule above bans **data** migrations (moving existing rows between shapes), not **DDL** (creating the tables).

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

Issue priority: `foundation` (highest) > `design-system` > `ui` > `backend`

## Architecture & Docs

> Do not infer architecture from existing code comments — they may be stale.

- Node storage / data models / DB queries: read [`data-layer.md`](../nodespace-docs/architecture/data-layer.md)
- Frontend state / persistence: read [`frontend-state-and-persistence.md`](../nodespace-docs/architecture/frontend-state-and-persistence.md)
- Full architecture: `../nodespace-docs/architecture/system-overview.md`, `technology-stack.md`

## Node Type System & Schema Architecture (CRITICAL)

Read before implementing any node type or property:
- [`node-behavior-system.md`](../nodespace-docs/components/node-behavior-system.md) — hybrid Core (hardcoded) vs Extension (schema-driven) architecture
- [`schema-management.md`](../nodespace-docs/components/schema-management.md) — namespace enforcement, protection levels

**Decision tree:**

```
Adding a property to a core node type?
  Core property the UI depends on → Edit hardcoded behavior in packages/core/src/behaviors/mod.rs
  Everything else → Use schema system with NAMESPACE PREFIX (custom:propertyName)

Adding a field to a type the user defined (Venue, Invoice, …)?
  Use a BARE property name (capacity) — no prefix

Creating a new node type?
  Built-in type everyone needs → Hardcoded behavior + schema (requires issue approval)
  Everything else → Schema-only type
```

**Rules** (see [ADR-063](../nodespace-docs/decisions/063-namespace-prefixes-scope.md)):
- ✅ Extending a core type (`task`, `text`, `date`, …) requires a prefix: `custom:`, `org:`, `plugin:`. `update_schema` rejects a bare name there — a future release may claim it as a core property
- ✅ Fields of a user-defined type are stored bare. Both `create_schema` routes store the name they are given, so the same intent yields the same key either way
- ✅ Stored names are user-visible keys: they appear in `titleTemplate` tokens, CEL selectors, query filters and frontend lookups. Changing one is a breaking change to every call site
- ✅ Avoid user-defined-type field names that shadow core properties (`status`, `priority`, `due_date`, …) — `create_schema` warns rather than renaming the field for you
- ❌ No unprefixed user properties on core types — conflicts with future core properties
- ❌ No deleting core properties from schemas — breaks UI
- ❌ No hardcoded behaviors for plugin/custom types

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
   bun run test              # Fast unit tests, Happy-DOM — use during development
   bun run test:unit         # Same as above
   bun run test:watch        # TDD watch mode
   bun run test:browser      # Real browser tests (focus/blur, Playwright/Chromium)
   bun run test:browser:watch
   bun run test:all          # Unit + scripts + skill + Rust — what the pre-push gate runs
   bun run test:all:coverage # Same, with coverage instrumentation (reporting only)
   bun run test:db           # Full SQLite integration (before merging critical changes)
   bun run test:perf         # Full performance validation (large datasets)
   bun run test:coverage
   ```

   - **Happy-DOM** (`bun run test`): 99% of tests — logic, services, utilities
   - **Browser mode** (`bun run test:browser`): only for real focus/blur or browser-specific DOM APIs; requires `bunx playwright install chromium`
   - **Performance**: fast mode (default) for daily dev, `bun run test:perf` before perf-critical merges
   - **Database mode**: full integration validation before merging critical changes

4. **Quality Checks & PR**
   ```bash
   bun run quality:fix       # MANDATORY — fix all lint/format issues
   git add . && git commit -m "Fix linting and formatting"
   git push origin HEAD:issue-<number>-brief-desc
   bun run gh:pr <number>    # Creates PR, updates status to "In Review"
   ```

   > Do **not** run `bun run test:all` by hand here — the pre-push gate runs it (and more) on
   > every push, so a manual run duplicates the entire pyramid. Run the narrower `bun run test`
   > during development for fast feedback; let the gate be the gate. `quality:fix` stays manual
   > because it rewrites files, which you want done before you commit.

   > ⚠️ **`bun run gh:pr` infers the head branch from the LOCAL branch name**, which `EnterWorktree` prefixes with `worktree-`. It therefore fails with `Validation Failed: {"field":"head","code":"invalid"}` against a remote branch pushed without that prefix. Create the PR directly instead, then set status:
   > ```bash
   > gh pr create --repo NodeSpaceAI/nodespace-core --base main \
   >   --head issue-<number>-brief-desc --title "..." --body "..."
   > bun run gh:status <number> "In Review"
   > ```
   >
   > The Status field accepts exactly six values — `Backlog`, `Todo`, `In Progress`, `In Review`, `Done`, `Blocked` — each mapped to a single-select option ID on the project board. Anything else is rejected; there is no "Ready for Review".

   > ⚠️ **`git push` runs a pre-push gate (`scripts/test-gate.ts`, ADR-047)** that re-runs `test:all`, then `cargo build --bin nodespaced`, then the full `test:e2e` suite — every push, not just the first. On a fresh worktree with no Rust build cache this can take **several minutes** (cold `cargo build` alone can exceed 5 minutes). Give the push command a long timeout (10+ minutes) or run it in the background and wait for completion — a command that times out before the hook finishes looks identical to a real failure but isn't one; check the tail of the actual output for a real test failure vs. an incomplete cold build before concluding the push failed. Do not reach for `--no-verify` to work around slowness — it's reserved for WIP Handoff Commits.

5. **Code Review** — run `/pragmatic-code-review` on every PR before merge. NEVER merge without it. **Always follow it with `/address-review`, unconditionally — even when the review comes back APPROVE with zero findings.** Do not pre-judge from the review text whether anything is "just nits" or "nothing to address" and skip the step on that basis; `/address-review` itself owns that triage and the "is a re-review needed?" decision. Repeat review → address-review until `/address-review` reports no re-review needed. Then STOP — merging is the user's call, not automatic.

6. **Merge & Clean Up** — order matters:
   ```bash
   # Step 1: Verify mergeable (from worktree)
   gh pr view <PR#> --json mergeable,reviewDecision,statusCheckRollup
   ```
   ```
   # Step 2: Exit BEFORE merging
   ExitWorktree({action: "remove", discard_changes: true})
   ```
   ```bash
   # Step 3: Merge from primary checkout
   gh pr merge <PR#> --squash --delete-branch
   git pull origin main
   bun run gh:status <issue#> "Done"
   ```
   `discard_changes: true` is safe — the squash merge supersedes local branch commits. Always ExitWorktree first: `gh pr merge --delete-branch` fails noisily if you're still inside the worktree.

**TodoWrite — NEW tasks:** First item must be the full startup sequence as a single step. Last items: "Run quality:fix and commit", "Push (pre-push gate runs the full pyramid)", "Create PR", "ExitWorktree + merge".

**TodoWrite — WIP continuation:** First item: "WIP continuation sequence: git status, pull branch, review WIP commit, resume from Remaining Work". Last items same as above.

## Plan Mode — CRITICAL CONTEXT PRESERVATION

The context window clears between planning and implementation. The implementation agent sees ONLY the plan.

Every plan MUST include:

1. **Step 0 — Startup sequence:**
   > `git status` and `git pull origin main` on primary checkout, `EnterWorktree({name: "issue-<N>-brief-desc"})` (the tool owns the location and branch name — accept them), then inside the worktree: `bun install`, `bun run test` (baseline), `bun run gh:comment <N> "..."`, `bun run gh:assign <N> "@me"`, `bun run gh:status <N> "In Progress"`

2. **Final steps:**
   > `bun run quality:fix` + commit, `git push origin HEAD:issue-<N>-brief-desc` (the pre-push gate runs the full test pyramid — don't run `test:all` by hand first), then `gh pr create --head issue-<N>-brief-desc` (not `bun run gh:pr` — it fails on the `worktree-` branch prefix). After approval: `gh pr view <PR#>`, `ExitWorktree({action: "remove", discard_changes: true})`, `gh pr merge <PR#> --squash --delete-branch`.

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
│   │   ├── src-tauri/            # Tauri backend
│   │   └── [configs]             # App-specific configurations
│   ├── nodespace-types/          # Shared wire types (core + Tauri command layer)
│   ├── core/                     # Knowledge graph data layer (NodeService, ops/)
│   ├── nlp-engine/               # LLM inference and embedding (llama.cpp)
│   ├── agent/                    # AI agent orchestration (ReAct loop, ACP client)
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
