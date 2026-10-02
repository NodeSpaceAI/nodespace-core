// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { AiChatProvider } from './ai-chat-provider';
import type { AiChatTurnStatus } from './ai-chat-turn-status';
import type { NodeReference } from './node-reference';

/**
 * An `ai-chat-native` node: a conversation run by NodeSpace's agent loop.
 */
export type AiChatNativeNode = {
  provider: AiChatProvider;
  turnStatus: AiChatTurnStatus;
  /**
   * Approximate token count of the conversation's context.
   */
  contextTokens: number;
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
