/**
 * collaboration-tab: the variant switch behind the single `collaboration` tab
 * contribution. sign-in and consent render the locked placeholder; relogin and
 * connected render the live view; every variant change remounts the content.
 * The live view is stubbed; the variant is driven through the real stores.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';

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
import { seedVariant, resetSyncVariantState } from '../helpers/sync-variant-fixtures';

const NODE_ID = 'col-7';

describe('CollaborationTab', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    resetSyncVariantState();
  });

  afterEach(() => {
    cleanup();
    resetSyncVariantState();
    vi.restoreAllMocks();
  });

  it.each(['sign-in', 'consent'] as const)('%s renders the locked placeholder for the collection', (variant) => {
    seedVariant(variant);
    const { container, queryByTestId } = render(CollaborationTab, { props: { nodeId: NODE_ID } });

    const locked = container.querySelector('.collab-locked');
    expect(locked?.getAttribute('data-collection-id')).toBe(NODE_ID);
    expect(queryByTestId('stub-collaboration-view')).toBeNull();
  });

  it.each(['relogin', 'connected'] as const)('%s renders the live view for the collection', (variant) => {
    seedVariant(variant);
    const { container, getByTestId } = render(CollaborationTab, { props: { nodeId: NODE_ID } });

    expect(getByTestId('stub-collaboration-view').getAttribute('data-collection-id')).toBe(NODE_ID);
    expect(container.querySelector('.collab-locked')).toBeNull();
  });

  it('swaps the locked placeholder for the live view when sync is turned on', async () => {
    seedVariant('consent');
    const { container, findByTestId } = render(CollaborationTab, { props: { nodeId: NODE_ID } });
    expect(container.querySelector('.collab-locked')).not.toBeNull();

    seedVariant('connected');

    await findByTestId('stub-collaboration-view');
    expect(container.querySelector('.collab-locked')).toBeNull();
  });

  it('remounts the live view on a change from relogin to connected', async () => {
    seedVariant('relogin');
    const { getByTestId } = render(CollaborationTab, { props: { nodeId: NODE_ID } });
    const before = getByTestId('stub-collaboration-view');

    seedVariant('connected');

    await waitFor(() => expect(before.isConnected).toBe(false));
    const after = getByTestId('stub-collaboration-view');
    expect(after).not.toBe(before);
    expect(after.getAttribute('data-collection-id')).toBe(NODE_ID);
  });

  it('leaves the content mounted while the variant does not change', async () => {
    seedVariant('connected');
    const { getByTestId } = render(CollaborationTab, { props: { nodeId: NODE_ID } });
    const before = getByTestId('stub-collaboration-view');

    // A settings write that resolves to the same variant.
    seedVariant('connected');
    await tick();

    expect(getByTestId('stub-collaboration-view')).toBe(before);
  });
});
