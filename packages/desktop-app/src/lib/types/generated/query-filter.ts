// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { FilterOperator } from './filter-operator';
import type { FilterType } from './filter-type';
import type { RelationshipPath } from './relationship-path';

/**
 * Individual filter condition
 */
export type QueryFilter = {
  /**
   * Filter category
   */
  type: FilterType;
  /**
   * Comparison operator
   */
  operator: FilterOperator;
  /**
   * Property key for property filters
   */
  property?: string | null;
  /**
   * Expected value
   */
  value?: unknown;
  /**
   * Case sensitivity for text comparisons
   */
  caseSensitive?: boolean | null;
  /**
   * The node a [`FilterType::Relationship`] filter's path must reach.
   */
  nodeId?: string | null;
  /**
   * The walk a [`FilterType::Relationship`] or [`FilterType::Related`]
   * filter makes from each candidate node: built-in, schema-declared and
   * reverse names, fixed or open-ended. [`Self::resolved_path`] carries
   * what the names resolve to and is what SQL compilation reads.
   */
  path?: RelationshipPath | null;
  /**
   * The nested filter a [`FilterType::Related`] filter evaluates against
   * the nodes [`Self::path`] reaches. Recursive by construction, but
   * validated by the query service to at most one level of `Related`
   * nesting.
   */
  filter?: QueryFilter | null;
};
