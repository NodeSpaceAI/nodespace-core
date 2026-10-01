// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { QueryFilter } from './query-filter';
import type { QueryGeneratedBy } from './query-generated-by';
import type { SortConfig } from './sort-config';

/**
 * The query schema's fields, decoded from a query node's properties.
 *
 * This is the only reader of a stored query: storage keys are the schema's
 * snake_case field names (`target_type`, `view_config`, …), hoisted by the
 * store under `properties.query.*`. [`Self::from_properties`] reads that
 * bucket, or the flat shape a node built in memory or a create payload
 * carries. On the wire the same fields travel camelCase at the top level of
 * a [`QueryNode`].
 */
export type QueryFields = {
  /**
   * The node type the query selects, or [`ALL_TYPES_TARGET`].
   */
  targetType: string;
  filters: Array<QueryFilter>;
  sorting?: Array<SortConfig>;
  limit?: number;
  generatedBy: QueryGeneratedBy;
  /**
   * Parent chat id for an AI-generated query.
   */
  generatorContext?: string;
  /**
   * System-managed.
   */
  executionCount: number;
  /**
   * System-managed.
   */
  lastExecuted?: string;
  /**
   * How the query renders. Its keys (`lastView`, `kanban.groupBy`) are the
   * viewer's own vocabulary, not schema field names, so the object is
   * carried as-is rather than typed here.
   */
  viewConfig?: Record<string, unknown>;
};
