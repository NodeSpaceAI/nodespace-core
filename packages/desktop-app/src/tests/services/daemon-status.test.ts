/**
 * Daemon status service — shared daemon-status listener that drives
 * AppShell's connecting/unreachable banner and fans out a reconnect signal
 * to daemon-dependent stores (schemas, collections, children-tree) so they
 * retry a failed load once the daemon becomes healthy, instead of staying
 * failed until a manual app restart.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const mockIsTauri = vi.fn(() => true);
// Defaults to a promise that never resolves, so the initial pull in
// startDaemonStatusListener() doesn't race the push-driven assertions below
// (tests that care about the pull path set this explicitly).
// Typed as a union because it stands in for several different commands: the
// status commands answer with a status string, `probe_and_recover_channel`
// with a boolean, and the incompatible-database commands with a record (or
// null). Inferring the type from this default alone would narrow it to
// `string` and reject what the other commands legitimately return.
const mockInvoke = vi.fn(
  (_cmd: string): Promise<string | boolean | object | null> => new Promise(() => {})
);
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({
    isTauri: () => mockIsTauri(),
    invoke: (cmd: string) => mockInvoke(cmd)
  })
);

// Capture the `daemon-status` handler so tests can drive it.
let daemonStatusHandler: ((event: { payload: string }) => void) | null = null;
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: string }) => void) => {
    if (name === 'daemon-status') {
      daemonStatusHandler = handler;
    }
    return () => {
      if (name === 'daemon-status') daemonStatusHandler = null;
    };
  })
}));

function emitDaemonStatus(payload: string) {
  daemonStatusHandler?.({ payload });
}

describe('daemon-status service', () => {
  beforeEach(async () => {
    vi.resetModules();
    vi.clearAllMocks();
    mockIsTauri.mockReturnValue(true);
    mockInvoke.mockReturnValue(new Promise<string>(() => {}));
    daemonStatusHandler = null;
  });

  afterEach(async () => {
    // Tear down the singleton's steady-state poll so its interval doesn't leak
    // into the next case (the module instance imported here is the same one the
    // test used — beforeEach's resetModules only fires before the next case).
    const { stopDaemonStatusListener } = await import('$lib/services/daemon-status');
    stopDaemonStatusListener();
  });

  it('clears connecting via the grace-period timer if no daemon-status event ever arrives', async () => {
    vi.useFakeTimers();
    try {
      const { daemonStatus, startDaemonStatusListener } = await import(
        '$lib/services/daemon-status'
      );
      startDaemonStatusListener();
      // Allow the async listen() registration to resolve without advancing timers.
      await vi.advanceTimersByTimeAsync(0);

      expect(get(daemonStatus).connecting).toBe(true);

      await vi.advanceTimersByTimeAsync(1500);

      expect(get(daemonStatus).connecting).toBe(false);
      expect(get(daemonStatus).unreachable).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('starts in a connecting state and clears it once a healthy event arrives', async () => {
    const { daemonStatus, startDaemonStatusListener } = await import('$lib/services/daemon-status');
    startDaemonStatusListener();
    // Allow the async listen() registration to resolve
    await Promise.resolve();
    await Promise.resolve();

    expect(get(daemonStatus).connecting).toBe(true);
    expect(get(daemonStatus).unreachable).toBe(false);

    emitDaemonStatus('healthy');

    expect(get(daemonStatus).connecting).toBe(false);
    expect(get(daemonStatus).unreachable).toBe(false);
  });

  it('marks unreachable on a not_running event and clears connecting', async () => {
    const { daemonStatus, startDaemonStatusListener } = await import('$lib/services/daemon-status');
    startDaemonStatusListener();
    await Promise.resolve();
    await Promise.resolve();

    emitDaemonStatus('not_running');

    expect(get(daemonStatus).connecting).toBe(false);
    expect(get(daemonStatus).unreachable).toBe(true);
  });

  it('clears unreachable once a later healthy event arrives', async () => {
    const { daemonStatus, startDaemonStatusListener } = await import('$lib/services/daemon-status');
    startDaemonStatusListener();
    await Promise.resolve();
    await Promise.resolve();

    emitDaemonStatus('not_running');
    expect(get(daemonStatus).unreachable).toBe(true);

    emitDaemonStatus('healthy');
    expect(get(daemonStatus).unreachable).toBe(false);
  });

  it('marks an incompatible database distinctly from a generic not_running', async () => {
    const { daemonStatus, startDaemonStatusListener } = await import('$lib/services/daemon-status');
    startDaemonStatusListener();
    await Promise.resolve();
    await Promise.resolve();

    emitDaemonStatus('incompatible_database');

    expect(get(daemonStatus)).toEqual({
      connecting: false,
      unreachable: false,
      incompatibleDatabase: true
    });

    emitDaemonStatus('not_running');
    expect(get(daemonStatus).incompatibleDatabase).toBe(false);
    expect(get(daemonStatus).unreachable).toBe(true);
  });

  it('resetIncompatibleDatabase applies the restarted status and fires reconnect listeners', async () => {
    const { daemonStatus, startDaemonStatusListener, onDaemonReconnect, resetIncompatibleDatabase } =
      await import('$lib/services/daemon-status');
    startDaemonStatusListener();
    await Promise.resolve();
    await Promise.resolve();
    emitDaemonStatus('incompatible_database');

    const callback = vi.fn();
    onDaemonReconnect(callback);
    const result = { backupPath: '/x/nodespace.db.incompatible-20260928-101500', status: 'healthy' };
    mockInvoke.mockResolvedValue(result);

    await expect(resetIncompatibleDatabase()).resolves.toEqual(result);

    expect(mockInvoke).toHaveBeenCalledWith('reset_incompatible_database');
    expect(get(daemonStatus).incompatibleDatabase).toBe(false);
    expect(callback).toHaveBeenCalledTimes(1);
  });

  it('resetIncompatibleDatabase leaves the state alone when the backend refuses', async () => {
    const { daemonStatus, startDaemonStatusListener, resetIncompatibleDatabase } = await import(
      '$lib/services/daemon-status'
    );
    startDaemonStatusListener();
    await Promise.resolve();
    await Promise.resolve();
    emitDaemonStatus('incompatible_database');

    mockInvoke.mockRejectedValue('there is no incompatible database to reset');

    await expect(resetIncompatibleDatabase()).rejects.toBe(
      'there is no incompatible database to reset'
    );
    expect(get(daemonStatus).incompatibleDatabase).toBe(true);
  });

  it('getIncompatibleDatabase returns the record, and null outside Tauri', async () => {
    const { getIncompatibleDatabase } = await import('$lib/services/daemon-status');
    const record = {
      databasePath: '/x/nodespace.db',
      detail: 'relationship: missing reverse_relationship_type',
      detectedAt: '2026-09-28T10:00:00Z'
    };
    mockInvoke.mockResolvedValue(record);
    await expect(getIncompatibleDatabase()).resolves.toEqual(record);
    expect(mockInvoke).toHaveBeenCalledWith('get_incompatible_database');

    mockInvoke.mockClear();
    mockIsTauri.mockReturnValue(false);
    await expect(getIncompatibleDatabase()).resolves.toBeNull();
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('fires onDaemonReconnect callbacks when the daemon transitions to healthy', async () => {
    const { startDaemonStatusListener, onDaemonReconnect } = await import(
      '$lib/services/daemon-status'
    );
    startDaemonStatusListener();
    await Promise.resolve();
    await Promise.resolve();

    const callback = vi.fn();
    onDaemonReconnect(callback);

    emitDaemonStatus('healthy');

    expect(callback).toHaveBeenCalledTimes(1);
  });

  it('does not fire reconnect callbacks for a not_running event', async () => {
    const { startDaemonStatusListener, onDaemonReconnect } = await import(
      '$lib/services/daemon-status'
    );
    startDaemonStatusListener();
    await Promise.resolve();
    await Promise.resolve();

    const callback = vi.fn();
    onDaemonReconnect(callback);

    emitDaemonStatus('not_running');

    expect(callback).not.toHaveBeenCalled();
  });

  it('does not re-fire reconnect callbacks for consecutive healthy events', async () => {
    const { startDaemonStatusListener, onDaemonReconnect } = await import(
      '$lib/services/daemon-status'
    );
    startDaemonStatusListener();
    await Promise.resolve();
    await Promise.resolve();

    const callback = vi.fn();
    onDaemonReconnect(callback);

    emitDaemonStatus('healthy');
    emitDaemonStatus('healthy');

    expect(callback).toHaveBeenCalledTimes(1);
  });

  it('fires reconnect callbacks again after a healthy -> not_running -> healthy cycle', async () => {
    const { startDaemonStatusListener, onDaemonReconnect } = await import(
      '$lib/services/daemon-status'
    );
    startDaemonStatusListener();
    await Promise.resolve();
    await Promise.resolve();

    const callback = vi.fn();
    onDaemonReconnect(callback);

    emitDaemonStatus('healthy');
    emitDaemonStatus('not_running');
    emitDaemonStatus('healthy');

    expect(callback).toHaveBeenCalledTimes(2);
  });

  it('an unsubscribe function stops further reconnect callbacks', async () => {
    const { startDaemonStatusListener, onDaemonReconnect } = await import(
      '$lib/services/daemon-status'
    );
    startDaemonStatusListener();
    await Promise.resolve();
    await Promise.resolve();

    const callback = vi.fn();
    const unsubscribe = onDaemonReconnect(callback);
    unsubscribe();

    emitDaemonStatus('healthy');

    expect(callback).not.toHaveBeenCalled();
  });

  it('does nothing outside Tauri (browser mode)', async () => {
    mockIsTauri.mockReturnValue(false);
    const { daemonStatus, startDaemonStatusListener } = await import('$lib/services/daemon-status');
    startDaemonStatusListener();
    await Promise.resolve();

    // Listener never registers, so state stays at its initial default.
    expect(get(daemonStatus).connecting).toBe(true);
    expect(daemonStatusHandler).toBeNull();
  });

  it('pulls the current status via check_daemon_status on start, not just push', async () => {
    mockInvoke.mockResolvedValue('healthy');
    const { daemonStatus, startDaemonStatusListener } = await import('$lib/services/daemon-status');
    startDaemonStatusListener();

    // No daemon-status event ever emitted — only the pull should settle this.
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    expect(mockInvoke).toHaveBeenCalledWith('check_daemon_status');
    expect(get(daemonStatus).connecting).toBe(false);
    expect(get(daemonStatus).unreachable).toBe(false);
  });

  it('fires onDaemonReconnect from the initial pull when no event ever arrives', async () => {
    mockInvoke.mockResolvedValue('healthy');
    const { startDaemonStatusListener, onDaemonReconnect } = await import(
      '$lib/services/daemon-status'
    );
    const callback = vi.fn();
    onDaemonReconnect(callback);

    startDaemonStatusListener();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    expect(callback).toHaveBeenCalledTimes(1);
  });

  it('refreshDaemonStatus re-pulls status and fires reconnect listeners on recovery', async () => {
    mockInvoke.mockResolvedValue('not_running');
    const { daemonStatus, startDaemonStatusListener, onDaemonReconnect, refreshDaemonStatus } =
      await import('$lib/services/daemon-status');
    startDaemonStatusListener();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    expect(get(daemonStatus).unreachable).toBe(true);

    const callback = vi.fn();
    onDaemonReconnect(callback);

    mockInvoke.mockResolvedValue('healthy');
    await refreshDaemonStatus();

    expect(get(daemonStatus).unreachable).toBe(false);
    expect(callback).toHaveBeenCalledTimes(1);
  });

  it('refreshDaemonStatus is a no-op before the listener has started', async () => {
    const { refreshDaemonStatus } = await import('$lib/services/daemon-status');
    await expect(refreshDaemonStatus()).resolves.toBeUndefined();
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('re-probes daemon health on a steady-state interval after start', async () => {
    vi.useFakeTimers();
    try {
      mockInvoke.mockResolvedValue('healthy');
      const { startDaemonStatusListener } = await import('$lib/services/daemon-status');
      startDaemonStatusListener();
      // Let the async listen() registration + the initial pull settle.
      await vi.advanceTimersByTimeAsync(0);
      const callsAfterStart = mockInvoke.mock.calls.length;
      expect(callsAfterStart).toBeGreaterThanOrEqual(1); // initial pull happened

      // One poll interval later, the steady-state poll fires another probe —
      // this is the signal source a mid-session restart needs, since the
      // backend never pushes daemon-status again after its startup task.
      await vi.advanceTimersByTimeAsync(15000);
      expect(mockInvoke.mock.calls.length).toBeGreaterThan(callsAfterStart);
    } finally {
      vi.useRealTimers();
    }
  });

  it('re-fires onDaemonReconnect when a steady-state poll observes a healthy → not_running → healthy restart', async () => {
    vi.useFakeTimers();
    try {
      let status = 'healthy';
      // The steady-state poll probes the gRPC channel after a healthy status,
      // so model both commands: the probe returns a boolean and must never be
      // mistaken for a status string. No wedge here → the channel is live.
      mockInvoke.mockImplementation((cmd: string) =>
        Promise.resolve(cmd === 'probe_and_recover_channel' ? false : status)
      );
      const { startDaemonStatusListener, onDaemonReconnect } = await import(
        '$lib/services/daemon-status'
      );
      const callback = vi.fn();
      onDaemonReconnect(callback);

      startDaemonStatusListener();
      // Initial pull observes healthy → first reconnect fires.
      await vi.advanceTimersByTimeAsync(0);
      expect(callback).toHaveBeenCalledTimes(1);

      // Daemon dies. Nothing is pushed mid-session; only the poll observes it.
      status = 'not_running';
      await vi.advanceTimersByTimeAsync(15000);
      expect(callback).toHaveBeenCalledTimes(1);

      // Daemon is relaunched. The poll observes healthy again and re-fires the
      // reconnect signal — the wedged-until-restart gap this fix closes.
      status = 'healthy';
      await vi.advanceTimersByTimeAsync(15000);
      expect(callback).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it('re-fires onDaemonReconnect when a steady-state poll finds a wedged channel and rebuilds it', async () => {
    vi.useFakeTimers();
    try {
      // The daemon stays healthy the whole time — the socket never drops. The
      // long-lived gRPC channel wedges instead, which the socket-based status
      // can't see. The probe reports it rebuilt the channel (true), and that
      // must re-fire reconnect even with no status transition for applyStatus
      // to observe.
      let wedged = false;
      mockInvoke.mockImplementation((cmd: string) =>
        Promise.resolve(cmd === 'probe_and_recover_channel' ? wedged : 'healthy')
      );
      const { startDaemonStatusListener, onDaemonReconnect } = await import(
        '$lib/services/daemon-status'
      );
      const callback = vi.fn();
      onDaemonReconnect(callback);

      startDaemonStatusListener();
      // Initial pull observes healthy → first reconnect fires (the initial
      // pull path never probes).
      await vi.advanceTimersByTimeAsync(0);
      expect(callback).toHaveBeenCalledTimes(1);

      // A poll with a live channel probes false → no extra reconnect.
      await vi.advanceTimersByTimeAsync(15000);
      expect(callback).toHaveBeenCalledTimes(1);

      // The channel wedges; the next poll's probe rebuilds it (true) and
      // re-fires reconnect so panes re-fetch on the fresh channel.
      wedged = true;
      await vi.advanceTimersByTimeAsync(15000);
      expect(callback).toHaveBeenCalledTimes(2);

      // Once rebuilt the probe reports live again → no repeated firing.
      wedged = false;
      await vi.advanceTimersByTimeAsync(15000);
      expect(callback).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });
});
