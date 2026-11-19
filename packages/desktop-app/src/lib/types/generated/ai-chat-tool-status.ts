// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Where a tool call stands. The agent reports the first four; `cancelled`
 * is set by NodeSpace on a call the agent never finished when the turn was
 * cancelled.
 */
export type AiChatToolStatus = 'pending' | 'in_progress' | 'completed' | 'failed' | 'cancelled';
