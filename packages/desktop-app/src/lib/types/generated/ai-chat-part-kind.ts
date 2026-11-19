// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * What one message of an ACP chat is, when it is one part of an agent's
 * turn. A message without a part kind is a whole message: a native chat's, a
 * user's, or a system notice.
 */
export type AiChatPartKind = 'text' | 'thought' | 'tool_call' | 'plan';
