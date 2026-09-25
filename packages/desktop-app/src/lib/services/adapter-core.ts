/**
 * Transport-agnostic backend adapter core.
 *
 * The daemon's NodeService contract (packages/proto/proto/node_service.proto) is
 * consumed by three independent client paths — TauriAdapter (IPC), HttpAdapter
 * (fetch), and the dev-proxy (REST→gRPC translation) — plus a fourth copy that
 * used to live in the e2e harness. This module is the single place that encodes
 * *what the wire contract is*: which fields are required vs. optional-omit vs.
 * tri-state-clearable, and what each logical operation is named per transport.
 * TauriAdapter/HttpAdapter/dev-proxy call these builders instead of re-deriving
 * the encoding by hand, so a field/shape change can no longer drift between them
 * silently — it becomes one function to update, used everywhere.
 */

// This file is imported directly by packages/dev-tools/src/dev-proxy.ts via a
// relative path (a separate workspace package with no SvelteKit/$lib alias
// resolution). Keep every `$lib`-aliased import type-only — TypeScript erases
// type-only imports before Bun ever needs to resolve the specifier, but a
// value-level `$lib` import here would break dev-proxy at runtime.
import type {
  Node,
  NodeReference,
  NodeWithChildren,
  PersonNode,
  PersonNodeUpdate,
  ProjectNode,
  ProjectNodeUpdate,
  TaskNode,
  TaskNodeUpdate
} from '$lib/types';
import type { SchemaNode } from '$lib/types/schema-node';
import type { QueryFilter, SortConfig } from '$lib/types/query';
// Type-only: relationship-grouping is a pure module (no Tauri/DOM/$lib value
// imports), so this is erased before the dev-proxy's Bun runtime resolves it.
import type { RawNodeRelationships } from './relationship-grouping';

// ============================================================================
// Shared types (public BackendAdapter surface)
// ============================================================================

/** Explicit insertion position for new or moved nodes. */
export type InsertPosition =
  | { type: 'beginning' }
  | { type: 'end' }
  | { type: 'after'; siblingId: string };

/** Factory helpers for building InsertPosition values. */
export const insertPosition = {
  beginning: (): InsertPosition => ({ type: 'beginning' }),
  end: (): InsertPosition => ({ type: 'end' }),
  after: (siblingId: string): InsertPosition => ({ type: 'after', siblingId }),
} as const;

export interface CreateNodeInput {
  id: string;
  nodeType: string;
  content: string;
  properties?: Record<string, unknown>;
  mentions?: string[];
  parentId?: string | null;
  /** Where to insert among siblings. Omit for End (default). */
  insertPosition?: InsertPosition | null;
}

export interface UpdateNodeInput {
  content?: string;
  nodeType?: string;
  properties?: Record<string, unknown>;
  mentions?: string[];
}

export interface DeleteResult {
  existed: boolean;
  deletedCount: number;
}

export interface EdgeRecord {
  id: string;
  in: string;
  out: string;
  order: number;
}

export interface NodeQuery {
  id?: string;
  mentionedBy?: string;
  contentContains?: string;
  titleContains?: string;
  nodeType?: string;
  limit?: number;
}

/**
 * A structured query executed by the backend's `QueryService` — the sorting-
 * and operator-capable engine, as opposed to `NodeQuery`'s type/text scoping.
 *
 * Mirrors `ExecuteQueryInput` in `packages/core/src/ops/query_ops.rs`. The
 * filter and sort shapes are the frontend's own `QueryFilter` / `SortConfig`;
 * the adapter converts their camelCase keys to the snake_case the ops layer
 * deserializes, so callers pass a `QueryDefinition` unchanged.
 */
export interface ExecuteQueryInput {
  targetType: string;
  filters?: QueryFilter[];
  sorting?: SortConfig[];
  limit?: number;
}

export interface CreateContainerInput {
  content: string;
  nodeType: string;
  properties?: Record<string, unknown>;
  mentionedBy?: string;
}

export interface BackendAdapter {
  // Node CRUD
  createNode(input: CreateNodeInput | Node): Promise<string>;
  getNode(id: string): Promise<Node | null>;
  updateNode(id: string, version: number, update: UpdateNodeInput): Promise<Node>;
  updateTaskNode(id: string, version: number, update: TaskNodeUpdate): Promise<TaskNode>;
  updatePersonNode(id: string, version: number, update: PersonNodeUpdate): Promise<PersonNode>;
  updateProjectNode(id: string, version: number, update: ProjectNodeUpdate): Promise<ProjectNode>;
  deleteNode(id: string, version: number): Promise<DeleteResult>;

  // Hierarchy
  getChildren(parentId: string): Promise<Node[]>;
  getDescendants(rootNodeId: string): Promise<Node[]>;
  getChildrenTree(parentId: string): Promise<NodeWithChildren | null>;
  moveNode(nodeId: string, version: number, newParentId: string | null, insertPosition: InsertPosition | null): Promise<Node>;
  moveChildrenToParent(newParentId: string, children: Array<{ id: string; version: number }>): Promise<Node[]>;

  // Mentions
  createMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void>;
  deleteMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void>;
  getOutgoingMentions(nodeId: string): Promise<string[]>;
  getIncomingMentions(nodeId: string): Promise<string[]>;
  getMentioningContainers(nodeId: string): Promise<NodeReference[]>;

  // Queries
  queryNodes(query: NodeQuery): Promise<Node[]>;
  /**
   * Execute a structured query through the backend's `QueryService`: property
   * filters with comparison operators, and ordering the caller specifies.
   *
   * Distinct from `queryNodes`, which scopes by type/text only and cannot sort.
   * Saved queries run here so filter and sort semantics have exactly one
   * implementation — notably `task.priority`'s urgency ranking, which is not
   * the alphabetical order of its values.
   */
  executeQuery(input: ExecuteQueryInput): Promise<Node[]>;
  /**
   * Count what `executeQuery` would match, without transferring the matches.
   *
   * Takes the same input so a caller can count exactly what it would execute,
   * but the backend ignores `sorting` and `limit`: ordering cannot change a
   * total, and a limit would cap the very number being asked for. The result is
   * therefore exact for any number of matches — unlike `executeQuery(...).length`,
   * which saturates at `MAX_QUERY_ROWS`.
   */
  countQuery(input: ExecuteQueryInput): Promise<number>;
  mentionAutocomplete(query: string, limit?: number): Promise<Node[]>;
  /** Title-prefix search over nodes of an optional type, for the target picker. */
  searchNodesByTitle(nodeType: string | null, titleContains: string, limit?: number): Promise<Node[]>;
  /**
   * Suggest-don't-block uniqueness lookup (ADR-065): the existing active node
   * whose `field` matches `value` for `node_type`, or `null` when the field
   * isn't flagged `unique`, `value` is empty, or there is no conflict. Never
   * rejects — callers use a hit to offer an adopt-existing suggestion.
   *
   * Pass `excludeId` (the caller's own node id) whenever that node's own
   * value could already equal `value` — otherwise a node whose own save has
   * already landed the same value can match itself and hide a real,
   * different duplicate.
   */
  findDuplicateFor(
    nodeType: string,
    field: string,
    value: string,
    excludeId?: string
  ): Promise<Node | null>;

  // Typed relationships (distinct from mentions)
  getNodeRelationships(nodeId: string): Promise<RawNodeRelationships>;
  createRelationship(
    sourceId: string,
    relationshipName: string,
    targetId: string,
    edgeData?: Record<string, unknown>
  ): Promise<void>;
  deleteRelationship(sourceId: string, relationshipName: string, targetId: string): Promise<void>;
  updateRelationshipProperties(
    sourceId: string,
    relationshipName: string,
    targetId: string,
    properties: Record<string, unknown>
  ): Promise<void>;

  // Composite operations
  createContainerNode(input: CreateContainerInput): Promise<string>;

  // Schema operations (read-only - mutation commands removed)
  // Returns SchemaNode with typed top-level fields (isCore, schemaVersion, description, fields)
  getAllSchemas(): Promise<SchemaNode[]>;
  getSchema(schemaId: string): Promise<SchemaNode>;

  // Daemon metadata
  /** The running daemon binary's semver (its compiled CARGO_PKG_VERSION). */
  getDaemonVersion(): Promise<string>;
}

// ============================================================================
// CreateNode — shared request shaping
// ============================================================================

/**
 * Normalized CreateNode wire fields, matching CreateNodeRequest in
 * node_service.proto: parentId/insertPosition are omittable (proto `optional`),
 * not merely nullable — a `null` sent over IPC/HTTP is a real "no parent" value
 * on the Rust side, not "field absent."
 */
export interface CreateNodeFields {
  id: string;
  nodeType: string;
  content: string;
  properties: Record<string, unknown>;
  mentions: string[];
  parentId: string | null;
  insertPosition: InsertPosition | null;
}

export function buildCreateNodeFields(input: CreateNodeInput | Node): CreateNodeFields {
  return {
    id: input.id,
    nodeType: input.nodeType,
    content: input.content,
    properties: input.properties ?? {},
    mentions: (input as CreateNodeInput).mentions ?? [],
    parentId: (input as CreateNodeInput).parentId ?? null,
    insertPosition: (input as CreateNodeInput).insertPosition ?? null,
  };
}

// ============================================================================
// UpdateTaskNode — tri-state clearable-field encoding
// ============================================================================

/**
 * Wire encoding for a single "optional, clearable" field, mirroring
 * OptionalStringClear / OptionalTimestampClear in node_service.proto:
 *   - field absent from the patch   → no change (outer None)
 *   - field present, value `null`   → clear the value (Some(None))
 *   - field present, value `T`      → set the value (Some(Some(T)))
 */
export type ClearableField<T> = { clear: true } | { clear: false; value: T } | undefined;

export interface TaskNodeUpdatePatch {
  status?: string;
  priority: ClearableField<string>;
  dueDate: ClearableField<string>;
  startedAt: ClearableField<string>;
  completedAt: ClearableField<string>;
  content?: string;
}

function clearable(value: string | null | undefined): ClearableField<string> {
  if (value === undefined) return undefined;
  if (value === null) return { clear: true };
  return { clear: false, value };
}

/**
 * The single authoritative mapping from the frontend's `TaskNodeUpdate` shape
 * (plain `null` = clear, `undefined`/absent = no change) to the tri-state wire
 * patch the daemon expects. Both the dev-proxy's gRPC request and any future
 * Tauri-side equivalent must derive from this function, not re-implement it.
 */
export function buildTaskNodeUpdatePatch(update: TaskNodeUpdate): TaskNodeUpdatePatch {
  return {
    status: update.status,
    priority: clearable(update.priority),
    dueDate: clearable(update.dueDate),
    startedAt: clearable(update.startedAt),
    completedAt: clearable(update.completedAt),
    content: update.content,
  };
}

export interface PersonNodeUpdatePatch {
  firstName: ClearableField<string>;
  lastName: ClearableField<string>;
  email: ClearableField<string>;
}

/** `PersonNodeUpdate` → tri-state wire patch. See `buildTaskNodeUpdatePatch`. */
export function buildPersonNodeUpdatePatch(update: PersonNodeUpdate): PersonNodeUpdatePatch {
  return {
    firstName: clearable(update.firstName),
    lastName: clearable(update.lastName),
    email: clearable(update.email),
  };
}

export interface ProjectNodeUpdatePatch {
  status?: string;
  priority: ClearableField<string>;
  startDate: ClearableField<string>;
  endDate: ClearableField<string>;
}

/** `ProjectNodeUpdate` → tri-state wire patch. See `buildTaskNodeUpdatePatch`. */
export function buildProjectNodeUpdatePatch(update: ProjectNodeUpdate): ProjectNodeUpdatePatch {
  return {
    status: update.status,
    priority: clearable(update.priority),
    startDate: clearable(update.startDate),
    endDate: clearable(update.endDate),
  };
}

// ============================================================================
// ExecuteQuery — structured query wire encoding
// ============================================================================

/**
 * The most rows the daemon will return for one query, whatever the request
 * asks for — `MAX_ROW_LIMIT` in `packages/daemon/src/services/node_service.rs`,
 * which clamps rather than rejects.
 *
 * `adapter-core.test.ts` reads that constant out of the Rust source and asserts
 * it equals this one, so the two cannot drift silently.
 *
 * Callers need this to tell a complete result from a truncated one: the clamp
 * is silent, so a request for more comes back looking exactly like a result
 * set that happened to be that size. Asking for more than this cannot return
 * more, so a caller that wants "everything" should ask for exactly this and
 * treat a full page as "there may be more".
 */
export const MAX_QUERY_ROWS = 500;

/**
 * Wire shape for `ExecuteQuery`, matching `ExecuteQueryRequest` in
 * node_service.proto. Filters and sorting cross as JSON strings rather than
 * modeled fields: a filter's `value` is free-form, which has no natural proto
 * representation without a oneof per scalar type, so the ops layer's serde
 * definition is the single authority on the shape.
 */
export interface ExecuteQueryWire {
  targetType: string;
  filtersJson: string;
  sortingJson: string | null;
  limit: number;
}

/**
 * Metadata fields `QueryService::resolve_field` reads as top-level SQL columns,
 * keyed by the camelCase spelling a stored `QueryDefinition` uses.
 *
 * A stored sort names `modifiedAt` (see `QueryNode`'s docs and
 * `QUERY_TEMPLATE_EXAMPLES`), but the backend matches the column name
 * `modified_at`. Anything it does not recognize becomes
 * `json_extract(properties, '$.<type>.<field>')` — for a metadata field that
 * path is structurally NULL, so an unconverted `modifiedAt` would not error,
 * it would silently order every row equally. Only these five are renamed;
 * property names are stored as authored and pass through untouched.
 *
 * `content` and `title` map to themselves deliberately: they are metadata
 * columns whose two spellings coincide, and listing them states that they were
 * considered rather than leaving a reader to wonder if they were missed.
 *
 * This list must agree with `resolve_field`'s, which is the kind of
 * cross-language pairing this module otherwise exists to avoid. It is tolerable
 * here because the set is five long-stable column names rather than a semantic
 * table, and a mismatch degrades to an unsorted result rather than wrong data.
 */
const METADATA_SORT_FIELDS: Readonly<Record<string, string>> = {
  createdAt: 'created_at',
  modifiedAt: 'modified_at',
  nodeType: 'node_type',
  content: 'content',
  title: 'title',
};

/** Resolve a sort field to the spelling `resolve_field` matches. */
export function encodeSortField(field: string): string {
  return METADATA_SORT_FIELDS[field] ?? field;
}

/**
 * Encode a query definition for the `ExecuteQuery` wire.
 *
 * `limit` uses 0 as the "unset" sentinel, matching the proto: the server then
 * applies its own default. Filter/sort keys are converted to the snake_case
 * `AgentFilterItem` / `AgentSortItem` deserialize, and absent optional keys are
 * omitted rather than sent as null — both structs are `deny_unknown_fields`,
 * and a null would not deserialize into `Option<T>`'s absent case the way a
 * missing key does.
 */
export function buildExecuteQueryWire(input: ExecuteQueryInput): ExecuteQueryWire {
  const filters = (input.filters ?? []).map((f) => ({
    type: f.type,
    operator: f.operator,
    ...(f.property !== undefined ? { property: f.property } : {}),
    ...(f.value !== undefined ? { value: f.value } : {}),
    ...(f.caseSensitive !== undefined ? { case_sensitive: f.caseSensitive } : {}),
    ...(f.relationshipType !== undefined ? { relationship_type: f.relationshipType } : {}),
    ...(f.nodeId !== undefined ? { node_id: f.nodeId } : {}),
  }));

  const sorting = input.sorting?.map((s) => ({
    field: encodeSortField(s.field),
    direction: s.direction,
  }));

  return {
    targetType: input.targetType,
    filtersJson: JSON.stringify(filters),
    sortingJson: sorting && sorting.length > 0 ? JSON.stringify(sorting) : null,
    limit: input.limit ?? 0,
  };
}

// ============================================================================
// MoveNode / CreateNode — InsertPosition wire encoding
// ============================================================================

/** Wire shape for InsertPosition, matching the `oneof position` in node_service.proto. */
export type InsertPositionWire =
  | { beginning: true }
  | { end: true }
  | { after: string }
  | Record<string, never>;

export function encodeInsertPosition(pos: InsertPosition | null | undefined): InsertPositionWire {
  if (!pos) return {};
  switch (pos.type) {
    case 'beginning':
      return { beginning: true };
    case 'end':
      return { end: true };
    case 'after':
      return { after: pos.siblingId };
  }
}

// ============================================================================
// Response normalization
// ============================================================================

/** Backend returns {} for a non-existent parent's children-tree; normalize to null. */
export function normalizeChildrenTree(
  result: NodeWithChildren | Record<string, never> | null | undefined,
): NodeWithChildren | null {
  if (!result || Object.keys(result).length === 0) return null;
  return result as NodeWithChildren;
}

// ============================================================================
// Route table — single source of truth for the HTTP surface
// ============================================================================

/**
 * HTTP route templates used by both HttpAdapter (to build fetch URLs) and the
 * dev-proxy (to route incoming requests to the matching gRPC call). Keeping
 * these in one place means a path change is a single edit instead of two
 * hand-synced pattern literals.
 */
export const HTTP_ROUTES = {
  createNode: () => '/api/nodes',
  getNode: (id: string) => `/api/nodes/${encodeURIComponent(id)}`,
  updateNode: (id: string) => `/api/nodes/${encodeURIComponent(id)}`,
  deleteNode: (id: string) => `/api/nodes/${encodeURIComponent(id)}`,
  updateTaskNode: (id: string) => `/api/tasks/${encodeURIComponent(id)}`,
  updatePersonNode: (id: string) => `/api/persons/${encodeURIComponent(id)}`,
  updateProjectNode: (id: string) => `/api/projects/${encodeURIComponent(id)}`,
  moveNode: (id: string) => `/api/nodes/${encodeURIComponent(id)}/parent`,
  moveChildrenToParent: (parentId: string) => `/api/nodes/${encodeURIComponent(parentId)}/move-children`,
  getChildren: (parentId: string) => `/api/nodes/${encodeURIComponent(parentId)}/children`,
  getChildrenTree: (parentId: string) => `/api/nodes/${encodeURIComponent(parentId)}/children-tree`,
  createMention: () => '/api/mentions',
  deleteMention: () => '/api/mentions',
  getOutgoingMentions: (nodeId: string) => `/api/nodes/${encodeURIComponent(nodeId)}/mentions/outgoing`,
  getIncomingMentions: (nodeId: string) => `/api/nodes/${encodeURIComponent(nodeId)}/mentions/incoming`,
  getMentioningContainers: (nodeId: string) => `/api/nodes/${encodeURIComponent(nodeId)}/mentions/roots`,
  queryNodes: () => '/api/query',
  executeQuery: () => '/api/query/execute',
  countQuery: () => '/api/query/count',
  mentionAutocomplete: () => '/api/mentions/autocomplete',
  findDuplicate: () => '/api/nodes/find-duplicate',
  getAllSchemas: () => '/api/schemas',
  getSchema: (schemaId: string) => `/api/schemas/${encodeURIComponent(schemaId)}`,
  getDaemonVersion: () => '/api/daemon/version',
  getNodeRelationships: (nodeId: string) => `/api/nodes/${encodeURIComponent(nodeId)}/relationships`,
  // create (POST) / delete (DELETE) / update-properties (PATCH) share one URL,
  // differentiated by HTTP method — mirrors the /api/mentions convention.
  createRelationship: () => '/api/relationships',
  deleteRelationship: () => '/api/relationships',
  updateRelationshipProperties: () => '/api/relationships',
} as const;

/**
 * Path-matching counterparts as RegExp, for the dev-proxy's router. Templates
 * with a dynamic segment expose the same key as HTTP_ROUTES so a new route
 * only needs one addition here plus one in the handler dispatch, not a
 * hand-copied regex that can drift from the URL the adapter actually builds.
 */
export const HTTP_ROUTE_PATTERNS = {
  getNode: /^\/api\/nodes\/([^/]+)$/,
  updateNode: /^\/api\/nodes\/([^/]+)$/,
  deleteNode: /^\/api\/nodes\/([^/]+)$/,
  updateTaskNode: /^\/api\/tasks\/([^/]+)$/,
  updatePersonNode: /^\/api\/persons\/([^/]+)$/,
  updateProjectNode: /^\/api\/projects\/([^/]+)$/,
  moveNode: /^\/api\/nodes\/([^/]+)\/parent$/,
  moveChildrenToParent: /^\/api\/nodes\/([^/]+)\/move-children$/,
  getChildren: /^\/api\/nodes\/([^/]+)\/children$/,
  getChildrenTree: /^\/api\/nodes\/([^/]+)\/children-tree$/,
  getOutgoingMentions: /^\/api\/nodes\/([^/]+)\/mentions\/outgoing$/,
  getIncomingMentions: /^\/api\/nodes\/([^/]+)\/mentions\/incoming$/,
  getMentioningContainers: /^\/api\/nodes\/([^/]+)\/mentions\/roots$/,
  getSchema: /^\/api\/schemas\/([^/]+)$/,
  getNodeRelationships: /^\/api\/nodes\/([^/]+)\/relationships$/,
} as const;
