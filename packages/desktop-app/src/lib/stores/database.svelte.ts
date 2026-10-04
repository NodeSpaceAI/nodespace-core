import { invoke } from '@tauri-apps/api/core';
import { createLogger } from '$lib/utils/logger';
import { onDaemonReconnect } from '$lib/services/daemon-status';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { collectionsData, collectionsState } from '$lib/stores/collections.svelte';
import { schemasData } from '$lib/stores/schemas.svelte';
import { aiChatsData } from '$lib/stores/ai-chats.svelte';
import { playsData } from '$lib/stores/plays.svelte';
import { savedQueriesData } from '$lib/stores/saved-queries.svelte';
import { conflictsStore } from '$lib/stores/conflicts.svelte';
import { resyncSchemaPluginsForDatabaseSwitch } from '$lib/plugins/schema-plugin-loader';
import { notifyDatabaseActivated } from '$lib/plugins/extension-lifecycle';
import {
  clearAllTabs,
  addTab,
  DAILY_JOURNAL_TAB_ID,
  DEFAULT_PANE_ID
} from '$lib/stores/navigation.svelte';
import { formatDateISO } from '$lib/utils/date-formatting';
import { toError } from '$lib/types/errors';
import { isRequiresExtension, type RequiresExtensionPayload } from '$lib/types/requires-extension';

const log = createLogger('DatabaseStore');

/**
 * A registered local database as surfaced by the `list_databases` command
 * (ADR-053: "One Daemon, Multiple Local Databases"). Mirrors the Rust
 * `DatabaseEntry` (camelCase).
 */
export interface DatabaseInfo {
  id: string;
  name: string;
  path: string;
  isDefault: boolean;
  /** "closed" | "open" | "missing" | "requires_extension" | "unknown". */
  status: string;
  createdAt: string;
  lastOpenedAt: string | null;
  /** Opaque per-database keys the registry stores for extensions. Core never
   * reads them; an extension that owns a key reads it from here. */
  extensions: Record<string, string>;
}

interface DatabaseListing {
  databases: DatabaseInfo[];
  defaultDatabaseId: string;
}

/**
 * A database the daemon refuses to open because it requires an extension this
 * build does not support (ADR-083 §2), with the refusal the app shows for it.
 */
export interface DatabaseRefusal {
  databaseId: string;
  requiresExtension: RequiresExtensionPayload;
}

/**
 * Whether the Tauri IPC bridge is present. Absent under `dev:browser`, where the
 * app runs in a plain browser against the dev-proxy — which forwards NodeService
 * but has no DatabaseService, so calling the `list_databases` invoke throws
 * `Cannot read properties of undefined (reading 'invoke')`. Same probe the model,
 * agent, and external-link surfaces use.
 */
function isTauriBridgePresent(): boolean {
  return (
    typeof window !== 'undefined' && ('__TAURI__' in window || '__TAURI_INTERNALS__' in window)
  );
}

/**
 * The single implicit database presented in browser dev mode: the dev-proxy is
 * bound to one daemon/database and exposes no registry, so the switcher shows one
 * concrete, non-switchable entry instead of erroring at boot.
 */
const IMPLICIT_BROWSER_DATABASE: DatabaseInfo = {
  id: 'default',
  name: 'Default',
  path: '',
  isDefault: true,
  status: 'open',
  createdAt: '',
  lastOpenedAt: null,
  extensions: {}
};

/**
 * The active-database selection is desktop-local (not a daemon concept), so it
 * is persisted in the webview's localStorage. This survives a reload (Cmd+R) and
 * an app restart, so the user stays on the database they switched to instead of
 * snapping back to the daemon's registry default.
 */
const ACTIVE_DB_STORAGE_KEY = 'nodespace.activeDatabaseId';

function rememberActiveDatabaseId(id: string): void {
  try {
    localStorage.setItem(ACTIVE_DB_STORAGE_KEY, id);
  } catch {
    // localStorage unavailable (e.g. some sandboxed contexts) — persistence is
    // best-effort; the selection simply won't survive a reload.
  }
}

function readRememberedActiveDatabaseId(): string | null {
  try {
    return localStorage.getItem(ACTIVE_DB_STORAGE_KEY);
  } catch {
    return null;
  }
}

/**
 * Manages the daemon's registry of local databases and the desktop-local
 * "which database am I viewing" selection.
 *
 * Switching flushes the frontend's per-database caches and resets the
 * workspace so open viewers remount against the newly-active database; the
 * daemon-side re-subscribe (driven by the `set_active_database` command) makes
 * live node events stream from the new database.
 */
class DatabaseStore {
  databases = $state<DatabaseInfo[]>([]);
  /** The database the app is currently viewing. `null` until `load()` runs. */
  activeDatabaseId = $state<string | null>(null);
  /** The daemon-wide default (the header-less routing target). */
  defaultDatabaseId = $state<string | null>(null);
  loading = $state(false);
  error = $state<string | null>(null);
  /**
   * The refusal of the selected database, while the daemon refuses to open it.
   * Read before a selection commits when the listing marks the database
   * `requires_extension`, otherwise right after it commits
   * (`checkSelectedDatabase`), and again when a later listing marks it with
   * no refusal recorded. Replaced by the next committed selection. Read it
   * through `activeRefusal`.
   */
  refusal = $state<DatabaseRefusal | null>(null);

  /**
   * Monotonic token bumped on every `switchTo`. A switch awaits (flush, then the
   * `set_active_database` command), so a second switch fired before the first
   * finishes would otherwise race — leaving `activeDatabaseId` and the routed
   * client transiently pointed at different databases. Each continuation checks
   * its captured token and bails if a newer switch superseded it, so the latest
   * switch always wins cleanly.
   */
  private switchSeq = 0;

  /**
   * Number of `switchTo` calls between their `switchSeq` bump and settling.
   * `load()` skips its startup routing while one is in flight: that switch
   * may already have sent `set_active_database`, and a later send from
   * `load()` would re-point routing away from the database the switch commits.
   */
  private switchesInFlight = 0;

  /** The database currently being viewed, or `null` if none is selected. */
  get activeDatabase(): DatabaseInfo | null {
    return this.databases.find((db) => db.id === this.activeDatabaseId) ?? null;
  }

  /**
   * The refusal the app shows instead of the workspace, or `null` while the
   * active database opens normally.
   */
  get activeRefusal(): RequiresExtensionPayload | null {
    const refusal = this.refusal;
    return refusal !== null && refusal.databaseId === this.activeDatabaseId
      ? refusal.requiresExtension
      : null;
  }

  /** Whether the latest listing marks `id` as a database the daemon refuses to open. */
  private isListedRefused(id: string): boolean {
    return this.databases.some((db) => db.id === id && db.status === 'requires_extension');
  }

  /**
   * Load the registry. Initializes `activeDatabaseId` to the daemon default the
   * first time (later loads preserve the current selection so a background
   * refresh never yanks the user back to the default).
   */
  async load(): Promise<void> {
    this.loading = true;
    this.error = null;

    // Browser dev mode (dev:browser): no Tauri bridge, so there is no database
    // registry to query. Present a single implicit database rather than logging a
    // boot error and rendering the switcher's failure fallback. The Tauri app,
    // where the bridge is present, is unaffected.
    if (!isTauriBridgePresent()) {
      this.databases = [IMPLICIT_BROWSER_DATABASE];
      this.defaultDatabaseId = IMPLICIT_BROWSER_DATABASE.id;
      if (this.activeDatabaseId === null) {
        this.activeDatabaseId = IMPLICIT_BROWSER_DATABASE.id;
      }
      this.loading = false;
      return;
    }

    try {
      const listing = await invoke<DatabaseListing>('list_databases');
      this.databases = listing.databases;
      this.defaultDatabaseId = listing.defaultDatabaseId || null;

      if (this.activeDatabaseId === null) {
        // Restore the last-active database across webview reloads / restarts.
        // Fall back to the daemon default, then the first registered database, so
        // the switcher always shows a concrete selection. Ignore ids that are no
        // longer registered (e.g. the database was deleted).
        //
        // A database named for *this launch* wins over the remembered one: it is
        // set only when the user picked that database from the tray, which is a
        // more specific instruction than "whatever you had open last time".
        const seqAtStart = this.switchSeq;
        const registered = (id: string | null): string | null =>
          id !== null && this.databases.some((db) => db.id === id) ? id : null;

        const requested = registered(await this.readInitialDatabaseId());
        const remembered = registered(readRememberedActiveDatabaseId());
        const resolved =
          requested ?? remembered ?? this.defaultDatabaseId ?? this.databases[0]?.id ?? null;

        // Re-check after the awaits above. A second `load()` runs on every
        // launch — the daemon-reconnect listener fires one — and both can pass
        // the outer check before either assigns. Assigning unconditionally lets
        // whichever finishes last overwrite a selection already made. A tray
        // pick (`switchTo`) that started since this load began, or is still in
        // flight, owns the selection and its routing, so defer to it too.
        const superseded = (): boolean =>
          this.activeDatabaseId !== null || this.switchSeq !== seqAtStart;
        if (superseded() || this.switchesInFlight > 0) return;

        // Point the routed gRPC clients (and the node-event watcher) at the
        // restored database before committing the selection, as `switchTo`
        // does. Otherwise the switcher and window pin show the restored
        // database while node/import/agent requests still route to the daemon
        // default. A switch that starts while this awaits sends its own
        // `set_active_database` after this one, so it wins; the re-check
        // below leaves its selection alone.
        await invoke('set_active_database', { id: resolved });
        if (superseded()) return;

        // A database the listing marks refused is read before the selection
        // commits, for the refusal's text, so the workspace never mounts
        // against it. Any other database commits at once and is checked
        // after, so its open never delays the selection.
        const refusal =
          resolved !== null && this.isListedRefused(resolved) ? await this.readRefusal() : null;
        if (superseded()) return;

        this.activeDatabaseId = resolved;
        this.refusal =
          resolved !== null && refusal !== null
            ? { databaseId: resolved, requiresExtension: refusal }
            : null;
        if (resolved !== null) this.pinWindowDatabase(resolved);

        if (resolved !== null && resolved !== this.defaultDatabaseId) {
          // The sidebar's boot-time loads went out before routing was set, so
          // the daemon default answered them. Drop them, and reload from the
          // restored database unless the daemon refuses it. Workspace panes
          // mount only once a database is selected, so restored tabs never
          // read before this point.
          this.evictDatabaseCaches();
          if (refusal === null) this.reloadDatabaseStores();
        }

        // The first resolution is a committed activation: tell extensions once
        // the restore above has finished evicting.
        if (resolved !== null) notifyDatabaseActivated(resolved);
        if (resolved !== null && refusal === null) void this.checkSelectedDatabase(resolved);
      } else if (
        this.isListedRefused(this.activeDatabaseId) &&
        this.activeRefusal === null &&
        this.switchesInFlight === 0
      ) {
        // The listing marks the selected database refused, but no refusal was
        // recorded (its read failed for another reason). With no switch in
        // flight, routing still points at it.
        await this.checkSelectedDatabase(this.activeDatabaseId);
      }
    } catch (err) {
      this.error = toError(err).message;
      log.error('Failed to load databases', err);
    } finally {
      this.loading = false;
    }
  }

  /**
   * Read the routed database once, and return its refusal: the payload of the
   * REQUIRES_EXTENSION error the daemon returns while it refuses to open a
   * database that requires an extension this build does not support
   * (ADR-083 §2). `null` when the read succeeds, or fails for any other reason.
   *
   * A routed command's error is the only place the refusal's message and
   * download link reach the frontend: the listing marks a refused database but
   * carries neither. Every routed command returns the same refusal, so this
   * sends a cheap one, a conflict-journal read of at most one record. Callers
   * send it while routing points at the database it should read.
   */
  private async readRefusal(): Promise<RequiresExtensionPayload | null> {
    try {
      await invoke('list_conflicts', { input: { status: null, kind: null, limit: 1 } });
      return null;
    } catch (err) {
      if (isRequiresExtension(err)) return err.requiresExtension;
      if (
        typeof err === 'object' &&
        err !== null &&
        'code' in err &&
        err.code === 'REQUIRES_EXTENSION'
      ) {
        // The daemon refuses the database, but with a payload this build
        // cannot render, so the workspace shows and every read in it fails.
        log.warn('Ignoring a REQUIRES_EXTENSION error with a malformed payload', err);
      }
      return null;
    }
  }

  /**
   * Read the selected database `id`, which routing points at, and record its
   * refusal if the daemon refuses it. Sent right after a selection commits,
   * for a database the listing did not mark (a listing older than the
   * database), so a switch to a database that opens never waits for its
   * open; and when a later listing marks the selected database with no
   * refusal recorded. A result for a database no longer selected is dropped.
   * Nothing is evicted: the refusal view replaces the workspace, and leaving
   * it goes through `switchTo`, which evicts.
   */
  private async checkSelectedDatabase(id: string): Promise<void> {
    const refusal = await this.readRefusal();
    if (refusal === null || this.activeDatabaseId !== id) return;
    this.refusal = { databaseId: id, requiresExtension: refusal };
  }

  /**
   * Declare to the backend which database this window is now showing. The
   * backend uses this to route
   * database-scoped events (`node:*`, `relationship:*`) to the correct
   * window instead of broadcasting to every open one, and to restore this
   * database's last saved window size/position. Best-effort and
   * fire-and-forget — a failure here only means this window's live events
   * fall back to focused-window routing (see `window_routing::emit_routed`)
   * instead of being pinned to this specific database, which is a strict
   * improvement over never pinning at all, never a regression.
   */
  private pinWindowDatabase(id: string): void {
    if (!isTauriBridgePresent()) return;
    // Wrapped in Promise.resolve(...) (adopts a real promise unchanged) and a
    // try/catch, rather than a bare `invoke(...).catch(...)`: this call must
    // never destabilize `load()`/`switchTo()`, including against a test
    // double that returns something other than a promise.
    try {
      Promise.resolve(invoke('pin_window_database', { id })).catch((err: unknown) => {
        log.debug('Failed to pin window to database', { id, error: err });
      });
    } catch (err) {
      log.debug('Failed to pin window to database', { id, error: err });
    }
  }

  /**
   * Refresh just the registry list (id/name/status/etc for every registered
   * database), without `load()`'s `activeDatabaseId`/`loading`/`error`
   * side effects. Used by `switchTo`'s unregistered-id guard as a
   * last-resort re-check before rejecting an id: the frontend's `databases`
   * list is loaded once at boot (and only otherwise refreshed by an
   * explicit registry mutation here in this store), so it can be stale
   * relative to the daemon's registry — e.g. a database registered via the
   * CLI after boot, whose tray submenu entry (populated from the daemon,
   * which live-refreshes it) is legitimately switchable even though this
   * store has never heard of it yet.
   */
  private async refreshDatabaseList(): Promise<void> {
    if (!isTauriBridgePresent()) return;
    try {
      const listing = await invoke<DatabaseListing>('list_databases');
      this.databases = listing.databases;
      this.defaultDatabaseId = listing.defaultDatabaseId || null;
    } catch (err) {
      log.debug('Failed to refresh database registry', err);
    }
  }

  /**
   * Pull-based fallback for a tray database pick that arrived before this
   * window's `tray:select-database` listener finished registering. Tauri
   * does not buffer or replay an event emitted while it has zero current
   * listeners, and the listener only exists once `app-shell.svelte`'s
   * `listen()` IPC round-trip has resolved — a relaunch in that narrow
   * window (webview still booting) would otherwise focus the window but
   * silently drop the switch, since the Rust side's `emit_to` call reaches
   * nobody. The backend stashes the id for exactly this gap
   * (`take_pending_tray_database_selection`); call this once, right after
   * the `tray:select-database` listener's `listen()` promise resolves, so a
   * pick that raced boot still lands. A no-op when nothing was stashed —
   * `switchTo` is also idempotent against a value the live event happened
   * to deliver in addition to (or instead of) this pull.
   */
  async applyPendingTraySelection(): Promise<void> {
    if (!isTauriBridgePresent()) return;
    try {
      const pendingId = await invoke<string | null>('take_pending_tray_database_selection');
      if (pendingId) {
        await this.switchTo(pendingId);
      }
    } catch (err) {
      log.debug('Failed to read pending tray database selection', err);
    }
  }

  /**
   * The database this launch was told to open, if any.
   *
   * Set by the daemon tray when the user picks a database from its submenu.
   * A failure here is not worth surfacing — it only means we fall through to
   * the remembered/default selection, which is the normal path anyway.
   */
  private async readInitialDatabaseId(): Promise<string | null> {
    try {
      return (await invoke<string | null>('initial_database_id')) ?? null;
    } catch (err) {
      log.debug('No launch-time database selection available', err);
      return null;
    }
  }

  /**
   * Switch the active database. Flushes pending writes to the current database
   * first (so they never land in the target), then re-points the routed
   * clients, clears the frontend caches, and resets the workspace so viewers
   * reload from the newly-active database.
   *
   * Rejects an `id` that is not in `this.databases` rather than committing to
   * it — every UI call site already only ever passes a known-registered id
   * (a button rendered from `databases`, or a fallback drawn from the same
   * list), so this only ever actually fires for the one caller that doesn't
   * control its input: the tray's `tray:select-database` relaunch event.
   * Without this, a database removed (or never registered — a forged relaunch
   * argv, however unlikely) between the tray click and the event arriving
   * would clear every cache and reset the workspace before the daemon's
   * per-request `NOT_FOUND` ever surfaced, leaving the user on a blank
   * workspace pointed at nothing with no rollback. Mirrors the `registered()`
   * guard `load()` already applies to this exact same untrusted input.
   *
   * An id missing from `this.databases` is re-checked against a fresh
   * registry pull before being rejected outright: that list is loaded once
   * at boot and otherwise only refreshed by a mutation made through this
   * store, so it goes stale the moment a database is registered by another
   * path (the shipped CLI, another window) — while the daemon tray's
   * submenu live-refreshes from the registry directly and legitimately
   * offers ids this store hasn't heard of yet. The same gap catches a tray
   * pick that arrives after this listener registers but before the very
   * first `load()` resolves, when `databases` is still `[]`.
   */
  async switchTo(id: string): Promise<void> {
    if (id === this.activeDatabaseId) return;
    if (!this.databases.some((db) => db.id === id)) {
      await this.refreshDatabaseList();
      if (!this.databases.some((db) => db.id === id)) {
        log.warn('switchTo called with an unregistered database id; ignoring', { id });
        return;
      }
    }
    this.error = null;
    const seq = ++this.switchSeq;
    this.switchesInFlight++;
    try {
      // Land any in-flight debounced saves in the database they were made
      // against before the routed clients re-point.
      await sharedNodeStore.flushAllPendingSaves();
      // A newer switch superseded this one while we flushed — let it win.
      if (seq !== this.switchSeq) return;

      await invoke('set_active_database', { id });
      if (seq !== this.switchSeq) return;
      // As in `load()`: read a database the listing marks refused before
      // committing, and check any other after.
      const refusal = this.isListedRefused(id) ? await this.readRefusal() : null;
      if (seq !== this.switchSeq) return;
      this.activeDatabaseId = id;
      this.refusal = refusal === null ? null : { databaseId: id, requiresExtension: refusal };
      // Remember the selection so a webview reload / app restart restores it
      // instead of snapping back to the daemon's registry default.
      rememberActiveDatabaseId(id);
      this.pinWindowDatabase(id);

      this.evictDatabaseCaches();
      // A refused database has nothing to load: every read of it is refused,
      // and the refusal view replaces the workspace that would show it.
      if (refusal === null) this.reloadDatabaseStores();

      // Extensions clear their own per-database caches here: the previous
      // database's data is evicted, and the workspace reset below has not run.
      notifyDatabaseActivated(id);

      // Reset the workspace: open tabs referenced the previous database's
      // nodes, so drop them and land on the new database's daily journal
      // (a date page exists in every database). Remounting the viewer reloads
      // its content from the now-active database.
      clearAllTabs();
      addTab(
        {
          id: DAILY_JOURNAL_TAB_ID,
          title: 'Daily Journal',
          type: 'node',
          content: { nodeId: formatDateISO(new Date()), nodeType: 'date' },
          closeable: true,
          paneId: DEFAULT_PANE_ID
        },
        true
      );
      if (refusal === null) void this.checkSelectedDatabase(id);
    } catch (err) {
      this.error = toError(err).message;
      log.error('Failed to switch database', { id, error: err });
    } finally {
      this.switchesInFlight--;
    }
  }

  /**
   * Evict the node caches and the per-database selection state, and
   * invalidate every database-scoped load still in flight, so nothing from
   * the previous database lands in a store afterwards. The sidebar stores'
   * lists stay until `reloadDatabaseStores` replaces them, except the plays
   * list, which reads the evicted nodes and so empties at once. Used by
   * `switchTo`, and by `load()` when the restored database is not the daemon
   * default (reads issued before routing was set were answered by the
   * default). A database the daemon refuses gets no reload: the refusal view
   * replaces everything that shows those lists, and leaving it goes through
   * `switchTo`, which reloads.
   */
  private evictDatabaseCaches(): void {
    // Evict the previous database's cached data. `clearAll()` also bumps the
    // store's database epoch, which closes the in-flight-read window: a read
    // (e.g. loadChildren/getNode) dispatched against the previous database
    // *before* this switch whose response resolves after this clear captured
    // the old epoch and is dropped instead of writing the previous
    // database's rows into the now-active store (see
    // `sharedNodeStore.currentEpoch()`).
    sharedNodeStore.clearAll();
    structureTree.clear();

    // The locally-created exemptions belong to the database being left —
    // collection ids are derived from the name, so keeping them would
    // wrongly un-hide a same-named empty collection in the new one.
    collectionsData.forgetLocallyCreated();
    // The per-collection member-node cache is keyed by collection id, which
    // is name-derived and can collide across databases — without this, a
    // same-named collection in the new database would render the *previous*
    // database's cached member nodes as its own contents.
    collectionsData.invalidateAllMembers();
    // Drop the sub-panel selection too: `collectionsState.selectedCollectionId`
    // / `subPanelOpen` are not evicted by anything above, so a panel left open
    // on a DB-A collection would otherwise keep rendering (now-stale) DB-A
    // members against DB-B, including for a DB-B collection that happens to
    // share the id.
    collectionsState.reset();
    // As with collectionsData.forgetLocallyCreated() above: invalidate any
    // in-flight loadSchemas, so its result can't land in a store that now
    // represents a different database.
    schemasData.invalidateForDatabaseSwitch();
    // Saved queries are per-database too.
    savedQueriesData.invalidateForDatabaseSwitch();
    // So are plays.
    playsData.invalidateForDatabaseSwitch();
    // As with collectionsData.forgetLocallyCreated() above: invalidate any
    // in-flight "+ New chat" create, so its result can't land in a store that
    // now represents a different database.
    aiChatsData.invalidateForDatabaseSwitch();
    // The conflict journal is per-database too (ADR-068): drop the previous
    // database's records and any load still in flight against it.
    conflictsStore.invalidateForDatabaseSwitch();
  }

  /**
   * Reload the database-scoped stores from the currently-routed database,
   * after `evictDatabaseCaches`.
   */
  private reloadDatabaseStores(): void {
    collectionsData.loadCollections();
    schemasData.loadSchemas();
    savedQueriesData.loadSavedQueries();
    playsData.loadPlays();
    aiChatsData.loadAiChats();
    void conflictsStore.load();
    // Re-sync the schema plugin registry (hasTitleTemplate/titleTemplate)
    // against the newly-active database's schemas — otherwise a custom type
    // keeps resolving titles via the previous database's template (or, for a
    // type unique to the new database, via no template at all) until the
    // next app restart.
    void resyncSchemaPluginsForDatabaseSwitch();
  }

  /**
   * Create a brand-new database and register it, then refresh the list. When
   * `path` is omitted the daemon places the file under its managed directory.
   * Returns the new entry so callers can switch to it.
   */
  async create(name: string, path?: string): Promise<DatabaseInfo | null> {
    return this.mutate(() => invoke<DatabaseInfo>('create_database', { name, path: path ?? null }));
  }

  /** Register an existing database file already on disk, then refresh the list. */
  async register(path: string): Promise<DatabaseInfo | null> {
    return this.mutate(() => invoke<DatabaseInfo>('register_database', { path }));
  }

  /** Rename a registered database's label, then refresh the list. */
  async rename(id: string, name: string): Promise<DatabaseInfo | null> {
    return this.mutate(() => invoke<DatabaseInfo>('rename_database', { id, name }));
  }

  /** Set the daemon-wide default database, then refresh the list. */
  async setDefault(id: string): Promise<DatabaseInfo | null> {
    return this.mutate(() => invoke<DatabaseInfo>('set_default_database', { id }));
  }

  /**
   * Unregister a database (never deletes the file), then refresh the list. If
   * the removed database was the active one, fall back to the daemon default.
   */
  async remove(id: string): Promise<void> {
    this.error = null;
    try {
      await invoke<string>('remove_database', { id });
      await this.load();
      if (this.activeDatabaseId === id) {
        const fallback = this.defaultDatabaseId ?? this.databases[0]?.id ?? null;
        if (fallback) {
          await this.switchTo(fallback);
        } else {
          this.activeDatabaseId = null;
        }
      }
    } catch (err) {
      this.error = toError(err).message;
      log.error('Failed to remove database', { id, error: err });
    }
  }

  /** Run a registry mutation, refresh the list, and return the mutation result. */
  private async mutate(op: () => Promise<DatabaseInfo>): Promise<DatabaseInfo | null> {
    this.error = null;
    try {
      const result = await op();
      await this.load();
      return result;
    } catch (err) {
      this.error = toError(err).message;
      log.error('Database registry operation failed', err);
      return null;
    }
  }
}

export const databaseStore = new DatabaseStore();

// Retry the initial registry load once the daemon becomes reachable, mirroring
// the collections/schemas stores: a `load()` that ran while the daemon was
// still starting fails and leaves `activeDatabaseId` null until a manual
// reload. Guarded on the not-yet-loaded state
// so a background reconnect never re-runs against an established selection.
onDaemonReconnect(() => {
  if (databaseStore.activeDatabaseId === null) {
    void databaseStore.load();
  }
});

/**
 * True when a `database_id` event envelope belongs to the active database.
 * An empty id (an event from an impl not opened through the registry) or an
 * as-yet-unloaded selection always applies — the guard only drops events
 * explicitly tagged for a different database, closing the race where a watch
 * stream open across a switch delivers the previous database's events.
 */
export function isActiveDatabaseEvent(databaseId: string | undefined): boolean {
  if (!databaseId) return true;
  if (databaseStore.activeDatabaseId === null) return true;
  return databaseId === databaseStore.activeDatabaseId;
}
