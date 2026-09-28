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
import { GATE_INFRA_EXIT } from "./gate-stage";
import {
  bisectBatch,
  changesDependencies,
  changesGate,
  describeLock,
  fenced,
  gateVerdict,
  type LandEntry,
  type Lander,
  landStack,
  type LockInfo,
  MergeQueue,
  parseLockInfo,
  parseLsRemote,
  prFromQueueRef,
  QUEUE_PREFIX,
  StaleWatch,
  stripAnsi,
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
    const renewed = await a.renew(first, info("a"));
    if (renewed.status !== "renewed") throw new Error(`expected a renewal, got ${renewed.status}`);
    expect(renewed.sha).not.toBe(first);

    // B judged it stale at `renewed.sha` and takes it over.
    expect(await b.tryAcquire(info("b"), renewed.sha)).not.toBeNull();
    expect(await a.renew(renewed.sha, info("a"))).toEqual({ status: "lost" });
  });

  test("a push that fails while the lock still reads as ours is unknown, not lost", async () => {
    const a = new MergeQueue(machineA);
    const held = (await a.tryAcquire(info("a"))) as string;
    // An unreachable remote: the push fails for a reason that isn't the lock moving.
    await $`git -C ${machineA} remote add broken ${join(dir, "no-such-remote.git")}`.quiet();
    expect(await new MergeQueue(machineA, "broken").renew(held, info("a"))).toEqual({ status: "unknown" });
    // The real lock is untouched.
    expect(await a.lockSha()).toBe(held);
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

describe("landStack", () => {
  const T0 = "tree-main";
  const entry = (pr: number): LandEntry => ({ pr, headRefName: `b${pr}`, head: `head${pr}`, commits: [`c${pr}`], tree: `tree${pr}` });

  /**
   * A fake origin and GitHub where everything goes right: main advances to a
   * PR's tree when it lands, and every check passes. `overrides` breaks one thing.
   */
  function fakeLander(overrides: Partial<Lander> = {}) {
    let mainTree = T0;
    const log: string[] = [];
    const lander: Lander = {
      confirmHolding: async () => (log.push("confirm"), true),
      isQueued: async () => true,
      main: async () => ({ sha: `sha-${mainTree}`, tree: mainTree }),
      headOf: async (branch) => `head${branch.slice(1)}`,
      replayOnto: async (_main, commits) => ({ tree: `tree${commits[0].slice(1)}`, tip: `tip${commits[0].slice(1)}` }),
      push: async (branch) => (log.push(`push ${branch}`), true),
      merge: async (pr) => (log.push(`merge #${pr}`), { ok: true }),
      landed: async (pr) => {
        log.push(`landed #${pr}`);
        mainTree = `tree${pr}`;
      },
      eject: async (pr) => void log.push(`eject #${pr}`),
      ...overrides,
    };
    return { lander, log };
  }

  test("lands every PR in order, confirming the lock before each push and each merge", async () => {
    const { lander, log } = fakeLander();
    expect(await landStack([entry(1), entry(2)], T0, lander)).toEqual({ landed: [1, 2] });
    expect(log).toEqual([
      "confirm", "push b1", "confirm", "merge #1", "landed #1",
      "confirm", "push b2", "confirm", "merge #2", "landed #2",
    ]);
  });

  test("lands nothing when main moved outside the queue", async () => {
    const { lander, log } = fakeLander({ main: async () => ({ sha: "x", tree: "someone-pushed" }) });
    expect(await landStack([entry(1)], T0, lander)).toEqual({ landed: [], stopped: "main moved outside the queue" });
    expect(log).toEqual([]);
  });

  test("a holder that can't confirm the lock rewrites no branch and merges nothing", async () => {
    const { lander, log } = fakeLander({ confirmHolding: async () => false });
    const outcome = await landStack([entry(1), entry(2)], T0, lander);
    expect(outcome.landed).toEqual([]);
    expect(outcome.stopped).toContain("lock");
    expect(log.filter((l) => l.startsWith("push") || l.startsWith("merge"))).toEqual([]);
  });

  test("ejects a PR GitHub definitely won't merge, keeping what landed before it and leaving the rest queued", async () => {
    const { lander, log } = fakeLander();
    const outcome = await landStack([entry(1), entry(2), entry(3)], T0, {
      ...lander,
      merge: async (pr) => (pr === 2 ? { ok: false, reason: "it's a draft", definite: true } : lander.merge(pr, "t")),
    });
    expect(outcome).toEqual({ landed: [1], stopped: "GitHub won't merge #2", ejected: 2 });
    expect(log).toContain("eject #2");
    expect(log).not.toContain("push b3");
  });

  test("a refusal that says nothing about the PR stops landing without ejecting it", async () => {
    const { lander, log } = fakeLander({ merge: async () => ({ ok: false, reason: "Base branch was modified", definite: false }) });
    const outcome = await landStack([entry(1), entry(2)], T0, lander);
    expect(outcome.landed).toEqual([]);
    expect(outcome.ejected).toBeUndefined();
    expect(outcome.stopped).toContain("Base branch was modified");
    expect(log.some((l) => l.startsWith("eject"))).toBe(false);
  });

  test.each([
    ["the PR left the queue", { isQueued: async () => false }, "left the queue"],
    ["its head moved", { headOf: async () => "someone-pushed" }, "head moved"],
    ["the replay doesn't reproduce the tested tree", { replayOnto: async () => ({ tree: "other", tip: "t" }) }, "tested tree"],
    ["the branch push is refused", { push: async () => false }, "head moved"],
  ] as [string, Partial<Lander>, string][])("stops without merging when %s", async (_label, override, reason) => {
    const { lander, log } = fakeLander(override);
    const outcome = await landStack([entry(1)], T0, lander);
    expect(outcome.landed).toEqual([]);
    expect(outcome.stopped).toContain(reason);
    expect(log.some((l) => l.startsWith("merge"))).toBe(false);
  });

  test("a PR already based on main is merged without a push", async () => {
    const { lander, log } = fakeLander({ replayOnto: async () => ({ tree: "tree1", tip: "head1" }) });
    expect((await landStack([entry(1)], T0, lander)).landed).toEqual([1]);
    expect(log).not.toContain("push b1");
  });
});

describe("gateVerdict", () => {
  test("the gate's infra code, or a kill by signal, is this machine's fault", () => {
    expect(gateVerdict(0)).toBe("passed");
    expect(gateVerdict(1)).toBe("failed");
    expect(gateVerdict(GATE_INFRA_EXIT)).toBe("infra");
    expect(gateVerdict(null)).toBe("infra");
  });

  test("a stack that changes the gate's own scripts owns an infra exit, so it's bisected out", () => {
    expect(gateVerdict(GATE_INFRA_EXIT, true)).toBe("failed");
    expect(gateVerdict(null, true)).toBe("failed");
    expect(gateVerdict(0, true)).toBe("passed");
  });
});

describe("changesGate", () => {
  test("covers the gate's scripts and nothing else", () => {
    expect(changesGate(["scripts/test-gate.ts"])).toBe(true);
    expect(changesGate(["scripts/gate-stage.ts"])).toBe(true);
    expect(changesGate(["scripts/gate-lock.test.ts"])).toBe(false);
    expect(changesGate(["scripts/merge-pr.ts", "packages/core/src/lib.rs"])).toBe(false);
  });
});

describe("changesDependencies", () => {
  test("a stack can break bun install only by changing what it reads", () => {
    expect(changesDependencies(["packages/desktop-app/package.json"])).toBe(true);
    expect(changesDependencies(["bun.lock"])).toBe(true);
    expect(changesDependencies(["packages/core/src/lib.rs", "scripts/merge-pr.ts"])).toBe(false);
    expect(changesDependencies(["docs/package.json.md"])).toBe(false);
  });
});

describe("eject comment formatting", () => {
  test("a fence is longer than any backtick run in the output, so it can't be closed early", () => {
    expect(fenced("plain")).toBe("```\nplain\n```");
    expect(fenced("has ``` inside and ````` too")).toMatch(/^``````\n[\s\S]*\n``````$/);
  });

  test("terminal colour codes are stripped", () => {
    const esc = String.fromCharCode(27);
    expect(stripAnsi(`${esc}[31m✗ failed${esc}[0m ${esc}[2K`)).toBe("✗ failed ");
  });
});
