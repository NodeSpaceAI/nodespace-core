// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * A node an agent turn asked to delete, held until the user confirms.
 *
 * Mirrors `nodespace_core::models::AiChatPendingDeletion`.
 */
export type AiChatPendingDeletion = {
  nodeId: string;
  title: string;
  nodeType: string;
  version: number;
  descendantCount: number;
};
