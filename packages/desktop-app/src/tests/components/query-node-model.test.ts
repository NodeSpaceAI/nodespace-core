/**
 * Unit tests for the query-node viewer model helpers (query-node-model.ts) —
 * the default-vs-saved branch decision, definition/view-config parsing, the
 * materialize payload shape, and the single-node filter evaluation behind
 * query-node-viewer.svelte's live-append gate.
 *
 * Executing a query is the backend's job (QueryService), so there is no
 * filter/sort/limit executor here to test. Sort semantics — notably the
 * task.priority urgency rank — are covered on the Rust side, and the encoding
 * that carries a sort there is covered in tests/services/adapter-core.test.ts.
 *
 * Follows the project pattern of testing extracted logic functions directly
 * (not rendering Svelte components).
 */

import { describe, it, expect } from 'vitest';
import type { Node } from '$lib/types';
import {
  nodeToQueryNode,
  type QueryDefinition,
  type QueryFilter,
  type QueryNode,
} from '$lib/types/query';
import { storageNodeToApiFields } from '$lib/services/node-normalize';
import {
  DEFAULT_QUERY_TITLE,
  MATERIALIZED_QUERY_TITLE,
  DEFAULT_VIEW_CONFIG,
  resolveViewerMode,
  parseQueryDefinition,
  parseViewConfig,
  mergeViewConfig,
  buildMaterializedProperties,
  isResultTruncated,
  matchesFilter,
  shouldShowCreatedNode,
  type QueryViewConfigState,
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

function queryNode(overrides: Partial<QueryNode> = {}): QueryNode {
  return nodeToQueryNode(node('q', { nodeType: 'query', ...overrides } as Partial<Node>));
}

describe('parseQueryDefinition', () => {
  it('reads the definition off the typed fields', () => {
    const filters: QueryFilter[] = [
      { type: 'property', operator: 'equals', property: 'status', value: 'open' },
    ];
    const def = parseQueryDefinition(
      queryNode({
        targetType: 'task',
        filters,
        sorting: [{ field: 'dueDate', direction: 'asc' }],
        limit: 25,
      })
    );
    expect(def).toEqual({
      targetType: 'task',
      filters,
      sorting: [{ field: 'dueDate', direction: 'asc' }],
      limit: 25,
    });
  });

  it('takes the schema defaults for a query that names nothing', () => {
    expect(parseQueryDefinition(queryNode())).toEqual({
      targetType: '*',
      filters: [],
      sorting: undefined,
      limit: undefined,
    });
  });

  it('never reads a query field out of properties', () => {
    const def = parseQueryDefinition(
      queryNode({ properties: { targetType: 'stale', filters: [{ type: 'content' }] } })
    );
    expect(def.targetType).toBe('*');
    expect(def.filters).toEqual([]);
  });
});

describe('parseViewConfig', () => {
  it('returns the default view config when none stored', () => {
    expect(parseViewConfig(queryNode())).toEqual(DEFAULT_VIEW_CONFIG);
    expect(parseViewConfig(null)).toEqual(DEFAULT_VIEW_CONFIG);
  });

  it('reads lastView and kanban groupBy', () => {
    const vc = parseViewConfig(
      queryNode({ viewConfig: { lastView: 'kanban', kanban: { groupBy: 'status' } } })
    );
    expect(vc).toEqual({ lastView: 'kanban', kanban: { groupBy: 'status' } });
  });

  it('falls back to table for an unrecognized lastView', () => {
    const vc = parseViewConfig(queryNode({ viewConfig: { lastView: 'grid' } }));
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
  const definition: QueryDefinition = {
    targetType: 'task', // overridden by the inherited type
    filters: [{ type: 'property', operator: 'equals', property: 'status', value: 'open' }],
    sorting: [{ field: 'due_date', direction: 'desc' }],
    limit: 50,
  };
  const viewConfig: QueryViewConfigState = { lastView: 'kanban', kanban: { groupBy: 'status' } };

  it('writes only the schema\'s snake_case storage keys, with one target holding the inherited type', () => {
    const props = buildMaterializedProperties({ targetType: 'invoice', definition, viewConfig });
    expect(Object.keys(props).sort()).toEqual([
      'filters',
      'generated_by',
      'limit',
      'sorting',
      'target_type',
      'view_config',
    ]);
    expect(props.target_type).toBe('invoice');
    expect(props.generated_by).toBe('user');
    expect(props.view_config).toEqual(viewConfig);
  });

  it('leaves unset optional fields out rather than writing them empty', () => {
    const props = buildMaterializedProperties({
      targetType: 'invoice',
      definition: { targetType: 'invoice', filters: [] },
      viewConfig: { lastView: 'table' },
    });
    expect('sorting' in props).toBe(false);
    expect('limit' in props).toBe(false);
  });

  // The stored bucket travels back through the same storage → wire promotion
  // the backend applies, so the reopened query must match what was saved.
  it('reopens with the same filters, sorting, view and Kanban groupBy', () => {
    const props = buildMaterializedProperties({ targetType: 'invoice', definition, viewConfig });
    const wire = storageNodeToApiFields('query', { query: props });
    const reopened = nodeToQueryNode({ ...node('q', { nodeType: 'query' }), ...wire } as Node);

    expect(reopened.properties).toEqual({});
    expect(parseQueryDefinition(reopened)).toEqual({ ...definition, targetType: 'invoice' });
    expect(parseViewConfig(reopened)).toEqual(viewConfig);
  });
});

describe('matchesFilter', () => {
  const invoice = node('n1', {
    content: 'Acme Corp invoice',
    properties: { status: 'open', amount: 500 },
  });

  it('declines a related-node filter, which is a condition on other nodes', () => {
    const anyNode = { id: 'n', nodeType: 'task', content: '', properties: {} } as unknown as Node;
    // No `value`, the shape an `equals` on an absent subject would otherwise pass.
    expect(
      matchesFilter(anyNode, {
        type: 'related',
        operator: 'equals',
        path: ['project'],
        filter: { type: 'property', operator: 'equals', property: 'status', value: 'active' }
      })
    ).toBe(false);
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

  it('reads a snake_case custom field by its stored name only', () => {
    const filter = {
      type: 'property',
      operator: 'equals',
      property: 'billing_contact',
      value: 'Avery',
    } as const;
    expect(matchesFilter(node('n2', { properties: { billing_contact: 'Avery' } }), filter)).toBe(
      true
    );
    // Neither a camelCase key in properties nor a top-level key is the field.
    expect(
      matchesFilter(
        node('n3', { properties: { billingContact: 'Avery' }, billingContact: 'Avery' }),
        filter
      )
    ).toBe(false);
  });

  it("reads a core type's declared field from its typed key", () => {
    const task = node('t1', { nodeType: 'task', dueDate: '2026-03-01' });
    expect(
      matchesFilter(task, {
        type: 'property',
        operator: 'equals',
        property: 'due_date',
        value: '2026-03-01',
      })
    ).toBe(true);
  });

  it("reads a subtype's inherited field from properties", () => {
    // Typed keys belong to the exact core type; a type extending task carries
    // the inherited field flat in properties.
    const bug = node('b1', { nodeType: 'bug', properties: { due_date: '2026-03-01' } });
    expect(
      matchesFilter(bug, {
        type: 'property',
        operator: 'equals',
        property: 'due_date',
        value: '2026-03-01',
      })
    ).toBe(true);
  });

  describe('metadata filters', () => {
    const titled = node('m1', {
      nodeType: 'invoice',
      content: 'Acme Corp invoice',
      title: 'Acme invoice',
      createdAt: '2026-10-01T09:00:00.000Z',
      modifiedAt: '2026-10-02T09:00:00.000Z',
    });

    it('matches node_type', () => {
      expect(
        matchesFilter(titled, {
          type: 'metadata',
          operator: 'equals',
          property: 'node_type',
          value: 'invoice',
        })
      ).toBe(true);
      expect(
        matchesFilter(titled, {
          type: 'metadata',
          operator: 'equals',
          property: 'node_type',
          value: 'task',
        })
      ).toBe(false);
    });

    it('compares created_at', () => {
      expect(
        matchesFilter(titled, {
          type: 'metadata',
          operator: 'gte',
          property: 'created_at',
          value: '2026-01-01',
        })
      ).toBe(true);
      expect(
        matchesFilter(titled, {
          type: 'metadata',
          operator: 'lt',
          property: 'created_at',
          value: '2026-01-01',
        })
      ).toBe(false);
    });

    it('compares modified_at', () => {
      expect(
        matchesFilter(titled, {
          type: 'metadata',
          operator: 'gt',
          property: 'modified_at',
          value: '2026-10-01T12:00:00.000Z',
        })
      ).toBe(true);
    });

    it('matches title and content', () => {
      expect(
        matchesFilter(titled, {
          type: 'metadata',
          operator: 'contains',
          property: 'title',
          value: 'Acme inv',
        })
      ).toBe(true);
      expect(
        matchesFilter(titled, {
          type: 'metadata',
          operator: 'contains',
          property: 'content',
          value: 'Corp',
        })
      ).toBe(true);
    });

    it('does not read a schema field or an unknown column', () => {
      const withProp = node('m2', { properties: { status: 'open' } });
      // `constructor` and `__proto__` are inherited object keys, not columns.
      for (const property of ['status', 'constructor', '__proto__']) {
        expect(matchesFilter(withProp, { type: 'metadata', operator: 'exists', property })).toBe(
          false
        );
      }
    });
  });

  it('evaluates node-local relationship filters and declines graph ones', () => {
    const withRels = node('n2', { mentions: ['m1'], mentionedIn: [{ id: 'src', title: null, nodeType: 'text' }] });
    expect(
      matchesFilter(withRels, { type: 'relationship', operator: 'exists', path: ['mentions'], nodeId: 'm1' })
    ).toBe(true);
    expect(
      matchesFilter(withRels, { type: 'relationship', operator: 'exists', path: ['mentioned_by'], nodeId: 'src' })
    ).toBe(true);
    // Parent/children need graph traversal the node doesn't carry. Unverifiable
    // is not matching: declining keeps a node the query may exclude out of the
    // view until the next load, where the backend evaluates it in SQL.
    expect(
      matchesFilter(withRels, { type: 'relationship', operator: 'exists', path: ['child_of'], nodeId: 'x' })
    ).toBe(false);
    expect(
      matchesFilter(withRels, { type: 'relationship', operator: 'exists', path: ['has_child'], nodeId: 'x' })
    ).toBe(false);
  });

  it('declines a relationship path it cannot walk from one node', () => {
    const withRels = node('n2', { mentions: ['m1'] });
    // Two hops, an open-ended hop and a missing path all reach past the node.
    for (const path of [
      ['mentions', 'mentions'],
      [{ name: 'mentions', open_ended: true }],
      [],
      undefined
    ]) {
      expect(
        matchesFilter(withRels, { type: 'relationship', operator: 'exists', path, nodeId: 'm1' })
      ).toBe(false);
    }
  });
});

describe('title constants', () => {
  it('exposes the default and materialized titles', () => {
    expect(DEFAULT_QUERY_TITLE).toBe('Default');
    expect(MATERIALIZED_QUERY_TITLE).toBe('Untitled Query');
  });
});

describe('isResultTruncated', () => {
  // The daemon clamps silently, so the row count is the only available signal.
  const MAX = 500;

  it('flags a full page when the query named no limit', () => {
    expect(isResultTruncated({ rowCount: MAX, requestedLimit: undefined, maxRows: MAX })).toBe(true);
  });

  it('does not flag a short page', () => {
    expect(isResultTruncated({ rowCount: 42, requestedLimit: undefined, maxRows: MAX })).toBe(false);
  });

  it('does not flag a query that asked for N and got N', () => {
    // The bound was the query's own intent, so the result is complete however
    // small it is — this is the case a naive `rowCount >= maxRows` gets wrong
    // in the other direction by never firing at all.
    expect(isResultTruncated({ rowCount: 25, requestedLimit: 25, maxRows: MAX })).toBe(false);
  });

  it('flags a query that asked for more than the daemon will return', () => {
    // Asking for 1000 yields at most 500, clamped without notice — the exact
    // mismatch that made the caveat unreachable when the viewer's own constant
    // exceeded the server ceiling.
    expect(isResultTruncated({ rowCount: MAX, requestedLimit: 1000, maxRows: MAX })).toBe(true);
  });

  it('does not flag an over-asking query that came back short', () => {
    // Asked for 1000, got 87: the clamp never bound it, so nothing is hidden.
    expect(isResultTruncated({ rowCount: 87, requestedLimit: 1000, maxRows: MAX })).toBe(false);
  });

  it('treats a limit equal to the ceiling as the query’s own bound', () => {
    expect(isResultTruncated({ rowCount: MAX, requestedLimit: MAX, maxRows: MAX })).toBe(false);
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
        node('t1', { nodeType: 'task', status: 'open' }),
        gate({ definition: withFilter })
      )
    ).toBe(true);
    expect(
      shouldShowCreatedNode(
        node('t2', { nodeType: 'task', status: 'done' }),
        gate({ definition: withFilter })
      )
    ).toBe(false);
  });
});
