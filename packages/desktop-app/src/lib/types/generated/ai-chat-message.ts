// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { AiChatCompletedWrite } from './ai-chat-completed-write';
import type { AiChatPendingDeletion } from './ai-chat-pending-deletion';
import type { AiChatResolvedEntity } from './ai-chat-resolved-entity';
import type { AiChatTurnOutcome } from './ai-chat-turn-outcome';

/**
 * A single message in an ai-chat conversation.
 */
export type AiChatMessage = {
  role: string;
  content: string;
  timestamp?: string;
  reasoning?: string;
  /**
   * Graph writes this assistant turn completed.
   */
  completedWrites?: Array<AiChatCompletedWrite>;
  /**
   * Graph entities this assistant turn's reads surfaced.
   */
  resolvedEntities?: Array<AiChatResolvedEntity>;
  /**
   * The clarifying question, when this message is a `route_clarify` turn
   * (ADR-038) rather than an ordinary reply. `content` still carries the
   * flattened text; this plus `options` is the same data unflattened, for
   * the frontend to render clickable options with.
   */
  question?: string;
  /**
   * Concrete options offered alongside `question`. Only meaningful when
   * `question` is `Some`.
   */
  options?: Array<string>;
  /**
   * Deletes this assistant turn is asking the user to confirm.
   */
  pendingDeletions?: Array<AiChatPendingDeletion>;
  /**
   * How this assistant turn ended, when an agent turn produced it.
   * Mirrors `nodespace_core::models::AiChatTurnOutcome`.
   */
  outcome?: AiChatTurnOutcome;
};
