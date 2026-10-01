// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { OrderBy } from './order-by';

export type NodeQuery = {
  id?: string;
  /**
   * Restrict the result to this explicit set of ids (e.g. a collection's
   * members). Translated to an `id IN (…)` clause, chunked under SQLite's
   * bound-parameter ceiling. `None` = no id restriction.
   */
  ids?: Array<string>;
  mentionedBy?: string;
  contentContains?: string;
  titleContains?: string;
  nodeType?: string;
  /**
   * Sort order applied by the store before `limit`/`offset` are sliced,
   * so pagination is stable across repeated calls with the same query.
   * `None` leaves the underlying result order store-defined.
   */
  orderBy?: OrderBy;
  limit?: number;
  offset?: number;
  /**
   * Also return archived nodes. The default leaves them out: an archived
   * node participates in nothing (ADR-087 §2). Omittable on the wire.
   */
  includeArchived?: boolean;
};
