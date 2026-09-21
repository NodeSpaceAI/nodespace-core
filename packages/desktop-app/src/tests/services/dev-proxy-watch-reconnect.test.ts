/**
 * Regression coverage for the bug behind core#2756: after the daemon a
 * dev-proxy process is watching gets killed and replaced by a new daemon on
 * the same Unix socket, `startWatchBridge()`'s `WatchNodes` bridge
 * (packages/dev-tools/src/dev-proxy.ts) never recovered — it retried
 * indefinitely even though the replacement daemon was fully healthy and
 * serving every other RPC.
 *
 * Root cause: grpc-js's `ClientReadableStream` reliably fires BOTH `'error'`
 * and `'end'` for a single dropped connection (confirmed live against a real
 * daemon kill with `GRPC_TRACE` enabled — every observed disconnect logged
 * "WatchNodes stream error" immediately followed by "WatchNodes stream
 * ended"). `connect()`'s `'error'` and `'end'` handlers each unconditionally
 * scheduled their own `setTimeout(connect, ...)` retry, so a single disconnect
 * kicked off TWO independent reconnect chains, each opening its own
 * concurrent `WatchNodes` stream. Every one of those that itself failed again
 * doubled the count once more — after a couple of daemon restarts, dev-proxy
 * had many overlapping reconnect chains in flight, which is what produced the
 * "never recovers" symptom: there was always at least one stacked chain
 * mid-retry, even though any single chain would have reconnected cleanly.
 *
 * This drives `createSingleFlightScheduler` (packages/dev-tools/src/grpc-client.ts)
 * DIRECTLY — the pure scheduling primitive `startWatchBridge()` uses to
 * collapse that 'error'+'end' pair into one reconnect attempt. It does not
 * spin up a real (or stub) gRPC server: the bug is pure JS event-handling
 * logic, reproducible by firing two callbacks for one logical failure, so no
 * daemon process is needed to cover it. The daemon-restart-recovery path
 * itself was verified live (two consecutive kill+restart cycles against a
 * real nodespaced on an isolated NODESPACE_HOME/socket, confirmed via
 * GRPC_TRACE showing exactly one `WatchNodes` stream attempt per genuine
 * disconnect, and via a live SSE subscription receiving a `nodeCreated` event
 * for a node created against the replacement daemon) — that requires two real
 * daemon processes and isn't practical to simulate here.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { createSingleFlightScheduler } from '../../../../dev-tools/src/grpc-client';

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('createSingleFlightScheduler', () => {
  it('runs a single trigger after its delay', () => {
    const scheduler = createSingleFlightScheduler();
    const callback = vi.fn();

    scheduler.trigger(1000, callback);
    expect(callback).not.toHaveBeenCalled();

    vi.advanceTimersByTime(1000);
    expect(callback).toHaveBeenCalledTimes(1);
  });

  it("collapses an 'error'+'end' pair for the same dropped stream into one reconnect — the core#2756 fix", () => {
    // Mirrors startWatchBridge()'s stream.on('error', ...) firing first with
    // a 2s delay, immediately followed by stream.on('end', ...) firing with a
    // 1s delay, for the SAME disconnect — the observed, reliable grpc-js
    // event ordering that caused the duplicate-reconnect bug.
    const scheduler = createSingleFlightScheduler();
    const onErrorReconnect = vi.fn();
    const onEndReconnect = vi.fn();

    scheduler.trigger(2000, onErrorReconnect);
    scheduler.trigger(1000, onEndReconnect); // must be a no-op: already pending

    vi.advanceTimersByTime(1000);
    // Pre-fix, onEndReconnect's independently-scheduled timer would have
    // fired here too, opening a second concurrent WatchNodes stream attempt.
    expect(onEndReconnect).not.toHaveBeenCalled();
    expect(onErrorReconnect).not.toHaveBeenCalled();

    vi.advanceTimersByTime(1000);
    expect(onErrorReconnect).toHaveBeenCalledTimes(1);
    expect(onEndReconnect).not.toHaveBeenCalled();
  });

  it('does not stack retries across repeated failures while one is already pending', () => {
    // Simulates several back-to-back terminal events arriving before the
    // pending reconnect fires (e.g. a flaky transport re-raising errors) —
    // without the guard this stacked one setTimeout per call.
    const scheduler = createSingleFlightScheduler();
    const callback = vi.fn();

    for (let i = 0; i < 5; i++) {
      scheduler.trigger(2000, callback);
    }

    vi.advanceTimersByTime(2000);
    expect(callback).toHaveBeenCalledTimes(1);
  });

  it('reset() allows a fresh trigger to schedule again, as connect() does at the top of each attempt', () => {
    const scheduler = createSingleFlightScheduler();
    const first = vi.fn();
    const second = vi.fn();

    scheduler.trigger(1000, first);
    vi.advanceTimersByTime(1000);
    expect(first).toHaveBeenCalledTimes(1);

    // A fresh connect() attempt (successful or not) resets the guard before
    // doing anything else, so its own eventual failure can schedule its own
    // next retry.
    scheduler.reset();
    scheduler.trigger(500, second);
    vi.advanceTimersByTime(500);
    expect(second).toHaveBeenCalledTimes(1);
  });

  it('without reset(), a second trigger after the first fires is still suppressed', () => {
    // Guards against a scheduler instance being reused without the connect()
    // call site actually reset()-ing it — the pending flag itself does not
    // self-clear when the delayed callback runs.
    const scheduler = createSingleFlightScheduler();
    const first = vi.fn();
    const second = vi.fn();

    scheduler.trigger(1000, first);
    vi.advanceTimersByTime(1000);
    expect(first).toHaveBeenCalledTimes(1);

    scheduler.trigger(500, second);
    vi.advanceTimersByTime(500);
    expect(second).not.toHaveBeenCalled();
  });
});
