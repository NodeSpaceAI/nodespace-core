// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Whether an inference turn is running on a native chat. Written by the
 * daemon, apart from the `processing` a client sets to request a turn.
 */
export type AiChatTurnStatus = 'idle' | 'processing';
