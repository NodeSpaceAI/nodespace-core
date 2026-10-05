#!/usr/bin/env bun
// Disk housekeeping for the runs that compile Rust: the merge gate
// (scripts/test-gate.ts) and the `bun run test:changed` Rust tier
// (scripts/test-changed.ts). ADR-047.
//
// rustc keeps one incremental-compilation directory per crate per build
// variant, `target/debug/incremental/<crate>-<hash>`. It collects old sessions
// inside a directory, but never removes a directory whose variant is no longer
// built, and a new variant appears whenever the hash's inputs change (a
// dependency added or bumped, a version bump, another profile or tool). Left
// alone the directories filled a 460 GB disk in a working day. So a run prunes
// the superseded ones, keeps what is left within a size budget, then checks
// free space.
//
// Cargo does the same beside it, in `target/debug/deps`: every build variant's
// artifacts are named `<name>-<hash>`, and a superseded variant's are never
// removed. Nothing on disk says which are current, so that directory is
// emptied whole once it passes a budget (pruneDeps).
//
// Both callers run this while they hold the machine slot, so no other gate or
// test:changed run is compiling into the same target/ during the prune.

import { existsSync, lstatSync, readdirSync, rmSync, statSync } from "node:fs";
import { join } from "node:path";
import { freeGiBFromDf } from "./gate-output";

const HOUR = 60 * 60_000;
const KIB_PER_GIB = 1024 ** 2;

/** Below this much free disk a run that compiles Rust refuses to start. */
export const MIN_FREE_GIB = 20;

export interface PruneLimits {
  /**
   * A directory is superseded once it was last compiled this long before the
   * newest compile of its crate in the same target/. Measured from that, not
   * from the clock, so a checkout that sat idle keeps its latest directories.
   */
  maxAgeMs: number;
  /** What may be left after the age rule; beyond it the oldest go first. */
  budgetGiB: number;
}

/**
 * The gate checkout's limits. A stack that changes the hash's inputs builds a
 * whole new set of directories, 8 to 10 GiB for the workspace, and that
 * checkout built five sets in four hours. Two hours keeps the sets of the
 * last few rounds, which a round that returns to main's inputs reuses; the
 * budget holds three sets and caps a busier day.
 */
export const GATE_INCREMENTAL: PruneLimits = { maxAgeMs: 2 * HOUR, budgetGiB: 30 };

/**
 * A working worktree's limits, applied by its test:changed Rust tier. Its set
 * is 3.5 to 6 GiB and changes only when it takes in a main with other inputs,
 * so it keeps a day's worth, within a smaller budget.
 */
export const WORKTREE_INCREMENTAL: PruneLimits = { maxAgeMs: 24 * HOUR, budgetGiB: 15 };

export interface IncrementalDir {
  name: string;
  /** When a compile last used the directory, in ms since the epoch. */
  lastUsedMs: number;
  /** Disk it uses, or null when that couldn't be measured. */
  kib: number | null;
}

/**
 * When a compile last used the crate directory at `path`: the newest
 * modification time of the directory and its entries. A compile starts by
 * creating a session directory inside and ends by renaming it, so both change
 * on every compile of that variant. A crate cargo finds fresh is not compiled,
 * and its directory is not touched. Null when the directory can't be read.
 */
function lastUsedMs(path: string): number | null {
  try {
    let newest = statSync(path).mtimeMs;
    for (const child of readdirSync(path)) {
      try {
        newest = Math.max(newest, statSync(join(path, child)).mtimeMs);
      } catch {
        // Gone since the listing; the directory's own time covers it.
      }
    }
    return newest;
  } catch {
    return null;
  }
}

/**
 * The output of a system tool, or null when it failed or isn't installed.
 * `Bun.spawnSync` throws for a missing executable; uncaught, that would end a
 * gate with the exit code of a failing test.
 */
export function toolOutput(argv: string[]): string | null {
  try {
    const proc = Bun.spawnSync(argv);
    return proc.exitCode === 0 ? proc.stdout.toString() : null;
  } catch {
    return null;
  }
}

/**
 * Disk used by each of `paths` in KiB, read with `du`. A path `du` couldn't
 * size is left out, and all of them when it isn't installed. Its exit code is
 * not read: `du` exits non-zero when one file vanishes while it walks, as a
 * build's do, and still prints every size it measured.
 */
export function diskUsageKiB(paths: string[], du: string = "du"): Map<string, number> {
  const sizes = new Map<string, number>();
  for (let i = 0; i < paths.length; i += 200) {
    let output: string;
    try {
      output = Bun.spawnSync([du, "-sk", "--", ...paths.slice(i, i + 200)]).stdout.toString();
    } catch {
      return new Map();
    }
    for (const line of output.split("\n")) {
      const tab = line.indexOf("\t");
      const kib = Number(line.slice(0, tab));
      if (tab > 0 && Number.isFinite(kib)) sizes.set(line.slice(tab + 1), kib);
    }
  }
  return sizes;
}

/**
 * The crate directories under `incrementalDir`. Only real directories: a
 * symlink is not followed and never listed. One whose times can't be read is
 * left out too, so the prune never removes what it couldn't date.
 */
export function listIncrementalDirs(incrementalDir: string): IncrementalDir[] {
  let names: string[];
  try {
    names = readdirSync(incrementalDir, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name);
  } catch {
    return [];
  }
  const sizes = diskUsageKiB(names.map((name) => join(incrementalDir, name)));
  const dirs: IncrementalDir[] = [];
  for (const name of names) {
    const used = lastUsedMs(join(incrementalDir, name));
    if (used !== null) dirs.push({ name, lastUsedMs: used, kib: sizes.get(join(incrementalDir, name)) ?? null });
  }
  return dirs;
}

/**
 * The crate a directory named `<crate>-<hash>` belongs to. That is the
 * target's name, not the package's: every package's integration-test binary
 * is `it` and every build script `build_script_build`, so those are each
 * dated as one crate.
 */
function crateOf(name: string): string {
  return name.replace(/-[^-]*$/, "");
}

/**
 * The directories to remove. Pure, for testing.
 *
 * First the superseded ones: last compiled more than `maxAgeMs` before the
 * newest compile of the same crate (or before `nowMs`, when that is dated in
 * the future). Each crate is measured against its own newest compile, because
 * a crate nothing has changed is not compiled at all: measured against the
 * newest compile of any crate, its current directories would age out while
 * the crates around it were rebuilt. Then, while the rest exceeds the budget,
 * the oldest of the rest, whichever crate it belongs to. The budget is
 * skipped when a size among the rest is unknown.
 */
export function selectSuperseded(dirs: IncrementalDir[], limits: PruneLimits, nowMs: number): string[] {
  const newestOf = new Map<string, number>();
  for (const dir of dirs) {
    const crate = crateOf(dir.name);
    newestOf.set(crate, Math.max(newestOf.get(crate) ?? dir.lastUsedMs, dir.lastUsedMs));
  }
  const superseded = (dir: IncrementalDir) =>
    Math.min(nowMs, newestOf.get(crateOf(dir.name)) ?? nowMs) - dir.lastUsedMs > limits.maxAgeMs;
  const oldestFirst = [...dirs].sort((a, b) => a.lastUsedMs - b.lastUsedMs);
  const selected = oldestFirst.filter(superseded);
  const rest = oldestFirst.filter((dir) => !superseded(dir));
  if (rest.every((dir) => dir.kib !== null)) {
    let restKiB = rest.reduce((sum, dir) => sum + (dir.kib ?? 0), 0);
    for (const dir of rest) {
      if (restKiB <= limits.budgetGiB * KIB_PER_GIB) break;
      selected.push(dir);
      restKiB -= dir.kib ?? 0;
    }
  }
  return selected.map((dir) => dir.name);
}

/**
 * Removes the crate directory at `path` unless a compile has used it since it
 * was listed with `listedLastUsedMs`, and says whether it did. The directory
 * is dated again here, right before the removal, because a cargo build
 * started by hand takes no machine slot and may have opened it meanwhile. One
 * that can't be removed is left for the next run.
 */
export function removeUnlessUsedSince(path: string, listedLastUsedMs: number): boolean {
  if (lastUsedMs(path) !== listedLastUsedMs) return false;
  try {
    rmSync(path, { recursive: true, force: true });
    return true;
  } catch {
    return false;
  }
}

export interface PruneResult {
  removed: number;
  kept: number;
  /** Disk the removed directories used, or null when it couldn't be measured. */
  freedGiB: number | null;
  /** Disk the kept directories use, or null when it couldn't be measured. */
  keptGiB: number | null;
}

/**
 * Prunes the incremental directories under `targetDir` to `limits`. Removing
 * one never makes cargo recompile a crate: freshness is decided by the
 * fingerprints, not by this cache. The cost of removing a directory that was
 * still current is one compile of that crate from scratch, the next time it
 * changes.
 */
export function pruneIncremental(targetDir: string, limits: PruneLimits, nowMs: number = Date.now()): PruneResult {
  const incrementalDir = join(targetDir, "debug", "incremental");
  if (!existsSync(incrementalDir)) return { removed: 0, kept: 0, freedGiB: 0, keptGiB: 0 };
  const dirs = listIncrementalDirs(incrementalDir);
  const selected = new Set(selectSuperseded(dirs, limits, nowMs));
  let removed = 0;
  let freedKiB = 0;
  let keptKiB = 0;
  for (const dir of dirs) {
    if (selected.has(dir.name) && removeUnlessUsedSince(join(incrementalDir, dir.name), dir.lastUsedMs)) {
      removed++;
      freedKiB += dir.kib ?? 0;
    } else {
      keptKiB += dir.kib ?? 0;
    }
  }
  const measured = dirs.every((dir) => dir.kib !== null);
  return {
    removed,
    kept: dirs.length - removed,
    freedGiB: measured ? freedKiB / KIB_PER_GIB : null,
    keptGiB: measured ? keptKiB / KIB_PER_GIB : null,
  };
}

/** The one line a run prints about its prune. */
export function formatPruneResult(result: PruneResult): string {
  const size = (gib: number | null) => (gib === null ? "" : ` (${gib.toFixed(1)} GiB)`);
  const dirs = (count: number) => `${count} ${count === 1 ? "directory" : "directories"}`;
  // Said, because the budget is skipped when a kept directory has no size.
  const unsized = result.keptGiB === null ? "; some sizes unavailable, so the budget may not have applied" : "";
  const kept = `${dirs(result.kept)} kept${size(result.keptGiB)}${unsized}`;
  if (result.removed === 0) return `  incremental cache: nothing to remove; ${kept}`;
  return `  incremental cache: removed ${dirs(result.removed)}${size(result.freedGiB)}; ${kept}`;
}

/**
 * What `target/debug/deps` may hold before a run empties it. One set of
 * artifacts for everything the merge gate builds is 7 GiB, and the gate
 * checkout gains a set whenever a queued PR changes the hash's inputs: 16 GiB
 * on the day measured. So this is about three sets, and that checkout passes
 * it about once a day.
 */
export const DEPS_BUDGET_GIB = 20;

export interface DepsPruneResult {
  /**
   * `kept`: within the budget. `emptied`: over it, and removed. `partly-removed`:
   * over it, and the removal failed partway. `absent`: there is no `deps/`.
   * `not-a-directory`: it is a symlink or a file. `unsized`: it couldn't be
   * read or measured. The last three leave it untouched.
   */
  outcome: "kept" | "emptied" | "partly-removed" | "absent" | "not-a-directory" | "unsized";
  /** Disk `deps/` used before the prune, or null when it wasn't measured. */
  sizeGiB: number | null;
}

/**
 * Empties `target/debug/deps` under `targetDir` when it uses more than
 * `budgetGiB`.
 *
 * All of it, because nothing on disk separates a current artifact from a
 * superseded one: a build that finds a crate fresh writes nothing, and one
 * crate has several current variants, built at different times. Cargo
 * rebuilds what is missing, with the incremental directories and sccache
 * still in place. Measured: the merge gate's builds took 103 s after an
 * emptying, against 8 s with nothing to do and 392 s from an empty target/.
 *
 * Unlike the incremental prune it has no second look before it removes, so a
 * caller runs it only while it holds the machine slot.
 */
export function pruneDeps(targetDir: string, budgetGiB: number = DEPS_BUDGET_GIB, du: string = "du"): DepsPruneResult {
  const depsDir = join(targetDir, "debug", "deps");
  if (!existsSync(depsDir)) return { outcome: "absent", sizeGiB: null };
  try {
    // lstat, so a symlink is seen as one and never followed.
    if (!lstatSync(depsDir).isDirectory()) return { outcome: "not-a-directory", sizeGiB: null };
  } catch {
    return { outcome: "unsized", sizeGiB: null };
  }
  const kib = diskUsageKiB([depsDir], du).get(depsDir);
  if (kib === undefined) return { outcome: "unsized", sizeGiB: null };
  const sizeGiB = kib / KIB_PER_GIB;
  if (sizeGiB <= budgetGiB) return { outcome: "kept", sizeGiB };
  try {
    // Cargo creates the directory again on its next build.
    rmSync(depsDir, { recursive: true, force: true });
    return { outcome: "emptied", sizeGiB };
  } catch {
    // Safe: cargo rebuilds whatever is missing.
    return { outcome: "partly-removed", sizeGiB };
  }
}

/** The one line a run prints about `deps/`. */
export function formatDepsResult(result: DepsPruneResult, budgetGiB: number = DEPS_BUDGET_GIB): string {
  const size = result.sizeGiB === null ? "" : `${result.sizeGiB.toFixed(1)} GiB`;
  const over = `${size}, over the ${budgetGiB} GiB budget`;
  const said = {
    kept: `${size} kept (emptied above ${budgetGiB} GiB)`,
    emptied: `${over}; emptied, so this run rebuilds them`,
    "partly-removed": `${over}; could not be fully removed, and this run rebuilds what is missing`,
    absent: "none yet",
    "not-a-directory": "not a directory, so left alone",
    unsized: "size unavailable, so left alone",
  }[result.outcome];
  return `  build artifacts (deps/): ${said}`;
}

/** Free space in GiB on the disk holding `path`, or null when it can't be read. */
export function freeGiB(path: string): number | null {
  const df = toolOutput(["df", "-Pk", path]);
  return df === null ? null : freeGiBFromDf(df);
}

/**
 * Why `run` must not start with `free` GiB left, or null when it may. Null
 * also when free space couldn't be read, so a run isn't refused over a parse
 * failure.
 */
export function freeSpaceRefusal(free: number | null, run: string): string | null {
  if (free === null || free >= MIN_FREE_GIB) return null;
  return (
    `\n✗ Only ${free.toFixed(1)} GiB free on this disk; ${run} needs at least ${MIN_FREE_GIB}.\n` +
    "  Each worktree's target/ holds its own build output. Free space by removing finished\n" +
    "  worktrees, or with `cargo clean` in worktrees that aren't building, then re-run.\n"
  );
}
