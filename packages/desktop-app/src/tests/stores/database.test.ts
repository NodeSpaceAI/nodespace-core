import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn()
  })
}));

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

// Collaborators exercised by switchTo — stubbed so we can assert the flush →
// switch → clear → reset → reload orchestration without their real behavior.
// `epochValue` backs currentEpoch() so tests can simulate a database switch
// landing while a settings refetch is in flight.
const flushAllPendingSaves = vi.fn((..._a: unknown[]) => Promise.resolve(new Set<string>()));
const clearAll = vi.fn((..._a: unknown[]) => undefined);
const setNode = vi.fn((..._a: unknown[]) => undefined);
// refreshDatabaseSettings() pins DATABASE_SETTINGS_NODE_ID reachable on
// every call (it backs always-mounted Pro-sync chrome with no structureTree
// relationship to any open tab — see database.svelte.ts's doc comment).
const pinNodes = vi.fn((..._a: unknown[]) => undefined);
let epochValue = 0;
vi.mock('$lib/services/shared-node-store.svelte', () => ({
  sharedNodeStore: {
    flushAllPendingSaves: (...a: unknown[]) => flushAllPendingSaves(...a),
    clearAll: (...a: unknown[]) => clearAll(...a),
    setNode: (...a: unknown[]) => setNode(...a),
    pinNodes: (...a: unknown[]) => pinNodes(...a),
    currentEpoch: () => epochValue
  }
}));

// The settings-node refetch goes through the backend adapter directly (it must
// bypass the cache-first ensureNode path).
const mockGetNode = vi.fn((..._a: unknown[]) => Promise.resolve<unknown>(null));
vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    getNode: (...a: unknown[]) => mockGetNode(...a)
  }
}));

const structureTreeClear = vi.fn((..._a: unknown[]) => undefined);
vi.mock('$lib/stores/reactive-structure-tree.svelte', () => ({
  structureTree: { clear: (...a: unknown[]) => structureTreeClear(...a) }
}));

const loadCollections = vi.fn((..._a: unknown[]) => undefined);
const forgetLocallyCreated = vi.fn((..._a: unknown[]) => undefined);
const invalidateAllMembers = vi.fn((..._a: unknown[]) => undefined);
const collectionsStateReset = vi.fn((..._a: unknown[]) => undefined);
vi.mock('$lib/stores/collections.svelte', () => ({
  collectionsData: {
    loadCollections: (...a: unknown[]) => loadCollections(...a),
    forgetLocallyCreated: (...a: unknown[]) => forgetLocallyCreated(...a),
    invalidateAllMembers: (...a: unknown[]) => invalidateAllMembers(...a)
  },
  collectionsState: {
    reset: (...a: unknown[]) => collectionsStateReset(...a)
  }
}));

const loadSchemas = vi.fn((..._a: unknown[]) => undefined);
const schemasInvalidateForDatabaseSwitch = vi.fn((..._a: unknown[]) => undefined);
vi.mock('$lib/stores/schemas.svelte', () => ({
  schemasData: {
    loadSchemas: (...a: unknown[]) => loadSchemas(...a),
    invalidateForDatabaseSwitch: (...a: unknown[]) => schemasInvalidateForDatabaseSwitch(...a)
  }
}));

const membershipInvalidateForDatabaseSwitch = vi.fn((..._a: unknown[]) => undefined);
vi.mock('$lib/stores/membership.svelte', () => ({
  membership: {
    invalidateForDatabaseSwitch: (...a: unknown[]) => membershipInvalidateForDatabaseSwitch(...a)
  }
}));

const resyncSchemaPluginsForDatabaseSwitch = vi.fn((..._a: unknown[]) => Promise.resolve());
vi.mock('$lib/plugins/schema-plugin-loader', () => ({
  resyncSchemaPluginsForDatabaseSwitch: (...a: unknown[]) =>
    resyncSchemaPluginsForDatabaseSwitch(...a)
}));

const loadAiChats = vi.fn((..._a: unknown[]) => undefined);
const invalidateForDatabaseSwitch = vi.fn((..._a: unknown[]) => undefined);
vi.mock('$lib/stores/ai-chats.svelte', () => ({
  aiChatsData: {
    loadAiChats: (...a: unknown[]) => loadAiChats(...a),
    invalidateForDatabaseSwitch: (...a: unknown[]) => invalidateForDatabaseSwitch(...a)
  }
}));

const clearAllTabs = vi.fn((..._a: unknown[]) => undefined);
const addTab = vi.fn((..._a: unknown[]) => undefined);
vi.mock('$lib/stores/navigation.svelte', () => ({
  clearAllTabs: (...a: unknown[]) => clearAllTabs(...a),
  addTab: (...a: unknown[]) => addTab(...a),
  DAILY_JOURNAL_TAB_ID: 'daily-journal',
  DEFAULT_PANE_ID: 'pane-1'
}));

vi.mock('$lib/utils/date-formatting', () => ({
  formatDateISO: () => '2026-07-09'
}));

import {
  databaseStore,
  isActiveDatabaseEvent,
  type DatabaseInfo
} from '$lib/stores/database.svelte';
import { DATABASE_SETTINGS_NODE_ID } from '$lib/plugins/ui-extensions';
import type { Node } from '$lib/types';

function db(id: string, overrides: Partial<DatabaseInfo> = {}): DatabaseInfo {
  return {
    id,
    name: `db-${id}`,
    path: `/tmp/${id}.db`,
    isDefault: false,
    status: 'closed',
    createdAt: '2026-01-01T00:00:00Z',
    lastOpenedAt: null,
    boundTenantSchema: null,
    boundTenantCollection: null,
    ...overrides
  };
}

function settingsNode(): Node {
  return {
    id: DATABASE_SETTINGS_NODE_ID,
    nodeType: 'database-settings',
    content: '',
    properties: { 'database-settings': { sync_enabled: false, auth_status: 'connected' } },
    mentions: [],
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1
  };
}

/** Let the fire-and-forget settings refetch (`.then` chain) settle. */
async function flushMicrotasks(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

describe('Database Store', () => {
  beforeEach(() => {
    // mockReset, not clearAllMocks: tests that install a persistent
    // mockImplementation would otherwise keep answering for every later test in
    // the file. clearAllMocks only clears recorded calls.
    mockInvoke.mockReset();
    vi.clearAllMocks();
    mockGetNode.mockReset();
    mockGetNode.mockResolvedValue(null);
    // The remembered-database id is read from localStorage at load(); a value
    // left behind by one test silently steers the next one.
    localStorage.clear();
    epochValue = 0;
    // The store gates `load()` on the Tauri bridge; present it so these tests
    // exercise the invoke path. The browser-mode describe removes it.
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    databaseStore.databases = [];
    databaseStore.activeDatabaseId = null;
    databaseStore.defaultDatabaseId = null;
    databaseStore.error = null;
  });

  afterEach(() => {
    // The bridge marker is installed for every test here, and several production
    // modules branch on its presence. Files share a vitest fork, so leaving it set
    // makes every later file run as though it were inside Tauri.
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  describe('load', () => {
    it('lists databases and initializes the active id to the default', async () => {
      mockInvoke.mockResolvedValueOnce({
        databases: [db('a'), db('b', { isDefault: true })],
        defaultDatabaseId: 'b'
      });

      await databaseStore.load();

      expect(mockInvoke).toHaveBeenCalledWith('list_databases');
      expect(databaseStore.databases).toHaveLength(2);
      expect(databaseStore.activeDatabaseId).toBe('b');
      expect(databaseStore.activeDatabase?.id).toBe('b');
    });

    it('falls back to the first database when no default is set', async () => {
      mockInvoke.mockResolvedValueOnce({
        databases: [db('a'), db('b')],
        defaultDatabaseId: ''
      });

      await databaseStore.load();

      expect(databaseStore.activeDatabaseId).toBe('a');
    });

    it('preserves an existing selection across a refresh', async () => {
      databaseStore.activeDatabaseId = 'a';
      mockInvoke.mockResolvedValueOnce({
        databases: [db('a'), db('b', { isDefault: true })],
        defaultDatabaseId: 'b'
      });

      await databaseStore.load();

      expect(databaseStore.activeDatabaseId).toBe('a');
    });

    it('opens the database this launch was told to open, over the remembered one', async () => {
      // The tray sets a launch-time selection when the user picks a specific
      // database from its submenu; that is a more specific instruction than
      // "whatever was open last time".
      localStorage.setItem('nodespace.activeDatabaseId', 'a');
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({
            databases: [db('a'), db('b'), db('c', { isDefault: true })],
            defaultDatabaseId: 'c'
          });
        }
        if (cmd === 'initial_database_id') return Promise.resolve('b');
        return Promise.resolve(undefined);
      });

      await databaseStore.load();

      expect(databaseStore.activeDatabaseId).toBe('b');
    });

    it('ignores a launch selection naming a database that is not registered', async () => {
      localStorage.setItem('nodespace.activeDatabaseId', 'a');
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({
            databases: [db('a'), db('c', { isDefault: true })],
            defaultDatabaseId: 'c'
          });
        }
        if (cmd === 'initial_database_id') return Promise.resolve('gone');
        return Promise.resolve(undefined);
      });

      await databaseStore.load();

      expect(databaseStore.activeDatabaseId).toBe('a');
    });

    it('does not let a concurrent load overwrite the selection already made', async () => {
      // A second load() runs on every launch (the daemon-reconnect listener
      // fires one), and both can pass the "no selection yet" check before
      // either assigns. What keeps them agreeing is that the launch id answers
      // the same thing to both — an earlier attempt to consume it on first read
      // made the second load resolve to the remembered database and overwrite
      // the tray's pick. This pins the outcome, so re-introducing a
      // consume-once read fails here rather than silently in the product.
      localStorage.setItem('nodespace.activeDatabaseId', 'a');
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({
            databases: [db('a'), db('b'), db('c', { isDefault: true })],
            defaultDatabaseId: 'c'
          });
        }
        if (cmd === 'initial_database_id') return Promise.resolve('b');
        return Promise.resolve(undefined);
      });

      await Promise.all([databaseStore.load(), databaseStore.load()]);

      expect(databaseStore.activeDatabaseId).toBe('b');
    });

    it('records an error when the list fails', async () => {
      mockInvoke.mockRejectedValueOnce(new Error('boom'));
      await databaseStore.load();
      expect(databaseStore.error).toContain('boom');
    });
  });

  describe('load in browser dev mode (no Tauri bridge)', () => {
    beforeEach(() => {
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    });

    it('presents a single implicit database without erroring or invoking', async () => {
      await databaseStore.load();

      // No registry query is attempted (the bridge, hence DatabaseService, is absent).
      expect(mockInvoke).not.toHaveBeenCalledWith('list_databases');
      expect(databaseStore.error).toBeNull();
      expect(databaseStore.databases).toHaveLength(1);
      expect(databaseStore.activeDatabaseId).toBe(databaseStore.databases[0].id);
      expect(databaseStore.activeDatabase).not.toBeNull();
    });
  });

  describe('switchTo', () => {
    beforeEach(() => {
      databaseStore.databases = [db('a'), db('b')];
      databaseStore.activeDatabaseId = 'a';
    });

    it('flushes, switches, clears caches, resets tabs, and reloads', async () => {
      mockInvoke.mockResolvedValue(undefined); // set_active_database + pro_activate_database

      await databaseStore.switchTo('b');

      expect(flushAllPendingSaves).toHaveBeenCalledOnce();
      expect(mockInvoke).toHaveBeenCalledWith('set_active_database', { id: 'b' });
      // The switch also re-targets Pro cloud-sync to the new database (ADR-053).
      // Dispatched via a serialized chain (prevents a rapid A->B->A switch
      // from resolving out of send order) rather than synchronously within
      // switchTo, so it lands a microtask or two after switchTo resolves.
      await vi.waitFor(() =>
        expect(mockInvoke).toHaveBeenCalledWith('pro_activate_database', { databaseId: 'b' })
      );
      expect(databaseStore.activeDatabaseId).toBe('b');
      expect(clearAll).toHaveBeenCalledOnce();
      expect(structureTreeClear).toHaveBeenCalledOnce();
      expect(clearAllTabs).toHaveBeenCalledOnce();
      expect(addTab).toHaveBeenCalledOnce();
      expect(loadCollections).toHaveBeenCalledOnce();
      expect(loadSchemas).toHaveBeenCalledOnce();
      expect(loadAiChats).toHaveBeenCalledOnce();
      // The hide-empty exemptions are per-database (collection ids are derived
      // from the name), so they are dropped before the new database loads —
      // otherwise a same-named empty collection would be un-hidden there.
      expect(forgetLocallyCreated).toHaveBeenCalledOnce();
      expect(forgetLocallyCreated.mock.invocationCallOrder[0]).toBeLessThan(
        loadCollections.mock.invocationCallOrder[0]
      );
      // Same discipline for ai-chats: an in-flight "+ New chat" create must be
      // invalidated before the reload, so its result can't land in the store
      // representing the newly-active database.
      expect(invalidateForDatabaseSwitch).toHaveBeenCalledOnce();
      expect(invalidateForDatabaseSwitch.mock.invocationCallOrder[0]).toBeLessThan(
        loadAiChats.mock.invocationCallOrder[0]
      );
      // Same discipline for schemas — an in-flight loadSchemas must
      // be invalidated before the reload, so its result can't land in the
      // store representing the newly-active database.
      expect(schemasInvalidateForDatabaseSwitch).toHaveBeenCalledOnce();
      expect(schemasInvalidateForDatabaseSwitch.mock.invocationCallOrder[0]).toBeLessThan(
        loadSchemas.mock.invocationCallOrder[0]
      );
      // The per-collection member cache and the sub-panel selection
      // are both keyed on a collection id that is name-derived and can collide
      // across databases — both must be dropped so a same-named collection in
      // the new database can't render the previous database's cached members.
      expect(invalidateAllMembers).toHaveBeenCalledOnce();
      expect(collectionsStateReset).toHaveBeenCalledOnce();
      // The Pro membership roster/invites/requests cache has the
      // same id-collision hazard (has_role edges are per-database, ADR-053).
      expect(membershipInvalidateForDatabaseSwitch).toHaveBeenCalledOnce();
      // The schema plugin registry (hasTitleTemplate/titleTemplate)
      // must re-sync against the newly-active database's schemas.
      expect(resyncSchemaPluginsForDatabaseSwitch).toHaveBeenCalledOnce();
      // switchTo wraps its whole body in a try/catch that only records the
      // failure on `this.error`, so anything that throws mid-switch — an
      // incomplete mock being the usual culprit — is otherwise swallowed and
      // shows up as a confusing "spy not called" rather than the real cause.
      // Asserting the switch left no error catches that wherever it happens,
      // including after the last side effect asserted above.
      expect(databaseStore.error).toBeNull();
    });

    it('completes the switch even if the Pro sync re-target fails', async () => {
      mockInvoke
        .mockResolvedValueOnce(undefined) // set_active_database
        .mockRejectedValueOnce(new Error('daemon unavailable')); // pro_activate_database

      await databaseStore.switchTo('b');

      // A sync re-target failure is best-effort: the already-committed routing
      // switch still lands (caches cleared, tabs reset), never left half-done.
      expect(databaseStore.activeDatabaseId).toBe('b');
      expect(clearAll).toHaveBeenCalledOnce();
      expect(clearAllTabs).toHaveBeenCalledOnce();
      // Dispatched via a serialized chain, so it lands a microtask or two
      // after switchTo resolves — and its rejection must not surface.
      await vi.waitFor(() =>
        expect(mockInvoke).toHaveBeenCalledWith('pro_activate_database', { databaseId: 'b' })
      );
      expect(databaseStore.error).toBeNull();
    });

    it('does not dispatch a later pro_activate_database call until an earlier one settles, so a rapid switch sequence cannot land out of send order', async () => {
      databaseStore.databases = [db('a'), db('b'), db('c')];
      databaseStore.activeDatabaseId = 'a';

      let resolveActivateB: (() => void) | null = null;
      const activateCalls: string[] = [];
      mockInvoke.mockImplementation((cmd: string, args?: { databaseId?: string }) => {
        if (cmd === 'pro_activate_database') {
          activateCalls.push(args!.databaseId!);
          if (args!.databaseId === 'b') {
            return new Promise<void>((resolve) => {
              resolveActivateB = () => resolve();
            });
          }
        }
        return Promise.resolve(undefined);
      });

      await databaseStore.switchTo('b');
      // switchTo('b') has resolved, but its fire-and-forget pro_activate_database('b')
      // call is still pending (deliberately unresolved above).
      await databaseStore.switchTo('c');

      // 'c'’s pro_activate_database call must not be dispatched yet — it's
      // queued behind 'b'’s still-unsettled one, so a slow-to-resolve 'b' can
      // never have its eventual response applied AFTER 'c'’s on the Rust side.
      expect(activateCalls).toEqual(['b']);

      resolveActivateB!();
      await vi.waitFor(() => expect(activateCalls).toEqual(['b', 'c']));
    });

    it('no-ops when switching to the already-active database', async () => {
      await databaseStore.switchTo('a');
      expect(mockInvoke).not.toHaveBeenCalled();
      expect(clearAll).not.toHaveBeenCalled();
    });

    it('ignores a switch to an id that is not in the registered databases list, even after a fresh registry pull', async () => {
      // Every UI call site only ever passes an id drawn from `databases`; the
      // one caller that doesn't control its input is the tray's
      // `tray:select-database` relaunch event (the database could have been
      // removed between the tray click and the event arriving). Committing to
      // it anyway would clear every cache and reset the workspace before the
      // daemon's per-request NOT_FOUND ever surfaced. The guard re-checks
      // against a fresh `list_databases` pull before rejecting — this id is
      // still absent from that fresh pull, so it must still be rejected.
      mockInvoke.mockResolvedValueOnce({
        databases: [db('a'), db('b')],
        defaultDatabaseId: ''
      });

      await databaseStore.switchTo('does-not-exist');

      expect(mockInvoke).toHaveBeenCalledWith('list_databases');
      expect(mockInvoke).not.toHaveBeenCalledWith('set_active_database', expect.anything());
      expect(clearAll).not.toHaveBeenCalled();
      expect(databaseStore.activeDatabaseId).toBe('a');
    });

    it('recognizes a database registered after boot instead of dropping the tray pick', async () => {
      // The frontend's `databases` list is loaded once at boot; a database
      // registered later via another path (the shipped CLI, another window)
      // is legitimately switchable — the daemon tray's submenu already
      // live-refreshes and offers it — even though this store has not heard
      // of it yet. The guard must re-pull the registry rather than reject
      // outright.
      databaseStore.databases = [db('a')];
      databaseStore.activeDatabaseId = 'a';
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({
            databases: [db('a'), db('work')],
            defaultDatabaseId: ''
          });
        }
        return Promise.resolve(undefined); // set_active_database, pro_activate_database
      });

      await databaseStore.switchTo('work');

      expect(mockInvoke).toHaveBeenCalledWith('list_databases');
      expect(mockInvoke).toHaveBeenCalledWith('set_active_database', { id: 'work' });
      expect(databaseStore.activeDatabaseId).toBe('work');
      expect(databaseStore.databases.map((d) => d.id)).toEqual(['a', 'work']);
      expect(clearAll).toHaveBeenCalledOnce();
    });

    it('recognizes a tray pick arriving before the very first load() resolves, when databases is still empty', async () => {
      databaseStore.databases = [];
      databaseStore.activeDatabaseId = null;
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({
            databases: [db('a'), db('b')],
            defaultDatabaseId: ''
          });
        }
        return Promise.resolve(undefined);
      });

      await databaseStore.switchTo('b');

      expect(mockInvoke).toHaveBeenCalledWith('list_databases');
      expect(mockInvoke).toHaveBeenCalledWith('set_active_database', { id: 'b' });
      expect(databaseStore.activeDatabaseId).toBe('b');
    });

    it('flushes pending saves BEFORE re-pointing the routed clients', async () => {
      const order: string[] = [];
      flushAllPendingSaves.mockImplementationOnce(async () => {
        order.push('flush');
        return new Set<string>();
      });
      mockInvoke.mockImplementationOnce(async () => {
        order.push('switch');
      });

      await databaseStore.switchTo('b');

      expect(order).toEqual(['flush', 'switch']);
    });

    it('lets the latest of two concurrent switches win and bails the superseded one', async () => {
      databaseStore.databases = [db('a'), db('b'), db('c')];
      databaseStore.activeDatabaseId = 'a';
      mockInvoke.mockResolvedValue(undefined); // set_active_database (winner only)

      // Fire both without awaiting: each captures its switch token synchronously
      // before the first await, so the later call supersedes the earlier one.
      const first = databaseStore.switchTo('b');
      const second = databaseStore.switchTo('c');
      await Promise.all([first, second]);

      expect(databaseStore.activeDatabaseId).toBe('c');
      // The superseded switch bailed before re-pointing the routed clients.
      expect(mockInvoke).not.toHaveBeenCalledWith('set_active_database', { id: 'b' });
      expect(mockInvoke).toHaveBeenCalledWith('set_active_database', { id: 'c' });
      // Only the winner cleared caches and reloaded — no stale mid-switch state.
      expect(clearAll).toHaveBeenCalledOnce();
      expect(loadCollections).toHaveBeenCalledOnce();
    });
  });

  describe('applyPendingTraySelection', () => {
    beforeEach(() => {
      databaseStore.databases = [db('a'), db('b')];
      databaseStore.activeDatabaseId = 'a';
    });

    it('switches to the pending selection when one was stashed', async () => {
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === 'take_pending_tray_database_selection') return Promise.resolve('b');
        return Promise.resolve(undefined); // set_active_database, pro_activate_database
      });

      await databaseStore.applyPendingTraySelection();

      expect(mockInvoke).toHaveBeenCalledWith('take_pending_tray_database_selection');
      expect(mockInvoke).toHaveBeenCalledWith('set_active_database', { id: 'b' });
      expect(databaseStore.activeDatabaseId).toBe('b');
    });

    it('is a no-op when nothing was stashed', async () => {
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === 'take_pending_tray_database_selection') return Promise.resolve(null);
        return Promise.resolve(undefined);
      });

      await databaseStore.applyPendingTraySelection();

      expect(mockInvoke).toHaveBeenCalledWith('take_pending_tray_database_selection');
      expect(mockInvoke).not.toHaveBeenCalledWith('set_active_database', expect.anything());
      expect(databaseStore.activeDatabaseId).toBe('a');
    });

    it('swallows a read failure rather than throwing', async () => {
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === 'take_pending_tray_database_selection') {
          return Promise.reject(new Error('ipc failure'));
        }
        return Promise.resolve(undefined);
      });

      await expect(databaseStore.applyPendingTraySelection()).resolves.toBeUndefined();
      expect(databaseStore.activeDatabaseId).toBe('a');
    });

    it('does nothing outside Tauri (no bridge, no invoke)', async () => {
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;

      await databaseStore.applyPendingTraySelection();

      expect(mockInvoke).not.toHaveBeenCalled();
    });
  });

  describe('refreshDatabaseSettings', () => {
    it('force-refetches the settings node via the adapter and lands it in the shared store', async () => {
      const node = settingsNode();
      mockGetNode.mockResolvedValueOnce(node);

      databaseStore.refreshDatabaseSettings();
      await flushMicrotasks();

      // Bypasses the cache-first ensureNode path: always a fresh backend read.
      expect(mockGetNode).toHaveBeenCalledWith(DATABASE_SETTINGS_NODE_ID);
      expect(setNode).toHaveBeenCalledWith(
        node,
        { type: 'database', reason: 'refresh-database-settings' },
        true
      );
    });

    it('pins DATABASE_SETTINGS_NODE_ID reachable, unconditionally and before the fetch resolves', () => {
      // The settings node backs always-mounted Pro-sync chrome (the sync
      // pill, membership gating) with no structureTree relationship to any
      // open tab, so SharedNodeStore's eviction feature can't see it as
      // reachable on its own — without this pin call it would be evicted
      // ~30s after the app's first tab-state report regardless of how often
      // it's refreshed. Asserted synchronously (no await): the pin must
      // happen up front, not only after the async fetch settles.
      databaseStore.refreshDatabaseSettings();

      expect(pinNodes).toHaveBeenCalledWith(expect.any(String), [DATABASE_SETTINGS_NODE_ID]);
    });

    it('re-pins on every refresh, so a database switch (which clears every pin) gets it back', async () => {
      mockGetNode.mockResolvedValue(settingsNode());

      databaseStore.refreshDatabaseSettings();
      await flushMicrotasks();
      pinNodes.mockClear();

      // Simulates the pin having been wiped by clearAll() during a switch —
      // the next refresh must re-establish it, not assume it's still there.
      databaseStore.refreshDatabaseSettings();

      expect(pinNodes).toHaveBeenCalledWith(expect.any(String), [DATABASE_SETTINGS_NODE_ID]);
    });

    it('load() hydrates the settings node through the same forced refetch', async () => {
      mockInvoke.mockResolvedValueOnce({
        databases: [db('a', { isDefault: true })],
        defaultDatabaseId: 'a'
      });
      mockGetNode.mockResolvedValueOnce(settingsNode());

      await databaseStore.load();
      await flushMicrotasks();

      expect(mockGetNode).toHaveBeenCalledWith(DATABASE_SETTINGS_NODE_ID);
      expect(setNode).toHaveBeenCalledOnce();
    });

    it('a missing settings node (older database) leaves the store untouched', async () => {
      mockGetNode.mockResolvedValueOnce(null);

      databaseStore.refreshDatabaseSettings();
      await flushMicrotasks();

      expect(setNode).not.toHaveBeenCalled();
    });

    it('drops the fetched node when the database epoch changed mid-flight', async () => {
      // The fetch resolves only after a database switch bumped the epoch — the
      // row belongs to the previous database and must not land.
      mockGetNode.mockImplementationOnce(async () => {
        epochValue = 1;
        return settingsNode();
      });

      databaseStore.refreshDatabaseSettings();
      await flushMicrotasks();

      expect(setNode).not.toHaveBeenCalled();
    });

    it('is fire-and-forget: a failed refetch is swallowed at debug level', async () => {
      mockGetNode.mockRejectedValueOnce(new Error('daemon unavailable'));

      expect(() => databaseStore.refreshDatabaseSettings()).not.toThrow();
      await flushMicrotasks();

      expect(setNode).not.toHaveBeenCalled();
    });

    it('switchTo re-pulls the new database settings after the caches are cleared', async () => {
      databaseStore.databases = [db('a'), db('b')];
      databaseStore.activeDatabaseId = 'a';
      mockInvoke.mockResolvedValueOnce(undefined); // set_active_database
      mockGetNode.mockResolvedValueOnce(settingsNode());

      await databaseStore.switchTo('b');
      await flushMicrotasks();

      expect(mockGetNode).toHaveBeenCalledWith(DATABASE_SETTINGS_NODE_ID);
      expect(setNode).toHaveBeenCalledOnce();
    });
  });

  describe('registry mutations', () => {
    it('create refreshes the list and returns the new entry', async () => {
      const created = db('c', { name: 'Work' });
      mockInvoke
        .mockResolvedValueOnce(created) // create_database
        .mockResolvedValueOnce({ databases: [created], defaultDatabaseId: '' }); // load

      const result = await databaseStore.create('Work');

      expect(mockInvoke).toHaveBeenCalledWith('create_database', { name: 'Work', path: null });
      expect(result?.id).toBe('c');
      expect(databaseStore.databases).toHaveLength(1);
    });

    it('register passes the path through', async () => {
      const registered = db('d');
      mockInvoke
        .mockResolvedValueOnce(registered)
        .mockResolvedValueOnce({ databases: [registered], defaultDatabaseId: '' });

      await databaseStore.register('/tmp/d.db');

      expect(mockInvoke).toHaveBeenCalledWith('register_database', { path: '/tmp/d.db' });
    });

    it('remove unregisters and refreshes', async () => {
      databaseStore.databases = [db('a'), db('b')];
      databaseStore.activeDatabaseId = 'a';
      mockInvoke
        .mockResolvedValueOnce('b') // remove_database
        .mockResolvedValueOnce({ databases: [db('a')], defaultDatabaseId: '' }); // load

      await databaseStore.remove('b');

      expect(mockInvoke).toHaveBeenCalledWith('remove_database', { id: 'b' });
      expect(databaseStore.databases).toHaveLength(1);
    });
  });

  describe('isActiveDatabaseEvent', () => {
    it('passes events with no database id (single-database / Pro daemon)', () => {
      databaseStore.activeDatabaseId = 'a';
      expect(isActiveDatabaseEvent(undefined)).toBe(true);
      expect(isActiveDatabaseEvent('')).toBe(true);
    });

    it('passes any event before a selection is loaded', () => {
      databaseStore.activeDatabaseId = null;
      expect(isActiveDatabaseEvent('a')).toBe(true);
    });

    it('drops events tagged for a different database', () => {
      databaseStore.activeDatabaseId = 'a';
      expect(isActiveDatabaseEvent('a')).toBe(true);
      expect(isActiveDatabaseEvent('b')).toBe(false);
    });
  });
});
