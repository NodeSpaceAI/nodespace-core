/**
 * Pure helpers for the query-node viewer.
 *
 * The viewer serves two shapes from one component, branching on the node it was
 * handed:
 *   - a **schema** node  → the *default* type view: all nodes of the type, no
 *     filters, nothing persisted until the user diverges.
 *   - a **query** node   → a *saved* query: a stored QueryDefinition + view
 *     config, executed with its filters.
 *
 * These functions own the branch decision, the definition/view-config parsing,
 * the materialize payload shape, and the client-side execution of a
 * QueryDefinition (filter/sort/limit). They are kept DOM-free and
 * side-effect-free so the rules can be unit-tested directly, following the
 * project convention of testing extracted logic rather than rendering Svelte
 * components.
 */

import type { Node } from '$lib/types';
import type { QueryDefinition, QueryFilter, SortConfig } from '$lib/types/query';
import { TaskNodeHelpers } from '$lib/types/task-node';

/** Header title shown for the (unpersisted) default type view. */
export const DEFAULT_QUERY_TITLE = 'Default';

/** Content a query node is materialized with when the user diverges without
 *  naming it (view change, filter edit). Renamed in place afterwards. */
export const MATERIALIZED_QUERY_TITLE = 'Untitled Query';

export type QueryViewKind = 'list' | 'table' | 'kanban';

/**
 * The minimal per-query view configuration persisted on a query node's
 * `properties.viewConfig`. Replaces the localStorage QueryPreferencesService for
 * this path so a board (and its group-by) travels with the query rather than
 * being stranded per-device.
 */
export interface QueryViewConfigState {
  lastView: QueryViewKind;
  kanban?: { groupBy?: string };
}

export const DEFAULT_VIEW_CONFIG: QueryViewConfigState = { lastView: 'table' };

export type ViewerMode = 'default' | 'saved';

/**
 * A node backs a *saved* query only when its nodeType is `'query'`. A schema
 * node (or a missing node on a fresh database) is the *default* type view — the
 * tab's decorative `nodeType: 'query'` flag is not trusted; the loaded node is.
 */
export function resolveViewerMode(node: Node | null | undefined): ViewerMode {
  return node?.nodeType === 'query' ? 'saved' : 'default';
}

/** Read the stored QueryDefinition off a query node's properties. */
export function parseQueryDefinition(node: Node): QueryDefinition {
  const props = (node.properties ?? {}) as Record<string, unknown>;
  return {
    targetType: typeof props.targetType === 'string' ? props.targetType : '',
    filters: Array.isArray(props.filters) ? (props.filters as QueryFilter[]) : [],
    sorting: Array.isArray(props.sorting) ? (props.sorting as SortConfig[]) : undefined,
    limit: typeof props.limit === 'number' ? props.limit : undefined,
  };
}

/** Read the stored view config off a query node's properties, with defaults. */
export function parseViewConfig(node: Node | null | undefined): QueryViewConfigState {
  const raw = (node?.properties as Record<string, unknown> | undefined)?.viewConfig;
  if (!raw || typeof raw !== 'object') return { ...DEFAULT_VIEW_CONFIG };

  const obj = raw as Record<string, unknown>;
  const lastView: QueryViewKind =
    obj.lastView === 'list' || obj.lastView === 'table' || obj.lastView === 'kanban'
      ? obj.lastView
      : DEFAULT_VIEW_CONFIG.lastView;

  const result: QueryViewConfigState = { lastView };

  const kanbanRaw = obj.kanban;
  if (kanbanRaw && typeof kanbanRaw === 'object') {
    const groupBy = (kanbanRaw as Record<string, unknown>).groupBy;
    result.kanban = typeof groupBy === 'string' ? { groupBy } : {};
  }

  return result;
}

/** Merge a partial view-config change onto an existing view config. */
export function mergeViewConfig(
  current: QueryViewConfigState,
  partial: Partial<QueryViewConfigState>
): QueryViewConfigState {
  const merged: QueryViewConfigState = { ...current, ...partial };
  if (current.kanban || partial.kanban) {
    merged.kanban = { ...current.kanban, ...partial.kanban };
  }
  return merged;
}

/**
 * Build the `properties` for a freshly materialized user query node. The
 * `targetType` is always the one supplied (inherited from the schema — never
 * asked for) regardless of what the definition carries, and `generatedBy` is
 * fixed to `'user'`.
 */
export function buildMaterializedProperties(input: {
  targetType: string;
  definition: QueryDefinition;
  viewConfig: QueryViewConfigState;
}): Record<string, unknown> {
  return {
    targetType: input.targetType,
    filters: input.definition.filters,
    sorting: input.definition.sorting,
    limit: input.definition.limit,
    generatedBy: 'user',
    viewConfig: input.viewConfig,
  };
}

// ============================================================================
// Client-side query execution
//
// `backendAdapter.queryNodes` only filters by nodeType (plus contentContains /
// mentionedBy / limit) — it cannot evaluate an arbitrary QueryFilter list. So a
// saved query is fetched by its `targetType` and then filtered, sorted, and
// limited here, on the client.
// ============================================================================

/** snake_case → camelCase, mirroring `kanban-grouping.ts` / `table-row.svelte`. */
function toCamelCase(name: string): string {
  return name.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
}

/**
 * Resolve a field's value on a node, mirroring the read order used elsewhere:
 * camelCase top-level (typed core fields) → snake_case top-level →
 * `properties[field]` (user-defined schema fields).
 */
function readFieldValue(node: Node, field: string): unknown {
  const rec = node as unknown as Record<string, unknown>;
  const props = node.properties as Record<string, unknown> | undefined;
  const camel = toCamelCase(field);
  if (rec[camel] !== undefined) return rec[camel];
  if (rec[field] !== undefined) return rec[field];
  return props?.[field];
}

function isEmpty(value: unknown): boolean {
  return value === null || value === undefined || value === '';
}

function equals(actual: unknown, expected: unknown, caseSensitive: boolean): boolean {
  if (actual === expected) return true;
  if (typeof actual === 'number' && typeof expected === 'number') return actual === expected;
  const a = String(actual);
  const b = String(expected);
  return caseSensitive ? a === b : a.toLowerCase() === b.toLowerCase();
}

function contains(actual: unknown, expected: unknown, caseSensitive: boolean): boolean {
  if (isEmpty(actual)) return false;
  const a = String(actual);
  const b = String(expected ?? '');
  return caseSensitive ? a.includes(b) : a.toLowerCase().includes(b.toLowerCase());
}

/** Numeric comparison; falls back to locale string comparison for non-numbers. */
function ordered(actual: unknown, expected: unknown): number {
  const an = Number(actual);
  const bn = Number(expected);
  if (!Number.isNaN(an) && !Number.isNaN(bn)) return an === bn ? 0 : an < bn ? -1 : 1;
  return String(actual).localeCompare(String(expected));
}

/**
 * Evaluate a single QueryFilter against a node.
 *
 * Supports `property`, `content`, and `metadata` filters fully, and the
 * node-local `relationship` filters (`mentions` via `node.mentions`,
 * `mentioned_by` via `node.mentionedIn`). Relationship filters that would need
 * graph traversal not present on the node (`parent`, `children`) cannot be
 * evaluated client-side and are treated as non-restricting (pass-through) so a
 * query still returns its type's nodes rather than silently emptying.
 */
export function matchesFilter(node: Node, filter: QueryFilter): boolean {
  const caseSensitive = filter.caseSensitive ?? false;

  if (filter.type === 'relationship') {
    switch (filter.relationshipType) {
      case 'mentions':
        return (node.mentions ?? []).some((id) => id === filter.nodeId);
      case 'mentioned_by':
        return (node.mentionedIn ?? []).some((ref) => ref.id === filter.nodeId);
      default:
        // parent / children — not evaluable from a single node. Pass-through.
        return true;
    }
  }

  const actual =
    filter.type === 'content'
      ? node.content
      : filter.property
        ? readFieldValue(node, filter.property)
        : undefined;

  switch (filter.operator) {
    case 'exists':
      return !isEmpty(actual);
    case 'equals':
      return equals(actual, filter.value, caseSensitive);
    case 'contains':
      return contains(actual, filter.value, caseSensitive);
    case 'in':
      return (
        Array.isArray(filter.value) && filter.value.some((v) => equals(actual, v, caseSensitive))
      );
    case 'gt':
      return !isEmpty(actual) && ordered(actual, filter.value) > 0;
    case 'gte':
      return !isEmpty(actual) && ordered(actual, filter.value) >= 0;
    case 'lt':
      return !isEmpty(actual) && ordered(actual, filter.value) < 0;
    case 'lte':
      return !isEmpty(actual) && ordered(actual, filter.value) <= 0;
    default:
      return true;
  }
}

/**
 * Filters the client-side executor cannot faithfully evaluate from a single
 * node — `parent`/`children` relationship filters need graph traversal not
 * present on the node, so `matchesFilter` lets them pass through. The viewer
 * surfaces these so a saved query doesn't silently render wider than its
 * definition. (`mentions`/`mentioned_by` and all property/content/metadata
 * operators ARE evaluable.)
 */
export function unevaluableFilters(filters: QueryFilter[] | undefined): QueryFilter[] {
  return (filters ?? []).filter(
    (f) =>
      f.type === 'relationship' &&
      (f.relationshipType === 'parent' || f.relationshipType === 'children')
  );
}

/** Keep only the nodes matching every filter (AND semantics). */
export function applyFilters(nodes: Node[], filters: QueryFilter[]): Node[] {
  if (!filters || filters.length === 0) return nodes;
  return nodes.filter((node) => filters.every((filter) => matchesFilter(node, filter)));
}

/**
 * Compare two `task.priority` values by urgency rank rather than alphabetically.
 *
 * Ascending yields highest, high, medium, low, lowest, then user-defined values
 * ordered lexicographically among themselves — matching `compare_priority_values`
 * in `packages/core/src/services/query_service/mod.rs`. An absent priority sorts
 * first, as it does there and in SQL.
 */
function comparePriority(a: unknown, b: unknown): number {
  if (isEmpty(a) || isEmpty(b)) {
    if (isEmpty(a) && isEmpty(b)) return 0;
    return isEmpty(a) ? -1 : 1;
  }
  const [sa, sb] = [String(a), String(b)];
  const rank = TaskNodeHelpers.priorityRank(sa) - TaskNodeHelpers.priorityRank(sb);
  // Ranks tie for two user-defined values; the value string breaks it.
  return rank !== 0 ? rank : sa.localeCompare(sb);
}

/**
 * Return a sorted copy of `nodes` per the sort config (stable, multi-key).
 *
 * `targetType` selects the same ordering rules the backend's `QueryService`
 * applies, so a query sorted here matches one sorted there. It gates the
 * priority rank to `task`, since `project.priority` is a different scale.
 */
export function applySorting(
  nodes: Node[],
  sorting?: SortConfig[],
  targetType?: string
): Node[] {
  if (!sorting || sorting.length === 0) return nodes;
  return [...nodes].sort((a, b) => {
    for (const sort of sorting) {
      const [va, vb] = [readFieldValue(a, sort.field), readFieldValue(b, sort.field)];
      const cmp =
        sort.field === 'priority' && targetType === 'task'
          ? comparePriority(va, vb)
          : ordered(va, vb);
      if (cmp !== 0) return sort.direction === 'desc' ? -cmp : cmp;
    }
    return 0;
  });
}

/**
 * Execute a QueryDefinition against a pre-fetched node set: filter → sort →
 * limit. The nodes are expected to already be scoped to `definition.targetType`
 * by the caller's `queryNodes({ nodeType })` fetch.
 */
export function executeQueryDefinition(nodes: Node[], definition: QueryDefinition): Node[] {
  let result = applyFilters(nodes, definition.filters);
  result = applySorting(result, definition.sorting, definition.targetType);
  if (typeof definition.limit === 'number' && definition.limit >= 0) {
    result = result.slice(0, definition.limit);
  }
  return result;
}

/** State a viewer needs to decide whether an externally-created node belongs. */
export interface CreatedNodeGate {
  /** The viewer's query lifecycle — only a settled ('success') view integrates. */
  queryState: string;
  /** The resolved type this view shows (`'*'` = an all-types saved query). */
  targetType: string;
  /** Ids already displayed — a node already shown is never re-added. */
  loadedNodeIds: readonly string[];
  /** The active query definition, whose filters the node must also satisfy. */
  definition: QueryDefinition;
}

/**
 * Whether a node created outside the viewer (CLI, agent, another tab) should be
 * folded into an already-open query view without a remount. Pure so the viewer's
 * `sharedNodeStore.subscribeAll` handler stays a one-liner and this logic is
 * testable in isolation. A node qualifies when the view has settled, the node
 * isn't already shown, its type matches the view (or the view is `'*'`), and it
 * passes the query's client-side filters (a default type view has none).
 */
export function shouldShowCreatedNode(node: Node, gate: CreatedNodeGate): boolean {
  if (gate.queryState !== 'success' || !gate.targetType) return false;
  if (gate.loadedNodeIds.includes(node.id)) return false;
  if (gate.targetType !== '*' && node.nodeType !== gate.targetType) return false;
  return executeQueryDefinition([node], gate.definition).length > 0;
}
