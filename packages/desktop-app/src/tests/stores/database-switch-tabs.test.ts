/**
 * Runtime database switch against the real navigation store and tab
 * persistence: each database keeps its own saved tabs across a switch.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const existingNodes = new Set<string>();
vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    getNode: vi.fn(async (id: string) => (existingNodes.has(id) ? { id } : null))
  }
}));

vi.mock('$lib/services/shared-node-store.svelte', async (importOriginal) => {
  const actual = await importOriginal<typeof import('$lib/services/shared-node-store.svelte')>();
  return {
    ...actual,
    sharedNodeStore: {
      flushAllPendingSaves: vi.fn(() => Promise.resolve(new Set<string>())),
      clearAll: vi.fn()
    }
  };
});
vi.mock('$lib/stores/reactive-structure-tree.svelte', () => ({ structureTree: { clear: vi.fn() } }));
vi.mock('$lib/stores/collections.svelte', () => ({
  collectionsData: {
    loadCollections: vi.fn(),
    forgetLocallyCreated: vi.fn(),
    invalidateAllMembers: vi.fn()
  },
  collectionsState: { reset: vi.fn() }
}));
vi.mock('$lib/stores/schemas.svelte', () => ({
  schemasData: { loadSchemas: vi.fn(), invalidateForDatabaseSwitch: vi.fn() }
}));
vi.mock('$lib/plugins/schema-plugin-loader', () => ({
  resyncSchemaPluginsForDatabaseSwitch: vi.fn(() => Promise.resolve())
}));
vi.mock('$lib/stores/ai-chats.svelte', () => ({
  aiChatsData: { loadAiChats: vi.fn(), invalidateForDatabaseSwitch: vi.fn() }
}));
vi.mock('$lib/stores/plays.svelte', () => ({
  playsData: { loadPlays: vi.fn(), invalidateForDatabaseSwitch: vi.fn() }
}));
vi.mock('$lib/stores/conflicts.svelte', () => ({
  conflictsStore: { load: vi.fn(() => Promise.resolve(true)), invalidateForDatabaseSwitch: vi.fn() }
}));

import { TabPersistenceService } from '$lib/services/tab-persistence-service';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { databaseStore, type DatabaseInfo } from '$lib/stores/database.svelte';
import {
  navigationStore,
  addTab,
  resetTabState,
  DAILY_JOURNAL_TAB_ID,
  DEFAULT_PANE_ID
} from '$lib/stores/navigation.svelte';

function db(id: string): DatabaseInfo {
  return {
    id,
    name: `db-${id}`,
    path: `/tmp/${id}.db`,
    isDefault: false,
    status: 'closed',
    createdAt: '2026-01-01T00:00:00Z',
    lastOpenedAt: null,
    extensions: {}
  };
}

function openNode(id: string): void {
  addTab({
    id,
    title: id,
    type: 'node',
    content: { nodeId: id, nodeType: 'text' },
    closeable: true,
    paneId: DEFAULT_PANE_ID
  });
}

const tabIds = (): string[] => navigationStore.state.tabs.map((t) => t.id);

/** Let the debounced tab save land under the active database's key. */
async function settlePersist(): Promise<void> {
  await vi.advanceTimersByTimeAsync(1000);
}

describe('database switch restores saved tabs', () => {
  beforeEach(async () => {
    vi.useFakeTimers();
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined);
    localStorage.clear();
    existingNodes.clear();
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    databaseStore.databases = [db('a'), db('b'), db('c')];
    databaseStore.activeDatabaseId = 'a';
    resetTabState();
    // Mark the store initialised so tab changes persist.
    await navigationStore.restorePersistedState('a', async () => true);
  });

  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    vi.useRealTimers();
  });

  it('reopens the first database tabs after switching away and back', async () => {
    existingNodes.add('node-a');
    existingNodes.add('node-b');
    openNode('node-a');
    await settlePersist();

    await databaseStore.switchTo('b');
    // b has no saved tabs: the daily journal opens
    expect(tabIds()).toEqual([DAILY_JOURNAL_TAB_ID]);
    openNode('node-b');
    await settlePersist();

    await databaseStore.switchTo('a');
    expect(tabIds()).toContain('node-a');
    expect(tabIds()).not.toContain('node-b');

    await databaseStore.switchTo('b');
    expect(tabIds()).toContain('node-b');
    expect(tabIds()).not.toContain('node-a');
  });

  it('writes a pending save under the database being left, not the new one', async () => {
    existingNodes.add('node-a');
    openNode('node-a');
    // The debounced save has not fired yet when the switch starts
    await databaseStore.switchTo('b');
    await settlePersist();

    const savedFor = (id: string): string[] => {
      const raw = localStorage.getItem(`nodespace:tab-state:${id}`);
      return raw ? (JSON.parse(raw).tabs as { id: string }[]).map((t) => t.id) : [];
    };
    expect(savedFor('a')).toContain('node-a');
    expect(savedFor('b')).not.toContain('node-a');
  });

  it('opens the daily journal for a database with no saved tabs', async () => {
    openNode('node-a');
    await settlePersist();

    await databaseStore.switchTo('b');

    expect(tabIds()).toEqual([DAILY_JOURNAL_TAB_ID]);
    expect(navigationStore.state.activeTabIds[DEFAULT_PANE_ID]).toBe(DAILY_JOURNAL_TAB_ID);
  });

  it('drops saved tabs whose nodes no longer resolve', async () => {
    existingNodes.add('node-a');
    openNode('node-a');
    openNode('node-gone');
    await settlePersist();
    await databaseStore.switchTo('b');
    await settlePersist();

    await databaseStore.switchTo('a');

    expect(tabIds()).toContain('node-a');
    expect(tabIds()).not.toContain('node-gone');
  });

  it('falls back to the daily journal when no saved tab resolves', async () => {
    openNode('node-gone');
    await settlePersist();
    await databaseStore.switchTo('b');
    await settlePersist();

    await databaseStore.switchTo('a');

    expect(tabIds()).toEqual([DAILY_JOURNAL_TAB_ID]);
  });

  const savedFor = (id: string): string[] => {
    const raw = localStorage.getItem(`nodespace:tab-state:${id}`);
    return raw ? (JSON.parse(raw).tabs as { id: string }[]).map((t) => t.id) : [];
  };

  function seedSaved(dbId: string, nodeId: string): void {
    localStorage.setItem(
      `nodespace:tab-state:${dbId}`,
      JSON.stringify({
        version: 1,
        tabs: [
          {
            id: nodeId,
            title: nodeId,
            type: 'node',
            content: { nodeId, nodeType: 'text' },
            closeable: true,
            paneId: DEFAULT_PANE_ID
          }
        ],
        panes: [{ id: DEFAULT_PANE_ID, width: 100, tabIds: [nodeId] }],
        activePaneId: DEFAULT_PANE_ID,
        activeTabIds: { [DEFAULT_PANE_ID]: nodeId }
      })
    );
  }

  it('keeps persistence on when a restore is superseded by a switch that fails', async () => {
    seedSaved('b', 'node-b');
    const { backendAdapter } = await import('$lib/services/backend-adapter');
    let release: () => void = () => {};
    vi.mocked(backendAdapter.getNode).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          release = () => resolve({ id: 'node-b' } as never);
        })
    );

    const toB = databaseStore.switchTo('b');
    await vi.waitFor(() => expect(databaseStore.activeDatabaseId).toBe('b'));
    // b is validating its tabs; c starts and fails before committing
    vi.mocked(sharedNodeStore.flushAllPendingSaves).mockRejectedValueOnce(new Error('boom'));
    await databaseStore.switchTo('c');
    release();
    await toB;

    openNode('node-new');
    await settlePersist();
    expect(savedFor('b')).toContain('node-new');
    expect(savedFor('a')).not.toContain('node-new');
  });

  it('keeps persistence on when the restore throws', async () => {
    seedSaved('b', 'node-b');
    existingNodes.add('node-b');
    const spy = vi.spyOn(TabPersistenceService, 'pruneTabs').mockImplementationOnce(() => {
      throw new Error('corrupt');
    });

    await databaseStore.switchTo('b');
    spy.mockRestore();
    expect(databaseStore.error).not.toBeNull();

    openNode('node-new');
    await settlePersist();
    expect(savedFor('b')).toContain('node-new');
  });
});
