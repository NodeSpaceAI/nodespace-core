/**
 * Pro-tier sync state — populated by the Tauri capability probe and
 * the WatchSyncStatus stream from the Pro daemon (nodespaced-pro).
 *
 * Driven by two Tauri events:
 *   - `pro:tier-detected` fired once at startup with
 *     `{ tier, initial_status }`.
 *   - `sync:status` fired repeatedly while subscribed to
 *     CloudSyncService.WatchSyncStatus, payload `{ state, detail, user_email,
 *     database_id }`.
 *
 * In community mode, `tier` stays `'community'` and the UI surfaces
 * gated on `isPro` never render.
 *
 * ADR-053: the daemon runs at most ONE live sync session — the active
 * database's — and identity/sign-in are per database. `state`/`detail`/
 * `userEmail`/`authRequiredEpisode`/`dismissedReloginEpisode` are therefore
 * held per database (keyed by database id, see {@link DatabaseSyncStatus}),
 * not as single global fields: switching the active database must show that
 * database's own status (or a synthetic 'local-only' for one with no bound
 * tenant), never a previous database's leaked-over state. `tier`/`isPro`
 * stay global — Pro-ness is a daemon-binary capability, not a per-database
 * fact (see {@link ProSyncStore.isPro}'s doc comment).
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { createLogger } from '$lib/utils/logger';
import { databaseStore } from '$lib/stores/database.svelte';

const log = createLogger('ProSync');

export type ProTier = 'pro' | 'community' | 'unknown';

/**
 * SyncStatusEvent.State proto enum mirrored as a TS type, plus one synthetic,
 * frontend-only variant:
 *   - `'local-only'` is never decoded from the proto — it's derived purely
 *     from the active database having no `boundTenantSchema` (ADR-053: an
 *     unbound database structurally cannot have a live sync session). Update
 *     both `decodeState` and the proto file if a new PROTO state is added;
 *     `'local-only'` is deliberately excluded from that switch.
 */
export type SyncState =
  | 'unspecified'
  | 'disconnected'
  | 'connecting'
  | 'authenticating'
  | 'auth-required'
  | 'syncing'
  | 'connected'
  | 'error'
  | 'local-only';

/**
 * Decode `nodespace.pro.v1.SyncStatusEvent.State` numeric enum to the
 * TS string variant. Update both this switch and the proto file if a
 * new state is added.
 */
function decodeState(n: number): SyncState {
  switch (n) {
    case 1:
      return 'disconnected';
    case 2:
      return 'connecting';
    case 3:
      return 'authenticating';
    case 4:
      return 'auth-required';
    case 5:
      return 'syncing';
    case 6:
      return 'connected';
    case 7:
      return 'error';
    default:
      return 'unspecified';
  }
}

/** Per-database realtime sync/auth status (ADR-053). */
interface DatabaseSyncStatus {
  state: SyncState;
  detail: string;
  userEmail: string;
  /**
   * Bumped each time this database's `state` transitions into
   * 'auth-required' from some other state. Lets consumers (e.g. the re-login
   * modal's dismissal) distinguish a fresh auth-required episode from one
   * already dismissed — scoped per database so a dismissal recorded for one
   * database's episode counter can never coincidentally match another
   * database's counter value and wrongly suppress its own re-login prompt.
   */
  authRequiredEpisode: number;
  /**
   * The `authRequiredEpisode` (of THIS database) last dismissed via "Work
   * offline". `-1` = nothing dismissed yet for this database.
   */
  dismissedReloginEpisode: number;
}

function emptyStatus(): DatabaseSyncStatus {
  return {
    state: 'unspecified',
    detail: '',
    userEmail: '',
    authRequiredEpisode: 0,
    dismissedReloginEpisode: -1
  };
}

/**
 * Fallback key for an event that arrives with no `database_id` (the proto
 * payload's empty-string convention, mirroring `isActiveDatabaseEvent`)
 * before the registry has resolved an active database yet — e.g. the very
 * first `pro:tier-detected` snapshot at boot, which can race
 * `databaseStore.load()`. Once the registry resolves and the frontend issues
 * its own `pro_activate_database` call, subsequent events are attributed to
 * a real database id and this key stops being written to.
 */
const UNSCOPED_KEY = '__unscoped__';

/**
 * Synthetic status for an active database affirmatively known to be unbound
 * (ADR-053: no bound tenant => structurally no sync session). Every field is
 * forced clean, not just `state` — a stray/stale cached entry under that
 * database's id (e.g. a leftover from before a genuine re-target's status
 * gets cleared, or any future bug that writes one) must not leak `userEmail`
 * into `signedIn` on the pill, or a non-`-1` `dismissedReloginEpisode`, while
 * the pill itself reads 'local-only'. Frozen — shared by every read, never
 * mutated.
 */
const LOCAL_ONLY_ENTRY: DatabaseSyncStatus = Object.freeze({
  state: 'local-only',
  detail: '',
  userEmail: '',
  authRequiredEpisode: 0,
  dismissedReloginEpisode: -1
});

/** Reactive Pro-sync state — Svelte 5 runes via class-based pattern. */
class ProSyncStore {
  tier = $state<ProTier>('unknown');

  /**
   * Per-database status, keyed by database id (falling back to
   * {@link UNSCOPED_KEY}). Never exposed directly — read through the
   * `state`/`detail`/`userEmail`/`authRequiredEpisode`/
   * `dismissedReloginEpisode` getters below, which resolve against the
   * ACTIVE database so every consumer (the pill, the re-login/consent
   * slots) automatically tracks database switches without having to know
   * about the map.
   */
  private byDatabase = $state<Record<string, DatabaseSyncStatus>>({});

  /**
   * Email of the signed-in user, from the daemon's `SyncStatusEvent.user_email`
   * (decoded from the session JWT). Empty when signed out / not yet authenticated.
   * Drives the "signed in as <email>" affordance — needed because on the
   * silent-resume path the frontend never sees an OAuth response.
   */
  get userEmail(): string {
    return this.activeEntry.userEmail;
  }
  set userEmail(next: string) {
    this.patch(this.activeKey, { userEmail: next });
  }

  /**
   * Bumped each time `state` transitions into `'auth-required'` from some other
   * state, for the ACTIVE database. Lets consumers (e.g. the re-login modal's
   * dismissal) distinguish a fresh auth-required episode from the one already
   * dismissed, without an effect that reads and writes its own dismissal flag.
   */
  get authRequiredEpisode(): number {
    return this.activeEntry.authRequiredEpisode;
  }

  /**
   * The active database's `authRequiredEpisode` last dismissed via "Work
   * offline". Held per database (not globally) so a dismissal survives the
   * re-login slot remounting when the resolved variant flips between
   * `relogin` and `connected` — both map to the same re-login slot, so a flip
   * must not re-arm an already-dismissed modal — while a *different*
   * database's own auth-required episode is never suppressed by a dismissal
   * that was never made for it. `-1` = nothing dismissed yet for this database.
   */
  get dismissedReloginEpisode(): number {
    return this.activeEntry.dismissedReloginEpisode;
  }
  set dismissedReloginEpisode(next: number) {
    this.patch(this.activeKey, { dismissedReloginEpisode: next });
  }

  /**
   * Bumped each time `userEmail` transitions from empty to non-empty — i.e. a fresh
   * sign-in. Lets consumers (the sync pill's first-run onboarding) react to the
   * transition via a derived comparison instead of an effect that reads and writes
   * its own "seen" flag. Same pattern as authRequiredEpisode.
   *
   * Kept as a single global counter (not per database): the per-database
   * decision this gates — "has the first-Pro consent already been declined
   * for the database now showing?" — is answered by
   * `first-pro-consent-slot.svelte`'s own localStorage lookup keyed on
   * `databaseStore.activeDatabaseId`, so this only needs to distinguish "a
   * sign-in just happened" from "nothing changed" in-session.
   */
  signedInEpisode = $state(0);

  /**
   * Whether the first-Pro data-sharing consent modal is open. Set true when the
   * user clicks the enable-sync affordance; the consent slot renders the modal
   * off this flag and clears it on either choice. Held here (rather than inside a
   * component) so the enable-sync pill and the modal slot — separate registry
   * contributions — share one source of truth.
   */
  consentPromptOpen = $state(false);

  /**
   * The `signedInEpisode` for which the user declined the first-Pro publish consent
   * ("Keep local"). The consent slot auto-opens the modal once per fresh sign-in
   * episode; recording the declined episode here stops it from immediately
   * reopening for that same session, while leaving the pill available to reopen it.
   * Held on the store (not the slot) so it survives the slot remounting. `-1` =
   * nothing declined yet. Same episode-comparison pattern as `dismissedReloginEpisode`.
   */
  consentDeclinedEpisode = $state(-1);

  /**
   * Axis 1 only: "this daemon binary CAN sync" (the capability probe found
   * a Pro daemon). This is NOT "sync is active" — whether sync is enabled and
   * authenticated for the *active database* is axis 2, held on that database's
   * DatabaseSettingsNode. Use `isProSyncActive()` from `ui-extensions.svelte` for
   * the combined two-axis gate (what the membership store keys off).
   */
  isPro = $derived(this.tier === 'pro');

  /**
   * Callbacks fired once when `tier` first resolves to 'pro'. Lets consumers run a
   * Pro-confirmed one-shot (e.g. `collaboration-view.svelte` loading its collection
   * membership) from this state transition rather than an $effect that guards on
   * isPro (ADR-049). A callback
   * registered after tier is already 'pro' fires immediately.
   *
   * `proConfirmed` is a deliberate one-way latch: Pro tier is a per-process capability
   * probe result, not a session-lifetime flag, so it never resets on sign-out (unlike
   * `signedInEpisode`, which tracks the per-session sign-in edge and re-bumps). A
   * consumer needing to re-run on a genuine tier downgrade→upgrade would need a
   * different, episode-style signal — none exists today because tier does not change
   * within a running app.
   */
  private proConfirmedCallbacks = new Set<() => void>();
  private proConfirmed = false;

  /** Register a one-shot callback for Pro-tier confirmation. Returns an unregister fn. */
  onProConfirmed(callback: () => void): () => void {
    if (this.proConfirmed) {
      callback();
      return () => {};
    }
    this.proConfirmedCallbacks.add(callback);
    return () => this.proConfirmedCallbacks.delete(callback);
  }

  /** The key {@link byDatabase} is read/written under for the active database. */
  private get activeKey(): string {
    return databaseStore.activeDatabaseId ?? UNSCOPED_KEY;
  }

  /** Snapshot for `key`, defaulted (never mutates `byDatabase`). */
  private entryFor(key: string): DatabaseSyncStatus {
    return this.byDatabase[key] ?? emptyStatus();
  }

  /** Merge `next` into `key`'s entry, reassigning immutably so runes react. */
  private patch(key: string, next: Partial<DatabaseSyncStatus>): void {
    const prev = this.entryFor(key);
    this.byDatabase = { ...this.byDatabase, [key]: { ...prev, ...next } };
  }

  /**
   * Resolve which database an incoming event's `database_id` belongs to.
   * Empty (the proto payload's convention when the backend hasn't attributed
   * a session to a database yet) resolves to whatever this frontend currently
   * considers active, mirroring `isActiveDatabaseEvent`'s "no id => applies to
   * the active database" rule for the other per-database event streams.
   */
  private targetKey(databaseId: string | undefined): string {
    return databaseId || this.activeKey;
  }

  /**
   * True when the ACTIVE database is affirmatively known to have no bound
   * cloud tenant (ADR-053: an unbound database never has a sync session, so
   * any cached status for it would be spurious). `false` — never forced —
   * while the registry hasn't resolved the active database yet, so a
   * pre-`databaseStore.load()` read (or a test that doesn't seed the
   * registry) falls through to the per-database entry instead of defaulting
   * to a misleading "local-only".
   */
  private get activeIsUnbound(): boolean {
    const active = databaseStore.activeDatabase;
    return active !== null && !active.boundTenantSchema;
  }

  /**
   * The status every read-only getter below resolves against: the frozen
   * {@link LOCAL_ONLY_ENTRY} when the active database is affirmatively known
   * to be unbound, else its own cached entry. Funneling every getter through
   * this ONE resolver (rather than only overriding `state`) means a stray or
   * stale cached entry under an unbound database's id can never leak through
   * `userEmail`/`detail`/`authRequiredEpisode`/`dismissedReloginEpisode` even
   * while `state` correctly reads 'local-only'.
   */
  private get activeEntry(): DatabaseSyncStatus {
    if (this.activeIsUnbound) return LOCAL_ONLY_ENTRY;
    return this.entryFor(this.activeKey);
  }

  /**
   * Realtime sync state for the ACTIVE database — 'local-only' (synthetic,
   * never from the daemon) when it has no bound tenant, else that database's
   * own last-known `SyncStatusEvent.state`.
   */
  get state(): SyncState {
    return this.activeEntry.state;
  }
  set state(next: SyncState) {
    this.patch(this.activeKey, { state: next });
  }

  get detail(): string {
    return this.activeEntry.detail;
  }
  set detail(next: string) {
    this.patch(this.activeKey, { detail: next });
  }

  /**
   * Apply a state transition to `key`'s entry: bumps `authRequiredEpisode` on
   * a fresh (non-redundant) entry into 'auth-required', and — only when `key`
   * is the currently-active database — re-pulls that database's settings node
   * so axis 2 (the settings node's auth_status/sync_enabled) stays current.
   * Re-pulling for a backgrounded (non-active) database's event would refetch
   * the WRONG (currently active) database's settings node, so it's skipped.
   */
  private setState(key: string, next: SyncState): void {
    const prev = this.entryFor(key);
    let authRequiredEpisode = prev.authRequiredEpisode;
    if (next === 'auth-required' && prev.state !== 'auth-required') {
      authRequiredEpisode++;
    }
    const changed = next !== prev.state;
    this.patch(key, { state: next, authRequiredEpisode });
    // Axis 2 (the settings node's auth_status/sync_enabled) generally changes
    // when axis 1 (this realtime state) transitions — e.g. a sign-in flips
    // auth_status to 'connected'. Re-pull the settings node on every edge
    // rather than relying solely on the `node:updated` watch event, which can
    // be lost for good (watcher reconnect backoff, broadcast lag drops, failed
    // coalescer refetch) and would leave the Pro-sync variant permanently
    // stuck — e.g. at `sign-in` with the consent modal never appearing.
    if (changed && key === this.activeKey) {
      databaseStore.refreshDatabaseSettings();
    }
  }

  /** Set tier, firing proConfirmed callbacks once on the first transition to 'pro'. */
  private setTier(next: ProTier) {
    this.tier = next;
    if (next === 'pro' && !this.proConfirmed) {
      this.proConfirmed = true;
      for (const cb of this.proConfirmedCallbacks) cb();
      this.proConfirmedCallbacks.clear();
    }
  }

  /**
   * Set `key`'s userEmail, bumping the global `signedInEpisode` on an
   * empty→non-empty (fresh sign-in) edge — but ONLY when `key` is the
   * currently-active database, mirroring `setState`'s existing
   * `key === this.activeKey` guard on its own global side effect
   * (`refreshDatabaseSettings`). A backgrounded database's own sign-in
   * transition must not bump this: `first-pro-consent-slot.svelte`'s
   * auto-open condition compares `signedInEpisode` against
   * `consentDeclinedEpisode`, and an unrelated bump would re-arm an
   * already-dismissed consent modal for whatever database happens to be
   * active at the time — leaking a backgrounded database's sign-in into the
   * active one's UI, the exact class of bug this store exists to prevent.
   */
  private setUserEmail(key: string, next: string) {
    const prev = this.entryFor(key);
    if (next !== '' && prev.userEmail === '' && key === this.activeKey) {
      this.signedInEpisode++;
    }
    this.patch(key, { userEmail: next });
  }

  private unlistenTier: UnlistenFn | null = null;
  private unlistenStatus: UnlistenFn | null = null;
  private started = false;

  /**
   * Mount the listeners + kick off the streaming subscription. Safe
   * to call multiple times — only the first call wires anything up.
   * Returns a cleanup function for the caller to invoke on unmount.
   */
  async start(): Promise<() => void> {
    if (this.started) {
      return () => this.stop();
    }
    this.started = true;
    log.debug('mounting pro-sync listeners');

    // Pull the current tier in case `pro:tier-detected` already
    // fired before we subscribed (Tauri events are not buffered).
    try {
      const t = await invoke<ProTier>('pro_tier');
      this.setTier(t);
    } catch (e) {
      log.warn('pro_tier invoke failed', { error: e });
    }

    // Re-hydrate the current status snapshot. On a webview reload the one-shot
    // `pro:tier-detected` event (which carries the initial status) does not
    // re-fire, and `sync:status` only pushes on change — so without this a
    // signed-in Pro user would appear signed out until the next daemon
    // transition. The daemon session is intact; this reflects it deterministically.
    try {
      const snapshot = await invoke<{
        state: number;
        detail: string;
        user_email?: string;
        database_id?: string;
      } | null>('pro_current_status');
      if (snapshot) {
        const key = this.targetKey(snapshot.database_id);
        this.setState(key, decodeState(snapshot.state));
        this.patch(key, { detail: snapshot.detail });
        this.setUserEmail(key, snapshot.user_email ?? '');
      }
    } catch (e) {
      log.warn('pro_current_status invoke failed', { error: e });
    }

    this.unlistenTier = await listen<{
      tier: ProTier;
      initial_status: {
        state: number;
        detail: string;
        user_email?: string;
        database_id?: string;
      } | null;
    }>('pro:tier-detected', async (event) => {
      const p = event.payload;
      log.info('tier detected', { tier: p.tier });
      this.setTier(p.tier);
      if (p.initial_status) {
        const key = this.targetKey(p.initial_status.database_id);
        this.setState(key, decodeState(p.initial_status.state));
        this.patch(key, { detail: p.initial_status.detail });
        this.setUserEmail(key, p.initial_status.user_email ?? '');
      }
      // The first pro_subscribe_sync_status invoke below races the
      // backend's async init (Tauri setup spawns the connect on the
      // runtime and returns immediately). If ProClient wasn't yet in
      // managed state at first invoke, the command no-op'd. Re-invoke
      // now that we know ProClient is ready — backend is idempotent.
      if (p.tier === 'pro') {
        try {
          await invoke('pro_subscribe_sync_status');
        } catch (e) {
          log.warn('pro_subscribe_sync_status (post-tier) failed', { error: e });
        }
      }
    });

    this.unlistenStatus = await listen<{
      state: number;
      detail: string;
      user_email?: string;
      database_id?: string;
    }>('sync:status', (event) => {
      const key = this.targetKey(event.payload.database_id);
      this.setState(key, decodeState(event.payload.state));
      this.patch(key, { detail: event.payload.detail });
      this.setUserEmail(key, event.payload.user_email ?? '');
    });

    // Idempotent on the Rust side — subsequent calls return early.
    try {
      await invoke('pro_subscribe_sync_status');
    } catch (e) {
      log.warn('pro_subscribe_sync_status invoke failed', { error: e });
    }

    return () => this.stop();
  }

  /**
   * Manual sign-out. Tells the daemon to drop its session and wipe the
   * persisted refresh token from the keychain (so a restart won't auto-resume),
   * then optimistically reflects signed-out locally. The daemon's AUTH_REQUIRED
   * transition confirms via `sync:status`. No-op in community mode (the backend
   * command returns early with no `ProClient`).
   */
  async signOut(): Promise<void> {
    try {
      await invoke('pro_signout');
    } catch (e) {
      log.warn('pro_signout invoke failed', { error: e });
    }
    this.setState(this.activeKey, 'auth-required');
    this.setUserEmail(this.activeKey, '');
  }

  stop() {
    if (!this.started) return;
    this.unlistenTier?.();
    this.unlistenStatus?.();
    this.unlistenTier = null;
    this.unlistenStatus = null;
    this.started = false;
  }

  /**
   * Plain-object snapshot for `JSON.stringify` (e.g. `captureStoreDump` in
   * debug-channel.ts). Every `$state`/`$derived` class field here compiles to
   * a prototype accessor (`get`/`set`), not an own instance property, so
   * `JSON.stringify(proSync)` would otherwise serialize to `{}` — silently
   * dropping tier/state/userEmail/etc from a diagnostic dump. Includes the
   * full per-database map (keyed by database id) alongside the
   * active-database getters' resolved values, since a dump of exactly this
   * per-database state is the most useful thing to inspect when debugging a
   * cross-database leak (the class of bug ADR-053's split store exists to
   * prevent).
   */
  toJSON(): Record<string, unknown> {
    return {
      tier: this.tier,
      isPro: this.isPro,
      state: this.state,
      detail: this.detail,
      userEmail: this.userEmail,
      authRequiredEpisode: this.authRequiredEpisode,
      dismissedReloginEpisode: this.dismissedReloginEpisode,
      signedInEpisode: this.signedInEpisode,
      consentPromptOpen: this.consentPromptOpen,
      consentDeclinedEpisode: this.consentDeclinedEpisode,
      byDatabase: this.byDatabase
    };
  }
}

export const proSync = new ProSyncStore();
