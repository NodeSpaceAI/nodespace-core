// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { QueryFilter } from './query-filter';
import type { QueryGeneratedBy } from './query-generated-by';
import type { SortConfig } from './sort-config';

/**
 * Partial update for a query's fields, received from the frontend.
 *
 * `target_type`, `filters` and `generated_by` have no clear path (the
 * schema requires them); the other fields are tri-state: absent leaves the
 * field unchanged, `null` clears it, and a value sets it. `view_config` is
 * replaced whole, never merged. The system-managed `execution_count` and
 * `last_executed` are not client-writable.
 */
export type QueryNodeUpdate = {
  targetType?: string;
  filters?: Array<QueryFilter>;
  sorting?: Array<SortConfig> | null;
  limit?: number | null;
  generatedBy?: QueryGeneratedBy;
  generatorContext?: string | null;
  viewConfig?: Record<string, unknown> | null;
};
