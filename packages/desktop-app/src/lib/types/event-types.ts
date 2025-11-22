/**
 * Event Types for LIVE SELECT and Internal Events
 *
 * Defines the event data structures used for real-time synchronization
 * via Tauri LIVE SELECT polling. These types are consumed by:
 * - tauri-sync-listener.ts (event bridging)
 * - reactive-node-data.svelte.ts (node state)
 * - reactive-structure-tree.svelte.ts (hierarchy state)
 */

// ============================================================================
// Node Event Data
// ============================================================================

/**
 * Node event data from LIVE SELECT
 * Contains minimal fields for real-time sync
 */
export interface NodeEventData {
  id: string;
  nodeType: string;
  content: string;
  version: number;
  modifiedAt: string;
}

// ============================================================================
// Hierarchy Event Data (Parent-Child Relationships)
// ============================================================================

/**
 * Hierarchy relationship event data from LIVE SELECT
 * Represents parent-child relationships in the node tree
 *
 * Note: The `in` and `out` field names come from SurrealDB's graph edge format.
 * Frontend code should use the domain-friendly aliases: parentId and childId.
 */
export interface HierarchyRelationship {
  id: string;
  in: string; // Parent node ID (SurrealDB edge format)
  out: string; // Child node ID (SurrealDB edge format)
  order: number; // Sort order for children (fractional ordering)
}

// Domain-friendly getters for the relationship
export function getParentId(rel: HierarchyRelationship): string {
  return rel.in;
}

export function getChildId(rel: HierarchyRelationship): string {
  return rel.out;
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
