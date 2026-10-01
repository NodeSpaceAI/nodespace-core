// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * A concrete graph entity a read-only tool call surfaced during an assistant
 * turn.
 *
 * `completed_writes` gives the next turn durable proof of what a write-tool
 * call did; nothing analogous existed for reads, so a turn that merely
 * *looked up* a node (`search_nodes`, `get_node`, ...) left no structured
 * trace once the ephemeral session ended — only the assistant's prose reply
 * survived, with no node id in it. A follow-up like "update that" then has
 * nothing to resolve "that" against. This is the read-side counterpart:
 * minimal identity only (no mutable fields, so it cannot go stale in a way
 * that misleads), populated from the same tool-execution records
 * `completed_writes` already derives from.
 */
export type AiChatResolvedEntity = {
  /**
   * ID of the node a read tool surfaced (as a `nodespace://` URI, matching
   * the form the model uses to refer to nodes elsewhere).
   */
  nodeId: string;
  /**
   * Short human-readable title for the node, when available.
   */
  title?: string;
  /**
   * The node's type (e.g. `"task"`), when available.
   */
  nodeType?: string;
};
