#!/usr/bin/env bun

/**
 * The local test gate, in two modes (ADR-047). This repo has no CI runner for
 * tests; this script is the only gate.
 *
 * - `push` (the default, run by the Husky pre-push hook): lint, plus the unit
 *   tiers this push's changes can reach (see gate-scope.ts). Minutes at most,
 *   so WIP and review-fix pushes stay cheap.
 * - `merge` (`--mode=merge`, run by `bun run merge <PR#>`): the full pyramid —
 *   every unit tier, the daemon build, the SKILL.md drift check, e2e and the
 *   Tauri-seam tests — unscoped, on the PR rebased onto current main. What
 *   lands on main is what passed here; scripts/merge-pr.ts records that as a
 *   commit status GitHub can require.
 *
 * The push mode activates once Husky has wired it in via the `prepare`
 * script (i.e. after `bun install`).
 *
 * Bypass: git push --no-verify. Reserved for WIP Handoff Commits (see
 * CLAUDE.md) — multi-session work, approaching context limits, a natural
 * breakpoint, or before a risky change. Not a general-purpose escape hatch
 * for "the suite is slow right now."
 */

import { $ } from "bun";
import { reportBranchBehind } from "./check-branch-behind";
import { classifyFailure, extractFailureOutput, formatAbortNote } from "./classify-test-failure";
import { reportUpstreamFixes } from "./correlate-upstream-fixes";
import { acquireGateLock, registerLockRelease } from "./gate-lock";
import { describeScope, FULL_SCOPE, gateScope } from "./gate-scope";

export type GateMode = "push" | "merge";

/** `--mode=merge` selects the full pre-merge gate; anything else is a push. */
export function parseMode(argv: string[]): GateMode {
  return argv.includes("--mode=merge") ? "merge" : "push";
}

const mode = parseMode(process.argv.slice(2));

async function run(label: string, cmd: () => Promise<unknown>) {
  console.log(`\n▶ ${label}`);
  try {
    await cmd();
  } catch (err) {
    const failureOutput = extractFailureOutput(err);
    // A load-induced process abort (e.g. a SIGSEGV under parallel-test
    // resource contention) and a genuine regression both surface here
    // identically otherwise — see classify-test-failure.ts. This does not
    // change the outcome (the push is still blocked either way); it only
    // tells the person which kind of failure they're looking at, so they
    // don't burn a multi-minute rerun to find out, or reach for
    // --no-verify out of frustration with a flake that looked real.
    if (classifyFailure(failureOutput) === "abort") {
      console.error(formatAbortNote(label));
    }
    // Same intent one step further: if a commit on origin/main already
    // touches the code that just failed, this failure may be stale code
    // rather than a live regression, and no amount of local debugging can
    // fix it. Advisory only — it never changes whether the push is blocked.
    // Its own errors are swallowed: the staleness check is the last thing
    // that should be able to obscure a real test failure.
    try {
      await reportUpstreamFixes(failureOutput);
    } catch {
      // Intentionally silent — reporting must not mask the failure below.
    }
    console.error(`\n✗ ${label} failed — push blocked.`);
    console.error("  Fix the failure, or if this is a WIP Handoff Commit (see CLAUDE.md),");
    console.error("  bypass with: git push --no-verify\n");
    process.exit(1);
  }
}

// Serialize against other gates on this machine before doing any real work —
// see gate-lock.ts. Everything below parallelizes across all cores, and N
// concurrent gates starve each other into worker/daemon timeouts on correct
// code. Acquired first so a queued push says so immediately, rather than
// after the staleness check's network round-trip.
//
// registerLockRelease() covers Ctrl-C and every early exit, including the
// process.exit(1) inside run() above.
const gateLock = await acquireGateLock();
registerLockRelease(gateLock);

// Staleness check, not a fix for the merge race — see check-branch-behind.ts.
// Runs before the several-minutes pyramid below so its warning (if any) is
// visible early, and never blocks: checkBranchBehind() already swallows every
// documented failure mode (fetch/rev-list) into a "skipped" result without
// throwing. This try/catch is defensive-only, guarding the one call in this
// entry sequence that isn't wrapped by run() — a future edit to
// check-branch-behind.ts that adds a throwing statement outside those
// try/catches must not be able to crash the push it's supposed to only warn
// about.
try {
  await reportBranchBehind();
} catch (err) {
  console.warn("\n⚠ origin/main staleness check crashed unexpectedly — skipping it.");
  console.warn(`  ${err instanceof Error ? err.message : String(err)}\n`);
}

// The merge gate never scopes: it is the one full run a change gets before
// it lands, and it runs on the rebased result, where untouched areas can
// still break.
const scope = mode === "merge" ? FULL_SCOPE : await gateScope();
const merge = mode === "merge";
console.log(
  merge
    ? "\n▶ Merge gate: full pyramid on the rebased PR."
    : `\n▶ Push check: ${describeScope(scope)}\n  The full pyramid runs once, before merge: bun run merge <PR#>`
);

/** Runs a stage when this push can affect it, and says so when it can't. */
async function stage(enabled: boolean, label: string, cmd: () => Promise<unknown>) {
  if (!enabled) {
    console.log(`\n⏭ ${label} — skipped (${merge ? "not reached" : "runs in the merge gate, or nothing in this push reaches it"})`);
    return;
  }
  await run(label, cmd);
}

const daemonBinary = `${process.cwd()}/target/debug/${process.platform === "win32" ? "nodespaced.exe" : "nodespaced"}`;

// Compiles the skill installer script: a nodespace-app unit test asserts the
// source checkout's `packages/skill/dist/install.js` exists, and the CLI's MCP
// integration test skips itself without it. That is just the skill package's
// `tsc` build — under a second. The rest of `build:skill` (staging the bundle,
// compiling the standalone installer) is release packaging; no build or test
// here reads it, and build.rs leaves anything unstaged out of a debug build.
await stage(scope.rust || scope.skill, "bun run --cwd packages/skill build (skill installer script)", () =>
  $`bun run --cwd packages/skill build`
);
// Lint gates cost seconds, so they run on every push regardless of scope.
await run("bun run quality:scripts:check (scripts/ lint + typecheck)", () => $`bun run quality:scripts:check`);
// The design-token gate (Stylelint over CSS and Svelte <style> blocks). It is
// wired into the desktop-app quality scripts, but nothing automated runs those
// and this repo has no CI, so without this line the gate depends on someone
// remembering to run it — documentation rather than enforcement.
await run(
  "bun run quality:design-tokens (design-token drift)",
  () => $`bun run --cwd packages/desktop-app quality:design-tokens`
);
// `test:all`, taken apart so each tier runs only when this push reaches it.
await stage(scope.frontend, "bun run test (frontend, Happy-DOM)", () => $`bun run test`);
await stage(scope.scripts, "bun run test:scripts (tooling)", () => $`bun run test:scripts`);
await stage(scope.skill, "bun run test:skill (skill package)", () => $`bun run test:skill`);
await stage(scope.rust, "bun run rust:test (Rust workspace, nextest)", async () => {
  // A missing nextest otherwise surfaces as cargo's bare "no such command".
  const probe = await $`cargo nextest --version`.quiet().nothrow();
  if (probe.exitCode !== 0) {
    throw new Error("cargo-nextest is not installed — run `bun install`, which installs it (scripts/setup-rust-tooling.ts).");
  }
  await $`bun run rust:test`;
});
// The browser tier: real focus/blur, drag-and-drop and layout that Happy-DOM
// can't model. About five seconds. The install is a no-op once Chromium is
// present and fetches it once on a fresh machine.
await stage(scope.frontend, "bun run test:browser (Chromium)", async () => {
  await $`bun run --cwd packages/desktop-app playwright install chromium`.quiet();
  await $`bun run --cwd packages/desktop-app test:browser`;
});
// Everything below needs a built daemon or a full CLI compile, and runs only
// in the merge gate.
await stage(merge, "cargo build --bin nodespaced (e2e harness daemon)", () =>
  $`cargo build --bin nodespaced`
);
// SKILL.md drift check (generated sections vs. the CLI definitions). Placed
// after the daemon build on purpose: its `cargo run --example` shares that
// dev-profile dependency tree, so it compiles only the CLI crate and the
// example.
await stage(merge, "bun run skill:check (SKILL.md drift)", () => $`bun run skill:check`);
await stage(merge, "bun run test:e2e (headless daemon round-trip)", () =>
  $`bun run test:e2e`.env({ ...process.env, NODESPACED_BINARY: daemonBinary })
);
await stage(merge, `cargo test -p nodespace-app --test "*" (Tauri-seam integration tests, ADR-048)`, () => {
  // --test "*": the `tests/*.rs` integration targets, and only those. This
  // crate's `src/` unit tests are in-process, need no daemon binary, and run
  // headless in ~2s at full parallelism, so `rust:test` (above) runs them
  // alongside every other crate's. Narrowing this step is what
  // leaves them free to do that — both a bare `cargo test -p nodespace-app`
  // and `--tests` would additionally re-run the lib/bin unittest targets
  // here, needlessly, under the =1 cap only this suite requires. (`--tests`
  // means "every target with test = true", not "the tests/ directory".)
  //
  // --test-threads=1: every test in this suite spawns a real nodespaced
  // process, which loads a real embedding model (Metal shader compilation
  // included) before its socket binds — far more load-sensitive than
  // nodespace-core's in-process assertions (which cap concurrency the same
  // way, see rust:test above, though that suite has no equivalent per-test
  // process-spawn cost). Cargo already runs each tests/*.rs file's binary
  // sequentially, but within one binary (e.g. node_crud_tauri_seam_test.rs's
  // 5 tests) all tests run concurrently by default. `SpawnedDaemon::spawn()`
  // happens BEFORE any test acquires test-support's CONNECT_MUTEX (which
  // only serializes the health-wait/connect step, not the spawn itself), so
  // without this flag several real daemon processes can be mid-spawn at
  // once fully uncoordinated — self-inflicted CPU/GPU contention this suite
  // creates on its own, made worse by whatever else is running on the
  // machine. Tried --test-threads=2 first; it still reproduced the same
  // daemon-health timeout under real background load, so =1 was needed, not
  // just a higher timeout. Serializing daemon spawns costs almost nothing
  // here — the suite's total wall-clock is dominated by the one
  // real-inference test (~25-40s), and the rest are sub-second each — so =1
  // trades no meaningful time for real reliability.
  // "*" stays quoted: bun's $ glob-expands a bare * against the working
  // directory, which would hand cargo a list of repo filenames instead.
  return $`cargo test -p nodespace-app --test "*" -- --test-threads=1`.env({
    ...process.env,
    NODESPACED_TEST_BIN: daemonBinary,
  });
});

console.log(merge ? "\n✓ Merge gate passed.\n" : "\n✓ Push check passed — pushing.\n");
