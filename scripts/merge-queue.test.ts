// Covers the team-wide merge queue's refs (scripts/merge-queue.ts). The lock
// is only as good as `git push --force-with-lease` being an atomic
// compare-and-swap on the server, so these tests push to a real bare
// repository standing in for origin, from two clones standing in for two
// machines — faking git away would leave the primitive untested.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { $ } from "bun";
import {
  bisectBatch,
  describeLock,
  type LockInfo,
  MergeQueue,
  parseLockInfo,
  parseLsRemote,
  prFromQueueRef,
  QUEUE_PREFIX,
  StaleWatch,
} from "./merge-queue";

let dir: string;
let machineA: string;
let machineB: string;
let head: string;

async function clone(name: string): Promise<string> {
  const path = join(dir, name);
  await $`git clone --quiet ${join(dir, "origin.git")} ${path}`.quiet();
  await $`git -C ${path} config user.email t@example.com`.quiet();
  await $`git -C ${path} config user.name t`.quiet();
  return path;
}

beforeEach(async () => {
  dir = mkdtempSync(join(tmpdir(), "merge-queue-test-"));
  await $`git init --quiet --bare ${join(dir, "origin.git")}`.quiet();
  machineA = await clone("a");
  await $`git -C ${machineA} commit --quiet --allow-empty -m init`.quiet();
  await $`git -C ${machineA} push --quiet origin HEAD:main`.quiet();
  head = (await $`git -C ${machineA} rev-parse HEAD`.quiet().text()).trim();
  machineB = await clone("b");
});

afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

const info = (host: string, prs: number[] = []): LockInfo => ({ host, user: "u", pid: 1, startedAt: 0, prs });

describe("the queue", () => {
  test("lists queued PRs oldest first, from any machine", async () => {
    await new MergeQueue(machineA).enqueue(3212, head);
    await new MergeQueue(machineB).enqueue(3190, head);
    expect(await new MergeQueue(machineA).queued()).toEqual([3190, 3212]);
    expect(await new MergeQueue(machineB).isQueued(3212)).toBe(true);
  });

  test("dequeue removes a PR, and is idempotent", async () => {
    const q = new MergeQueue(machineA);
    await q.enqueue(1, head);
    await q.dequeue(1);
    await q.dequeue(1);
    expect(await q.queued()).toEqual([]);
    expect(await q.isQueued(1)).toBe(false);
  });
});

describe("the lock", () => {
  test("of two machines taking a free lock, exactly one wins", async () => {
    const [a, b] = await Promise.all([
      new MergeQueue(machineA).tryAcquire(info("a")),
      new MergeQueue(machineB).tryAcquire(info("b")),
    ]);
    expect([a, b].filter((sha) => sha !== null)).toHaveLength(1);
  });

  test("a held lock can't be taken, and says who holds it", async () => {
    const a = new MergeQueue(machineA);
    const held = await a.tryAcquire(info("a", [7, 9]));
    expect(held).not.toBeNull();
    const b = new MergeQueue(machineB);
    expect(await b.tryAcquire(info("b"))).toBeNull();
    const sha = await b.lockSha();
    expect(sha).toBe(held);
    expect(await b.lockInfo(sha as string)).toEqual(info("a", [7, 9]));
  });

  test("a heartbeat moves the lock; a holder that was taken over can't renew", async () => {
    const a = new MergeQueue(machineA);
    const b = new MergeQueue(machineB);
    const first = (await a.tryAcquire(info("a"))) as string;
    const second = await a.renew(first, info("a"));
    expect(second).not.toBeNull();
    expect(second).not.toBe(first);

    // B judged it stale at `second` and takes it over.
    expect(await b.tryAcquire(info("b"), second as string)).not.toBeNull();
    expect(await a.renew(second as string, info("a"))).toBeNull();
  });

  test("a takeover only succeeds against the sha judged stale", async () => {
    const a = new MergeQueue(machineA);
    const first = (await a.tryAcquire(info("a"))) as string;
    await a.renew(first, info("a"));
    // B's view is out of date: the holder heartbeated since.
    expect(await new MergeQueue(machineB).tryAcquire(info("b"), first)).toBeNull();
  });

  test("release frees the lock, but only for its holder", async () => {
    const a = new MergeQueue(machineA);
    const b = new MergeQueue(machineB);
    const held = (await a.tryAcquire(info("a"))) as string;
    await b.release("0".repeat(40));
    expect(await b.lockSha()).toBe(held);
    await a.release(held);
    expect(await b.lockSha()).toBeNull();
    expect(await b.tryAcquire(info("b"))).not.toBeNull();
  });
});

describe("StaleWatch", () => {
  test("a lock unchanged for the whole window is stale", () => {
    let t = 0;
    const watch = new StaleWatch(1000, () => t);
    expect(watch.observe("aaa")).toBe(false);
    t = 999;
    expect(watch.observe("aaa")).toBe(false);
    t = 1000;
    expect(watch.observe("aaa")).toBe(true);
  });

  test("a heartbeat restarts the window, and a free lock is never stale", () => {
    let t = 0;
    const watch = new StaleWatch(1000, () => t);
    watch.observe("aaa");
    t = 900;
    watch.observe("bbb");
    t = 1500;
    expect(watch.observe("bbb")).toBe(false);
    t = 5000;
    expect(watch.observe(null)).toBe(false);
    t = 9000;
    expect(watch.observe(null)).toBe(false);
  });
});

describe("bisectBatch", () => {
  test("retests the first half of a failed batch, down to a single PR", () => {
    expect(bisectBatch([1, 2, 3, 4, 5])).toEqual([1, 2]);
    expect(bisectBatch([1, 2])).toEqual([1]);
    expect(bisectBatch([1])).toEqual([1]);
  });
});

describe("parsing", () => {
  test("parseLsRemote keeps only well-formed lines", () => {
    const sha = "a".repeat(40);
    expect(parseLsRemote(`${sha}\trefs/x\n\njunk\n`)).toEqual(new Map([["refs/x", sha]]));
  });

  test("prFromQueueRef reads the PR number and nothing else", () => {
    expect(prFromQueueRef(`${QUEUE_PREFIX}3212`)).toBe(3212);
    expect(prFromQueueRef(`${QUEUE_PREFIX}abc`)).toBeNull();
    expect(prFromQueueRef("refs/heads/main")).toBeNull();
  });

  test("a lock record round-trips, and junk is not one", () => {
    const record = info("mini", [1, 2]);
    expect(parseLockInfo(JSON.stringify(record))).toEqual(record);
    expect(parseLockInfo("not json")).toBeNull();
    expect(parseLockInfo('{"host":"x"}')).toBeNull();
    expect(describeLock(record)).toBe("u@mini (pid 1) on #1, #2");
  });
});
