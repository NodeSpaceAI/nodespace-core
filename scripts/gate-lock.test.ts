// Covers the machine-wide gate lock that serializes pre-push gates
// (scripts/gate-lock.ts). Concurrent gates on one machine oversubscribe the
// CPU and produce worker/daemon timeouts on correct code; this lock makes
// them queue instead.
//
// These tests use real lockfiles in a temp directory — the mutual exclusion
// being tested IS the filesystem's O_EXCL behaviour, so faking it away would
// leave the actual primitive untested. Clock and sleeping are injected so no
// test waits on wall-clock time.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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

  test("only one of many simultaneous acquires wins — real O_EXCL, not a mock", async () => {
    const results = await Promise.all(
      Array.from({ length: 8 }, () =>
        acquireGateLock(harness({ isAlive: () => true, maxWaitMs: 0 }).options)
      )
    );
    expect(results.filter((r) => r.held)).toHaveLength(1);
    results.find((r) => r.held)?.release();
  });

  test("reclaims a lock whose owning pid is gone — no manual cleanup step", async () => {
    plantLock({ pid: 999_002 });
    const { options, logged } = harness({ isAlive: () => false });

    const lock = await acquireGateLock(options);

    expect(lock.held).toBe(true);
    expect(logged.join("\n")).toContain("reclaiming a stale gate lock");
    expect(parseHolder(readFileSync(lockPath, "utf8"))?.pid).toBe(process.pid);
    lock.release();
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
