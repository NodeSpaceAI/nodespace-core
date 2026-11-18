// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { AiChatSessionStatus } from './ai-chat-session-status';
import type { NodeReference } from './node-reference';

/**
 * An `ai-chat-acp` node: a conversation with an external agent driven over
 * the Agent Client Protocol.
 *
 * Its messages are `ai-chat-message` children, as a native chat's are.
 */
export type AiChatAcpNode = {
  sessionStatus: AiChatSessionStatus;
  /**
   * The id the agent gave its own session, which its resume call takes.
   * Names state on this machine, so it never leaves it.
   */
  sessionId?: string;
  id: string;
  nodeType: string;
  content: string;
  version: number;
  createdAt: string;
  modifiedAt: string;
  properties: Record<string, unknown>;
  mentions?: Array<string>;
  mentionedIn?: Array<NodeReference>;
  title?: string | null;
  lifecycleStatus: string;
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
