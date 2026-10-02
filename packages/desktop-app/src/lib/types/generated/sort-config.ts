// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { SortDirection } from './sort-direction';

/**
 * Sorting configuration
 *
 * A nested stored value, like [`QueryFilter`]: snake_case keys, one spelling,
 * unknown keys rejected.
 */
export type SortConfig = {
  /**
   * The field to sort by, under its stored name: a schema field
   * (`due_date`) or a metadata column (`created_at`, `modified_at`,
   * `node_type`, `content`, `title`)
   */
  field: string;
  /**
   * Sort direction
   */
  direction: SortDirection;
};
