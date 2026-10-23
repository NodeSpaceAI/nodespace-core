# Code review swarm

A resumable, multi-agent review of the whole core and sync code bases. Finder agents read the code, independent skeptics try to refute every finding, a review chair decides, and only approved findings become GitHub issues. The run is split into three parts (one per night). Each part is an independent run with its own state directory, candidates, verdicts and report.

## What it does

1. Find: one agent per review unit and lens (opus on risky code, sonnet elsewhere). Finders must quote exact code with path and line, give a concrete trigger scenario and proof.
2. Consolidate: one opus agent dedupes, spot-checks the quotes against the pinned checkout, drops anything already in the known-issues list (all three repos, open and closed) and caps the list at 60 candidates. The overflow is kept as unverified and is never filed.
3. Verify: skeptics try to refute each candidate (P0 and P1 get an opus and a sonnet skeptic, P2 two sonnets, P3 one sonnet).
4. Review: a chair (opus for P0 to P2, sonnet for P3) decides APPROVE, REJECT, DUPLICATE, BY_DESIGN or NEEDS_DECISION.
5. File: only APPROVE is filed, by a filer that re-checks GitHub for duplicates.
6. Report: `report.md` in the state directory.

Nothing is filed before verification and review. Core issue bodies end with the Claude Code line, sync issue bodies have no robot footer. Nobody is assigned, tagged or mentioned. All agents are read-only on the code: no builds, no tests, no model loading, no comments on issues or PRs.

## The three parts

| Part | Covers |
|---|---|
| 1 | Sync runtime (the sync daemon, the sync library, the app plugin, installer scripts, workflows) and core `packages/core` db and services |
| 2 | Core daemon, cli, desktop-app app-lib, agent, playbook / ops / schema / markdown, nlp-engine, skill, core scripts and workflows |
| 3 | Desktop-app frontend, nodespace-types, and the backend and integration tests of core and sync |

Not covered: the cloud repo, the website, the docs, frontend unit tests under `packages/desktop-app/src/tests`, the agent and skill package tests, vendored `reference/` copies and generated code. `packages/agent` belongs to its owner: issues may be filed about it, nothing is fixed.

Rough size (the exact numbers are printed by `make-units.ts`): part 1 about 25 units and 160k lines, part 2 about 30 units and 240k lines, part 3 about 15 units and 190k lines.

## Prerequisites

- `gh` logged in with access to NodeSpaceAI (`gh auth status`); it lists existing issues and files the approved ones.
- `bun`, `git`, `jq`.
- A Claude Code session that has the Workflow tool. Keep the machine awake for the whole run, for example `caffeinate -i -t 43200` in another terminal.
- Checkouts: this repo (core), the sync repo, and optionally the docs repo (without it the reviewers skip the ADR checks). Locations are found like this, or set them with environment variables:
  - `CORE_REPO`: default is the git checkout that contains this kit.
  - `SYNC_REPO`: default `$HOME/nodespace/sync`, then a sibling directory of this checkout named `nodespace-` plus `sync`. preflight stops with a message if neither exists.
  - `DOCS_REPO`: default `$HOME/nodespace/docs`, then `../nodespace-docs` next to core. Optional.
  - `SWARM_DIR`: the state directory. Default `$HOME/nodespace-swarm/night-<part>`, one directory per night, so the parts never share state.
- Do not run more than one swarm at a time.

## Launch one night (example: part 2)

Run the shell commands from the core checkout, then the Workflow tool from Claude Code.

```sh
cd scripts/code-review-swarm
bash preflight.sh --part 2            # pins origin/main of core and sync, builds the known-issues list
bun make-units.ts --part 2            # reads the pinned snapshots, writes units.json and plan-units.json
bun make-args.ts --part 2             # writes $SWARM_DIR/args.json (and prints it)
```

`preflight.sh` creates `$SWARM_DIR/src-core` and `$SWARM_DIR/src-sync` (detached git worktrees of origin/main), `shas.json`, `existing-issues.json` (all issues of core, sync and cloud, any state) and `config.json`. Do not regenerate the units after a run has started.

Then, in the Claude Code session, run the Workflow tool with:

- `scriptPath`: the absolute path of `scripts/code-review-swarm/swarm.workflow.js` in your checkout
- `args`: the JSON value in `$SWARM_DIR/args.json` (default `~/nodespace-swarm/night-2/args.json`), passed as the object itself, not a string

Add `--profile lean` (drops the hygiene pass on critical units and the tests tier, about 40 percent cheaper) or `--max 40` (fewer candidates) to `make-args.ts` if wanted.

## Duration and cost

All three parts together are roughly 300 agents and tens of millions of tokens. One part is about a third of that: expect roughly 100 agents and 2 to 4 hours of wall clock at the 8-agent concurrency cap, longer when a quota stop interrupts it. Part 1 and part 2 use opus for the critical code, part 3 is all sonnet at the finder stage. Watch the quota; the run is built to be stopped and resumed.

## Resume after a quota stop or a crash

Every agent writes its result file and starts by reading that file if it exists, so nothing is redone.

```sh
cd scripts/code-review-swarm
bash preflight.sh --part 2            # safe: only creates what is missing, keeps the same snapshots
bun make-args.ts --part 2             # rewrites args.json; prints how many find / verify / review / filed files exist
```

Run the Workflow tool again with the same `scriptPath` and the new `args`. Finished work is skipped, unfinished agents continue from their own last file. `reuseCandidates` is set only once every finder file exists. Use the same part number and the same `SWARM_DIR` as the first run.

## Kill switch

Stop the workflow task (TaskStop in Claude Code). Result files stay. Nothing is half-filed: an issue exists only if `filed/<id>.json` has a url, and the filer re-checks GitHub for duplicates anyway.

## After the run

1. Read `$SWARM_DIR/report.md`. Go through it by priority: filed issues P0 to P3 with URLs, then NEEDS_DECISION, then rejected / duplicate / by-design counts and the unverified overflow, then incomplete units (resume to finish them).
2. `bash teardown.sh --part 2` removes the pinned snapshots (results stay).

## Files in the state directory

`units.json`, `plan-units.json`, `args.json`, `config.json`, `shas.json`, `existing-issues.json`, `src-core/`, `src-sync/`, `findings/`, `candidates.json`, `overflow-candidates.json`, `verdicts/`, `review/`, `filed/`, `scratch/`, `report.md`.

## Tests

`bun test scripts/code-review-swarm` mock-runs the workflow with fake agents (fresh, resume and lean), and covers the args builder, the unit builder and preflight / teardown against throwaway git repos. It starts no real agent.
