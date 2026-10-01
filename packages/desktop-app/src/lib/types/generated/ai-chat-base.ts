// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * The fields every chat carries, declared by the abstract `ai-chat` schema
 * and embedded by each subtype's struct.
 */
export type AiChatBase = {
  /**
   * Who runs the conversation: [`NODESPACE_AGENT`], or a terminal harness.
   */
  agent: string;
  /**
   * The model identifier, when known. Never a harness name.
   */
  model?: string;
  /**
   * Prose summary of the conversation, filled per subtype.
   */
  summary?: string;
  /**
   * When the chat was last active (RFC 3339).
   */
  lastActive?: string;
};
