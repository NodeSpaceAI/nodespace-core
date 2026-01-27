/**
 * Unified Node Type System
 *
 * This is the ONLY node schema in the codebase.
 * Matches Rust backend EXACTLY - no other Node interfaces should exist.
 *
 * Philosophy: Single source of truth, zero schema drift.
 */

/**
 * Node - Matches Rust backend schema exactly
 *
 * This is the authoritative node type. All services and components
 * must use this interface.
 *
 * Fields:
 * - Persisted: Stored in database
 * - Computed: Calculated on-demand, not stored
 */
export interface Node {
  // ============================================================================
  // Persisted Fields (stored in database)
  // ============================================================================

  /** Unique identifier (UUID or deterministic like YYYY-MM-DD for dates) */
  id: string;

  /** Node type (e.g., "text", "task", "date") */
  nodeType: string;

  /** Primary content/text of the node */
  content: string;

  /**
   * Parent node ID (graph edge relationship)
   * Managed via graph edges in backend, cached in frontend for performance
   * null = root-level node (no parent)
   */
  parentId?: string | null;

  /** Creation timestamp (ISO 8601) - backend sets this */
  createdAt: string;

  /** Last modification timestamp (ISO 8601) - backend auto-updates this */
  modifiedAt: string;

  /**
   * Optimistic Concurrency Control (OCC) version counter
   *
   * This field enables safe concurrent modifications by multiple clients (Frontend UI,
   * MCP servers, AI assistants) without database locks.
   *
   * ## How OCC Works
   *
   * 1. **Read**: Client fetches node with current version (e.g., `version: 5`)
   * 2. **Modify**: Client makes local changes while holding version reference
   * 3. **Write**: Client submits update with expected version (`version: 5`)
   * 4. **Verify**: Backend atomically checks if current version still matches
   *    - Match → Update succeeds, version increments to 6
   *    - Mismatch → Update fails with VERSION_CONFLICT error
   *
   * ## Version Lifecycle
   *
   * - **Initial value**: 1 (when node is first created)
   * - **Increments**: On every successful update/move/reorder operation
   * - **Never decrements**: Monotonically increasing
   * - **Survives**: All modification types (content, properties, hierarchy)
   *
   * ## Usage Requirements
   *
   * **CRITICAL**: Always provide this field when calling update/delete/move/reorder:
   *
   * ```typescript
   * // ✅ CORRECT: Provide version from latest read
   * const node = await getNode(nodeId);
   * await updateNode(nodeId, node.version, { content: 'New content' });
   *
   * // ❌ WRONG: Don't use stale version from cache
   * await updateNode(nodeId, cachedVersion, { content: 'New content' });
   *
   * // ❌ WRONG: Never hardcode version numbers
   * await updateNode(nodeId, 1, { content: 'New content' });
   * ```
   *
   * ## Conflict Handling
   *
   * When you receive a VERSION_CONFLICT error:
   *
   * 1. Error includes current node state for merge reference
   * 2. Frontend shows conflict resolution UI (auto-merge or manual)
   * 3. MCP clients implement domain-specific merge logic
   * 4. Retry with merged changes and fresh version
   *
   * Example conflict response:
   * ```typescript
   * {
   *   error: "VERSION_CONFLICT",
   *   expectedVersion: 5,
   *   actualVersion: 7,
   *   currentNode: { ...latestState }
   * }
   * ```
   *
   * ## Performance Impact
   *
   * - Overhead: < 5ms per operation (empirically validated)
   * - No database locks required (optimistic approach)
   * - Scales linearly with concurrent clients
   *
   * ## Security Notes
   *
   * - Version parameter is **mandatory** (not optional) to prevent TOCTOU attacks
   * - Clients cannot bypass version checks (enforced by backend)
   * - Version spoofing is impossible (must match current exactly)
   *
   * @see /docs/architecture/data/optimistic-concurrency-control.md - Complete OCC guide
   * @see ConflictResolver - Frontend auto-merge strategies
   */
  version: number;

  /** All entity-specific fields (Pure JSON schema) */
  properties: Record<string, unknown>;

  /** Optional vector embedding for semantic search (F32 blob) */
  embeddingVector?: number[] | null;

  /**
   * Indexed title for efficient @mention autocomplete search (Issue #821)
   *
   * Contains markdown-stripped content for clean display and search.
   * Populated only for:
   * - Root nodes (no parent) - excludes date and schema types
   * - Task nodes (always, regardless of hierarchy)
   *
   * For other nodes (child text, headers, etc.), this field is undefined.
   */
  title?: string | null;

  // ============================================================================
  // Computed Fields (NOT persisted, calculated on-demand)
  // ============================================================================

  /**
   * Extracted mentions from content (e.g., @node-id)
   * Computed by ContentProcessor.extractMentions(node.content)
   * NOT stored in database
   */
  mentions?: string[];

  /**
   * Collection memberships - IDs of collections this node belongs to
   *
   * Populated from member_of edges in the database (member_of.in = this.id).
   * This field is read-only and computed on query - to modify memberships,
   * use the add_to_collection/remove_from_collection operations.
   *
   * ## Collection System Overview
   *
   * Collections are a flexible organizational structure (like tags but hierarchical):
   * - Any node can belong to multiple collections (many-to-many)
   * - Collections can be nested (DAG structure, not tree)
   * - Unlike parent/child hierarchy, this is a "membership" relationship
   *
   * ## Path Syntax
   *
   * Collections are organized using colon-separated paths:
   * - "hr" → Top-level collection
   * - "hr:policy" → "policy" collection under "hr"
   * - "hr:policy:vacation" → "vacation" under "hr:policy"
   *
   * ## Usage
   *
   * ```typescript
   * // Check if node belongs to any collections
   * if (node.memberOf && node.memberOf.length > 0) {
   *   console.log('Node belongs to:', node.memberOf);
   * }
   *
   * // Add node to collection via MCP
   * await updateNode(node.id, node.version, { add_to_collection: 'hr:policy' });
   *
   * // Remove from collection
   * await updateNode(node.id, node.version, { remove_from_collection: 'collection-id' });
   * ```
   *
   * @see CollectionNode for the collection node type itself
   */
  memberOf?: string[];

  // ============================================================================
  // Spoke Fields (for typed nodes like TaskNode)
  // ============================================================================
  // These fields are populated when the backend returns a typed node.
  // They're at the top level (flat structure) matching Rust serialization.
  // See TaskNode interface for strong typing.

  /** Task status (for task nodes) */
  status?: string;

  /** Task priority (for task nodes) */
  priority?: number | string;

  /** Task due date (for task nodes) */
  dueDate?: string | null;

  /** Task assignee (for task nodes) */
  assignee?: string | null;
}

// ============================================================================
// Collection Node Types
// ============================================================================

/**
 * CollectionNode - A specialized node type for organizing other nodes
 *
 * Collections provide a flexible, hierarchical organizational structure similar
 * to tags but with additional features:
 * - Hierarchical nesting (collections can contain sub-collections)
 * - Many-to-many membership (nodes can belong to multiple collections)
 * - DAG structure (directed acyclic graph - not strictly tree)
 * - Path-based navigation (e.g., "hr:policy:vacation")
 *
 * ## Path Syntax
 *
 * Collections use colon-separated paths for intuitive navigation:
 * - "hr" → Top-level HR collection
 * - "hr:policy" → Policy sub-collection under HR
 * - "hr:policy:vacation" → Vacation policy under HR policy
 *
 * ## Usage Example
 *
 * ```typescript
 * // Create a collection
 * const collection = await createNode({
 *   nodeType: 'collection',
 *   content: 'HR Policies',
 *   properties: { description: 'Human resources policy documents' }
 * });
 *
 * // Add a document to the collection
 * await updateNode(docId, docVersion, { add_to_collection: 'hr:policy' });
 *
 * // Query all members of a collection
 * const members = await queryNodes({ collection: 'hr:policy' });
 * ```
 *
 * ## Difference from Parent-Child Hierarchy
 *
 * | Feature | Parent-Child | Collections |
 * |---------|-------------|-------------|
 * | Cardinality | Node has 1 parent | Node has N collections |
 * | Structure | Tree | DAG (directed acyclic graph) |
 * | Use case | Document structure | Cross-cutting organization |
 * | Path syntax | N/A | colon-separated (hr:policy) |
 *
 * @see Node.memberOf for collection membership on regular nodes
 */
export interface CollectionNode extends Node {
  /** Always 'collection' for collection nodes */
  nodeType: 'collection';

  /** Collection-specific properties */
  properties: {
    /** Optional description of the collection's purpose */
    description?: string;

    /** Optional icon identifier for UI display */
    icon?: string;

    /** Optional color for UI display (hex or color name) */
    color?: string;

    /** Allow additional plugin/custom properties */
    [key: string]: unknown;
  };
}

/**
 * Type guard to check if a node is a CollectionNode
 */
export function isCollectionNode(node: Node): node is CollectionNode {
  return node.nodeType === 'collection';
}

/**
 * Collection membership info - extended data about a node's collection memberships
 *
 * Used when you need more than just the collection IDs (which are in node.memberOf).
 * This provides full collection details for UI display.
 */
export interface CollectionMembership {
  /** The collection node this membership refers to */
  collection: CollectionNode;

  /** When the membership was created */
  addedAt: string;
}

/**
 * Collection path segment - used when parsing collection paths
 *
 * Each segment represents one level in the path hierarchy.
 */
export interface CollectionPathSegment {
  /** The segment name (e.g., "policy" in "hr:policy") */
  name: string;

  /** The resolved collection node ID, if known */
  collectionId?: string;
}

/**
 * Parse a collection path into segments
 *
 * @param path - Collection path like "hr:policy:vacation"
 * @returns Array of path segments
 *
 * @example
 * ```typescript
 * const segments = parseCollectionPath('hr:policy:vacation');
 * // Returns: [{ name: 'hr' }, { name: 'policy' }, { name: 'vacation' }]
 * ```
 */
export function parseCollectionPath(path: string): CollectionPathSegment[] {
  if (!path || path.trim() === '') {
    return [];
  }

  return path
    .split(':')
    .filter((segment) => segment.trim() !== '')
    .map((name) => ({ name: name.trim() }));
}

/**
 * Format collection path segments back into a path string
 *
 * @param segments - Array of path segments
 * @returns Colon-separated path string
 */
export function formatCollectionPath(segments: CollectionPathSegment[]): string {
  return segments.map((s) => s.name).join(':');
}

/**
 * NodeUpdate - Partial updates for PATCH operations
 *
 * All fields optional to support partial updates.
 * Only provided fields will be updated.
 *
 * Note: created_at and modified_at are NOT updatable.
 * Backend automatically sets modified_at on updates.
 */
/**
 * NodeUpdate - Partial node update interface
 *
 * This interface maps to Rust's `NodeUpdate` struct which uses Option<Option<T>>
 * for nullable fields. Understanding this mapping is critical for correct updates.
 *
 * ## Type System Mapping (TypeScript → Rust)
 *
 * For nullable fields (parentId, embeddingVector):
 *
 * | TypeScript Value | Meaning | Rust Type | Behavior |
 * |-----------------|---------|-----------|----------|
 * | `undefined` (field omitted) | Don't update this field | `None` | Field unchanged in database |
 * | `null` | Clear this field | `Some(None)` | Field set to NULL in database |
 * | `"value"` | Set to this value | `Some(Some("value"))` | Field set to value in database |
 *
 * ## Example Usage
 *
 * ```typescript
 * // Don't update parentId (leave as-is)
 * updateNode('node-1', { content: 'New content' });
 *
 * // Clear parentId (set to NULL, making it a root node)
 * updateNode('node-1', { parentId: null });
 *
 * // Set parentId to a value
 * updateNode('node-1', { parentId: 'parent-node-id' });
 * ```
 *
 * ## Implementation Details
 *
 * The Rust backend uses a custom deserializer (see nodespace-core/src/models/node.rs)
 * to handle this three-way mapping. This allows TypeScript to use natural optional
 * types while Rust maintains explicit control over "don't update" vs "set to null".
 */
export interface NodeUpdate {
  /** Update node type */
  nodeType?: string;

  /** Update primary content */
  content?: string;

  /** Update or merge properties */
  properties?: Record<string, unknown>;

  /**
   * Update embedding vector
   *
   * - `undefined`: Don't update (keep current vector)
   * - `null`: Clear vector (set to NULL)
   * - `number[]`: Set to new vector
   */
  embeddingVector?: number[] | null;
}

/**
 * NodeUIState - Separate UI state storage
 *
 * Stored in parallel Map in ReactiveNodeService.
 * Keeps UI concerns separate from data model.
 *
 * Why separate?
 * - Data (Node) is persisted and synced
 * - UI state is ephemeral and local-only
 * - Clean separation of concerns
 */
export interface NodeUIState {
  /** Node ID this state belongs to */
  nodeId: string;

  /** Hierarchy depth (0 = root, 1 = child of root, etc.) */
  depth: number;

  /** Whether node's children are visible */
  expanded: boolean;

  /** Whether this node should receive focus */
  autoFocus: boolean;

  /** Inherited header level for rendering */
  inheritHeaderLevel: number;

  /** Whether this is a placeholder node (not yet persisted) */
  isPlaceholder: boolean;
}

/**
 * Type guard to check if an object is a Node
 */
export function isNode(obj: unknown): obj is Node {
  if (typeof obj !== 'object' || obj === null) return false;

  const node = obj as Record<string, unknown>;

  return (
    typeof node.id === 'string' &&
    typeof node.nodeType === 'string' &&
    typeof node.content === 'string' &&
    typeof node.createdAt === 'string' &&
    typeof node.modifiedAt === 'string' &&
    typeof node.properties === 'object' &&
    node.properties !== null
  );
}

/**
 * Create default UI state for a node
 */
export function createDefaultUIState(
  nodeId: string,
  overrides?: Partial<NodeUIState>
): NodeUIState {
  return {
    nodeId,
    depth: 0,
    expanded: false,
    autoFocus: false,
    inheritHeaderLevel: 0,
    isPlaceholder: false,
    ...overrides
  };
}
