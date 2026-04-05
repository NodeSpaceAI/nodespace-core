/**
 * Query Node Type Definitions
 *
 * QueryNode follows the Universal Graph Architecture (similar to TaskNode/SchemaNode):
 * - Node content (`node.content`): Plain text description (e.g., "All open high-priority tasks")
 * - Node properties (`node.properties`): Structured query definition fields stored as JSON
 * - No separate query table — Universal Graph Architecture uses only `node`, `relationship`, `embedding`
 *
 * Primary use case: AI chat creating queries as child nodes (not manual search UI).
 *
 * @example Node content
 * ```
 * "All open tasks with high priority due this week"
 * ```
 *
 * @example Node properties
 * ```typescript
 * {
 *   targetType: "task",
 *   filters: [{type: "property", operator: "equals", property: "status", value: "open"}],
 *   sorting: [{field: "dueDate", direction: "asc"}],
 *   limit: 50,
 *   generatedBy: "ai",
 *   generatorContext: "chat-node-123"
 * }
 * ```
 */

/**
 * Strongly-typed QueryNode structure
 *
 * Deserialized directly from node properties with base node data via record link.
 * Follows the same pattern as TaskNode and SchemaNode.
 */
export interface QueryNode {
	// Node fields (from query.node.* via record link)
	id: string;
	/** Plain text description of the query */
	content: string;
	version: number;
	createdAt: string;
	modifiedAt: string;

	// Type-specific fields (deserialized from node.properties)
	/** Target node type: 'task', 'text', 'date', or '*' for all types */
	targetType: string;
	/** Filter conditions to apply */
	filters: QueryFilter[];
	/** Optional sorting configuration */
	sorting?: SortConfig[];
	/** Optional result limit (default: 50) */
	limit?: number;
	/** Who created this query: 'ai' or 'user' */
	generatedBy: 'ai' | 'user';
	/** Parent chat ID for AI-generated queries (optional) */
	generatorContext?: string;
	/** Number of times query has been executed (system-managed) */
	executionCount?: number;
	/** ISO timestamp of last execution (system-managed) */
	lastExecuted?: string;
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
 * View configuration (discriminated union)
 *
 * View type and configuration are specified at render time via
 * QueryPreferencesService (#443), enabling different users to view
 * the same query differently.
 */
export interface BaseViewConfig {
	view: 'list' | 'table' | 'kanban';
}

export interface ListViewConfig extends BaseViewConfig {
	view: 'list';
	layout: 'compact' | 'comfortable' | 'spacious';
	showProperties?: string[];
	groupBy?: string;
}

export interface TableViewConfig extends BaseViewConfig {
	view: 'table';
	columns: ColumnConfig[];
	sortBy?: { field: string; direction: 'asc' | 'desc' };
}

export interface KanbanViewConfig extends BaseViewConfig {
	view: 'kanban';
	groupBy: string; // REQUIRED for kanban
	cardLayout: 'compact' | 'detailed';
}

export type QueryViewConfig = ListViewConfig | TableViewConfig | KanbanViewConfig;

export interface ColumnConfig {
	field: string;
	label: string;
	width?: number;
	sortable?: boolean;
	format?: 'text' | 'date' | 'number' | 'enum';
}

/**
 * A QueryDefinition is the subset of QueryNode fields that define the query
 * itself — stored as node.properties on a query node.
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
