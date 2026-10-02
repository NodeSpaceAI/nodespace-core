/**
 * QueryNodeViewer "+ New" on a type with a required field that has no default.
 *
 * The instance opens as an unsaved placeholder — nothing is written to the
 * backend and it is not a query result — and the store creates it once the
 * required field is filled.
 */
import { describe, it, expect, beforeEach, afterEach, vi, type MockInstance } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import type { SchemaNode } from '$lib/types/schema-node';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

const navigateToNodeInOtherPane = vi.fn();
vi.mock('$lib/services/navigation-service', () => ({
  getNavigationService: () => ({
    focusNodeTab: () => false,
    navigateToNodeInOtherPane: (...args: unknown[]) => navigateToNodeInOtherPane(...args)
  })
}));

import QueryNodeViewer from '$lib/components/viewers/query-node-viewer.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';

const TYPE = 'gadget';

function schema(fields: SchemaNode['fields']): SchemaNode {
  return {
    lifecycleStatus: 'active' as const,
    properties: {},
    id: TYPE,
    nodeType: 'schema',
    content: 'Gadget',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: false,
    schemaVersion: 1,
    relationships: [],
    fields
  };
}

const requiredDescription = {
  name: 'description',
  friendlyName: 'Description',
  type: 'text',
  protection: 'user',
  indexed: false,
  required: true
} as SchemaNode['fields'][number];

const optionalNote = {
  name: 'note',
  friendlyName: 'Note',
  type: 'text',
  protection: 'user',
  indexed: false
} as SchemaNode['fields'][number];

function openedNodeId(): string {
  return navigateToNodeInOtherPane.mock.calls[0][0] as string;
}

describe('QueryNodeViewer — "+ New" with required fields', () => {
  // The store persists through the real adapter module, so the backend is
  // observed with spies on it rather than a module mock.
  let mockGetNode: MockInstance<typeof backendAdapter.getNode>;
  let mockGetSchema: MockInstance<typeof backendAdapter.getSchema>;
  let mockCreateNode: MockInstance<typeof backendAdapter.createNode>;

  beforeEach(() => {
    localStorage.clear();
    sharedNodeStore.clearAll();
    navigateToNodeInOtherPane.mockClear();
    mockGetNode = vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(null);
    mockGetSchema = vi.spyOn(backendAdapter, 'getSchema');
    vi.spyOn(backendAdapter, 'queryNodes').mockResolvedValue([]);
    vi.spyOn(backendAdapter, 'executeQuery').mockResolvedValue([]);
    mockCreateNode = vi
      .spyOn(backendAdapter, 'createNode')
      .mockImplementation(async (input) => ({ id: (input as { id: string }).id, placement: null }));
  });

  afterEach(() => {
    cleanup();
    sharedNodeStore.clearAll();
    vi.restoreAllMocks();
  });

  it('opens an unsaved placeholder without writing to the backend or listing it', async () => {
    mockGetSchema.mockResolvedValue(schema([requiredDescription]));
    const { getByRole, container } = render(QueryNodeViewer, {
      props: { nodeId: TYPE, onNodeIdChange: () => {} }
    });
    await waitFor(() => expect(getByRole('button', { name: '+ New' })).toBeTruthy());

    await fireEvent.click(getByRole('button', { name: '+ New' }));

    await waitFor(() => expect(navigateToNodeInOtherPane).toHaveBeenCalledTimes(1));
    const id = openedNodeId();
    expect(sharedNodeStore.isUnsavedPlaceholder(id)).toBe(true);
    expect(mockCreateNode).not.toHaveBeenCalled();
    // Not a result of the type's view until it is saved.
    expect(container.querySelectorAll('tbody tr').length).toBe(0);
    expect(container.querySelector('.create-error')).toBeNull();
  });

  it('saves the instance and lists it once the required field is filled', async () => {
    mockGetSchema.mockResolvedValue(schema([requiredDescription]));
    const { getByRole, container } = render(QueryNodeViewer, {
      props: { nodeId: TYPE, onNodeIdChange: () => {} }
    });
    await waitFor(() => expect(getByRole('button', { name: '+ New' })).toBeTruthy());
    await fireEvent.click(getByRole('button', { name: '+ New' }));
    await waitFor(() => expect(navigateToNodeInOtherPane).toHaveBeenCalledTimes(1));
    const id = openedNodeId();

    sharedNodeStore.updateNode(
      id,
      { properties: { description: 'Does things' } },
      { type: 'viewer', viewerId: 'test' }
    );

    await waitFor(() => expect(mockCreateNode).toHaveBeenCalledTimes(1));
    expect(mockCreateNode.mock.calls[0][0]).toEqual(
      expect.objectContaining({ id, nodeType: TYPE, properties: { description: 'Does things' } })
    );
    expect(sharedNodeStore.isUnsavedPlaceholder(id)).toBe(false);
    await waitFor(() => expect(container.querySelectorAll('tbody tr').length).toBe(1));
  });

  it('still creates immediately when the type has no required field without a default', async () => {
    mockGetSchema.mockResolvedValue(schema([optionalNote]));
    mockGetNode.mockImplementation(async (id: string) =>
      id === TYPE
        ? null
        : ({
            lifecycleStatus: 'active',
            id,
            nodeType: TYPE,
            content: '',
            createdAt: '2026-01-01T00:00:00Z',
            modifiedAt: '2026-01-01T00:00:00Z',
            version: 1,
            properties: {},
            mentions: []
          } as Node)
    );
    const { getByRole } = render(QueryNodeViewer, {
      props: { nodeId: TYPE, onNodeIdChange: () => {} }
    });
    await waitFor(() => expect(getByRole('button', { name: '+ New' })).toBeTruthy());

    await fireEvent.click(getByRole('button', { name: '+ New' }));

    await waitFor(() => expect(mockCreateNode).toHaveBeenCalledTimes(1));
    const id = (mockCreateNode.mock.calls[0][0] as { id: string }).id;
    expect(sharedNodeStore.isUnsavedPlaceholder(id)).toBe(false);
  });
});
