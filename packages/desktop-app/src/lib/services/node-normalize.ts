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
 * - `flattenTypedFieldsFromStorage` below, for the browser/dev-proxy HTTP
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
 * partial write doesn't drop sibling keys. Merges one level, plus one level
 * deeper into the type namespace (e.g. `properties.task.*`) so a nested patch
 * like `{ task: { status } }` doesn't clobber `properties.task.priority`.
 */
export function deepMergeProperties(
  existing: Record<string, unknown> | undefined,
  incoming: Record<string, unknown>,
  nodeType: string
): Record<string, unknown> {
  const base = existing ?? {};
  const merged: Record<string, unknown> = { ...base, ...incoming };

  const baseNs = base[nodeType];
  const incomingNs = incoming[nodeType];
  if (isPlainObject(baseNs) && isPlainObject(incomingNs)) {
    merged[nodeType] = { ...baseNs, ...incomingNs };
  }

  return merged;
}

/**
 * Compute the top-level typed fields to promote for an optimistic update.
 *
 * Only promotes a field that is actually present in this write — either flat
 * under `properties` (ai-chat stores `properties.model`) or nested under the
 * type namespace (`properties.task.status`). The "present in this write" guard
 * is load-bearing: it prevents overwriting an existing top-level value with
 * `undefined` when a caller omits a field (e.g. sending a message writes
 * `properties.messages` but not `properties.model`).
 */
export function promoteTypedFields(
  nodeType: string,
  changesProperties: Record<string, unknown>,
  mergedProperties: Record<string, unknown>
): Record<string, unknown> {
  const fields = OPTIMISTIC_TYPED_FIELDS[nodeType];
  if (!fields) return {};

  const nestedChanges = changesProperties[nodeType];
  const nestedMerged = mergedProperties[nodeType];
  const promoted: Record<string, unknown> = {};

  for (const { from, to } of fields) {
    if (Object.prototype.hasOwnProperty.call(changesProperties, from)) {
      // Flat shape (e.g. ai-chat: properties.model)
      promoted[to] = mergedProperties[from];
    } else if (
      isPlainObject(nestedChanges) &&
      Object.prototype.hasOwnProperty.call(nestedChanges, from)
    ) {
      // Nested shape (e.g. task schema form: properties.task.status)
      promoted[to] = isPlainObject(nestedMerged) ? nestedMerged[from] : undefined;
    }
  }

  return promoted;
}

/**
 * Promote a fetched node's namespaced typed-field bucket to top-level fields.
 *
 * Storage/wire shape from the browser HTTP transport is always namespaced
 * (`properties.<type>.*` — e.g. `properties['ai-chat'].model`), never flat:
 * unlike `promoteTypedFields` above (built for a partial WRITE payload, which
 * ai-chat sends flat), this reads a FULL fetched node's own bucket. This is
 * the browser-transport counterpart to the backend's `node_to_typed_value`
 * (`packages/nodespace-types/src/convert.rs`) — the Tauri IPC layer routes
 * every node through that function before it reaches the frontend, so
 * `nodeToAiChatNode`/`nodeToTaskNode` trust top-level fields are already
 * present and never read `properties.<type>` themselves. The dev-proxy HTTP
 * bridge (`packages/dev-tools/src/dev-proxy.ts`) has no access to that Rust
 * function and returns storage-shape `properties` verbatim, so it must call
 * this before handing a node to the frontend — otherwise a node's typed
 * fields silently read as `undefined` at the top level for that transport
 * only, even though the underlying data is intact.
 */
export function flattenTypedFieldsFromStorage(
  nodeType: string,
  properties: unknown
): Record<string, unknown> {
  const fields = OPTIMISTIC_TYPED_FIELDS[nodeType];
  if (!fields || !isPlainObject(properties)) return {};

  const bucket = properties[nodeType];
  if (!isPlainObject(bucket)) return {};

  const promoted: Record<string, unknown> = {};
  for (const { from, to } of fields) {
    if (Object.prototype.hasOwnProperty.call(bucket, from)) {
      promoted[to] = bucket[from];
    }
  }
  return promoted;
}
