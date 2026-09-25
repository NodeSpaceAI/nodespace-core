import type { Node } from '$lib/types/node';
import { nodeToTaskNode } from '$lib/types/task-node';
import { nodeToPersonNode } from '$lib/types/person-node';
import { nodeToProjectNode } from '$lib/types/project-node';
import { nodeToAiChatNode } from '$lib/types/ai-chat-node';
import { TYPED_CORE_DEFAULTS, TYPED_CORE_FIELDS } from '$lib/types/typed-core-fields';

/**
 * Normalize raw node data from a sync boundary (Tauri domain events or SSE) to the
 * type-specific flat format expected by frontend stores and components.
 *
 * Single authoritative implementation — both sync paths (Tauri and browser) call this
 * so a future type branch (e.g. SchemaNode) is added in exactly one place.
 */
export function normalizeNodeData(nodeData: Node): Node {
  switch (nodeData.nodeType) {
    case 'task':
      return nodeToTaskNode(nodeData) as unknown as Node;
    case 'person':
      return nodeToPersonNode(nodeData) as unknown as Node;
    case 'project':
      return nodeToProjectNode(nodeData) as unknown as Node;
    case 'ai-chat':
      return nodeToAiChatNode(nodeData) as unknown as Node;
    default:
      return nodeData;
  }
}

/**
 * One ai-chat field the backend promotes: `from` is the property key the write
 * payload uses (what `changesProperties`/`mergedProperties` are keyed by), `to`
 * is the top-level `Node` key viewers read. They differ for the canonical
 * snake_case keys (`turn_status` → `turnStatus`) — see `ai_chat_node_to_value`
 * in `packages/nodespace-types/src/convert.rs`.
 */
interface PromotedField {
  from: string;
  to: string;
}

/**
 * ai-chat fields the backend lifts from `properties` to the top level while
 * also leaving them in `properties` — ai-chat writes them through the
 * generic properties path, unlike the typed core types (`task`, `person`,
 * `project`, see `TYPED_CORE_FIELDS`), whose core fields have exactly one home.
 *
 * Two consumers:
 * - `promoteTypedFields`, for an optimistic (pre-round-trip) `updateNode` —
 *   reflects these fields immediately instead of waiting a full RPC round
 *   trip. The backend response is spread over the node afterward, so drift
 *   here degrades optimistic latency only.
 * - `storageNodeToApiFields`, for the browser/dev-proxy transport. Drift there
 *   is NOT latency-only: a field this map omits never reaches the top level
 *   over that transport (e.g. `AiChatNodeViewer`'s `node?.provider`).
 *
 * Keep in sync with convert.rs.
 */
export const OPTIMISTIC_TYPED_FIELDS: Record<string, readonly PromotedField[]> = {
  'ai-chat': [
    { from: 'turn_status', to: 'turnStatus' },
    { from: 'session_status', to: 'sessionStatus' },
    { from: 'provider', to: 'provider' },
    { from: 'model', to: 'model' },
    { from: 'messages', to: 'messages' }
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
 * Mirror `normalize_date_field`: a `YYYY-MM-DD` date passes through and an
 * RFC 3339 datetime reduces to its date in its own offset — the literal date
 * prefix. Only `T`-separated datetimes are reduced, the only form written.
 */
function normalizeDate(value: string): string {
  return /^\d{4}-\d{2}-\d{2}T/.test(value) ? value.slice(0, 10) : value;
}

/**
 * Convert a node's storage-shape `properties` into the API shape the frontend
 * reads: `properties` flattened, typed core fields moved to the top level.
 *
 * This is the browser-transport counterpart to the backend's
 * `node_to_typed_value` (`packages/nodespace-types/src/convert.rs`), which the
 * Tauri IPC layer routes every node through. The dev-proxy HTTP bridge
 * (`packages/dev-tools/src/dev-proxy.ts`) has no access to that Rust function
 * and receives storage-shape `properties` (`{ person: { first_name } }`) from
 * gRPC, so it must call this before handing a node to the frontend. Without it
 * the two transports deliver different shapes. Keep in sync with convert.rs:
 *
 * - Flattening mirrors `flatten_namespaced_properties`: when the type's own
 *   bucket is present, its non-`_` keys become the properties (object-valued
 *   fields included); otherwise the bag is already flat and only its
 *   non-object, non-`_` keys survive — a nested object there can only be
 *   another type's dormant namespace.
 * - Typed core types (`TYPED_CORE_FIELDS`) move each core field to its typed
 *   key — read under either spelling, typed key first, as the Rust converters
 *   do — normalize dates, fill the backend's defaults, and drop both
 *   spellings from `properties`.
 * - ai-chat promotes its fields and leaves them in `properties`.
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

  const promoted: Record<string, unknown> = { ...(TYPED_CORE_DEFAULTS[nodeType] ?? {}) };
  for (const { storage, wire, date } of TYPED_CORE_FIELDS[nodeType] ?? []) {
    const raw = properties[wire] ?? properties[storage];
    if (typeof raw === 'string') {
      promoted[wire] = date ? normalizeDate(raw) : raw;
    }
    delete properties[storage];
    delete properties[wire];
  }
  for (const { from, to } of OPTIMISTIC_TYPED_FIELDS[nodeType] ?? []) {
    if (Object.prototype.hasOwnProperty.call(properties, from)) {
      promoted[to] = properties[from];
    }
  }
  return { ...promoted, properties };
}
