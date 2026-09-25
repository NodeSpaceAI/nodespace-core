import type { Node } from '$lib/types/node';
import { nodeToTaskNode } from '$lib/types/task-node';
import { nodeToAiChatNode } from '$lib/types/ai-chat-node';

/**
 * Normalize raw node data from a sync boundary (Tauri domain events or SSE) to the
 * type-specific flat format expected by frontend stores and components.
 *
 * Single authoritative implementation — both sync paths (Tauri and browser) call this
 * so a future type branch (e.g. SchemaNode) is added in exactly one place.
 */
export function normalizeNodeData(nodeData: Node): Node {
  if (nodeData.nodeType === 'task') {
    return nodeToTaskNode(nodeData) as unknown as Node;
  }
  if (nodeData.nodeType === 'ai-chat') {
    return nodeToAiChatNode(nodeData) as unknown as Node;
  }
  return nodeData;
}

/**
 * One promoted field: `from` is the property key the write payload actually
 * uses (what `changesProperties`/`mergedProperties` are keyed by), `to` is the
 * top-level `Node` key viewers read. The two differ for ai-chat's canonical
 * snake_case property keys (`turn_status`, `session_status`), which the
 * backend promotes to camelCase top-level fields (`turnStatus`,
 * `sessionStatus`) — see `ai_chat_node_to_value` in
 * `packages/nodespace-types/src/convert.rs`. They're equal everywhere else.
 */
interface PromotedField {
  from: string;
  to: string;
}

/**
 * Mirror of the backend's typed-field promotion (`node_to_typed_value` /
 * `flatten_properties_for_api` in `packages/nodespace-types/src/convert.rs`).
 * For each node type, lists the type-specific fields the backend lifts from
 * the stored `properties` bag up to the TOP LEVEL of the node (the fields
 * viewers actually read).
 *
 * Two independent consumers:
 * - `promoteTypedFields` below, for an optimistic (pre-round-trip)
 *   `updateNode` — reflects these fields immediately instead of waiting a
 *   full RPC round trip. The backend response is always spread over the node
 *   afterward, so drift here degrades optimistic latency only.
 * - `storageNodeToApiFields` below, for the browser/dev-proxy HTTP
 *   transport (`packages/dev-tools/src/dev-proxy.ts`), which has no access to
 *   `node_to_typed_value` (Rust) and returns nodes straight from storage
 *   shape. Drift here is NOT latency-only — a promoted field this map omits
 *   never reaches the top level over that transport at all, silently
 *   breaking any viewer that reads it (e.g. `AiChatNodeViewer`'s
 *   `node?.provider`/`node?.model`).
 *
 * Keep in sync with convert.rs when the promoted field set changes.
 */
export const OPTIMISTIC_TYPED_FIELDS: Record<string, readonly PromotedField[]> = {
  'ai-chat': [
    { from: 'turn_status', to: 'turnStatus' },
    { from: 'session_status', to: 'sessionStatus' },
    { from: 'provider', to: 'provider' },
    { from: 'model', to: 'model' },
    { from: 'messages', to: 'messages' }
  ],
  task: [
    { from: 'status', to: 'status' },
    { from: 'priority', to: 'priority' },
    { from: 'dueDate', to: 'dueDate' },
    { from: 'startedAt', to: 'startedAt' },
    { from: 'completedAt', to: 'completedAt' }
  ]
};

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * Merge an incoming `properties` patch onto the existing `properties` bag so a
 * partial write doesn't drop sibling keys. Both bags are flat (see
 * `storageNodeToApiFields`), so a one-level merge is complete.
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
 * Only promotes a field that is actually present in this write. That guard is
 * load-bearing: it prevents overwriting an existing top-level value with
 * `undefined` when a caller omits a field (e.g. sending a message writes
 * `properties.messages` but not `properties.model`).
 */
export function promoteTypedFields(
  nodeType: string,
  changesProperties: Record<string, unknown>,
  mergedProperties: Record<string, unknown>
): Record<string, unknown> {
  const promoted: Record<string, unknown> = {};
  for (const { from, to } of OPTIMISTIC_TYPED_FIELDS[nodeType] ?? []) {
    if (Object.prototype.hasOwnProperty.call(changesProperties, from)) {
      promoted[to] = mergedProperties[from];
    }
  }
  return promoted;
}

/**
 * Convert a node's storage-shape `properties` into the API shape the frontend
 * reads: `properties` flattened, plus typed fields promoted to the top level.
 *
 * This is the browser-transport counterpart to the backend's
 * `node_to_typed_value` (`packages/nodespace-types/src/convert.rs`), which the
 * Tauri IPC layer routes every node through. The dev-proxy HTTP bridge
 * (`packages/dev-tools/src/dev-proxy.ts`) has no access to that Rust function
 * and receives storage-shape `properties` (`{ person: { first_name } }`) from
 * gRPC, so it must call this before handing a node to the frontend. Without it
 * the two transports deliver different shapes and every reader has to guess
 * which one it got.
 *
 * The flattening mirrors `flatten_namespaced_properties` exactly: when the
 * type's own bucket is present, its non-`_` keys become the properties
 * (object-valued fields included); otherwise the bag is already flat and only
 * its non-object, non-`_` keys survive — a nested object there can only be
 * another type's dormant namespace. Keep in sync with convert.rs.
 */
export function storageNodeToApiFields(
  nodeType: string,
  storageProperties: unknown
): { properties: Record<string, unknown> } & Record<string, unknown> {
  const properties: Record<string, unknown> = {};
  if (isPlainObject(storageProperties)) {
    const bucket = storageProperties[nodeType];
    if (isPlainObject(bucket)) {
      for (const [key, value] of Object.entries(bucket)) {
        if (!key.startsWith('_')) properties[key] = value;
      }
    } else {
      for (const [key, value] of Object.entries(storageProperties)) {
        if (!key.startsWith('_') && !isPlainObject(value)) properties[key] = value;
      }
    }
  }

  const promoted: Record<string, unknown> = {};
  for (const { from, to } of OPTIMISTIC_TYPED_FIELDS[nodeType] ?? []) {
    if (Object.prototype.hasOwnProperty.call(properties, from)) {
      promoted[to] = properties[from];
    }
  }
  return { ...promoted, properties };
}
