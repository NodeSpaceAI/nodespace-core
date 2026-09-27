#!/usr/bin/env bun
// Warns when the current branch is behind origin/main.
//
// `bun run test:changed` (scripts/test-changed.ts, ADR-047) runs tests
// against the working tree as it stands — on the branch's OWN base, not on
// current main. A failure there may be stale code that main has already
// fixed, and a pass says nothing about a semantic conflict with work merged
// since (one PR adds a validation rule, another adds data violating it).
// The merge gate settles both by testing the PR rebased onto main.
//
// This is a staleness WARNING, not a fix: it is deliberately non-blocking (a
// branch being behind is normal mid-feature) and deliberately does not
// auto-rebase (rewriting history the user didn't ask for is worse than the
// problem it solves).

import { $ } from "bun";

export type BehindCheckStatus = "up-to-date" | "behind" | "skipped";

export interface BehindCheckResult {
  status: BehindCheckStatus;
  /** Commits HEAD is behind origin/main. 0 unless status is "behind". */
  count: number;
  /** Present only when status is "skipped" — why the check didn't run. */
  reason?: string;
}

async function defaultFetch(): Promise<void> {
  await $`git fetch origin main --quiet`.quiet();
}

// Separated from defaultCountBehind so the parsing/validation is directly
// unit-testable without mocking the `$` shell.
export function parseRevListCount(rawOutput: string): number {
  const out = rawOutput.trim();
  if (out === "") {
    // Number("") is 0, not NaN — an empty (but zero-exit) result would
    // otherwise silently read as "up-to-date" rather than "skipped".
    throw new Error("empty rev-list output");
  }
  const n = Number(out);
  if (!Number.isFinite(n)) {
    throw new Error(`unparseable rev-list output: ${JSON.stringify(out)}`);
  }
  return n;
}

async function defaultCountBehind(): Promise<number> {
  const out = await $`git rev-list --count HEAD..origin/main`.text();
  return parseRevListCount(out);
}

// Bun's ShellError carries the real diagnostic text on `.stderr`/`.stdout`
// (Buffers, populated alongside the live stream to the terminal — `.quiet()`
// only suppresses the echo, not the buffering) — `.message` alone is a
// generic "Failed with exit code N" that never says why. Prefer stderr
// (where git's own error text lands), then stdout, then fall back to
// `.message` for a non-shell error.
function errorMessage(err: unknown): string {
  if (err && typeof err === "object") {
    const stderr = readBufferField(err, "stderr");
    if (stderr) return stderr;
    const stdout = readBufferField(err, "stdout");
    if (stdout) return stdout;
  }
  return err instanceof Error ? err.message : String(err);
}

function readBufferField(obj: object, key: "stdout" | "stderr"): string {
  if (!(key in obj)) return "";
  const value = (obj as Record<string, unknown>)[key];
  return Buffer.isBuffer(value) ? value.toString().trim() : "";
}

export interface CheckBranchBehindDeps {
  fetch?: () => Promise<void>;
  countBehind?: () => Promise<number>;
}

/**
 * Fetches origin/main and reports how many commits HEAD is behind it.
 *
 * Never throws: a fetch or rev-list failure (no network, unknown remote, a
 * detached HEAD with no upstream, etc.) degrades to a "skipped" result so
 * this check can never itself fail a run — only the tests it runs
 * alongside can do that.
 */
export async function checkBranchBehind(deps: CheckBranchBehindDeps = {}): Promise<BehindCheckResult> {
  const fetch = deps.fetch ?? defaultFetch;
  const countBehind = deps.countBehind ?? defaultCountBehind;

  try {
    await fetch();
  } catch (err) {
    return {
      status: "skipped",
      count: 0,
      reason: `git fetch origin main failed: ${errorMessage(err)}`,
    };
  }

  try {
    const count = await countBehind();
    return count > 0 ? { status: "behind", count } : { status: "up-to-date", count: 0 };
  } catch (err) {
    return {
      status: "skipped",
      count: 0,
      reason: `git rev-list --count failed: ${errorMessage(err)}`,
    };
  }
}

export function formatBehindWarning(count: number): string {
  const commitWord = count === 1 ? "commit" : "commits";
  return (
    `\n⚠ Branch is ${count} ${commitWord} behind origin/main.\n` +
    "  These tests ran against your base, not the merge result.\n" +
    "  Rebase before merging: git rebase origin/main\n"
  );
}

export function formatSkippedNote(reason: string): string {
  return `\n⚠ Skipped origin/main staleness check: ${reason}\n`;
}

/**
 * Runs the check and prints its result. Prints nothing on the common
 * (up-to-date) path — no noise for the case that is fine.
 */
export async function reportBranchBehind(deps: CheckBranchBehindDeps = {}): Promise<BehindCheckResult> {
  const result = await checkBranchBehind(deps);
  if (result.status === "behind") {
    console.warn(formatBehindWarning(result.count));
  } else if (result.status === "skipped" && result.reason) {
    console.warn(formatSkippedNote(result.reason));
  }
  return result;
}

if (import.meta.main) {
  await reportBranchBehind();
}
