// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Who sent a message in a native chat (ADR-088). A message written without
 * a role is the user's.
 */
export type AiChatMessageRole = 'user' | 'assistant' | 'system';
