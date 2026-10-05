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
// the ones no compile has used for a set period, then checks free space.
//
// Both callers run this while they hold the machine slot, so no other gate or
// test:changed run is compiling into the same target/ during the prune.

import { existsSync, readdirSync, rmSync, statSync } from "node:fs";
import { join } from "node:path";
import { freeGiBFromDf } from "./gate-output";

const HOUR = 60 * 60_000;

/** Below this much free disk a run that compiles Rust refuses to start. */
export const MIN_FREE_GIB = 20;

/**
 * How long an incremental directory may go without a compile before the merge
 * gate removes it from the gate checkout. A stack that changes the hash's
 * inputs builds a whole new set of directories, about 8 GiB for the
 * workspace, and that checkout built five sets in four hours. Two hours keeps
 * the sets of the last few rounds, which a round that returns to main's
 * inputs reuses.
 */
export const GATE_INCREMENTAL_MAX_AGE_MS = 2 * HOUR;

/**
 * The same period for a working worktree, pruned by its test:changed Rust
 * tier. A worktree's set is about 3.5 GiB and changes only when it takes in
 * a main with other inputs, so it keeps a day's worth.
 */
export const WORKTREE_INCREMENTAL_MAX_AGE_MS = 24 * HOUR;

export interface IncrementalDir {
  name: string;
  /** When a compile last used the directory, in ms since the epoch. */
  lastUsedMs: number;
}

function mtimeMs(path: string): number {
  try {
    return statSync(path).mtimeMs;
  } catch {
    return 0;
  }
}

/**
 * The crate directories under `incrementalDir`, each with when it was last
 * used: the newest modification time of the directory and its entries. A
 * compile starts by creating a session directory inside and ends by renaming
 * it, so both change on every compile of that variant. A crate cargo finds
 * fresh is not compiled, and its directory is not touched.
 */
export function listIncrementalDirs(incrementalDir: string): IncrementalDir[] {
  let entries;
  try {
    entries = readdirSync(incrementalDir, { withFileTypes: true });
  } catch {
    return [];
  }
  return entries
    .filter((entry) => entry.isDirectory())
    .map((entry) => {
      const path = join(incrementalDir, entry.name);
      let children: string[] = [];
      try {
        children = readdirSync(path);
      } catch {
        // Unreadable: its own modification time is all there is.
      }
      const lastUsedMs = Math.max(mtimeMs(path), ...children.map((child) => mtimeMs(join(path, child))));
      return { name: entry.name, lastUsedMs };
    });
}

/** The directories no compile has used for longer than `maxAgeMs`. Pure, for testing. */
export function selectSuperseded(dirs: IncrementalDir[], nowMs: number, maxAgeMs: number): string[] {
  return dirs.filter((dir) => nowMs - dir.lastUsedMs > maxAgeMs).map((dir) => dir.name);
}

/** Disk used by `paths` in KiB, or null when `du` can't say. */
function diskUsageKiB(paths: string[]): number | null {
  let total = 0;
  for (let i = 0; i < paths.length; i += 200) {
    const du = Bun.spawnSync(["du", "-sk", "--", ...paths.slice(i, i + 200)]);
    if (du.exitCode !== 0) return null;
    for (const line of du.stdout.toString().split("\n")) {
      const kib = Number(line.trim().split(/\s+/)[0]);
      if (line.trim() !== "" && Number.isFinite(kib)) total += kib;
    }
  }
  return total;
}

export interface PruneResult {
  removed: number;
  kept: number;
  /** Disk the removed directories used, or null when it couldn't be measured. */
  freedGiB: number | null;
}

/**
 * Removes the incremental directories under `targetDir` that no compile has
 * used for longer than `maxAgeMs`. Removing one never makes cargo recompile a
 * crate: freshness is decided by the fingerprints, not by this cache. The cost
 * of removing a directory that was still current is one compile of that crate
 * from scratch, the next time it changes. A directory that can't be removed
 * is left and counted as kept.
 */
export function pruneIncremental(targetDir: string, maxAgeMs: number, nowMs: number = Date.now()): PruneResult {
  const incrementalDir = join(targetDir, "debug", "incremental");
  if (!existsSync(incrementalDir)) return { removed: 0, kept: 0, freedGiB: 0 };
  const dirs = listIncrementalDirs(incrementalDir);
  const superseded = selectSuperseded(dirs, nowMs, maxAgeMs).map((name) => join(incrementalDir, name));
  const usedKiB = diskUsageKiB(superseded);
  let removed = 0;
  for (const path of superseded) {
    try {
      rmSync(path, { recursive: true, force: true });
      removed++;
    } catch {
      // Left for the next run.
    }
  }
  return {
    removed,
    kept: dirs.length - removed,
    freedGiB: usedKiB === null || removed < superseded.length ? null : usedKiB / 1024 ** 2,
  };
}

/** The one line a run prints about its prune. */
export function formatPruneResult(result: PruneResult, maxAgeMs: number): string {
  const age = `${Math.round(maxAgeMs / HOUR)}h`;
  if (result.removed === 0) {
    return `  incremental cache: nothing unused for ${age} (${result.kept} directories kept)`;
  }
  const size = result.freedGiB === null ? "" : ` (${result.freedGiB.toFixed(1)} GiB)`;
  const noun = result.removed === 1 ? "directory" : "directories";
  return `  incremental cache: removed ${result.removed} ${noun}${size} unused for ${age}; ${result.kept} kept`;
}

/** Free space in GiB on the disk holding `path`, or null when it can't be read. */
export function freeGiB(path: string): number | null {
  const df = Bun.spawnSync(["df", "-Pk", path]);
  return df.exitCode === 0 ? freeGiBFromDf(df.stdout.toString()) : null;
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
