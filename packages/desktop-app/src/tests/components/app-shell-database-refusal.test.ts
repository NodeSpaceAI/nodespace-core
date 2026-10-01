/**
 * AppShell with a refused database: while the daemon refuses the active
 * database because it requires an extension this build does not support
 * (ADR-083 §2), the shell shows the refusal view in place of the sidebar and
 * the workspace, and goes back to them once another database opens.
 *
 * The real database store runs against a mocked app library that answers like
 * a daemon refusing the databases a test names: every command routed to a
 * refused database rejects with the REQUIRES_EXTENSION CommandError. The
 * sidebar, the workspace and the other chrome are stubbed; this checks only
 * which of the two the shell shows.
 */
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, cleanup, screen, waitFor } from '@testing-library/svelte';

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

import AppShell from '$lib/components/layout/app-shell.svelte';
import { databaseStore, type DatabaseInfo } from '$lib/stores/database.svelte';
import type { RequiresExtensionPayload } from '$lib/types/requires-extension';

const REFUSAL: RequiresExtensionPayload = {
  unsupportedExtensions: ['fixture-ext'],
  message: 'This database needs Fixture App',
  downloadLabel: 'Download Fixture App',
  downloadUrl: 'https://example.test/fixture-app'
};

function db(id: string, overrides: Partial<DatabaseInfo> = {}): DatabaseInfo {
  return {
    id,
    name: `db-${id}`,
    path: `/tmp/${id}.db`,
    isDefault: false,
    status: 'closed',
    createdAt: '',
    lastOpenedAt: null,
    extensions: {},
    ...overrides
  };
}

/**
 * Answer like the app library over a daemon that refuses the databases in
 * `refused`. Registry commands answer from `databases`; every other command
 * reads the routed database and rejects with the refusal when it is refused.
 */
function daemon(databases: DatabaseInfo[], defaultId: string, refused: string[]): void {
  let routed: string | null = null;
  mockInvoke.mockImplementation((cmd: string, args?: { id?: string }) => {
    switch (cmd) {
      case 'list_databases':
        return Promise.resolve({ databases, defaultDatabaseId: defaultId });
      case 'initial_database_id':
        return Promise.resolve(null);
      case 'set_active_database':
        routed = args?.id ?? null;
        return Promise.resolve();
      case 'pin_window_database':
        return Promise.resolve();
    }
    if (routed !== null && refused.includes(routed)) {
      return Promise.reject({
        message: REFUSAL.message,
        code: 'REQUIRES_EXTENSION',
        details: 'FailedPrecondition',
        requiresExtension: REFUSAL
      });
    }
    return Promise.resolve(cmd === 'get_stale_root_count' ? 0 : []);
  });
}

function refusalHeading(): HTMLElement | null {
  return screen.queryByRole('heading', { level: 1, name: REFUSAL.message });
}

function workspace(container: HTMLElement): Element | null {
  return container.querySelector('.pane-manager-wrapper');
}

describe('AppShell with a refused database', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    localStorage.clear();
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    databaseStore.databases = [];
    databaseStore.activeDatabaseId = null;
    databaseStore.defaultDatabaseId = null;
    databaseStore.error = null;
    databaseStore.refusal = null;
  });

  afterEach(() => {
    cleanup();
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    databaseStore.databases = [];
    databaseStore.activeDatabaseId = null;
    databaseStore.refusal = null;
  });

  it('shows the refusal instead of the workspace when the first read of the restored database is refused', async () => {
    localStorage.setItem('nodespace.activeDatabaseId', 'a');
    daemon([db('a'), db('b', { isDefault: true })], 'b', ['a']);
    const { container } = render(AppShell);

    await databaseStore.load();

    await waitFor(() => expect(refusalHeading()).not.toBeNull());
    expect(workspace(container)).toBeNull();
    expect(screen.getByRole('button', { name: REFUSAL.downloadLabel })).toBeTruthy();
  });

  it('shows the refusal when the registry default is refused at startup', async () => {
    daemon([db('a'), db('b', { isDefault: true, status: 'requires_extension' })], 'b', ['b']);
    const { container } = render(AppShell);

    await databaseStore.load();

    await waitFor(() => expect(refusalHeading()).not.toBeNull());
    expect(workspace(container)).toBeNull();
  });

  it('shows the workspace again once the user switches to a database that opens', async () => {
    daemon([db('a'), db('b', { isDefault: true, status: 'requires_extension' })], 'b', ['b']);
    const { container } = render(AppShell);
    await databaseStore.load();
    await waitFor(() => expect(refusalHeading()).not.toBeNull());

    await databaseStore.switchTo('a');

    await waitFor(() => expect(refusalHeading()).toBeNull());
    expect(workspace(container)).not.toBeNull();
    expect(databaseStore.activeDatabaseId).toBe('a');
  });

  it('holds the startup conflicts read for the first database that opens', async () => {
    daemon([db('a'), db('b', { isDefault: true, status: 'requires_extension' })], 'b', ['b']);
    render(AppShell);
    await databaseStore.load();
    await waitFor(() => expect(refusalHeading()).not.toBeNull());

    // Only the store's one-record refusal read touched the conflict journal.
    const journalLoads = () =>
      mockInvoke.mock.calls.filter(
        ([cmd, args]) =>
          cmd === 'list_conflicts' &&
          (args as { input: { limit: number | null } }).input.limit === null
      ).length;
    expect(journalLoads()).toBe(0);

    await databaseStore.switchTo('a');
    await waitFor(() => expect(refusalHeading()).toBeNull());
    // The database switch reloads the journal, and the startup notice reads it once more.
    await waitFor(() => expect(journalLoads()).toBe(2));
  });

  it('pauses the indexing-queue poll while the database is refused', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    try {
      daemon([db('a'), db('b', { isDefault: true, status: 'requires_extension' })], 'b', ['b']);
      render(AppShell);
      await databaseStore.load();
      await waitFor(() => expect(refusalHeading()).not.toBeNull());
      const polls = () =>
        mockInvoke.mock.calls.filter(([cmd]) => cmd === 'get_stale_root_count').length;
      const before = polls();

      vi.advanceTimersByTime(15000);
      expect(polls()).toBe(before);

      await databaseStore.switchTo('a');
      await waitFor(() => expect(refusalHeading()).toBeNull());
      vi.advanceTimersByTime(5000);
      expect(polls()).toBe(before + 1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('shows the workspace, not the refusal, for a database that opens', async () => {
    daemon([db('a', { isDefault: true })], 'a', []);
    const { container } = render(AppShell);

    await databaseStore.load();

    await waitFor(() => expect(databaseStore.activeDatabaseId).toBe('a'));
    expect(workspace(container)).not.toBeNull();
    expect(refusalHeading()).toBeNull();
  });
});
