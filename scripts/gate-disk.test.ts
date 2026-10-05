// Covers the disk housekeeping of the runs that compile Rust
// (scripts/gate-disk.ts): which incremental directories a prune removes, and
// the free-space refusal.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  formatPruneResult,
  freeGiB,
  freeSpaceRefusal,
  GATE_INCREMENTAL_MAX_AGE_MS,
  listIncrementalDirs,
  MIN_FREE_GIB,
  pruneIncremental,
  selectSuperseded,
  WORKTREE_INCREMENTAL_MAX_AGE_MS,
} from "./gate-disk";

const HOUR = 60 * 60_000;
const NOW = Date.UTC(2026, 9, 5, 12);

let target: string;
let incremental: string;

beforeEach(() => {
  target = mkdtempSync(join(tmpdir(), "gate-disk-test-"));
  incremental = join(target, "debug", "incremental");
  mkdirSync(incremental, { recursive: true });
});

afterEach(() => {
  rmSync(target, { recursive: true, force: true });
});

function setMtime(path: string, ms: number): void {
  utimesSync(path, ms / 1000, ms / 1000);
}

/**
 * A crate's incremental directory as rustc leaves it: a session directory and
 * its lock file, all last modified `hoursAgo` before NOW.
 */
function crateDir(name: string, hoursAgo: number): string {
  const dir = join(incremental, name);
  const session = join(dir, "s-hmxc908iox-19oyelo-an6ha9ui8q6i8bj12q4rbgsbj");
  mkdirSync(session, { recursive: true });
  writeFileSync(join(session, "dep-graph.bin"), "x".repeat(4096));
  writeFileSync(join(dir, "s-hmxc908iox-19oyelo.lock"), "");
  const at = NOW - hoursAgo * HOUR;
  for (const path of [join(session, "dep-graph.bin"), session, join(dir, "s-hmxc908iox-19oyelo.lock"), dir]) {
    setMtime(path, at);
  }
  return dir;
}

describe("selectSuperseded", () => {
  const dirs = [
    { name: "nodespace_core-old", lastUsedMs: NOW - 5 * HOUR },
    { name: "nodespace_core-edge", lastUsedMs: NOW - 2 * HOUR },
    { name: "nodespace_core-live", lastUsedMs: NOW - 10 * 60_000 },
  ];

  test("selects the directories unused for longer than the period", () => {
    expect(selectSuperseded(dirs, NOW, 2 * HOUR)).toEqual(["nodespace_core-old"]);
  });

  test("keeps everything when nothing is older than the period", () => {
    expect(selectSuperseded(dirs, NOW, 24 * HOUR)).toEqual([]);
  });

  test("keeps a directory used after the prune's own clock reading", () => {
    expect(selectSuperseded([{ name: "it-racing", lastUsedMs: NOW + 1000 }], NOW, 2 * HOUR)).toEqual([]);
  });
});

describe("listIncrementalDirs", () => {
  test("dates a directory by its newest entry, so one a compile has just opened counts as in use", () => {
    const dir = crateDir("nodespace_core-0j8syoplaewcn", 30);
    // A compile under way: a working session directory created just now. The
    // crate directory's own time is put back, as if only the entry were new.
    const working = join(dir, "s-hmxd0000aa-0000000-working");
    mkdirSync(working);
    setMtime(working, NOW - 1000);
    setMtime(dir, NOW - 30 * HOUR);

    const [listed] = listIncrementalDirs(incremental);
    expect(listed.name).toBe("nodespace_core-0j8syoplaewcn");
    expect(listed.lastUsedMs).toBe(NOW - 1000);
  });

  test("ignores plain files and reports nothing for a missing directory", () => {
    writeFileSync(join(incremental, "stray-file"), "");
    expect(listIncrementalDirs(incremental)).toEqual([]);
    expect(listIncrementalDirs(join(target, "no-such-dir"))).toEqual([]);
  });
});

describe("pruneIncremental", () => {
  test("removes the directories unused for the period and keeps the rest", () => {
    const stale = crateDir("nodespace_core-0j8syoplaewcn", 5);
    const alsoStale = crateDir("nodespace_types-2065x6qla1y24", 30);
    const live = crateDir("nodespace_core-3pnxa8lpia5a9", 1);

    const result = pruneIncremental(target, 2 * HOUR, NOW);

    expect(result.removed).toBe(2);
    expect(result.kept).toBe(1);
    expect(result.freedGiB).toBeGreaterThan(0);
    expect(existsSync(stale)).toBe(false);
    expect(existsSync(alsoStale)).toBe(false);
    expect(existsSync(live)).toBe(true);
    expect(readdirSync(live)).toHaveLength(2);
  });

  test("keeps an old directory that a compile is using now", () => {
    const dir = crateDir("nodespace_daemon-1b4kgzkp3as2o", 30);
    const working = join(dir, "s-hmxd0000aa-0000000-working");
    mkdirSync(working);
    setMtime(working, NOW - 1000);
    setMtime(dir, NOW - 30 * HOUR);

    expect(pruneIncremental(target, 2 * HOUR, NOW)).toEqual({ removed: 0, kept: 1, freedGiB: 0 });
    expect(existsSync(working)).toBe(true);
  });

  test("touches nothing outside debug/incremental", () => {
    crateDir("nodespace_core-0j8syoplaewcn", 30);
    const deps = join(target, "debug", "deps");
    mkdirSync(deps);
    writeFileSync(join(deps, "libnodespace_core-abc.rlib"), "x");
    setMtime(join(deps, "libnodespace_core-abc.rlib"), NOW - 30 * HOUR);
    setMtime(deps, NOW - 30 * HOUR);

    pruneIncremental(target, 2 * HOUR, NOW);

    expect(readdirSync(deps)).toEqual(["libnodespace_core-abc.rlib"]);
    expect(existsSync(incremental)).toBe(true);
  });

  test("does nothing in a checkout that has never built", () => {
    rmSync(join(target, "debug"), { recursive: true });
    expect(pruneIncremental(target, 2 * HOUR, NOW)).toEqual({ removed: 0, kept: 0, freedGiB: 0 });
  });

  test("the gate checkout's period is shorter than a working worktree's", () => {
    expect(GATE_INCREMENTAL_MAX_AGE_MS).toBeLessThan(WORKTREE_INCREMENTAL_MAX_AGE_MS);
  });
});

describe("formatPruneResult", () => {
  test("says how many directories and how much disk a prune removed", () => {
    expect(formatPruneResult({ removed: 212, kept: 158, freedGiB: 31.42 }, 2 * HOUR)).toBe(
      "  incremental cache: removed 212 directories (31.4 GiB) unused for 2h; 158 kept"
    );
  });

  test("still reports the count when the size couldn't be measured", () => {
    expect(formatPruneResult({ removed: 1, kept: 0, freedGiB: null }, 24 * HOUR)).toBe(
      "  incremental cache: removed 1 directory unused for 24h; 0 kept"
    );
  });

  test("says so when there was nothing to remove", () => {
    expect(formatPruneResult({ removed: 0, kept: 111, freedGiB: 0 }, 24 * HOUR)).toBe(
      "  incremental cache: nothing unused for 24h (111 directories kept)"
    );
  });
});

describe("freeSpaceRefusal", () => {
  test("refuses below the floor, naming the run, the cause and how to free space", () => {
    const refusal = freeSpaceRefusal(3.25, "the test:changed Rust tier");
    expect(refusal).toContain(`Only 3.3 GiB free on this disk; the test:changed Rust tier needs at least ${MIN_FREE_GIB}.`);
    expect(refusal).toContain("target/ holds its own build output");
    expect(refusal).toContain("removing finished");
    expect(refusal).toContain("cargo clean");
  });

  test("allows a run at or above the floor", () => {
    expect(freeSpaceRefusal(MIN_FREE_GIB, "the merge gate")).toBeNull();
    expect(freeSpaceRefusal(151.8, "the merge gate")).toBeNull();
  });

  test("doesn't refuse when free space couldn't be read", () => {
    expect(freeSpaceRefusal(null, "the merge gate")).toBeNull();
  });
});

describe("freeGiB", () => {
  test("reads the free space of the disk holding a path", () => {
    expect(freeGiB(target)).toBeGreaterThan(0);
  });

  test("is null for a path df can't read", () => {
    expect(freeGiB(join(target, "no-such-dir"))).toBeNull();
  });
});
