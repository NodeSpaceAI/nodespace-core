/**
 * CollectionNodeViewer: the header shows the collection's description, a typed
 * field at the top level of the collection node.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';

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
import { collectionService } from '$lib/services/collection-service';
import type { CollectionNode } from '$lib/types';
import { COLLECTION_ID, COLLECTION_NAME } from '../helpers/collection-viewer-mocks';

function collection(description?: string): CollectionNode {
  return {
    lifecycleStatus: 'active',
    id: COLLECTION_ID,
    nodeType: 'collection',
    content: COLLECTION_NAME,
    createdAt: '2026-01-01T00:00:00.000Z',
    modifiedAt: '2026-01-01T00:00:00.000Z',
    version: 1,
    properties: {},
    ...(description ? { description } : {})
  };
}

describe('CollectionNodeViewer description', () => {
  afterEach(() => {
    cleanup();
  });

  it('shows the typed description under the name', async () => {
    vi.mocked(collectionService.getCollectionByName).mockResolvedValueOnce(
      collection('Accounts we bill')
    );
    const { findByText } = render(CollectionNodeViewer, { props: { nodeId: COLLECTION_ID } });

    await findByText(COLLECTION_NAME);
    expect((await findByText('Accounts we bill')).className).toContain('collection-description');
  });

  it('shows no description line when the collection has none', async () => {
    vi.mocked(collectionService.getCollectionByName).mockResolvedValueOnce(collection());
    const { findByText, container } = render(CollectionNodeViewer, {
      props: { nodeId: COLLECTION_ID }
    });

    await findByText(COLLECTION_NAME);
    expect(container.querySelector('.collection-description')).toBeNull();
  });
});
