// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { AiChatCompletedWrite } from './ai-chat-completed-write';
import type { AiChatMessageRole } from './ai-chat-message-role';
import type { AiChatPendingDeletion } from './ai-chat-pending-deletion';
import type { AiChatResolvedEntity } from './ai-chat-resolved-entity';
import type { AiChatTurnOutcome } from './ai-chat-turn-outcome';

/**
 * A single message in an ai-chat conversation.
 */
export type AiChatMessage = {
  /**
   * Who sent the message.
   */
  role: AiChatMessageRole;
  /**
   * Message text.
   */
  content: string;
  /**
   * When the message was created (RFC3339), when known.
   */
  timestamp?: string;
  /**
   * Model chain-of-thought reasoning toward the answer, when captured.
   */
  reasoning?: string;
  /**
   * Graph writes this assistant turn completed. Empty for user messages and
   * for assistant turns that only read.
   */
  completedWrites?: Array<AiChatCompletedWrite>;
  /**
   * Concrete graph entities this assistant turn's read-only tool calls
   * surfaced (deduplicated by node id). Empty for user messages and for
   * assistant turns whose reads found nothing. See
   * [`AiChatResolvedEntity`].
   */
  resolvedEntities?: Array<AiChatResolvedEntity>;
  /**
   * The clarifying question, when this message is a `route_clarify` turn
   * (ADR-038) rather than an ordinary reply. `content` still carries the
   * flattened `"{opener}. {question}\n\n- opt1\n- opt2"` text for plain-text
   * readers and the LLM-facing history; this field plus `options` is the
   * same data unflattened, so the frontend can render clickable options
   * instead of parsing markdown bullets back out of prose.
   */
  question?: string;
  /**
   * Concrete options offered alongside `question`. Only meaningful when
   * `question` is `Some`.
   */
  options?: Array<string>;
  /**
   * Deletes this assistant turn is asking the user to confirm. Only the
   * user's next message can confirm them; see [`AiChatPendingDeletion`].
   */
  pendingDeletions?: Array<AiChatPendingDeletion>;
  /**
   * How this assistant turn ended, when an agent turn produced it. `None`
   * for user messages and for assistant text no turn produced (a failed
   * turn's error notice). See [`AiChatTurnOutcome`].
   */
  outcome?: AiChatTurnOutcome;
};
