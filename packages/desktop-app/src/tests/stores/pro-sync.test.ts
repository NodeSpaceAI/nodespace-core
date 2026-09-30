/**
 * ProSync store — signed-in identity + manual sign-out.
 *
 * Asserts the store surfaces `SyncStatusEvent.user_email` (the "signed in as
 * <email>" affordance) from the `sync:status` stream and clears it on sign-out,
 * and that `signOut()` drives the daemon command + optimistically resets state.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

// Capture the `sync:status` / `pro:tier-detected` handlers so tests can drive them.
const listeners = new Map<string, (event: { payload: unknown }) => void>();
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler);
    return () => listeners.delete(name);
  })
}));

import { proSync } from '$lib/stores/pro-sync.svelte';
import { databaseStore, type DatabaseInfo } from '$lib/stores/database.svelte';

function emit(name: string, payload: unknown) {
  listeners.get(name)?.({ payload });
}

function db(id: string, overrides: Partial<DatabaseInfo> = {}): DatabaseInfo {
  return {
    id,
    name: id,
    path: `/tmp/${id}`,
    isDefault: false,
    status: 'open',
    createdAt: new Date().toISOString(),
    lastOpenedAt: null,
    boundTenantSchema: null,
    boundTenantCollection: null,
    extensions: {},
    ...overrides
  };
}

describe('ProSync store — signed-in identity + sign-out', () => {
  beforeEach(async () => {
    vi.clearAllMocks();
    mockInvoke.mockResolvedValue('pro');
    await proSync.start();
    emit('pro:tier-detected', { tier: 'pro', initial_status: null });
  });

  it('surfaces user_email from sync:status and clears it when signed out', () => {
    emit('sync:status', { state: 6, detail: '', user_email: 'mayank@nodespace.dev' });
    expect(proSync.state).toBe('connected');
    expect(proSync.userEmail).toBe('mayank@nodespace.dev');

    // A signed-out (AUTH_REQUIRED) event with no email clears the affordance.
    emit('sync:status', { state: 4, detail: 'signed out', user_email: '' });
    expect(proSync.state).toBe('auth-required');
    expect(proSync.userEmail).toBe('');
  });

  it('tolerates a missing user_email field (older payloads) as empty', () => {
    emit('sync:status', { state: 6, detail: '' });
    expect(proSync.userEmail).toBe('');
  });

  it('signOut() invokes pro_signout and optimistically resets', async () => {
    emit('sync:status', { state: 6, detail: '', user_email: 'mayank@nodespace.dev' });
    mockInvoke.mockClear();
    mockInvoke.mockResolvedValue(undefined);

    await proSync.signOut();

    expect(mockInvoke).toHaveBeenCalledWith('pro_signout');
    expect(proSync.userEmail).toBe('');
    expect(proSync.state).toBe('auth-required');
  });
});

describe('ProSync store — authRequiredEpisode transition counter', () => {
  // proSync is a module-level singleton, so state carries over between tests.
  // Force a known non-auth-required starting state before each test so the
  // transition-into-auth-required edge is always exercised freshly.
  beforeEach(async () => {
    vi.clearAllMocks();
    mockInvoke.mockResolvedValue('pro');
    await proSync.start();
    emit('sync:status', { state: 6, detail: '', user_email: 'mayank@nodespace.dev' });
    expect(proSync.state).toBe('connected');
  });

  it('bumps the episode counter when transitioning into auth-required', () => {
    const before = proSync.authRequiredEpisode;

    emit('sync:status', { state: 4, detail: 'signed out', user_email: '' });

    expect(proSync.state).toBe('auth-required');
    expect(proSync.authRequiredEpisode).toBe(before + 1);
  });

  it('does not re-bump the counter while repeatedly re-entering auth-required via redundant events', () => {
    emit('sync:status', { state: 4, detail: '', user_email: '' });
    const afterFirst = proSync.authRequiredEpisode;

    // Same state delivered again (e.g. a duplicate/retried sync:status event).
    emit('sync:status', { state: 4, detail: '', user_email: '' });

    expect(proSync.authRequiredEpisode).toBe(afterFirst);
  });

  it('bumps the counter again on a distinct re-entry into auth-required', () => {
    emit('sync:status', { state: 4, detail: '', user_email: '' });
    const firstEpisode = proSync.authRequiredEpisode;

    // Leaves auth-required...
    emit('sync:status', { state: 6, detail: '', user_email: 'mayank@nodespace.dev' });
    expect(proSync.state).toBe('connected');

    // ...then re-enters it. This is a new episode and must be distinguishable
    // from the first, so a dismissal recorded against firstEpisode doesn't
    // silently suppress the re-login prompt for the new episode.
    emit('sync:status', { state: 4, detail: '', user_email: '' });

    expect(proSync.authRequiredEpisode).toBe(firstEpisode + 1);
  });

  it('bumps the counter when the cold-start initial_status is already auth-required', async () => {
    // A fresh cold-start: tier-detected delivers initial_status directly,
    // without any prior sync:status event establishing a baseline state.
    const before = proSync.authRequiredEpisode;

    emit('pro:tier-detected', {
      tier: 'pro',
      initial_status: { state: 4, detail: '', user_email: '' }
    });

    expect(proSync.state).toBe('auth-required');
    expect(proSync.authRequiredEpisode).toBe(before + 1);
  });
});


describe('ProSync store — signedInEpisode transition counter', () => {
  beforeEach(async () => {
    vi.clearAllMocks();
    mockInvoke.mockResolvedValue('pro');
    await proSync.start();
    // Reset to a signed-out baseline so the empty→non-empty edge fires freshly.
    emit('sync:status', { state: 4, detail: '', user_email: '' });
    expect(proSync.userEmail).toBe('');
  });

  it('bumps the episode when userEmail transitions from empty to non-empty', () => {
    const before = proSync.signedInEpisode;

    emit('sync:status', { state: 6, detail: '', user_email: 'mayank@nodespace.dev' });

    expect(proSync.userEmail).toBe('mayank@nodespace.dev');
    expect(proSync.signedInEpisode).toBe(before + 1);
  });

  it('does not bump while already signed in (non-empty → non-empty)', () => {
    emit('sync:status', { state: 6, detail: '', user_email: 'mayank@nodespace.dev' });
    const afterSignIn = proSync.signedInEpisode;

    // A subsequent status update for the same signed-in user must not re-bump.
    emit('sync:status', { state: 5, detail: 'syncing', user_email: 'mayank@nodespace.dev' });

    expect(proSync.signedInEpisode).toBe(afterSignIn);
  });

  it('bumps again on a fresh sign-in after signing out', () => {
    emit('sync:status', { state: 6, detail: '', user_email: 'mayank@nodespace.dev' });
    const firstEpisode = proSync.signedInEpisode;

    emit('sync:status', { state: 4, detail: '', user_email: '' }); // sign out
    emit('sync:status', { state: 6, detail: '', user_email: 'other@nodespace.dev' }); // sign back in

    expect(proSync.signedInEpisode).toBe(firstEpisode + 1);
  });
});

describe('ProSync store — onProConfirmed', () => {
  // proSync is a module-level singleton and proConfirmed is a one-way latch, so once
  // any prior test has confirmed Pro tier it stays confirmed. These assertions are
  // therefore written to be order-independent: they exercise the "already pro" path,
  // which is the steady state after tier detection.
  beforeEach(async () => {
    vi.clearAllMocks();
    mockInvoke.mockResolvedValue('pro');
    await proSync.start();
    emit('pro:tier-detected', { tier: 'pro', initial_status: null });
  });

  it('fires a callback registered after tier is already pro immediately', () => {
    const cb = vi.fn();
    proSync.onProConfirmed(cb);
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it('fires each registered callback exactly once, not again on re-confirmation', () => {
    const cb = vi.fn();
    proSync.onProConfirmed(cb);
    expect(cb).toHaveBeenCalledTimes(1);

    // A subsequent pro re-confirmation must not re-fire it.
    emit('pro:tier-detected', { tier: 'pro', initial_status: null });
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it('returns an unregister function', () => {
    const cb = vi.fn();
    const unregister = proSync.onProConfirmed(cb);
    expect(typeof unregister).toBe('function');
    expect(() => unregister()).not.toThrow();
  });
});

describe('ProSync store — reload re-hydration', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Reset the idempotent-start latch so start() re-runs its hydration, as it
    // does on a fresh page load / webview reload. Clear carry-over identity from
    // earlier tests (proSync is a module singleton).
    proSync.stop();
    proSync.userEmail = '';
    proSync.state = 'unspecified';
  });

  it('re-hydrates signed-in identity from the current-status snapshot on start', async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'pro_tier') return 'pro';
      if (cmd === 'pro_current_status') {
        return { state: 6, detail: 'live', user_email: 'mayank@nodespace.dev' };
      }
      return undefined;
    });

    await proSync.start();

    // The reload path: neither `pro:tier-detected` nor `sync:status` fires, yet
    // the signed-in state is restored deterministically from the snapshot.
    expect(mockInvoke).toHaveBeenCalledWith('pro_current_status');
    expect(proSync.tier).toBe('pro');
    expect(proSync.state).toBe('connected');
    expect(proSync.userEmail).toBe('mayank@nodespace.dev');
  });

  it('leaves the snapshot alone when the daemon has no status yet (null)', async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'pro_tier') return 'pro';
      if (cmd === 'pro_current_status') return null;
      return undefined;
    });

    await proSync.start();

    expect(proSync.tier).toBe('pro');
    // No snapshot → userEmail is not populated (stays signed-out).
    expect(proSync.userEmail).toBe('');
  });
});

describe('ProSync store — per-database status (ADR-053: per-database sync/auth, single-active session)', () => {
  beforeEach(async () => {
    vi.clearAllMocks();
    proSync.stop();
    mockInvoke.mockResolvedValue('pro');
    await proSync.start();
    emit('pro:tier-detected', { tier: 'pro', initial_status: null });
  });

  afterEach(() => {
    databaseStore.databases = [];
    databaseStore.activeDatabaseId = null;
  });

  it('switching between a synced and a local-only database shows the correct state for each, never leaking', () => {
    databaseStore.databases = [db('db-synced', { boundTenantSchema: 'tenant_alpha' }), db('db-local')];

    // The synced database goes live.
    databaseStore.activeDatabaseId = 'db-synced';
    emit('sync:status', {
      state: 6,
      detail: '',
      user_email: 'mayank@nodespace.dev',
      database_id: 'db-synced'
    });
    expect(proSync.state).toBe('connected');
    expect(proSync.userEmail).toBe('mayank@nodespace.dev');

    // Switching to the unbound database must show a clean local-only state —
    // never the previous database's 'connected'/signed-in state leaking over.
    databaseStore.activeDatabaseId = 'db-local';
    expect(proSync.state).toBe('local-only');
    expect(proSync.userEmail).toBe('');

    // Switching back shows db-synced's own last-known state again, without
    // needing a fresh event — it was never overwritten by the local-only view.
    databaseStore.activeDatabaseId = 'db-synced';
    expect(proSync.state).toBe('connected');
    expect(proSync.userEmail).toBe('mayank@nodespace.dev');
  });

  it('forces local-only for an unbound active database even if a stray entry exists under its id', () => {
    databaseStore.databases = [db('db-local')];
    databaseStore.activeDatabaseId = 'db-local';
    // A status somehow tagged for this (structurally sync-less) database —
    // must never surface; the unbound check wins over any cached entry.
    emit('sync:status', {
      state: 6,
      detail: 'catching up',
      user_email: 'x@example.com',
      database_id: 'db-local'
    });
    // Also drive an auth-required episode + a dismissal under this same id,
    // so authRequiredEpisode/dismissedReloginEpisode have a non-default,
    // stray value too — not just state/userEmail/detail.
    emit('sync:status', { state: 4, detail: '', user_email: '', database_id: 'db-local' });
    proSync.dismissedReloginEpisode = proSync.authRequiredEpisode;

    // Regression: the local-only override must cover EVERY field the pill
    // and re-login/consent slots read, not just `state` — otherwise a grey
    // "Local only" pill could still be clickable (userEmail !== '') and open
    // an account menu claiming "Signed in as x@example.com".
    expect(proSync.state).toBe('local-only');
    expect(proSync.userEmail).toBe('');
    expect(proSync.detail).toBe('');
    expect(proSync.authRequiredEpisode).toBe(0);
    expect(proSync.dismissedReloginEpisode).toBe(-1);
  });

  it('does not force local-only while the registry has not resolved the active database yet', () => {
    // No databases loaded — activeDatabase is null (unresolved), so the
    // unbound override must fail safe to the cached/default entry rather than
    // forcing 'local-only' for an unknown database.
    databaseStore.databases = [];
    databaseStore.activeDatabaseId = 'not-yet-loaded';

    expect(proSync.state).toBe('unspecified');
  });

  it('caches independent status per database from database_id-tagged events, regardless of switch order', () => {
    databaseStore.databases = [
      db('db-alpha', { boundTenantSchema: 'tenant_alpha' }),
      db('db-beta', { boundTenantSchema: 'tenant_beta' })
    ];

    emit('sync:status', { state: 5, detail: 'catching up', user_email: 'a@example.com', database_id: 'db-alpha' });
    emit('sync:status', { state: 4, detail: '', user_email: '', database_id: 'db-beta' });

    databaseStore.activeDatabaseId = 'db-alpha';
    expect(proSync.state).toBe('syncing');
    expect(proSync.detail).toBe('catching up');
    expect(proSync.userEmail).toBe('a@example.com');

    databaseStore.activeDatabaseId = 'db-beta';
    expect(proSync.state).toBe('auth-required');
    expect(proSync.userEmail).toBe('');
  });

  it('does not leak a signed-in email into a different, never-signed-in database', () => {
    // Regression: resolveProSyncVariant's `authed` check falls back to
    // `proSync.userEmail !== ''` — a global field would leak a signed-in
    // email from a previous database into an unrelated fresh one, wrongly
    // resolving that database's variant as authenticated.
    databaseStore.databases = [
      db('db-signed-in', { boundTenantSchema: 'tenant_alpha' }),
      db('db-fresh', { boundTenantSchema: 'tenant_beta' })
    ];
    databaseStore.activeDatabaseId = 'db-signed-in';
    emit('sync:status', {
      state: 6,
      detail: '',
      user_email: 'mayank@nodespace.dev',
      database_id: 'db-signed-in'
    });
    expect(proSync.userEmail).toBe('mayank@nodespace.dev');

    databaseStore.activeDatabaseId = 'db-fresh';
    expect(proSync.userEmail).toBe('');
  });

  it('isolates authRequiredEpisode/dismissedReloginEpisode per database so a dismissal on one never suppresses another', () => {
    databaseStore.databases = [
      db('db-alpha', { boundTenantSchema: 'tenant_alpha' }),
      db('db-beta', { boundTenantSchema: 'tenant_beta' })
    ];

    // db-alpha goes auth-required and the user dismisses it ("Work offline").
    databaseStore.activeDatabaseId = 'db-alpha';
    emit('sync:status', { state: 4, detail: '', user_email: '', database_id: 'db-alpha' });
    const alphaEpisode = proSync.authRequiredEpisode;
    proSync.dismissedReloginEpisode = alphaEpisode;
    expect(proSync.dismissedReloginEpisode).toBe(proSync.authRequiredEpisode);

    // db-beta independently goes auth-required — its own episode counter
    // starts from its own history and must not read as already-dismissed
    // just because db-alpha's counter happened to reach the same value.
    databaseStore.activeDatabaseId = 'db-beta';
    emit('sync:status', { state: 4, detail: '', user_email: '', database_id: 'db-beta' });

    expect(proSync.state).toBe('auth-required');
    expect(proSync.dismissedReloginEpisode).not.toBe(proSync.authRequiredEpisode);
  });

  it('does not bump the global signedInEpisode for a BACKGROUNDED database\'s own sign-in event', () => {
    // Regression: a background database's sign-in must not re-arm the first-Pro
    // consent modal's auto-open for whatever OTHER database is currently active
    // (first-pro-consent-slot.svelte compares signedInEpisode against
    // consentDeclinedEpisode — both global — to decide whether to auto-pop).
    databaseStore.databases = [
      db('db-active', { boundTenantSchema: 'tenant_active' }),
      db('db-background', { boundTenantSchema: 'tenant_background' })
    ];
    databaseStore.activeDatabaseId = 'db-active';

    const before = proSync.signedInEpisode;
    // db-background (NOT active) signs in for the first time.
    emit('sync:status', {
      state: 6,
      detail: '',
      user_email: 'background@example.com',
      database_id: 'db-background'
    });

    expect(proSync.signedInEpisode).toBe(before);
    // The active database's own userEmail is untouched by the background event.
    expect(proSync.userEmail).toBe('');

    // But db-background's OWN sign-in did land in its own entry — switching to
    // it later shows the real state, it just didn't fire the global episode
    // bump while backgrounded.
    databaseStore.activeDatabaseId = 'db-background';
    expect(proSync.userEmail).toBe('background@example.com');
  });

  it('bumps the global signedInEpisode for the ACTIVE database\'s own sign-in event', () => {
    databaseStore.databases = [db('db-active', { boundTenantSchema: 'tenant_active' })];
    databaseStore.activeDatabaseId = 'db-active';

    const before = proSync.signedInEpisode;
    emit('sync:status', {
      state: 6,
      detail: '',
      user_email: 'active@example.com',
      database_id: 'db-active'
    });

    expect(proSync.signedInEpisode).toBe(before + 1);
  });

  it('an event with no database_id attributes to whichever database is active at receipt time', () => {
    databaseStore.databases = [db('db-gamma', { boundTenantSchema: 'tenant_gamma' })];
    databaseStore.activeDatabaseId = 'db-gamma';

    // Older/synthetic payload shape with no database_id (e.g. emit_disconnected).
    emit('sync:status', { state: 1, detail: 'sync-status stream ended' });

    expect(proSync.state).toBe('disconnected');
    expect(proSync.detail).toBe('sync-status stream ended');
  });

  it('toJSON exposes the active-database getters (every $state field is a prototype accessor, invisible to plain JSON.stringify)', () => {
    databaseStore.databases = [db('db-delta', { boundTenantSchema: 'tenant_delta' })];
    databaseStore.activeDatabaseId = 'db-delta';
    emit('sync:status', {
      state: 6,
      detail: 'live',
      user_email: 'mayank@nodespace.dev',
      database_id: 'db-delta'
    });

    const dumped = JSON.parse(JSON.stringify(proSync));

    expect(dumped.tier).toBe('pro');
    expect(dumped.state).toBe('connected');
    expect(dumped.userEmail).toBe('mayank@nodespace.dev');
    expect(dumped.byDatabase['db-delta'].state).toBe('connected');
  });
});
