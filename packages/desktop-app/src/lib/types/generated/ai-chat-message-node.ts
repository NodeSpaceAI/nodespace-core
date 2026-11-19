// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { AiChatMessageRole } from './ai-chat-message-role';
import type { AiChatPartKind } from './ai-chat-part-kind';
import type { AiChatToolKind } from './ai-chat-tool-kind';
import type { AiChatToolStatus } from './ai-chat-tool-status';
import type { AiChatTurnOutcome } from './ai-chat-turn-outcome';
import type { NodeReference } from './node-reference';

/**
 * An `ai-chat-message` node: one message of a native chat (ADR-088 §3).
 *
 * A `has_child` child of its chat, in conversation order. Its text is its
 * `content`; a clarifying question is the content too, with the choices it
 * offers in `options`. What the message wrote, looked up or asked to delete
 * is on its `wrote`, `resolved` and `pending_delete` edges.
 */
export type AiChatMessageNode = {
  /**
   * Who sent the message.
   */
  role: AiChatMessageRole;
  /**
   * When the message was sent (RFC 3339), when known.
   */
  timestamp?: string;
  /**
   * The model's chain-of-thought toward the answer, when captured.
   */
  reasoning?: string;
  /**
   * How the turn that produced an assistant message ended. `None` for a
   * user message and for assistant text no turn produced (a failed turn's
   * error notice).
   */
  outcome?: AiChatTurnOutcome;
  /**
   * The choices offered with a clarifying question.
   */
  options?: Array<string>;
  /**
   * Which part of an agent's turn this message is, in an ACP chat. `None`
   * for a whole message.
   */
  partKind?: AiChatPartKind;
  /**
   * The agent's id for the tool call this message records; an update to the
   * call finds its message by it.
   */
  toolCallId?: string;
  /**
   * The tool call's title as the agent gave it. Raw inputs and outputs are
   * never stored.
   */
  toolTitle?: string;
  /**
   * What kind of work the tool call does.
   */
  toolKind?: AiChatToolKind;
  /**
   * Where the tool call stands.
   */
  toolStatus?: AiChatToolStatus;
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
};
