/**
 * Drives the sync variant machine through the real stores for component tests:
 * the Labs toggle, the daemon tier, and the active database's settings node.
 */
import type { Node } from '$lib/types';
import { proSync } from '$lib/stores/pro-sync.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import { SharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { DATABASE_SETTINGS_NODE_ID } from '$lib/constants/database-settings';
import type { ProSyncVariant } from '$lib/plugins/pro-sync-variant.svelte';

/** The settings the daemon serializes, as FLAT properties on the singleton. */
export type SettingsSeed = {
  sync_enabled?: boolean;
  auth_status?: string;
};

/** The settings that resolve to each variant that shows a sync surface (Labs toggle on, syncing daemon). */
const SETTINGS_FOR_VARIANT: Record<Exclude<ProSyncVariant, 'teaser'>, SettingsSeed> = {
  'sign-in': { sync_enabled: false, auth_status: 'local' },
  consent: { sync_enabled: false, auth_status: 'connected' },
  relogin: { sync_enabled: true, auth_status: 'local' },
  connected: { sync_enabled: true, auth_status: 'connected' }
};

/** Seed the active database's settings singleton with the given props. */
export function seedSettings(props: SettingsSeed): void {
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

/** Put the stores in the state that resolves to `variant`. Safe to call again to move to another variant. */
export function seedVariant(variant: Exclude<ProSyncVariant, 'teaser'>): void {
  labsFlags.syncEnabled = true;
  proSync.tier = 'pro';
  seedSettings(SETTINGS_FOR_VARIANT[variant]);
}

/** Back to the default state, which resolves to the teaser variant. */
export function resetProSyncState(): void {
  proSync.tier = 'unknown';
  proSync.userEmail = '';
  labsFlags.syncEnabled = false;
  SharedNodeStore.resetInstance();
}
