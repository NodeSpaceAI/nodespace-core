// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * What kind of work a tool call does, as the agent classifies it. A kind the
 * vocabulary does not name is `other`.
 */
export type AiChatToolKind =
  | 'read'
  | 'edit'
  | 'delete'
  | 'move'
  | 'search'
  | 'execute'
  | 'think'
  | 'fetch'
  | 'switch_mode'
  | 'other';
