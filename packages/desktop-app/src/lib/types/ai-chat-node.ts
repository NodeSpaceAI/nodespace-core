import type { Node, NodeEnvelope } from './node';
import { isExactly } from './core-node-types';

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

/**
 * Mirrors `AI_CHAT_PROVIDERS` in `packages/core/src/models/ai_chat_node.rs`,
 * which is what the backend schema enum and validation accept.
 */
export type AiChatProvider ='native' | 'openai-compat' | 'pty';

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
   * Edges the write evicted, rendered `"from -[type]-> to"`. Only a
   * cardinality-one `create_relationship` populates it.
   */
  replaced?: string[];
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

/**
 * A node an agent turn asked to delete, held until the user confirms it.
 * `version` and `descendantCount` are what the user was shown; a change to
 * either before the answer aborts the delete.
 */
export interface AiChatPendingDeletion {
  nodeId: string;
  title: string;
  nodeType: string;
  version: number;
  descendantCount: number;
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
  /**
   * Deletes this assistant turn asks the user to confirm. Only an affirmative
   * reply to this message deletes them, against exactly these ids.
   */
  pendingDeletions?: AiChatPendingDeletion[];
  /**
   * How this assistant turn ended, when an agent turn produced it: a write
   * succeeded (or a delete was put up for confirmation), it asked a composed
   * clarifying question, or it replied without changing anything.
   */
  outcome?: 'acted' | 'clarified' | 'replied';
}

/**
 * AiChatNode - typed interface for ai-chat nodes.
 *
 * Flat structure matching the wire format (daemon flattens the 'ai-chat' namespace
 * via flatten_properties_for_api before sending over gRPC/Tauri).
 *
 * Always use nodeToAiChatNode() to convert a generic Node from the store.
 */
export interface AiChatNode extends NodeEnvelope {
  nodeType: 'ai-chat';

  turnStatus: AiChatTurnStatus;
  sessionStatus: AiChatSessionStatus;
  provider?: AiChatProvider;
  model?: string;
  messages: AiChatMessage[];
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
  const chat = node as unknown as Partial<AiChatNode>;
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
    messages: Array.isArray(chat.messages) ? chat.messages : [],
  };
}
