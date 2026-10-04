/**
 * Pure helpers for the query-node viewer.
 *
 * The viewer serves two shapes from one component, branching on the node it was
 * handed:
 *   - a **schema** node  → the *default* type view: all nodes of the type, no
 *     filters, nothing persisted until the user diverges.
 *   - a **query** node   → a *saved* query: a `QueryNode`, whose typed
 *     definition fields and view config are executed and restored.
 *
 * These functions own the branch decision, the definition/view-config parsing,
 * and the materialize payload shape. Executing a query is the backend's job —
 * `backendAdapter.executeQuery` reaches `QueryService`, the single
 * implementation of filter and sort semantics. What stays here is single-node
 * filter evaluation, for deciding whether a node created elsewhere belongs in
 * an open view (see the section comment below).
 *
 * They are kept DOM-free and side-effect-free so the rules can be unit-tested
 * directly, following the project convention of testing extracted logic rather
 * than rendering Svelte components.
 */

import { isExactly } from '$lib/types/core-node-types';
import type { Node } from '$lib/types';
import type { QueryDefinition, QueryFilter, QueryNode } from '$lib/types/query';
import { resolveFieldValue } from '$lib/components/schema/schema-field-resolution';

/** Header title shown for the (unpersisted) default type view. */
export const DEFAULT_QUERY_TITLE = 'Default';

/** Content a query node is materialized with when the user diverges without
 *  naming it (view change, filter edit). Renamed in place afterwards. */
export const MATERIALIZED_QUERY_TITLE = 'Untitled Query';

export type QueryViewKind = 'list' | 'table' | 'kanban';

/**
 * The minimal per-query view configuration persisted as a query node's
 * `viewConfig` (stored as the schema field `view_config`), so a board and its
 * group-by travel with the query rather than being stranded per-device. Its
 * keys are the viewer's own vocabulary, not schema field names.
 */
export interface QueryViewConfigState {
  lastView: QueryViewKind;
  kanban?: {
    groupBy?: string;
    /** The board's column order, per group-by field: field name → enum values
     *  in display order. Kept per field so switching group-by loses nothing. */
    columnOrder?: Record<string, string[]>;
  };
}

export const DEFAULT_VIEW_CONFIG: QueryViewConfigState = { lastView: 'table' };

export type ViewerMode = 'default' | 'saved';

/**
 * A node backs a *saved* query only when its nodeType is `'query'`. A schema
 * node (or a missing node on a fresh database) is the *default* type view — the
 * tab's decorative `nodeType: 'query'` flag is not trusted; the loaded node is.
 */
export function resolveViewerMode(node: Node | null | undefined): ViewerMode {
  return isExactly(node?.nodeType, 'query') ? 'saved' : 'default';
}

/**
 * The definition a saved query executes — the one mapping from a `QueryNode`
 * to the execution shape.
 */
export function parseQueryDefinition(node: QueryNode): QueryDefinition {
  return {
    targetType: node.targetType,
    filters: node.filters,
    sorting: node.sorting,
    limit: node.limit,
  };
}

/**
 * Read a stored `kanban.columnOrder`: the entries that are a list of strings.
 * `undefined` when nothing usable is stored.
 */
function parseColumnOrder(raw: unknown): Record<string, string[]> | undefined {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return undefined;
  const entries = Object.entries(raw).filter(
    (entry): entry is [string, string[]] =>
      Array.isArray(entry[1]) && entry[1].every((value) => typeof value === 'string')
  );
  return entries.length > 0 ? Object.fromEntries(entries) : undefined;
}

/**
 * Read a stored view config object, with defaults — the one reader for both
 * places the shape is stored: a query node's `viewConfig` and the default
 * view's per-type preferences.
 */
export function parseViewConfigObject(raw: unknown): QueryViewConfigState {
  if (!raw || typeof raw !== 'object') return { ...DEFAULT_VIEW_CONFIG };
  const obj = raw as Record<string, unknown>;

  const lastView: QueryViewKind =
    obj.lastView === 'list' || obj.lastView === 'table' || obj.lastView === 'kanban'
      ? obj.lastView
      : DEFAULT_VIEW_CONFIG.lastView;

  const result: QueryViewConfigState = { lastView };

  const kanbanRaw = obj.kanban;
  if (kanbanRaw && typeof kanbanRaw === 'object') {
    const { groupBy, columnOrder: columnOrderRaw } = kanbanRaw as Record<string, unknown>;
    const columnOrder = parseColumnOrder(columnOrderRaw);
    result.kanban = {
      ...(typeof groupBy === 'string' ? { groupBy } : {}),
      ...(columnOrder ? { columnOrder } : {})
    };
  }

  return result;
}

/** Read a query node's view config, with defaults. */
export function parseViewConfig(node: QueryNode | null | undefined): QueryViewConfigState {
  return parseViewConfigObject(node?.viewConfig);
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
 * Build the create `properties` for a freshly materialized user query node,
 * under the query schema's snake_case storage keys — what `createNode`
 * receives for every core type. The target is always the one supplied
 * (inherited from the schema — never asked for) regardless of what the
 * definition carries, and `generated_by` is fixed to `'user'`. Unset optional
 * fields are left out rather than written empty.
 */
export function buildMaterializedProperties(input: {
  targetType: string;
  definition: QueryDefinition;
  viewConfig: QueryViewConfigState;
}): Record<string, unknown> {
  const { sorting, limit, filters } = input.definition;
  return {
    target_type: input.targetType,
    filters,
    ...(sorting !== undefined ? { sorting } : {}),
    ...(limit !== undefined ? { limit } : {}),
    generated_by: 'user',
    view_config: input.viewConfig,
  };
}

// ============================================================================
// Single-node filter evaluation
//
// The narrow question the backend cannot answer cheaply: when a node is created
// outside this viewer (CLI, an agent tool call, another tab), does it belong in
// the already-open result set? Re-running the whole query per created node
// would be a round-trip each time, so `shouldShowCreatedNode` evaluates the
// filters against that one in-memory node instead.
//
// This is deliberately *not* a query executor: there is no sorting here (the
// appended node lands at the end until the next real query settles) and no
// limit. Filters that need graph traversal are declined rather than guessed at
// — see `matchesFilter`.
// ============================================================================

function isEmpty(value: unknown): boolean {
  return value === null || value === undefined || value === '';
}

/**
 * The backend's SQL `=`. It never folds case (`case_sensitive` applies to
 * `contains` only), and an unset subject equals nothing. A property is
 * compared as the JSON value it is, so `500` is not `"500"`; a node column
 * (`metadata`, `content`) is text, and SQL converts the operand to text.
 */
function equals(actual: unknown, expected: unknown, sameTypeOnly: boolean): boolean {
  if (actual === null || actual === undefined) return false;
  if (actual === expected) return true;
  return !sameTypeOnly && String(actual) === String(expected);
}

/**
 * Case-insensitive matching folds every letter here and only ASCII letters in
 * SQL, so `Élan` matches `élan` here and not there.
 */
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
 * The node columns a `metadata` filter may name, and where each travels on the
 * node. Mirrors the backend's allow-list (`build_metadata_filter`); a name
 * outside it has no value, as the backend rejects it.
 */
const METADATA_FIELDS: Readonly<Record<string, (node: Node) => unknown>> = {
  created_at: (node) => node.createdAt,
  modified_at: (node) => node.modifiedAt,
  node_type: (node) => node.nodeType,
  content: (node) => node.content,
  title: (node) => node.title,
};

/** The value a non-relationship filter compares against. */
function filterSubject(node: Node, filter: QueryFilter): unknown {
  if (filter.type === 'content') return node.content;
  if (!filter.property) return undefined;
  if (filter.type === 'metadata') {
    return Object.hasOwn(METADATA_FIELDS, filter.property)
      ? METADATA_FIELDS[filter.property](node)
      : undefined;
  }
  return resolveFieldValue(node, filter.property);
}

/**
 * Evaluate a single QueryFilter against a node.
 *
 * Supports `property`, `content`, and `metadata` filters fully, and the
 * node-local `relationship` filters: a path of the single hop `mentions` (via
 * `node.mentions`) or `mentioned_by` (via `node.mentionedIn`).
 *
 * Any other path (`child_of`, `has_child`, a schema-declared relationship,
 * several hops, an open-ended hop) needs graph traversal the node doesn't
 * carry, so it returns false: unverifiable is not the same as matching. The
 * caller is
 * deciding whether to *add* a node to a settled result set, and the cost of
 * being wrong is asymmetric — declining leaves the node out until the next
 * query load includes it (the backend evaluates these filters in SQL), while
 * passing it through would show a node the query may well exclude, with
 * nothing to correct it until a reload.
 */
export function matchesFilter(node: Node, filter: QueryFilter): boolean {
  // The backend's default: a `contains` is case-sensitive unless the filter
  // says otherwise.
  const caseSensitive = filter.case_sensitive ?? true;

  // A related-node filter is a condition on other nodes, which this node
  // alone cannot answer.
  if (filter.type === 'related') return false;

  if (filter.type === 'relationship') {
    // A fixed hop travels as its bare name; an open-ended one is an object.
    const [hop, ...more] = filter.path ?? [];
    if (more.length > 0 || typeof hop !== 'string') return false;
    switch (hop) {
      case 'mentions':
        return (node.mentions ?? []).some((id) => id === filter.node_id);
      case 'mentioned_by':
        return (node.mentionedIn ?? []).some((ref) => ref.id === filter.node_id);
      default:
        return false;
    }
  }

  const actual = filterSubject(node, filter);
  const sameTypeOnly = filter.type === 'property';

  switch (filter.operator) {
    case 'exists':
      return !isEmpty(actual);
    case 'equals':
      return equals(actual, filter.value, sameTypeOnly);
    case 'contains':
      return contains(actual, filter.value, caseSensitive);
    case 'in':
      return (
        Array.isArray(filter.value) && filter.value.some((v) => equals(actual, v, sameTypeOnly))
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
 * Whether a result of `rowCount` rows may be hiding further matches.
 *
 * The daemon clamps every query to `maxRows` and says nothing about having done
 * so, so a truncated result is indistinguishable from a complete one by
 * inspection — the row count is the only signal available.
 *
 * A full page counts as truncated only when the bound was the system's rather
 * than the query's own: a query that asked for 25 and got 25 got what it asked
 * for, while one that named no limit, or asked for more than the daemon will
 * return, hit a ceiling it never chose. Extracted here so the rule is pinned by
 * a test rather than living inline in the viewer, where the two constants it
 * compares drifted apart unnoticed once already.
 */
export function isResultTruncated(input: {
  rowCount: number;
  requestedLimit: number | undefined;
  maxRows: number;
}): boolean {
  const systemBounded = input.requestedLimit === undefined || input.requestedLimit > input.maxRows;
  return systemBounded && input.rowCount >= input.maxRows;
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
 * passes the query's filters (a default type view has none).
 *
 * The node is appended to the end of the displayed set regardless of the
 * query's `sorting` — placing it correctly would mean re-deriving the backend's
 * ordering here, which is exactly the duplication this module no longer does.
 * The next query load puts it in its proper place. The query's `limit` is not
 * enforced either, for the same reason: which node the limit would evict
 * depends on that ordering.
 *
 * Corollary of `matchesFilter` declining graph filters: a definition whose
 * filters walk a path it cannot evaluate from one node (anything but a single
 * `mentions` / `mentioned_by` hop) never live-appends, since every node fails
 * the gate, and neither does one with a `related` filter. The same goes for a
 * node of a subtype of the target type: the backend's query returns it, but
 * this gate compares the type exactly, so it appears on the next load. Such a view refreshes on its next load rather than
 * incrementally — acceptable because the filter editor emits property filters
 * only, so those definitions arrive from AI or programmatic creation.
 */
export function shouldShowCreatedNode(node: Node, gate: CreatedNodeGate): boolean {
  if (gate.queryState !== 'success' || !gate.targetType) return false;
  if (gate.loadedNodeIds.includes(node.id)) return false;
  if (gate.targetType !== '*' && node.nodeType !== gate.targetType) return false;
  return (gate.definition.filters ?? []).every((filter) => matchesFilter(node, filter));
}

/** Tooltip on the disabled Kanban tab when the type has nothing to group by. */
export const KANBAN_UNAVAILABLE_REASON =
  'No properties to build a Kanban board from. Kanban groups by a choice (enum) property.';

/**
 * The view actually shown for a requested one: Kanban needs a groupable
 * property, so a stored/saved `kanban` on a type without one opens in List.
 */
export function resolveEffectiveView(
  requested: QueryViewKind,
  kanbanAvailable: boolean
): QueryViewKind {
  return requested === 'kanban' && !kanbanAvailable ? 'list' : requested;
}
