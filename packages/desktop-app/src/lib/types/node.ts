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

  // ============================================================================
  // Computed Fields (NOT persisted, calculated on-demand)
  // ============================================================================

  /**
   * Extracted mentions from content (e.g., @node-id)
   * Computed by ContentProcessor.extractMentions(node.content)
   * NOT stored in database
   */
  mentions?: string[];
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
