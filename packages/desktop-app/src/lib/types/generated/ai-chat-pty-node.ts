// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { AiChatSessionStatus } from './ai-chat-session-status';
import type { NodeReference } from './node-reference';

/**
 * An `ai-chat-pty` node: an external coding agent in a terminal.
 *
 * It has no messages: NodeSpace sees only the terminal's output stream. What
 * capture records when the session ends is the base's `summary` and the
 * fields below.
 */
export type AiChatPtyNode = {
  sessionStatus: AiChatSessionStatus;
  /**
   * The session's id. Names state on this machine, so it never leaves it.
   */
  sessionId?: string;
  /**
   * The raw terminal scrollback. Never leaves the machine.
   */
  transcript?: string;
  /**
   * The exit code of the session's process, once it has ended.
   */
  exitCode?: number;
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
