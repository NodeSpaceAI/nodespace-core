// Covers the disk housekeeping of the runs that compile Rust
// (scripts/gate-disk.ts): which incremental directories a prune removes, and
// the free-space refusal.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, symlinkSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  diskUsageKiB,
  formatPruneResult,
  freeGiB,
  freeSpaceRefusal,
  GATE_INCREMENTAL,
  listIncrementalDirs,
  MIN_FREE_GIB,
  pruneIncremental,
  removeUnlessUsedSince,
  selectSuperseded,
  toolOutput,
  WORKTREE_INCREMENTAL,
  type PruneLimits,
} from "./gate-disk";

const HOUR = 60 * 60_000;
const GIB = 1024 ** 2; // in KiB
const NOW = Date.UTC(2026, 9, 5, 12);
const TWO_HOURS: PruneLimits = { maxAgeMs: 2 * HOUR, budgetGiB: 30 };

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
function crateDir(name: string, hoursAgo: number, root: string = incremental): string {
  const dir = join(root, name);
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

/** A compile under way in `dir`: a working session created a second ago, the directory's own time left old. */
function compileUnderWay(dir: string, dirHoursAgo: number): string {
  const working = join(dir, "s-hmxd0000aa-0000000-working");
  mkdirSync(working);
  setMtime(working, NOW - 1000);
  setMtime(dir, NOW - dirHoursAgo * HOUR);
  return working;
}

describe("selectSuperseded", () => {
  const dir = (name: string, hoursAgo: number, gib: number | null = 1) => ({
    name,
    lastUsedMs: NOW - hoursAgo * HOUR,
    kib: gib === null ? null : gib * GIB,
  });

  test("selects the directories last compiled longer than the period before the newest compile", () => {
    const dirs = [dir("core-live", 0.1), dir("core-old", 5), dir("core-edge", 2.1), dir("core-recent", 1)];
    expect(selectSuperseded(dirs, TWO_HOURS, NOW)).toEqual(["core-old"]);
  });

  test("measures from the newest compile, so an idle checkout keeps its latest directories", () => {
    // Nothing built for three days: by the clock every directory is old.
    const dirs = [dir("core-latest", 72), dir("types-latest", 72.5), dir("core-superseded", 80)];
    expect(selectSuperseded(dirs, TWO_HOURS, NOW)).toEqual(["core-superseded"]);
  });

  test("a directory dated in the future doesn't make every other one look old", () => {
    const dirs = [dir("core-skewed", -48), dir("core-live", 0.5), dir("core-old", 5)];
    expect(selectSuperseded(dirs, TWO_HOURS, NOW)).toEqual(["core-old"]);
  });

  test("measures each crate from its own newest compile, so a crate nothing changed keeps its current directories", () => {
    // Set A built ten hours ago; since then a round recompiled only core.
    const dirs = [dir("core-a", 10), dir("types-a", 10), dir("daemon-a", 10.1), dir("core-b", 0.1)];
    expect(selectSuperseded(dirs, TWO_HOURS, NOW)).toEqual(["core-a"]);
  });

  test("reads the crate from a name whose crate part has underscores", () => {
    const dirs = [dir("nodespace_core-0j8syoplaewcn", 5), dir("nodespace_core-3pnxa8lpia5a9", 0.1), dir("nodespace_core_x-2065x6qla1y24", 5)];
    expect(selectSuperseded(dirs, TWO_HOURS, NOW)).toEqual(["nodespace_core-0j8syoplaewcn"]);
  });

  test("dates targets that share a name across packages as one crate", () => {
    // Every package's integration-test binary is `it`: an unchanged package's
    // directory ages out once another package's `it` is compiled.
    const dirs = [dir("it-0j8syoplaewcn", 5), dir("it-3pnxa8lpia5a9", 0.1)];
    expect(selectSuperseded(dirs, TWO_HOURS, NOW)).toEqual(["it-0j8syoplaewcn"]);
  });

  test("keeps a directory exactly the period old, and a crate's only directory however old", () => {
    expect(selectSuperseded([dir("core-edge", 2), dir("core-live", 0)], TWO_HOURS, NOW)).toEqual([]);
    expect(selectSuperseded([dir("core-only", 500)], TWO_HOURS, NOW)).toEqual([]);
  });

  test("past the budget, removes the oldest of what the age rule kept until it fits", () => {
    const dirs = [dir("core-set3", 0.1, 12), dir("core-set1", 1.5, 12), dir("core-set2", 1, 12), dir("core-old", 6, 12)];
    expect(selectSuperseded(dirs, TWO_HOURS, NOW)).toEqual(["core-old", "core-set1"]);
  });

  test("the budget takes the oldest whichever crate it belongs to, and enough of a tie to fit", () => {
    const dirs = [dir("types-a", 1, 20), dir("core-a", 1, 20), dir("daemon-a", 1, 20)];
    expect(selectSuperseded(dirs, TWO_HOURS, NOW)).toEqual(["types-a", "core-a"]);
  });

  test("keeps everything within the period and the budget", () => {
    const dirs = [dir("core-set1", 1.5, 10), dir("core-set2", 1, 10), dir("core-set3", 0.1, 10)];
    expect(selectSuperseded(dirs, TWO_HOURS, NOW)).toEqual([]);
  });

  test("applies only the age rule when a size among the kept is unknown", () => {
    const dirs = [dir("core-set3", 0.1, 40), dir("core-set2", 1, null), dir("core-old", 6, 40)];
    expect(selectSuperseded(dirs, TWO_HOURS, NOW)).toEqual(["core-old"]);
  });

  test("still applies the budget when only a superseded directory's size is unknown", () => {
    const dirs = [dir("core-set3", 0.1, 20), dir("core-set2", 1, 20), dir("core-old", 6, null)];
    expect(selectSuperseded(dirs, TWO_HOURS, NOW)).toEqual(["core-old", "core-set2"]);
  });

  test("selects nothing from an empty listing", () => {
    expect(selectSuperseded([], TWO_HOURS, NOW)).toEqual([]);
  });
});

describe("listIncrementalDirs", () => {
  test("dates a directory by its newest entry, so one a compile has just opened counts as in use", () => {
    compileUnderWay(crateDir("nodespace_core-0j8syoplaewcn", 30), 30);

    const [listed] = listIncrementalDirs(incremental);
    expect(listed.name).toBe("nodespace_core-0j8syoplaewcn");
    expect(listed.lastUsedMs).toBe(NOW - 1000);
    expect(listed.kib).toBeGreaterThan(0);
  });

  test("lists only real directories: no plain file, and no symlink to a directory", () => {
    const elsewhere = mkdtempSync(join(tmpdir(), "gate-disk-elsewhere-"));
    try {
      crateDir("outside-target", 30, elsewhere);
      writeFileSync(join(incremental, "stray-file"), "");
      symlinkSync(join(elsewhere, "outside-target"), join(incremental, "linked-0j8syoplaewcn"));
      expect(listIncrementalDirs(incremental)).toEqual([]);
    } finally {
      rmSync(elsewhere, { recursive: true, force: true });
    }
  });

  test("reports nothing for a missing directory", () => {
    expect(listIncrementalDirs(join(target, "no-such-dir"))).toEqual([]);
  });
});

describe("removeUnlessUsedSince", () => {
  test("removes a directory no compile has used since it was listed", () => {
    const dir = crateDir("nodespace_core-0j8syoplaewcn", 5);
    expect(removeUnlessUsedSince(dir, NOW - 5 * HOUR)).toBe(true);
    expect(existsSync(dir)).toBe(false);
  });

  test("keeps a directory a compile opened after it was listed", () => {
    const dir = crateDir("nodespace_core-0j8syoplaewcn", 5);
    const listed = NOW - 5 * HOUR;
    const working = compileUnderWay(dir, 5);

    expect(removeUnlessUsedSince(dir, listed)).toBe(false);
    expect(existsSync(working)).toBe(true);
  });

  test("reports a directory that has gone as not removed", () => {
    expect(removeUnlessUsedSince(join(incremental, "nodespace_core-gone"), NOW)).toBe(false);
  });
});

describe("pruneIncremental", () => {
  test("removes the superseded directories and keeps the rest", () => {
    const stale = crateDir("nodespace_core-0j8syoplaewcn", 5);
    const alsoStale = crateDir("nodespace_types-2065x6qla1y24", 30);
    const live = crateDir("nodespace_core-3pnxa8lpia5a9", 1);
    const unchangedCrate = crateDir("nodespace_agent-1b9p21sqfl6v9", 30);
    crateDir("nodespace_types-2g5jwlt1mzz7q", 1);

    const result = pruneIncremental(target, TWO_HOURS, NOW);

    expect(result.removed).toBe(2);
    expect(result.kept).toBe(3);
    expect(existsSync(unchangedCrate)).toBe(true);
    expect(result.freedGiB).toBeGreaterThan(0);
    expect(result.keptGiB).toBeGreaterThan(0);
    expect(existsSync(stale)).toBe(false);
    expect(existsSync(alsoStale)).toBe(false);
    expect(existsSync(live)).toBe(true);
    expect(readdirSync(live)).toHaveLength(2);
  });

  test("keeps an old directory that a compile is using now", () => {
    crateDir("nodespace_core-3pnxa8lpia5a9", 0.1);
    const working = compileUnderWay(crateDir("nodespace_daemon-1b4kgzkp3as2o", 30), 30);

    const result = pruneIncremental(target, TWO_HOURS, NOW);

    expect(result.removed).toBe(0);
    expect(result.kept).toBe(2);
    expect(existsSync(working)).toBe(true);
  });

  test("never follows or removes a symlink, nor what it points to", () => {
    const elsewhere = mkdtempSync(join(tmpdir(), "gate-disk-elsewhere-"));
    try {
      const outside = crateDir("outside-target", 30, elsewhere);
      const link = join(incremental, "linked-0j8syoplaewcn");
      symlinkSync(outside, link);
      crateDir("nodespace_core-3pnxa8lpia5a9", 0.1);

      expect(pruneIncremental(target, TWO_HOURS, NOW).removed).toBe(0);
      expect(existsSync(link)).toBe(true);
      expect(readdirSync(outside)).toHaveLength(2);
    } finally {
      rmSync(elsewhere, { recursive: true, force: true });
    }
  });

  test("touches nothing outside debug/incremental", () => {
    crateDir("nodespace_core-0j8syoplaewcn", 30);
    crateDir("nodespace_core-3pnxa8lpia5a9", 0.1);
    const deps = join(target, "debug", "deps");
    mkdirSync(deps);
    writeFileSync(join(deps, "libnodespace_core-abc.rlib"), "x");
    setMtime(join(deps, "libnodespace_core-abc.rlib"), NOW - 30 * HOUR);
    setMtime(deps, NOW - 30 * HOUR);

    expect(pruneIncremental(target, TWO_HOURS, NOW).removed).toBe(1);

    expect(readdirSync(deps)).toEqual(["libnodespace_core-abc.rlib"]);
    expect(existsSync(incremental)).toBe(true);
  });

  test("does nothing in a checkout that has never built", () => {
    rmSync(join(target, "debug"), { recursive: true });
    expect(pruneIncremental(target, TWO_HOURS, NOW)).toEqual({ removed: 0, kept: 0, freedGiB: 0, keptGiB: 0 });
  });

  test("the gate checkout's limits are tighter than a working worktree's period", () => {
    expect(GATE_INCREMENTAL.maxAgeMs).toBeLessThan(WORKTREE_INCREMENTAL.maxAgeMs);
  });
});

describe("formatPruneResult", () => {
  test("says how many directories and how much disk a prune removed and kept", () => {
    expect(formatPruneResult({ removed: 184, kept: 302, freedGiB: 15.93, keptGiB: 24.61 })).toBe(
      "  incremental cache: removed 184 directories (15.9 GiB); 302 directories kept (24.6 GiB)"
    );
  });

  test("reports the counts, and that the budget may not have applied, when sizes couldn't be measured", () => {
    expect(formatPruneResult({ removed: 1, kept: 1, freedGiB: null, keptGiB: null })).toBe(
      "  incremental cache: removed 1 directory; 1 directory kept; some sizes unavailable, so the budget may not have applied"
    );
  });

  test("says so when there was nothing to remove", () => {
    expect(formatPruneResult({ removed: 0, kept: 111, freedGiB: 0, keptGiB: 3.4 })).toBe(
      "  incremental cache: nothing to remove; 111 directories kept (3.4 GiB)"
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

describe("system tools", () => {
  test("a tool that isn't installed gives null, not a crash", () => {
    expect(toolOutput(["nodespace-no-such-tool", "-Pk", target])).toBeNull();
  });

  test("a tool that fails gives null", () => {
    expect(toolOutput(["false"])).toBeNull();
  });

  test("sizes each directory with du", () => {
    const dir = crateDir("nodespace_core-0j8syoplaewcn", 1);
    expect(diskUsageKiB([dir]).get(dir)).toBeGreaterThan(0);
  });

  test("keeps the sizes du measured when one path has gone and du exits non-zero", () => {
    const dir = crateDir("nodespace_core-0j8syoplaewcn", 1);
    const sizes = diskUsageKiB([dir, join(incremental, "nodespace_core-gone")]);
    expect(sizes.size).toBe(1);
    expect(sizes.get(dir)).toBeGreaterThan(0);
  });

  test("sizes are unknown, not zero, on a machine with no du", () => {
    const dir = crateDir("nodespace_core-0j8syoplaewcn", 1);
    expect(diskUsageKiB([dir], "nodespace-no-such-tool").size).toBe(0);
  });
});
