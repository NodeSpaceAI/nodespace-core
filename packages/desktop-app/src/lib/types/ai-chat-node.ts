import type { Node } from './node';

/**
 * Inference turn state, daemon-owned. `idle` is what the daemon writes when a
 * turn completes or is reset (`append_assistant_message` /
 * `write_ai_chat_turn_status`); the frontend writes `processing` to trigger a
 * turn. The viewer only ever tests for `processing`, so any other member
 * simply clears the typing indicator.
 *
 * Independent of {@link AiChatSessionStatus} — these used to share one
 * `status` key, which made a session archived mid-turn unrepresentable.
 */
export type AiChatTurnStatus = 'idle' | 'processing';

/**
 * Session lifecycle, PTY-owned. `active` is the initial state of a node that
 * has never run a session; `archived` is set by capture backfill once a PTY
 * session ends.
 *
 * Independent of {@link AiChatTurnStatus} — see that type's doc for why.
 */
export type AiChatSessionStatus = 'active' | 'archived';

export type AiChatProvider = 'native' | 'openai' | 'openai-compat' | 'pty';

export interface OpenAiCompatConfig {
  id: string;       // uuid, generated client-side
  name: string;     // user-provided display name (cosmetic only, never sent to the endpoint)
  baseUrl: string;  // e.g. "https://api.openai.com/v1"
  apiKey: string;   // stored on the daemon (~/.nodespace/daemon.toml, 0600)
  model: string;    // wire-protocol "model" field, e.g. "gpt-4o" — required by the real OpenAI API
}

/**
 * A graph write completed during an assistant turn.
 *
 * Persisted so the next turn can tell a satisfied instruction from a pending
 * one — the agent session is rebuilt from these messages on every turn.
 */
export interface AiChatCompletedWrite {
  /** Tool that performed the write, e.g. 'create_node'. */
  tool: string;
  /** Node the write produced or affected, when the tool reported one. */
  nodeId?: string;
  /** Short label for the written node, when available. */
  summary?: string;
  /**
   * The call's arguments, canonicalised. With `tool`, this is the write's
   * identity for the backend's cross-turn duplicate guard.
   *
   * Two forms: the canonical JSON verbatim when small enough to store, or
   * `sha256:<hex>` of it when not — which keeps a large write (an entire
   * markdown import, say) guarded without copying its content into this
   * message history a second time. Always present; treat it as opaque.
   */
  canonicalArgs: string;
}

/**
 * A concrete graph entity a read-only tool call surfaced during an assistant
 * turn (e.g. a `search_nodes` result the reply refers to). Minimal identity
 * only, so the next turn can resolve "that"/"it" back to a node id even
 * though only prose survives from a read-only turn otherwise.
 */
export interface AiChatResolvedEntity {
  /** Node the read tool surfaced. */
  nodeId: string;
  /** Short title for the node, when available. */
  title?: string;
  /** The node's type (e.g. 'task'), when available. */
  nodeType?: string;
}

export interface AiChatMessage {
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp?: string;
  /** Model chain-of-thought reasoning toward the answer, when captured. */
  reasoning?: string;
  /** Graph writes this assistant turn completed. Absent when the turn only read. */
  completedWrites?: AiChatCompletedWrite[];
  /** Graph entities this assistant turn's reads surfaced. */
  resolvedEntities?: AiChatResolvedEntity[];
  /**
   * The clarifying question, when this message is a `route_clarify` turn
   * (ADR-038) rather than an ordinary reply. `content` still carries the
   * flattened `"{opener}. {question}\n\n- opt1\n- opt2"` text; this plus
   * `options` is the same data unflattened, so the UI can render clickable
   * options instead of parsing markdown bullets back out of prose.
   */
  question?: string;
  /** Concrete options offered alongside `question`. */
  options?: string[];
}

/**
 * AiChatNode - typed interface for ai-chat nodes.
 *
 * Flat structure matching the wire format (daemon flattens the 'ai-chat' namespace
 * via flatten_properties_for_api before sending over gRPC/Tauri).
 *
 * Always use nodeToAiChatNode() to convert a generic Node from the store.
 */
export interface AiChatNode {
  id: string;
  nodeType: 'ai-chat';
  content: string;
  version: number;
  lifecycleStatus?: string;
  createdAt: string;
  modifiedAt: string;

  turnStatus: AiChatTurnStatus;
  sessionStatus: AiChatSessionStatus;
  provider?: AiChatProvider;
  model?: string;
  messages: AiChatMessage[];
}

export function isAiChatNode(node: Node | AiChatNode): node is AiChatNode {
  return node.nodeType === 'ai-chat';
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
  const chat = node as unknown as Partial<AiChatNode> & { lifecycleStatus?: string };
  return {
    id: node.id,
    nodeType: 'ai-chat',
    content: node.content,
    version: node.version,
    lifecycleStatus: chat.lifecycleStatus,
    createdAt: node.createdAt,
    modifiedAt: node.modifiedAt,
    turnStatus: chat.turnStatus ?? 'idle',
    sessionStatus: chat.sessionStatus ?? 'active',
    provider: chat.provider,
    model: chat.model,
    messages: Array.isArray(chat.messages) ? chat.messages : [],
  };
}
