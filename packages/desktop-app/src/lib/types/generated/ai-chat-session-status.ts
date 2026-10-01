// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Whether a terminal chat's session is running. A finished session is
 * `ended`; resuming one sets it back to `active`.
 */
export type AiChatSessionStatus = 'active' | 'ended';
