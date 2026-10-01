/**
 * AI chat node helpers.
 *
 * `AiChatNode`, `AiChatMessage` and the records a message carries are
 * generated from Rust's `nodespace-types` (`./generated`): the backend promotes
 * the ai-chat fields to the top level of the node for every transport.
 *
 * Always use `nodeToAiChatNode()` to convert a generic Node from the store.
 */

import type { Node } from './node';
import type { AiChatNode } from './generated';
import { isExactly } from './core-node-types';

export type {
  AiChatCompletedWrite,
  AiChatMessage,
  AiChatNode,
  AiChatPendingDeletion,
  AiChatResolvedEntity,
  AiChatTurnOutcome
} from './generated';

/**
 * The values `AiChatNode.provider` takes. Mirrors `AI_CHAT_PROVIDERS` in
 * `packages/core/src/models/ai_chat_node.rs`, which is what the backend schema
 * enum and validation accept.
 */
export type AiChatProvider = 'native' | 'openai-compat' | 'pty';

export interface OpenAiCompatConfig {
  id: string; // uuid, generated client-side
  name: string; // user-provided display name (cosmetic only, never sent to the endpoint)
  baseUrl: string; // e.g. "https://api.openai.com/v1"
  apiKey: string; // stored on the daemon (~/.nodespace/daemon.toml, 0600)
  model: string; // wire-protocol "model" field, e.g. "gpt-4o" — required by the real OpenAI API
}

export function isAiChatNode(node: Node | AiChatNode): node is AiChatNode {
  return isExactly(node.nodeType, 'ai-chat');
}

/**
 * Convert a generic Node to AiChatNode.
 *
 * The backend (`node_to_typed_value` in `nodespace-types`) is the single typing
 * authority: for every transport (Tauri IPC and HTTP/SSE) it promotes ai-chat
 * fields to the TOP LEVEL of the node and flattens the `properties.ai-chat`
 * namespace away. See the `wire_contract` tests in `nodespace-types/src/convert.rs`.
 * This converter therefore trusts the flat contract and only fills defaults.
 */
export function nodeToAiChatNode(node: Node): AiChatNode {
  const chat = node as Node & Partial<AiChatNode>;
  return {
    id: node.id,
    nodeType: 'ai-chat',
    content: node.content,
    version: node.version,
    lifecycleStatus: node.lifecycleStatus,
    createdAt: node.createdAt,
    modifiedAt: node.modifiedAt,
    properties: node.properties ?? {},
    title: node.title,
    mentions: node.mentions,
    mentionedIn: node.mentionedIn,
    turnStatus: chat.turnStatus ?? 'idle',
    sessionStatus: chat.sessionStatus ?? 'active',
    provider: chat.provider,
    model: chat.model,
    messages: Array.isArray(chat.messages) ? chat.messages : []
  };
}
