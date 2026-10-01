/**
 * Module mocks for rendering `CollectionNodeViewer` without a daemon. Use from a
 * test file as:
 *
 *   vi.mock('$lib/services/collection-service', async () =>
 *     (await import('../helpers/collection-viewer-mocks')).collectionServiceModule()
 *   );
 */
import { vi } from 'vitest';
import type { CollectionNode } from '$lib/types';

export const COLLECTION_ID = 'col-1';
export const COLLECTION_NAME = 'Architecture';

function collectionNode(id: string): CollectionNode {
  return {
    lifecycleStatus: 'active',
    id,
    nodeType: 'collection',
    content: COLLECTION_NAME,
    createdAt: '2026-01-01T00:00:00.000Z',
    modifiedAt: '2026-01-01T00:00:00.000Z',
    version: 1,
    properties: {}
  };
}

export function collectionServiceModule() {
  return {
    collectionService: {
      getCollectionMembers: vi.fn(async () => []),
      getCollectionByName: vi.fn(async (id: string) => collectionNode(id)),
      addNodeToCollection: vi.fn(async () => undefined),
      removeNodeFromCollection: vi.fn(async () => undefined)
    }
  };
}

export function collectionAuthoringModule() {
  return {
    createNodeInCollection: vi.fn(async () => 'new-node'),
    searchAddableNodes: vi.fn(async () => [])
  };
}

export function navigationServiceModule() {
  return { getNavigationService: () => ({ focusOrOpenNode: vi.fn() }) };
}
