/**
 * Sync-status → settings-node re-hydration.
 *
 * The Pro-sync variant machine resolves from the active database's
 * DatabaseSettingsNode (axis 2), which used to be read once per app life and
 * then kept fresh only by `node:updated` watch events — with unrecoverable loss
 * modes (watcher reconnect backoff, broadcast lag drops, failed coalescer
 * refetch). Miss one and the variant stayed at `sign-in` forever: no consent
 * modal, wrong pill.
 *
 * These tests drive a real `sync:status` transition edge through the proSync
 * store and assert the settings node is force-refetched (bypassing the
 * cache-first path) so the variant flips `sign-in → consent` once the fresh
 * settings land — no watch event involved.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { Node } from '$lib/types';

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

// The forced refetch reads through the backend adapter (not invoke, not the
// cache-first ensureNode path) — intercept it here.
const mockGetNode = vi.fn((..._a: unknown[]) => Promise.resolve<Node | null>(null));
vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    getNode: (...a: unknown[]) => mockGetNode(...a)
  }
}));

import { proSync } from '$lib/stores/pro-sync.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import { SharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { resolveProSyncVariant } from '$lib/plugins/ui-extensions.svelte';
import { DATABASE_SETTINGS_NODE_ID } from '$lib/plugins/ui-extensions';

function emit(name: string, payload: unknown) {
  listeners.get(name)?.({ payload });
}

// The daemon serializes DatabaseSettingsNode with FLAT properties
// (`sync_enabled`/`auth_status` directly on `properties`), so mirror that shape.
function settingsNode(props: { sync_enabled?: boolean; auth_status?: string }): Node {
  return {
    id: DATABASE_SETTINGS_NODE_ID,
    nodeType: 'database-settings',
    content: '',
    properties: props,
    mentions: [],
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1
  };
}

/** Seed the shared store's settings singleton directly (the pre-refresh state). */
function seedSettings(props: { sync_enabled?: boolean; auth_status?: string }): void {
  SharedNodeStore.getInstance().setNode(
    settingsNode(props),
    { type: 'database', reason: 'seed' },
    true
  );
}

describe('sync:status transition → DatabaseSettingsNode re-hydration', () => {
  beforeEach(async () => {
    vi.clearAllMocks();
    listeners.clear();
    mockGetNode.mockReset();
    mockGetNode.mockResolvedValue(null);
    mockInvoke.mockImplementation(async (cmd: string) => (cmd === 'pro_tier' ? 'pro' : null));
    // proSync is a module singleton: normalize to a known signed-out baseline so
    // each test exercises a fresh transition edge.
    proSync.stop();
    proSync.state = 'unspecified';
    proSync.userEmail = '';
    proSync.tier = 'pro';
    // This suite exercises the tier/settings-node state machine directly, not
    // the Labs "Team synchronization" gate ahead of it (see ui-extensions.test.ts) —
    // default the gate ON so `resolveProSyncVariant()` still reflects those axes.
    labsFlags.syncEnabled = true;
    await proSync.start();
  });

  afterEach(() => {
    proSync.stop();
    proSync.tier = 'unknown';
    proSync.state = 'unspecified';
    proSync.userEmail = '';
    labsFlags.syncEnabled = false;
    SharedNodeStore.getInstance().clearAll();
    vi.restoreAllMocks();
  });

  it('a state edge force-refetches the settings node and flips sign-in → consent', async () => {
    // Stale axis-2 state: signed out, sync off → the sign-in variant.
    seedSettings({ sync_enabled: false, auth_status: 'local' });
    expect(resolveProSyncVariant()).toBe('sign-in');

    // The daemon has since flipped auth_status (the watch event for it was
    // lost); the forced refetch is the only feed left.
    mockGetNode.mockResolvedValue(settingsNode({ sync_enabled: false, auth_status: 'connected' }));

    // Realtime edge: unspecified → connected (sign-in completed).
    emit('sync:status', { state: 6, detail: '', user_email: 'mayank@nodespace.dev' });

    expect(mockGetNode).toHaveBeenCalledWith(DATABASE_SETTINGS_NODE_ID);
    // Once the fresh settings land, the consent modal's variant resolves — the
    // flow un-sticks without any node:updated event.
    await vi.waitFor(() => expect(resolveProSyncVariant()).toBe('consent'));
  });

  it('does not refetch on a redundant (non-edge) status event', async () => {
    emit('sync:status', { state: 6, detail: '', user_email: 'mayank@nodespace.dev' });
    expect(proSync.state).toBe('connected');
    mockGetNode.mockClear();

    // Same state delivered again (duplicate/heartbeat) — no new transition.
    emit('sync:status', { state: 6, detail: '', user_email: 'mayank@nodespace.dev' });

    expect(mockGetNode).not.toHaveBeenCalled();
  });

  it('refetches on every distinct transition (connected → syncing → connected)', async () => {
    emit('sync:status', { state: 6, detail: '', user_email: 'mayank@nodespace.dev' });
    mockGetNode.mockClear();

    emit('sync:status', { state: 5, detail: '', user_email: 'mayank@nodespace.dev' });
    emit('sync:status', { state: 6, detail: '', user_email: 'mayank@nodespace.dev' });

    expect(mockGetNode).toHaveBeenCalledTimes(2);
  });
});
