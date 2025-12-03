/**
 * Query Node Type Definitions
 *
 * QueryNode follows the hub-and-spoke architecture (similar to TaskNode/SchemaNode):
 * - Hub (`node.content`): Plain text description (e.g., "All open high-priority tasks")
 * - Spoke (`query` table): Structured query definition fields
 * - Auto-generated spoke table by SchemaTableManager from schema definition
 *
 * Primary use case: AI chat creating queries as child nodes (not manual search UI).
 *
 * @example Hub content
 * ```
 * "All open tasks with high priority due this week"
 * ```
 *
 * @example Spoke fields
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
 * Deserialized directly from spoke table with hub data via record link.
 * Follows the same pattern as TaskNode and SchemaNode.
 */
export interface QueryNode {
	// Hub fields (from query.node.* via record link)
	id: string;
	/** Plain text description of the query */
	content: string;
	version: number;
	createdAt: string;
	modifiedAt: string;

	// Spoke fields (direct from query table)
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
