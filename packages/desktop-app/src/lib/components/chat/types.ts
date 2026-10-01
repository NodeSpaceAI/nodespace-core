/**
 * View-model types for the chat UI components.
 *
 * `DisplayMessage` is the UI-facing message shape rendered by `ChatMessage` and
 * owned by `AiChatNativeNodeViewer`. It is deliberately distinct from the two other
 * message shapes in play (see [[project_frontend_type_layering]]):
 *   - `ChatMessage` (`$lib/types/agent-types`) — the protocol/wire shape.
 *   - the ADR-028 persisted JSON shape in `ai-chat-native` node `messages`.
 * These three do NOT converge; converters bridge them at the viewer boundary.
 */

import type { ChatMessage, ToolExecutionRecord } from '$lib/types/agent-types';

/** UI display message with tool executions and streaming state. */
export interface DisplayMessage {
  readonly id: string;
  readonly role: ChatMessage['role'];
  content: string;
  readonly toolExecutions: ToolExecutionRecord[];
  readonly timestamp: number;
  /** Model chain-of-thought, rendered as a collapsible section under the answer. */
  readonly reasoning?: string;
  /**
   * A `route_clarify` question (ADR-038), when this turn is a structured
   * clarification rather than an ordinary reply. `content` still carries the
   * flattened text as a fallback; when `options` is non-empty the UI renders
   * them as clickable choices instead of relying on markdown bullet prose.
   */
  readonly question?: string;
  readonly options?: string[];
}
