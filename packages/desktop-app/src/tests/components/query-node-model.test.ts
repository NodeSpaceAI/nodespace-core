/**
 * Unit tests for the query-node viewer model helpers (query-node-model.ts) —
 * the default-vs-saved branch decision, definition/view-config parsing, the
 * materialize payload shape, and client-side query execution (filter/sort/limit)
 * behind query-node-viewer.svelte.
 *
 * Follows the project pattern of testing extracted logic functions directly
 * (not rendering Svelte components).
 */

import { describe, it, expect } from 'vitest';
import type { Node } from '$lib/types';
import type { QueryDefinition, QueryFilter } from '$lib/types/query';
import {
  DEFAULT_QUERY_TITLE,
  MATERIALIZED_QUERY_TITLE,
  DEFAULT_VIEW_CONFIG,
  resolveViewerMode,
  parseQueryDefinition,
  parseViewConfig,
  mergeViewConfig,
  buildMaterializedProperties,
  matchesFilter,
  applyFilters,
  applySorting,
  unevaluableFilters,
  executeQueryDefinition,
  shouldShowCreatedNode,
} from '$lib/components/query/query-node-model';

function node(id: string, overrides: Partial<Node> & Record<string, unknown> = {}): Node {
  return {
    id,
    nodeType: 'invoice',
    content: '',
    createdAt: '2026-01-01T00:00:00.000Z',
    modifiedAt: '2026-01-01T00:00:00.000Z',
    version: 1,
    properties: {},
    ...overrides,
  } as Node;
}

describe('resolveViewerMode', () => {
  it('treats a query node as the saved branch', () => {
    expect(resolveViewerMode(node('q', { nodeType: 'query' }))).toBe('saved');
  });

  it('treats a schema node as the default branch', () => {
    expect(resolveViewerMode(node('invoice', { nodeType: 'schema' }))).toBe('default');
  });

  it('treats a missing node as the default branch', () => {
    expect(resolveViewerMode(null)).toBe('default');
    expect(resolveViewerMode(undefined)).toBe('default');
  });
});

describe('parseQueryDefinition', () => {
  it('reads the definition off node properties', () => {
    const filters: QueryFilter[] = [
      { type: 'property', operator: 'equals', property: 'status', value: 'open' },
    ];
    const def = parseQueryDefinition(
      node('q', {
        nodeType: 'query',
        properties: {
          targetType: 'task',
          filters,
          sorting: [{ field: 'dueDate', direction: 'asc' }],
          limit: 25,
        },
      })
    );
    expect(def).toEqual({
      targetType: 'task',
      filters,
      sorting: [{ field: 'dueDate', direction: 'asc' }],
      limit: 25,
    });
  });

  it('falls back to empty definition when properties are absent or malformed', () => {
    const def = parseQueryDefinition(node('q', { nodeType: 'query', properties: {} }));
    expect(def).toEqual({ targetType: '', filters: [], sorting: undefined, limit: undefined });
  });

  it('ignores non-array filters/sorting and non-number limit', () => {
    const def = parseQueryDefinition(
      node('q', {
        nodeType: 'query',
        properties: { targetType: 'task', filters: 'nope', sorting: 5, limit: 'ten' },
      })
    );
    expect(def).toEqual({ targetType: 'task', filters: [], sorting: undefined, limit: undefined });
  });
});

describe('parseViewConfig', () => {
  it('returns the default view config when none stored', () => {
    expect(parseViewConfig(node('invoice', { nodeType: 'schema' }))).toEqual(DEFAULT_VIEW_CONFIG);
    expect(parseViewConfig(null)).toEqual(DEFAULT_VIEW_CONFIG);
  });

  it('reads lastView and kanban groupBy', () => {
    const vc = parseViewConfig(
      node('q', {
        nodeType: 'query',
        properties: { viewConfig: { lastView: 'kanban', kanban: { groupBy: 'status' } } },
      })
    );
    expect(vc).toEqual({ lastView: 'kanban', kanban: { groupBy: 'status' } });
  });

  it('falls back to table for an unrecognized lastView', () => {
    const vc = parseViewConfig(
      node('q', { nodeType: 'query', properties: { viewConfig: { lastView: 'grid' } } })
    );
    expect(vc.lastView).toBe('table');
  });
});

describe('mergeViewConfig', () => {
  it('overrides lastView while preserving kanban', () => {
    const merged = mergeViewConfig({ lastView: 'kanban', kanban: { groupBy: 'status' } }, {
      lastView: 'table',
    });
    expect(merged).toEqual({ lastView: 'table', kanban: { groupBy: 'status' } });
  });

  it('merges a kanban groupBy change without dropping lastView', () => {
    const merged = mergeViewConfig({ lastView: 'kanban' }, { kanban: { groupBy: 'priority' } });
    expect(merged).toEqual({ lastView: 'kanban', kanban: { groupBy: 'priority' } });
  });
});

describe('buildMaterializedProperties', () => {
  it('forces the inherited targetType and generatedBy: user', () => {
    const definition: QueryDefinition = {
      targetType: 'task', // should be overridden by the inherited type
      filters: [{ type: 'property', operator: 'equals', property: 'status', value: 'open' }],
      limit: 50,
    };
    const props = buildMaterializedProperties({
      targetType: 'invoice',
      definition,
      viewConfig: { lastView: 'kanban', kanban: { groupBy: 'status' } },
    });
    expect(props.targetType).toBe('invoice');
    expect(props.generatedBy).toBe('user');
    expect(props.filters).toEqual(definition.filters);
    expect(props.limit).toBe(50);
    expect(props.viewConfig).toEqual({ lastView: 'kanban', kanban: { groupBy: 'status' } });
  });
});

describe('matchesFilter', () => {
  const invoice = node('n1', {
    content: 'Acme Corp invoice',
    properties: { status: 'open', amount: 500 },
  });

  it('matches property equals (case-insensitive by default)', () => {
    expect(
      matchesFilter(invoice, { type: 'property', operator: 'equals', property: 'status', value: 'OPEN' })
    ).toBe(true);
  });

  it('respects caseSensitive when set', () => {
    expect(
      matchesFilter(invoice, {
        type: 'property',
        operator: 'equals',
        property: 'status',
        value: 'OPEN',
        caseSensitive: true,
      })
    ).toBe(false);
  });

  it('matches content contains', () => {
    expect(matchesFilter(invoice, { type: 'content', operator: 'contains', value: 'acme' })).toBe(
      true
    );
  });

  it('matches numeric comparisons', () => {
    expect(
      matchesFilter(invoice, { type: 'property', operator: 'gt', property: 'amount', value: 100 })
    ).toBe(true);
    expect(
      matchesFilter(invoice, { type: 'property', operator: 'lt', property: 'amount', value: 100 })
    ).toBe(false);
  });

  it('matches "in" against an array value', () => {
    expect(
      matchesFilter(invoice, {
        type: 'property',
        operator: 'in',
        property: 'status',
        value: ['open', 'in_progress'],
      })
    ).toBe(true);
  });

  it('handles exists', () => {
    expect(
      matchesFilter(invoice, { type: 'property', operator: 'exists', property: 'status' })
    ).toBe(true);
    expect(
      matchesFilter(invoice, { type: 'property', operator: 'exists', property: 'missing' })
    ).toBe(false);
  });

  it('evaluates node-local relationship filters and passes through graph ones', () => {
    const withRels = node('n2', { mentions: ['m1'], mentionedIn: [{ id: 'src', title: null, nodeType: 'text' }] });
    expect(
      matchesFilter(withRels, { type: 'relationship', operator: 'exists', relationshipType: 'mentions', nodeId: 'm1' })
    ).toBe(true);
    expect(
      matchesFilter(withRels, { type: 'relationship', operator: 'exists', relationshipType: 'mentioned_by', nodeId: 'src' })
    ).toBe(true);
    // parent/children can't be evaluated from a single node → pass-through (true)
    expect(
      matchesFilter(withRels, { type: 'relationship', operator: 'exists', relationshipType: 'parent', nodeId: 'x' })
    ).toBe(true);
  });
});

describe('applyFilters', () => {
  const nodes = [
    node('a', { properties: { status: 'open' } }),
    node('b', { properties: { status: 'closed' } }),
    node('c', { properties: { status: 'open' } }),
  ];

  it('returns all nodes when there are no filters', () => {
    expect(applyFilters(nodes, [])).toHaveLength(3);
  });

  it('ANDs multiple filters', () => {
    const result = applyFilters(nodes, [
      { type: 'property', operator: 'equals', property: 'status', value: 'open' },
    ]);
    expect(result.map((n) => n.id)).toEqual(['a', 'c']);
  });
});

describe('applySorting', () => {
  it('sorts ascending and descending by a property', () => {
    const nodes = [
      node('a', { properties: { amount: 30 } }),
      node('b', { properties: { amount: 10 } }),
      node('c', { properties: { amount: 20 } }),
    ];
    expect(applySorting(nodes, [{ field: 'amount', direction: 'asc' }]).map((n) => n.id)).toEqual([
      'b',
      'c',
      'a',
    ]);
    expect(applySorting(nodes, [{ field: 'amount', direction: 'desc' }]).map((n) => n.id)).toEqual([
      'a',
      'c',
      'b',
    ]);
  });

  // Priority is an enum whose alphabetical order (`high, highest, low, lowest,
  // medium`) is meaningless, so it sorts by urgency rank instead. This mirrors
  // TaskPriority::rank() and compare_priority_values in the Rust query service;
  // saved queries sort here on the client, so the two must agree.
  describe('task priority ranking', () => {
    const byPriority = (...priorities: string[]) =>
      priorities.map((p) => node(p, { properties: { priority: p } }));

    const sortedIds = (
      nodes: ReturnType<typeof byPriority>,
      direction: 'asc' | 'desc' = 'asc',
      type = 'task'
    ) => applySorting(nodes, [{ field: 'priority', direction }], type).map((n) => n.id);

    it('sorts ascending by urgency rank, not alphabetically', () => {
      const nodes = byPriority('low', 'highest', 'medium', 'lowest', 'high');
      expect(sortedIds(nodes)).toEqual(['highest', 'high', 'medium', 'low', 'lowest']);
    });

    it('reverses the rank when descending', () => {
      const nodes = byPriority('medium', 'lowest', 'highest', 'high', 'low');
      expect(sortedIds(nodes, 'desc')).toEqual(['lowest', 'low', 'medium', 'high', 'highest']);
    });

    it('places user-defined values after all core values, lexicographically', () => {
      const nodes = byPriority('critical', 'low', 'highest', 'blocker', 'medium');
      expect(sortedIds(nodes)).toEqual(['highest', 'medium', 'low', 'blocker', 'critical']);
    });

    it('sorts an absent priority first, matching SQL NULL ordering', () => {
      const nodes = [
        node('low', { properties: { priority: 'low' } }),
        node('none', { properties: {} }),
        node('highest', { properties: { priority: 'highest' } }),
      ];
      expect(sortedIds(nodes)).toEqual(['none', 'highest', 'low']);
    });

    it('does not apply the task rank to other target types', () => {
      // project.priority is a separate scale, and a wildcard query spans types
      // whose priorities are not comparable — both fall back to plain ordering,
      // exactly as resolve_order_field does in the backend.
      const nodes = byPriority('low', 'highest', 'medium');
      expect(sortedIds(nodes, 'asc', 'project')).toEqual(['highest', 'low', 'medium']);
      expect(sortedIds(nodes, 'asc', '*')).toEqual(['highest', 'low', 'medium']);
    });
  });
});

describe('executeQueryDefinition', () => {
  const nodes = [
    node('a', { properties: { status: 'open', amount: 30 } }),
    node('b', { properties: { status: 'closed', amount: 10 } }),
    node('c', { properties: { status: 'open', amount: 20 } }),
    node('d', { properties: { status: 'open', amount: 5 } }),
  ];

  it('filters, sorts, then limits', () => {
    const def: QueryDefinition = {
      targetType: 'invoice',
      filters: [{ type: 'property', operator: 'equals', property: 'status', value: 'open' }],
      sorting: [{ field: 'amount', direction: 'asc' }],
      limit: 2,
    };
    expect(executeQueryDefinition(nodes, def).map((n) => n.id)).toEqual(['d', 'c']);
  });

  it('returns everything for an empty definition', () => {
    const def: QueryDefinition = { targetType: 'invoice', filters: [] };
    expect(executeQueryDefinition(nodes, def)).toHaveLength(4);
  });
});

describe('title constants', () => {
  it('exposes the default and materialized titles', () => {
    expect(DEFAULT_QUERY_TITLE).toBe('Default');
    expect(MATERIALIZED_QUERY_TITLE).toBe('Untitled Query');
  });
});

describe('unevaluableFilters', () => {
  const mentions: QueryFilter = {
    type: 'relationship',
    operator: 'equals',
    relationshipType: 'mentions',
    nodeId: 'n1'
  };
  const child: QueryFilter = {
    type: 'relationship',
    operator: 'equals',
    relationshipType: 'children',
    nodeId: 'n1'
  };
  const parent: QueryFilter = {
    type: 'relationship',
    operator: 'equals',
    relationshipType: 'parent',
    nodeId: 'n1'
  };
  const prop: QueryFilter = { type: 'property', operator: 'equals', property: 'status', value: 'open' };

  it('flags only parent/children relationship filters (graph traversal not on the node)', () => {
    expect(unevaluableFilters([parent, child]).length).toBe(2);
  });

  it('treats mentions / mentioned_by and property/content filters as evaluable', () => {
    expect(unevaluableFilters([mentions, prop])).toEqual([]);
  });

  it('returns [] for no filters', () => {
    expect(unevaluableFilters(undefined)).toEqual([]);
    expect(unevaluableFilters([])).toEqual([]);
  });
});

describe('shouldShowCreatedNode', () => {
  const gate = (overrides: Partial<Parameters<typeof shouldShowCreatedNode>[1]> = {}) => ({
    queryState: 'success',
    targetType: 'task',
    loadedNodeIds: [] as string[],
    definition: { targetType: 'task', filters: [] } as QueryDefinition,
    ...overrides,
  });

  it('integrates a settled, matching, not-yet-shown node', () => {
    expect(shouldShowCreatedNode(node('t1', { nodeType: 'task' }), gate())).toBe(true);
  });

  it('rejects a node of a different type (scoped to the view)', () => {
    expect(shouldShowCreatedNode(node('p1', { nodeType: 'project' }), gate())).toBe(false);
  });

  it('rejects a node already shown', () => {
    expect(
      shouldShowCreatedNode(node('t1', { nodeType: 'task' }), gate({ loadedNodeIds: ['t1'] }))
    ).toBe(false);
  });

  it('rejects while the view is still loading (staleness)', () => {
    expect(shouldShowCreatedNode(node('t1', { nodeType: 'task' }), gate({ queryState: 'loading' }))).toBe(
      false
    );
  });

  it('rejects when there is no resolved target type', () => {
    expect(shouldShowCreatedNode(node('t1', { nodeType: 'task' }), gate({ targetType: '' }))).toBe(false);
  });

  it('accepts any type for an all-types (*) saved query', () => {
    expect(
      shouldShowCreatedNode(
        node('x1', { nodeType: 'anything' }),
        gate({ targetType: '*', definition: { targetType: '*', filters: [] } as QueryDefinition })
      )
    ).toBe(true);
  });

  it("honors the saved query's client-side filters", () => {
    const withFilter: QueryDefinition = {
      targetType: 'task',
      filters: [{ type: 'property', operator: 'equals', property: 'status', value: 'open' }],
    };
    expect(
      shouldShowCreatedNode(
        node('t1', { nodeType: 'task', properties: { status: 'open' } }),
        gate({ definition: withFilter })
      )
    ).toBe(true);
    expect(
      shouldShowCreatedNode(
        node('t2', { nodeType: 'task', properties: { status: 'done' } }),
        gate({ definition: withFilter })
      )
    ).toBe(false);
  });
});
