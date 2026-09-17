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
  isResultTruncated,
  matchesFilter,
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

  it('evaluates node-local relationship filters and declines graph ones', () => {
    const withRels = node('n2', { mentions: ['m1'], mentionedIn: [{ id: 'src', title: null, nodeType: 'text' }] });
    expect(
      matchesFilter(withRels, { type: 'relationship', operator: 'exists', relationshipType: 'mentions', nodeId: 'm1' })
    ).toBe(true);
    expect(
      matchesFilter(withRels, { type: 'relationship', operator: 'exists', relationshipType: 'mentioned_by', nodeId: 'src' })
    ).toBe(true);
    // parent/children need graph traversal the node doesn't carry. Unverifiable
    // is not matching: declining keeps a node the query may exclude out of the
    // view until the next load, where the backend evaluates it in SQL.
    expect(
      matchesFilter(withRels, { type: 'relationship', operator: 'exists', relationshipType: 'parent', nodeId: 'x' })
    ).toBe(false);
    expect(
      matchesFilter(withRels, { type: 'relationship', operator: 'exists', relationshipType: 'children', nodeId: 'x' })
    ).toBe(false);
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
