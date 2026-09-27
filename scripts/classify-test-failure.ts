#!/usr/bin/env bun
// Distinguishes a process abort under resource contention (a signal crash —
// SIGSEGV, SIGABRT, ...) from a genuine test/assertion failure, for the
// pre-push gate's reporting (scripts/test-gate.ts, ADR-047).
//
// This does NOT root-cause the underlying SIGSEGV (observed intermittently in
// nodespace-core's lib tests under parallel load, likely libsql/SQLite FFI
// under concurrency, or a post-merge cold-build/test-load overlap) — that
// needs real Rust/FFI investigation, out of scope for a reporting change.
// What this DOES fix: today, a load-induced abort and a real regression look
// identical in the gate's output ("script exited with code 101, push
// blocked"), which wastes time re-running a multi-minute suite to tell them
// apart, and trains people toward `git push --no-verify`. Scanning the
// captured output for known abort signatures and saying so plainly removes
// that ambiguity — it does not change whether the push is blocked (a load
// abort still blocks the push, same as any other failure; retrying is on the
// person, not automated here).

export type FailureKind = "abort" | "failure";

// Patterns a process abort leaves in cargo/test-harness output. Case-
// sensitivity follows convention: signal names are conventionally uppercase,
// so left case-sensitive to avoid false positives on prose that happens to
// contain the word in lowercase.
const ABORT_PATTERNS: RegExp[] = [
  /signal:\s*\d+/i, // cargo's own "(signal: 11, SIGSEGV: ...)" wording
  /SIGSEGV/,
  /SIGABRT/,
  /SIGBUS/,
  /SIGILL/,
  /SIGKILL/,
  /segmentation fault/i,
  /core dumped/i,
  /process didn't exit successfully.*\(signal/i,
  /STATUS_ACCESS_VIOLATION/i, // Windows equivalent
];

/**
 * Classifies captured command output as a process abort (a signal-level
 * crash) or a genuine test/assertion failure. Defaults to "failure" —
 * silence is the safe default; only a positive abort signature should
 * soften the report, never absence of output.
 */
export function classifyFailure(output: string): FailureKind {
  return ABORT_PATTERNS.some((re) => re.test(output)) ? "abort" : "failure";
}

export function formatAbortNote(label: string): string {
  return (
    `\n⚠ ${label} looks like a process abort under load (e.g. a signal crash), not a\n` +
    "  genuine assertion failure. This suite is known to hit resource contention\n" +
    "  under concurrent compilation/test load. Rerun before assuming the code broke.\n"
  );
}
