/**
 * CollectionNodeViewer with the built-in sync extension: the Collaboration tab is
 * one contribution, so a selected tab survives every variant change (for
 * example "Turn on sync", consent accepted, connected) instead of dropping
 * back to Contents.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, fireEvent, cleanup, waitFor } from '@testing-library/svelte';

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
import { seedVariant, setSyncToggle, resetSyncVariantState } from '../helpers/sync-variant-fixtures';
import { COLLECTION_ID, COLLECTION_NAME } from '../helpers/collection-viewer-mocks';

describe('CollectionNodeViewer Collaboration tab', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    resetSyncVariantState();
  });

  afterEach(() => {
    cleanup();
    resetSyncVariantState();
    vi.restoreAllMocks();
  });

  it('shows no tab strip while the Labs toggle is off, even for a synced database', async () => {
    seedVariant('connected');
    setSyncToggle(false);
    const view = render(CollectionNodeViewer, { props: { nodeId: COLLECTION_ID } });
    await view.findByText(COLLECTION_NAME);

    expect(view.queryByRole('tablist')).toBeNull();
  });

  it('keeps the Collaboration tab selected from consent through to connected', async () => {
    seedVariant('consent');
    const { container, getByRole, findByRole, findByTestId, queryByRole } = render(
      CollectionNodeViewer,
      { props: { nodeId: COLLECTION_ID } }
    );

    await fireEvent.click(await findByRole('tab', { name: 'Collaboration' }));
    await waitFor(() => expect(container.querySelector('.collab-locked')).not.toBeNull());

    // Sync is turned on: same tab, now showing the live view.
    seedVariant('connected');

    await findByTestId('stub-collaboration-view');
    expect(container.querySelector('.collab-locked')).toBeNull();
    expect(getByRole('tab', { name: 'Collaboration' }).getAttribute('aria-selected')).toBe('true');
    expect(getByRole('tab', { name: 'Contents' }).getAttribute('aria-selected')).toBe('false');
    expect(queryByRole('button', { name: /new node/i })).toBeNull();
  });

  it('drops back to Contents when sync is switched off in Labs', async () => {
    seedVariant('connected');
    const { getByRole, findByRole, findByTestId, queryByRole, queryByTestId } = render(
      CollectionNodeViewer,
      { props: { nodeId: COLLECTION_ID } }
    );
    await fireEvent.click(await findByRole('tab', { name: 'Collaboration' }));
    await findByTestId('stub-collaboration-view');

    setSyncToggle(false);

    await waitFor(() => expect(queryByTestId('stub-collaboration-view')).toBeNull());
    expect(queryByRole('tablist')).toBeNull();
    expect(getByRole('button', { name: /new node/i })).toBeTruthy();
  });
});
