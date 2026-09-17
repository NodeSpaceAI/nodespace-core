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
//
// The mutual-exclusion primitive is link(2)'s atomic fail-on-EEXIST, NOT
// open(O_EXCL) — see tryCreateLock for why that distinction is the whole
// correctness argument rather than an implementation detail.

import { linkSync, readFileSync, readdirSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { hostname, tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";

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
 * Returns null for anything uninterpretable — hand-edited junk, a truncated
 * file, or a record missing the fields that make it actionable. Such a lock
 * is reaped rather than waited on: a lockfile nobody can interpret must never
 * be able to wedge every future push on the machine.
 *
 * Note that this module cannot itself produce a half-written lock —
 * tryCreateLock publishes whole files — so a null here means genuine external
 * damage, not a race with a concurrent acquirer.
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

/**
 * Publishes a fully-written lockfile at `lockPath`, atomically.
 *
 * The obvious implementation — `open(lockPath, "wx")` then `write` — is
 * WRONG, and wrong in a way that silently defeats the whole module. Those are
 * two syscalls, and between them the lockfile exists with zero bytes. A
 * concurrent waiter reading in that window sees "", cannot parse a holder out
 * of it, correctly concludes nobody owns it, and reaps it — deleting a live
 * holder's lock and then taking it. Both processes then run the gate
 * believing they hold the lock, which is worse than having no lock at all: it
 * removes the suspicion of contention that would otherwise explain the
 * resulting timeout.
 *
 * So the file is written under a unique staging name first and only becomes
 * reachable at `lockPath` once it is complete. `link(2)` is atomic and fails
 * with EEXIST rather than clobbering — which is why it is used instead of
 * `rename(2)`, whose silent replace-on-collide is exactly the wrong
 * behaviour for a lock.
 */
/**
 * Minimum age before an orphaned staging file is swept. Many orders of
 * magnitude beyond the window it guards (a single writeFileSync), so it can
 * never delete one a live process is still between `write` and `link` on.
 */
const STAGING_SWEEP_AGE_MS = 60_000;

/**
 * Removes staging files abandoned by a process that died between writing one
 * and publishing it.
 *
 * `tryCreateLock`'s `finally` covers every exit the process survives —
 * ordinary exceptions and `process.exit` included — but not a SIGKILL, an OOM
 * kill, or a power loss. Those leave a `<lockPath>.<pid>.<uuid>` behind with
 * nothing to collect it.
 *
 * This is litter, never a correctness problem: a staging file is not at
 * `lockPath`, so `readHolder` never sees it and it cannot be mistaken for a
 * claim. It is swept because it is nearly free to do so, not because it is
 * dangerous.
 *
 * Best-effort throughout — a sweep that cannot read the directory, or that
 * races another process's own cleanup, must never affect whether a lock can
 * be acquired.
 */
function sweepOrphanedStaging(lockPath: string, now: number): void {
  const dir = dirname(lockPath);
  const prefix = `${basename(lockPath)}.`;
  let entries: string[];
  try {
    entries = readdirSync(dir);
  } catch {
    return;
  }
  for (const name of entries) {
    if (!name.startsWith(prefix)) continue;
    const full = join(dir, name);
    try {
      if (now - statSync(full).mtimeMs > STAGING_SWEEP_AGE_MS) unlinkSync(full);
    } catch {
      // Vanished under us (its owner cleaned up), or not ours to remove.
    }
  }
}

function tryCreateLock(lockPath: string, holder: LockHolder): boolean {
  // Same directory as the lock: link(2) cannot cross filesystems.
  const staging = `${lockPath}.${process.pid}.${randomUUID()}`;
  writeFileSync(staging, serializeHolder(holder));
  try {
    linkSync(staging, lockPath);
    return true;
  } catch (err) {
    if (errorCode(err) === "EEXIST") return false;
    throw err;
  } finally {
    // The staged copy has served its purpose either way: on success the lock
    // path is a second name for the same inode, and on failure it is garbage.
    try {
      unlinkSync(staging);
    } catch {
      // Nothing to clean up, or we cannot — either way it must not fail the
      // acquisition, which has already been decided above.
    }
  }
}

/** What a read of the lock path found. See readHolder(). */
export type LockRead =
  | { state: "absent" }
  | { state: "unreadable" }
  | { state: "held"; holder: LockHolder };

/**
 * Reads the lock path, distinguishing "nothing is there" from "something is
 * there but we cannot interpret it".
 *
 * These must not collapse into one value. They call for opposite actions: an
 * absent lock means retry the create (there is nothing to delete, and
 * unlinking blind would destroy a successor's lock that appeared in the
 * meantime), while an unreadable one is genuine garbage to reap. Folding both
 * into `null` also makes the logs lie — reporting a corrupt lockfile when the
 * file had simply been released.
 */
export function readHolder(lockPath: string): LockRead {
  let raw: string;
  try {
    raw = readFileSync(lockPath, "utf8");
  } catch (err) {
    if (errorCode(err) === "ENOENT") return { state: "absent" };
    return { state: "unreadable" };
  }
  const holder = parseHolder(raw);
  return holder === null ? { state: "unreadable" } : { state: "held", holder };
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
 * Removes the lock only if the file still names `claimantPid` as its holder.
 *
 * Serves both callers that remove a lock, because both need the same
 * predicate — "is this still the claim I think it is?" — differing only in
 * whose pid that is:
 *   - a holder releasing its own lock, passing its own pid;
 *   - a waiter reaping a corpse, passing the dead pid it just judged stale.
 *
 * An unconditional unlink would be a correctness bug in either role, not just
 * untidy: the file at that path may by then belong to a DIFFERENT gate, and
 * deleting it hands the machine to two gates at once — the exact failure this
 * module exists to prevent, made harder to diagnose by the fact that both
 * gates believe they hold the lock. For the reaping caller this is the live
 * hazard: two waiters can judge the same corpse reapable, and without this
 * check the second would unlink a lock the first had already reaped and
 * legitimately retaken.
 *
 * The read-then-unlink is not atomic, so in principle an unlucky interleaving
 * could delete a successor's lock. What rules that out for a self-releasing
 * holder is the reap path's liveness gate: a waiter only reclaims a lock whose
 * owning pid is gone, and a process is by definition alive while executing
 * this function, so no waiter can replace its lock underneath it. That
 * argument depends on every reap being gated on liveness — which is why the
 * `absent` case in the acquire loop retries instead of unlinking, and why only
 * `unreadable` and dead-pid locks are reaped.
 */
function removeLockIfHeldBy(lockPath: string, claimantPid: number): void {
  const current = readHolder(lockPath);
  // "held by someone else" is the one case we must not touch. An absent lock
  // has nothing to remove, and an unreadable one cannot be anyone's claim —
  // because tryCreateLock publishes whole files, so a partially-written lock
  // is not a state this module can produce.
  if (current.state === "held" && current.holder.pid !== claimantPid) return;
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

  // Once per acquisition, not per poll: this is housekeeping for a rare
  // SIGKILL-class death, and a waiter polling every 2s has no reason to
  // re-scan the directory each time.
  sweepOrphanedStaging(lockPath, now());

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
      return { held: true, release: () => removeLockIfHeldBy(lockPath, holder.pid) };
    }

    const current = readHolder(lockPath);

    // Released between our failed create and this read. Nothing to reap —
    // unlinking here would destroy whatever successor has since claimed the
    // path. Just try again.
    if (current.state === "absent") continue;

    // Genuine garbage: a hand-edited file, or one truncated by something
    // outside this module. Nobody can own it, so it must not be allowed to
    // wedge every future push on the machine.
    if (current.state === "unreadable") {
      log("  reclaiming an unreadable gate lock.");
      removeLock(lockPath);
      continue;
    }

    const holderNow = current.holder;
    lastSeen = holderNow;

    // Reap a lock whose owning process is gone — a killed session must not
    // wedge future pushes. A foreign-host lock is never reaped (its pid means
    // nothing here) but is still bounded by maxWaitMs below, so it cannot
    // wedge us either.
    if (!isForeignHost(holderNow, host) && !isAlive(holderNow.pid)) {
      log(`  reclaiming a stale gate lock (pid ${holderNow.pid} is gone).`);
      // Re-read immediately before unlinking, and only remove the file if it
      // still names the dead holder we judged. Two waiters can reach this
      // point on the same corpse; without the re-check, the second would
      // unlink a lock the first had already reaped and legitimately retaken.
      removeLockIfHeldBy(lockPath, holderNow.pid);
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
    log(formatWaitingLine(holderNow, now(), waitedMs));
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
    const onSignal = () => {
      release();
      // Re-raise so the exit status reflects the signal rather than this
      // handler swallowing it into a clean exit — git needs a non-zero status
      // to abort the push. Remove only our own listener: if nothing else is
      // subscribed, node restores the default (terminate) behaviour, and if
      // something is, that handler is not ours to cancel.
      process.removeListener(signal, onSignal);
      process.kill(process.pid, signal);
    };
    process.on(signal, onSignal);
  }
}
