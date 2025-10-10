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

  /** Parent node ID (creation context) */
  parentId: string | null;

  /**
   * Root document ID (NULL means this node IS root/page)
   *
   * Rule: containerNodeId = parentId for direct children of roots
   * Example:
   *   Date node: { parentId: null, containerNodeId: null }  // IS root
   *   Child of date: { parentId: "2025-10-04", containerNodeId: "2025-10-04" }
   */
  containerNodeId: string | null;

  /** Sibling ordering reference (single-pointer linked list) */
  beforeSiblingId: string | null;

  /** Creation timestamp (ISO 8601) - backend sets this */
  createdAt: string;

  /** Last modification timestamp (ISO 8601) - backend auto-updates this */
  modifiedAt: string;

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
export interface NodeUpdate {
  /** Update node type */
  nodeType?: string;

  /** Update primary content */
  content?: string;

  /** Update parent reference */
  parentId?: string | null;

  /** Update root reference */
  containerNodeId?: string | null;

  /** Update sibling ordering */
  beforeSiblingId?: string | null;

  /** Update or merge properties */
  properties?: Record<string, unknown>;

  /** Update embedding vector */
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
    (node.parentId === null || typeof node.parentId === 'string') &&
    (node.containerNodeId === null || typeof node.containerNodeId === 'string') &&
    (node.beforeSiblingId === null || typeof node.beforeSiblingId === 'string') &&
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
