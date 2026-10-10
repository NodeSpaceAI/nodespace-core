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
  CollectionNode,
  CollectionNodeUpdate,
  DatabaseSettingsNode,
  DatabaseSettingsNodeUpdate,
  DecisionNode,
  DecisionNodeUpdate,
  Node,
  NodeReference,
  NodeWithChildren,
  PersonNode,
  PersonNodeUpdate,
  PlanNode,
  PlanNodeUpdate,
  PlayNode,
  PlayNodeUpdate,
  ProjectNode,
  ProjectNodeUpdate,
  QueryNode,
  QueryNodeUpdate,
  SkillNode,
  SkillNodeUpdate,
  SpecNode,
  SpecNodeUpdate,
  TaskNode,
  TaskNodeUpdate
} from '$lib/types';
import type { DeleteResult, NodeQuery } from '$lib/types/generated';
import type { SchemaNode } from '$lib/types/schema-node';
import type { QueryFilter, SortConfig } from '$lib/types/query';
// Type-only: relationship-grouping is a pure module (no Tauri/DOM/$lib value
// imports), so this is erased before the dev-proxy's Bun runtime resolves it.
import type { CreateRelationshipResult, RawNodeRelationships } from './relationship-grouping';

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

export type { DeleteResult, NodeQuery };

export interface EdgeRecord {
  id: string;
  in: string;
  out: string;
  order: number;
}

/**
 * A structured query executed by the backend's `QueryService` — the sorting-
 * and operator-capable engine, as opposed to `NodeQuery`'s type/text scoping.
 *
 * Mirrors `ExecuteQueryInput` in `packages/core/src/ops/query_ops.rs`. The
 * filter and sort shapes are the generated `QueryFilter` / `SortConfig`, whose
 * keys are the snake_case the ops layer deserializes, so a `QueryDefinition`
 * is sent as it is stored.
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

/**
 * Where a create or move placed its node's `has_child` edge: the store's order
 * key for that edge, plus the new key of every sibling a re-spread rewrote.
 *
 * The client that made the write never receives that write's own relationship
 * events (same-origin echo suppression), so the reply is the only place it
 * learns the authoritative keys. Apply with `applyChildPlacement`.
 */
export interface ChildPlacement {
  parentId: string;
  order: number;
  respread: Array<{ nodeId: string; order: number }>;
}

/** Result of `createNode`. `placement` is null for a root node. */
export interface CreatedNode {
  id: string;
  placement: ChildPlacement | null;
}

/** Result of `moveNode`: the node with its bumped version. `placement` is null for a move to root. */
export interface MovedNode {
  node: Node;
  placement: ChildPlacement | null;
}

/** Result of `moveChildrenToParent`: the children with bumped versions, and each new edge's store order key. */
export interface MovedChildren {
  nodes: Node[];
  orders: Array<{ nodeId: string; order: number }>;
}

export interface BackendAdapter {
  // Node CRUD
  createNode(input: CreateNodeInput | Node): Promise<CreatedNode>;
  getNode(id: string): Promise<Node | null>;
  updateNode(id: string, version: number, update: UpdateNodeInput): Promise<Node>;
  updateTaskNode(id: string, version: number, update: TaskNodeUpdate): Promise<TaskNode>;
  updatePersonNode(id: string, version: number, update: PersonNodeUpdate): Promise<PersonNode>;
  updateProjectNode(id: string, version: number, update: ProjectNodeUpdate): Promise<ProjectNode>;
  updateQueryNode(id: string, version: number, update: QueryNodeUpdate): Promise<QueryNode>;
  updatePlayNode(id: string, version: number, update: PlayNodeUpdate): Promise<PlayNode>;
  updateCollectionNode(
    id: string,
    version: number,
    update: CollectionNodeUpdate
  ): Promise<CollectionNode>;
  updateSkillNode(id: string, version: number, update: SkillNodeUpdate): Promise<SkillNode>;
  updateSpecNode(id: string, version: number, update: SpecNodeUpdate): Promise<SpecNode>;
  updatePlanNode(id: string, version: number, update: PlanNodeUpdate): Promise<PlanNode>;
  updateDecisionNode(
    id: string,
    version: number,
    update: DecisionNodeUpdate
  ): Promise<DecisionNode>;
  updateDatabaseSettingsNode(
    id: string,
    version: number,
    update: DatabaseSettingsNodeUpdate
  ): Promise<DatabaseSettingsNode>;
  deleteNode(id: string, version: number): Promise<DeleteResult>;

  // Hierarchy
  getChildren(parentId: string): Promise<Node[]>;
  getDescendants(rootNodeId: string): Promise<Node[]>;
  getChildrenTree(parentId: string): Promise<NodeWithChildren | null>;
  /** The node a node is a child of, or null for a root. */
  getParent(nodeId: string): Promise<NodeReference | null>;
  moveNode(nodeId: string, version: number, newParentId: string | null, insertPosition: InsertPosition | null): Promise<MovedNode>;
  moveChildrenToParent(newParentId: string, children: Array<{ id: string; version: number }>): Promise<MovedChildren>;

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
  ): Promise<CreateRelationshipResult>;
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
  // Returns the typed SchemaNode: fields, relationships, extends and the rest at the top level
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
  pullRequest: OptionalJsonClear | undefined;
  commits: OptionalJsonClear | undefined;
  requiresSpec?: boolean;
}

function clearable(value: string | null | undefined): ClearableField<string> {
  if (value === undefined) return undefined;
  if (value === null) return { clear: true };
  return { clear: false, value };
}

/**
 * A structured value (a link, a list of links) as the daemon's JSON wrapper
 * (`OptionalJsonClear`): `clear` empties the field, otherwise `valueJson` is the
 * JSON-encoded value. Absent stays absent.
 */
export type OptionalJsonClear = { clear: boolean; valueJson: string };

function jsonClearable<T>(value: T | null | undefined): OptionalJsonClear | undefined {
  if (value === undefined) return undefined;
  if (value === null) return { clear: true, valueJson: '' };
  return { clear: false, valueJson: JSON.stringify(value) };
}

/**
 * The fields each per-field typed update carries: its type's schema fields, by
 * wire name. `satisfies Record<keyof …, true>` makes each list exactly the
 * update interface's keys, so a field added to one can't be missed here.
 */
const TYPED_UPDATE_FIELDS = {
  task: {
    status: true,
    priority: true,
    dueDate: true,
    startedAt: true,
    completedAt: true,
    pullRequest: true,
    commits: true,
    requiresSpec: true
  } satisfies Record<keyof TaskNodeUpdate, true>,
  person: {
    firstName: true,
    lastName: true,
    email: true
  } satisfies Record<keyof PersonNodeUpdate, true>,
  project: {
    status: true,
    priority: true,
    startDate: true,
    endDate: true,
    repository: true,
    checkoutPath: true
  } satisfies Record<keyof ProjectNodeUpdate, true>
} as const;

/** The core types whose typed update travels as one wire field per schema field. */
export type PerFieldTypedUpdateType = keyof typeof TYPED_UPDATE_FIELDS;

/** The fields `nodeType`'s typed update carries, by wire name. */
export function typedUpdateFieldNames(nodeType: PerFieldTypedUpdateType): string[] {
  return Object.keys(TYPED_UPDATE_FIELDS[nodeType]);
}

/**
 * The keys of a typed update request body that are not fields of that type's
 * update (`version` travels beside them). `content` and `properties` are the
 * ones a caller is likely to send: both belong to the generic node update.
 */
export function unknownTypedUpdateKeys(
  nodeType: PerFieldTypedUpdateType,
  body: Record<string, unknown>
): string[] {
  const fields: Record<string, true> = TYPED_UPDATE_FIELDS[nodeType];
  return Object.keys(body).filter((key) => key !== 'version' && !Object.hasOwn(fields, key));
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
    pullRequest: jsonClearable(update.pullRequest),
    commits: jsonClearable(update.commits),
    requiresSpec: update.requiresSpec
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
  repository: OptionalJsonClear | undefined;
  checkoutPath: ClearableField<string>;
}

/** `ProjectNodeUpdate` → tri-state wire patch. See `buildTaskNodeUpdatePatch`. */
export function buildProjectNodeUpdatePatch(update: ProjectNodeUpdate): ProjectNodeUpdatePatch {
  return {
    status: update.status,
    priority: clearable(update.priority),
    startDate: clearable(update.startDate),
    endDate: clearable(update.endDate),
    repository: jsonClearable(update.repository),
    checkoutPath: clearable(update.checkoutPath),
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
 * Encode a query definition for the `ExecuteQuery` wire.
 *
 * `limit` uses 0 as the "unset" sentinel, matching the proto: the server then
 * applies its own default. Filters and sorting are sent as written: their keys
 * and the field names they carry have one spelling, the stored one.
 */
export function buildExecuteQueryWire(input: ExecuteQueryInput): ExecuteQueryWire {
  const sorting = input.sorting;

  return {
    targetType: input.targetType,
    filtersJson: JSON.stringify(input.filters ?? []),
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
  updateQueryNode: (id: string) => `/api/queries/${encodeURIComponent(id)}`,
  updatePlayNode: (id: string) => `/api/plays/${encodeURIComponent(id)}`,
  updateCollectionNode: (id: string) => `/api/collections/${encodeURIComponent(id)}`,
  updateSkillNode: (id: string) => `/api/skills/${encodeURIComponent(id)}`,
  updateSpecNode: (id: string) => `/api/specs/${encodeURIComponent(id)}`,
  updatePlanNode: (id: string) => `/api/plans/${encodeURIComponent(id)}`,
  updateDecisionNode: (id: string) => `/api/decisions/${encodeURIComponent(id)}`,
  updateDatabaseSettingsNode: (id: string) => `/api/database-settings/${encodeURIComponent(id)}`,
  moveNode: (id: string) => `/api/nodes/${encodeURIComponent(id)}/parent`,
  moveChildrenToParent: (parentId: string) => `/api/nodes/${encodeURIComponent(parentId)}/move-children`,
  getChildren: (parentId: string) => `/api/nodes/${encodeURIComponent(parentId)}/children`,
  getChildrenTree: (parentId: string) => `/api/nodes/${encodeURIComponent(parentId)}/children-tree`,
  getParent: (nodeId: string) => `/api/nodes/${encodeURIComponent(nodeId)}/parent-node`,
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
  updateQueryNode: /^\/api\/queries\/([^/]+)$/,
  updatePlayNode: /^\/api\/plays\/([^/]+)$/,
  updateCollectionNode: /^\/api\/collections\/([^/]+)$/,
  updateSkillNode: /^\/api\/skills\/([^/]+)$/,
  updateSpecNode: /^\/api\/specs\/([^/]+)$/,
  updatePlanNode: /^\/api\/plans\/([^/]+)$/,
  updateDecisionNode: /^\/api\/decisions\/([^/]+)$/,
  updateDatabaseSettingsNode: /^\/api\/database-settings\/([^/]+)$/,
  moveNode: /^\/api\/nodes\/([^/]+)\/parent$/,
  moveChildrenToParent: /^\/api\/nodes\/([^/]+)\/move-children$/,
  getChildren: /^\/api\/nodes\/([^/]+)\/children$/,
  getChildrenTree: /^\/api\/nodes\/([^/]+)\/children-tree$/,
  getParent: /^\/api\/nodes\/([^/]+)\/parent-node$/,
  getOutgoingMentions: /^\/api\/nodes\/([^/]+)\/mentions\/outgoing$/,
  getIncomingMentions: /^\/api\/nodes\/([^/]+)\/mentions\/incoming$/,
  getMentioningContainers: /^\/api\/nodes\/([^/]+)\/mentions\/roots$/,
  getSchema: /^\/api\/schemas\/([^/]+)$/,
  getNodeRelationships: /^\/api\/nodes\/([^/]+)\/relationships$/,
} as const;
