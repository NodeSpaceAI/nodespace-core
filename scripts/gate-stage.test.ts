// Covers the stage runner the merge gate and test:changed share
// (scripts/gate-stage.ts): above all, that a hung stage is killed at its
// timeout with its whole process tree, so it can't hold the machine slot or
// the CPU after the gate gives up on it.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { descendantPids, killActiveStages, runStage, TIERS } from "./gate-stage";

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

  test("a grandchild cleaning up after SIGTERM gets the grace period, not an instant SIGKILL", async () => {
    const marker = join(dir, "cleaned-up");
    const script = join(dir, "slow-cleanup.sh");
    // Traps SIGTERM, takes half a second to "clean up", then records that it
    // finished. `sleep & wait` keeps the trap responsive while idle.
    writeFileSync(script, `trap 'sleep 0.5; touch ${marker}; exit 0' TERM\nsleep 300 &\nwait\n`);
    const command = `sh ${script} & wait`;

    expect(await runStage({ label: "slow cleanup", command, timeoutMs: 500 }, dir)).toBe(false);
    expect(existsSync(marker)).toBe(true);
  }, 20_000);
});

describe("the merge gate's clippy stage", () => {
  const gate = readFileSync(join(import.meta.dir, "test-gate.ts"), "utf8");
  // The call as a statement of its own: a commented-out one doesn't match.
  const stage = gate.search(/^\s*await run\(TIERS\.rustLint\);$/m);

  test("runs clippy over the whole workspace with warnings as errors", () => {
    // The version first, so a failing stage's log shows which clippy ran.
    expect(TIERS.rustLint.command).toBe("cargo clippy --version && bun run rust:lint");
    const scripts = JSON.parse(readFileSync(join(import.meta.dir, "..", "package.json"), "utf8")).scripts;
    const [workspace, types] = scripts["rust:lint"].split("&&");
    expect(workspace).toContain("cargo clippy --all-targets");
    // No package filter: an error in any crate fails the stage.
    expect(/\s(-p|--package|--exclude)\s/.test(workspace)).toBe(false);
    expect(types).toContain("cargo clippy -p nodespace-types --all-targets --features ts");
    for (const half of [workspace, types]) expect(half).toContain("-D warnings");
  });

  test("is in merge mode only, and under the machine slot", () => {
    expect(stage).toBeGreaterThan(-1);
    // A push exits before it, so it stays lint-of-scripts only and takes seconds.
    expect(stage).toBeGreaterThan(gate.indexOf('console.log("\\n✓ Push check passed'));
    expect(gate.indexOf('console.log("\\n✓ Push check passed')).toBeGreaterThan(-1);
    expect(stage).toBeGreaterThan(gate.indexOf("registerLockRelease(machineSlot)"));
    expect(gate.indexOf("registerLockRelease(machineSlot)")).toBeGreaterThan(-1);
  });

  test("names itself, so a failing gate says which stage failed", () => {
    expect(TIERS.rustLint.label).toContain("rust:lint");
    expect(TIERS.rustLint.label).toContain("clippy");
  });
});

// The clippy stage's verdict is the same on every machine only while one exact
// toolchain is pinned and nothing installs another beside it.
describe("the Rust toolchain pin", () => {
  const root = join(import.meta.dir, "..");
  const pin = Bun.TOML.parse(readFileSync(join(root, "rust-toolchain.toml"), "utf8")) as {
    toolchain: { channel: string; components: string[] };
  };

  test("names one exact release, not a channel that moves", () => {
    // `stable` or `1.97` would resolve to whatever each machine last updated to.
    expect(pin.toolchain.channel).toMatch(/^\d+\.\d+\.\d+$/);
  });

  test("installs clippy and rustfmt with it", () => {
    expect(pin.toolchain.components).toContain("clippy");
    expect(pin.toolchain.components).toContain("rustfmt");
  });

  test("the release workflow installs the pinned toolchain in every job that compiles Rust", () => {
    const workflow = readFileSync(join(root, ".github", "workflows", "release.yml"), "utf8");
    // An action that installs its own toolchain puts the targets on one cargo
    // never uses here, so a cross-compile fails for a missing target.
    expect(workflow).not.toContain("rust-toolchain@");
    // Each job that compiles Rust has one Rust cache step.
    const compilingJobs = workflow.match(/uses: swatinem\/rust-cache@/g) ?? [];
    const installs = workflow.match(/rustup toolchain install\n\s+rustup target add \S+/g) ?? [];
    expect(compilingJobs.length).toBeGreaterThan(0);
    expect(installs.length).toBe(compilingJobs.length);
  });
});

// Last in the file: killActiveStages() marks the module as stopping for good,
// which is right for a gate about to exit but would silence later tests here.
describe("killActiveStages", () => {
  test("kills every concurrent stage with its tree, and they fail without reporting", async () => {
    const pidFiles = [join(dir, "a.pid"), join(dir, "b.pid")];
    const stages = pidFiles.map((pidFile, i) =>
      runStage({ label: `lane ${i}`, command: `sh -c 'sleep 300 & echo $! > ${pidFile}; wait'`, timeoutMs: 600_000 }, dir)
    );
    while (!pidFiles.every((f) => existsSync(f) && readFileSync(f, "utf8").trim() !== "")) await Bun.sleep(20);
    const errors: string[] = [];
    const originalError = console.error;
    console.error = (...args: unknown[]) => errors.push(args.join(" "));
    try {
      const started = Date.now();
      await killActiveStages();
      expect(await Promise.all(stages)).toEqual([false, false]);
      expect(Date.now() - started).toBeLessThan(15_000);
    } finally {
      console.error = originalError;
    }
    expect(errors).toEqual([]);
    await Bun.sleep(200);
    for (const f of pidFiles) expect(isAlive(Number(readFileSync(f, "utf8").trim()))).toBe(false);
  }, 20_000);
});
