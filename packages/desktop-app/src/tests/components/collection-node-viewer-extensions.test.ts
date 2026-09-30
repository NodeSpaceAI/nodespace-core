/**
 * CollectionNodeViewer: registry-contributed tabs, keyed by contribution key.
 * Uses the fixture extension, so it exercises the generic host only.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
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

import CollectionNodeViewer from '$lib/components/viewers/collection-node-viewer.svelte';
import { uiExtensionRegistry } from '$lib/plugins/ui-extensions';
import {
  TEST_EXTENSION_ID,
  createTestExtension,
  resetTestExtension,
  testExtensionFlags
} from '../fixtures/test-extension';
import { COLLECTION_ID, COLLECTION_NAME } from '../helpers/collection-viewer-mocks';

async function renderViewer() {
  const view = render(CollectionNodeViewer, { props: { nodeId: COLLECTION_ID } });
  // The Contents view is showing once the collection has loaded.
  await view.findByRole('button', { name: /new node/i });
  await view.findByText(COLLECTION_NAME);
  return view;
}

describe('CollectionNodeViewer extension tabs', () => {
  afterEach(() => {
    cleanup();
    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    resetTestExtension();
  });

  it('shows no tab strip while no tab is contributed', async () => {
    uiExtensionRegistry.register(createTestExtension());
    const { queryByRole } = await renderViewer();

    expect(queryByRole('tablist')).toBeNull();
  });

  it('shows Contents plus the contributed tab once it is shown', async () => {
    uiExtensionRegistry.register(createTestExtension());
    const { findByRole, getAllByRole } = await renderViewer();

    testExtensionFlags.tab = true;
    await findByRole('tablist');

    const tabs = getAllByRole('tab');
    expect(tabs.map((t) => t.textContent?.trim())).toEqual(['Contents', 'Test tab']);
    expect(tabs[0].getAttribute('aria-selected')).toBe('true');
    expect(tabs[1].getAttribute('aria-selected')).toBe('false');
  });

  it('mounts the contributed component with the collection id when its tab is clicked', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.tab = true;
    const { getByRole, findByTestId, queryByRole } = await renderViewer();

    await fireEvent.click(getByRole('tab', { name: 'Test tab' }));

    const mounted = await findByTestId('test-viewer-tab');
    expect(mounted.getAttribute('data-node-id')).toBe(COLLECTION_ID);
    // The tab replaces the Contents view rather than sitting beside it.
    expect(queryByRole('button', { name: /new node/i })).toBeNull();
    expect(getByRole('tab', { name: 'Test tab' }).getAttribute('aria-selected')).toBe('true');
    expect(getByRole('tab', { name: 'Contents' }).getAttribute('aria-selected')).toBe('false');
  });

  it('falls back to Contents when the selected tab is hidden', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.tab = true;
    const { getByRole, findByTestId, queryByTestId, queryByRole } = await renderViewer();
    await fireEvent.click(getByRole('tab', { name: 'Test tab' }));
    await findByTestId('test-viewer-tab');

    testExtensionFlags.tab = false;

    await waitFor(() => expect(queryByTestId('test-viewer-tab')).toBeNull());
    expect(getByRole('button', { name: /new node/i })).toBeTruthy();
    expect(queryByRole('tablist')).toBeNull();
  });

  it('keeps the strip and marks Contents selected when another tab remains after the selected one is hidden', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.tab = true;
    testExtensionFlags.tabSecondary = true;
    const { getByRole, findByTestId, queryByTestId } = await renderViewer();
    await fireEvent.click(getByRole('tab', { name: 'Test tab' }));
    await findByTestId('test-viewer-tab');

    testExtensionFlags.tab = false;

    await waitFor(() => expect(queryByTestId('test-viewer-tab')).toBeNull());
    expect(getByRole('button', { name: /new node/i })).toBeTruthy();
    expect(getByRole('tab', { name: 'Contents' }).getAttribute('aria-selected')).toBe('true');
    expect(getByRole('tab', { name: 'Second test tab' }).getAttribute('aria-selected')).toBe(
      'false'
    );
  });

  it('keys each tab by its contribution key, so equal contribution ids in two extensions stay apart', async () => {
    const otherId = 'test-extension-other';
    uiExtensionRegistry.register(createTestExtension());
    // Same contribution id (`tab`) as the fixture's tab, in a different extension.
    uiExtensionRegistry.register(
      createTestExtension({
        id: otherId,
        chrome: [],
        viewerTabs: [
          {
            id: 'tab',
            nodeType: 'collection',
            label: 'Other extension tab',
            priority: -1,
            when: () => testExtensionFlags.tabSecondary,
            load: () => import('../fixtures/test-extension/test-viewer-tab.svelte')
          }
        ]
      })
    );
    try {
      testExtensionFlags.tab = true;
      testExtensionFlags.tabSecondary = true;
      const { getByRole, findByTestId } = await renderViewer();

      await fireEvent.click(getByRole('tab', { name: 'Other extension tab' }));
      await findByTestId('test-viewer-tab');

      expect(getByRole('tab', { name: 'Other extension tab' }).getAttribute('aria-selected')).toBe(
        'true'
      );
      expect(getByRole('tab', { name: 'Test tab' }).getAttribute('aria-selected')).toBe('false');
    } finally {
      uiExtensionRegistry.unregister(otherId);
    }
  });
});
