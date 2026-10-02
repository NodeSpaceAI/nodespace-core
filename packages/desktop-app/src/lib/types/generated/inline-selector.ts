// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { QueryFilter } from './query-filter';

/**
 * A selector written out in the rule: the type and filters a query node
 * stores to say which nodes it selects.
 */
export type InlineSelector = {
  /**
   * The node type selected, subtypes included, or `*` for every type.
   */
  target_type: string;
  /**
   * Filters every selected node satisfies, as a query node stores them.
   */
  filters?: Array<QueryFilter>;
};
