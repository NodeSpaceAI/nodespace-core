#!/usr/bin/env bun
// Runs the stages of the merge gate (scripts/test-gate.ts) and of
// `bun run test:changed` (scripts/test-changed.ts), and defines the test tiers
// both run.
//
// Each stage's full output goes to a log file, and only a line per stage
// reaches the caller's output (plus the log's tail on failure). See
// test-gate.ts for why a gate must never stream test output into the terminal
// of the session that started it.
//
// Every stage has a timeout. A hung stage used to hold the machine's lock
// until someone killed it by hand, stalling every gate queued behind it. A
// timeout is sized well above a loaded-machine run (a loaded run takes about
// twice as long as a quiet one), so it catches hangs, not slowness. On expiry
// the stage's whole process tree is killed, not just the shell the stage runs
// in: a hung vitest or nextest worker would otherwise outlive the gate and
// keep the CPU busy.

import { closeSync, mkdirSync, openSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { classifyFailure, formatAbortNote } from "./classify-test-failure";
import { reportUpstreamFixes } from "./correlate-upstream-fixes";
import { exitStatusLine, stageLogName, tail } from "./gate-output";

const MINUTE = 60_000;

/**
 * The merge gate's exit code when this machine couldn't run it at all — too
 * little disk, no machine slot, missing tools, a stage that couldn't start —
 * as opposed to 1, the code under test failing. The merge queue must never
 * blame a PR for its machine: on this code it ejects nobody and backs off.
 */
export const GATE_INFRA_EXIT = 3;

/** How long a stage gets after SIGTERM to exit before its tree is SIGKILLed. */
const KILL_GRACE_MS = 10_000;

export interface StageSpec {
  label: string;
  command: string;
  /** Kills the stage after this long — sized to catch hangs, not slowness. */
  timeoutMs: number;
  env?: Record<string, string>;
  /** Run at low CPU priority, yielding to a merge gate on the same machine. */
  nice?: boolean;
}

/**
 * The test tiers a change can reach (see gate-scope.ts). The merge gate runs
 * them all; test:changed runs the ones the working diff reaches.
 */
export const TIERS = {
  // Compiles the skill installer script: a nodespace-app-lib unit test asserts the
  // source checkout's `packages/skill/dist/install.js` exists, and the CLI's MCP
  // integration test skips itself without it. That is just the skill package's
  // `tsc` build — under a second. The rest of `build:skill` (staging the bundle,
  // compiling the standalone installer) is release packaging; no build or test
  // here reads it, and build.rs leaves anything unstaged out of a debug build.
  skillInstaller: {
    label: "skill installer script (tsc)",
    command: "bun run --cwd packages/skill build",
    timeoutMs: 5 * MINUTE,
  },
  frontend: { label: "test (frontend, Happy-DOM)", command: "bun run test", timeoutMs: 15 * MINUTE },
  scripts: { label: "test:scripts (tooling)", command: "bun run test:scripts", timeoutMs: 10 * MINUTE },
  skill: { label: "test:skill (skill package)", command: "bun run test:skill", timeoutMs: 5 * MINUTE },
  rust: { label: "rust:test (Rust workspace, nextest)", command: "bun run rust:test", timeoutMs: 20 * MINUTE },
  // Regenerates the frontend's wire types from nodespace-types and fails when
  // the committed files differ (ADR-086 section 8). Part of the Rust tier: it
  // compiles that crate with its `ts` feature.
  typesCheck: {
    label: "types:check (generated TypeScript drift)",
    command: "bun run types:check",
    timeoutMs: 15 * MINUTE,
  },
  // Compares the per-type reference in the docs repository with the registry
  // and the seeded core schemas (ADR-086). Part of the Rust tier: it runs a
  // nodespace-core example. This is the working-tree form test:changed runs;
  // the merge gate compares with a commit of the docs remote's main
  // (nodeTypesCheckAt). A machine with no docs checkout can't run either:
  // the caller finds that out first and says so on its own output, since a
  // stage that skipped itself would show as a pass.
  nodeTypesCheck: {
    label: "node-types:check (node-types.md vs. the core schemas)",
    command: "bun run node-types:check",
    timeoutMs: 20 * MINUTE,
  },
  // The browser tier: real focus/blur, drag-and-drop and layout that Happy-DOM
  // can't model. About five seconds. The install is a no-op once Chromium is
  // present and fetches it once on a fresh machine.
  browser: {
    label: "test:browser (Chromium)",
    command:
      "bun run --cwd packages/desktop-app playwright install chromium && bun run --cwd packages/desktop-app test:browser",
    timeoutMs: 10 * MINUTE,
  },
} satisfies Record<string, StageSpec>;

export const NODE_TYPES_CHECK_PUBLISHED_LABEL = "node-types:check (published node-types.md vs. the core schemas)";

/**
 * The merge gate's node-types.md check, against the reference at `commit` of
 * the docs checkout. `fetchFailed` says the commit is the published main only
 * as last fetched; the label carries it, because the gate repeats a failing
 * stage's label where the reason for an ejection is read.
 */
export function nodeTypesCheckAt(commit: string, fetchFailed: boolean): StageSpec {
  return {
    label: fetchFailed
      ? `${NODE_TYPES_CHECK_PUBLISHED_LABEL} [the docs fetch failed: compared as last fetched]`
      : NODE_TYPES_CHECK_PUBLISHED_LABEL,
    command: `bun run node-types:check --at ${commit}`,
    timeoutMs: 20 * MINUTE,
  };
}

/** A fresh directory for one run's stage logs, named for the worktree and `kind`. */
export function createLogDir(kind: string): string {
  const dir = join(
    tmpdir(),
    "nodespace-gate-logs",
    `${basename(process.cwd())}-${kind}-${new Date().toISOString().replace(/[:.]/g, "-")}`
  );
  mkdirSync(dir, { recursive: true });
  return dir;
}

export function formatMinutes(ms: number): string {
  return `${Math.round(ms / MINUTE)}m`;
}

/**
 * Every descendant of `root` in a `ps -A -o pid=,ppid=` listing. Pure, for
 * testing.
 */
export function descendantPids(psOutput: string, root: number): number[] {
  const children = new Map<number, number[]>();
  for (const line of psOutput.split("\n")) {
    const [pid, ppid] = line.trim().split(/\s+/).map(Number);
    if (!Number.isInteger(pid) || !Number.isInteger(ppid)) continue;
    children.set(ppid, [...(children.get(ppid) ?? []), pid]);
  }
  const found = new Set<number>();
  const pending = [root];
  while (pending.length > 0) {
    for (const child of children.get(pending.pop() as number) ?? []) {
      if (found.has(child)) continue;
      found.add(child);
      pending.push(child);
    }
  }
  return [...found];
}

function treeOf(root: number): number[] {
  const ps = Bun.spawnSync(["ps", "-A", "-o", "pid=,ppid="]);
  return [root, ...descendantPids(ps.stdout.toString(), root)];
}

function signalAll(pids: Iterable<number>, signal: "SIGTERM" | "SIGKILL"): void {
  for (const pid of pids) {
    try {
      process.kill(pid, signal);
    } catch {
      // Already exited.
    }
  }
}

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

/**
 * SIGTERM for the stage's whole tree, then SIGKILL for whatever is still
 * alive after a grace period — waiting on every process in the tree, not just
 * the root, which usually dies first. The tree is listed before anything is
 * signalled: a child whose parent dies is re-parented to init, and a listing
 * taken afterwards would no longer find it.
 */
async function killTree(root: number): Promise<void> {
  const tree = new Set(treeOf(root));
  signalAll(tree, "SIGTERM");
  const deadline = Date.now() + KILL_GRACE_MS;
  while (Date.now() < deadline && [...tree].some(isAlive)) await Bun.sleep(100);
  for (const pid of treeOf(root)) tree.add(pid);
  signalAll([...tree].filter(isAlive), "SIGKILL");
}

/** The root pid of every stage still running — the gate runs some concurrently. */
const activeStages = new Set<number>();

/** Set once the gate is stopping; a stage killed by it fails silently. */
let aborting = false;

/**
 * Kills every stage still running, with its process tree. For a gate whose
 * concurrent stage just failed: it is about to exit, and a sibling stage left
 * running would outlive it, holding the CPU with nothing to report to. The
 * killed stages return false without printing, so the failure that stopped
 * the gate is the only one reported.
 */
export async function killActiveStages(): Promise<void> {
  aborting = true;
  await Promise.all([...activeStages].map(killTree));
}

/**
 * Runs one stage as a shell command, output to its log in `logDir`. Returns
 * whether it passed; on failure, has already printed why. stdout and stderr
 * share one handle on the log file, so it fills as the stage runs (`tail -f`
 * it to watch a slow one) and the two streams stay in the order written.
 */
export async function runStage(stage: StageSpec, logDir: string): Promise<boolean> {
  const logPath = join(logDir, stageLogName(stage.label));
  const started = Date.now();
  console.log(`▶ ${stage.label} (timeout ${formatMinutes(stage.timeoutMs)})`);
  const fd = openSync(logPath, "w");
  let exitCode: number;
  let signalCode: string | null;
  let killing: Promise<void> | null = null;
  try {
    // `nice` execs its command, so the pid stays the root of the stage's tree.
    const argv = ["sh", "-c", stage.command];
    const proc = Bun.spawn(stage.nice ? ["nice", "-n", "10", ...argv] : argv, {
      stdout: fd,
      stderr: fd,
      stdin: "ignore",
      env: { ...process.env, ...stage.env },
    });
    activeStages.add(proc.pid);
    const timer = setTimeout(() => {
      killing = killTree(proc.pid);
    }, stage.timeoutMs);
    exitCode = await proc.exited;
    clearTimeout(timer);
    activeStages.delete(proc.pid);
    // The caller may exit as soon as this returns, which would cut the
    // SIGKILL pass short.
    if (killing) await killing;
    signalCode = proc.signalCode;
  } finally {
    closeSync(fd);
  }
  const timedOut = killing !== null;
  if (exitCode === 0 && !timedOut) {
    // Named, because stages that run concurrently interleave their lines.
    console.log(`  ✓ ${stage.label} ${((Date.now() - started) / 1000).toFixed(0)}s`);
    return true;
  }
  if (aborting) return false;

  const status = timedOut
    ? `[stage timed out after ${formatMinutes(stage.timeoutMs)} and was killed with its process tree]`
    : exitStatusLine(exitCode, signalCode);
  const failureOutput = `${readFileSync(logPath, "utf8")}\n${status}`;
  console.error(`\n${tail(failureOutput, 40)}\n`);
  console.error(`  full output: ${logPath}`);
  if (timedOut) return false;
  // A load-induced process abort (e.g. a SIGSEGV under parallel-test
  // resource contention) and a genuine regression both surface here
  // identically otherwise — see classify-test-failure.ts. This does not
  // change the outcome; it only tells the person which kind of failure
  // they're looking at, so they don't burn a multi-minute rerun to find out.
  if (classifyFailure(failureOutput) === "abort") {
    console.error(formatAbortNote(stage.label));
  }
  // Same intent one step further: if a commit on origin/main already
  // touches the code that just failed, this failure may be stale code
  // rather than a live regression, and no amount of local debugging can
  // fix it. Advisory only. Its own errors are swallowed: the staleness
  // check is the last thing that should be able to obscure a real failure.
  try {
    await reportUpstreamFixes(failureOutput);
  } catch {
    // Intentionally silent — reporting must not mask the failure above.
  }
  return false;
}
