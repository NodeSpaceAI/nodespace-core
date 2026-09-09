/**
 * Free-User Guardrail Suite
 * =========================
 *
 * A single, always-runnable suite that asserts every Pro/sync-gated **frontend**
 * feature stays completely inert in the community (free) build — i.e. when
 * `proSync.isPro` is false. The open-core rule is that Pro/sync work must keep the
 * community build behaviorally unchanged; this suite turns that rule into a
 * regression test you can run after any cross-repo or Pro-feature change.
 *
 *   bun run test:free-users
 *
 * If a future Pro feature adds a frontend surface, add its "inert in community"
 * assertion here so the guarantee stays enforced in one place.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { Node } from '$lib/types';

// Mock the Tauri bridge so we can assert Pro-gated daemon commands are NEVER
// invoked in the community build. `mock`-prefixed names satisfy vitest's hoisting.
const mockInvoke = vi.fn();
import { mockTauriCore } from './helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

const mockListeners = new Map<string, (event: { payload: unknown }) => void>();
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    mockListeners.set(name, handler);
    return () => mockListeners.delete(name);
  })
}));

import { proSync } from '$lib/stores/pro-sync.svelte';
import { sharedNodeStore, SharedNodeStore } from '$lib/services/shared-node-store.svelte';
import * as backendAdapterModule from '$lib/services/backend-adapter';
import { initializeTauriSyncListeners } from '$lib/services/tauri-sync-listener';
import {
  resolveProSyncVariant,
  isProSyncActive,
  getActiveChromeContributions,
  getActiveViewerExtensions
} from '$lib/plugins/ui-extensions.svelte';

function testNode(id: string, content = 'community content'): Node {
  return {
    id,
    nodeType: 'text',
    content,
    properties: {},
    mentions: [],
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1
  };
}

describe('Free-user guardrail: Pro features stay inert in the community build', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockListeners.clear();
    SharedNodeStore.resetInstance();
    // Community/free build: not Pro.
    proSync.tier = 'community';
    (global.window as unknown as { __TAURI__?: unknown }).__TAURI__ = {};
    vi.spyOn(backendAdapterModule.backendAdapter, 'getNode').mockImplementation(
      async (id: string) => testNode(id)
    );
  });

  afterEach(() => {
    proSync.tier = 'unknown';
    sharedNodeStore.clearAll();
    SharedNodeStore.resetInstance();
    delete (global.window as unknown as { __TAURI__?: unknown }).__TAURI__;
    vi.restoreAllMocks();
  });

  it('community tier → proSync.isPro is false', () => {
    expect(proSync.isPro).toBe(false);
  });

  it("the default 'unknown' tier (fresh app, before any probe) is also not Pro", () => {
    proSync.tier = 'unknown';
    expect(proSync.isPro).toBe(false);
  });

  // -------------------------------------------------------------------------
  // Reconnect-replay render coalescing — Pro-only. In community, incoming
  // node events must still apply through the unchanged per-event path; the
  // coalescing window must NOT engage. The guarantee for free users is simply:
  // live node updates still land in the store.
  // -------------------------------------------------------------------------
  describe('Reconnect-replay coalescer does not alter community behavior', () => {
    it('a node:updated event still applies to the store (per-event path)', async () => {
      await initializeTauriSyncListeners();
      const handler = mockListeners.get('node:updated');
      expect(handler).toBeDefined();

      handler!({ payload: { id: 'c1' } });

      await vi.waitFor(() => {
        expect(sharedNodeStore.hasNode('c1')).toBe(true);
      });
      // Fetched via the normal per-event path, not a Pro daemon command.
      expect(backendAdapterModule.backendAdapter.getNode).toHaveBeenCalledWith('c1');
      expect(mockInvoke).not.toHaveBeenCalled();
    });

    it('a node:deleted event still removes the node in community', async () => {
      sharedNodeStore.setNode(testNode('c2'), { type: 'database', reason: 'test' }, false);
      expect(sharedNodeStore.hasNode('c2')).toBe(true);

      await initializeTauriSyncListeners();
      mockListeners.get('node:deleted')!({ payload: { id: 'c2' } });

      expect(sharedNodeStore.hasNode('c2')).toBe(false);
    });
  });

  // -------------------------------------------------------------------------
  // Pro-UI registry — in community the only surface contributed is the
  // static upgrade teaser (ADR-039). Every daemon-backed Pro surface (the live
  // sync pill, the turn-on-sync prompt, the consent/re-login modals, the
  // collaboration tab) resolves out, so nothing that talks to a Pro daemon can
  // render.
  // -------------------------------------------------------------------------
  describe('Pro-UI registry resolves to only the static upgrade teaser', () => {
    it("community tier resolves the variant to 'teaser' and sync is inactive", () => {
      expect(resolveProSyncVariant()).toBe('teaser');
      expect(isProSyncActive()).toBe(false);
    });

    it('the overlay slot contributes exactly the teaser (no live/enable pill)', () => {
      const overlay = getActiveChromeContributions('app-shell-overlay');
      expect(overlay).toHaveLength(1);
      expect(overlay[0].variant).toBe('teaser');
      // None of the daemon-backed pill variants are active.
      for (const v of ['sign-in', 'consent', 'relogin', 'connected'] as const) {
        expect(overlay.some((c) => c.variant === v)).toBe(false);
      }
    });

    it('the modal slot and the collaboration tab contribute nothing in community', () => {
      expect(getActiveChromeContributions('app-shell-modal')).toEqual([]);
      expect(getActiveViewerExtensions('collection')).toEqual([]);
    });

    it("the default 'unknown' tier (pre-probe) is teaser-only too — no Pro surface flashes", () => {
      proSync.tier = 'unknown';
      expect(resolveProSyncVariant()).toBe('teaser');
      expect(getActiveChromeContributions('app-shell-modal')).toEqual([]);
      expect(getActiveViewerExtensions('collection')).toEqual([]);
    });
  });
});
