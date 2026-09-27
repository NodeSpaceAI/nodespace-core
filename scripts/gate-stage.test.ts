// Covers the stage runner the merge gate and test:changed share
// (scripts/gate-stage.ts): above all, that a hung stage is killed at its
// timeout with its whole process tree, so it can't hold the machine slot or
// the CPU after the gate gives up on it.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { descendantPids, runStage } from "./gate-stage";

let dir: string;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "gate-stage-test-"));
});

afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

describe("descendantPids", () => {
  test("finds children and grandchildren, and nothing outside the tree", () => {
    const ps = ["1 0", "10 1", "11 10", "12 11", "13 10", "20 1", "21 20"].join("\n");
    expect(descendantPids(ps, 10).sort()).toEqual([11, 12, 13]);
  });

  test("skips blank and malformed lines", () => {
    expect(descendantPids("\n  10 1\n  junk\n  11 10\n", 10)).toEqual([11]);
  });
});

describe("runStage", () => {
  test("a passing stage returns true and writes its output to the log", async () => {
    const ok = await runStage({ label: "echo stage", command: "echo hello", timeoutMs: 10_000 }, dir);
    expect(ok).toBe(true);
    expect(readFileSync(join(dir, "echo-stage.log"), "utf8")).toContain("hello");
  });

  test("a failing stage returns false", async () => {
    expect(await runStage({ label: "fail", command: "exit 3", timeoutMs: 10_000 }, dir)).toBe(false);
  });

  test("a nice'd stage still runs its command", async () => {
    expect(await runStage({ label: "nice", command: "true", timeoutMs: 10_000, nice: true }, dir)).toBe(true);
  });

  test("a hung stage fails at its timeout, and its grandchildren die with it", async () => {
    const pidFile = join(dir, "grandchild.pid");
    // A background grandchild, like a vitest fork or a nextest test process,
    // plus a foreground wait that never ends on its own.
    const command = `sh -c 'sleep 300 & echo $! > ${pidFile}; wait'`;
    const started = Date.now();

    const ok = await runStage({ label: "hang", command, timeoutMs: 500 }, dir);

    expect(ok).toBe(false);
    expect(Date.now() - started).toBeLessThan(15_000);
    const grandchild = Number(readFileSync(pidFile, "utf8").trim());
    expect(Number.isInteger(grandchild)).toBe(true);
    // Killed asynchronously after the stage's exit resolves; give it a moment.
    await Bun.sleep(200);
    expect(isAlive(grandchild)).toBe(false);
  }, 20_000);
});
