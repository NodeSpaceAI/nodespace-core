/**
 * QueryNodeViewer — materialize-then-remount race.
 *
 * Renaming the default, unsaved type view materializes a real
 * `nodeType: 'query'` node and reroutes the tab to it. pane-content.svelte remounts the viewer against the new
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
import type { Node, QueryNode, QueryNodeUpdate } from '$lib/types';

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
const mockUpdateQueryNode = vi.fn();

vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    getNode: (...args: unknown[]) => mockGetNode(...args),
    getSchema: (...args: unknown[]) => mockGetSchema(...args),
    queryNodes: (...args: unknown[]) => mockQueryNodes(...args),
    executeQuery: (...args: unknown[]) => mockExecuteQuery(...args),
    createNode: (...args: unknown[]) => mockCreateNode(...args),
    updateNode: (...args: unknown[]) => mockUpdateNode(...args),
    updateQueryNode: (...args: unknown[]) => mockUpdateQueryNode(...args)
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

/** A query node as the backend returns it: typed top-level fields. */
function materializedQueryNode(id: string): QueryNode & Node {
  return {
    id,
    nodeType: 'query',
    content: 'Untitled Query',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    targetType: SCHEMA_ID,
    filters: [],
    generatedBy: 'user',
    executionCount: 0,
    viewConfig: { lastView: 'kanban' },
    mentions: []
  };
}

describe('QueryNodeViewer — materialize race', () => {
  beforeEach(() => {
    localStorage.clear();
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

  const widgetRow = (id: string) => ({
    id,
    nodeType: SCHEMA_ID,
    content: 'Widget One',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: { status: 'open' },
    mentions: []
  });

  it('renaming the default view materializes a query node and reroutes the tab', async () => {
    mockGetNode.mockResolvedValue(null); // fresh default-view load: no query node exists yet
    mockCreateNode.mockImplementation(async (input: { id: string }) => ({
      id: input.id,
      placement: null
    }));

    let reroutedTo = '';
    const { getByLabelText } = render(QueryNodeViewer, {
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

    const title = getByLabelText('Query name') as HTMLInputElement;
    await fireEvent.focus(title);
    await fireEvent.input(title, { target: { value: 'My Board' } });
    await fireEvent.blur(title);

    await waitFor(() => expect(mockCreateNode).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(reroutedTo).toBeTruthy());
    expect(sharedNodeStore.getNode(reroutedTo)?.nodeType).toBe('query');

    // Created under the query schema's storage keys, with one target key
    // holding the schema's own type — never the schema default '*'.
    const { properties, content } = mockCreateNode.mock.calls[0][0] as {
      properties: Record<string, unknown>;
      content: string;
    };
    expect(content).toBe('My Board');
    expect(Object.keys(properties).sort()).toEqual([
      'filters',
      'generated_by',
      'target_type',
      'view_config'
    ]);
    expect(properties.target_type).toBe(SCHEMA_ID);
  });

  it('switching view or group-by on the default view creates no node, keeps the title "Default", and persists per type', async () => {
    mockGetNode.mockResolvedValue(null);
    mockQueryNodes.mockResolvedValue([widgetRow('w1')]);

    const { getByRole, getByLabelText, unmount } = render(QueryNodeViewer, {
      props: { nodeId: SCHEMA_ID, onNodeIdChange: () => {} }
    });
    await waitFor(() => expect(mockGetSchema).toHaveBeenCalledWith(SCHEMA_ID));
    await waitFor(() => expect(getByRole('button', { name: '+ New' })).toBeTruthy());

    await fireEvent.click(getByRole('button', { name: 'List' }));
    expect(getByRole('button', { name: 'List' }).getAttribute('aria-pressed')).toBe('true');
    await fireEvent.click(getByRole('button', { name: 'Kanban' }));
    expect(getByRole('button', { name: 'Kanban' }).getAttribute('aria-pressed')).toBe('true');
    expect((getByLabelText('Query name') as HTMLInputElement).value).toBe('Default');

    expect(mockCreateNode).not.toHaveBeenCalled();
    expect(mockUpdateQueryNode).not.toHaveBeenCalled();

    await fireEvent.change(getByLabelText('Group by'), { target: { value: 'status' } });
    expect(mockCreateNode).not.toHaveBeenCalled();

    // A fresh mount of the same type restores the choice.
    unmount();
    const remounted = render(QueryNodeViewer, {
      props: { nodeId: SCHEMA_ID, onNodeIdChange: () => {} }
    });
    await waitFor(() => {
      expect(remounted.getByRole('button', { name: 'Kanban' }).getAttribute('aria-pressed')).toBe(
        'true'
      );
    });
    await waitFor(() => {
      expect((remounted.getByLabelText('Group by') as HTMLSelectElement).value).toBe('status');
    });
    expect((remounted.getByLabelText('Query name') as HTMLInputElement).value).toBe('Default');
    expect(mockCreateNode).not.toHaveBeenCalled();
  });

  it('blurring the default title without changing it is not a rename', async () => {
    mockGetNode.mockResolvedValue(null);
    const { getByLabelText } = render(QueryNodeViewer, {
      props: { nodeId: SCHEMA_ID, onNodeIdChange: () => {} }
    });
    await waitFor(() => expect(mockGetSchema).toHaveBeenCalledWith(SCHEMA_ID));
    const title = getByLabelText('Query name');
    await fireEvent.focus(title);
    await fireEvent.blur(title);
    expect(mockCreateNode).not.toHaveBeenCalled();
  });

  it('shows no item-count pill and no Edit Query button', async () => {
    mockGetNode.mockResolvedValue(null);
    mockQueryNodes.mockResolvedValue([widgetRow('w1')]);
    const { queryByText, queryByRole, getByRole } = render(QueryNodeViewer, {
      props: { nodeId: SCHEMA_ID, onNodeIdChange: () => {} }
    });
    await waitFor(() => expect(getByRole('button', { name: '+ New' })).toBeTruthy());
    expect(queryByText(/^\d+ items?$/)).toBeNull();
    expect(queryByRole('button', { name: 'Edit Query' })).toBeNull();
  });

  describe('Kanban gating', () => {
    const noEnumSchema = (): SchemaNode => ({ ...schema(), fields: [] });

    it('disables the Kanban tab with an explanation when the type has no enum field', async () => {
      mockGetNode.mockResolvedValue(null);
      mockGetSchema.mockResolvedValue(noEnumSchema());
      const { getByRole, container } = render(QueryNodeViewer, {
        props: { nodeId: SCHEMA_ID, onNodeIdChange: () => {} }
      });
      await waitFor(() => expect(getByRole('button', { name: '+ New' })).toBeTruthy());
      const kanban = getByRole('button', { name: 'Kanban' }) as unknown as { disabled: boolean };
      expect(kanban.disabled).toBe(true);
      expect(container.querySelector('.view-tab-wrap')?.getAttribute('title')).toContain(
        'No properties to build a Kanban board from'
      );
    });

    it('enables the Kanban tab when an enum field exists', async () => {
      mockGetNode.mockResolvedValue(null);
      const { getByRole } = render(QueryNodeViewer, {
        props: { nodeId: SCHEMA_ID, onNodeIdChange: () => {} }
      });
      await waitFor(() => expect(getByRole('button', { name: '+ New' })).toBeTruthy());
      expect((getByRole('button', { name: 'Kanban' }) as unknown as { disabled: boolean }).disabled).toBe(false);
    });

    it('a saved query with lastView kanban on a type with no enum field opens in List', async () => {
      const savedId = 'saved-no-enum';
      mockGetSchema.mockResolvedValue(noEnumSchema());
      mockGetNode.mockResolvedValue({
        ...materializedQueryNode(savedId),
        content: 'Board',
        viewConfig: { lastView: 'kanban' }
      });
      const { getByRole } = render(QueryNodeViewer, {
        props: { nodeId: savedId, onNodeIdChange: () => {} }
      });
      await waitFor(() => expect(getByRole('button', { name: '+ New' })).toBeTruthy());
      expect(getByRole('button', { name: 'List' }).getAttribute('aria-pressed')).toBe('true');
      expect(getByRole('button', { name: 'Kanban' }).getAttribute('aria-pressed')).toBe('false');
    });
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

  it('does not disturb an already-saved query switching views (no regression)', async () => {
    const savedId = 'saved-query-1';
    const saved = {
      ...materializedQueryNode(savedId),
      content: 'My Board',
      viewConfig: { lastView: 'table' }
    };
    mockGetNode.mockResolvedValue(saved);
    mockUpdateQueryNode.mockImplementation(
      async (_id: string, _version: number, update: QueryNodeUpdate) => ({
        ...saved,
        version: 2,
        ...update
      })
    );

    const { getByRole } = render(QueryNodeViewer, {
      props: { nodeId: savedId, onNodeIdChange: () => {} }
    });

    await waitFor(() => expect(getByRole('button', { name: '+ New' })).toBeTruthy());
    expect(getByRole('button', { name: 'Table' }).getAttribute('aria-pressed')).toBe('true');

    await fireEvent.click(getByRole('button', { name: 'Kanban' }));

    // Saved-mode view changes persist onto the existing node — no create —
    // through the typed update, never the properties bag.
    await waitFor(() => {
      expect(getByRole('button', { name: 'Kanban' }).getAttribute('aria-pressed')).toBe('true');
    });
    expect(mockCreateNode).not.toHaveBeenCalled();
    await waitFor(() => expect(mockUpdateQueryNode).toHaveBeenCalledTimes(1));
    expect(mockUpdateQueryNode).toHaveBeenCalledWith(savedId, 1, {
      viewConfig: { lastView: 'kanban' }
    });
    expect(mockUpdateNode).not.toHaveBeenCalled();
  });
});
