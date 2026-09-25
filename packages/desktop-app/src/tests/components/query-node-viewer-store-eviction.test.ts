/**
 * QueryNodeViewer — database switch must not live-append evicted nodes.
 *
 * The viewer's `subscribeAll` handler folds every node it is told about into
 * the open view when it matches the query. `sharedNodeStore.clearAll()` (the
 * database hot-swap) reports every evicted node to wildcard subscribers, so
 * without the viewer skipping `isStoreEviction` sources, a matching node from
 * the previous database that this view had not loaded would be appended into
 * it mid-switch.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';
import type { SchemaNode } from '$lib/types/schema-node';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

vi.mock('$lib/services/navigation-service', () => ({
  getNavigationService: () => ({
    focusNodeTab: () => false,
    navigateToNodeInOtherPane: () => {}
  })
}));

const mockGetNode = vi.fn();
const mockGetSchema = vi.fn();
const mockQueryNodes = vi.fn();
const mockExecuteQuery = vi.fn();

vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    getNode: (...args: unknown[]) => mockGetNode(...args),
    getSchema: (...args: unknown[]) => mockGetSchema(...args),
    queryNodes: (...args: unknown[]) => mockQueryNodes(...args),
    executeQuery: (...args: unknown[]) => mockExecuteQuery(...args)
  }
}));

import QueryNodeViewer from '$lib/components/viewers/query-node-viewer.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';

const SCHEMA_ID = 'widget';
const QUERY_ID = 'saved-query-1';
const seed = { type: 'database', reason: 'test seed' } as const;

const schema: SchemaNode = {
  id: SCHEMA_ID,
  nodeType: 'schema',
  content: 'Widget',
  createdAt: '2026-01-01T00:00:00Z',
  modifiedAt: '2026-01-01T00:00:00Z',
  version: 1,
  isCore: false,
  schemaVersion: 1,
  description: '',
  fields: []
};

const queryNode: Node = {
  id: QUERY_ID,
  nodeType: 'query',
  content: 'Widgets',
  createdAt: '2026-01-01T00:00:00Z',
  modifiedAt: '2026-01-01T00:00:00Z',
  version: 1,
  properties: {
    targetType: SCHEMA_ID,
    filters: [],
    generatedBy: 'user',
    viewConfig: { lastView: 'table' }
  },
  mentions: []
};

function widget(id: string): Node {
  return {
    id,
    nodeType: SCHEMA_ID,
    content: id,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    mentions: []
  };
}

describe('QueryNodeViewer — store eviction', () => {
  beforeEach(() => {
    sharedNodeStore.clearAll();
    vi.clearAllMocks();
    mockGetSchema.mockResolvedValue(schema);
    mockGetNode.mockResolvedValue(queryNode);
    mockQueryNodes.mockResolvedValue([widget('w1')]);
    mockExecuteQuery.mockResolvedValue([widget('w1')]);
  });

  afterEach(() => {
    cleanup();
    sharedNodeStore.clearAll();
  });

  it('does not append nodes evicted by clearAll() into the open view', async () => {
    // Cached in the store (previous database) but not part of this view's results.
    sharedNodeStore.setNode(widget('w-unloaded'), seed);

    const { getByText } = render(QueryNodeViewer, {
      props: { nodeId: QUERY_ID, onNodeIdChange: () => {} }
    });
    await waitFor(() => expect(getByText('1 item')).toBeTruthy());

    // Positive control: a genuinely new matching node IS live-appended, so the
    // assertion below is not passing merely because the handler never fires.
    sharedNodeStore.setNode(widget('w-created'), seed);
    await waitFor(() => expect(getByText('2 items')).toBeTruthy());

    sharedNodeStore.clearAll();
    await Promise.resolve();

    expect(getByText('2 items')).toBeTruthy();
  });
});
