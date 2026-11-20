// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * How a remote-tool confirmation stands. `pending` becomes one of the others
 * once, and stays there.
 */
export type AiChatConfirmationStatus = 'pending' | 'allowed' | 'denied' | 'expired' | 'cancelled';
