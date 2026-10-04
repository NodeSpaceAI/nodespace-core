// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { NodeReference } from './node-reference';
import type { QueryFilter } from './query-filter';
import type { QueryGeneratedBy } from './query-generated-by';
import type { SortConfig } from './sort-config';

/**
 * Wire shape for query nodes sent to the frontend.
 *
 * Produced by `node_to_typed_value` for `node_type == "query"`: the query
 * schema's fields are promoted to the top level (camelCase, see
 * [`QueryFields`]) and `properties` keeps only extension fields. Maps
 * directly to the TypeScript `QueryNode` interface.
 */
export type QueryNode = {
  id: string;
  nodeType: string;
  content: string;
  version: number;
  createdAt: string;
  modifiedAt: string;
  properties: Record<string, unknown>;
  mentions?: Array<string>;
  mentionedIn?: Array<NodeReference>;
  title?: string | null;
  lifecycleStatus: string;
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
   * How the query renders. Its keys (`lastView`, `kanban.groupBy`,
   * `kanban.columnOrder`) are the viewer's own vocabulary, not schema field
   * names, so the object is carried as-is rather than typed here.
   */
  viewConfig?: Record<string, unknown>;
};
