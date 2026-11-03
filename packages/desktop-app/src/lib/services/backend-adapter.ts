/**
 * Backend adapter: the frontend's one door to the backend.
 *
 * # Architecture
 *
 * - **TauriAdapter**: Uses Tauri's `invoke()` for IPC (the desktop app)
 * - **MockAdapter**: Returns empty/default values for the test environment
 *
 * TauriAdapter is a thin transport shim: the request shaping rules (which
 * fields are optional-omit vs. tri-state-clearable, matching node_service.proto)
 * live in ./adapter-core.
 *
 * # Usage
 *
 * ```typescript
 * import { backendAdapter } from '$lib/services/backend-adapter';
 *
 * const nodes = await backendAdapter.getChildren('parent-id');
 * ```
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
import type { SchemaNode } from '$lib/types/schema-node';
import type { CreateRelationshipResult, RawNodeRelationships } from './relationship-grouping';
import { createLogger } from '$lib/utils/logger';
import { withDiagnosticLogging } from './diagnostic-logger';
import { invoke } from '@tauri-apps/api/core';
import {
  buildCreateNodeFields,
  buildExecuteQueryWire,
  normalizeChildrenTree,
  type BackendAdapter,
  type CreateNodeInput,
  type UpdateNodeInput,
  type DeleteResult,
  type NodeQuery,
  type ExecuteQueryInput,
  type CreateContainerInput,
  type InsertPosition,
  type CreatedNode,
  type MovedChildren,
  type MovedNode,
} from './adapter-core';

const log = createLogger('BackendAdapter');

export type {
  BackendAdapter,
  InsertPosition,
  CreateNodeInput,
  UpdateNodeInput,
  DeleteResult,
  EdgeRecord,
  NodeQuery,
  ExecuteQueryInput,
  CreateContainerInput,
  ChildPlacement,
  CreatedNode,
  MovedChildren,
  MovedNode,
} from './adapter-core';
export { insertPosition } from './adapter-core';

// ============================================================================
// Tauri Adapter (Desktop App - IPC)
// ============================================================================

class TauriAdapter implements BackendAdapter {
  async createNode(input: CreateNodeInput | Node): Promise<CreatedNode> {
    // Tauri 2.x with #[serde(rename_all = "camelCase")] expects camelCase field names
    const nodeInput = buildCreateNodeFields(input);
    return withDiagnosticLogging(
      'createNode',
      () => invoke<CreatedNode>('create_node', { node: nodeInput }),
      [nodeInput]
    );
  }

  async getNode(id: string): Promise<Node | null> {
    return withDiagnosticLogging(
      'getNode',
      () => invoke<Node | null>('get_node', { id }),
      [id]
    );
  }

  async updateNode(id: string, version: number, update: UpdateNodeInput): Promise<Node> {
    return withDiagnosticLogging(
      'updateNode',
      () => invoke<Node>('update_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updateTaskNode(id: string, version: number, update: TaskNodeUpdate): Promise<TaskNode> {
    // Forwards `update` as-is rather than calling buildTaskNodeUpdatePatch —
    // the tri-state clear/set/no-change encoding for this path is done by
    // the Rust #[tauri::command] handler, not here; the Tauri-seam tests
    // (ADR-048) hold that encoding to its documented result.
    return withDiagnosticLogging(
      'updateTaskNode',
      () => invoke<TaskNode>('update_task_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updateIssueNode(id: string, version: number, update: IssueNodeUpdate): Promise<IssueNode> {
    return withDiagnosticLogging(
      'updateIssueNode',
      () => invoke<IssueNode>('update_issue_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updatePersonNode(id: string, version: number, update: PersonNodeUpdate): Promise<PersonNode> {
    // Like updateTaskNode, the tri-state encoding happens in the Rust command.
    return withDiagnosticLogging(
      'updatePersonNode',
      () => invoke<PersonNode>('update_person_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updateProjectNode(
    id: string,
    version: number,
    update: ProjectNodeUpdate
  ): Promise<ProjectNode> {
    return withDiagnosticLogging(
      'updateProjectNode',
      () => invoke<ProjectNode>('update_project_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updateSoftwareProjectNode(
    id: string,
    version: number,
    update: SoftwareProjectNodeUpdate
  ): Promise<SoftwareProjectNode> {
    return withDiagnosticLogging(
      'updateSoftwareProjectNode',
      () =>
        invoke<SoftwareProjectNode>('update_software_project_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updateQueryNode(id: string, version: number, update: QueryNodeUpdate): Promise<QueryNode> {
    return withDiagnosticLogging(
      'updateQueryNode',
      () => invoke<QueryNode>('update_query_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updatePlayNode(id: string, version: number, update: PlayNodeUpdate): Promise<PlayNode> {
    return withDiagnosticLogging(
      'updatePlayNode',
      () => invoke<PlayNode>('update_play_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updateCollectionNode(
    id: string,
    version: number,
    update: CollectionNodeUpdate
  ): Promise<CollectionNode> {
    return withDiagnosticLogging(
      'updateCollectionNode',
      () => invoke<CollectionNode>('update_collection_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updateSkillNode(id: string, version: number, update: SkillNodeUpdate): Promise<SkillNode> {
    return withDiagnosticLogging(
      'updateSkillNode',
      () => invoke<SkillNode>('update_skill_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updateSpecNode(id: string, version: number, update: SpecNodeUpdate): Promise<SpecNode> {
    return withDiagnosticLogging(
      'updateSpecNode',
      () => invoke<SpecNode>('update_spec_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updatePlanNode(id: string, version: number, update: PlanNodeUpdate): Promise<PlanNode> {
    return withDiagnosticLogging(
      'updatePlanNode',
      () => invoke<PlanNode>('update_plan_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updateDecisionNode(id: string, version: number, update: DecisionNodeUpdate): Promise<DecisionNode> {
    return withDiagnosticLogging(
      'updateDecisionNode',
      () => invoke<DecisionNode>('update_decision_node', { id, version, update }),
      [id, version, update]
    );
  }

  async updateDatabaseSettingsNode(
    id: string,
    version: number,
    update: DatabaseSettingsNodeUpdate
  ): Promise<DatabaseSettingsNode> {
    return withDiagnosticLogging(
      'updateDatabaseSettingsNode',
      () =>
        invoke<DatabaseSettingsNode>('update_database_settings_node', { id, version, update }),
      [id, version, update]
    );
  }

  async deleteNode(id: string, version: number): Promise<DeleteResult> {
    return withDiagnosticLogging(
      'deleteNode',
      () => invoke<DeleteResult>('delete_node', { id, version }),
      [id, version]
    );
  }

  async getChildren(parentId: string): Promise<Node[]> {
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'getChildren',
      () => invoke<Node[]>('get_children', { parentId }),
      [parentId]
    );
  }

  async getDescendants(rootNodeId: string): Promise<Node[]> {
    return getDescendantsViaChildrenTree(
      rootNodeId,
      (parentId) => this.getChildrenTree(parentId),
      (parentId) => this.getChildren(parentId)
    );
  }

  async getParent(nodeId: string): Promise<NodeReference | null> {
    return withDiagnosticLogging(
      'getParent',
      () => invoke<NodeReference | null>('get_parent', { nodeId }),
      [nodeId]
    );
  }

  async getChildrenTree(parentId: string): Promise<NodeWithChildren | null> {
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'getChildrenTree',
      async () => {
        const result = await invoke<NodeWithChildren | Record<string, never>>('get_children_tree', { parentId });
        return normalizeChildrenTree(result);
      },
      [parentId]
    );
  }

  async moveNode(nodeId: string, version: number, newParentId: string | null, insertPosition: InsertPosition | null): Promise<MovedNode> {
    return withDiagnosticLogging(
      'moveNode',
      () => invoke<MovedNode>('move_node', {
        nodeId,
        version,
        newParentId,
        insertPosition
      }),
      [nodeId, version, newParentId, insertPosition]
    );
  }

  async moveChildrenToParent(newParentId: string, children: Array<{ id: string; version: number }>): Promise<MovedChildren> {
    return withDiagnosticLogging(
      'moveChildrenToParent',
      () => invoke<MovedChildren>('move_children_to_parent', {
        newParentId,
        children: children.map(c => ({ nodeId: c.id, version: c.version }))
      }),
      [newParentId, children.length]
    );
  }

  async createMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'createMention',
      () => invoke<void>('create_node_mention', {
        mentioningNodeId,
        mentionedNodeId
      }),
      [mentioningNodeId, mentionedNodeId]
    );
  }

  async deleteMention(mentioningNodeId: string, mentionedNodeId: string): Promise<void> {
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'deleteMention',
      () => invoke<void>('delete_node_mention', {
        mentioningNodeId,
        mentionedNodeId
      }),
      [mentioningNodeId, mentionedNodeId]
    );
  }

  async getOutgoingMentions(nodeId: string): Promise<string[]> {
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'getOutgoingMentions',
      () => invoke<string[]>('get_outgoing_mentions', { nodeId }),
      [nodeId]
    );
  }

  async getIncomingMentions(nodeId: string): Promise<string[]> {
    // Tauri 2.x auto-converts snake_case to camelCase
    return withDiagnosticLogging(
      'getIncomingMentions',
      () => invoke<string[]>('get_incoming_mentions', { nodeId }),
      [nodeId]
    );
  }

  async getMentioningContainers(nodeId: string): Promise<NodeReference[]> {
    // Tauri 2.x auto-converts snake_case Rust params to camelCase JS params
    return withDiagnosticLogging(
      'getMentioningContainers',
      () => invoke<NodeReference[]>('get_mentioning_roots', { nodeId }),
      [nodeId]
    );
  }

  async queryNodes(query: NodeQuery): Promise<Node[]> {
    return withDiagnosticLogging(
      'queryNodes',
      () => invoke<Node[]>('query_nodes_simple', { query }),
      [query]
    );
  }

  async executeQuery(input: ExecuteQueryInput): Promise<Node[]> {
    return withDiagnosticLogging(
      'executeQuery',
      () => invoke<Node[]>('execute_query', { request: buildExecuteQueryWire(input) }),
      [input]
    );
  }

  async countQuery(input: ExecuteQueryInput): Promise<number> {
    return withDiagnosticLogging(
      'countQuery',
      () => invoke<number>('count_query', { request: buildExecuteQueryWire(input) }),
      [input]
    );
  }

  async mentionAutocomplete(query: string, limit?: number): Promise<Node[]> {
    return withDiagnosticLogging(
      'mentionAutocomplete',
      () => invoke<Node[]>('mention_autocomplete', { query, limit }),
      [query, limit]
    );
  }

  async findDuplicateFor(
    nodeType: string,
    field: string,
    value: string,
    excludeId?: string
  ): Promise<Node | null> {
    return withDiagnosticLogging(
      'findDuplicateFor',
      () => invoke<Node | null>('find_duplicate', { nodeType, field, value, excludeId }),
      [nodeType, field, value, excludeId]
    );
  }

  async createContainerNode(input: CreateContainerInput): Promise<string> {
    // Keep snake_case for struct fields to match Rust serde expectations
    const rustInput = {
      content: input.content,
      node_type: input.nodeType,
      properties: input.properties ?? {},
      mentioned_by: input.mentionedBy
    };
    return withDiagnosticLogging(
      'createContainerNode',
      () => invoke<string>('create_root_node', { input: rustInput }),
      [input]
    );
  }

  async getAllSchemas(): Promise<SchemaNode[]> {
    return withDiagnosticLogging(
      'getAllSchemas',
      () => invoke<SchemaNode[]>('get_all_schemas'),
      []
    );
  }

  async getSchema(schemaId: string): Promise<SchemaNode> {
    return withDiagnosticLogging(
      'getSchema',
      () => invoke<SchemaNode>('get_schema_definition', { schemaId }),
      [schemaId]
    );
  }

  async searchNodesByTitle(nodeType: string | null, titleContains: string, limit?: number): Promise<Node[]> {
    return this.queryNodes({ nodeType: nodeType ?? undefined, titleContains, limit });
  }

  async getNodeRelationships(nodeId: string): Promise<RawNodeRelationships> {
    // The get_node_relationships command already parses relationships_json into
    // a serde_json::Value and returns it; its shape matches RawNodeRelationships.
    return withDiagnosticLogging(
      'getNodeRelationships',
      () => invoke<RawNodeRelationships>('get_node_relationships', { nodeId }),
      [nodeId]
    );
  }

  async createRelationship(
    sourceId: string,
    relationshipName: string,
    targetId: string,
    edgeData?: Record<string, unknown>
  ): Promise<CreateRelationshipResult> {
    return withDiagnosticLogging(
      'createRelationship',
      () => invoke<CreateRelationshipResult>('create_relationship', {
        sourceId,
        relationshipName,
        targetId,
        edgeData: edgeData ?? null
      }),
      [sourceId, relationshipName, targetId]
    );
  }

  async deleteRelationship(sourceId: string, relationshipName: string, targetId: string): Promise<void> {
    return withDiagnosticLogging(
      'deleteRelationship',
      () => invoke<void>('delete_relationship', { sourceId, relationshipName, targetId }),
      [sourceId, relationshipName, targetId]
    );
  }

  async updateRelationshipProperties(
    sourceId: string,
    relationshipName: string,
    targetId: string,
    properties: Record<string, unknown>
  ): Promise<void> {
    return withDiagnosticLogging(
      'updateRelationshipProperties',
      () => invoke<void>('update_relationship_properties', {
        sourceId,
        relationshipName,
        targetId,
        properties
      }),
      [sourceId, relationshipName, targetId]
    );
  }

  async getDaemonVersion(): Promise<string> {
    return withDiagnosticLogging(
      'getDaemonVersion',
      () => invoke<string>('get_daemon_version'),
      []
    );
  }
}

// ============================================================================
// Shared helpers (not transport-specific — recursion, not wire shaping)
// ============================================================================

/**
 * Flattens a `NodeWithChildren` tree into a level-ordered `Node[]`, excluding
 * the tree's own root. Level order matches the per-level BFS fallback below,
 * so a caller sees the same ordering regardless of which path served it.
 */
function flattenChildrenTree(tree: NodeWithChildren): Node[] {
  const allNodes: Node[] = [];
  const queue: NodeWithChildren[] = [...(tree.children ?? [])];

  while (queue.length > 0) {
    const current = queue.shift()!;
    // Same destructure-and-cast idiom shared-node-store.svelte.ts's
    // loadChildrenTree flatten uses (drop the nested `children` key, keep
    // the rest as the wire-shaped Node) — traversal order here is
    // breadth-first via an explicit queue, not loadChildrenTree's
    // depth-first recursion.
    const { children, ...nodeFields } = current;
    allNodes.push(nodeFields as Node);
    if (children && children.length > 0) {
      queue.push(...children);
    }
  }

  return allNodes;
}

/**
 * Collects all descendants of a node (excluding the node itself), using a
 * single `get_children_tree` round trip in the common case instead of one
 * `get_children` call per descendant.
 *
 * `get_children_tree` refuses to serialize a subtree past a server-side
 * depth or node-count ceiling (`MAX_TREE_DEPTH` / `MAX_TREE_NODES` in
 * `packages/core/src/services/node_service/mod.rs`) — a legitimate, if
 * unusually large or deep, subtree that single call cannot answer. Rather
 * than let that surface as a delete-confirmation failure, fall back to a
 * per-level BFS over `getChildren`, which has no such ceiling and still
 * parallelizes every level instead of awaiting one node at a time.
 */
async function getDescendantsViaChildrenTree(
  rootNodeId: string,
  getChildrenTree: (parentId: string) => Promise<NodeWithChildren | null>,
  getChildren: (parentId: string) => Promise<Node[]>,
): Promise<Node[]> {
  try {
    const tree = await getChildrenTree(rootNodeId);
    return tree ? flattenChildrenTree(tree) : [];
  } catch (error) {
    log.warn(
      `getChildrenTree failed for root ${rootNodeId}; falling back to per-level BFS`,
      error
    );
    return getDescendantsViaChildrenLevels(rootNodeId, getChildren);
  }
}

/** Per-level-parallel BFS fallback — no depth/size ceiling, unlike getChildrenTree. */
async function getDescendantsViaChildrenLevels(
  rootNodeId: string,
  getChildren: (parentId: string) => Promise<Node[]>,
): Promise<Node[]> {
  const allNodes: Node[] = [];
  let currentLevel: string[] = [rootNodeId];

  while (currentLevel.length > 0) {
    const levelResults = await Promise.all(
      currentLevel.map((parentId) => getChildren(parentId))
    );
    const nextLevel: string[] = [];
    for (const children of levelResults) {
      allNodes.push(...children);
      nextLevel.push(...children.map((c) => c.id));
    }
    currentLevel = nextLevel;
  }

  return allNodes;
}

// ============================================================================
// Mock Adapter (Test Environment)
// ============================================================================

class MockAdapter implements BackendAdapter {
  async createNode(_input: CreateNodeInput | Node): Promise<CreatedNode> {
    return { id: 'mock-id', placement: null };
  }
  async getNode(_id: string): Promise<Node | null> {
    return null;
  }
  async updateNode(_id: string, _version: number, _update: UpdateNodeInput): Promise<Node> {
    return {} as Node;
  }
  async updateTaskNode(_id: string, _version: number, _update: TaskNodeUpdate): Promise<TaskNode> {
    return {} as TaskNode;
  }
  async updateIssueNode(
    _id: string,
    _version: number,
    _update: IssueNodeUpdate
  ): Promise<IssueNode> {
    return {} as IssueNode;
  }
  async updatePersonNode(
    _id: string,
    _version: number,
    _update: PersonNodeUpdate
  ): Promise<PersonNode> {
    return {} as PersonNode;
  }
  async updateProjectNode(
    _id: string,
    _version: number,
    _update: ProjectNodeUpdate
  ): Promise<ProjectNode> {
    return {} as ProjectNode;
  }
  async updateSoftwareProjectNode(
    _id: string,
    _version: number,
    _update: SoftwareProjectNodeUpdate
  ): Promise<SoftwareProjectNode> {
    return {} as SoftwareProjectNode;
  }

  async updateQueryNode(
    _id: string,
    _version: number,
    _update: QueryNodeUpdate
  ): Promise<QueryNode> {
    return {} as QueryNode;
  }
  async updatePlayNode(_id: string, _version: number, _update: PlayNodeUpdate): Promise<PlayNode> {
    return {} as PlayNode;
  }
  async updateCollectionNode(
    _id: string,
    _version: number,
    _update: CollectionNodeUpdate
  ): Promise<CollectionNode> {
    return {} as CollectionNode;
  }
  async updateSkillNode(
    _id: string,
    _version: number,
    _update: SkillNodeUpdate
  ): Promise<SkillNode> {
    return {} as SkillNode;
  }
  async updateSpecNode(
    _id: string,
    _version: number,
    _update: SpecNodeUpdate
  ): Promise<SpecNode> {
    return {} as SpecNode;
  }
  async updatePlanNode(
    _id: string,
    _version: number,
    _update: PlanNodeUpdate
  ): Promise<PlanNode> {
    return {} as PlanNode;
  }
  async updateDecisionNode(
    _id: string,
    _version: number,
    _update: DecisionNodeUpdate
  ): Promise<DecisionNode> {
    return {} as DecisionNode;
  }
  async updateDatabaseSettingsNode(
    _id: string,
    _version: number,
    _update: DatabaseSettingsNodeUpdate
  ): Promise<DatabaseSettingsNode> {
    return {} as DatabaseSettingsNode;
  }
  async deleteNode(_id: string, _version: number): Promise<DeleteResult> {
    return { existed: true, deleted_count: 0 };
  }
  async getChildren(_parentId: string): Promise<Node[]> {
    return [];
  }
  async getParent(_nodeId: string): Promise<NodeReference | null> {
    return null;
  }
  async getChildrenTree(parentId: string): Promise<NodeWithChildren | null> {
    // Return null for non-existent parent (consistent with API contract)
    if (!parentId || parentId === 'non-existent') {
      return null;
    }
    // Return realistic mock structure with empty children
    return {
      id: parentId,
      nodeType: 'text',
      content: '',
      version: 0,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      children: []
    };
  }
  async getDescendants(_rootNodeId: string): Promise<Node[]> {
    return [];
  }
  async moveNode(_nodeId: string, _version: number, _newParentId: string | null, _insertPosition: InsertPosition | null): Promise<MovedNode> {
    return { node: {} as Node, placement: null };
  }
  async moveChildrenToParent(_newParentId: string, children: Array<{ id: string; version: number }>): Promise<MovedChildren> {
    return { nodes: children.map(() => ({} as Node)), orders: [] };
  }
  async createMention(_mentioningNodeId: string, _mentionedNodeId: string): Promise<void> {}
  async deleteMention(_mentioningNodeId: string, _mentionedNodeId: string): Promise<void> {}
  async getOutgoingMentions(_nodeId: string): Promise<string[]> {
    return [];
  }
  async getIncomingMentions(_nodeId: string): Promise<string[]> {
    return [];
  }
  async getMentioningContainers(_nodeId: string): Promise<NodeReference[]> {
    return [];
  }
  async queryNodes(_query: NodeQuery): Promise<Node[]> {
    return [];
  }
  async executeQuery(_input: ExecuteQueryInput): Promise<Node[]> {
    return [];
  }
  async countQuery(_input: ExecuteQueryInput): Promise<number> {
    return 0;
  }
  async mentionAutocomplete(_query: string, _limit?: number): Promise<Node[]> {
    return [];
  }
  async findDuplicateFor(
    _nodeType: string,
    _field: string,
    _value: string,
    _excludeId?: string
  ): Promise<Node | null> {
    return null;
  }
  async createContainerNode(_input: CreateContainerInput): Promise<string> {
    return 'mock-container-id';
  }
  async getAllSchemas(): Promise<SchemaNode[]> {
    return [];
  }
  async getSchema(schemaId: string): Promise<SchemaNode> {
    // Return a mock schema node with typed top-level fields
    return {
      id: schemaId,
      nodeType: 'schema',
      content: schemaId,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      lifecycleStatus: 'active',
      properties: {},
      // Typed schema fields at top level (not in properties)
      isCore: false,
      schemaVersion: 1,
      relationships: [],
      fields: []
    };
  }
  async searchNodesByTitle(_nodeType: string | null, _titleContains: string, _limit?: number): Promise<Node[]> {
    return [];
  }
  async getNodeRelationships(nodeId: string): Promise<RawNodeRelationships> {
    return { nodeId, nodeType: '', groups: [] };
  }
  async createRelationship(
    _sourceId: string,
    _relationshipName: string,
    _targetId: string,
    _edgeData?: Record<string, unknown>
  ): Promise<CreateRelationshipResult> {
    return { replaced: [] };
  }
  async deleteRelationship(_sourceId: string, _relationshipName: string, _targetId: string): Promise<void> {}
  async updateRelationshipProperties(
    _sourceId: string,
    _relationshipName: string,
    _targetId: string,
    _properties: Record<string, unknown>
  ): Promise<void> {}
  async getDaemonVersion(): Promise<string> {
    return '0.0.0-mock';
  }
}

// ============================================================================
// Environment Detection & Factory
// ============================================================================

/**
 * Check if running in test environment
 */
function isTestEnvironment(): boolean {
  return typeof process !== 'undefined' && process.env.NODE_ENV === 'test';
}

/**
 * Get the backend adapter for the runtime environment
 *
 * - Test environment: Returns MockAdapter (no-op mocks)
 * - Otherwise: Returns TauriAdapter (uses IPC)
 */
export function getBackendAdapter(): BackendAdapter {
  if (isTestEnvironment()) {
    return new MockAdapter();
  }

  log.debug('Using Tauri IPC adapter');
  return new TauriAdapter();
}

/**
 * Singleton instance for convenient access
 */
export const backendAdapter = getBackendAdapter();
