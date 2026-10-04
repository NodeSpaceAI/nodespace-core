/**
 * navigation-sidebar.svelte — saved queries listed under their node type.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';

const focusOrOpenNode = vi.fn();
vi.mock('$lib/services/navigation-service', () => ({
  getNavigationService: () => ({ focusOrOpenNode })
}));

// The sidebar's onMount loads schemas and saved queries through the backend
// adapter; serve them from mutable fixtures so the real load path is exercised.
const backend = vi.hoisted(() => ({
  schemas: [] as unknown[],
  queries: [] as unknown[]
}));
vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    getAllSchemas: async () => backend.schemas,
    getNode: async () => null,
    queryNodes: async (query: { nodeType?: string }) =>
      query.nodeType === 'query' ? backend.queries : []
  }
}));

import NavigationSidebar from '$lib/components/layout/navigation-sidebar.svelte';
import { schemasStore } from '$lib/stores/schemas.svelte';
import { savedQueriesData } from '$lib/stores/saved-queries.svelte';
import { layoutStore } from '$lib/stores/layout.svelte';
import type { SchemaNode } from '$lib/types/schema-node';

function makeSchema(id: string, content: string, isCore = false): SchemaNode {
  return { id, content, isCore } as unknown as SchemaNode;
}

/** Set the queries the backend holds; the sidebar's mount-time load reads them. */
function setBackendQueries(items: Array<[string, string, string]>) {
  backend.queries = items.map(([id, name, targetType]) => ({
    id,
    nodeType: 'query',
    content: name,
    targetType,
    properties: {}
  }));
}

/** Change the backend queries and reload through the store, as a domain-event refresh does. */
async function seedQueries(items: Array<[string, string, string]>) {
  setBackendQueries(items);
  await savedQueriesData.loadSavedQueries();
}

function expandNodeTypes() {
  layoutStore.state = {
    ...layoutStore.state,
    sidebarCollapsed: false,
    nodeTypesExpanded: true,
    collapsedTypeViews: []
  };
}

/** Text of every row in the Node Types list, in DOM order. */
function rows(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll('.schema-type-list .schema-type-item')).map(
    (el) => el.textContent?.trim() ?? ''
  );
}

describe('NavigationSidebar — saved queries under Node Types', () => {
  beforeEach(() => {
    focusOrOpenNode.mockClear();
    backend.schemas = [makeSchema('spec', 'Spec'), makeSchema('plan', 'Plan')];
    backend.queries = [];
    savedQueriesData.reset();
    expandNodeTypes();
  });

  afterEach(() => {
    cleanup();
    savedQueriesData.reset();
    backend.schemas = [];
    schemasStore.schemas = [];
  });

  it('lists each query directly under the type it targets, ordered by name', async () => {
    setBackendQueries([
      ['q-plan', 'Plans by Status', 'plan'],
      ['q-spec-b', 'Specs by Status', 'spec'],
      ['q-spec-a', 'Open specs', 'spec']
    ]);

    const { container } = render(NavigationSidebar);

    await waitFor(() => expect(rows(container)).toHaveLength(5));
    expect(rows(container)).toEqual([
      'Spec',
      'Open specs',
      'Specs by Status',
      'Plan',
      'Plans by Status'
    ]);
  });

  it('opens the query when a saved query is clicked, and the type Default view when the type is clicked', async () => {
    setBackendQueries([['q-spec', 'Specs by Status', 'spec']]);
    const { getByText, findByText } = render(NavigationSidebar);

    await fireEvent.click(await findByText('Specs by Status'));
    expect(focusOrOpenNode).toHaveBeenLastCalledWith('q-spec', { nodeType: 'query' });

    await fireEvent.click(getByText('Spec'));
    expect(focusOrOpenNode).toHaveBeenLastCalledWith('spec', { nodeType: 'query' });
  });

  it('shows a type with no queries as a plain row', async () => {
    setBackendQueries([['q-spec', 'Specs by Status', 'spec']]);
    const { container } = render(NavigationSidebar);

    await waitFor(() => expect(rows(container)).toHaveLength(3));
    expect(rows(container)).toEqual(['Spec', 'Specs by Status', 'Plan']);
  });

  it('collapses and expands a type\'s views from its chevron, and remembers it', async () => {
    setBackendQueries([
      ['q-spec', 'Specs by Status', 'spec'],
      ['q-plan', 'Plans by Status', 'plan']
    ]);
    const { container, getByLabelText } = render(NavigationSidebar);
    await waitFor(() => expect(rows(container)).toHaveLength(4));

    await fireEvent.click(getByLabelText('Collapse Spec views'));
    expect(rows(container)).toEqual(['Spec', 'Plan', 'Plans by Status']);
    expect(layoutStore.state.collapsedTypeViews).toEqual(['spec']);

    const toggle = getByLabelText('Expand Spec views');
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    await fireEvent.click(toggle);
    expect(rows(container)).toEqual(['Spec', 'Specs by Status', 'Plan', 'Plans by Status']);
    expect(layoutStore.state.collapsedTypeViews).toEqual([]);
  });

  it('gives only a type with views a chevron, and only a view a query icon', async () => {
    setBackendQueries([['q-spec', 'Specs by Status', 'spec']]);
    const { container } = render(NavigationSidebar);
    await waitFor(() => expect(rows(container)).toHaveLength(3));

    const toggles = container.querySelectorAll('[data-testid="type-views-toggle"]');
    expect(Array.from(toggles).map((el) => el.getAttribute('aria-label'))).toEqual([
      'Collapse Spec views'
    ]);

    const withIcon = Array.from(container.querySelectorAll('.schema-type-item'))
      .filter((el) => el.querySelector('.query-icon'))
      .map((el) => el.textContent?.trim());
    expect(withIcon).toEqual(['Specs by Status']);
    // The icon is decorative: the view's accessible name is its own name.
    expect(container.querySelector('.query-icon')?.closest('[aria-hidden="true"]')).not.toBeNull();
  });

  it('does not list queries whose target type has no entry in the list', async () => {
    setBackendQueries([['q-x', 'Orphan', 'unknown-type']]);
    const { container, queryByText } = render(NavigationSidebar);

    await waitFor(() => expect(rows(container)).toEqual(['Spec', 'Plan']));
    expect(queryByText('Orphan')).toBeNull();
  });

  it('updates without a remount when queries are added, renamed and removed', async () => {
    const { container, queryByText } = render(NavigationSidebar);
    await waitFor(() => expect(rows(container)).toEqual(['Spec', 'Plan']));

    await seedQueries([['q1', 'Specs by Status', 'spec']]);
    await waitFor(() => expect(rows(container)).toEqual(['Spec', 'Specs by Status', 'Plan']));

    await seedQueries([['q1', 'Renamed', 'spec']]);
    await waitFor(() => expect(queryByText('Renamed')).not.toBeNull());
    expect(queryByText('Specs by Status')).toBeNull();

    await seedQueries([]);
    await waitFor(() => expect(rows(container)).toEqual(['Spec', 'Plan']));
  });
});
