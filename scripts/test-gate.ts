#!/usr/bin/env bun

/**
 * Local pre-push test gate.
 *
 * Runs the full test pyramid (frontend, skill, Rust, e2e) before code leaves
 * the machine. This repo has no CI runner for tests — this hook is the only
 * gate. See ADR-047.
 *
 * This gate only activates once Husky has wired it in via the `prepare`
 * script (i.e. after `bun install`). It is a local convenience, not a
 * server-side enforcement backstop — a push from a machine that never ran
 * `bun install` is not gated.
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

// Compiles the skill installer script BEFORE `test:all`: a nodespace-app unit
// test asserts the source checkout's `packages/skill/dist/install.js` exists,
// and the CLI's MCP integration test skips itself without it. That is just
// the skill package's `tsc` build — under a second. The rest of
// `build:skill` (staging the bundle, compiling the standalone installer) is
// release packaging; no build or test here reads it, and build.rs leaves
// anything unstaged out of a debug build's bundle.
await run("bun run --cwd packages/skill build (skill installer script)", () => $`bun run --cwd packages/skill build`);
await run("bun run quality:scripts:check (scripts/ lint + typecheck)", () => $`bun run quality:scripts:check`);
// The design-token gate (Stylelint over CSS and Svelte <style> blocks). It is
// wired into the desktop-app quality scripts, but nothing automated runs those
// and this repo has no CI, so without this line the gate depends on someone
// remembering to run it — documentation rather than enforcement. Seconds to
// run, unlike the Rust steps below.
await run(
  "bun run quality:design-tokens (design-token drift)",
  () => $`bun run --cwd packages/desktop-app quality:design-tokens`
);
await run("bun run test:all (frontend + skill + Rust)", () => $`bun run test:all`);
// The browser tier: real focus/blur, drag-and-drop and layout that Happy-DOM
// can't model. About five seconds, so there's no reason to leave it to chance.
// The install is a no-op once Chromium is present and fetches it once on a
// fresh machine.
await run("bun run test:browser (Chromium)", async () => {
  await $`bun run --cwd packages/desktop-app playwright install chromium`.quiet();
  await $`bun run --cwd packages/desktop-app test:browser`;
});
await run("cargo build --bin nodespaced (e2e harness daemon)", () => $`cargo build --bin nodespaced`);
// SKILL.md drift check (generated sections vs. the CLI definitions). Placed
// after the daemon build on purpose: its `cargo run --example` shares that
// dev-profile dependency tree, so it compiles only the CLI crate and the
// example. Run first, it paid for a cold dev-profile build on its own.
await run("bun run skill:check (SKILL.md drift)", () => $`bun run skill:check`);
await run("bun run test:e2e (headless daemon round-trip)", () => {
  const binaryName = process.platform === "win32" ? "nodespaced.exe" : "nodespaced";
  const binary = `${process.cwd()}/target/debug/${binaryName}`;
  return $`bun run test:e2e`.env({ ...process.env, NODESPACED_BINARY: binary });
});
await run(`cargo test -p nodespace-app --test "*" (Tauri-seam integration tests, ADR-048)`, () => {
  const binaryName = process.platform === "win32" ? "nodespaced.exe" : "nodespaced";
  const binary = `${process.cwd()}/target/debug/${binaryName}`;
  // --test "*": the `tests/*.rs` integration targets, and only those. This
  // crate's `src/` unit tests are in-process, need no daemon binary, and run
  // headless in ~2s at full parallelism, so `rust:test` (above, via test:all)
  // runs them alongside every other crate's. Narrowing this step is what
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
    NODESPACED_TEST_BIN: binary,
  });
});

console.log("\n✓ All tests passed — pushing.\n");
