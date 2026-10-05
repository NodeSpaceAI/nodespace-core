#!/usr/bin/env bun
// `bun run test:changed` — runs the test tiers the working diff against
// origin/main reaches (gate-scope.ts): the fast-feedback loop during
// development. Pushes only lint and the merge gate is the one full run
// (ADR-047), so this is where a change's tests run before it is pushed.
//
// It takes no lock for the frontend, scripts and skill tiers, and runs them
// under `nice` so they yield CPU to a merge gate testing at the same time.
// The Rust tier builds and runs the whole workspace's tests, which starves a
// merge gate's tests even under `nice`, so it waits for the machine slot
// (gate-lock.ts) — which a merge gate holds for its whole run — and holds it
// only while that tier runs.
//
// Output works as in the gate: one line per tier, full output in a log file,
// the log's tail on failure.
//
//   bun run test:changed          the tiers the diff reaches
//   NODESPACE_TEST_ALL=1 bun run test:changed   every tier

import { join } from "node:path";
import { reportBranchBehind } from "./check-branch-behind";
import { stageSkipReason } from "./check-node-types-doc";
import { acquireGateLock, DISABLE_ENV_VAR, MACHINE_LOCK_PATH, MACHINE_SLOT_WHAT, registerLockRelease } from "./gate-lock";
import { describeScope, gateScope } from "./gate-scope";
import { formatDepsResult, formatPruneResult, freeGiB, freeSpaceRefusal, pruneDeps, pruneIncremental, WORKTREE_INCREMENTAL } from "./gate-disk";
import { createLogDir, GATE_INFRA_EXIT, runStage, TIERS, type StageSpec } from "./gate-stage";

const MINUTE = 60_000;

// A failure here may come from a stale base rather than this change; the
// merge gate tests the rebased result. Never blocks: checkBranchBehind()
// swallows its documented failure modes, and this try/catch covers the rest.
try {
  await reportBranchBehind();
} catch (err) {
  console.warn("\n⚠ origin/main staleness check crashed unexpectedly — skipping it.");
  console.warn(`  ${err instanceof Error ? err.message : String(err)}\n`);
}

const scope = await gateScope();
console.log(`\n▶ test:changed: ${describeScope(scope)}`);
const logDir = createLogDir("changed");
console.log(`  stage logs: ${logDir}\n`);
if (!scope.frontend && !scope.rust && !scope.skill && !scope.scripts) {
  console.log("✓ Nothing to test — no change reaches a test tier.\n");
  process.exit(0);
}

const niced = (stage: StageSpec): StageSpec => ({ ...stage, nice: true });

async function run(stage: StageSpec) {
  if (await runStage(stage, logDir)) return;
  console.error(`\n✗ ${stage.label} failed.\n`);
  process.exit(1);
}

if (scope.rust || scope.skill) await run(niced(TIERS.skillInstaller));
if (scope.frontend) await run(niced(TIERS.frontend));
if (scope.scripts) await run(niced(TIERS.scripts));
if (scope.skill) await run(niced(TIERS.skill));
if (scope.frontend) await run(niced(TIERS.browser));
if (scope.rust) {
  const slot = await acquireGateLock({ lockPath: MACHINE_LOCK_PATH, what: MACHINE_SLOT_WHAT, shared: true });
  // Past the wait cap the lock would let this run anyway, building Rust beside
  // a merge gate's tests. Stop instead — unless the person opted out of the
  // lock on purpose.
  if (!slot.held && !process.env[DISABLE_ENV_VAR]) {
    console.error("\n✗ The machine slot stayed busy (see above) — re-run the Rust tier when it is free.\n");
    process.exit(1);
  }
  registerLockRelease(slot);
  // Under the slot, before the first compile: drop this worktree's superseded
  // incremental directories and empty an over-budget deps/ (gate-disk.ts),
  // then refuse a disk too full to build on. A build that fills the disk fails
  // partway with an I/O error and stops every other session on the machine
  // with it. That is this machine's fault, not the change's, so it exits like
  // the merge gate's refusal.
  console.log(formatPruneResult(pruneIncremental(join(process.cwd(), "target"), WORKTREE_INCREMENTAL)));
  // Only with the slot: a run that opted out of the lock may be building
  // beside another, and emptying deps/ under a running compile fails it.
  if (slot.held) console.log(formatDepsResult(pruneDeps(join(process.cwd(), "target"))));
  const refusal = freeSpaceRefusal(freeGiB("."), "the test:changed Rust tier");
  if (refusal !== null) {
    console.error(refusal);
    process.exit(GATE_INFRA_EXIT);
  }
  await run(TIERS.typesCheck);
  // First to compile nodespace-core in a fresh worktree, so it gets the cold
  // build's time, like the Rust tier below.
  const nodeTypesSkip = await stageSkipReason();
  if (nodeTypesSkip === null) await run({ ...TIERS.nodeTypesCheck, timeoutMs: 60 * MINUTE });
  else console.warn(`  ⚠ ${TIERS.nodeTypesCheck.label} SKIPPED: ${nodeTypesSkip}`);
  // Unlike the merge gate's, this tier compiles as well as tests, in this
  // worktree's own incremental build — a cold one can take many minutes.
  await run({ ...TIERS.rust, timeoutMs: 60 * MINUTE });
  slot.release();
}

console.log("\n✓ test:changed passed.\n");
