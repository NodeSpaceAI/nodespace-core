// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { FilterOperator } from './filter-operator';
import type { FilterType } from './filter-type';
import type { RelationshipPath } from './relationship-path';
import type { RelativeDate } from './relative-date';

/**
 * Individual filter condition
 *
 * A nested stored value: its keys are these field names, snake_case, in the
 * database, on the wire and in the CLI's and the agent's input alike
 * (ADR-086 §9). An unknown key is rejected, so a key in the wrong case fails
 * the write instead of leaving a filter that looks applied and is not.
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
   * A date relative to the day the query runs, compared in place of
   * [`Self::value`] by a [`FilterType::Property`] filter on a date field.
   * A filter carries one or the other, never both.
   */
  relative_date?: RelativeDate | null;
  /**
   * Case sensitivity for text comparisons
   */
  case_sensitive?: boolean | null;
  /**
   * The node a [`FilterType::Relationship`] filter's path must reach.
   */
  node_id?: string | null;
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
