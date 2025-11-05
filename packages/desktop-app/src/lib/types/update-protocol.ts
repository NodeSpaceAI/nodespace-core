/**
 * Update Protocol Types for Multi-Source Synchronization
 *
 * Defines types for tracking node updates from multiple sources (viewers, database, MCP server)
 * and coordinating real-time synchronization with conflict detection.
 *
 * Phase 2 Implementation: Multi-Source Update Handling
 */

import type { Node } from '$lib/types';

/**
 * Source of a node update - tracks data provenance for debugging and conflict resolution.
 *
 * IMPORTANT: UpdateSource describes WHERE the update originated from (data provenance),
 * NOT whether the node should be persisted. Use UpdateOptions.persist for persistence control.
 *
 * Type Semantics (Phase 1 Refactor - Issue #393):
 * - 'viewer': User interaction in the UI (typing, clicking, editing)
 * - 'database': Loaded from backend database (navigation, fetch, initial load)
 * - 'mcp-server': Synchronized from external MCP client (future Phase 3)
 *
 * Persistence Control:
 * - Use UpdateOptions.persist to control persistence behavior explicitly
 * - Use UpdateOptions.markAsPersistedOnly to track backend-loaded nodes
 * - See UpdateOptions documentation for detailed examples
 *
 * @example
 * // User typing in UI
 * { type: 'viewer', viewerId: 'editor-1' }
 *
 * // Loaded from backend database (mark as already persisted)
 * store.setNode(
 *   node,
 *   { type: 'database', reason: 'navigation' },
 *   { markAsPersistedOnly: true }
 * )
 *
 * // Internal operation requiring immediate persistence
 * store.updateNode(
 *   nodeId,
 *   changes,
 *   { type: 'database', reason: 'reconciliation' },
 *   { persist: 'immediate' }
 * )
 *
 * // Future: MCP client sync
 * { type: 'mcp-server', serverId: 'obsidian-plugin' }
 */
export type UpdateSource =
  | { type: 'viewer'; viewerId: string; userId?: string }
  | { type: 'database'; reason: string }
  | { type: 'mcp-server'; serverId?: string; agentId?: string };

/**
 * Complete node update with metadata for tracking and conflict resolution
 */
export interface NodeUpdate {
  nodeId: string;
  changes: Partial<Node>;
  source: UpdateSource;
  timestamp: number;
  version?: number; // For optimistic concurrency control
  previousVersion?: number; // Previous version for conflict detection
}

/**
 * Detected conflict between two concurrent updates
 */
export interface Conflict {
  nodeId: string;
  localUpdate: NodeUpdate;
  remoteUpdate: NodeUpdate;
  conflictType: 'concurrent-edit' | 'version-mismatch' | 'deleted-node';
  detectedAt: number;
}

/**
 * Result of conflict resolution
 */
export interface ConflictResolution {
  nodeId: string;
  resolvedNode: Node;
  strategy: 'last-write-wins' | 'field-merge' | 'manual' | 'operational-transform';
  discardedUpdate?: NodeUpdate; // Update that was overwritten
  mergedFields?: string[]; // Fields that were merged (for field-level resolution)
}

/**
 * Conflict resolver interface - pluggable strategy pattern
 * Allows upgrading from Last-Write-Wins to Field-Level or OT without rewriting core logic
 */
export interface ConflictResolver {
  /**
   * Resolve a conflict between two updates
   * @param conflict - The detected conflict
   * @param existingNode - The current state of the node before resolution
   * @returns The resolved node state
   */
  resolve(conflict: Conflict, existingNode: Node): ConflictResolution;

  /**
   * Get the name of this resolution strategy
   */
  getStrategyName(): string;
}

/**
 * Subscription callback for node changes
 */
export type NodeChangeCallback = (node: Node, source: UpdateSource) => void;

/**
 * Unsubscribe function returned by subscribe()
 */
export type Unsubscribe = () => void;

/**
 * Performance metrics for SharedNodeStore
 */
export interface StoreMetrics {
  updateCount: number;
  avgUpdateTime: number;
  maxUpdateTime: number;
  subscriptionCount: number;
  conflictCount: number;
  rollbackCount: number;
}

/**
 * Batch configuration for atomic multi-property updates
 */
export interface BatchOptions {
  /** Explicit batch ID (auto-generated if not provided) */
  batchId?: string;
  /** Auto-create batch for this update */
  autoBatch?: boolean;
  /** Max time before auto-commit in ms (default: 2000ms = 2 seconds) */
  batchTimeout?: number;
  /** Immediately commit batch after this update */
  commitImmediately?: boolean;
}

/**
 * Options for updateNode operations
 */
export interface UpdateOptions {
  /** Skip conflict detection (use for trusted sources like database) */
  skipConflictDetection?: boolean;
  /** Skip persistence (for temporary UI-only updates) */
  skipPersistence?: boolean;
  /** Force update even if version mismatch (dangerous) */
  force?: boolean;
  /** Notify subscribers even if no actual changes */
  forceNotify?: boolean;
  /** Additional dependencies for persistence sequencing (node IDs or lambda functions) */
  persistenceDependencies?: Array<string | (() => Promise<void>)>;
  /**
   * Indicates this update is for a computed/derived field (mentions, tags, backlinks, etc.)
   * Automatically implies skipPersistence=true and skipConflictDetection=true
   * Use this for fields that are computed from content and shouldn't be persisted directly
   */
  isComputedField?: boolean;
  /**
   * Batch configuration for atomic multi-property updates
   * Used for pattern conversions where content + nodeType must persist together
   */
  batch?: BatchOptions;
  /**
   * Explicit persistence control (NEW - Phase 1 of UpdateSource refactor)
   *
   * Clarifies persistence intent independently of UpdateSource type:
   * - 'immediate': Persist immediately (bypasses debouncing)
   * - 'debounced': Use normal debounced persistence (default for user edits)
   * - false: Skip persistence (for in-memory/temporary updates)
   * - true: Trigger persistence with auto-determined mode (structural=immediate, content=debounced)
   * - undefined: Use auto-determined persistence based on source type (legacy default)
   *
   * The difference between `true` and `undefined`:
   * - `true`: Explicitly request persistence with auto mode selection
   * - `undefined`: Defer to legacy source.type-based behavior (Phase 1 backward compatibility)
   *
   * @example
   * // Explicit immediate persistence (instead of using 'external' source)
   * store.setNode(node, source, { persist: 'immediate' })
   *
   * // Explicitly skip persistence (clearer than skipPersistence flag)
   * store.setNode(node, source, { persist: false })
   *
   * // Force persistence with auto-determined mode (useful for structural changes)
   * store.updateNode(nodeId, { parentId: newParent }, source, { persist: true })
   *
   * // Mark node as already persisted without re-persisting
   * store.setNode(node, source, { markAsPersistedOnly: true })
   */
  persist?: boolean | 'debounced' | 'immediate';
  /**
   * Mark node as already persisted in database without triggering persistence operation.
   *
   * Use when loading nodes from backend to track persistence state explicitly.
   * Replaces implicit side-effect of using `type: 'database'` source.
   *
   * @example
   * // Loading node from backend (new explicit approach)
   * store.setNode(node, { type: 'backend-load', reason: 'navigation' }, {
   *   markAsPersistedOnly: true  // Explicitly mark as persisted
   * })
   */
  markAsPersistedOnly?: boolean;
}
