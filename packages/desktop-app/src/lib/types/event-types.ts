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
