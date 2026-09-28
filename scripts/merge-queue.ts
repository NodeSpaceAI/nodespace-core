#!/usr/bin/env bun
// The team-wide merge queue (ADR-047): one queue and one gate at a time
// across every machine, kept in git refs on origin — no service to run.
//
// Before it, each machine had its own merge lock, so a merge landing from one
// machine moved main under a gate running on another, which then rebased and
// ran the whole pyramid again — up to three times, then gave up. With ~10
// parallel worktrees across the team, most of that work was thrown away.
//
// - Queue: `refs/nodespace/merge-queue/<PR#>`, one ref per PR waiting to
//   land. `bun run merge` adds its PR and waits for it to land or be ejected.
// - Lock: `refs/nodespace/merge-lock`, which points at a commit whose message
//   says who holds it. Taken and released with `git push --force-with-lease`,
//   which the server applies atomically: of two machines pushing against the
//   same expected value, exactly one wins. The holder re-pushes it every
//   minute (the heartbeat). A waiter that watches it stay unchanged for
//   STALE_AFTER_MS — timed by the waiter's own clock, so skew between
//   machines can't matter — takes it over from a holder that died.
//
// main moves only through a queue landing, so a running gate is never
// invalidated by another merge. (A push straight to main — a release's
// version bump — is caught before landing; see merge-pr.ts.)

import { $ } from "bun";
import { randomUUID } from "node:crypto";
import { hostname } from "node:os";
import { currentUser } from "./gate-lock";
import { GATE_INFRA_EXIT } from "./gate-stage";

export const QUEUE_PREFIX = "refs/nodespace/merge-queue/";
export const LOCK_REF = "refs/nodespace/merge-lock";

/** Git's well-known empty tree, which every repository has without storing it. */
const EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/** Where a fetched lock commit is kept locally, to read who holds it. */
const SEEN_LOCK_REF = "refs/nodespace-seen/merge-lock";

/** How often the holder re-pushes the lock. */
export const HEARTBEAT_MS = 60 * 1000;

/**
 * A lock unchanged this long (as a waiter watched it) belongs to a holder
 * that died. Well past a heartbeat interval plus a slow push, so a live
 * holder on a flaky network isn't taken over.
 */
export const STALE_AFTER_MS = 10 * 60 * 1000;

/** Who holds the lock, as recorded in the lock commit's message. */
export interface LockInfo {
  host: string;
  user: string;
  pid: number;
  /** Epoch ms the holder took the lock. */
  startedAt: number;
  /** The PRs in the round it's running, for the waiting line. */
  prs: number[];
}

export function describeLock(info: LockInfo): string {
  const prs = info.prs.length > 0 ? ` on ${info.prs.map((p) => `#${p}`).join(", ")}` : "";
  return `${info.user}@${info.host} (pid ${info.pid})${prs}`;
}

/** Parses a lock commit's message; null for anything that isn't one. */
export function parseLockInfo(message: string): LockInfo | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(message);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object") return null;
  const { host, user, pid, startedAt, prs } = parsed as Record<string, unknown>;
  if (typeof host !== "string" || typeof user !== "string") return null;
  if (typeof pid !== "number" || typeof startedAt !== "number") return null;
  if (!Array.isArray(prs) || !prs.every((p) => typeof p === "number")) return null;
  return { host, user, pid, startedAt, prs };
}

/** This process's lock record. */
export function lockInfoHere(prs: number[] = []): LockInfo {
  return { host: hostname(), user: currentUser(), pid: process.pid, startedAt: Date.now(), prs };
}

/** The PR number a queue ref names, or null for anything else under the prefix. */
export function prFromQueueRef(ref: string): number | null {
  if (!ref.startsWith(QUEUE_PREFIX)) return null;
  const pr = Number(ref.slice(QUEUE_PREFIX.length));
  return Number.isInteger(pr) && pr > 0 ? pr : null;
}

/** Parses `git ls-remote` output into ref → sha. */
export function parseLsRemote(output: string): Map<string, string> {
  const refs = new Map<string, string>();
  for (const line of output.split("\n")) {
    const [sha, ref] = line.trim().split(/\s+/);
    if (sha && ref && /^[0-9a-f]{40}$/.test(sha)) refs.set(ref, sha);
  }
  return refs;
}

/**
 * The PRs to test together this round, after `failed` (a batch, in queue
 * order) failed its gate: its first half. Retesting a prefix, rather than
 * any subset, means whatever passes can land in queue order at once, and each
 * round either lands something or halves the suspects — so a batch of N costs
 * at most about log2(N) extra runs to find the PR that broke it.
 */
export function bisectBatch<T>(failed: T[]): T[] {
  return failed.slice(0, Math.max(1, Math.floor(failed.length / 2)));
}

export type RenewResult = { status: "renewed"; sha: string } | { status: "lost" } | { status: "unknown" };

/** The queue and lock on one remote, operated from the checkout at `cwd`. */
export class MergeQueue {
  constructor(
    private readonly cwd: string,
    private readonly remote = "origin"
  ) {}

  private git(args: string[]) {
    return $`git -c core.hooksPath=/dev/null ${args}`.cwd(this.cwd).quiet().nothrow();
  }

  private async lsRemote(pattern: string): Promise<Map<string, string>> {
    const out = await this.git(["ls-remote", this.remote, pattern]);
    if (out.exitCode !== 0) throw new Error(`git ls-remote failed: ${out.stderr.toString().trim()}`);
    return parseLsRemote(out.stdout.toString());
  }

  /**
   * Pushes `sha` to `ref` only if the remote's `ref` is `expected` right now
   * ("" = must not exist). The server checks and updates atomically, so this
   * is the compare-and-swap everything here is built on. False when the ref
   * had moved.
   */
  private async compareAndSwap(ref: string, expected: string, sha: string | null): Promise<boolean> {
    const target = sha === null ? `:${ref}` : `${sha}:${ref}`;
    const out = await this.git(["push", "--quiet", "--no-verify", `--force-with-lease=${ref}:${expected}`, this.remote, target]);
    return out.exitCode === 0;
  }

  /** The PRs waiting to land, oldest PR first. */
  async queued(): Promise<number[]> {
    const refs = await this.lsRemote(`${QUEUE_PREFIX}*`);
    return [...refs.keys()]
      .map(prFromQueueRef)
      .filter((pr): pr is number => pr !== null)
      .sort((a, b) => a - b);
  }

  async isQueued(pr: number): Promise<boolean> {
    return (await this.lsRemote(`${QUEUE_PREFIX}${pr}`)).has(`${QUEUE_PREFIX}${pr}`);
  }

  /**
   * Adds `pr` to the queue. The ref points at the head it was queued at — for
   * anyone inspecting the queue; a round reads the head afresh from origin.
   */
  async enqueue(pr: number, head: string): Promise<void> {
    const out = await this.git(["push", "--quiet", "--no-verify", "--force", this.remote, `${head}:${QUEUE_PREFIX}${pr}`]);
    if (out.exitCode !== 0) throw new Error(`could not queue #${pr}: ${out.stderr.toString().trim()}`);
  }

  /** Removes `pr` from the queue. Idempotent. */
  async dequeue(pr: number): Promise<void> {
    await this.git(["push", "--quiet", "--no-verify", this.remote, `:${QUEUE_PREFIX}${pr}`]);
  }

  /**
   * A commit holding `info` as its message, on the empty tree. `beat` makes
   * every one unique: commit-tree is deterministic, and a heartbeat within the
   * same second as the last would otherwise produce the same sha — no change
   * for a waiter to see, and a takeover that should have failed would succeed.
   */
  private async lockCommit(info: LockInfo): Promise<string> {
    const out = await $`git commit-tree ${EMPTY_TREE} -m ${JSON.stringify({ ...info, beat: randomUUID() })}`
      .cwd(this.cwd)
      .env({ ...process.env, GIT_AUTHOR_NAME: "merge-queue", GIT_AUTHOR_EMAIL: "merge-queue@nodespace", GIT_COMMITTER_NAME: "merge-queue", GIT_COMMITTER_EMAIL: "merge-queue@nodespace" })
      .quiet();
    return out.stdout.toString().trim();
  }

  /** The lock's current commit on the remote, or null when nobody holds it. */
  async lockSha(): Promise<string | null> {
    return (await this.lsRemote(LOCK_REF)).get(LOCK_REF) ?? null;
  }

  /** Who holds the lock at `sha`, or null when it can't be read. */
  async lockInfo(sha: string): Promise<LockInfo | null> {
    const fetched = await this.git(["fetch", "--quiet", "--no-tags", this.remote, `+${LOCK_REF}:${SEEN_LOCK_REF}`]);
    if (fetched.exitCode !== 0) return null;
    const out = await this.git(["log", "-1", "--format=%B", sha]);
    return out.exitCode === 0 ? parseLockInfo(out.stdout.toString().trim()) : null;
  }

  /**
   * Takes the lock if it is free (`expected` = "") or still at the stale
   * `expected` sha being taken over. Returns the new lock sha, or null when
   * someone else got there first.
   */
  async tryAcquire(info: LockInfo, expected = ""): Promise<string | null> {
    const sha = await this.lockCommit(info);
    return (await this.compareAndSwap(LOCK_REF, expected, sha)) ? sha : null;
  }

  /**
   * Re-pushes the lock (the heartbeat), optionally with a new PR list.
   *
   * - `renewed`: pushed; the lock is this process's, at `sha`.
   * - `lost`: the lock is no longer `current` — it was taken over, so this
   *   process must stop before it lands anything.
   * - `unknown`: the push failed and the lock couldn't be read, or still reads
   *   `current` (a network blip, a 5xx). Still held as far as anyone knows;
   *   the next beat retries. Not proof of holding, though: only `renewed` is.
   *
   * A push the server applied but whose response was lost reads back as the
   * new sha: that's `renewed`, not `lost`.
   */
  async renew(current: string, info: LockInfo): Promise<RenewResult> {
    let sha: string | null = null;
    try {
      sha = await this.lockCommit(info);
      if (await this.compareAndSwap(LOCK_REF, current, sha)) return { status: "renewed", sha };
    } catch {
      // Fall through to reading the lock.
    }
    try {
      const now = await this.lockSha();
      if (sha !== null && now === sha) return { status: "renewed", sha };
      return now === current ? { status: "unknown" } : { status: "lost" };
    } catch {
      return { status: "unknown" };
    }
  }

  /** Releases the lock if it is still `current`. */
  async release(current: string): Promise<void> {
    await this.compareAndSwap(LOCK_REF, current, null);
  }
}

/**
 * Watches the lock as a waiter sees it, and says when it has gone stale: the
 * same sha, seen continuously for `staleAfterMs` of this process's own time.
 */
export class StaleWatch {
  private sha: string | null = null;
  private since = 0;

  constructor(
    private readonly staleAfterMs = STALE_AFTER_MS,
    private readonly now: () => number = Date.now
  ) {}

  /** Records a sighting of the lock at `sha` (null = free). True once it is stale. */
  observe(sha: string | null): boolean {
    const t = this.now();
    if (sha !== this.sha) {
      this.sha = sha;
      this.since = t;
      return false;
    }
    return sha !== null && t - this.since >= this.staleAfterMs;
  }
}

/** What landing needs from git and GitHub; injected so every stop path is testable. */
export interface Lander {
  /** A live check that this process still holds the lock: true only on a successful heartbeat. */
  confirmHolding(): Promise<boolean>;
  isQueued(pr: number): Promise<boolean>;
  /** main on origin right now. */
  main(): Promise<{ sha: string; tree: string }>;
  /** The PR branch's head on origin, or null when it can't be read. */
  headOf(branch: string): Promise<string | null>;
  /** Replays commits onto `mainSha`: the resulting tree and tip, or null on any failure. */
  replayOnto(mainSha: string, commits: string[]): Promise<{ tree: string; tip: string } | null>;
  /** Moves the PR branch from `head` to `tip`; false when its head had moved. */
  push(branch: string, head: string, tip: string): Promise<boolean>;
  /**
   * Squash-merges the PR at exactly `tip`. On a refusal, `definite` says
   * whether it's the PR's own doing (a draft, closed, conflicting) — as
   * opposed to GitHub being slow, rate-limited or mid-recompute, which says
   * nothing about the PR. A merge that went through despite an error is `ok`.
   */
  merge(pr: number, tip: string): Promise<{ ok: true } | { ok: false; reason: string; definite: boolean }>;
  /** After a landing: out of the queue, branch deleted. */
  landed(pr: number, branch: string): Promise<void>;
  eject(pr: number, reason: string): Promise<void>;
}

/** A stacked PR, as landing needs it. */
export interface LandEntry {
  pr: number;
  headRefName: string;
  head: string;
  commits: string[];
  /** The tree of main plus this PR and every PR below it. */
  tree: string;
}

export interface LandOutcome {
  landed: number[];
  /** Why landing stopped before the end of the stack; the rest stay queued. */
  stopped?: string;
  /** A PR GitHub definitely won't merge, ejected so the queue can't loop on it. */
  ejected?: number;
}

/**
 * Squash-merges each stacked PR in order, checking before each that nothing
 * the round relied on has changed: the PR is still queued, main is still the
 * tree below it in the stack (no push outside the queue), its head hasn't
 * moved, replaying it onto main reproduces the tested tree, and — last, at the
 * moments of the side effects (the branch push and the merge) — this process
 * still holds the lock. Stops at the first that fails; the rest stay queued
 * and are retested next round. A merge GitHub definitely won't do (the PR
 * became a draft, was closed, conflicts) ejects that PR, so the next round
 * doesn't rerun the gate only to be refused again; any other refusal just
 * stops, so a GitHub hiccup never costs a PR its place.
 */
export async function landStack(stack: LandEntry[], baseTree: string, l: Lander): Promise<LandOutcome> {
  const landed: number[] = [];
  let expectedTree = baseTree;
  const stop = (stopped: string): LandOutcome => ({ landed, stopped });
  for (const entry of stack) {
    if (!(await l.isQueued(entry.pr))) return stop(`#${entry.pr} left the queue`);
    const main = await l.main();
    if (main.tree !== expectedTree) return stop("main moved outside the queue");
    if ((await l.headOf(entry.headRefName)) !== entry.head) return stop(`#${entry.pr}'s head moved`);
    const replay = await l.replayOnto(main.sha, entry.commits);
    if (replay === null || replay.tree !== entry.tree) {
      return stop(`replaying #${entry.pr} onto main didn't reproduce the tested tree`);
    }
    const lostLock = "this machine couldn't confirm it still holds the queue's lock";
    if (replay.tip !== entry.head) {
      if (!(await l.confirmHolding())) return stop(lostLock);
      if (!(await l.push(entry.headRefName, entry.head, replay.tip))) return stop(`#${entry.pr}'s head moved`);
    }
    if (!(await l.confirmHolding())) return stop(lostLock);
    const merged = await l.merge(entry.pr, replay.tip);
    if (!merged.ok) {
      if (!merged.definite) return stop(`GitHub didn't merge #${entry.pr} (${merged.reason})`);
      await l.eject(entry.pr, `GitHub won't merge it: ${merged.reason}`);
      return { landed, stopped: `GitHub won't merge #${entry.pr}`, ejected: entry.pr };
    }
    landed.push(entry.pr);
    expectedTree = entry.tree;
    await l.landed(entry.pr, entry.headRefName);
  }
  return { landed };
}

/** Terminal colour and cursor codes: an ESC, `[`, parameters, a final letter. */
const ANSI_ESCAPE = new RegExp(`${String.fromCharCode(27)}\\[[0-9;]*[A-Za-z]`, "g");

/** Strips terminal escape codes, which render as noise in a PR comment. */
export function stripAnsi(text: string): string {
  return text.replace(ANSI_ESCAPE, "");
}

/** `text` in a code fence longer than any run of backticks inside it, so gate output can't close it early. */
export function fenced(text: string): string {
  const longest = Math.max(0, ...[...text.matchAll(/`+/g)].map((m) => m[0].length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}\n${text}\n${fence}`;
}

/**
 * How a gate run ended, from its exit code: the code's verdict, or this
 * machine's failure to give one. No exit code means the gate was killed by a
 * signal (the OOM killer, a Ctrl-C) — the machine, not the code. But when the
 * stack changes the gate's own scripts, those scripts are what decided the
 * exit code, so an infra exit is theirs to own: otherwise a PR that breaks
 * the gate would stall the queue forever without being bisected out.
 */
export function gateVerdict(exitCode: number | null, stackChangesGate = false): "passed" | "failed" | "infra" {
  if (exitCode === 0) return "passed";
  if (exitCode === GATE_INFRA_EXIT || exitCode === null) return stackChangesGate ? "failed" : "infra";
  return "failed";
}

/** Whether a stack changes the merge gate's own scripts (see gateVerdict). */
export function changesGate(changedPaths: string[]): boolean {
  return changedPaths.some((path) => /^scripts\/(test-gate|gate-[\w-]+)\.ts$/.test(path));
}

/**
 * Whether a stack changes what `bun install` reads — only then can a failing
 * install be the stack's fault rather than the registry's or the network's.
 */
export function changesDependencies(changedPaths: string[]): boolean {
  return changedPaths.some((path) => /(^|\/)(package\.json|bun\.lockb?)$/.test(path));
}
