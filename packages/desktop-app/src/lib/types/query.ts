/**
 * Query Node Type Definitions
 *
 * `QueryNode` matches the Rust `QueryNode` wire shape
 * (`packages/nodespace-types/src/query.rs`): the query schema's fields travel
 * as typed top-level fields (`targetType`, `filters`, `viewConfig`, …).
 * `properties` carries only extension fields (`custom:…`), never a query
 * field. Storage uses the schema's snake_case names (`target_type`,
 * `view_config`, …) under `properties.query` — the shape a query is created
 * with (see `buildMaterializedProperties`), and nothing else reads it.
 *
 * Node content is the query's name (e.g. "All open high-priority tasks").
 */

import type { Node } from './node';

export interface QueryNode {
  id: string;
  nodeType: 'query';
  content: string;
  title?: string | null;
  version: number;
  createdAt: string;
  modifiedAt: string;
  /** Extension fields only — query fields are the typed fields below. */
  properties: Record<string, unknown>;

  /** Target node type, or '*' for all types */
  targetType: string;
  /** Filter conditions to apply */
  filters: QueryFilter[];
  sorting?: SortConfig[];
  limit?: number;
  /** Who created this query */
  generatedBy: QueryGeneratedBy;
  /** Parent chat ID for AI-generated queries */
  generatorContext?: string;
  /** System-managed */
  executionCount: number;
  /** ISO timestamp of last execution (system-managed) */
  lastExecuted?: string;
  /**
   * How the query renders. Its keys (`lastView`, `kanban.groupBy`) are the
   * viewer's own vocabulary — `parseViewConfig` in `query-node-model.ts` is
   * their reader.
   */
  viewConfig?: Record<string, unknown>;
}

export type QueryGeneratedBy = 'ai' | 'user';

/**
 * Partial update for a query's fields. Mirrors the Rust `QueryNodeUpdate`:
 * absent = no change, `null` = clear, a value = set. `targetType`, `filters`
 * and `generatedBy` cannot be cleared (the schema requires them);
 * `viewConfig` is replaced whole. The system-managed fields are not
 * writable.
 */
export interface QueryNodeUpdate {
  targetType?: string;
  filters?: QueryFilter[];
  sorting?: SortConfig[] | null;
  limit?: number | null;
  generatedBy?: QueryGeneratedBy;
  generatorContext?: string | null;
  viewConfig?: Record<string, unknown> | null;
}

/**
 * Convert a node received over any transport to a `QueryNode`. The backend
 * (`node_to_typed_value`) already promotes the query fields to the top level
 * for every transport, so this only narrows the type and fills the schema
 * defaults.
 */
export function nodeToQueryNode(node: Node): QueryNode {
  const query = node as unknown as QueryNode;
  return {
    ...query,
    nodeType: 'query',
    properties: node.properties ?? {},
    targetType: query.targetType ?? '*',
    filters: query.filters ?? [],
    generatedBy: query.generatedBy ?? 'user',
    executionCount: query.executionCount ?? 0
  };
}

/**
 * Individual filter condition
 *
 * Filters can target properties, content, relationships, or metadata.
 */
export interface QueryFilter {
	/** Filter category */
	type: 'property' | 'content' | 'relationship' | 'metadata';

	/** Comparison operator */
	operator: 'equals' | 'contains' | 'gt' | 'lt' | 'gte' | 'lte' | 'in' | 'exists';

	/** Property key for property filters */
	property?: string;

	/** Expected value */
	value?: unknown;

	/** Case sensitivity for text comparisons */
	caseSensitive?: boolean;

	/** Relationship type for relationship filters */
	relationshipType?: 'parent' | 'children' | 'mentions' | 'mentioned_by';

	/** Target node ID for relationship filters */
	nodeId?: string;
}

/**
 * Sorting configuration
 */
export interface SortConfig {
	/** Property or field to sort by */
	field: string;

	/** Sort direction */
	direction: 'asc' | 'desc';
}

/**
 * A QueryDefinition is the subset of QueryNode fields that define the query
 * itself — the execution shape `backendAdapter.executeQuery` takes. Built
 * from a `QueryNode` by `parseQueryDefinition`.
 *
 * Extracted here so both components and services can import it without
 * coupling to a specific .svelte file.
 */
export interface QueryDefinition {
	targetType: string;
	filters: QueryFilter[];
	sorting?: SortConfig[];
	limit?: number;
}

export const DEFAULT_QUERY: QueryDefinition = {
	targetType: 'task',
	filters: [],
	limit: 50,
};

export const QUERY_TEMPLATE_EXAMPLES: Array<{ label: string; definition: QueryDefinition }> = [
	{
		label: 'All incomplete tasks',
		definition: {
			targetType: 'task',
			filters: [
				{
					type: 'property',
					operator: 'in',
					property: 'status',
					value: ['open', 'in_progress'],
				},
			],
			limit: 50,
		},
	},
	{
		label: 'Recent text nodes with keyword',
		definition: {
			targetType: 'text',
			filters: [
				{
					type: 'content',
					operator: 'contains',
					value: 'keyword',
				},
			],
			sorting: [{ field: 'modifiedAt', direction: 'desc' }],
			limit: 25,
		},
	},
	{
		label: 'Tasks by priority',
		definition: {
			targetType: 'task',
			filters: [
				{
					type: 'property',
					operator: 'equals',
					property: 'priority',
					value: 'high',
				},
			],
			sorting: [{ field: 'dueDate', direction: 'asc' }],
			limit: 50,
		},
	},
];
