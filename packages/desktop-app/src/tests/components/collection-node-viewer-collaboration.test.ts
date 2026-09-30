/**
 * CollectionNodeViewer with the built-in Pro extension: the Collaboration tab is
 * one contribution, so a selected tab survives every variant change (for
 * example "Turn on sync", consent accepted, connected) instead of dropping
 * back to Contents.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, fireEvent, cleanup, waitFor } from '@testing-library/svelte';
import type { Node } from '$lib/types';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));
vi.mock('$lib/services/collection-service', async () =>
  (await import('../helpers/collection-viewer-mocks')).collectionServiceModule()
);
vi.mock('$lib/services/collection-authoring', async () =>
  (await import('../helpers/collection-viewer-mocks')).collectionAuthoringModule()
);
vi.mock('$lib/services/navigation-service', async () =>
  (await import('../helpers/collection-viewer-mocks')).navigationServiceModule()
);
vi.mock('$lib/components/collaboration/collaboration-view.svelte', async () => ({
  default: (await import('../fixtures/stub-collaboration-view.svelte')).default
}));

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

import CollectionNodeViewer from '$lib/components/viewers/collection-node-viewer.svelte';
import { proSync } from '$lib/stores/pro-sync.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import { SharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { DATABASE_SETTINGS_NODE_ID } from '$lib/constants/database-settings';
import { COLLECTION_ID, COLLECTION_NAME } from '../helpers/collection-viewer-mocks';

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

describe('CollectionNodeViewer Collaboration tab', () => {
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

  it('shows no tab strip in the community build', async () => {
    proSync.tier = 'community';
    const view = render(CollectionNodeViewer, { props: { nodeId: COLLECTION_ID } });
    await view.findByText(COLLECTION_NAME);

    expect(view.queryByRole('tablist')).toBeNull();
  });

  it('keeps the Collaboration tab selected from consent through to connected', async () => {
    seedSettings({ sync_enabled: false, auth_status: 'connected' });
    const { container, getByRole, findByRole, findByTestId, queryByRole } = render(
      CollectionNodeViewer,
      { props: { nodeId: COLLECTION_ID } }
    );

    await fireEvent.click(await findByRole('tab', { name: 'Collaboration' }));
    await waitFor(() => expect(container.querySelector('.collab-locked')).not.toBeNull());

    // Sync is turned on: same tab, now showing the live view.
    seedSettings({ sync_enabled: true, auth_status: 'connected' });

    await findByTestId('stub-collaboration-view');
    expect(container.querySelector('.collab-locked')).toBeNull();
    expect(getByRole('tab', { name: 'Collaboration' }).getAttribute('aria-selected')).toBe('true');
    expect(getByRole('tab', { name: 'Contents' }).getAttribute('aria-selected')).toBe('false');
    expect(queryByRole('button', { name: /new node/i })).toBeNull();
  });

  it('drops back to Contents when sync is switched off in Labs', async () => {
    seedSettings({ sync_enabled: true, auth_status: 'connected' });
    const { getByRole, findByRole, findByTestId, queryByRole, queryByTestId } = render(
      CollectionNodeViewer,
      { props: { nodeId: COLLECTION_ID } }
    );
    await fireEvent.click(await findByRole('tab', { name: 'Collaboration' }));
    await findByTestId('stub-collaboration-view');

    labsFlags.syncEnabled = false;

    await waitFor(() => expect(queryByTestId('stub-collaboration-view')).toBeNull());
    expect(queryByRole('tablist')).toBeNull();
    expect(getByRole('button', { name: /new node/i })).toBeTruthy();
  });
});
