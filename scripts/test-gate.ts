#!/usr/bin/env bun

/**
 * The local gate, in two modes (ADR-047). This repo has no CI runner for
 * tests; this script is the only gate.
 *
 * - `push` (the default, run by the Husky pre-push hook): lint and the
 *   app-version drift check only. Seconds,
 *   and it takes no lock. A push only publishes a branch; the merge is what
 *   changes main, and the merge gate tests it. Test the tiers a change
 *   reaches while developing, with `bun run test:changed`.
 * - `merge` (`--mode=merge`, run by `bun run merge <PR#>`): lint, then the
 *   full pyramid — every unit tier, the daemon build, the SKILL.md drift
 *   check, e2e and the Tauri-seam tests — unscoped, on the PR rebased onto
 *   current main. The only automated test run a change gets.
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
import { $ } from "bun";
import { acquireGateLock, DISABLE_ENV_VAR, MACHINE_LOCK_PATH, MACHINE_SLOT_WHAT, registerLockRelease } from "./gate-lock";
import { SCCACHE_CACHE_SIZE, sccacheServerUds } from "./gate-sccache";
import { createLogDir, killActiveStages, runStage, TIERS, type StageSpec } from "./gate-stage";
import { TOOLS_DIR } from "./setup-rust-tooling";
import { freeGiBFromDf } from "./gate-output";

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

/** Below this much free disk the merge gate refuses to start. */
const MIN_FREE_GIB = 20;

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

const logDir = createLogDir(mode);

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
  try {
    passed = await runStage(stage, logDir);
  } catch (err) {
    // A stage that can't even start (e.g. its log can't be opened) fails the
    // gate like any other, so the other lane is still stopped.
    console.error(`\n${stage.label} could not run: ${err instanceof Error ? err.message : String(err)}`);
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
  process.exit(1);
}

// A merge gate always takes the machine slot; refuse the opt-out before any work.
if (merge && process.env[DISABLE_ENV_VAR]) {
  console.error(`\n✗ ${DISABLE_ENV_VAR} is set; the merge gate always takes the machine slot. Unset it and re-run.\n`);
  process.exit(1);
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

if (!merge) {
  console.log("\n✓ Push check passed — pushing.\n");
  process.exit(0);
}

// ── Merge gate ─────────────────────────────────────────────────────────────

// Every worktree compiles into its own target/, and the gate can need several
// gigabytes more. Running out halfway surfaces as a confusing I/O failure in
// whichever stage hit it. Checked up front instead, with the cause named.
const free = freeGiBFromDf(await $`df -Pk .`.quiet().nothrow().text());
if (free !== null && free < MIN_FREE_GIB) {
  console.error(
    `\n✗ Only ${free.toFixed(1)} GiB free on this disk; the merge gate needs at least ${MIN_FREE_GIB}.\n` +
      "  Each worktree's target/ holds its own build output. Free space by removing finished\n" +
      "  worktrees, or with `cargo clean` in worktrees that aren't building, then re-run.\n"
  );
  process.exit(1);
}
// A missing nextest otherwise surfaces as a bare "command not found".
if (!existsSync(join(TOOLS_DIR, "bin", "cargo-nextest"))) {
  console.error(`\n✗ ${TOOLS_DIR}/bin/cargo-nextest is missing — run \`bun install\`, which installs it (scripts/setup-rust-tooling.ts).\n`);
  process.exit(1);
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
  process.exit(1);
}
registerLockRelease(machineSlot);

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
    await run({ label: "compile Rust test binaries", command: "bun run rust:test:build", timeoutMs: 60 * MINUTE });
    await run({
      label: "compile nodespaced and the Tauri-seam test binary",
      command: `cargo build --bin nodespaced && cargo test -p nodespace-app --test it --no-run`,
      timeoutMs: 45 * MINUTE,
    });
    // SKILL.md drift check (generated sections vs. the CLI definitions). After
    // the daemon build so its `cargo run --example` reuses that dev-profile
    // dependency tree.
    await run({ label: "skill:check (SKILL.md drift)", command: "bun run skill:check", timeoutMs: 20 * MINUTE });
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
// to do that — both a bare `cargo test -p nodespace-app` and `--tests` would
// additionally re-run the lib/bin unittest targets here, needlessly, under the
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
  command: `cargo test -p nodespace-app --test it -- --test-threads=1`,
  timeoutMs: 15 * MINUTE,
  env: { NODESPACED_TEST_BIN: daemonBinary },
});

console.log("\n✓ Merge gate passed.\n");
