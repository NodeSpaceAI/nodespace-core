/**
 * collaboration-tab: the variant switch behind the single `collaboration` tab
 * contribution. sign-in and consent render the locked placeholder; relogin and
 * connected render the live view; every variant change remounts the content.
 * The live view is stubbed; the variant is driven through the real stores.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';
import type { Node } from '$lib/types';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));
vi.mock('$lib/components/collaboration/collaboration-view.svelte', async () => ({
  default: (await import('../fixtures/stub-collaboration-view.svelte')).default
}));

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

import CollaborationTab from '$lib/components/collaboration/collaboration-tab.svelte';
import { proSync } from '$lib/stores/pro-sync.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import { SharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { DATABASE_SETTINGS_NODE_ID } from '$lib/constants/database-settings';

/** Seed the active database's settings singleton (FLAT properties, as the daemon serializes them). */
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

const SIGN_IN = { sync_enabled: false, auth_status: 'local' };
const CONSENT = { sync_enabled: false, auth_status: 'connected' };
const RELOGIN = { sync_enabled: true, auth_status: 'local' };
const CONNECTED = { sync_enabled: true, auth_status: 'connected' };

const NODE_ID = 'col-7';

describe('CollaborationTab', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    SharedNodeStore.resetInstance();
    labsFlags.syncEnabled = true;
    proSync.tier = 'pro';
    proSync.userEmail = '';
  });

  afterEach(() => {
    cleanup();
    proSync.tier = 'unknown';
    proSync.userEmail = '';
    labsFlags.syncEnabled = false;
    SharedNodeStore.resetInstance();
    vi.restoreAllMocks();
  });

  it.each([
    ['sign-in', SIGN_IN],
    ['consent', CONSENT]
  ])('%s renders the locked placeholder for the collection', (_variant, settings) => {
    seedSettings(settings);
    const { container, queryByTestId } = render(CollaborationTab, { props: { nodeId: NODE_ID } });

    const locked = container.querySelector('.collab-locked');
    expect(locked?.getAttribute('data-collection-id')).toBe(NODE_ID);
    expect(queryByTestId('stub-collaboration-view')).toBeNull();
  });

  it.each([
    ['relogin', RELOGIN],
    ['connected', CONNECTED]
  ])('%s renders the live view for the collection', (_variant, settings) => {
    seedSettings(settings);
    const { container, getByTestId } = render(CollaborationTab, { props: { nodeId: NODE_ID } });

    expect(getByTestId('stub-collaboration-view').getAttribute('data-collection-id')).toBe(NODE_ID);
    expect(container.querySelector('.collab-locked')).toBeNull();
  });

  it('swaps the locked placeholder for the live view when sync is turned on', async () => {
    seedSettings(CONSENT);
    const { container, findByTestId } = render(CollaborationTab, { props: { nodeId: NODE_ID } });
    expect(container.querySelector('.collab-locked')).not.toBeNull();

    seedSettings(CONNECTED);

    await findByTestId('stub-collaboration-view');
    expect(container.querySelector('.collab-locked')).toBeNull();
  });

  it('remounts the live view on a change from relogin to connected', async () => {
    seedSettings(RELOGIN);
    const { getByTestId } = render(CollaborationTab, { props: { nodeId: NODE_ID } });
    const before = getByTestId('stub-collaboration-view');

    seedSettings(CONNECTED);

    await waitFor(() => expect(before.isConnected).toBe(false));
    const after = getByTestId('stub-collaboration-view');
    expect(after).not.toBe(before);
    expect(after.getAttribute('data-collection-id')).toBe(NODE_ID);
  });

  it('leaves the content mounted while the variant does not change', async () => {
    seedSettings(CONNECTED);
    const { getByTestId } = render(CollaborationTab, { props: { nodeId: NODE_ID } });
    const before = getByTestId('stub-collaboration-view');

    // A settings write that resolves to the same variant.
    seedSettings({ ...CONNECTED });
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(getByTestId('stub-collaboration-view')).toBe(before);
  });
});
