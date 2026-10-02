// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * A selector that names a saved `query` node. The set of nodes is whatever
 * that query's type and filters select when the trigger fires; its sorting,
 * limit and view belong to the viewer and play no part.
 */
export type SavedQuerySelector = { query_id: string };
