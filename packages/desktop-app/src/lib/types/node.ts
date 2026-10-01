/**
 * Unified Node Type System
 *
 * This is the ONLY node schema in the codebase.
 * Matches Rust backend EXACTLY - no other Node interfaces should exist.
 *
 * Philosophy: Single source of truth, zero schema drift.
 */

import { isExactly } from './core-node-types';

/**
 * Lightweight reference to a node for backlinks display
 *
 * Contains minimal data needed to show a link: id, title, and type.
 * Used by the `mentionedIn` field to provide backlinks without N+1 queries.
 */
export interface NodeReference {
  /** Node ID */
  id: string;
  /** Display title (markdown-stripped content for root/task nodes) */
  title: string | null;
  /** Node type (e.g., "text", "task", "date") */
  nodeType: string;
}

/**
 * Direction of a relationship relative to a node
 *
 * - `out`: The relationship points FROM this node TO another (outgoing)
 * - `in`: The relationship points FROM another node TO this node (incoming)
 */
export type RelationshipDirection = 'out' | 'in';

/**
 * Represents a relationship with another node, including direction
 *
 * Used for returning relationships in subtree queries where we need to know
 * both the related node and the direction of the relationship.
 *
 * For outgoing relationships (direction = 'out'):
 * - `id` is the target node ID
 * - The current node points TO this node
 *
 * For incoming relationships (direction = 'in'):
 * - `id` is the source node ID
 * - This node points TO the current node
 */
export interface NodeRelationship {
  /** The related node's ID */
  id: string;
  /** The related node's title (for display) */
  title: string | null;
  /** Direction of the relationship relative to the node this is attached to */
  direction: RelationshipDirection;
  /** Type of relationship (e.g., "mentions", "has_child", "member_of") */
  relationshipType: string;
}

/** Lifecycle status of a node: governance state, never a type-specific flag. */
export type LifecycleStatus = 'active' | 'archived';

/**
 * NodeEnvelope - the fields every node carries on the wire
 *
 * Mirrors Rust's `NodeEnvelope` (`packages/nodespace-types/src/node.rs`). The
 * generic `Node` and every typed node interface extend it, so no node
 * interface restates or omits a universal field.
 *
 * Fields:
 * - Persisted: Stored in database
 * - Computed: Calculated on-demand, not stored
 */
export interface NodeEnvelope {
  // ============================================================================
  // Persisted Fields (stored in database)
  // ============================================================================

  /** Unique identifier (UUID or deterministic like YYYY-MM-DD for dates) */
  id: string;

  /** Node type (e.g., "text", "task", "date") */
  nodeType: string;

  /** Primary content/text of the node */
  content: string;

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
   */
  version: number;

  /**
   * Governance state, always present on the wire (ADR-087). Never read for a
   * type-specific meaning: `archived` is not "done" or "retired" for any type.
   */
  lifecycleStatus: LifecycleStatus;

  /** All entity-specific fields (Pure JSON schema) */
  properties: Record<string, unknown>;

  /**
   * Indexed title for efficient @mention autocomplete search
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
   * Derived from content by ReactiveNodeService.updateNodeMentions
   * NOT stored in database
   */
  mentions?: string[];

  /**
   * Nodes that mention this node (backlinks) with preview data
   *
   * Populated during root fetch (get_children_tree) for efficient UI display.
   * Contains {id, title, nodeType} for each mentioning node's container (root or task).
   *
   * This eliminates N+1 queries - backlink data comes with the initial node fetch.
   * The SharedNodeStore caches this data, and domain events trigger refetch on changes.
   *
   * ## Usage in BacklinksPanel
   *
   * ```typescript
   * let node = $derived(sharedNodeStore.getNode(nodeId));
   * let backlinks = $derived(node?.mentionedIn ?? []);
   *
   * {#each backlinks as backlink}
   *   <a href="nodespace://{backlink.id}">{backlink.title || backlink.id}</a>
   * {/each}
   * ```
   */
  mentionedIn?: NodeReference[];
}

/**
 * Node - the generic node: the envelope, with every field of its type inside
 * `properties`. A primitive type has no fields, so this is also its whole
 * wire shape.
 *
 * All services and components use this type for a node of any type; a type
 * with its own fields has a typed interface that extends `NodeEnvelope`.
 */
export type Node = NodeEnvelope;

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
 * await collectionService.addNodeToCollectionPath(docId, 'hr:policy');
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
 */
export interface CollectionNode extends NodeEnvelope {
  /** Always 'collection' for collection nodes */
  nodeType: 'collection';

  /** Collection-specific properties */
  properties: {
    /** Optional description of the collection's purpose */
    description?: string;

    /** Allow additional plugin/custom properties */
    [key: string]: unknown;
  };
}

/**
 * Type guard to check if a node is a CollectionNode
 */
export function isCollectionNode(node: Node): node is CollectionNode {
  return isExactly(node.nodeType, 'collection');
}

/**
 * Collection membership info - extended data about a node's collection memberships
 *
 * Provides full collection details for UI display.
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
 * Maps to Rust's `NodeUpdate` struct. An omitted field is left unchanged.
 *
 * ```typescript
 * // Update content only
 * updateNode('node-1', { content: 'New content' });
 * ```
 */
export interface NodeUpdate {
  /** Update node type */
  nodeType?: string;

  /** Update primary content */
  content?: string;

  /** Update or merge properties */
  properties?: Record<string, unknown>;
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
