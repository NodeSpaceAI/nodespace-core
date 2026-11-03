/**
 * Transport-agnostic backend adapter core.
 *
 * The daemon's NodeService contract (packages/proto/proto/node_service.proto) is
 * consumed by TauriAdapter (IPC). This module is the single place that encodes
 * *what the wire contract is*: which fields are required vs. optional-omit vs.
 * tri-state-clearable.
 */

import type {
  CollectionNode,
  CollectionNodeUpdate,
  DatabaseSettingsNode,
  DatabaseSettingsNodeUpdate,
  DecisionNode,
  DecisionNodeUpdate,
  IssueNode,
  IssueNodeUpdate,
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
  SoftwareProjectNode,
  SoftwareProjectNodeUpdate,
  SpecNode,
  SpecNodeUpdate,
  TaskNode,
  TaskNodeUpdate
} from '$lib/types';
import type { DeleteResult, NodeQuery } from '$lib/types/generated';
import type { SchemaNode } from '$lib/types/schema-node';
import type { QueryFilter, SortConfig } from '$lib/types/query';
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
  /**
   * Collection to file the new node into. The backend commits the node and its
   * membership in one transaction, so a failed join leaves no node behind.
   */
  collectionId?: string | null;
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
  updateIssueNode(id: string, version: number, update: IssueNodeUpdate): Promise<IssueNode>;
  updatePersonNode(id: string, version: number, update: PersonNodeUpdate): Promise<PersonNode>;
  updateProjectNode(id: string, version: number, update: ProjectNodeUpdate): Promise<ProjectNode>;
  updateSoftwareProjectNode(
    id: string,
    version: number,
    update: SoftwareProjectNodeUpdate
  ): Promise<SoftwareProjectNode>;
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
 * not merely nullable — a `null` sent over IPC is a real "no parent" value
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
  /** Present only when the node is to be filed into a collection. */
  collectionId?: string;
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
    ...((input as CreateNodeInput).collectionId
      ? { collectionId: (input as CreateNodeInput).collectionId as string }
      : {}),
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
// Response normalization
// ============================================================================

/** Backend returns {} for a non-existent parent's children-tree; normalize to null. */
export function normalizeChildrenTree(
  result: NodeWithChildren | Record<string, never> | null | undefined,
): NodeWithChildren | null {
  if (!result || Object.keys(result).length === 0) return null;
  return result as NodeWithChildren;
}
