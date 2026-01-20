/**
 * Event Types for Domain Events
 *
 * Defines the event data structures used for real-time synchronization
 * via domain events. The backend emits domain events from NodeService,
 * which are forwarded to the frontend via Tauri events (desktop) or
 * SSE (browser dev mode). These types are consumed by:
 * - tauri-sync-listener.ts (event bridging)
 * - shared-node-store.svelte.ts (node state)
 * - reactive-structure-tree.svelte.ts (hierarchy state)
 */

// ============================================================================
// Node Event Data
// ============================================================================

/**
 * Node event data from domain events (Issue #724)
 *
 * Events now send only node ID for efficiency.
 * Frontend fetches full node data via get_node() API if needed.
 */
export interface NodeEventData {
  id: string;
}

// ============================================================================
// Hierarchy Event Data (Parent-Child Relationships)
// ============================================================================

/**
 * Hierarchy relationship event data from domain events
 * Represents parent-child relationships in the node tree
 *
 * Backend converts SurrealDB edge format to this type at the
 * serialization boundary, isolating database implementation details.
 */
export interface HierarchyRelationship {
  parentId: string; // Parent node ID
  childId: string; // Child node ID
  order: number; // Sort order for children (fractional ordering)
}

// ============================================================================
// Unified Relationship Event (Issue #811)
// ============================================================================

/**
 * Unified relationship event for all relationship types
 *
 * This generic structure supports all relationship types: has_child, member_of,
 * mentions, and custom types. It replaces the enum-based approach that required
 * modifying the event system for each new relationship type.
 *
 * Emitted by the store layer for all relationship operations.
 */
export interface RelationshipEvent {
  /** Unique relationship ID in SurrealDB format (e.g., "relationship:abc123") */
  id: string;
  /** Source node ID (the "from" node in the relationship) */
  fromId: string;
  /** Target node ID (the "to" node in the relationship) */
  toId: string;
  /** Relationship type: "has_child", "member_of", "mentions", or custom types */
  relationshipType: string;
  /** Type-specific properties (order for has_child, context for mentions, etc.) */
  properties: Record<string, unknown>;
}

/**
 * Payload for relationship deleted events
 *
 * Contains the ID, from/to node IDs, and type.
 */
export interface RelationshipDeletedPayload {
  id: string;
  fromId: string;
  toId: string;
  relationshipType: string;
}

// ============================================================================
// Nested Tree Structure (for Recursive FETCH optimization)
// ============================================================================

/**
 * Node with nested children structure for recursive tree fetching
 *
 * Represents a single node with all its descendants recursively nested.
 * Used for optimizing initial load by fetching the entire subtree in
 * a single database query instead of reconstructing the tree in the frontend.
 */
export interface NodeWithChildren {
  // Node fields (same as Node interface)
  id: string;
  nodeType: string;
  content: string;
  version: number;
  createdAt: string;
  modifiedAt: string;
  properties?: Record<string, unknown>;
  embeddingVector?: number[];
  embeddingStale?: boolean;
  mentions?: string[];
  mentionedBy?: string[];
  _schema_version?: number;

  // Nested children (recursive)
  children?: NodeWithChildren[];
}

// ============================================================================
// Error Events
// ============================================================================

/**
 * Persistence failure event for optimistic operation rollbacks
 * Emitted when a backend operation fails after optimistic update
 */
export interface PersistenceFailedEvent {
  type: 'error:persistence-failed';
  namespace: 'error';
  timestamp: number;
  source: string;
  message: string;
  failedNodeIds: string[];
  failureReason: 'timeout' | 'foreign-key-constraint' | 'database-locked' | 'unknown';
  canRetry: boolean;
  affectedOperations: Array<{
    nodeId: string;
    operation: string;
    error: string;
  }>;
  metadata?: {
    originalError: string;
    description: string;
  };
}
