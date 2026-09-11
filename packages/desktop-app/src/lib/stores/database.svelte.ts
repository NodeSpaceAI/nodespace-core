import { invoke } from '@tauri-apps/api/core';
import { createLogger } from '$lib/utils/logger';
import { backendAdapter } from '$lib/services/backend-adapter';
import { onDaemonReconnect } from '$lib/services/daemon-status';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { collectionsData, collectionsState } from '$lib/stores/collections.svelte';
import { schemasData } from '$lib/stores/schemas.svelte';
import { aiChatsData } from '$lib/stores/ai-chats.svelte';
import { membership } from '$lib/stores/membership.svelte';
import { resyncSchemaPluginsForDatabaseSwitch } from '$lib/plugins/schema-plugin-loader';
import {
  clearAllTabs,
  addTab,
  DAILY_JOURNAL_TAB_ID,
  DEFAULT_PANE_ID
} from '$lib/stores/navigation.svelte';
import { formatDateISO } from '$lib/utils/date-formatting';
import { DATABASE_SETTINGS_NODE_ID } from '$lib/plugins/ui-extensions';

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
  /** "closed" | "open" | "missing" | "unknown". */
  status: string;
  createdAt: string;
  lastOpenedAt: string | null;
  /** Cloud tenant schema this database syncs to (ADR-053); null/empty when the
   * database is local-only (not bound to any tenant). */
  boundTenantSchema: string | null;
  /** The bound tenant's default (landing) collection id (ADR-053), a
   * per-install root. Used as the tree root to hide from the sidebar so
   * top-level collections render as peers rather than nested under the root.
   * null on the public/legacy tenant, where the collections store falls back to
   * the well-known root id. */
  boundTenantCollection: string | null;
}

interface DatabaseListing {
  databases: DatabaseInfo[];
  defaultDatabaseId: string;
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
  boundTenantSchema: null,
  boundTenantCollection: null
};

/**
 * The active-database selection is desktop-local (not a daemon concept), so it
 * is persisted in the webview's localStorage. This survives a reload (Cmd+R) and
 * an app restart, so the user stays on the database they switched to instead of
 * snapping back to the daemon's registry default.
 */
const ACTIVE_DB_STORAGE_KEY = 'nodespace.activeDatabaseId';

/**
 * Owner key `DATABASE_SETTINGS_NODE_ID` is pinned reachable under (see
 * SharedNodeStore.pinNodes) — never unpinned, since the singleton backs
 * always-mounted Pro-sync chrome (pro-sync-pill.svelte, app-shell.svelte,
 * membership.svelte.ts, collaboration-locked.svelte) that has nothing to do
 * with which tab/pane is open. It has no structureTree relationship to any
 * open tab and isn't itself a tab root, so without an explicit pin it's
 * unreachable — and thus evictable — the instant it's cached.
 */
const DATABASE_SETTINGS_PIN_OWNER = 'database-settings-node';

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
   * Monotonic token bumped on every `switchTo`. A switch awaits (flush, then the
   * `set_active_database` command), so a second switch fired before the first
   * finishes would otherwise race — leaving `activeDatabaseId` and the routed
   * client transiently pointed at different databases. Each continuation checks
   * its captured token and bails if a newer switch superseded it, so the latest
   * switch always wins cleanly.
   */
  private switchSeq = 0;

  /** The database currently being viewed, or `null` if none is selected. */
  get activeDatabase(): DatabaseInfo | null {
    return this.databases.find((db) => db.id === this.activeDatabaseId) ?? null;
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
        this.refreshDatabaseSettings();
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
        const registered = (id: string | null): string | null =>
          id !== null && this.databases.some((db) => db.id === id) ? id : null;

        const requested = registered(await this.readInitialDatabaseId());
        const remembered = registered(readRememberedActiveDatabaseId());
        const resolved =
          requested ?? remembered ?? this.defaultDatabaseId ?? this.databases[0]?.id ?? null;

        // Re-check after the awaits above. A second `load()` runs on every
        // launch — the daemon-reconnect listener fires one — and both can pass
        // the outer check before either assigns. Assigning unconditionally lets
        // whichever finishes last overwrite a selection already made.
        if (this.activeDatabaseId === null) {
          this.activeDatabaseId = resolved;
          // Hydrate the active database's DatabaseSettingsNode so the Pro-sync
          // variant machine can read sync_enabled/auth_status.
          this.refreshDatabaseSettings();
          if (resolved !== null) {
            this.pinWindowDatabase(resolved);
            this.activateProSync(resolved);
          }
        }
      }
    } catch (err) {
      this.error = String(err);
      log.error('Failed to load databases', err);
    } finally {
      this.loading = false;
    }
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
   * Serializes `activateProSync` calls so a rapid A -> B -> A switch sequence
   * can never have its underlying `pro_activate_database` invokes resolve out
   * of send order. Each call is chained to start only once the previous one
   * has settled, guaranteeing arrival order on the Rust side matches send
   * order — without this, the daemon (and the Rust-tracked
   * `active_database_id` every subsequent unscoped `sync:status` event is
   * attributed against) could end up pointed at a stale target if an earlier
   * call's RPC happens to complete after a later one's.
   */
  private proActivateChain: Promise<void> = Promise.resolve();

  /**
   * Re-target the Pro cloud-sync session to `id` (ADR-053 single-active sync).
   * Called both from `load()`'s first resolution (so a fresh launch's default
   * selection is the daemon's sync target from the start, not just after the
   * first manual switch) and from `switchTo()`. No-ops in community mode (the
   * Tauri command returns early without a `ProClient`). Best-effort and
   * fire-and-forget from the CALLER's perspective (same contract as
   * `pinWindowDatabase` above — a failure here must never destabilize the
   * caller), but internally serialized via `proActivateChain` — see there.
   */
  private activateProSync(id: string): void {
    if (!isTauriBridgePresent()) return;
    this.proActivateChain = this.proActivateChain
      // A previous call's rejection must not abort this one's turn in the chain.
      .catch(() => {})
      .then(async () => {
        try {
          await invoke('pro_activate_database', { databaseId: id });
        } catch (err) {
          log.warn('Failed to re-target sync to the database', { id, error: err });
        }
      });
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
    try {
      // Land any in-flight debounced saves in the database they were made
      // against before the routed clients re-point.
      await sharedNodeStore.flushAllPendingSaves();
      // A newer switch superseded this one while we flushed — let it win.
      if (seq !== this.switchSeq) return;

      await invoke('set_active_database', { id });
      if (seq !== this.switchSeq) return;
      this.activeDatabaseId = id;
      // Remember the selection so a webview reload / app restart restores it
      // instead of snapping back to the daemon's registry default.
      rememberActiveDatabaseId(id);
      this.pinWindowDatabase(id);

      // Re-target Pro cloud-sync to follow the newly-active database (ADR-053
      // single-active sync): switching database in the app switches which tenant
      // syncs, not just which one is read/written. Best-effort — a re-target
      // failure must not abort the already-committed routing switch.
      this.activateProSync(id);

      // Evict the previous database's cached data. `clearAll()` also bumps the
      // store's database epoch, which closes the in-flight-read window: a read
      // (e.g. loadChildren/getNode) dispatched against the previous database
      // *before* this switch whose response resolves after this clear captured
      // the old epoch and is dropped instead of writing the previous
      // database's rows into the now-active store (see
      // `sharedNodeStore.currentEpoch()`).
      sharedNodeStore.clearAll();
      structureTree.clear();

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

      // Reload the sidebar from the new database. The locally-created
      // exemptions belong to the database being left — collection ids are
      // derived from the name, so keeping them would wrongly un-hide a
      // same-named empty collection in the new one.
      collectionsData.forgetLocallyCreated();
      // The per-collection member-node cache is keyed by collection id, which
      // is name-derived and can collide across databases — without this, a
      // same-named collection in the new database would render the *previous*
      // database's cached member nodes as its own contents.
      collectionsData.invalidateAllMembers();
      collectionsData.loadCollections();
      // Drop the sub-panel selection too: `collectionsState.selectedCollectionId`
      // / `subPanelOpen` are not evicted by anything above, so a panel left open
      // on a DB-A collection would otherwise keep rendering (now-stale) DB-A
      // members against DB-B, including for a DB-B collection that happens to
      // share the id.
      collectionsState.reset();
      // Same id-collision hazard as the member cache above, for the Pro
      // membership roster/invites/requests cache (has_role edges are
      // per-database — ADR-053).
      membership.invalidateForDatabaseSwitch();
      // As with collectionsData.forgetLocallyCreated() above: invalidate any
      // in-flight loadSchemas before reloading, so its result can't land in
      // a store that now represents a different database.
      schemasData.invalidateForDatabaseSwitch();
      schemasData.loadSchemas();
      // As with collectionsData.forgetLocallyCreated() above: invalidate any
      // in-flight "+ New chat" create before reloading, so its result can't
      // land in a store that now represents a different database.
      aiChatsData.invalidateForDatabaseSwitch();
      aiChatsData.loadAiChats();
      // Re-sync the schema plugin registry (hasTitleTemplate/titleTemplate)
      // against the newly-active database's schemas — otherwise a custom type
      // keeps resolving titles via the previous database's template (or, for a
      // type unique to the new database, via no template at all) until the
      // next app restart.
      void resyncSchemaPluginsForDatabaseSwitch();
      // Re-hydrate the new database's DatabaseSettingsNode (the previous one was
      // evicted by clearAll) so the Pro-sync variant re-resolves for it.
      this.refreshDatabaseSettings();
    } catch (err) {
      this.error = String(err);
      log.error('Failed to switch database', { id, error: err });
    }
  }

  /**
   * Force-refetch the active database's `DatabaseSettingsNode` into the shared
   * store (bypassing the cache-first `ensureNode` path) so the Pro-sync variant
   * machine re-resolves from fresh `sync_enabled`/`auth_status` values.
   *
   * The node is otherwise read once per app life and then kept fresh only by
   * `node:updated` watch events, which have unrecoverable loss modes (watcher
   * reconnect backoff, broadcast lag drops, failed coalescer refetch) — miss one
   * and the variant is stuck stale, e.g. at `sign-in` after the daemon already
   * flipped `auth_status` to connected. Callers re-pull on the
   * transitions that matter: sync-status edges, the consent decision, database
   * switch, and initial load.
   *
   * Fire-and-forget: callers must not block on it, and a missing node (older
   * database not yet backfilled) simply leaves the variant at its default.
   *
   * Also (re-)pins the singleton reachable, unconditionally and up front —
   * not only inside the fetch's `.then()` — so there is no window where the
   * node is cached but not pinned regardless of fetch timing, and so a
   * database switch's `clearAll()` (which wipes every pin along with every
   * cached node) gets its pin re-established the moment this runs again, the
   * same call that re-hydrates the node itself. See
   * `DATABASE_SETTINGS_PIN_OWNER`'s doc comment for why this needs pinning
   * at all: nothing about it is reachable via the structureTree walk.
   */
  refreshDatabaseSettings(): void {
    sharedNodeStore.pinNodes(DATABASE_SETTINGS_PIN_OWNER, [DATABASE_SETTINGS_NODE_ID]);

    const epoch = sharedNodeStore.currentEpoch();
    backendAdapter
      .getNode(DATABASE_SETTINGS_NODE_ID)
      .then((node) => {
        // The active database switched while the read was in flight — the row
        // belongs to the previous database, so drop it (see `currentEpoch()`).
        if (!node || sharedNodeStore.currentEpoch() !== epoch) return;
        sharedNodeStore.setNode(
          node,
          { type: 'database', reason: 'refresh-database-settings' },
          true
        );
      })
      .catch((err: unknown) => {
        log.debug('Could not refresh DatabaseSettingsNode', { error: err });
      });
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
      this.error = String(err);
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
      this.error = String(err);
      log.error('Database registry operation failed', err);
      return null;
    }
  }
}

export const databaseStore = new DatabaseStore();

// Retry the initial registry load once the daemon becomes reachable, mirroring
// the collections/schemas stores: a `load()` that ran while the daemon was
// still starting fails and leaves `activeDatabaseId` null (and the settings
// node unhydrated) until a manual reload. Guarded on the not-yet-loaded state
// so a background reconnect never re-runs against an established selection.
onDaemonReconnect(() => {
  if (databaseStore.activeDatabaseId === null) {
    void databaseStore.load();
  }
});

/**
 * True when a `database_id` event envelope belongs to the active database.
 * An empty id (single-database / Pro daemon) or an as-yet-unloaded selection
 * always applies — the guard only drops events explicitly tagged for a
 * different database, closing the race where a watch stream open across a
 * switch delivers the previous database's events.
 */
export function isActiveDatabaseEvent(databaseId: string | undefined): boolean {
  if (!databaseId) return true;
  if (databaseStore.activeDatabaseId === null) return true;
  return databaseId === databaseStore.activeDatabaseId;
}
