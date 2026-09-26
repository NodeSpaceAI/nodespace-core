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
 *   Tauri-seam tests — unscoped, on the PR rebased onto current main.
 *
 * Two phases. Everything that isn't timing-sensitive — lint, the staleness
 * check, and all compilation — runs first, without the machine-wide test
 * lock and at low CPU priority. Only then does the gate queue for the lock,
 * and it holds it just while tests execute. The lock exists because
 * concurrent test runs starve each other into timeouts on correct code; a
 * compile only gets slower. Holding the lock through a cold Rust build made
 * every other session on the machine wait through it.
 *
 * Output: each stage's full output goes to a log file, and the gate prints
 * one line per stage (plus the log's tail when a stage fails). The gate runs
 * under `git push` in whatever session pushed, and that session may not be
 * reading its output — an unwatched terminal. Streaming tens of thousands of
 * test lines into an unread pipe fills it and blocks the gate mid-test while
 * it holds the lock, so every queued gate waits until someone looks at that
 * session. A few lines can't fill a pipe.
 *
 * The push mode activates once Husky has wired it in via the `prepare`
 * script (i.e. after `bun install`).
 *
 * Bypass: git push --no-verify. Reserved for WIP Handoff Commits (see
 * CLAUDE.md) — multi-session work, approaching context limits, a natural
 * breakpoint, or before a risky change. Not a general-purpose escape hatch
 * for "the suite is slow right now."
 */

import { closeSync, mkdirSync, openSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { $ } from "bun";
import { reportBranchBehind } from "./check-branch-behind";
import { classifyFailure, formatAbortNote } from "./classify-test-failure";
import { reportUpstreamFixes } from "./correlate-upstream-fixes";
import { acquireGateLock, registerLockRelease } from "./gate-lock";
import { describeScope, FULL_SCOPE, gateScope } from "./gate-scope";
import { freeGiBFromDf, stageLogName, tail } from "./gate-output";

export type GateMode = "push" | "merge";

/** `--mode=merge` selects the full pre-merge gate; anything else is a push. */
export function parseMode(argv: string[]): GateMode {
  return argv.includes("--mode=merge") ? "merge" : "push";
}

const mode = parseMode(process.argv.slice(2));
const merge = mode === "merge";

/** Below this much free disk a gate that compiles refuses to start. */
const MIN_FREE_GIB = 20;

// No incremental compilation in gate builds. The incremental cache is most
// of a worktree's target/ (~15 GB), it is private to each worktree, and
// sccache can't cache incremental compiles — so without it sccache covers
// workspace crates too. The cost is recompiling a changed crate whole on a
// repeat push, rather than incrementally. Cargo counts the setting in its
// build fingerprint, so alternating a gate with an incremental `cargo test`
// or `tauri:dev` in the same worktree rebuilds workspace crates each switch.
process.env.CARGO_INCREMENTAL = "0";

const logDir = join(
  tmpdir(),
  "nodespace-gate-logs",
  `${basename(process.cwd())}-${new Date().toISOString().replace(/[:.]/g, "-")}`
);
mkdirSync(logDir, { recursive: true });

/**
 * Runs one stage as a shell command. stdout and stderr share one handle on
 * the stage's log file, so the log fills as the stage runs (`tail -f` it to
 * watch a slow one) and the two streams stay in the order they were written.
 * Nothing reaches this process's own output except the summary lines.
 */
async function run(label: string, command: string, env: Record<string, string> = {}) {
  const logPath = join(logDir, stageLogName(label));
  const started = Date.now();
  console.log(`▶ ${label}`);
  const fd = openSync(logPath, "w");
  let exitCode: number;
  try {
    const proc = Bun.spawn(["sh", "-c", command], {
      stdout: fd,
      stderr: fd,
      stdin: "ignore",
      env: { ...process.env, ...env },
    });
    exitCode = await proc.exited;
  } finally {
    closeSync(fd);
  }
  if (exitCode === 0) {
    console.log(`  ✓ ${((Date.now() - started) / 1000).toFixed(0)}s`);
    return;
  }

  const failureOutput = readFileSync(logPath, "utf8");
  console.error(`\n${tail(failureOutput, 40)}\n`);
  console.error(`  full output: ${logPath}`);
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
  console.error(`\n✗ ${label} failed — ${merge ? "merge blocked" : "push blocked"}.`);
  console.error("  Fix the failure, or if this is a WIP Handoff Commit (see CLAUDE.md),");
  console.error("  bypass with: git push --no-verify\n");
  process.exit(1);
}

// Staleness check, not a fix for the merge race — see check-branch-behind.ts.
// Never blocks: checkBranchBehind() already swallows every documented failure
// mode (fetch/rev-list) into a "skipped" result without throwing. This
// try/catch is defensive-only — a future edit that adds a throwing statement
// there must not be able to crash the push it's supposed to only warn about.
try {
  await reportBranchBehind();
} catch (err) {
  console.warn("\n⚠ origin/main staleness check crashed unexpectedly — skipping it.");
  console.warn(`  ${err instanceof Error ? err.message : String(err)}\n`);
}

// The merge gate never scopes: it is the one full run a change gets before
// it lands, and it runs on the rebased result, where untouched areas can
// still break.
const scope = merge ? FULL_SCOPE : await gateScope();
console.log(
  merge
    ? "\n▶ Merge gate: full pyramid on the rebased PR."
    : `\n▶ Push check: ${describeScope(scope)}\n  The full pyramid runs once, before merge: bun run merge <PR#>`
);
console.log(`  stage logs: ${logDir}\n`);

// Every worktree compiles into its own target/, and a compiling gate can
// need several gigabytes more. Running out halfway surfaces as a confusing
// I/O failure in whichever stage hit it — and fails every other queued gate
// the same way. Checked up front instead, with the cause named. A gate that
// compiles nothing (docs, lint-only) doesn't need the headroom.
if (merge || scope.rust) {
  const free = freeGiBFromDf(await $`df -Pk .`.quiet().nothrow().text());
  if (free !== null && free < MIN_FREE_GIB) {
    console.error(
      `\n✗ Only ${free.toFixed(1)} GiB free on this disk; a gate that compiles needs at least ${MIN_FREE_GIB}.\n` +
        "  Each worktree's target/ holds its own build output. Free space by removing finished\n" +
        "  worktrees, or with `cargo clean` in worktrees that aren't building, then push again.\n"
    );
    process.exit(1);
  }
  // A missing nextest otherwise surfaces as cargo's bare "no such command".
  if ((await $`cargo nextest --version`.quiet().nothrow()).exitCode !== 0) {
    console.error("\n✗ cargo-nextest is not installed — run `bun install`, which installs it (scripts/setup-rust-tooling.ts).\n");
    process.exit(1);
  }
}

/** Runs a stage when this push can affect it, and says so when it can't. */
async function stage(enabled: boolean, label: string, command: string, env: Record<string, string> = {}) {
  if (!enabled) {
    console.log(`⏭ ${label} — skipped (${merge ? "not reached" : "runs in the merge gate, or nothing in this push reaches it"})`);
    return;
  }
  await run(label, command, env);
}

const daemonBinary = `${process.cwd()}/target/debug/${process.platform === "win32" ? "nodespaced.exe" : "nodespaced"}`;

// ── Phase 1: no lock, low priority ─────────────────────────────────────────
// Lint and compilation. `nice` keeps a compile here from slowing whichever
// gate is running tests under the lock right now.

// Compiles the skill installer script: a nodespace-app unit test asserts the
// source checkout's `packages/skill/dist/install.js` exists, and the CLI's MCP
// integration test skips itself without it. That is just the skill package's
// `tsc` build — under a second. The rest of `build:skill` (staging the bundle,
// compiling the standalone installer) is release packaging; no build or test
// here reads it, and build.rs leaves anything unstaged out of a debug build.
await stage(scope.rust || scope.skill, "skill installer script (tsc)", "nice -n 10 bun run --cwd packages/skill build");
await run("quality:scripts:check (scripts/ lint + typecheck)", "nice -n 10 bun run quality:scripts:check");
// The design-token gate (Stylelint over CSS and Svelte <style> blocks). It is
// wired into the desktop-app quality scripts, but nothing automated runs those
// and this repo has no CI, so without this line the gate depends on someone
// remembering to run it — documentation rather than enforcement.
await run(
  "quality:design-tokens (design-token drift)",
  "nice -n 10 bun run --cwd packages/desktop-app quality:design-tokens"
);
await stage(scope.rust, "compile Rust test binaries", "nice -n 10 bun run rust:test:build");
// "*" is quoted so the shell hands cargo the pattern, not a list of filenames.
await stage(
  merge,
  "compile nodespaced and the Tauri-seam test binaries",
  `nice -n 10 cargo build --bin nodespaced && nice -n 10 cargo test -p nodespace-app --test "*" --no-run`
);
// SKILL.md drift check (generated sections vs. the CLI definitions). A compile
// and a text comparison, nothing timing-sensitive. After the daemon build so
// its `cargo run --example` reuses that dev-profile dependency tree.
await stage(merge, "skill:check (SKILL.md drift)", "nice -n 10 bun run skill:check");

// ── Phase 2: under the test lock ───────────────────────────────────────────
// Serialize against other gates on this machine only now, for the stages
// whose timing is what concurrent gates break. registerLockRelease() covers
// Ctrl-C and every early exit, including the process.exit(1) inside run().
// A merge gate's tests queue ahead of push checks' (see ticketName).
const testLock = await acquireGateLock({ urgent: merge });
registerLockRelease(testLock);

await stage(scope.frontend, "test (frontend, Happy-DOM)", "bun run test");
await stage(scope.scripts, "test:scripts (tooling)", "bun run test:scripts");
await stage(scope.skill, "test:skill (skill package)", "bun run test:skill");
await stage(scope.rust, "rust:test (Rust workspace, nextest)", "bun run rust:test");
// The browser tier: real focus/blur, drag-and-drop and layout that Happy-DOM
// can't model. About five seconds. The install is a no-op once Chromium is
// present and fetches it once on a fresh machine.
await stage(
  scope.frontend,
  "test:browser (Chromium)",
  "bun run --cwd packages/desktop-app playwright install chromium && bun run --cwd packages/desktop-app test:browser"
);
await stage(merge, "test:e2e (headless daemon round-trip)", "bun run test:e2e", { NODESPACED_BINARY: daemonBinary });
// --test "*": the `tests/*.rs` integration targets, and only those. This
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
// in-process assertions. Cargo already runs each tests/*.rs file's binary
// sequentially, but within one binary (e.g. node_crud_tauri_seam_test.rs's
// 5 tests) all tests run concurrently by default. `SpawnedDaemon::spawn()`
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
await stage(
  merge,
  "Tauri-seam integration tests (ADR-048)",
  `cargo test -p nodespace-app --test "*" -- --test-threads=1`,
  { NODESPACED_TEST_BIN: daemonBinary }
);

console.log(merge ? "\n✓ Merge gate passed.\n" : "\n✓ Push check passed — pushing.\n");
