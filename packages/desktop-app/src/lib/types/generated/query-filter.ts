// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { FilterOperator } from './filter-operator';
import type { FilterType } from './filter-type';
import type { RelationshipType } from './relationship-type';

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
   * Relationship type for relationship filters
   */
  relationshipType?: RelationshipType | null;
  /**
   * Target node ID for relationship filters
   */
  nodeId?: string | null;
  /**
   * Relationship name for a [`FilterType::Related`] filter, exactly as the
   * caller supplied it — a schema-declared name (forward or reverse) or a
   * built-in structural name. This is the caller-facing identity of the
   * relationship; [`Self::resolved_relationship`] carries what it resolves
   * to and is what SQL compilation actually reads.
   */
  relationshipName?: string | null;
  /**
   * The nested filter a [`FilterType::Related`] filter evaluates against
   * the related node(s) reached by [`Self::relationship_name`]. Recursive
   * by construction, but validated by the query service to at most one
   * level of `Related` nesting.
   */
  filter?: QueryFilter | null;
};
