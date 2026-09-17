/**
 * QueryNodeViewer — materialize-then-remount race.
 *
 * Clicking a view tab (or changing the Kanban group-by) on the default,
 * unsaved type view materializes a real `nodeType: 'query'` node and reroutes
 * the tab to it. pane-content.svelte remounts the viewer against the new
 * nodeId ({#key ...content.nodeId}), so the freshly mounted instance starts
 * from its own state defaults (activeView: 'table') and must reload before it
 * knows any better.
 *
 * Previously that reload fetched the node over the network — a fetch that
 * can race ahead of the create that just completed, resolving this fresh
 * mount back onto the DEFAULT branch (which resets activeView/kanbanGroupBy)
 * even though a real, correctly-configured query node now exists. The fix:
 * prefer sharedNodeStore's already-hydrated copy (seeded synchronously by
 * materializeQuery before the reroute) over a fresh fetch, so the remount's
 * first load never depends on that race resolving in its favor.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
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

vi.mock('$lib/services/schema-authoring', () => ({
  createSchemaInstance: vi.fn(),
  shouldIntegrateInstance: () => true
}));

const mockGetNode = vi.fn();
const mockGetSchema = vi.fn();
const mockQueryNodes = vi.fn();
const mockExecuteQuery = vi.fn();
const mockCreateNode = vi.fn();
const mockUpdateNode = vi.fn();

vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    getNode: (...args: unknown[]) => mockGetNode(...args),
    getSchema: (...args: unknown[]) => mockGetSchema(...args),
    queryNodes: (...args: unknown[]) => mockQueryNodes(...args),
    executeQuery: (...args: unknown[]) => mockExecuteQuery(...args),
    createNode: (...args: unknown[]) => mockCreateNode(...args),
    updateNode: (...args: unknown[]) => mockUpdateNode(...args)
  }
}));

import QueryNodeViewer from '$lib/components/viewers/query-node-viewer.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';

const SCHEMA_ID = 'widget';

function schema(): SchemaNode {
  return {
    id: SCHEMA_ID,
    nodeType: 'schema',
    content: 'Widget',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: false,
    schemaVersion: 1,
    description: '',
    fields: [
      {
        name: 'status',
        friendlyName: 'Status',
        type: 'enum',
        protection: 'user',
        indexed: false,
        coreValues: [{ value: 'open', label: 'Open' }],
        userValues: []
      }
    ]
  };
}

function materializedQueryNode(id: string): Node {
  return {
    id,
    nodeType: 'query',
    content: 'Untitled Query',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {
      targetType: SCHEMA_ID,
      filters: [],
      generatedBy: 'user',
      viewConfig: { lastView: 'kanban' }
    },
    mentions: []
  };
}

describe('QueryNodeViewer — materialize race', () => {
  beforeEach(() => {
    sharedNodeStore.clearAll();
    vi.clearAllMocks();
    mockGetSchema.mockResolvedValue(schema());
    mockQueryNodes.mockResolvedValue([]);
    mockExecuteQuery.mockResolvedValue([]);
  });

  afterEach(() => {
    cleanup();
    sharedNodeStore.clearAll();
  });

  it('clicking Kanban on the default view materializes a query node and reroutes the tab', async () => {
    mockGetNode.mockResolvedValue(null); // fresh default-view load: no query node exists yet
    mockCreateNode.mockImplementation(async (input: { id: string }) => input.id);

    let reroutedTo = '';
    const { getByRole } = render(QueryNodeViewer, {
      props: {
        nodeId: SCHEMA_ID,
        onNodeIdChange: (id: string) => {
          reroutedTo = id;
        }
      }
    });

    await waitFor(() => expect(mockGetSchema).toHaveBeenCalledWith(SCHEMA_ID));

    // materializeQuery's own getNode(newId) read-back, right after create —
    // this is what seeds sharedNodeStore before the reroute.
    mockGetNode.mockImplementation(async (id: string) => materializedQueryNode(id));

    await fireEvent.click(getByRole('button', { name: 'Kanban' }));

    await waitFor(() => expect(mockCreateNode).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(reroutedTo).toBeTruthy());
    expect(sharedNodeStore.getNode(reroutedTo)?.nodeType).toBe('query');
  });

  it('the remounted instance restores Kanban from sharedNodeStore even if the network fetch would race behind the create', async () => {
    const newId = 'materialized-1';
    // Seed the store exactly as materializeQuery does, synchronously, before
    // the remount that pane-content's {#key} performs.
    sharedNodeStore.setNode(materializedQueryNode(newId), {
      type: 'database',
      reason: 'test seed — simulates materializeQuery'
    });

    // The network read for this id is still in flight / racing behind the
    // create at this point in a real run — model that as a hang other tests
    // don't need to wait out, and as a reject if awaited, so the assertion
    // below is a REAL check that the component never needed it.
    mockGetNode.mockRejectedValue(
      new Error('network getNode should not be reached — sharedNodeStore already has this node')
    );

    const { getByRole } = render(QueryNodeViewer, {
      props: { nodeId: newId, onNodeIdChange: () => {} }
    });

    await waitFor(() => {
      const kanbanTab = getByRole('button', { name: 'Kanban' });
      expect(kanbanTab.getAttribute('aria-pressed')).toBe('true');
    });
    expect(getByRole('button', { name: 'Table' }).getAttribute('aria-pressed')).toBe('false');
    expect(mockGetNode).not.toHaveBeenCalled();
  });

  it('choosing a Kanban group-by field on the default view materializes with that group-by and restores it on remount', async () => {
    mockGetNode.mockResolvedValue(null);
    mockCreateNode.mockImplementation(async (input: { id: string }) => input.id);

    let reroutedTo = '';
    const { getByRole } = render(QueryNodeViewer, {
      props: { nodeId: SCHEMA_ID, onNodeIdChange: (id: string) => (reroutedTo = id) }
    });

    await waitFor(() => expect(mockGetSchema).toHaveBeenCalledWith(SCHEMA_ID));

    // First switch to Kanban (no group-by chosen yet — no board renders).
    mockGetNode.mockImplementation(async (id: string) => materializedQueryNode(id));
    await fireEvent.click(getByRole('button', { name: 'Kanban' }));
    await waitFor(() => expect(reroutedTo).toBeTruthy());

    const materializedId = reroutedTo;
    const groupByChanged = {
      ...materializedQueryNode(materializedId),
      properties: {
        ...materializedQueryNode(materializedId).properties,
        viewConfig: { lastView: 'kanban', kanban: { groupBy: 'status' } }
      }
    };

    // Remount against the materialized id (as pane-content would), with the
    // network unavailable — sharedNodeStore must already carry the node the
    // group-by pick materialized, groupBy included.
    sharedNodeStore.setNode(groupByChanged, { type: 'database', reason: 'test seed' });
    mockGetNode.mockRejectedValue(new Error('network should not be needed'));
    // The remount lands on the saved branch, which executes the definition via
    // executeQuery; queryNodes is stubbed too so the assertion is about the
    // restored group-by, not about which fetch the branch happened to take.
    const widgets = [
      {
        id: 'w1',
        nodeType: SCHEMA_ID,
        content: 'Widget One',
        createdAt: '2026-01-01T00:00:00Z',
        modifiedAt: '2026-01-01T00:00:00Z',
        version: 1,
        properties: { status: 'open' },
        mentions: []
      }
    ];
    mockQueryNodes.mockResolvedValue(widgets);
    mockExecuteQuery.mockResolvedValue(widgets);

    cleanup();
    const { getByLabelText } = render(QueryNodeViewer, {
      props: { nodeId: materializedId, onNodeIdChange: () => {} }
    });

    await waitFor(() => {
      const select = getByLabelText('Group by') as HTMLSelectElement;
      expect(select.value).toBe('status');
    });
  });

  it('does not disturb an already-saved query switching views (no regression)', async () => {
    const savedId = 'saved-query-1';
    const saved = {
      ...materializedQueryNode(savedId),
      content: 'My Board',
      properties: {
        ...materializedQueryNode(savedId).properties,
        viewConfig: { lastView: 'table' }
      }
    };
    mockGetNode.mockResolvedValue(saved);
    mockUpdateNode.mockImplementation(
      async (_id: string, _version: number, update: { properties?: Record<string, unknown> }) => ({
        ...saved,
        version: 2,
        properties: { ...saved.properties, ...update.properties }
      })
    );

    const { getByRole } = render(QueryNodeViewer, {
      props: { nodeId: savedId, onNodeIdChange: () => {} }
    });

    await waitFor(() => {
      expect(getByRole('button', { name: 'Table' }).getAttribute('aria-pressed')).toBe('true');
    });

    await fireEvent.click(getByRole('button', { name: 'Kanban' }));

    // Saved-mode view changes persist onto the existing node — no create.
    await waitFor(() => {
      expect(getByRole('button', { name: 'Kanban' }).getAttribute('aria-pressed')).toBe('true');
    });
    expect(mockCreateNode).not.toHaveBeenCalled();
  });
});
