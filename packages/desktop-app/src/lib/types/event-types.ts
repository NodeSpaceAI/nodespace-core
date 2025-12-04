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
// Mention Event Data (Bidirectional References)
// ============================================================================

/**
 * Mention relationship event data from domain events
 * Represents bidirectional references between nodes
 *
 * Unlike hierarchies, mentions are not parent-child relationships
 * and do not have ordering. They represent connections or references.
 */
export interface MentionRelationship {
  sourceId: string; // Node making the reference
  targetId: string; // Node being referenced
}

// ============================================================================
// Edge Relationship Type (Union of all edge types)
// ============================================================================

/**
 * Edge relationship event data from domain events
 * Internally-tagged union type representing different relationship types
 *
 * Rust uses #[serde(tag = "type")] which produces an internally-tagged format
 * where the discriminator is merged with the struct fields (NOT nested):
 *
 * - Hierarchy: { type: "hierarchy", parentId: "...", childId: "...", order: 1.0 }
 * - Mention: { type: "mention", sourceId: "...", targetId: "..." }
 *
 * TypeScript intersection types model this correctly.
 */
export type HierarchyEdge = { type: 'hierarchy' } & HierarchyRelationship;
export type MentionEdge = { type: 'mention' } & MentionRelationship;
export type EdgeRelationship = HierarchyEdge | MentionEdge;

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
