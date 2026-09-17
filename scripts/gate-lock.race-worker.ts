#!/usr/bin/env bun
// Test worker for the cross-process mutual-exclusion test in
// gate-lock.test.ts. Not part of the gate — it exists so that test can race
// REAL operating-system processes against one lock path.
//
// It has to be a separate file rather than an inline `bun -e` string because
// the invariant under test is only observable across process boundaries: the
// acquire path's syscalls are synchronous, so several in-process callers on
// one event loop can never interleave inside them, and an in-process "race"
// proves nothing about the property that actually matters.
//
// Prints one HELD line and one DONE line, bracketing the interval during
// which it believed it held the lock. The test asserts no two intervals
// overlap.

import { acquireGateLock } from "./gate-lock";

const [lockPath, holdMsRaw, startAtRaw] = process.argv.slice(2);
if (!lockPath) throw new Error("usage: gate-lock.race-worker.ts <lockPath> [holdMs] [startAtEpochMs]");
const holdMs = Number(holdMsRaw ?? 150);

// Barrier: every worker busy-waits until the same wall-clock instant before
// racing. Without it, `bun` process startup jitter (~100ms) staggers the
// workers so widely that the first acquirer has long finished publishing its
// lockfile before any other arrives — and the interleaving under test, where
// several processes are inside the acquire path at once, never occurs.
const startAt = Number(startAtRaw ?? 0);
while (Date.now() < startAt) {
  // Spin rather than sleep: the window being probed is sub-millisecond, and a
  // timer's resolution is coarser than the thing it would be waiting for.
}

const lock = await acquireGateLock({
  lockPath,
  // Poll as fast as possible: a waiter must actually look at the lock path
  // while a concurrent acquirer is mid-publish for the race to be observable.
  pollIntervalMs: 0,
  maxWaitMs: 60_000,
  // Silence the queueing chatter; stdout carries the HELD/DONE protocol.
  log: () => {},
});

if (lock.held) {
  console.log(`HELD ${process.pid} ${Date.now()}`);
  await new Promise((resolve) => setTimeout(resolve, holdMs));
  console.log(`DONE ${process.pid} ${Date.now()}`);
  lock.release();
} else {
  console.log(`UNHELD ${process.pid} ${Date.now()}`);
}
