// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { SortDirection } from './sort-direction';

/**
 * Sorting configuration
 */
export type SortConfig = {
  /**
   * Property or field to sort by
   */
  field: string;
  /**
   * Sort direction
   */
  direction: SortDirection;
};
