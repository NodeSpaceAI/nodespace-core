#!/usr/bin/env bun
// Machine-wide advisory locks that serialize the heavy work on this machine:
// the gate checkout (MERGE_LOCK_PATH) and the CPU-heavy runs themselves
// (MACHINE_LOCK_PATH) — a merge gate for its whole run, or `bun run
// test:changed` while it builds and runs the Rust tier.
//
// The merge gate (scripts/test-gate.ts, ADR-047) compiles the workspace and
// runs the full test pyramid, each step parallelizing across every core it
// can find. Nothing coordinated between worktrees, so N concurrent sessions
// each assumed they had the whole machine to themselves. On a 14-core box
// that oversubscribes it several times over, and the failures that produces
// are not assertion failures — they are worker timeouts and daemon-health
// timeouts on code that is perfectly correct. Retrying on a quiet machine
// "fixes" them, which is exactly what makes them expensive: the signal is
// indistinguishable from a real regression until several minutes have been
// spent on it.
//
// So heavy runs queue instead of competing. The alternative — capping each
// run's parallelism (--maxWorkers, CARGO_BUILD_JOBS) — was rejected: it slows
// the common case (one run, idle machine) permanently in order to fix the
// contended case, and N throttled runs still exceed the core count anyway.
//
// Why a lockfile and not flock(2): the lock has to say who holds it and for
// how long, so a queued run can print something honest instead of sitting
// silent (indistinguishable, to the person watching, from a hang). A file
// whose contents are the holder's identity gives us that for free; an flock
// on an empty file does not.
//
// The mutual-exclusion primitive is link(2)'s atomic fail-on-EEXIST, NOT
// open(O_EXCL) — see tryCreateLock for why that distinction is the whole
// correctness argument rather than an implementation detail.
//
// Waiters are served in arrival order. Each files a ticket in a queue
// directory beside the lock, named for when it started waiting, and only the
// oldest live ticket may try to create the lock. Without the queue every
// waiter polled the lockfile and whoever polled first after a release won, so
// one gate could lose every race for the full wait cap while later arrivals
// went ahead of it — and then run unserialized anyway, the exact contention
// the lock exists to prevent.

import { chmodSync, linkSync, mkdirSync, readFileSync, readdirSync, renameSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { hostname, tmpdir, userInfo } from "node:os";
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

/** Give up waiting after this long and run anyway, with a warning. */
export const DEFAULT_MAX_WAIT_MS = 30 * 60 * 1000;

/** How often to re-check the lock (and re-print the waiting line). */
export const DEFAULT_POLL_INTERVAL_MS = 2000;

/** Escape hatch for someone who knowingly wants parallel gates. */
export const DISABLE_ENV_VAR = "NODESPACE_GATE_NO_LOCK";

// Where the locks live. Worktrees of the same repo are the thing being
// coordinated, but so are separate clones and separate macOS accounts — the
// resource under contention is the CPU, which is per-machine, not per-repo or
// per-user.

/**
 * The directory every account on this machine shares for the machine slot.
 *
 * Not `tmpdir()`: on macOS that is per-user (/var/folders/...), so two
 * accounts on one Mac each got their own slot, and one account's Rust tier
 * ran beside another's merge gate (load average ~175, a gate killed for low
 * memory). Not `/tmp` either: its sticky bit stops one user unlinking
 * another's lock, ticket or staging file, so reclaiming a dead holder and
 * sweeping its litter would fail with EPERM. `/Users/Shared` is itself
 * sticky, which is why the lock lives one level down, in a directory created
 * world-writable and not sticky (see ensureDir).
 *
 * Elsewhere `tmpdir()` is the machine's shared temp directory or the machine
 * has one developer account; a subdirectory keeps the files together.
 */
export const SHARED_LOCK_DIR =
  process.platform === "darwin" ? "/Users/Shared/nodespace-gate" : join(tmpdir(), "nodespace-gate");

/**
 * The gate-checkout lock, for `bun run merge` (scripts/merge-pr.ts): this
 * account's one gate checkout, held while a merge-queue round or a --dry-run
 * uses it. Merges themselves are serialized team-wide by the merge queue's
 * lock on origin (scripts/merge-queue.ts), which a round takes first; a
 * --dry-run takes only this one. Per-user: each account has its own gate
 * checkout.
 */
export const MERGE_LOCK_PATH = join(tmpdir(), "nodespace-merge.lock");

/**
 * The machine slot: one CPU-heavy run at a time. A merge gate takes it before
 * its first compile and holds it until it exits, so no other Rust build or
 * Rust test run overlaps its tests — a compile beside a timed test run slows
 * it several-fold even under `nice`. `bun run test:changed` takes it only
 * around its Rust tier. Locks are only ever taken in the order merge →
 * machine, so the two can't deadlock. Shared by every account on the machine
 * (SHARED_LOCK_DIR); callers pass `shared: true`.
 */
export const MACHINE_LOCK_PATH = join(SHARED_LOCK_DIR, "machine.lock");

/** What the machine slot serializes, for its waiting messages. */
export const MACHINE_SLOT_WHAT = "heavy run (merge gate or test:changed Rust tier)";

export interface LockHolder {
  pid: number;
  /** Epoch ms when the holder acquired the lock. */
  startedAt: number;
  /** Machine that wrote the lock — see isForeignHost(). */
  host: string;
  /** Account that holds it, so a waiter can tell another account's run from its own. */
  user: string;
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
 * be able to wedge every future run on the machine.
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
  const { pid, startedAt, host, user, cwd } = parsed as Record<string, unknown>;
  if (typeof pid !== "number" || !Number.isInteger(pid) || pid <= 0) return null;
  if (typeof startedAt !== "number" || !Number.isFinite(startedAt)) return null;
  if (typeof host !== "string" || typeof cwd !== "string") return null;
  // `user` only labels the waiting line, so it never decides whether a record
  // is interpretable — that would let a display field get a live lock reaped.
  return { pid, startedAt, host, user: typeof user === "string" ? user : "?", cwd };
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

/**
 * Who holds the lock and for how long, e.g. "pid 123 (alice) in
 * issue-45-foo, holding 3m05s". Built from the lock's own contents only: the
 * holder's worktree may be in another account's home, unreadable to us.
 */
function describeHolder(holder: LockHolder, now: number): string {
  return `pid ${holder.pid} (${holder.user}) in ${basename(holder.cwd)}, holding ${formatDuration(now - holder.startedAt)}`;
}

/** This process's account name, or its uid where the name can't be looked up. */
export function currentUser(): string {
  try {
    return userInfo().username;
  } catch {
    return `uid ${process.getuid?.() ?? "?"}`;
  }
}

export function formatWaitingLine(holder: LockHolder, now: number, waitedMs: number): string {
  return `  waiting (${describeHolder(holder, now)}) — waited ${formatDuration(waitedMs)}`;
}

/** The waiting line for a gate that still has others ahead of it in the queue. */
export function formatQueuedLine(ahead: number, holder: LockHolder | null, now: number, waitedMs: number): string {
  const position = `${ahead} gate${ahead === 1 ? "" : "s"} ahead`;
  const running = holder ? `, current: ${describeHolder(holder, now)}` : "";
  return `  queued (${position}${running}) — waited ${formatDuration(waitedMs)}`;
}

export function formatTimeoutWarning(holder: LockHolder | null, maxWaitMs: number): string {
  const who = holder ? `pid ${holder.pid}` : "the holder";
  return (
    `\n⚠ Still waiting on another run (${who}) after ${formatDuration(maxWaitMs)} — running anyway.\n` +
    "  Both runs now share the machine, so timeouts and perf assertions in this\n" +
    "  run are less trustworthy than usual. If it fails oddly, re-run it alone.\n"
  );
}

export interface AcquireOptions {
  lockPath: string;
  maxWaitMs?: number;
  pollIntervalMs?: number;
  /** Injected for tests; defaults to real wall-clock and real sleeping. */
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
  log?: (message: string) => void;
  host?: string;
  isAlive?: (pid: number) => boolean;
  /** Injected for tests, which run several waiters in one process. */
  pid?: number;
  /** What the lock serializes, for the waiting messages. */
  what?: string;
  /** Queue ahead of every non-urgent waiter (the merge gate). */
  urgent?: boolean;
  /**
   * Every account on the machine uses this lock (the machine slot): create
   * its directory and queue world-writable, so any account can take, reclaim
   * and sweep them. See SHARED_LOCK_DIR.
   */
  shared?: boolean;
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

/**
 * Writes a lock or ticket record, readable by every account whatever the
 * writer's umask. Under umask 077 it would be 0600, and another account
 * reading a shared lock it can't open must not mistake it for garbage.
 * writeFileSync's `mode` is itself masked by the umask, hence the chmod.
 */
function writeRecord(path: string, holder: LockHolder): void {
  writeFileSync(path, serializeHolder(holder));
  chmodSync(path, 0o644);
}

function tryCreateLock(lockPath: string, holder: LockHolder): boolean {
  // Same directory as the lock: link(2) cannot cross filesystems.
  const staging = `${lockPath}.${process.pid}.${randomUUID()}`;
  writeRecord(staging, holder);
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
  | { state: "forbidden" }
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
 *
 * `forbidden` — a lock we lack permission to read — is neither: it is someone
 * else's claim, most likely another account's, and is waited on, never
 * reaped. Deleting a lock needs positive evidence that it is garbage.
 */
export function readHolder(lockPath: string): LockRead {
  let raw: string;
  try {
    raw = readFileSync(lockPath, "utf8");
  } catch (err) {
    const code = errorCode(err);
    if (code === "ENOENT") return { state: "absent" };
    if (code === "EACCES" || code === "EPERM") return { state: "forbidden" };
    return { state: "unreadable" };
  }
  const holder = parseHolder(raw);
  return holder === null ? { state: "unreadable" } : { state: "held", holder };
}

/**
 * Unlinks the lock. False only when it is still there and we could not
 * remove it — a directory this account may not unlink in — so a reaping
 * waiter waits instead of spinning on create → EEXIST → reap.
 */
function removeLock(lockPath: string): boolean {
  try {
    unlinkSync(lockPath);
    return true;
  } catch (err) {
    // Already gone — someone reaped it, or we never held it. Either way
    // there is nothing to undo.
    return errorCode(err) === "ENOENT";
  }
}

/**
 * Removes the lock only if the file still names `claimantPid` as its holder.
 * False when the lock it should have removed is still there (see removeLock).
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
 * `unreadable` and dead-pid locks are reaped — never a `forbidden` one.
 */
function removeLockIfHeldBy(lockPath: string, claimantPid: number): boolean {
  const current = readHolder(lockPath);
  // "held by someone else" is the one case we must not touch. An absent lock
  // has nothing to remove, and an unreadable one cannot be anyone's claim —
  // because tryCreateLock publishes whole files, so a partially-written lock
  // is not a state this module can produce.
  if (current.state === "forbidden") return false;
  if (current.state === "held" && current.holder.pid !== claimantPid) return true;
  return removeLock(lockPath);
}

/** The FIFO queue of waiting gates, beside the lock. */
export function queueDir(lockPath: string): string {
  return `${lockPath}.queue`;
}

/**
 * A ticket's file name. The leading class puts every merge gate's ticket
 * ahead of every other waiter's: a merge is finished work waiting to land,
 * a test:changed run is mid-iteration, so a merge waits only for the run
 * already holding the lock, never for a line of queued ones. Within a class the
 * zero-padded start time makes lexical order arrival order; the pid breaks
 * ties between waiters that arrived in the same millisecond.
 */
export function ticketName(startedWaitingAt: number, pid: number, urgent = false): string {
  return `${urgent ? 0 : 1}-${String(startedWaitingAt).padStart(16, "0")}-${pid}`;
}

/**
 * Creates `dir` if missing. Not recursive: a lock location whose parent
 * doesn't exist is an unusable one — the caller degrades, it doesn't build it.
 *
 * A shared directory is chmod'ed 0777 by whoever creates it, because mkdir
 * applies the umask (0755 by default, which would leave every other account
 * unable to create or unlink anything in it). Only the creator may chmod, so
 * an existing directory is left as its creator made it.
 */
export function ensureDir(dir: string, shared: boolean): void {
  try {
    mkdirSync(dir);
  } catch (err) {
    if (errorCode(err) !== "EEXIST") throw err;
    // A creator killed between mkdir and chmod (or two accounts racing the
    // first creation) leaves it at the umask's mode, which locks every other
    // account out for good. The owner repairs it on its next run.
    if (shared) repairSharedDir(dir);
    return;
  }
  if (shared) chmodSync(dir, 0o777);
}

function repairSharedDir(dir: string): void {
  try {
    const { uid, mode } = statSync(dir);
    if (uid === process.getuid?.() && (mode & 0o7777) !== 0o777) chmodSync(dir, 0o777);
  } catch {
    // Best-effort: an unusable directory surfaces as the caller's EACCES.
  }
}

/** How to fix a shared lock directory another account can't write, for the warning. */
export function sharedDirHint(dir: string): string {
  try {
    const { uid, mode } = statSync(dir);
    if ((mode & 0o7777) === 0o777) return "";
    const octal = (mode & 0o7777).toString(8);
    return `\n  ${dir} is mode ${octal}, owned by uid ${uid}; every account needs it 777.` + `\n  Its owner fixes it with: chmod 777 ${dir}`;
  } catch {
    return "";
  }
}

/**
 * Files this waiter's ticket. Written under a staging name and renamed into
 * place so no reader ever sees a partial ticket; rename is fine here (unlike
 * for the lock itself) because every ticket name is unique to its waiter.
 */
function fileTicket(dir: string, name: string, holder: LockHolder, shared: boolean): void {
  ensureDir(dir, shared);
  const staging = join(dir, `.${name}.${randomUUID()}`);
  writeRecord(staging, holder);
  renameSync(staging, join(dir, name));
}

function removeTicket(dir: string, name: string): void {
  try {
    unlinkSync(join(dir, name));
  } catch {
    // Already gone — reaped as stale by another waiter, or never filed.
  }
}

/**
 * The live tickets queued ahead of `ours`, oldest first. A ticket whose
 * process is gone is reaped here, the same judgement the lock itself gets:
 * a killed session must not hold its place in line forever. A foreign-host
 * ticket can't be judged by pid, so it is dropped once it is older than the
 * wait cap — no live waiter would still be in line by then.
 */
function ticketsAhead(
  dir: string,
  ours: string,
  host: string,
  isAlive: (pid: number) => boolean,
  now: number,
  maxWaitMs: number
): LockHolder[] {
  let names: string[];
  try {
    names = readdirSync(dir).filter((name) => !name.startsWith(".") && name < ours);
  } catch {
    return [];
  }
  const ahead: LockHolder[] = [];
  for (const name of names.sort()) {
    let holder: LockHolder | null;
    try {
      holder = parseHolder(readFileSync(join(dir, name), "utf8"));
    } catch {
      continue;
    }
    if (holder === null || (isForeignHost(holder, host) ? now - holder.startedAt > maxWaitMs : !isAlive(holder.pid))) {
      removeTicket(dir, name);
      continue;
    }
    ahead.push(holder);
  }
  return ahead;
}

/**
 * How often an unchanged waiting status is repeated, so a wait never looks
 * hung. A longer interval left agents inspecting pids and log times to tell a
 * queue from a hang.
 */
export const STATUS_HEARTBEAT_MS = 60 * 1000;

/**
 * A logger for the waiting status that prints only when what it reports
 * changes (keyed by `key`), or once per heartbeat.
 *
 * A waiting gate used to print a line every poll — every 2s — into the
 * terminal of whichever session started it. When that terminal isn't being read,
 * the pipe fills within minutes and the waiter blocks mid-print. It is still
 * alive, so its ticket is never reaped; when its turn comes it can't take the
 * lock, and with first-come-first-served ordering every gate behind it waits
 * until someone looks at that session. A line a minute can't fill a pipe.
 */
export function statusLogger(
  log: (message: string) => void,
  now: () => number,
  heartbeatMs: number = STATUS_HEARTBEAT_MS
): (key: string, line: string) => void {
  let lastKey: string | null = null;
  let lastAt = 0;
  return (key, line) => {
    const t = now();
    if (key === lastKey && t - lastAt < heartbeatMs) return;
    lastKey = key;
    lastAt = t;
    log(line);
  };
}

const realSleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Acquires a machine-wide lock, waiting for any current holder.
 *
 * Always returns — it never throws and never blocks forever. If the wait
 * exceeds `maxWaitMs` it gives up and returns `held: false`, letting the gate
 * run unserialized with a warning, because a gate that refuses to run is
 * worse than a gate that runs slowly: the failure mode of blocking forever is
 * that nothing can merge at all.
 */
export async function acquireGateLock(options: AcquireOptions): Promise<GateLock> {
  const lockPath = options.lockPath;
  const maxWaitMs = options.maxWaitMs ?? DEFAULT_MAX_WAIT_MS;
  const pollIntervalMs = options.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS;
  const now = options.now ?? Date.now;
  const sleep = options.sleep ?? realSleep;
  const log = options.log ?? ((message: string) => console.log(message));
  const host = options.host ?? hostname();
  const isAlive = options.isAlive ?? ((pid: number) => isPidAlive(pid));
  const what = options.what ?? "heavy run";
  const status = statusLogger(log, now);

  if (process.env[DISABLE_ENV_VAR]) {
    log(`\n⚠ ${DISABLE_ENV_VAR} set — running without the lock (heavy runs may overlap).\n`);
    return { held: false, release: () => {} };
  }

  const startedWaitingAt = now();
  const pid = options.pid ?? process.pid;
  const holder: LockHolder = { pid, startedAt: startedWaitingAt, host, user: currentUser(), cwd: process.cwd() };
  let announced = false;
  let lastSeen: LockHolder | null = null;

  const queue = queueDir(lockPath);
  const ticket = ticketName(startedWaitingAt, pid, options.urgent ?? false);
  const shared = options.shared ?? false;
  try {
    if (shared) ensureDir(dirname(lockPath), true);
    fileTicket(queue, ticket, holder, shared);
  } catch (err) {
    const hint = shared && errorCode(err) === "EACCES" ? sharedDirHint(dirname(lockPath)) + sharedDirHint(queue) : "";
    console.warn(
      `\n⚠ Could not join the gate queue (${err instanceof Error ? err.message : String(err)}).` +
        hint +
        "\n  Running unserialized — concurrent gates on this machine may contend.\n"
    );
    return { held: false, release: () => {} };
  }
  // Leaving the queue is part of every way out of this function; a ticket
  // left behind by a crash is reaped by the next waiter's pid check.
  const leaveQueue = () => removeTicket(queue, ticket);

  // Once per acquisition, not per poll: this is housekeeping for a rare
  // SIGKILL-class death, and a waiter polling every 2s has no reason to
  // re-scan the directory each time.
  sweepOrphanedStaging(lockPath, now());

  for (;;) {
    const ahead = ticketsAhead(queue, ticket, host, isAlive, now(), maxWaitMs);
    if (ahead.length > 0) {
      const waitedMs = now() - startedWaitingAt;
      if (waitedMs >= maxWaitMs) {
        leaveQueue();
        console.warn(formatTimeoutWarning(lastSeen, maxWaitMs));
        return { held: false, release: () => {} };
      }
      const current = readHolder(lockPath);
      if (current.state === "held") lastSeen = current.holder;
      if (!announced) {
        log(`\n⏳ Another ${what} is running on this machine — queueing behind it.`);
        log("   (gates are serialized so they don't starve each other of CPU; see ADR-047)");
        announced = true;
      }
      const holding = current.state === "held" ? current.holder : null;
      status(`queued:${ahead.length}:${holding?.pid ?? ""}`, formatQueuedLine(ahead.length, holding, now(), waitedMs));
      await sleep(pollIntervalMs);
      continue;
    }

    // startedAt is stamped at acquisition, not at first attempt, so the
    // "running Xm" a waiter prints is how long the holder has held the lock
    // rather than how long it has been trying to.
    holder.startedAt = now();
    let created: boolean;
    try {
      created = tryCreateLock(lockPath, holder);
    } catch (err) {
      // The lock is an advisory optimization; it must never be the reason a
      // run cannot happen. A read-only or full tmpdir, an unwritable path —
      // degrade to running unserialized rather than blocking the run on
      // infrastructure that has nothing to do with the tests.
      leaveQueue();
      console.warn(
        `\n⚠ Could not use the gate lock (${err instanceof Error ? err.message : String(err)}).` +
          "\n  Running unserialized — concurrent gates on this machine may contend.\n"
      );
      return { held: false, release: () => {} };
    }
    if (created) {
      leaveQueue();
      if (announced) log("  lock acquired — starting.\n");
      // Idempotent: a run may release early (test:changed's machine slot)
      // and again from its exit handler. A second release could otherwise read the lock
      // just as another gate takes it and remove theirs.
      let released = false;
      return {
        held: true,
        release: () => {
          if (released) return;
          released = true;
          removeLockIfHeldBy(lockPath, holder.pid);
        },
      };
    }

    const current = readHolder(lockPath);

    // Released between our failed create and this read. Nothing to reap —
    // unlinking here would destroy whatever successor has since claimed the
    // path. Just try again.
    if (current.state === "absent") continue;

    // Genuine garbage: a hand-edited file, or one truncated by something
    // outside this module. Nobody can own it, so it must not be allowed to
    // wedge every future run on the machine.
    if (current.state === "unreadable" && removeLock(lockPath)) {
      log("  reclaiming an unreadable gate lock.");
      continue;
    }

    // Someone else's claim we can't read, or garbage we can't remove —
    // waited on like a live holder, bounded by maxWaitMs like a foreign-host
    // one. Nothing here can tell whether its holder is gone, so the line
    // names the file for a person to judge.
    if (current.state === "unreadable" || current.state === "forbidden") {
      const waitedMs = now() - startedWaitingAt;
      if (waitedMs >= maxWaitMs) {
        leaveQueue();
        console.warn(formatTimeoutWarning(lastSeen, maxWaitMs));
        return { held: false, release: () => {} };
      }
      status(
        `waiting:${current.state}`,
        `  waiting on ${lockPath}, which this account can't read or remove — if no run holds it, delete it by hand` +
          ` — waited ${formatDuration(waitedMs)}`
      );
      await sleep(pollIntervalMs);
      continue;
    }

    const holderNow = current.holder;
    lastSeen = holderNow;

    // Reap a lock whose owning process is gone — a killed session must not
    // wedge future runs. A foreign-host lock is never reaped (its pid means
    // nothing here) but is still bounded by maxWaitMs below, so it cannot
    // wedge us either.
    // Re-read immediately before unlinking, and only remove the file if it
    // still names the dead holder we judged. Two waiters can reach this
    // point on the same corpse; without the re-check, the second would
    // unlink a lock the first had already reaped and legitimately retaken.
    // Then loop rather than acquire directly: another waiter may have won
    // the race to recreate it, and tryCreateLock is the only thing allowed
    // to decide who holds it. A corpse we can't remove falls through to the
    // wait below instead of spinning.
    if (!isForeignHost(holderNow, host) && !isAlive(holderNow.pid) && removeLockIfHeldBy(lockPath, holderNow.pid)) {
      log(`  reclaiming a stale gate lock (pid ${holderNow.pid} is gone).`);
      continue;
    }

    const waitedMs = now() - startedWaitingAt;
    if (waitedMs >= maxWaitMs) {
      leaveQueue();
      console.warn(formatTimeoutWarning(lastSeen, maxWaitMs));
      return { held: false, release: () => {} };
    }

    if (!announced) {
      log(`\n⏳ Another ${what} is running on this machine — queueing behind it.`);
      log("   (gates are serialized so they don't starve each other of CPU; see ADR-047)");
      announced = true;
    }
    // A local holder that is gone but whose lock we couldn't remove is not a
    // busy slot: say so, and name the file, rather than report it as holding.
    const corpse = !isForeignHost(holderNow, host) && !isAlive(holderNow.pid);
    status(
      `waiting:${holderNow.pid}`,
      corpse
        ? `  waiting on ${lockPath}: pid ${holderNow.pid} (${holderNow.user}) is gone but this account can't remove its lock — delete it by hand — waited ${formatDuration(waitedMs)}`
        : formatWaitingLine(holderNow, now(), waitedMs)
    );
    await sleep(pollIntervalMs);
  }
}

/**
 * Wires release() to process exit and to the signals a terminal actually
 * sends, so the lock survives none of: a failing step's process.exit(1),
 * Ctrl-C, or a terminal closing. Without this an aborted gate would leave a
 * lock that the next run has to wait out (until its pid check reaps it —
 * which works, but only after that run has already printed a confusing wait).
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
      // to fail the run. Remove only our own listener: if nothing else is
      // subscribed, node restores the default (terminate) behaviour, and if
      // something is, that handler is not ours to cancel.
      process.removeListener(signal, onSignal);
      process.kill(process.pid, signal);
    };
    process.on(signal, onSignal);
  }
}
