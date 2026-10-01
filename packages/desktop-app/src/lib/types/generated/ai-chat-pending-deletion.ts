// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * A node an agent turn asked to delete, held until the user confirms.
 *
 * The agent's `delete_node` does not delete: it resolves the target and the
 * turn ends asking the user to confirm. The confirmation message carries these
 * records, and only an affirmative reply to that message deletes — against
 * exactly these ids, not a re-resolution of the request. `version` and
 * `descendant_count` are what the user was shown; a change to either between
 * the question and the answer aborts the delete rather than removing
 * something the user did not see.
 */
export type AiChatPendingDeletion = {
  /**
   * Bare node id (no `nodespace://` prefix).
   */
  nodeId: string;
  /**
   * How the node was named to the user.
   */
  title: string;
  /**
   * The node's type (e.g. `"task"`).
   */
  nodeType: string;
  /**
   * The node's version when the user was asked.
   */
  version: number;
  /**
   * Nodes beneath it that the delete cascades to (ADR-041).
   */
  descendantCount: number;
};
