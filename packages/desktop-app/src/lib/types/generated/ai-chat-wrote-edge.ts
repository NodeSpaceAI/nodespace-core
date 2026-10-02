// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { AiChatWrite } from './ai-chat-write';

/**
 * The fields of a `wrote` edge: every write the message made to the node,
 * in call order. One edge joins a message and a node, and a turn may write
 * the same node more than once (create it, then link it), so the edge holds
 * a list.
 */
export type AiChatWroteEdge = { writes: Array<AiChatWrite> };
