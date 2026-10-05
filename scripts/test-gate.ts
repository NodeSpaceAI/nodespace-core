#!/usr/bin/env bun

/**
 * The local gate, in two modes (ADR-047). This repo has no CI runner for
 * tests; this script is the only gate.
 *
 * - `push` (the default, run by the Husky pre-push hook): lint, the
 *   app-version drift check and the code-boundary ratchet only. Seconds,
 *   and it takes no lock. A push only publishes a branch; the merge is what
 *   changes main, and the merge gate tests it. Test the tiers a change
 *   reaches while developing, with `bun run test:changed`.
 * - `merge` (`--mode=merge`, run by `bun run merge <PR#>`): lint, then the
 *   full pyramid — every unit tier, workspace clippy, the daemon build, the
 *   SKILL.md, generated-TypeScript and node-types.md drift checks, e2e and
 *   the Tauri-seam tests — unscoped, on the PR rebased onto current main.
 *   The only automated test run a change gets.
 *
 * The merge gate holds the machine slot (gate-lock.ts) from its first compile
 * until it exits. Tests starve into timeouts on correct code when another
 * compile or test run shares the machine, and a Rust build beside a merge's
 * tests roughly doubled the gate's runtime even under `nice`.
 *
 * Every stage has a timeout (gate-stage.ts). A stage that exceeds it is
 * killed with its process tree and fails the gate, which releases its locks.
 *
 * Output: each stage's full output goes to a log file, and the gate prints
 * one line per stage (plus the log's tail when a stage fails). The gate runs
 * in whatever session started it, and that session may not be reading its
 * output — an unwatched terminal. Streaming tens of thousands of test lines
 * into an unread pipe fills it and blocks the gate mid-test while it holds
 * the machine slot, so every queued run waits until someone looks at that
 * session. A few lines can't fill a pipe.
 *
 * The push mode activates once Husky has wired it in via the `prepare`
 * script (i.e. after `bun install`).
 *
 * Bypass: git push --no-verify. Reserved for WIP Handoff Commits and
 * non-executable diffs (see CLAUDE.md).
 */

import { existsSync, realpathSync } from "node:fs";
import { join, resolve } from "node:path";
import { resolveDocsDir, resolvePublished } from "./check-node-types-doc";
import { acquireGateLock, DISABLE_ENV_VAR, MACHINE_LOCK_PATH, MACHINE_SLOT_WHAT, registerLockRelease } from "./gate-lock";
import { SCCACHE_CACHE_SIZE, sccacheServerUds } from "./gate-sccache";
import {
  createLogDir,
  GATE_INFRA_EXIT,
  killActiveStages,
  NODE_TYPES_CHECK_PUBLISHED_LABEL,
  nodeTypesCheckAt,
  runStage,
  TIERS,
  type StageSpec,
} from "./gate-stage";
import { TOOLS_DIR } from "./setup-rust-tooling";
import { formatPruneResult, freeGiB, freeSpaceRefusal, GATE_INCREMENTAL_MAX_AGE_MS, pruneIncremental } from "./gate-disk";

export type GateMode = "push" | "merge";

/** `--mode=merge` selects the full pre-merge gate; anything else is a push. */
export function parseMode(argv: string[]): GateMode {
  return argv.includes("--mode=merge") ? "merge" : "push";
}

const mode = parseMode(process.argv.slice(2));
const merge = mode === "merge";

const MINUTE = 60_000;

/** How long the merge gate waits for the machine slot before failing. */
const MACHINE_SLOT_WAIT_CAP_MS = 2 * 60 * MINUTE;

/** `path` with symlinks resolved — a worktree's `.tools` links to the primary's. */
function realpathOrSelf(path: string): string {
  try {
    return realpathSync(path);
  } catch {
    return resolve(path);
  }
}

// Gate builds compile workspace crates incrementally, like development builds
// (the dev profile's default). The gate once turned it off so sccache could
// cache workspace crates, back when every push compiled in its own worktree.
// Now only the persistent gate checkout compiles, where an unchanged crate is
// already fresh in target/ and a changed one never hits the cache — so that
// bought nothing, and cost the recompile after every rebase: rebuilding the
// test binaries after a real nodespace-core edit took 39-64s without
// incremental and 13-15s with it.

// Gate builds go through the repository's own sccache (scripts/setup-rust-
// tooling.ts), configured here so the gate doesn't depend on the checkout's
// generated `.cargo/config.toml`; these variables take precedence over it.
// It shares its cache directory in `.tools/` with development builds
// (devRustcWrapper, ./gate-sccache.ts) but not its server: sccache compiles
// in the server process, so each checkout's builds keep a server of their
// own, and this gate's private unix socket (sccacheServerUds) keeps it apart
// from every development server and from another OS user's gate.
const sccache = join(realpathOrSelf(TOOLS_DIR), "bin", "sccache");
if (existsSync(sccache)) {
  process.env.RUSTC_WRAPPER = sccache;
  process.env.CMAKE_C_COMPILER_LAUNCHER = sccache;
  process.env.CMAKE_CXX_COMPILER_LAUNCHER = sccache;
  process.env.SCCACHE_DIR = join(realpathOrSelf(TOOLS_DIR), "sccache-cache");
  process.env.SCCACHE_CACHE_SIZE = SCCACHE_CACHE_SIZE;
  process.env.SCCACHE_SERVER_UDS = sccacheServerUds(process.getuid?.() ?? 0);
}

// A disk too full or read-only for the stage logs is this machine's fault.
const logDir = (() => {
  try {
    return createLogDir(mode);
  } catch (err) {
    console.error(`\n✗ Could not create the stage log directory: ${err instanceof Error ? err.message : String(err)}\n`);
    return process.exit(GATE_INFRA_EXIT);
  }
})();

/** Set by the first failing stage; see run(). */
let failed = false;

/**
 * Runs a stage, and stops the gate if it fails. Concurrent stages are killed
 * first (killActiveStages), and a stage that fails only because of that kill
 * waits for the exit rather than reporting a second failure.
 */
async function run(stage: StageSpec) {
  // Once the gate is failing, a lane whose stage just finished starts nothing
  // new: the kill below only reaches stages already running, and one started
  // during its grace period would outlive the gate and the machine slot.
  if (failed) return new Promise<never>(() => {});
  let passed: boolean;
  // A stage that can't even start (e.g. its log can't be opened) is this
  // machine's fault, not the code's — decided by this stage alone, so a
  // sibling lane's failure can't relabel a real one.
  let couldNotRun = false;
  try {
    passed = await runStage(stage, logDir);
  } catch (err) {
    console.error(`\n${stage.label} could not run: ${err instanceof Error ? err.message : String(err)}`);
    couldNotRun = true;
    passed = false;
  }
  if (passed) return;
  if (failed) return new Promise<never>(() => {});
  failed = true;
  await killActiveStages();
  if (merge) {
    console.error(`\n✗ ${stage.label} failed — merge blocked.`);
    console.error("  Fix the failure, push, and re-run: bun run merge <PR#>\n");
  } else {
    console.error(`\n✗ ${stage.label} failed — push blocked.`);
    console.error("  Fix it (bun run quality:fix fixes most lint), or if this is a WIP Handoff Commit");
    console.error("  (see CLAUDE.md), bypass with: git push --no-verify\n");
  }
  process.exit(couldNotRun ? GATE_INFRA_EXIT : 1);
}

// A merge gate always takes the machine slot; refuse the opt-out before any work.
if (merge && process.env[DISABLE_ENV_VAR]) {
  console.error(`\n✗ ${DISABLE_ENV_VAR} is set; the merge gate always takes the machine slot. Unset it and re-run.\n`);
  process.exit(GATE_INFRA_EXIT);
}

console.log(
  merge
    ? "\n▶ Merge gate: full pyramid on the rebased PR."
    : "\n▶ Push check: lint only. Tests run once, before merge: bun run merge <PR#>"
);
console.log(`  stage logs: ${logDir}\n`);

// ── Lint (both modes) ──────────────────────────────────────────────────────
// A push's lint runs at low priority so it yields to a merge gate's tests.
await run({
  label: "quality:scripts:check (scripts/ lint + typecheck)",
  command: "bun run quality:scripts:check",
  timeoutMs: 10 * MINUTE,
  nice: !merge,
});
// The design-token gate (Stylelint over CSS and Svelte <style> blocks). It is
// wired into the desktop-app quality scripts, but nothing automated runs those
// and this repo has no CI, so without this line the gate depends on someone
// remembering to run it — documentation rather than enforcement.
await run({
  label: "quality:design-tokens (design-token drift)",
  command: "bun run --cwd packages/desktop-app quality:design-tokens",
  timeoutMs: 5 * MINUTE,
  nice: !merge,
});
// App-version drift between tauri.conf.json and its siblings. A push is the
// only check `bun run release`'s version-bump commit gets on its way to main,
// so this runs at push time, not only in the merge gate.
await run({
  label: "check-version-sync (app version drift)",
  command: "bun run scripts/check-version-sync.ts",
  timeoutMs: 5 * MINUTE,
  nice: !merge,
});
// The code-boundary hard ban (ADR-081 section 8): fails on any marker hit outside
// the allowlist. test:changed skips the scripts tier for frontend-only and .md-only
// diffs, which is where most out-of-place code lands, so a push is the first
// automated check such a change gets. In merge mode this runs before the machine
// slot, so a hit fails in seconds instead of minutes into a gate run.
await run({
  label: "check-pro-boundary (ADR-081 code-boundary ratchet)",
  command: "bun run scripts/check-pro-boundary.ts",
  timeoutMs: 5 * MINUTE,
  nice: !merge,
});
// Literal node-type comparisons (ADR-086 section 5): a `node_type == "task"` is
// false for every type that extends task, so each one is a rule a subtype falls
// out of. Like the boundary check above, most such lines land in diffs that
// test:changed routes around the scripts tier, so the push is where they are
// first seen.
await run({
  label: "check-node-type-literals (ADR-086 chain-aware type rules)",
  command: "bun run scripts/check-node-type-literals.ts",
  timeoutMs: 5 * MINUTE,
  nice: !merge,
});
// lifecycle_status reads (ADR-087 section 2): the field is read through the
// governance module's participation check and nowhere else, so a comparison in
// a diff is a surface applying its own variant of the rule. Run on the push for
// the same reason as the two checks above.
await run({
  label: "check-lifecycle-reads (ADR-087 one participation check)",
  command: "bun run scripts/check-lifecycle-reads.ts",
  timeoutMs: 5 * MINUTE,
  nice: !merge,
});

if (!merge) {
  console.log("\n✓ Push check passed — pushing.\n");
  process.exit(0);
}

// ── Merge gate ─────────────────────────────────────────────────────────────

// A missing nextest otherwise surfaces as a bare "command not found".
if (!existsSync(join(TOOLS_DIR, "bin", "cargo-nextest"))) {
  console.error(`\n✗ ${TOOLS_DIR}/bin/cargo-nextest is missing — run \`bun install\`, which installs it (scripts/setup-rust-tooling.ts).\n`);
  process.exit(GATE_INFRA_EXIT);
}

// The reference the node-types.md check compares with: the docs remote's main,
// resolved to a commit here, once, so a sheet pushed while this gate runs
// doesn't change what it is judged against. A docs checkout that can't give
// that commit is this machine's fault; a machine with no docs checkout skips
// the stage, said here because a skipped stage would otherwise show as a pass.
const publishedSheet = resolvePublished(await resolveDocsDir());
if ("unreadable" in publishedSheet) {
  console.error(`\n✗ The node-types.md check can't read the published reference: ${publishedSheet.unreadable}\n`);
  process.exit(GATE_INFRA_EXIT);
}
if ("skip" in publishedSheet) console.warn(`  ⚠ ${NODE_TYPES_CHECK_PUBLISHED_LABEL} SKIPPED: ${publishedSheet.skip}`);
else if (publishedSheet.fetchFailed) {
  console.warn("  ⚠ node-types:check: the docs fetch failed; comparing with the published reference as last fetched.");
}

// Held until this process exits: registerLockRelease() covers Ctrl-C and every
// early exit, including the process.exit(1) inside run(). A merge's ticket
// queues ahead of every test:changed run's. The lock's usual degrade-and-run
// is refused here (as is the no-lock opt-out, at the top): a merge gate
// sharing the machine is exactly what the slot exists to prevent. The cap sits
// above the longest legitimate hold ahead of it (a test:changed Rust tier's
// 60-minute timeout).
const machineSlot = await acquireGateLock({
  lockPath: MACHINE_LOCK_PATH,
  what: MACHINE_SLOT_WHAT,
  urgent: true,
  shared: true,
  maxWaitMs: MACHINE_SLOT_WAIT_CAP_MS,
});
if (!machineSlot.held) {
  console.error("\n✗ Could not take the machine slot (see above), so this gate would share the machine. Re-run when it is free.\n");
  process.exit(GATE_INFRA_EXIT);
}
registerLockRelease(machineSlot);

// Under the machine slot, so nothing else is compiling into this target/:
// drop the incremental directories of stacks this checkout no longer builds
// (gate-disk.ts), then check what is left. Every worktree compiles into its
// own target/, and the gate can need several gigabytes more. Running out
// halfway surfaces as a confusing I/O failure in whichever stage hit it, so
// it is checked before the first compile, with the cause named.
console.log(formatPruneResult(pruneIncremental(join(process.cwd(), "target"), GATE_INCREMENTAL_MAX_AGE_MS), GATE_INCREMENTAL_MAX_AGE_MS));
const refusal = freeSpaceRefusal(freeGiB("."), "the merge gate");
if (refusal !== null) {
  console.error(refusal);
  process.exit(GATE_INFRA_EXIT);
}

const daemonBinary = `${process.cwd()}/target/debug/${process.platform === "win32" ? "nodespaced.exe" : "nodespaced"}`;

await run(TIERS.skillInstaller);

// Two lanes at once: the Rust builds, and the tiers that need no Rust build.
// One after the other, the frontend, scripts and skill tiers (~90s) waited on
// a compile they never read. Their tests carry no wall-clock assertions
// (those run at release time), so sharing the cores slows them, not breaks
// them. The skill installer above goes first because the skill tier and the
// Rust tests both read its output.
await Promise.all([
  (async () => {
    // First in this lane: it builds one small crate, so a Rust wire-type change
    // without its regenerated TypeScript fails here in seconds. In this lane
    // because cargo allows one build in a target directory at a time.
    await run(TIERS.typesCheck);
    // Before the test binaries: clippy only checks, so a lint error fails here
    // without waiting for the full compile. In this lane because it builds in
    // the same target directory, and so under the machine slot.
    await run(TIERS.rustLint);
    await run({ label: "compile Rust test binaries", command: "bun run rust:test:build", timeoutMs: 60 * MINUTE });
    await run({
      label: "compile nodespaced and the Tauri-seam test binary",
      command: `cargo build --bin nodespaced && cargo test -p nodespace-app-lib --test it --no-run`,
      timeoutMs: 45 * MINUTE,
    });
    // SKILL.md drift check (generated sections vs. the CLI definitions). After
    // the daemon build so its `cargo run --example` reuses that dev-profile
    // dependency tree.
    await run({ label: "skill:check (SKILL.md drift)", command: "bun run skill:check", timeoutMs: 20 * MINUTE });
    // After the daemon build for the same reason: its example links the
    // nodespace-core that build already compiled.
    if ("commit" in publishedSheet) await run(nodeTypesCheckAt(publishedSheet.commit, publishedSheet.fetchFailed));
  })(),
  (async () => {
    await run(TIERS.frontend);
    await run(TIERS.scripts);
    await run(TIERS.skill);
  })(),
]);

await run(TIERS.rust);
await run(TIERS.browser);
await run({
  label: "test:e2e (headless daemon round-trip)",
  command: "bun run test:e2e",
  timeoutMs: 10 * MINUTE,
  env: { NODESPACED_BINARY: daemonBinary },
});
// --test it: the crate's integration-test binary (tests/it/), and only it. This
// crate's `src/` unit tests are in-process, need no daemon binary, and run
// headless in ~2s at full parallelism, so `rust:test` (above) runs them
// alongside every other crate's. Narrowing this step is what leaves them free
// to do that — both a bare `cargo test -p nodespace-app-lib` and `--tests` would
// additionally re-run the lib unittest target here, needlessly, under the
// =1 cap only this suite requires. (`--tests` means "every target with
// test = true", not "the tests/ directory".)
//
// --test-threads=1: every test in this suite spawns a real nodespaced
// process, which loads a real embedding model (Metal shader compilation
// included) before its socket binds — far more load-sensitive than
// in-process assertions. All of them are in one binary (tests/it/), and
// within a binary every test runs concurrently by default. `SpawnedDaemon::spawn()`
// happens BEFORE any test acquires test-support's CONNECT_MUTEX (which only
// serializes the health-wait/connect step, not the spawn itself), so without
// this flag several real daemon processes can be mid-spawn at once fully
// uncoordinated — self-inflicted CPU/GPU contention this suite creates on its
// own, made worse by whatever else is running on the machine. Tried
// --test-threads=2 first; it still reproduced the same daemon-health timeout
// under real background load, so =1 was needed, not just a higher timeout.
// Serializing daemon spawns costs almost nothing here — the suite's total
// wall-clock is dominated by the one real-inference test (~25-40s), and the
// rest are sub-second each — so =1 trades no meaningful time for real
// reliability.
await run({
  label: "Tauri-seam integration tests (ADR-048)",
  command: `cargo test -p nodespace-app-lib --test it -- --test-threads=1`,
  timeoutMs: 15 * MINUTE,
  env: { NODESPACED_TEST_BIN: daemonBinary },
});

console.log("\n✓ Merge gate passed.\n");
