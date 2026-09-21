/**
 * Regression coverage for `createRunOnceGuard` (packages/dev-tools/src/grpc-client.ts)
 * — the primitive `startWatchBridge()`'s `connect()` in dev-proxy.ts uses,
 * one fresh instance per `WatchNodes` stream, to fix a real dev-proxy bug.
 * See that function's doc comment for the full root-cause story (a dropped
 * stream reliably firing both `'error'` and `'end'`) and why the guard is
 * scoped per-stream rather than shared across the bridge's lifetime.
 *
 * This drives the guard directly — pure JS event-handling logic, reproducible
 * by calling `run()` more than once, so no daemon or gRPC server is needed to
 * cover it. The daemon-restart-recovery path itself was verified live (two
 * consecutive kill+restart cycles against a real nodespaced on an isolated
 * NODESPACE_HOME/socket, confirmed via GRPC_TRACE showing exactly one
 * WatchNodes stream attempt per genuine disconnect, and via a live SSE
 * subscription receiving a nodeCreated event for a node created against the
 * replacement daemon) — that requires two real daemon processes and isn't
 * practical to simulate here.
 */

import { describe, it, expect, vi } from 'vitest';
import { createRunOnceGuard } from '../../../../dev-tools/src/grpc-client';

describe('createRunOnceGuard', () => {
  it('runs the first callback', () => {
    const guard = createRunOnceGuard();
    const callback = vi.fn();

    guard.run(callback);

    expect(callback).toHaveBeenCalledTimes(1);
  });

  it("suppresses an 'error'+'end' pair for the same dropped stream — the core bug this fixes", () => {
    // Mirrors startWatchBridge()'s stream.on('error', ...) firing first,
    // immediately followed by stream.on('end', ...), for the SAME disconnect
    // — the observed, reliable grpc-js event ordering that caused the
    // duplicate-reconnect bug pre-fix.
    const guard = createRunOnceGuard();
    const onError = vi.fn();
    const onEnd = vi.fn();

    guard.run(onError);
    guard.run(onEnd); // must be a no-op: this stream already reported its outcome

    expect(onError).toHaveBeenCalledTimes(1);
    expect(onEnd).not.toHaveBeenCalled();
  });

  it('suppresses any number of repeated calls, not just a second one', () => {
    const guard = createRunOnceGuard();
    const callback = vi.fn();

    for (let i = 0; i < 5; i++) {
      guard.run(callback);
    }

    expect(callback).toHaveBeenCalledTimes(1);
  });

  it('two independent instances do not affect each other — a superseded stream cannot interfere with a newer one', () => {
    // connect() constructs a fresh guard per attempt (per stream). This is
    // what makes a late event from an old, already-superseded stream unable
    // to touch a newer, already-reconnected attempt's state: the old stream's
    // handlers close over their OWN guard, not a shared one.
    const staleStreamGuard = createRunOnceGuard();
    const freshStreamGuard = createRunOnceGuard();
    const staleCallback = vi.fn();
    const freshCallback = vi.fn();

    staleStreamGuard.run(staleCallback);
    expect(staleCallback).toHaveBeenCalledTimes(1);

    // The stale stream's guard being spent has no bearing on the fresh one.
    freshStreamGuard.run(freshCallback);
    expect(freshCallback).toHaveBeenCalledTimes(1);

    // And a further stray event from the stale stream stays suppressed.
    staleStreamGuard.run(staleCallback);
    expect(staleCallback).toHaveBeenCalledTimes(1);
  });
});
