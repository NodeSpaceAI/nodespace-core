/**
 * AppShell's not-running banner: its Retry starts the daemon again
 * (`retryDaemonStart`, the `retry_daemon_start` command) rather than only
 * re-reading the status. A start is what brings a stopped daemon back, and
 * the only thing that lets the app's held calls through once its own daemon
 * answers, including after the hold's limit has passed.
 *
 * The sidebar, the workspace and the other chrome are stubbed, and the status
 * comes from a source the test drives; this checks only the banner and what
 * its Retry runs.
 */
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, cleanup, screen, waitFor, fireEvent } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';
vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
  emit: vi.fn(() => Promise.resolve())
}));

vi.mock('$lib/components/layout/navigation-sidebar.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/layout/pane-manager.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/status-bar.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/onboarding/onboarding-wizard.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/references/node-ref-preview.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/conflict-toast.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/update-banner.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/settings/import-options-modal.svelte', () => ({ default: () => {} }));
vi.mock('$lib/stores/update-status.svelte', () => ({
  updateStatus: { init: () => Promise.resolve(), stop: () => {} }
}));

// The real status service, with the start its Retry runs stood in for.
const mockRetryDaemonStart = vi.fn<() => Promise<string>>();
vi.mock('$lib/services/daemon-status', async () => ({
  ...(await vi.importActual<typeof import('$lib/services/daemon-status')>(
    '$lib/services/daemon-status'
  )),
  retryDaemonStart: () => mockRetryDaemonStart()
}));

import AppShell from '$lib/components/layout/app-shell.svelte';
import { startDaemonStatusListener, stopDaemonStatusListener } from '$lib/services/daemon-status';

const NOT_RUNNING = /background service is not running/i;

/** Starts the status service on a daemon reported down; returns a way to push a status. */
function daemonDown(): (status: string) => void {
  let push: (status: string) => void = () => {};
  startDaemonStatusListener({
    getCurrent: () => Promise.resolve('not_running'),
    subscribe(callback) {
      push = callback;
      return () => {};
    }
  });
  return (status) => push(status);
}

/** Makes the next Retry's start resolve to whatever the returned function is given. */
function pendingStart(): (status: string) => void {
  let finish: (status: string) => void = () => {};
  mockRetryDaemonStart.mockReturnValue(new Promise<string>((resolve) => (finish = resolve)));
  return (status) => finish(status);
}

describe('AppShell daemon banner Retry', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(null);
    mockRetryDaemonStart.mockReset();
    localStorage.clear();
  });

  afterEach(() => {
    cleanup();
    stopDaemonStatusListener();
  });

  it('starts the daemon again, and the banner goes once the daemon is healthy', async () => {
    const push = daemonDown();
    const finish = pendingStart();
    render(AppShell);
    await waitFor(() => expect(screen.getByText(NOT_RUNNING)).toBeTruthy());

    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    expect(mockRetryDaemonStart).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'Retrying…' }).hasAttribute('disabled')).toBe(true);

    finish('healthy');
    push('healthy');
    await waitFor(() => expect(screen.queryByText(NOT_RUNNING)).toBeNull());
  });

  it('keeps the banner and a usable Retry when the start finds the daemon still down', async () => {
    daemonDown();
    const finish = pendingStart();
    render(AppShell);
    await waitFor(() => expect(screen.getByText(NOT_RUNNING)).toBeTruthy());

    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    finish('not_running');

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Retry' }).hasAttribute('disabled')).toBe(false);
    });
    expect(screen.getByText(NOT_RUNNING)).toBeTruthy();
  });
});
