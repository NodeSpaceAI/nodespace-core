/**
 * Drives the sync variant machine through the real stores for component tests:
 * the Labs toggle, the daemon tier, and the active database's settings node.
 */
import type { Node } from '$lib/types';
import { proSync } from '$lib/stores/pro-sync.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import { SharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { DATABASE_SETTINGS_NODE_ID } from '$lib/constants/database-settings';

/** The variants that show a sync surface; every other state resolves to the teaser. */
type Variant = 'sign-in' | 'consent' | 'relogin' | 'connected';

/** The settings the daemon serializes, as FLAT properties on the singleton. */
const settingsFor = (on: boolean, authed: boolean) =>
  ({ sync_enabled: on, auth_status: authed ? 'connected' : 'local' });

/** The settings that resolve to each variant (Labs toggle on, syncing daemon). */
const SETTINGS_FOR_VARIANT: Record<Variant, Record<string, unknown>> = {
  'sign-in': settingsFor(false, false),
  consent: settingsFor(false, true),
  relogin: settingsFor(true, false),
  connected: settingsFor(true, true)
};

/** Seed the active database's settings singleton with the given props. */
export function seedSettings(props: Record<string, unknown>): void {
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

/** Turn the Labs sync toggle on or off; while it is off every variant resolves to `teaser`. */
export function setSyncToggle(on: boolean): void {
  labsFlags.syncEnabled = on;
}

/** Put the stores in the state that resolves to `variant`. Safe to call again to move to another variant. */
export function seedVariant(variant: Variant): void {
  setSyncToggle(true);
  proSync.tier = 'pro';
  seedSettings(SETTINGS_FOR_VARIANT[variant]);
}

/** Back to the default state, which resolves to the teaser variant. */
export function resetSyncVariantState(): void {
  Object.assign(proSync, { tier: 'unknown', userEmail: '' });
  setSyncToggle(false);
  SharedNodeStore.resetInstance();
}
