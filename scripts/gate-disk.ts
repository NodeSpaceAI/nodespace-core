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
// Both callers run this while they hold the machine slot, so no other gate or
// test:changed run is compiling into the same target/ during the prune.

import { existsSync, readdirSync, rmSync, statSync } from "node:fs";
import { join } from "node:path";
import { freeGiBFromDf } from "./gate-output";

const HOUR = 60 * 60_000;
const KIB_PER_GIB = 1024 ** 2;

/** Below this much free disk a run that compiles Rust refuses to start. */
export const MIN_FREE_GIB = 20;

export interface PruneLimits {
  /**
   * A directory is superseded once it was last compiled this long before the
   * newest compile in the same target/. Measured from the newest compile, not
   * from the clock, so a checkout that sat idle keeps its latest directories.
   */
  maxAgeMs: number;
  /** What may be left after the age rule; beyond it the oldest go first. */
  budgetGiB: number;
}

/**
 * The gate checkout's limits. A stack that changes the hash's inputs builds a
 * whole new set of directories, about 8 GiB for the workspace, and that
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

/** Disk used by each of `paths` in KiB, read with `du`. Empty when `du` can't say. */
export function diskUsageKiB(paths: string[], du: string = "du"): Map<string, number> {
  const sizes = new Map<string, number>();
  for (let i = 0; i < paths.length; i += 200) {
    const output = toolOutput([du, "-sk", "--", ...paths.slice(i, i + 200)]);
    if (output === null) return new Map();
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
 * The directories to remove, oldest first. Pure, for testing.
 *
 * First the superseded ones: last compiled more than `maxAgeMs` before the
 * newest compile (or before `nowMs`, when a directory is dated in the
 * future). Then, while the rest exceeds the budget, the oldest of the rest.
 * The budget is skipped when any size is unknown.
 */
export function selectSuperseded(dirs: IncrementalDir[], limits: PruneLimits, nowMs: number): string[] {
  if (dirs.length === 0) return [];
  const newest = Math.min(nowMs, Math.max(...dirs.map((dir) => dir.lastUsedMs)));
  const oldestFirst = [...dirs].sort((a, b) => a.lastUsedMs - b.lastUsedMs);
  const selected = oldestFirst.filter((dir) => newest - dir.lastUsedMs > limits.maxAgeMs);
  const rest = oldestFirst.slice(selected.length);
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
 *
 * A directory is dated again just before it is removed, and kept if a compile
 * has used it since the listing: a cargo build started by hand takes no
 * machine slot. One that can't be removed is left and counted as kept.
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
    const path = join(incrementalDir, dir.name);
    if (selected.has(dir.name) && lastUsedMs(path) === dir.lastUsedMs) {
      try {
        rmSync(path, { recursive: true, force: true });
        removed++;
        freedKiB += dir.kib ?? 0;
        continue;
      } catch {
        // Left for the next run.
      }
    }
    keptKiB += dir.kib ?? 0;
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
  const kept = `${dirs(result.kept)} kept${size(result.keptGiB)}`;
  if (result.removed === 0) return `  incremental cache: nothing to remove; ${kept}`;
  return `  incremental cache: removed ${dirs(result.removed)}${size(result.freedGiB)}; ${kept}`;
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
