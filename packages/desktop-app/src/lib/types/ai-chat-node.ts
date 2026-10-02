/**
 * AI chat node helpers.
 *
 * The chat types and `AiChatMessageNode` are generated from Rust's
 * `nodespace-types` (`./generated`): the backend promotes a chat's fields to
 * the top level of the node for every transport.
 *
 * A native chat's messages are not a field of it: each is an `ai-chat-message`
 * child node (ADR-088 §3), converted with `nodeToAiChatMessageNode()`.
 *
 * `ai-chat` is an abstract base (ADR-088): no node is ever of exactly that
 * type. A chat is an `ai-chat-native` (NodeSpace's own agent loop answers) or
 * an `ai-chat-pty` (an external agent harness in a terminal), and both carry
 * the base's fields (`AiChatBase`).
 *
 * Always use `nodeToAiChatNativeNode()` / `nodeToAiChatPtyNode()` to convert a
 * generic Node from the store.
 */

import type { Node } from './node';
import type {
  AiChatBase,
  AiChatMessageNode,
  AiChatNativeNode,
  AiChatPtyNode
} from './generated';
import { isA, isExactly } from './core-node-types';

export type {
  AiChatBase,
  AiChatMessageNode,
  AiChatMessageRole,
  AiChatNativeNode,
  AiChatProvider,
  AiChatPtyNode,
  AiChatSessionStatus,
  AiChatTurnOutcome,
  AiChatTurnStatus
} from './generated';

export interface OpenAiCompatConfig {
  id: string; // uuid, generated client-side
  name: string; // user-provided display name (cosmetic only, never sent to the endpoint)
  baseUrl: string; // e.g. "https://api.openai.com/v1"
  apiKey: string; // stored on the daemon (~/.nodespace/daemon.toml, 0600)
  model: string; // wire-protocol "model" field, e.g. "gpt-4o" — required by the real OpenAI API
}

/** Any chat node. */
export type AiChatNode = AiChatNativeNode | AiChatPtyNode;

export function isAiChatNativeNode(node: Node | AiChatNode): node is AiChatNativeNode {
  return isExactly(node.nodeType, 'ai-chat-native');
}

export function isAiChatPtyNode(node: Node | AiChatNode): node is AiChatPtyNode {
  return isExactly(node.nodeType, 'ai-chat-pty');
}

export function isAiChatMessageNode(node: Node | AiChatMessageNode): node is AiChatMessageNode {
  return isExactly(node.nodeType, 'ai-chat-message');
}

/** Whether a node is a chat of either subtype. */
export function isAiChatNode(node: Node | AiChatNode): node is AiChatNode {
  return isA(node.nodeType, 'ai-chat');
}

/**
 * Convert a generic Node to AiChatNativeNode.
 *
 * The backend (`node_to_typed_value` in `nodespace-types`) is the single typing
 * authority: for every transport (Tauri IPC and HTTP/SSE) it promotes the
 * chat's fields to the TOP LEVEL of the node. This converter therefore trusts
 * the flat contract and only fills the schema defaults.
 */
export function nodeToAiChatNativeNode(node: Node): AiChatNativeNode {
  const chat = node as Node & Partial<AiChatNativeNode>;
  return {
    ...envelope(node),
    ...aiChatBase(node),
    provider: chat.provider ?? 'native',
    turnStatus: chat.turnStatus ?? 'idle',
    contextTokens: chat.contextTokens ?? 0
  };
}

/** Convert a generic Node to AiChatMessageNode; see {@link nodeToAiChatNativeNode}. */
export function nodeToAiChatMessageNode(node: Node): AiChatMessageNode {
  const message = node as Node & Partial<AiChatMessageNode>;
  return {
    ...envelope(node),
    role: message.role ?? 'user',
    timestamp: message.timestamp,
    reasoning: message.reasoning,
    outcome: message.outcome,
    options: Array.isArray(message.options) ? message.options : undefined
  };
}

/** Convert a generic Node to AiChatPtyNode; see {@link nodeToAiChatNativeNode}. */
export function nodeToAiChatPtyNode(node: Node): AiChatPtyNode {
  const chat = node as Node & Partial<AiChatPtyNode>;
  return {
    ...envelope(node),
    ...aiChatBase(node),
    sessionStatus: chat.sessionStatus ?? 'active',
    sessionId: chat.sessionId,
    transcript: chat.transcript,
    exitCode: chat.exitCode
  };
}

function envelope(node: Node) {
  return {
    id: node.id,
    nodeType: node.nodeType,
    content: node.content,
    version: node.version,
    lifecycleStatus: node.lifecycleStatus,
    createdAt: node.createdAt,
    modifiedAt: node.modifiedAt,
    properties: node.properties ?? {},
    title: node.title,
    mentions: node.mentions,
    mentionedIn: node.mentionedIn
  };
}

function aiChatBase(node: Node): AiChatBase {
  const chat = node as Node & Partial<AiChatBase>;
  return {
    agent: chat.agent ?? '',
    model: chat.model,
    summary: chat.summary,
    lastActive: chat.lastActive
  };
}
