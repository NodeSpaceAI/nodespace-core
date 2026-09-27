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
  // Compiles the skill installer script: a nodespace-app unit test asserts the
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
  const found: number[] = [];
  const pending = [root];
  while (pending.length > 0) {
    for (const child of children.get(pending.pop() as number) ?? []) {
      if (found.includes(child)) continue;
      found.push(child);
      pending.push(child);
    }
  }
  return found;
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

/**
 * SIGTERM for the stage's whole tree, then SIGKILL for whatever is left after
 * a grace period. The tree is listed before anything is signalled: a child
 * whose parent dies is re-parented to init, and a listing taken afterwards
 * would no longer find it.
 */
async function killTree(root: number, exited: Promise<unknown>): Promise<void> {
  const tree = new Set(treeOf(root));
  signalAll(tree, "SIGTERM");
  await Promise.race([exited, Bun.sleep(KILL_GRACE_MS)]);
  for (const pid of treeOf(root)) tree.add(pid);
  signalAll(tree, "SIGKILL");
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
    const timer = setTimeout(() => {
      killing = killTree(proc.pid, proc.exited);
    }, stage.timeoutMs);
    exitCode = await proc.exited;
    clearTimeout(timer);
    // The caller may exit as soon as this returns, which would cut the
    // SIGKILL pass short.
    if (killing) await killing;
    signalCode = proc.signalCode;
  } finally {
    closeSync(fd);
  }
  const timedOut = killing !== null;
  if (exitCode === 0 && !timedOut) {
    console.log(`  ✓ ${((Date.now() - started) / 1000).toFixed(0)}s`);
    return true;
  }

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
