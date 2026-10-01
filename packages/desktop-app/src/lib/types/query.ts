/**
 * Query node helpers.
 *
 * `QueryNode`, `QueryNodeUpdate`, `QueryFilter` and `SortConfig` are generated
 * from Rust's `nodespace-types` (`./generated`): the query schema's fields
 * travel as typed top-level fields (`targetType`, `filters`, `viewConfig`, …),
 * and `properties` carries only extension fields (`custom:…`). Storage uses the
 * schema's snake_case names (`target_type`, `view_config`, …) under
 * `properties.query`, the shape a query is created with (see
 * `buildMaterializedProperties`); nothing else reads it.
 *
 * `viewConfig`'s keys (`lastView`, `kanban.groupBy`) are the viewer's own
 * vocabulary; `parseViewConfig` in `query-node-model.ts` is their reader.
 *
 * Node content is the query's name (e.g. "All open high-priority tasks").
 */

import type { Node } from './node';
import type { QueryFilter, QueryNode, SortConfig } from './generated';

export type {
  QueryFilter,
  QueryGeneratedBy,
  QueryNode,
  QueryNodeUpdate,
  SortConfig
} from './generated';

/**
 * Convert a node received over any transport to a `QueryNode`. The backend
 * (`node_to_typed_value`) already promotes the query fields to the top level
 * for every transport, so this only narrows the type and fills the schema
 * defaults.
 */
export function nodeToQueryNode(node: Node): QueryNode {
  const query = node as Node & Partial<QueryNode>;
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
