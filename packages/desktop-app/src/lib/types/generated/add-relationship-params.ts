// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Params of an `add_relationship` action. `edge_data` is open because the
 * relationship's declared edge fields decide its keys.
 */
export type AddRelationshipParams = {
  source_id: string;
  relationship_type: string;
  target_id: string;
  edge_data?: Record<string, unknown>;
};
