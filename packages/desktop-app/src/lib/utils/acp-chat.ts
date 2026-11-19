/**
 * Pure rules of the ACP chat viewer: which row a stored message renders as,
 * which permission choices the card offers, and the words for tool state.
 */

import type {
  AiChatMessageNode,
  AiChatToolKind,
  AiChatToolStatus
} from '$lib/types/ai-chat-node';
import type { AcpPermissionOption, AcpPermissionOptionKind } from '$lib/types/acp-session';

export type AcpRowKind = 'user' | 'text' | 'thought' | 'plan' | 'tool' | 'notice';

/** Which row an ACP chat message renders as. */
export function acpRowKind(message: AiChatMessageNode): AcpRowKind {
  if (message.role === 'user') return 'user';
  if (message.role === 'system') return 'notice';
  switch (message.partKind) {
    case 'thought':
      return 'thought';
    case 'plan':
      return 'plan';
    case 'tool_call':
      return 'tool';
    default:
      return 'text';
  }
}

const ONCE_ONLY: ReadonlySet<AcpPermissionOptionKind> = new Set(['allow_once', 'reject_once']);

/**
 * The choices a permission card shows: only the once-only ones. An "always"
 * choice makes the agent write a standing rule into its own settings, so
 * NodeSpace never offers one; a kind it does not recognise is not shown.
 */
export function onceOnlyOptions(options: readonly AcpPermissionOption[]): AcpPermissionOption[] {
  return options.filter((option) => ONCE_ONLY.has(option.kind));
}

export function permissionOptionKindLabel(kind: AcpPermissionOptionKind): string {
  return kind === 'allow_once' ? 'Allow once' : 'Reject once';
}

const TOOL_STATUS_LABELS: Record<AiChatToolStatus, string> = {
  pending: 'Waiting',
  in_progress: 'Running',
  completed: 'Done',
  failed: 'Failed',
  cancelled: 'Cancelled'
};

/** A tool call's status in words; a call with no status yet is waiting. */
export function toolStatusLabel(status: AiChatToolStatus | undefined): string {
  return TOOL_STATUS_LABELS[status ?? 'pending'];
}

/** Whether the call is still going: pending or running. */
export function isToolActive(status: AiChatToolStatus | undefined): boolean {
  return status === undefined || status === 'pending' || status === 'in_progress';
}

const TOOL_KIND_LABELS: Record<AiChatToolKind, string> = {
  read: 'Read',
  edit: 'Edit',
  delete: 'Delete',
  move: 'Move',
  search: 'Search',
  execute: 'Execute',
  think: 'Think',
  fetch: 'Fetch',
  switch_mode: 'Switch mode',
  other: 'Tool'
};

export function toolKindLabel(kind: AiChatToolKind | undefined): string {
  return kind ? TOOL_KIND_LABELS[kind] : 'Tool';
}
