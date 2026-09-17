/**
 * UI-extension registry + Pro-sync variant machine.
 *
 * Exercises the two-signal state machine (`proSync.tier` × the active database's
 * DatabaseSettingsNode) and the registry filtering that resolves which chrome /
 * viewer contributions are active for each variant. The flow is sign-in-first:
 * sign-in → consent → connected, with relogin as the re-auth state for an
 * already-enabled database.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { Node } from '$lib/types';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

// SharedNodeStore.setNode does not persist here (skipPersistence), but stub the
// Tauri bridge so nothing reaches a real daemon.
const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

import { proSync } from '$lib/stores/pro-sync.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import { SharedNodeStore } from '$lib/services/shared-node-store.svelte';
import {
  resolveProSyncVariant,
  isProSyncActive,
  getActiveChromeContributions,
  getActiveViewerExtensions
} from '$lib/plugins/ui-extensions.svelte';
import { uiExtensionRegistry, DATABASE_SETTINGS_NODE_ID } from '$lib/plugins/ui-extensions';

/**
 * Seed the active database's settings singleton with the given props. The daemon
 * serializes DatabaseSettingsNode with FLAT properties (`sync_enabled`/`auth_status`
 * directly on `properties`), so mirror that shape here.
 */
function seedSettings(props: { sync_enabled?: boolean; auth_status?: string }): void {
  const node: Node = {
    id: DATABASE_SETTINGS_NODE_ID,
    nodeType: 'database-settings',
    content: '',
    properties: props,
    mentions: [],
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1
  };
  SharedNodeStore.getInstance().setNode(node, { type: 'database', reason: 'seed' }, true);
}

describe('UI-extension registry', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    SharedNodeStore.resetInstance();
    proSync.tier = 'unknown';
    proSync.userEmail = '';
    // The Labs "Team synchronization" toggle (default OFF) is a separate,
    // client-side visibility gate checked ahead of the tier/settings axes
    // below (see the "Labs syncEnabled gate" describe block) — default it ON
    // here so the pre-existing two-axis variant-resolution tests continue to
    // exercise those axes in isolation, as they did before that gate existed.
    labsFlags.syncEnabled = true;
  });

  afterEach(() => {
    proSync.tier = 'unknown';
    proSync.userEmail = '';
    labsFlags.syncEnabled = false;
    SharedNodeStore.resetInstance();
    vi.restoreAllMocks();
  });

  describe('registry registration', () => {
    it('registers the built-in pro-sync extension with all contributions', () => {
      expect(uiExtensionRegistry.has('pro-sync')).toBe(true);
      // No overlay chrome — the top-right pill was removed; account access
      // moved to Settings.
      expect(uiExtensionRegistry.chromeFor('app-shell-overlay')).toEqual([]);
      // 3 modals (consent / relogin / connected) — no modal for teaser or sign-in.
      expect(uiExtensionRegistry.chromeFor('app-shell-modal')).toHaveLength(3);
      // 4 collaboration viewer extensions (sign-in / consent / relogin / connected).
      expect(uiExtensionRegistry.viewersFor('collection')).toHaveLength(4);
      expect(uiExtensionRegistry.viewersFor('text')).toEqual([]);
    });
  });

  describe('variant resolution', () => {
    it("tier !== 'pro' → teaser, regardless of the settings node", () => {
      proSync.tier = 'community';
      seedSettings({ sync_enabled: true, auth_status: 'connected' });
      expect(resolveProSyncVariant()).toBe('teaser');
      expect(isProSyncActive()).toBe(false);
    });

    it('pro + no hydrated settings node → sign-in (not enabled, not authed)', () => {
      proSync.tier = 'pro';
      expect(resolveProSyncVariant()).toBe('sign-in');
      expect(isProSyncActive()).toBe(false);
    });

    it("pro + sync_enabled: false + auth_status 'local' → sign-in", () => {
      proSync.tier = 'pro';
      seedSettings({ sync_enabled: false, auth_status: 'local' });
      expect(resolveProSyncVariant()).toBe('sign-in');
      expect(isProSyncActive()).toBe(false);
    });

    it("pro + sync_enabled: false + auth_status 'connected' → consent (signed in, publish pending)", () => {
      proSync.tier = 'pro';
      seedSettings({ sync_enabled: false, auth_status: 'connected' });
      expect(resolveProSyncVariant()).toBe('consent');
      // Not active yet — consent gates the sync_enabled flip.
      expect(isProSyncActive()).toBe(false);
    });

    it('pro + live sign-in (userEmail) but settings node not hydrated → consent, not sign-in', () => {
      // A fresh Pro sign-in where the DatabaseSettingsNode has not hydrated its
      // auth_status yet: the live WatchSyncStatus signal (userEmail) must still
      // resolve `consent` so the enable-sync affordance appears — otherwise a new
      // Pro user has no way to turn sync on.
      proSync.tier = 'pro';
      proSync.userEmail = 'new-user@example.com';
      // no seedSettings → settings node absent (unhydrated)
      expect(resolveProSyncVariant()).toBe('consent');
    });

    it('pro + live sign-in (userEmail) overrides a stale auth_status:local → consent', () => {
      proSync.tier = 'pro';
      proSync.userEmail = 'new-user@example.com';
      seedSettings({ sync_enabled: false, auth_status: 'local' });
      expect(resolveProSyncVariant()).toBe('consent');
    });

    it('pro + signed out (no userEmail) + unhydrated settings → sign-in (fallback does not false-positive)', () => {
      proSync.tier = 'pro';
      proSync.userEmail = '';
      expect(resolveProSyncVariant()).toBe('sign-in');
    });

    it("pro + sync_enabled + auth_status 'local' → relogin (enabled but session lapsed)", () => {
      proSync.tier = 'pro';
      seedSettings({ sync_enabled: true, auth_status: 'local' });
      expect(resolveProSyncVariant()).toBe('relogin');
      expect(isProSyncActive()).toBe(true);
    });

    it("pro + sync_enabled + auth_status 'connected' → connected (sync active)", () => {
      proSync.tier = 'pro';
      seedSettings({ sync_enabled: true, auth_status: 'connected' });
      expect(resolveProSyncVariant()).toBe('connected');
      expect(isProSyncActive()).toBe(true);
    });

    it('sign-in-first transition: sign-in → consent → connected as auth then sync land', () => {
      proSync.tier = 'pro';
      // Fresh Pro database: sign in first.
      seedSettings({ sync_enabled: false, auth_status: 'local' });
      expect(resolveProSyncVariant()).toBe('sign-in');
      // After sign-in, the publish consent is presented — still nothing enabled.
      seedSettings({ sync_enabled: false, auth_status: 'connected' });
      expect(resolveProSyncVariant()).toBe('consent');
      expect(isProSyncActive()).toBe(false);
      // Merge flips sync_enabled → connected and active.
      seedSettings({ sync_enabled: true, auth_status: 'connected' });
      expect(resolveProSyncVariant()).toBe('connected');
      expect(isProSyncActive()).toBe(true);
    });
  });

  describe('active contribution filtering', () => {
    it('teaser: no overlay, no modal, no collab tab', () => {
      proSync.tier = 'community';
      expect(getActiveChromeContributions('app-shell-overlay')).toEqual([]);
      expect(getActiveChromeContributions('app-shell-modal')).toEqual([]);
      expect(getActiveViewerExtensions('collection')).toEqual([]);
    });

    it('sign-in: no overlay, no modal, a locked collab tab', () => {
      proSync.tier = 'pro';
      seedSettings({ sync_enabled: false, auth_status: 'local' });
      expect(getActiveChromeContributions('app-shell-overlay')).toEqual([]);
      // No consent modal before sign-in.
      expect(getActiveChromeContributions('app-shell-modal')).toEqual([]);
      const viewers = getActiveViewerExtensions('collection');
      expect(viewers).toHaveLength(1);
      expect(viewers[0].variant).toBe('sign-in');
      expect(viewers[0].tab.id).toBe('collaboration');
    });

    it('consent: no overlay, the consent modal, a locked collab tab', () => {
      proSync.tier = 'pro';
      seedSettings({ sync_enabled: false, auth_status: 'connected' });
      expect(getActiveChromeContributions('app-shell-overlay')).toEqual([]);
      const modal = getActiveChromeContributions('app-shell-modal');
      expect(modal).toHaveLength(1);
      expect(modal[0].variant).toBe('consent');
      const viewers = getActiveViewerExtensions('collection');
      expect(viewers).toHaveLength(1);
      expect(viewers[0].variant).toBe('consent');
    });

    it('relogin: no overlay, the relogin modal, the live collab tab', () => {
      proSync.tier = 'pro';
      seedSettings({ sync_enabled: true, auth_status: 'local' });
      expect(getActiveChromeContributions('app-shell-overlay')).toEqual([]);
      const modal = getActiveChromeContributions('app-shell-modal');
      expect(modal).toHaveLength(1);
      expect(modal[0].variant).toBe('relogin');
      const viewers = getActiveViewerExtensions('collection');
      expect(viewers).toHaveLength(1);
      expect(viewers[0].variant).toBe('relogin');
    });

    it('connected: no overlay, the relogin modal, the live collab tab', () => {
      proSync.tier = 'pro';
      seedSettings({ sync_enabled: true, auth_status: 'connected' });
      expect(getActiveChromeContributions('app-shell-overlay')).toEqual([]);
      const modal = getActiveChromeContributions('app-shell-modal');
      expect(modal).toHaveLength(1);
      expect(modal[0].variant).toBe('connected');
      const viewers = getActiveViewerExtensions('collection');
      expect(viewers).toHaveLength(1);
      expect(viewers[0].variant).toBe('connected');
    });
  });

  describe('Labs syncEnabled gate (default OFF)', () => {
    it('forces teaser regardless of tier/settings when the flag is off — the chokepoint used by every consumer', () => {
      labsFlags.syncEnabled = false;
      proSync.tier = 'pro';
      seedSettings({ sync_enabled: true, auth_status: 'connected' });

      // Otherwise this would resolve 'connected' — see the equivalent test
      // above with the flag on. Off, it must short-circuit to 'teaser' BEFORE
      // even checking `proSync.tier`, hiding every downstream surface at once.
      expect(resolveProSyncVariant()).toBe('teaser');
      expect(isProSyncActive()).toBe(false);
      expect(getActiveChromeContributions('app-shell-modal')).toEqual([]);
      expect(getActiveViewerExtensions('collection')).toEqual([]);
    });

    it('flipping the flag back on reveals exactly the pre-existing variant, unchanged', () => {
      proSync.tier = 'pro';
      seedSettings({ sync_enabled: true, auth_status: 'connected' });

      labsFlags.syncEnabled = false;
      expect(resolveProSyncVariant()).toBe('teaser');

      labsFlags.syncEnabled = true;
      expect(resolveProSyncVariant()).toBe('connected');
      expect(isProSyncActive()).toBe(true);
    });

    it("does not change a community build's variant either way — already teaser via tier alone", () => {
      proSync.tier = 'community';

      labsFlags.syncEnabled = false;
      expect(resolveProSyncVariant()).toBe('teaser');

      labsFlags.syncEnabled = true;
      expect(resolveProSyncVariant()).toBe('teaser');
    });
  });
});
