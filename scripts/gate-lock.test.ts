// Covers the machine-wide gate lock that serializes pre-push gates
// (scripts/gate-lock.ts). Concurrent gates on one machine oversubscribe the
// CPU and produce worker/daemon timeouts on correct code; this lock makes
// them queue instead.
//
// These tests use real lockfiles in a temp directory — the mutual exclusion
// being tested IS the filesystem's atomic link(2) behaviour, so faking it away
// would leave the actual primitive untested. Clock and sleeping are injected
// so no test waits on wall-clock time.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  acquireGateLock,
  DISABLE_ENV_VAR,
  errorCode,
  formatDuration,
  formatTimeoutWarning,
  formatWaitingLine,
  isForeignHost,
  isPidAlive,
  parseHolder,
  readHolder,
  registerLockRelease,
  serializeHolder,
  type LockHolder,
} from "./gate-lock";

let dir: string;
let lockPath: string;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "gate-lock-test-"));
  lockPath = join(dir, "gate.lock");
  delete process.env[DISABLE_ENV_VAR];
});

afterEach(() => {
  delete process.env[DISABLE_ENV_VAR];
  rmSync(dir, { recursive: true, force: true });
});

const HOST = "test-host";

/** Throws a syscall-shaped error, the way node's fs/process APIs do. */
function throwErrno(message: string, code: string): never {
  throw Object.assign(new Error(message), { code });
}

describe("errorCode", () => {
  test("reads the errno string off a syscall error", () => {
    expect(errorCode(Object.assign(new Error("exists"), { code: "EEXIST" }))).toBe("EEXIST");
  });

  test.each([
    ["a plain Error with no code", new Error("boom")],
    ["a non-numeric-coded object", { code: 42 }],
    ["null", null],
    ["a string", "EEXIST"],
  ])("returns empty for %s, so it never matches a real errno by accident", (_label, value) => {
    expect(errorCode(value)).toBe("");
  });
});

function holderFile(overrides: Partial<LockHolder> = {}): LockHolder {
  return { pid: 999_001, startedAt: 1000, host: HOST, cwd: "/tmp/other-worktree", ...overrides };
}

/** Puts a lockfile in place as if another gate had acquired it. */
function plantLock(overrides: Partial<LockHolder> = {}): LockHolder {
  const holder = holderFile(overrides);
  writeFileSync(lockPath, serializeHolder(holder));
  return holder;
}

/** Test harness: fixed host, alive-by-default pid check, captured log. */
function harness(extra: Parameters<typeof acquireGateLock>[0] = {}) {
  const logged: string[] = [];
  return {
    logged,
    options: {
      lockPath,
      host: HOST,
      pollIntervalMs: 0,
      sleep: async () => {},
      log: (m: string) => logged.push(m),
      ...extra,
    },
  };
}

describe("parseHolder", () => {
  test("round-trips a serialized holder", () => {
    const holder = holderFile();
    expect(parseHolder(serializeHolder(holder))).toEqual(holder);
  });

  test.each([
    ["truncated mid-write JSON", '{"pid":123,"star'],
    ["hand-edited junk", "not json at all"],
    ["an empty file", ""],
    ["a JSON array rather than an object", "[1,2,3]"],
    ["JSON null", "null"],
    ["a missing pid", '{"startedAt":1,"host":"h","cwd":"/x"}'],
    ["a non-integer pid", '{"pid":1.5,"startedAt":1,"host":"h","cwd":"/x"}'],
    ["a zero pid", '{"pid":0,"startedAt":1,"host":"h","cwd":"/x"}'],
    ["a negative pid", '{"pid":-4,"startedAt":1,"host":"h","cwd":"/x"}'],
    ["a non-numeric startedAt", '{"pid":1,"startedAt":"soon","host":"h","cwd":"/x"}'],
    ["a missing host", '{"pid":1,"startedAt":1,"cwd":"/x"}'],
  ])("returns null for %s, so it can be reaped rather than wedging pushes", (_label, raw) => {
    expect(parseHolder(raw)).toBeNull();
  });
});

describe("readHolder", () => {
  // These three states must stay distinct: an absent lock means "retry the
  // create", while an unreadable one means "reap it". Collapsing them (as an
  // earlier version did, into a single null) made waiters unlink locks that
  // had simply been released — and made the logs claim corruption that never
  // happened.
  test("reports absent when nothing is at the path", () => {
    expect(readHolder(lockPath)).toEqual({ state: "absent" });
  });

  test("reports the holder when the file is a valid lock", () => {
    const holder = plantLock({ pid: 4242 });
    expect(readHolder(lockPath)).toEqual({ state: "held", holder });
  });

  test("reports unreadable for a truncated or hand-edited file, distinctly from absent", () => {
    writeFileSync(lockPath, '{"pid":123,"star');
    expect(readHolder(lockPath)).toEqual({ state: "unreadable" });
  });

  test("reports unreadable for an empty file — the window an atomic create must prevent", () => {
    writeFileSync(lockPath, "");
    expect(readHolder(lockPath)).toEqual({ state: "unreadable" });
  });
});

describe("isPidAlive", () => {
  test("reports this very process as alive", () => {
    expect(isPidAlive(process.pid)).toBe(true);
  });

  test("reports a dead pid as not alive (ESRCH)", () => {
    expect(isPidAlive(999_999, () => throwErrno("no such process", "ESRCH"))).toBe(false);
  });

  test("treats EPERM as alive — another user's process still holds the lock", () => {
    expect(isPidAlive(1, () => throwErrno("not permitted", "EPERM"))).toBe(true);
  });
});

describe("isForeignHost", () => {
  test("is false for a lock written by this machine", () => {
    expect(isForeignHost(holderFile({ host: "mine" }), "mine")).toBe(false);
  });

  test("is true for another machine, whose pids mean nothing here", () => {
    expect(isForeignHost(holderFile({ host: "elsewhere" }), "mine")).toBe(true);
  });
});

describe("formatDuration", () => {
  test.each([
    [0, "0s"],
    [1500, "1s"],
    [59_000, "59s"],
    [60_000, "1m00s"],
    [252_000, "4m12s"],
    [3_600_000, "60m00s"],
  ])("formats %ims as %s", (ms, expected) => {
    expect(formatDuration(ms)).toBe(expected);
  });

  test("clamps a negative span to 0s rather than printing '-1s'", () => {
    expect(formatDuration(-5000)).toBe("0s");
  });
});

describe("waiting output", () => {
  test("names the holder's pid, how long it has held, and how long we have waited", () => {
    const line = formatWaitingLine(holderFile({ pid: 4242, startedAt: 1_000_000 }), 1_252_000, 30_000);
    expect(line).toContain("pid 4242");
    expect(line).toContain("running 4m12s");
    expect(line).toContain("waited 30s");
  });

  test("the timeout warning says it is proceeding anyway and why that is risky", () => {
    const warning = formatTimeoutWarning(holderFile({ pid: 77 }), 1_800_000);
    expect(warning).toContain("pid 77");
    expect(warning).toContain("30m00s");
    expect(warning).toMatch(/running anyway/i);
  });

  test("the timeout warning degrades gracefully when the holder was never readable", () => {
    expect(formatTimeoutWarning(null, 60_000)).toContain("the holder");
  });
});

describe("acquireGateLock", () => {
  test("acquires an uncontended lock immediately and writes its own identity", async () => {
    const { options, logged } = harness();
    const lock = await acquireGateLock(options);

    expect(lock.held).toBe(true);
    expect(existsSync(lockPath)).toBe(true);
    const written = parseHolder(readFileSync(lockPath, "utf8"));
    expect(written?.pid).toBe(process.pid);
    expect(written?.host).toBe(HOST);
    // The uncontended path is the common case — it must print nothing.
    expect(logged).toEqual([]);

    lock.release();
    expect(existsSync(lockPath)).toBe(false);
  });

  test("a second acquire blocks while the first is held, then succeeds after release", async () => {
    const { options: firstOpts } = harness();
    const first = await acquireGateLock(firstOpts);
    expect(first.held).toBe(true);

    // The holder is this very process, so it is genuinely alive — the
    // second acquire must queue rather than reap it.
    let polls = 0;
    const { options, logged } = harness({
      isAlive: () => true,
      sleep: async () => {
        polls += 1;
        // Release on the third poll, standing in for the first gate finishing.
        if (polls === 3) first.release();
      },
    });

    const second = await acquireGateLock(options);
    expect(second.held).toBe(true);
    expect(polls).toBe(3);
    expect(logged.join("\n")).toContain("queueing behind it");
    expect(logged.join("\n")).toContain("lock acquired");
    // The waiting line repeats as it waits, so the push never looks hung.
    expect(logged.filter((l) => l.includes("waiting for another gate")).length).toBe(3);

    second.release();
  });

  test(
    "no two REAL processes ever hold the lock at once",
    async () => {
      // The invariant that matters, tested the only way it can be: across
      // actual OS processes. An in-process version is tautological — the
      // acquire path's syscalls are synchronous, so several callers on one
      // event loop cannot interleave inside them, and such a test passes even
      // when cross-process exclusion is completely broken.
      //
      // What this guards against: an earlier implementation published the
      // lockfile with `open(path, "wx")` then `write`, leaving a window in
      // which the file existed but was empty. A waiter reading in that window
      // saw no holder, judged the lock garbage, reaped it out from under the
      // live owner, and took it — two gates, both believing they held it.
      //
      // Honest limitation: that window is sub-millisecond, and six processes
      // do not reliably land inside it, so this test does NOT dependably fail
      // against that specific bug on its own. Its value was established by
      // widening the window artificially (a 5ms spin at the publish point):
      // with the two-syscall implementation the test then failed on
      // overlapping held-intervals, and with the current write-then-link
      // implementation it still passed — because a staged file is not
      // reachable at the lock path until `link(2)` publishes it whole, so
      // there is no window to widen. The assertion is therefore a real
      // detector of overlapping holds; what is probabilistic is only whether
      // a given interleaving arises.
      // A shared start instant, far enough out that every worker has finished
      // booting and is spinning on the barrier. Without it their staggered
      // startup serializes them by accident and the race never occurs.
      const startAt = Date.now() + 2500;
      const workers = Array.from({ length: 6 }, () =>
        Bun.spawn(
          [
            "bun",
            "run",
            join(import.meta.dir, "gate-lock.race-worker.ts"),
            lockPath,
            "120",
            String(startAt),
          ],
          { stdout: "pipe", stderr: "pipe" }
        )
      );

      const outputs = await Promise.all(workers.map((w) => Bun.readableStreamToText(w.stdout)));
      await Promise.all(workers.map((w) => w.exited));

      const intervals = outputs.map((out, i) => {
        const held = out.match(/^HELD \d+ (\d+)$/m);
        const done = out.match(/^DONE \d+ (\d+)$/m);
        if (!held || !done) throw new Error(`worker ${i} did not acquire the lock:\n${out}`);
        return { start: Number(held[1]), end: Number(done[1]) };
      });

      // Every worker got the lock (none timed out), and no two held windows
      // overlap. Sorting by start makes the check a simple neighbour scan.
      expect(intervals).toHaveLength(6);
      intervals.sort((a, b) => a.start - b.start);
      for (let i = 1; i < intervals.length; i++) {
        const previous = intervals[i - 1]!;
        const next = intervals[i]!;
        expect(next.start).toBeGreaterThanOrEqual(previous.end);
      }
    },
    // Six workers × ~120ms of held time, plus bun startup per process.
    30_000
  );

  test("reclaims a lock whose owning pid is gone — no manual cleanup step", async () => {
    plantLock({ pid: 999_002 });
    const { options, logged } = harness({ isAlive: () => false });

    const lock = await acquireGateLock(options);

    expect(lock.held).toBe(true);
    expect(logged.join("\n")).toContain("reclaiming a stale gate lock");
    expect(parseHolder(readFileSync(lockPath, "utf8"))?.pid).toBe(process.pid);
    lock.release();
  });

  test("does not unlink when the lock vanished mid-poll — it retries the create instead", async () => {
    // The holder released between our failed create and our read. There is
    // nothing to reap, and unlinking blind here would destroy whatever
    // successor claimed the path in the meantime.
    let firstAttempt = true;
    const { options, logged } = harness({
      isAlive: () => true,
      sleep: async () => {
        throw new Error("must not sleep — an absent lock should retry immediately");
      },
    });
    // Occupy the path for exactly one create attempt, then clear it.
    plantLock();
    const lock = await acquireGateLock({
      ...options,
      now: () => {
        if (firstAttempt) {
          firstAttempt = false;
          rmSync(lockPath, { force: true });
        }
        return 0;
      },
    });

    expect(lock.held).toBe(true);
    expect(logged.join("\n")).not.toContain("reclaiming");
    lock.release();
  });

  test("sweeps staging files orphaned by a SIGKILLed acquirer, but spares fresh ones", async () => {
    // tryCreateLock's finally covers every exit the process survives, but not
    // a SIGKILL between writing the staged file and publishing it. Nothing
    // else collects those, so they would accumulate in tmpdir forever.
    const orphan = `${lockPath}.99999.dead-beef`;
    const fresh = `${lockPath}.99998.still-working`;
    writeFileSync(orphan, "{}");
    writeFileSync(fresh, "{}");
    // Age the orphan past the threshold; leave the other at "now".
    const longAgo = new Date(Date.now() - 10 * 60_000);
    utimesSync(orphan, longAgo, longAgo);

    const lock = await acquireGateLock(harness().options);

    expect(existsSync(orphan)).toBe(false);
    // Critical: a staging file a live process is mid-publish on must survive.
    expect(existsSync(fresh)).toBe(true);
    lock.release();
  });

  test("the sweep never touches the lock itself, only its staging siblings", async () => {
    const held = plantLock({ pid: 999_007 });
    const ancient = new Date(Date.now() - 10 * 60_000);
    utimesSync(lockPath, ancient, ancient);

    // An old lock is stale-reaped by liveness, never by the age sweep — so
    // with its owner reported alive it must survive untouched.
    const { options } = harness({
      isAlive: () => true,
      maxWaitMs: 0,
      now: () => 0,
    });
    const lock = await acquireGateLock(options);

    expect(lock.held).toBe(false);
    expect(parseHolder(readFileSync(lockPath, "utf8"))?.pid).toBe(held.pid);
  });

  test("reclaims an unparseable lockfile rather than waiting it out forever", async () => {
    writeFileSync(lockPath, '{"pid":123,"star');
    const { options, logged } = harness({
      isAlive: () => {
        throw new Error("liveness must not be consulted for an unreadable lock");
      },
    });

    const lock = await acquireGateLock(options);

    expect(lock.held).toBe(true);
    expect(logged.join("\n")).toContain("reclaiming an unreadable gate lock");
    lock.release();
  });

  test("never reaps a lock from another machine, whose pid is meaningless here", async () => {
    plantLock({ host: "some-other-machine" });
    const { options } = harness({
      maxWaitMs: 10,
      now: (() => {
        // 0, then past the deadline, so it gives up without real waiting.
        const stamps = [0, 0, 999];
        let i = 0;
        return () => stamps[Math.min(i++, stamps.length - 1)] ?? 999;
      })(),
      isAlive: () => {
        throw new Error("liveness must not be consulted for a foreign-host lock");
      },
    });

    const lock = await acquireGateLock(options);

    // Gave up rather than stealing it, and left the foreign lock in place.
    expect(lock.held).toBe(false);
    expect(parseHolder(readFileSync(lockPath, "utf8"))?.host).toBe("some-other-machine");
  });

  test("gives up after maxWaitMs and runs unserialized rather than blocking pushes forever", async () => {
    plantLock();
    let clock = 0;
    const { options } = harness({
      isAlive: () => true,
      maxWaitMs: 5000,
      now: () => clock,
      sleep: async () => {
        clock += 2000;
      },
    });

    const lock = await acquireGateLock(options);

    expect(lock.held).toBe(false);
    // The other gate's lock is untouched — we proceeded, we did not steal it.
    expect(parseHolder(readFileSync(lockPath, "utf8"))?.pid).toBe(999_001);
    // release() on a non-held lock is a safe no-op, never deleting the holder's file.
    lock.release();
    expect(existsSync(lockPath)).toBe(true);
  });

  test("degrades to running unserialized when the lock path is unusable, rather than blocking the push", async () => {
    // A directory that does not exist — stands in for a read-only or full
    // tmpdir. The lock is advisory; it must never be why a push cannot happen.
    const { options } = harness({ lockPath: join(dir, "no-such-dir", "gate.lock") });

    const lock = await acquireGateLock(options);

    expect(lock.held).toBe(false);
    lock.release();
  });

  test(`${DISABLE_ENV_VAR} skips locking entirely for a knowingly-parallel gate`, async () => {
    process.env[DISABLE_ENV_VAR] = "1";
    plantLock();
    const { options, logged } = harness({
      isAlive: () => true,
      sleep: async () => {
        throw new Error("must not wait when locking is disabled");
      },
    });

    const lock = await acquireGateLock(options);

    expect(lock.held).toBe(false);
    expect(logged.join("\n")).toContain(DISABLE_ENV_VAR);
    lock.release();
    // The escape hatch must not delete someone else's live lock.
    expect(existsSync(lockPath)).toBe(true);
  });

  test("release() is safe to call twice — the second is a no-op, not a steal", async () => {
    const { options } = harness();
    const lock = await acquireGateLock(options);
    lock.release();

    // Another gate takes the lock in between.
    plantLock({ pid: 999_003 });
    lock.release();

    expect(parseHolder(readFileSync(lockPath, "utf8"))?.pid).toBe(999_003);
  });

  test("release() does not delete a successor's lock after we were reaped as stale", async () => {
    const { options } = harness();
    const lock = await acquireGateLock(options);

    // A waiter decided we were gone, reaped our lock, and took it. Our
    // eventual release must not hand the machine to two gates at once.
    plantLock({ pid: 999_004 });
    lock.release();

    expect(parseHolder(readFileSync(lockPath, "utf8"))?.pid).toBe(999_004);
  });
});

describe("registerLockRelease", () => {
  test("releases on process exit, and only once", () => {
    let releases = 0;
    registerLockRelease({ held: true, release: () => (releases += 1) });

    // Invoke the handler the way the runtime would, without exiting the test run.
    process.emit("exit", 0);
    process.emit("exit", 0);
    expect(releases).toBe(1);
  });

  test("registers nothing when the lock is not held", () => {
    const before = process.listenerCount("exit");
    registerLockRelease({ held: false, release: () => {} });
    expect(process.listenerCount("exit")).toBe(before);
  });
});
