// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * The node a [`FilterType::Relationship`] filter's path must reach, named by
 * who is asking instead of by id, so a saved query is the same in every
 * database (ADR-099 §4).
 *
 * It is stored as written and resolved each time the query runs, the way a
 * [`RelativeDate`] is.
 */
export type FilterAnchor = 'current_person';
