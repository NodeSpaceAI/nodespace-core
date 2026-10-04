import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';

const logWarn = vi.fn();
vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: (...a: unknown[]) => logWarn(...a),
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
const flushAllPendingSaves = vi.fn((..._a: unknown[]) => Promise.resolve(new Set<string>()));
const clearAll = vi.fn((..._a: unknown[]) => undefined);
vi.mock('$lib/services/shared-node-store.svelte', () => ({
  sharedNodeStore: {
    flushAllPendingSaves: (...a: unknown[]) => flushAllPendingSaves(...a),
    clearAll: (...a: unknown[]) => clearAll(...a)
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

const loadPlays = vi.fn((..._a: unknown[]) => undefined);
const playsInvalidateForDatabaseSwitch = vi.fn((..._a: unknown[]) => undefined);
vi.mock('$lib/stores/plays.svelte', () => ({
  playsData: {
    loadPlays: (...a: unknown[]) => loadPlays(...a),
    invalidateForDatabaseSwitch: (...a: unknown[]) => playsInvalidateForDatabaseSwitch(...a)
  }
}));

const loadConflicts = vi.fn((..._a: unknown[]) => Promise.resolve(true));
const conflictsInvalidateForDatabaseSwitch = vi.fn((..._a: unknown[]) => undefined);
vi.mock('$lib/stores/conflicts.svelte', () => ({
  conflictsStore: {
    load: (...a: unknown[]) => loadConflicts(...a),
    invalidateForDatabaseSwitch: (...a: unknown[]) => conflictsInvalidateForDatabaseSwitch(...a)
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
import type { RequiresExtensionPayload } from '$lib/types/requires-extension';
import { uiExtensionRegistry } from '$lib/plugins/ui-extensions';
import { TEST_EXTENSION_ID, createTestExtension } from '../fixtures/test-extension';
import { createTestLifecycleParts } from '../fixtures/test-extension/lifecycle';

function db(id: string, overrides: Partial<DatabaseInfo> = {}): DatabaseInfo {
  return {
    id,
    name: `db-${id}`,
    path: `/tmp/${id}.db`,
    isDefault: false,
    status: 'closed',
    createdAt: '2026-01-01T00:00:00Z',
    lastOpenedAt: null,
    extensions: {},
    ...overrides
  };
}

const REFUSAL: RequiresExtensionPayload = {
  unsupportedExtensions: ['fixture-ext'],
  message: 'This database needs Fixture App',
  downloadLabel: 'Download Fixture App',
  downloadUrl: 'https://example.test/fixture-app'
};

/** The error the app library returns for any command routed to a refused database. */
function refusedRead(): Record<string, unknown> {
  return {
    message: REFUSAL.message,
    code: 'REQUIRES_EXTENSION',
    details: 'FailedPrecondition',
    requiresExtension: REFUSAL
  };
}

/**
 * Answer like the app library over a daemon that refuses the databases in
 * `refused`: the registry commands answer from `databases`, and every other
 * command reads the routed database, rejecting when it is refused.
 * `routedTo` is the database routing already points at.
 */
function daemon(
  databases: DatabaseInfo[],
  defaultId: string,
  refused: string[],
  routedTo: string | null = null
): void {
  let routed = routedTo;
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
      default:
        return routed !== null && refused.includes(routed)
          ? Promise.reject(refusedRead())
          : Promise.resolve([]);
    }
  });
}

describe('Database Store', () => {
  beforeEach(() => {
    // mockReset, not clearAllMocks: tests that install a persistent
    // mockImplementation would otherwise keep answering for every later test in
    // the file. clearAllMocks only clears recorded calls.
    mockInvoke.mockReset();
    vi.clearAllMocks();
    // The remembered-database id is read from localStorage at load(); a value
    // left behind by one test silently steers the next one.
    localStorage.clear();
    // The store gates `load()` on the Tauri bridge; present it so these tests
    // exercise the invoke path. The browser-mode describe removes it.
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    databaseStore.databases = [];
    databaseStore.activeDatabaseId = null;
    databaseStore.defaultDatabaseId = null;
    databaseStore.error = null;
    databaseStore.refusal = null;
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

    it('routes the gRPC clients to the restored database before committing the selection', async () => {
      // A remembered (non-default) database must also become the routing
      // target; otherwise the switcher shows it while every request still goes
      // to the daemon default.
      localStorage.setItem('nodespace.activeDatabaseId', 'a');
      let activeWhenRouted: string | null | undefined;
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({
            databases: [db('a'), db('b', { isDefault: true })],
            defaultDatabaseId: 'b'
          });
        }
        if (cmd === 'set_active_database') activeWhenRouted = databaseStore.activeDatabaseId;
        return Promise.resolve(undefined);
      });

      await databaseStore.load();

      expect(mockInvoke).toHaveBeenCalledWith('set_active_database', { id: 'a' });
      expect(activeWhenRouted).toBeNull();
      expect(databaseStore.activeDatabaseId).toBe('a');
      expect(mockInvoke).toHaveBeenCalledWith('pin_window_database', { id: 'a' });
    });

    it('reloads the database-scoped stores when the restored database is not the default', async () => {
      // The sidebar's boot-time loads went out unrouted (answered by the
      // default), so they must be dropped and reloaded from the restored one.
      localStorage.setItem('nodespace.activeDatabaseId', 'a');
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({
            databases: [db('a'), db('b', { isDefault: true })],
            defaultDatabaseId: 'b'
          });
        }
        return Promise.resolve(undefined);
      });

      await databaseStore.load();

      expect(clearAll).toHaveBeenCalledOnce();
      expect(forgetLocallyCreated).toHaveBeenCalledOnce();
      expect(loadCollections).toHaveBeenCalledOnce();
      expect(schemasInvalidateForDatabaseSwitch).toHaveBeenCalledOnce();
      expect(loadSchemas).toHaveBeenCalledOnce();
      expect(invalidateForDatabaseSwitch).toHaveBeenCalledOnce();
      expect(loadAiChats).toHaveBeenCalledOnce();
      expect(conflictsInvalidateForDatabaseSwitch).toHaveBeenCalledOnce();
      expect(loadConflicts).toHaveBeenCalledOnce();
      // Startup keeps the restored tabs; only a switch resets the workspace.
      expect(clearAllTabs).not.toHaveBeenCalled();
    });

    it('does not reload the stores when the restored database is the default', async () => {
      mockInvoke.mockResolvedValueOnce({
        databases: [db('a'), db('b', { isDefault: true })],
        defaultDatabaseId: 'b'
      });

      await databaseStore.load();

      expect(mockInvoke).toHaveBeenCalledWith('set_active_database', { id: 'b' });
      expect(clearAll).not.toHaveBeenCalled();
      expect(loadCollections).not.toHaveBeenCalled();
    });

    it('sends only the registry, launch-selection, routing, refusal-check and window-pin commands', async () => {
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({
            databases: [db('a'), db('b', { isDefault: true })],
            defaultDatabaseId: 'b'
          });
        }
        return Promise.resolve(undefined);
      });

      await databaseStore.load();

      expect(mockInvoke.mock.calls.map(([cmd]) => cmd)).toEqual([
        'list_databases',
        'initial_database_id',
        'set_active_database',
        'pin_window_database',
        // The refusal check reads the restored database once it commits.
        'list_conflicts'
      ]);
    });

    it('lets a tray switch that lands while load() is routing win', async () => {
      let releaseRouting: () => void = () => {};
      mockInvoke.mockImplementation((cmd: string, args?: { id?: string }) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({
            databases: [db('a'), db('b', { isDefault: true })],
            defaultDatabaseId: 'b'
          });
        }
        if (cmd === 'set_active_database' && args?.id === 'b') {
          return new Promise<void>((resolve) => (releaseRouting = resolve));
        }
        return Promise.resolve(undefined);
      });

      const loading = databaseStore.load();
      await vi.waitFor(() =>
        expect(mockInvoke).toHaveBeenCalledWith('set_active_database', { id: 'b' })
      );
      await databaseStore.switchTo('a');
      releaseRouting();
      await loading;

      expect(databaseStore.activeDatabaseId).toBe('a');
    });

    it('does not route when a tray switch already sent its own routing call', async () => {
      // The reverse ordering: switchTo('a') has sent set_active_database but
      // not committed yet when load() resolves. A later send from load() would
      // re-point routing to 'b' while the switch commits 'a'.
      let releaseSwitch: () => void = () => {};
      mockInvoke.mockImplementation((cmd: string, args?: { id?: string }) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({
            databases: [db('a'), db('b', { isDefault: true })],
            defaultDatabaseId: 'b'
          });
        }
        if (cmd === 'set_active_database' && args?.id === 'a') {
          return new Promise<void>((resolve) => (releaseSwitch = resolve));
        }
        return Promise.resolve(undefined);
      });
      databaseStore.databases = [db('a'), db('b', { isDefault: true })];

      const switching = databaseStore.switchTo('a');
      await vi.waitFor(() =>
        expect(mockInvoke).toHaveBeenCalledWith('set_active_database', { id: 'a' })
      );
      await databaseStore.load();
      releaseSwitch();
      await switching;

      expect(mockInvoke).not.toHaveBeenCalledWith('set_active_database', { id: 'b' });
      expect(databaseStore.activeDatabaseId).toBe('a');
    });

    it('records an error when the list fails', async () => {
      mockInvoke.mockRejectedValueOnce(new Error('boom'));
      await databaseStore.load();
      expect(databaseStore.error).toContain('boom');
    });

    it('surfaces the real CommandError message when list_databases rejects with a plain object, not "[object Object]"', async () => {
      // What Tauri hands back for a Rust `Result<_, CommandError>` `Err` — a
      // plain object, never an Error instance. The old bare `String(err)`
      // fallback here (no `instanceof Error` guard at all) stringified this
      // to the literal "[object Object]".
      mockInvoke.mockRejectedValueOnce({
        message: 'Database registry is locked by another process',
        code: 'REGISTRY_LOCKED'
      });
      await databaseStore.load();
      expect(databaseStore.error).toBe('Database registry is locked by another process');
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
      mockInvoke.mockResolvedValue(undefined); // set_active_database, pin_window_database

      await databaseStore.switchTo('b');

      expect(flushAllPendingSaves).toHaveBeenCalledOnce();
      expect(mockInvoke).toHaveBeenCalledWith('set_active_database', { id: 'b' });
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
      // And for plays: the previous database's list is dropped before the reload.
      expect(playsInvalidateForDatabaseSwitch).toHaveBeenCalledOnce();
      expect(loadPlays).toHaveBeenCalledOnce();
      expect(playsInvalidateForDatabaseSwitch.mock.invocationCallOrder[0]).toBeLessThan(
        loadPlays.mock.invocationCallOrder[0]
      );
      // Same discipline for schemas — an in-flight loadSchemas must
      // be invalidated before the reload, so its result can't land in the
      // store representing the newly-active database.
      expect(schemasInvalidateForDatabaseSwitch).toHaveBeenCalledOnce();
      expect(schemasInvalidateForDatabaseSwitch.mock.invocationCallOrder[0]).toBeLessThan(
        loadSchemas.mock.invocationCallOrder[0]
      );
      // The conflict journal is per-database: its records and any in-flight
      // load against the previous database are dropped before the reload.
      expect(conflictsInvalidateForDatabaseSwitch).toHaveBeenCalledOnce();
      expect(loadConflicts).toHaveBeenCalledOnce();
      expect(conflictsInvalidateForDatabaseSwitch.mock.invocationCallOrder[0]).toBeLessThan(
        loadConflicts.mock.invocationCallOrder[0]
      );
      // The per-collection member cache and the sub-panel selection
      // are both keyed on a collection id that is name-derived and can collide
      // across databases — both must be dropped so a same-named collection in
      // the new database can't render the previous database's cached members.
      expect(invalidateAllMembers).toHaveBeenCalledOnce();
      expect(collectionsStateReset).toHaveBeenCalledOnce();
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

    it('sends only the routing, refusal-check and window-pin commands', async () => {
      mockInvoke.mockResolvedValue(undefined);

      await databaseStore.switchTo('b');
      // Give any fire-and-forget command a chance to be sent.
      await Promise.resolve();
      await Promise.resolve();

      expect(mockInvoke.mock.calls).toEqual([
        ['set_active_database', { id: 'b' }],
        ['pin_window_database', { id: 'b' }],
        ['list_conflicts', { input: { status: null, kind: null, limit: 1 } }]
      ]);
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
        return Promise.resolve(undefined); // set_active_database, pin_window_database
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
        return Promise.resolve(undefined); // set_active_database, pin_window_database
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

  describe('extension lifecycle hooks', () => {
    // The database each activation saw as active when the hook ran, so a test
    // can tell "after the commit" from "before it".
    let activeAtHook: (string | null)[];
    let onDatabaseActivated: ReturnType<typeof vi.fn>;

    beforeEach(() => {
      activeAtHook = [];
      onDatabaseActivated = vi.fn(() => {
        activeAtHook.push(databaseStore.activeDatabaseId);
      });
      uiExtensionRegistry.register(
        createTestExtension(createTestLifecycleParts({ onDatabaseActivated }).parts)
      );
    });

    afterEach(() => {
      uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    });

    /** `list_databases` answers `a` and `b` with `b` as the default; everything else resolves. */
    function listing(): void {
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({
            databases: [db('a'), db('b', { isDefault: true })],
            defaultDatabaseId: 'b'
          });
        }
        return Promise.resolve(undefined);
      });
    }

    it('load() restoring a non-default database fires the hook once, after the caches are evicted', async () => {
      localStorage.setItem('nodespace.activeDatabaseId', 'a');
      listing();

      await databaseStore.load();

      expect(onDatabaseActivated).toHaveBeenCalledOnce();
      expect(onDatabaseActivated).toHaveBeenCalledWith('a');
      // The selection is committed before the hook, and the whole eviction has
      // run, and the reload after it: the schema-plugin resync is the last
      // step of `reloadDatabaseStores`.
      expect(activeAtHook).toEqual(['a']);
      expect(clearAll).toHaveBeenCalledOnce();
      expect(clearAll.mock.invocationCallOrder[0]).toBeLessThan(
        onDatabaseActivated.mock.invocationCallOrder[0]
      );
      expect(resyncSchemaPluginsForDatabaseSwitch.mock.invocationCallOrder[0]).toBeLessThan(
        onDatabaseActivated.mock.invocationCallOrder[0]
      );
    });

    it('load() restoring a refused database still fires the hook once, after the caches are evicted', async () => {
      localStorage.setItem('nodespace.activeDatabaseId', 'a');
      daemon([db('a', { status: 'requires_extension' }), db('b', { isDefault: true })], 'b', ['a']);

      await databaseStore.load();

      expect(databaseStore.activeRefusal).toEqual(REFUSAL);
      expect(onDatabaseActivated).toHaveBeenCalledOnce();
      expect(onDatabaseActivated).toHaveBeenCalledWith('a');
      expect(activeAtHook).toEqual(['a']);
      expect(clearAll.mock.invocationCallOrder[0]).toBeLessThan(
        onDatabaseActivated.mock.invocationCallOrder[0]
      );
    });

    it('switchTo a refused database still fires the hook once, after clearAll and before clearAllTabs', async () => {
      databaseStore.databases = [db('a'), db('b', { status: 'requires_extension' })];
      databaseStore.activeDatabaseId = 'a';
      daemon(databaseStore.databases, 'a', ['b']);

      await databaseStore.switchTo('b');

      expect(databaseStore.activeRefusal).toEqual(REFUSAL);
      expect(onDatabaseActivated).toHaveBeenCalledOnce();
      expect(onDatabaseActivated).toHaveBeenCalledWith('b');
      expect(activeAtHook).toEqual(['b']);
      const hookOrder = onDatabaseActivated.mock.invocationCallOrder[0];
      expect(clearAll.mock.invocationCallOrder[0]).toBeLessThan(hookOrder);
      expect(hookOrder).toBeLessThan(clearAllTabs.mock.invocationCallOrder[0]);
    });

    it('load() on the default database fires the hook once, with nothing evicted', async () => {
      listing();

      await databaseStore.load();

      expect(onDatabaseActivated).toHaveBeenCalledOnce();
      expect(onDatabaseActivated).toHaveBeenCalledWith('b');
      expect(activeAtHook).toEqual(['b']);
      expect(clearAll).not.toHaveBeenCalled();
    });

    it('switchTo fires the hook once, after clearAll and before clearAllTabs', async () => {
      databaseStore.databases = [db('a'), db('b')];
      databaseStore.activeDatabaseId = 'a';
      mockInvoke.mockResolvedValue(undefined);

      await databaseStore.switchTo('b');

      expect(onDatabaseActivated).toHaveBeenCalledOnce();
      expect(onDatabaseActivated).toHaveBeenCalledWith('b');
      expect(activeAtHook).toEqual(['b']);
      const hookOrder = onDatabaseActivated.mock.invocationCallOrder[0];
      expect(clearAll.mock.invocationCallOrder[0]).toBeLessThan(hookOrder);
      expect(hookOrder).toBeLessThan(clearAllTabs.mock.invocationCallOrder[0]);
    });

    it('fires only for the winning switch when a switch is superseded', async () => {
      databaseStore.databases = [db('a'), db('b'), db('c')];
      databaseStore.activeDatabaseId = 'a';
      mockInvoke.mockResolvedValue(undefined);

      await Promise.all([databaseStore.switchTo('b'), databaseStore.switchTo('c')]);

      expect(onDatabaseActivated.mock.calls).toEqual([['c']]);
    });

    it('does not fire for a load() a tray switch superseded, only for the switch', async () => {
      let releaseRouting: () => void = () => {};
      mockInvoke.mockImplementation((cmd: string, args?: { id?: string }) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({
            databases: [db('a'), db('b', { isDefault: true })],
            defaultDatabaseId: 'b'
          });
        }
        if (cmd === 'set_active_database' && args?.id === 'b') {
          return new Promise<void>((resolve) => (releaseRouting = resolve));
        }
        return Promise.resolve(undefined);
      });

      const loading = databaseStore.load();
      await vi.waitFor(() =>
        expect(mockInvoke).toHaveBeenCalledWith('set_active_database', { id: 'b' })
      );
      await databaseStore.switchTo('a');
      releaseRouting();
      await loading;

      expect(onDatabaseActivated.mock.calls).toEqual([['a']]);
    });

    it('does not fire when the switch never commits', async () => {
      databaseStore.databases = [db('a'), db('b')];
      databaseStore.activeDatabaseId = 'a';
      mockInvoke.mockRejectedValue(new Error('daemon unavailable'));

      await databaseStore.switchTo('b');

      expect(databaseStore.activeDatabaseId).toBe('a');
      expect(onDatabaseActivated).not.toHaveBeenCalled();
    });

    it('does not fire when switching to the database that is already active', async () => {
      databaseStore.databases = [db('a'), db('b')];
      databaseStore.activeDatabaseId = 'a';

      await databaseStore.switchTo('a');

      expect(onDatabaseActivated).not.toHaveBeenCalled();
    });

    it('does not fire in browser dev mode', async () => {
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;

      await databaseStore.load();

      expect(databaseStore.activeDatabaseId).not.toBeNull();
      expect(onDatabaseActivated).not.toHaveBeenCalled();
    });

    it('does not fire again for a later load() that keeps the selection', async () => {
      listing();

      await databaseStore.load();
      await databaseStore.load();

      expect(onDatabaseActivated).toHaveBeenCalledOnce();
    });

    it('a throwing hook does not abort load()', async () => {
      localStorage.setItem('nodespace.activeDatabaseId', 'a');
      listing();
      onDatabaseActivated.mockImplementation(() => {
        throw new Error('hook failed');
      });

      await databaseStore.load();

      expect(databaseStore.activeDatabaseId).toBe('a');
      expect(databaseStore.error).toBeNull();
      expect(databaseStore.loading).toBe(false);
    });

    it('load() does not fire the hook when evicting the previous caches throws, and records the error', async () => {
      localStorage.setItem('nodespace.activeDatabaseId', 'a');
      listing();
      clearAll.mockImplementationOnce(() => {
        throw new Error('evict failed');
      });

      await databaseStore.load();

      expect(onDatabaseActivated).not.toHaveBeenCalled();
      expect(databaseStore.error).toBe('evict failed');
    });

    it('switchTo does not fire the hook when evicting the previous caches throws, and records the error', async () => {
      databaseStore.databases = [db('a'), db('b')];
      databaseStore.activeDatabaseId = 'a';
      mockInvoke.mockResolvedValue(undefined);
      clearAll.mockImplementationOnce(() => {
        throw new Error('evict failed');
      });

      await databaseStore.switchTo('b');

      expect(onDatabaseActivated).not.toHaveBeenCalled();
      expect(databaseStore.error).toBe('evict failed');
      expect(clearAllTabs).not.toHaveBeenCalled();
    });

    it('a throwing hook does not abort the switch', async () => {
      databaseStore.databases = [db('a'), db('b')];
      databaseStore.activeDatabaseId = 'a';
      mockInvoke.mockResolvedValue(undefined);
      onDatabaseActivated.mockImplementation(() => {
        throw new Error('hook failed');
      });

      await databaseStore.switchTo('b');

      expect(onDatabaseActivated).toHaveBeenCalledOnce();
      expect(databaseStore.activeDatabaseId).toBe('b');
      expect(databaseStore.error).toBeNull();
      expect(clearAllTabs).toHaveBeenCalledOnce();
      expect(addTab).toHaveBeenCalledOnce();
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

  describe('refusal of a database that requires an extension', () => {
    /** Calls of the refusal read. */
    function refusalReads(): number {
      return mockInvoke.mock.calls.filter(([cmd]) => cmd === 'list_conflicts').length;
    }

    /** The commands sent so far, in order. */
    function commands(): string[] {
      return mockInvoke.mock.calls.map(([cmd]) => cmd as string);
    }

    it('reads a database the listing marks refused before the selection commits, and loads nothing from it', async () => {
      localStorage.setItem('nodespace.activeDatabaseId', 'a');
      daemon([db('a', { status: 'requires_extension' }), db('b', { isDefault: true })], 'b', ['a']);

      await databaseStore.load();

      expect(databaseStore.activeDatabaseId).toBe('a');
      expect(databaseStore.activeRefusal).toEqual(REFUSAL);
      expect(databaseStore.error).toBeNull();
      // Read before the commit, which pins the window, and only once.
      expect(commands().indexOf('list_conflicts')).toBeLessThan(
        commands().indexOf('pin_window_database')
      );
      expect(refusalReads()).toBe(1);
      // The boot-time loads the default answered are dropped, and nothing is
      // reloaded from the refused database.
      expect(clearAll).toHaveBeenCalledOnce();
      expect(loadCollections).not.toHaveBeenCalled();
      expect(loadSchemas).not.toHaveBeenCalled();
      expect(loadAiChats).not.toHaveBeenCalled();
      expect(loadConflicts).not.toHaveBeenCalled();
      expect(resyncSchemaPluginsForDatabaseSwitch).not.toHaveBeenCalled();
    });

    it('shows the refusal of a registry default the daemon refuses at startup', async () => {
      daemon([db('a'), db('b', { isDefault: true, status: 'requires_extension' })], 'b', ['b']);

      await databaseStore.load();

      expect(databaseStore.activeDatabaseId).toBe('b');
      expect(databaseStore.activeRefusal).toEqual(REFUSAL);
      expect(databaseStore.refusal?.databaseId).toBe('b');
    });

    it('records the refusal of a database the listing did not mark when its first read, after the commit, is refused', async () => {
      localStorage.setItem('nodespace.activeDatabaseId', 'a');
      daemon([db('a'), db('b', { isDefault: true })], 'b', ['a']);

      await databaseStore.load();

      expect(databaseStore.activeDatabaseId).toBe('a');
      await vi.waitFor(() => expect(databaseStore.activeRefusal).toEqual(REFUSAL));
      expect(commands().indexOf('pin_window_database')).toBeLessThan(
        commands().indexOf('list_conflicts')
      );
    });

    it('records no refusal when the read fails for another reason', async () => {
      localStorage.setItem('nodespace.activeDatabaseId', 'a');
      daemon([db('a', { status: 'requires_extension' }), db('b', { isDefault: true })], 'b', []);
      const answer = mockInvoke.getMockImplementation()!;
      mockInvoke.mockImplementation((cmd: string, args?: unknown) =>
        cmd === 'list_conflicts'
          ? Promise.reject({ message: 'busy', code: 'GRPC_ERROR' })
          : answer(cmd, args)
      );

      await databaseStore.load();
      // The read before the commit and the check after it both failed.
      await vi.waitFor(() => expect(refusalReads()).toBe(2));

      expect(databaseStore.activeDatabaseId).toBe('a');
      expect(databaseStore.activeRefusal).toBeNull();
      expect(loadCollections).toHaveBeenCalledOnce();
      expect(logWarn).not.toHaveBeenCalled();
    });

    it('ignores, with a warning, a REQUIRES_EXTENSION error whose payload is malformed', async () => {
      localStorage.setItem('nodespace.activeDatabaseId', 'a');
      daemon([db('a'), db('b', { isDefault: true })], 'b', []);
      const answer = mockInvoke.getMockImplementation()!;
      mockInvoke.mockImplementation((cmd: string, args?: unknown) =>
        cmd === 'list_conflicts'
          ? Promise.reject({
              ...refusedRead(),
              requiresExtension: { ...REFUSAL, downloadUrl: 'mailto:someone@example.test' }
            })
          : answer(cmd, args)
      );

      await databaseStore.load();

      await vi.waitFor(() => expect(logWarn).toHaveBeenCalledOnce());
      expect(databaseStore.activeRefusal).toBeNull();
    });

    it('switching to a database the listing marks refused records its refusal before committing, and loads nothing', async () => {
      databaseStore.databases = [db('a'), db('b', { status: 'requires_extension' })];
      databaseStore.activeDatabaseId = 'a';
      daemon(databaseStore.databases, 'a', ['b']);

      await databaseStore.switchTo('b');

      expect(databaseStore.activeDatabaseId).toBe('b');
      expect(databaseStore.activeRefusal).toEqual(REFUSAL);
      expect(databaseStore.error).toBeNull();
      expect(commands()).toEqual(['set_active_database', 'list_conflicts', 'pin_window_database']);
      expect(clearAll).toHaveBeenCalledOnce();
      expect(clearAllTabs).toHaveBeenCalledOnce();
      expect(loadCollections).not.toHaveBeenCalled();
      expect(loadSchemas).not.toHaveBeenCalled();
      expect(loadConflicts).not.toHaveBeenCalled();
      expect(resyncSchemaPluginsForDatabaseSwitch).not.toHaveBeenCalled();
    });

    it('does not hold a switch to a database the listing does not mark until it opens', async () => {
      databaseStore.databases = [db('a'), db('b')];
      databaseStore.activeDatabaseId = 'a';
      // The read waits on the database's open, which never finishes here.
      mockInvoke.mockImplementation((cmd: string) =>
        cmd === 'list_conflicts' ? new Promise(() => {}) : Promise.resolve(undefined)
      );

      const settled = await Promise.race([
        databaseStore.switchTo('b').then(() => true),
        new Promise<boolean>((resolve) => setTimeout(() => resolve(false), 200))
      ]);

      expect(settled).toBe(true);
      expect(databaseStore.activeDatabaseId).toBe('b');
      expect(loadCollections).toHaveBeenCalledOnce();
      expect(refusalReads()).toBe(1);
    });

    it('records the refusal of a database the listing did not mark after switching to it', async () => {
      databaseStore.databases = [db('a'), db('b')];
      databaseStore.activeDatabaseId = 'a';
      daemon(databaseStore.databases, 'a', ['b']);

      await databaseStore.switchTo('b');

      expect(databaseStore.activeDatabaseId).toBe('b');
      await vi.waitFor(() => expect(databaseStore.activeRefusal).toEqual(REFUSAL));
    });

    it('switching to a database that opens clears the refusal and reloads', async () => {
      databaseStore.databases = [db('a', { status: 'requires_extension' }), db('b')];
      databaseStore.activeDatabaseId = 'a';
      databaseStore.refusal = { databaseId: 'a', requiresExtension: REFUSAL };
      daemon(databaseStore.databases, 'a', ['a']);

      await databaseStore.switchTo('b');

      expect(databaseStore.activeDatabaseId).toBe('b');
      expect(databaseStore.activeRefusal).toBeNull();
      expect(databaseStore.refusal).toBeNull();
      expect(loadCollections).toHaveBeenCalledOnce();
    });

    it('creating a database and switching to it clears the refusal', async () => {
      const databases = [db('a', { isDefault: true, status: 'requires_extension' })];
      databaseStore.databases = [...databases];
      databaseStore.activeDatabaseId = 'a';
      databaseStore.refusal = { databaseId: 'a', requiresExtension: REFUSAL };
      daemon(databases, 'a', ['a']);
      const answer = mockInvoke.getMockImplementation()!;
      mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
        if (cmd !== 'create_database') return answer(cmd, args);
        const created = db('c', { name: 'Fresh' });
        databases.push(created);
        return Promise.resolve(created);
      });

      const entry = await databaseStore.create('Fresh');
      expect(entry?.id).toBe('c');
      await databaseStore.switchTo('c');

      expect(databaseStore.activeDatabaseId).toBe('c');
      expect(databaseStore.activeRefusal).toBeNull();
    });

    it('records the refusal a listing marks on the selected database when none was recorded', async () => {
      // Selected and routed earlier, when its refusal read failed for another reason.
      databaseStore.activeDatabaseId = 'a';
      daemon([db('a', { isDefault: true, status: 'requires_extension' })], 'a', ['a'], 'a');

      await databaseStore.load();
      expect(databaseStore.activeRefusal).toEqual(REFUSAL);
      expect(refusalReads()).toBe(1);

      // Recorded now, so a later listing reads nothing.
      await databaseStore.load();
      expect(refusalReads()).toBe(1);
    });

    it('does not re-read a listed refusal while a switch is in flight', async () => {
      databaseStore.databases = [db('a', { status: 'requires_extension' }), db('b')];
      databaseStore.activeDatabaseId = 'a';
      daemon(databaseStore.databases, 'a', ['a'], 'a');
      // Hold the switch in its flush, before it routes anywhere.
      let releaseFlush: () => void = () => {};
      flushAllPendingSaves.mockImplementationOnce(
        () => new Promise((resolve) => (releaseFlush = () => resolve(new Set<string>())))
      );

      const switching = databaseStore.switchTo('b');
      await databaseStore.load();
      expect(refusalReads()).toBe(0);

      releaseFlush();
      await switching;
      expect(databaseStore.activeDatabaseId).toBe('b');
      expect(databaseStore.refusal).toBeNull();
    });

    it('drops a refusal read for a database that is no longer selected', async () => {
      databaseStore.databases = [db('a', { status: 'requires_extension' }), db('b')];
      databaseStore.activeDatabaseId = 'a';
      let routed: string | null = 'a';
      let refuseA: (() => void) | null = null;
      mockInvoke.mockImplementation((cmd: string, args?: { id?: string }) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({ databases: databaseStore.databases, defaultDatabaseId: 'a' });
        }
        if (cmd === 'set_active_database') routed = args?.id ?? null;
        if (cmd === 'list_conflicts' && routed === 'a') {
          return new Promise((_resolve, reject) => {
            refuseA = () => reject(refusedRead());
          });
        }
        return Promise.resolve([]);
      });

      const loading = databaseStore.load();
      await vi.waitFor(() => expect(refuseA).not.toBeNull());
      await databaseStore.switchTo('b');
      refuseA!();
      await loading;

      expect(databaseStore.activeDatabaseId).toBe('b');
      expect(databaseStore.refusal).toBeNull();
    });

    it('does not read a selected database the listing does not mark refused', async () => {
      databaseStore.activeDatabaseId = 'a';
      daemon([db('a', { isDefault: true, status: 'open' })], 'a', [], 'a');

      await databaseStore.load();

      expect(refusalReads()).toBe(0);
      expect(databaseStore.activeRefusal).toBeNull();
    });

    it("lets a tray switch that lands during load()'s refusal read win, and discards that read", async () => {
      let refuseB: (() => void) | null = null;
      let routed: string | null = null;
      mockInvoke.mockImplementation((cmd: string, args?: { id?: string }) => {
        if (cmd === 'list_databases') {
          return Promise.resolve({
            databases: [db('a'), db('b', { isDefault: true, status: 'requires_extension' })],
            defaultDatabaseId: 'b'
          });
        }
        if (cmd === 'set_active_database') routed = args?.id ?? null;
        if (cmd === 'list_conflicts' && routed === 'b') {
          return new Promise((_resolve, reject) => {
            refuseB = () => reject(refusedRead());
          });
        }
        return Promise.resolve(undefined);
      });

      const loading = databaseStore.load();
      await vi.waitFor(() => expect(refuseB).not.toBeNull());
      await databaseStore.switchTo('a');
      refuseB!();
      await loading;

      expect(databaseStore.activeDatabaseId).toBe('a');
      expect(databaseStore.refusal).toBeNull();
    });

    it('discards the refusal read of a switch a newer switch superseded', async () => {
      databaseStore.databases = [db('a'), db('b', { status: 'requires_extension' }), db('c')];
      databaseStore.activeDatabaseId = 'a';
      let routed: string | null = null;
      let refuseB: (() => void) | null = null;
      mockInvoke.mockImplementation((cmd: string, args?: { id?: string }) => {
        if (cmd === 'set_active_database') routed = args?.id ?? null;
        if (cmd === 'list_conflicts' && routed === 'b') {
          return new Promise((_resolve, reject) => {
            refuseB = () => reject(refusedRead());
          });
        }
        return Promise.resolve([]);
      });

      const first = databaseStore.switchTo('b');
      // Let the first switch flush, route and send its refusal read.
      await vi.waitFor(() => expect(refuseB).not.toBeNull());
      const second = databaseStore.switchTo('c');
      await second;
      refuseB!();
      await first;

      expect(databaseStore.activeDatabaseId).toBe('c');
      expect(databaseStore.refusal).toBeNull();
      expect(databaseStore.activeRefusal).toBeNull();
    });

    it('shows a refusal only for the database it was read for', () => {
      databaseStore.refusal = { databaseId: 'a', requiresExtension: REFUSAL };

      databaseStore.activeDatabaseId = 'b';
      expect(databaseStore.activeRefusal).toBeNull();

      databaseStore.activeDatabaseId = 'a';
      expect(databaseStore.activeRefusal).toEqual(REFUSAL);
    });
  });

  describe('isActiveDatabaseEvent', () => {
    it('passes events with no database id (an impl not opened through the registry)', () => {
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
