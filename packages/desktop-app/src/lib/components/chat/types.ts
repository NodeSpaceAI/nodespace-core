/**
 * View-model types for the chat UI components.
 *
 * `DisplayMessage` is the UI-facing message shape rendered by `ChatMessage` and
 * owned by `AiChatNativeNodeViewer`. It is deliberately distinct from the two other
 * message shapes in play (see [[project_frontend_type_layering]]):
 *   - `ChatMessage` (`$lib/types/agent-types`) — the protocol/wire shape.
 *   - `AiChatMessageNode` (`$lib/types/ai-chat-node`) — the persisted
 *     `ai-chat-message` child node of a chat (ADR-088 §3).
 * These three do NOT converge; converters bridge them at the viewer boundary.
 */

import type { ChatMessage, ToolExecutionRecord } from '$lib/types/agent-types';

/** UI display message with tool executions and streaming state. */
export interface DisplayMessage {
  /** The message node's id; `streaming` for the in-flight overlay. */
  readonly id: string;
  readonly role: ChatMessage['role'];
  content: string;
  readonly toolExecutions: ToolExecutionRecord[];
  readonly timestamp: number;
  /** Model chain-of-thought, rendered as a collapsible section under the answer. */
  readonly reasoning?: string;
  /**
   * The choices of a structured clarification (ADR-038). `content` carries the
   * flattened text ("opener. question" then a bullet list); when `options` is
   * non-empty the UI renders them as clickable choices instead of relying on
   * markdown bullet prose.
   */
  readonly options?: string[];
}
