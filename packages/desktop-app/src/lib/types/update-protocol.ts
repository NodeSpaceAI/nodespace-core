/**
 * Update Protocol Types for Multi-Source Synchronization
 *
 * Defines types for tracking node updates from multiple sources (viewers, database, MCP server)
 * and coordinating real-time synchronization.
 */

import type { Node } from '$lib/types';

/**
 * Source of a node update - tracks data provenance for debugging and conflict resolution.
 *
 * IMPORTANT: UpdateSource describes WHERE the update originated from (data provenance),
 * NOT whether the node should be persisted. Use UpdateOptions.persist for persistence control.
 *
 * Type Semantics (Phase 1 Refactor):
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
  previousVersion?: number; // Snapshot of version before update, used for rollback
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
  /** Skip persistence (for temporary UI-only updates) */
  skipPersistence?: boolean;
  /**
   * Replace `properties` wholesale instead of deep-merging the incoming patch
   * onto the existing bag. Default (false/undefined) deep-merges so a partial
   * `{ properties: {...} }` write doesn't drop sibling keys. Set true when the
   * caller has already computed the full intended `properties` bag (e.g.
   * `updateNodeProperties`, which does its own merge) and must not have it
   * re-merged.
   */
  replaceProperties?: boolean;
  /** Force update even if version mismatch (dangerous) */
  force?: boolean;
  /** Notify subscribers even if no actual changes */
  forceNotify?: boolean;
  /** Additional dependencies for persistence sequencing (node IDs or lambda functions) */
  persistenceDependencies?: Array<string | (() => Promise<void>)>;
  /**
   * Indicates this update is for a computed/derived field (mentions, tags, backlinks, etc.)
   * Automatically implies skipPersistence=true.
   * Use this for fields that are computed from content and shouldn't be persisted directly.
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
   * - undefined: Use auto-determined persistence based on source type (default behavior)
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
  /**
   * Called when this specific write's change did not reach the server:
   * - it failed for a non-OCC reason (network error, daemon offline,
   *   validation error);
   * - another write's version conflict dropped or skipped it before it was
   *   sent.
   * A version conflict on this write's own RPC does not invoke it — that has
   * its own resync/hydration handling. Neither does a newer write for the same
   * node replacing it. The store still applies its own
   * generic handling regardless (metrics, a `write-failure` conflict
   * notification) — this is an *additional*, opt-in hook for a caller that
   * knows how to make a more specific, narrowly-scoped correction than the
   * store's generic handling can (e.g. reverting just the one field this
   * particular call changed, rather than any store-wide mechanism).
   */
  onPersistError?: (_error: Error) => void;
  /**
   * Called when this specific write's persistence succeeds. Opt-in,
   * symmetric with `onPersistError` — for a caller that needs to know its
   * own write settled (e.g. to stop tracking a since-superseded revert
   * target once a later write in a chain is confirmed), not for general
   * "did it save" UI, which reads the store's resulting state instead.
   */
  onPersistSuccess?: () => void;
}
