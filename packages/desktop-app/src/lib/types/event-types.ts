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
// Edge Event Data
// ============================================================================

/**
 * Edge event data from LIVE SELECT
 * Represents parent-child relationships
 */
export interface EdgeEventData {
  id: string;
  in: string; // Parent node ID
  out: string; // Child node ID
  edgeType: string;
  positionId?: string;
  order: number; // Sort order for children
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
