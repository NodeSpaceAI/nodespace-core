// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * The fields of a `pending_delete` edge: what the user was shown when the
 * delete was proposed. A node that differs from it when the user answers is
 * not deleted, so nothing the user did not see is removed.
 */
export type AiChatPendingDeleteEdge = {
  /**
   * The node's version.
   */
  version: number;
  /**
   * How many nodes beneath it the delete cascades to.
   */
  descendant_count: number;
};
