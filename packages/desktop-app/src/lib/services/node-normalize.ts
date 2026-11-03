import type { Node } from '$lib/types/node';
import { nodeToTaskNode } from '$lib/types/task-node';
import { nodeToIssueNode } from '$lib/types/issue-node';
import { nodeToPersonNode } from '$lib/types/person-node';
import { nodeToProjectNode } from '$lib/types/project-node';
import { nodeToSoftwareProjectNode } from '$lib/types/software-project-node';
import { nodeToSpecNode } from '$lib/types/spec-node';
import { nodeToPlanNode } from '$lib/types/plan-node';
import { nodeToDecisionNode } from '$lib/types/decision-node';
import { nodeToQueryNode } from '$lib/types/query';
import {
  nodeToAiChatMessageNode,
  nodeToAiChatNativeNode,
  nodeToAiChatPtyNode
} from '$lib/types/ai-chat-node';
import { hasTypedUpdate, TYPED_CORE_FIELDS } from '$lib/types/typed-core-fields';

/** The typed wire shape each type with one is converted to, keyed by exact type. */
const TYPED_WIRE_CONVERTERS: ReadonlyMap<string, (node: Node) => Node> = new Map([
  ['task', (node) => nodeToTaskNode(node) as unknown as Node],
  ['issue', (node) => nodeToIssueNode(node) as unknown as Node],
  ['person', (node) => nodeToPersonNode(node) as unknown as Node],
  ['project', (node) => nodeToProjectNode(node) as unknown as Node],
  ['software-project', (node) => nodeToSoftwareProjectNode(node) as unknown as Node],
  ['spec', (node) => nodeToSpecNode(node) as unknown as Node],
  ['plan', (node) => nodeToPlanNode(node) as unknown as Node],
  ['decision', (node) => nodeToDecisionNode(node) as unknown as Node],
  ['query', (node) => nodeToQueryNode(node) as unknown as Node],
  ['ai-chat-native', (node) => nodeToAiChatNativeNode(node) as unknown as Node],
  ['ai-chat-pty', (node) => nodeToAiChatPtyNode(node) as unknown as Node],
  ['ai-chat-message', (node) => nodeToAiChatMessageNode(node) as unknown as Node]
]);

/**
 * Normalize raw node data from a sync boundary (Tauri domain events) to the
 * type-specific flat format expected by frontend stores and components.
 *
 * Single authoritative implementation, so a future type branch (e.g. SchemaNode) is
 * added in exactly one place.
 */
export function normalizeNodeData(nodeData: Node): Node {
  // Keyed by exact type: a wire shape is never borrowed by a subtype, so a
  // user-defined `chore extends task` stays the generic node, and the core
  // subtype `issue` has a converter of its own.
  const toTyped = TYPED_WIRE_CONVERTERS.get(nodeData.nodeType);
  return toTyped ? toTyped(nodeData) : nodeData;
}

/**
 * Merge an incoming `properties` patch onto the existing `properties` bag so a
 * partial write doesn't drop sibling keys. Both bags are flat, so a one-level
 * merge is complete.
 */
export function mergeProperties(
  existing: Record<string, unknown> | undefined,
  incoming: Record<string, unknown>
): Record<string, unknown> {
  return { ...(existing ?? {}), ...incoming };
}

/**
 * Compute the top-level typed fields to promote for an optimistic update.
 *
 * Applies to a type whose typed fields are written as generic `properties`
 * patches (the chat family, see `hasTypedUpdate`) — the property key a write
 * uses is the field's storage name (`turn_status`), the key viewers read is its
 * wire name (`turnStatus`). Reflecting the write immediately instead of
 * waiting a full RPC round trip degrades latency only if it drifts: the
 * backend response is spread over the node afterward.
 *
 * Only promotes a field that is actually present in this write. That guard is
 * load-bearing: it prevents overwriting an existing top-level value with
 * `undefined` when a caller omits a field (e.g. sending a message writes
 * `properties.turn_status` but not `properties.model`).
 */
export function promoteTypedFields(
  nodeType: string,
  changesProperties: Record<string, unknown>,
  mergedProperties: Record<string, unknown>
): Record<string, unknown> {
  const promoted: Record<string, unknown> = {};
  if (hasTypedUpdate(nodeType)) return promoted;
  for (const { storage, wire } of TYPED_CORE_FIELDS[nodeType] ?? []) {
    if (Object.prototype.hasOwnProperty.call(changesProperties, storage)) {
      promoted[wire] = mergedProperties[storage];
    }
  }
  return promoted;
}
