#!/usr/bin/env bun
// Machine-wide advisory lock that serializes pre-push gates.
//
// The gate (scripts/test-gate.ts, ADR-047) runs test:all + cargo build +
// test:e2e, each of which parallelizes across every core it can find. Nothing
// coordinated between worktrees, so N concurrent sessions each assumed they
// had the whole machine to themselves. On a 14-core box, five simultaneous
// gates oversubscribe it several times over, and the failures that produces
// are not assertion failures — they are worker timeouts and daemon-health
// timeouts on code that is perfectly correct. Retrying on a quiet machine
// "fixes" them, which is exactly what makes them expensive: the signal is
// indistinguishable from a real regression until several minutes have been
// spent on it.
//
// So the gates queue instead of competing. The alternative — capping each
// gate's parallelism (--maxWorkers, CARGO_BUILD_JOBS) — was rejected: it
// slows the common case (one gate, idle machine) permanently in order to fix
// the contended case, and N throttled gates still exceed the core count
// anyway.
//
// Why a lockfile and not flock(2): the lock has to say who holds it and for
// how long, so a queued push can print something honest instead of sitting
// silent for four minutes (indistinguishable, to the person watching, from
// the multi-minute cold `cargo build` the gate already does). A file whose
// contents are the holder's identity gives us that for free; an flock on an
// empty file does not.

import { openSync, closeSync, writeSync, readFileSync, unlinkSync } from "node:fs";
import { hostname, tmpdir } from "node:os";
import { join } from "node:path";

/**
 * The errno string off a thrown syscall error, or "" for anything that isn't
 * one. `NodeJS.ErrnoException` isn't in scope under this package's
 * bun-types-only tsconfig, and the distinctions here (EEXIST vs. anything
 * else, EPERM vs. ESRCH) decide correctness rather than just messaging.
 */
export function errorCode(err: unknown): string {
  if (err && typeof err === "object" && "code" in err) {
    const code = (err as { code: unknown }).code;
    if (typeof code === "string") return code;
  }
  return "";
}

/**
 * Where the lock lives. Machine-wide on purpose: worktrees of the same repo
 * are the thing being coordinated, but so are separate clones — the resource
 * under contention is the CPU, which is per-machine, not per-repo.
 *
 * On macOS `tmpdir()` is per-user (/var/folders/...), which is the right
 * scope in practice: gates are run by a developer, and one developer's gates
 * are what collide.
 */
export const LOCK_PATH = join(tmpdir(), "nodespace-test-gate.lock");

/** Give up waiting after this long and run anyway, with a warning. */
export const DEFAULT_MAX_WAIT_MS = 30 * 60 * 1000;

/** How often to re-check the lock (and re-print the waiting line). */
export const DEFAULT_POLL_INTERVAL_MS = 2000;

/** Escape hatch for someone who knowingly wants parallel gates. */
export const DISABLE_ENV_VAR = "NODESPACE_GATE_NO_LOCK";

export interface LockHolder {
  pid: number;
  /** Epoch ms when the holder acquired the lock. */
  startedAt: number;
  /** Machine that wrote the lock — see isForeignHost(). */
  host: string;
  /** Working directory of the holder, so the waiting line can name the worktree. */
  cwd: string;
}

export function serializeHolder(holder: LockHolder): string {
  return `${JSON.stringify(holder)}\n`;
}

/**
 * Reads a lockfile's contents into a holder record.
 *
 * Returns null for anything unreadable — a truncated file (we were mid-write
 * when the reader looked), hand-edited junk, or a record missing the fields
 * that make it actionable. A null here is treated exactly like a stale lock:
 * a lockfile we cannot interpret must never be able to wedge every future
 * push on the machine.
 */
export function parseHolder(raw: string): LockHolder | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object") return null;
  const { pid, startedAt, host, cwd } = parsed as Record<string, unknown>;
  if (typeof pid !== "number" || !Number.isInteger(pid) || pid <= 0) return null;
  if (typeof startedAt !== "number" || !Number.isFinite(startedAt)) return null;
  if (typeof host !== "string" || typeof cwd !== "string") return null;
  return { pid, startedAt, host, cwd };
}

/**
 * Whether a pid is still running.
 *
 * `kill(pid, 0)` sends no signal; it only performs the permission and
 * existence checks. EPERM means the process exists but belongs to another
 * user — still alive, so still a valid holder.
 */
export function isPidAlive(pid: number, kill: (pid: number, signal: 0) => void = process.kill): boolean {
  try {
    kill(pid, 0);
    return true;
  } catch (err) {
    return errorCode(err) === "EPERM";
  }
}

/**
 * A lock written by a different machine cannot be liveness-checked: the pid
 * in it is meaningless here, and `kill(pid, 0)` would answer about whatever
 * local process happens to share that number — possibly reporting a dead
 * holder as alive, or (worse) reclaiming a live one. This only arises if
 * tmpdir is on shared storage, which is unusual but not impossible.
 */
export function isForeignHost(holder: LockHolder, localHost: string = hostname()): boolean {
  return holder.host !== localHost;
}

export function formatDuration(ms: number): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return minutes > 0 ? `${minutes}m${String(seconds).padStart(2, "0")}s` : `${seconds}s`;
}

export function formatWaitingLine(holder: LockHolder, now: number, waitedMs: number): string {
  const heldFor = formatDuration(now - holder.startedAt);
  const waited = formatDuration(waitedMs);
  return `  waiting for another gate (pid ${holder.pid}, running ${heldFor}) — waited ${waited}`;
}

export function formatTimeoutWarning(holder: LockHolder | null, maxWaitMs: number): string {
  const who = holder ? `pid ${holder.pid}` : "the holder";
  return (
    `\n⚠ Still waiting on another gate (${who}) after ${formatDuration(maxWaitMs)} — running anyway.\n` +
    "  Both gates now share the machine, so timeouts and perf assertions in this\n" +
    "  run are less trustworthy than usual. If it fails oddly, re-run it alone.\n"
  );
}

export interface AcquireOptions {
  lockPath?: string;
  maxWaitMs?: number;
  pollIntervalMs?: number;
  /** Injected for tests; defaults to real wall-clock and real sleeping. */
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
  log?: (message: string) => void;
  host?: string;
  isAlive?: (pid: number) => boolean;
}

/** Returned by acquireGateLock; call release() exactly once when the gate is done. */
export interface GateLock {
  /** False when the lock was skipped or waited out — the gate ran unserialized. */
  held: boolean;
  release: () => void;
}

function tryCreateLock(lockPath: string, holder: LockHolder): boolean {
  try {
    // "wx" is O_CREAT|O_EXCL: it succeeds only if we are the process that
    // created the file. This is the whole mutual-exclusion primitive — every
    // other check in this module is diagnostics or stale-reaping around it.
    const fd = openSync(lockPath, "wx");
    try {
      writeSync(fd, serializeHolder(holder));
    } finally {
      closeSync(fd);
    }
    return true;
  } catch (err) {
    if (errorCode(err) === "EEXIST") return false;
    throw err;
  }
}

function readHolder(lockPath: string): LockHolder | null {
  try {
    return parseHolder(readFileSync(lockPath, "utf8"));
  } catch {
    // ENOENT: the holder released between our failed create and this read.
    // Anything else: unreadable, which we treat the same as unparseable.
    return null;
  }
}

function removeLock(lockPath: string): void {
  try {
    unlinkSync(lockPath);
  } catch {
    // Already gone — someone reaped it, or we never held it. Either way
    // there is nothing to undo.
  }
}

/**
 * Releases the lock only if the file still names us as its holder.
 *
 * An unconditional unlink here would be a correctness bug, not just untidy:
 * once we have released (or been reaped as stale by a waiter that decided we
 * were gone), the file at that path belongs to a DIFFERENT gate, and deleting
 * it hands the machine to two gates at once — the exact failure this module
 * exists to prevent, made harder to diagnose by the fact that both gates
 * believe they hold the lock.
 *
 * The read-then-unlink is not atomic, so a sufficiently unlucky interleaving
 * could still delete a successor's lock. That race requires us to be reaped
 * as dead while we are in fact alive and mid-release, which cannot happen
 * while we are the live pid the reaper checks. Closing it properly needs an
 * open handle and inode comparison; the sequence that would defeat this check
 * is not reachable from how the gate actually runs.
 */
function releaseIfOwner(lockPath: string, ownerPid: number): void {
  const current = readHolder(lockPath);
  if (current !== null && current.pid !== ownerPid) return;
  removeLock(lockPath);
}

const realSleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Acquires the machine-wide gate lock, waiting for any current holder.
 *
 * Always returns — it never throws and never blocks forever. If the wait
 * exceeds `maxWaitMs` it gives up and returns `held: false`, letting the gate
 * run unserialized with a warning, because a gate that refuses to run is
 * worse than a gate that runs slowly: the failure mode of blocking forever is
 * that nobody can push at all.
 */
export async function acquireGateLock(options: AcquireOptions = {}): Promise<GateLock> {
  const lockPath = options.lockPath ?? LOCK_PATH;
  const maxWaitMs = options.maxWaitMs ?? DEFAULT_MAX_WAIT_MS;
  const pollIntervalMs = options.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS;
  const now = options.now ?? Date.now;
  const sleep = options.sleep ?? realSleep;
  const log = options.log ?? ((message: string) => console.log(message));
  const host = options.host ?? hostname();
  const isAlive = options.isAlive ?? ((pid: number) => isPidAlive(pid));

  if (process.env[DISABLE_ENV_VAR]) {
    log(`\n⚠ ${DISABLE_ENV_VAR} set — running without the gate lock (gates may run concurrently).\n`);
    return { held: false, release: () => {} };
  }

  const startedWaitingAt = now();
  const holder: LockHolder = { pid: process.pid, startedAt: startedWaitingAt, host, cwd: process.cwd() };
  let announced = false;
  let lastSeen: LockHolder | null = null;

  for (;;) {
    // startedAt is stamped at acquisition, not at first attempt, so the
    // "running Xm" a waiter prints is how long the holder has held the lock
    // rather than how long it has been trying to.
    holder.startedAt = now();
    let created: boolean;
    try {
      created = tryCreateLock(lockPath, holder);
    } catch (err) {
      // The lock is an advisory optimization; it must never be the reason a
      // push cannot happen. A read-only or full tmpdir, an unwritable path —
      // degrade to running unserialized rather than blocking the push on
      // infrastructure that has nothing to do with the tests.
      console.warn(
        `\n⚠ Could not use the gate lock (${err instanceof Error ? err.message : String(err)}).` +
          "\n  Running unserialized — concurrent gates on this machine may contend.\n"
      );
      return { held: false, release: () => {} };
    }
    if (created) {
      if (announced) log("  lock acquired — starting.\n");
      return { held: true, release: () => releaseIfOwner(lockPath, holder.pid) };
    }

    const current = readHolder(lockPath);
    lastSeen = current ?? lastSeen;

    // Reap a lock nobody owns: an unparseable file, or one whose pid is gone
    // (a killed session must not wedge every future push). A foreign-host
    // lock is never reaped — its pid means nothing here — but it is still
    // bounded by maxWaitMs below, so it cannot wedge us either.
    const reclaimable = current === null || (!isForeignHost(current, host) && !isAlive(current.pid));
    if (reclaimable) {
      log(
        current === null
          ? "  reclaiming an unreadable gate lock."
          : `  reclaiming a stale gate lock (pid ${current.pid} is gone).`
      );
      removeLock(lockPath);
      // Loop rather than acquire directly: another waiter may have won the
      // race to recreate it, and tryCreateLock is the only thing allowed to
      // decide who holds it.
      continue;
    }

    const waitedMs = now() - startedWaitingAt;
    if (waitedMs >= maxWaitMs) {
      console.warn(formatTimeoutWarning(lastSeen, maxWaitMs));
      return { held: false, release: () => {} };
    }

    if (!announced) {
      log("\n⏳ Another pre-push gate is running on this machine — queueing behind it.");
      log("   (gates are serialized so they don't starve each other of CPU; see ADR-047)");
      announced = true;
    }
    log(formatWaitingLine(current, now(), waitedMs));
    await sleep(pollIntervalMs);
  }
}

/**
 * Wires release() to process exit and to the signals a terminal actually
 * sends, so the lock survives none of: a failing step's process.exit(1),
 * Ctrl-C, or a terminal closing. Without this an aborted gate would leave a
 * lock that the next push has to wait out (until its pid check reaps it —
 * which works, but only after that push has already printed a confusing wait).
 *
 * "exit" cannot do async work, and unlinkSync is sync, which is why the whole
 * module uses the sync fs API rather than fs/promises.
 */
export function registerLockRelease(lock: GateLock): void {
  if (!lock.held) return;
  let released = false;
  const release = () => {
    if (released) return;
    released = true;
    lock.release();
  };

  process.on("exit", release);
  for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"] as const) {
    process.on(signal, () => {
      release();
      // Re-raise with the default handler so the exit status reflects the
      // signal, rather than this handler swallowing it into a clean exit —
      // git needs a non-zero status to abort the push.
      process.removeAllListeners(signal);
      process.kill(process.pid, signal);
    });
  }
}
