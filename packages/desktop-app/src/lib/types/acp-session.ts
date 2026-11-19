/**
 * The live side of an ACP chat, as the viewer sees it.
 *
 * What is stored (messages, `session_status`, `session_id`) comes from the
 * chat's nodes. What exists only while a session runs is held in memory by
 * whatever drives the session and handed to the viewer as an
 * {@link AcpSessionSource}: the pending permission request, whether a turn is
 * running, the context usage and the controls. A viewer given no source is a
 * read-only view of the stored conversation.
 */

import type { AiChatToolKind } from './ai-chat-node';

/** What an option of a permission request does, as the agent classifies it. */
export type AcpPermissionOptionKind =
  | 'allow_once'
  | 'allow_always'
  | 'reject_once'
  | 'reject_always';

export interface AcpPermissionOption {
  optionId: string;
  name: string;
  kind: AcpPermissionOptionKind;
}

/** A request the agent is blocked on. In memory only; never stored. */
export interface AcpPermissionRequest {
  requestId: string;
  toolCallId?: string;
  toolTitle: string;
  toolKind?: AiChatToolKind;
  /** Every option the agent offered; the card shows only the once-only ones. */
  options: AcpPermissionOption[];
}

/** The answer to a permission request. */
export type AcpPermissionOutcome = { selected: string } | 'cancelled';

export interface AcpSessionUsage {
  /** Tokens in the context window. */
  used: number;
  /** The context window's size. */
  size: number;
}

/** The running session's state and controls. Getters are reactive. */
export interface AcpSessionSource {
  readonly pendingPermission: AcpPermissionRequest | null;
  readonly turnRunning: boolean;
  readonly usage: AcpSessionUsage | null;
  /** A problem the session reports (the agent exited, sign-in is needed). */
  readonly error: string | null;
  /** Each control is present only when the session supports it. */
  sendPrompt?(text: string): Promise<void>;
  cancelTurn?(): Promise<void>;
  answerPermission?(requestId: string, outcome: AcpPermissionOutcome): Promise<void>;
  endSession?(): Promise<void>;
  resumeSession?(): Promise<void>;
}
