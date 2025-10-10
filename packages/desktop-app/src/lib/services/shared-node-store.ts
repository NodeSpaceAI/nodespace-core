/**
 * SharedNodeStore - Singleton Reactive Store for Multi-Viewer Support
 *
 * Phase 1-2 Implementation:
 * - Single source of truth for all node data (Svelte 5 $state)
 * - Observer pattern for viewer subscriptions
 * - Real-time synchronization across multiple viewers
 * - Conflict detection and resolution (Last-Write-Wins)
 * - Optimistic updates with rollback
 * - Performance tracking and metrics
 *
 * Architecture:
 * - Singleton pattern ensures single shared store
 * - Multiple ReactiveNodeService instances read from same store
 * - Per-viewer UI state (expand/collapse, focus) stored separately
 * - Database writes coordinated through existing queueDatabaseWrite()
 *
 * Future Phases:
 * - Phase 3: MCP server integration
 * - Phase 4-5: Tab/Pane management and coordination
 * - Phase 6: Memory optimization and lazy loading
 * - Phase 7: Advanced conflict resolution (OT/CRDT)
 */

import { eventBus } from './event-bus';
import { tauriNodeService } from './tauri-node-service';
import type { Node } from '$lib/types';
import type {
  NodeUpdate,
  UpdateSource,
  Conflict,
  ConflictResolver,
  NodeChangeCallback,
  Unsubscribe,
  StoreMetrics,
  UpdateOptions
} from '$lib/types/update-protocol';
import type { ConflictResolvedEvent, UpdateRolledBackEvent } from './event-types';
import { createDefaultResolver } from './conflict-resolvers';

// ============================================================================
// Database Write Coordination (Phase 2.4)
// ============================================================================

// Database write queue for coordinated persistence
const pendingDatabaseWrites = new Map<string, Promise<void>>();

/**
 * Queue a database write operation to prevent concurrent writes to same node
 * This ensures database consistency and prevents race conditions
 */
async function queueDatabaseWrite(nodeId: string, operation: () => Promise<void>): Promise<void> {
  // Wait for any pending writes to this node
  const pending = pendingDatabaseWrites.get(nodeId);
  if (pending) {
    await pending;
  }

  // Execute and track this write
  const writePromise = operation();
  pendingDatabaseWrites.set(nodeId, writePromise);

  try {
    await writePromise;
  } finally {
    // Clean up if this was the last write
    if (pendingDatabaseWrites.get(nodeId) === writePromise) {
      pendingDatabaseWrites.delete(nodeId);
    }
  }
}

/**
 * Subscription metadata for debugging and cleanup
 */
interface Subscription {
  id: string;
  nodeId: string | null; // null = subscribe to all nodes
  callback: NodeChangeCallback;
  createdAt: number;
  callCount: number;
}

/**
 * SharedNodeStore - Reactive singleton store for node data
 */
export class SharedNodeStore {
  private static instance: SharedNodeStore | null = null;

  // Core state - using plain Map (reactivity provided by ReactiveNodeService adapter)
  private nodes = new Map<string, Node>();

  // Subscriptions for change notifications
  private subscriptions = new Map<string, Set<Subscription>>();
  private wildcardSubscriptions = new Set<Subscription>();
  private subscriptionIdCounter = 0;

  // Conflict resolution
  private conflictResolver: ConflictResolver = createDefaultResolver();
  private conflictWindowMs = 5000; // 5 seconds default for conflict detection

  // Pending operations (optimistic updates)
  private pendingUpdates = new Map<string, NodeUpdate[]>();

  // Performance metrics
  private metrics: StoreMetrics = {
    updateCount: 0,
    avgUpdateTime: 0,
    maxUpdateTime: 0,
    subscriptionCount: 0,
    conflictCount: 0,
    rollbackCount: 0
  };

  // Version tracking for optimistic concurrency
  private versions = new Map<string, number>();

  private constructor() {
    // Private constructor for singleton
  }

  /**
   * Get singleton instance
   */
  static getInstance(): SharedNodeStore {
    if (!SharedNodeStore.instance) {
      SharedNodeStore.instance = new SharedNodeStore();
    }
    return SharedNodeStore.instance;
  }

  /**
   * Reset singleton (for testing only)
   */
  static resetInstance(): void {
    SharedNodeStore.instance = null;
  }

  // ========================================================================
  // Core Node Operations
  // ========================================================================

  /**
   * Get a node by ID
   */
  getNode(nodeId: string): Node | undefined {
    return this.nodes.get(nodeId);
  }

  /**
   * Get all nodes (returns reactive Map)
   */
  getAllNodes(): Map<string, Node> {
    return this.nodes;
  }

  /**
   * Get nodes filtered by parent ID
   */
  getNodesForParent(parentId: string | null): Node[] {
    const result: Node[] = [];
    for (const node of this.nodes.values()) {
      if (node.parentId === parentId) {
        result.push(node);
      }
    }
    return result;
  }

  /**
   * Check if a node exists
   */
  hasNode(nodeId: string): boolean {
    return this.nodes.has(nodeId);
  }

  /**
   * Get node count
   */
  getNodeCount(): number {
    return this.nodes.size;
  }

  // ========================================================================
  // Update Operations with Conflict Detection
  // ========================================================================

  /**
   * Update a node with conflict detection and source tracking
   *
   * @param nodeId - ID of node to update
   * @param changes - Partial node changes to apply
   * @param source - Source of the update (viewer, database, MCP)
   * @param options - Update options (conflict detection, persistence, etc.)
   */
  updateNode(
    nodeId: string,
    changes: Partial<Node>,
    source: UpdateSource,
    options: UpdateOptions = {}
  ): void {
    const startTime = performance.now();

    try {
      // Get existing node
      const existingNode = this.nodes.get(nodeId);
      if (!existingNode) {
        console.warn(`[SharedNodeStore] Cannot update non-existent node: ${nodeId}`);
        return;
      }

      // Create update record
      const update: NodeUpdate = {
        nodeId,
        changes,
        source,
        timestamp: Date.now(),
        version: this.getNextVersion(nodeId),
        previousVersion: this.versions.get(nodeId)
      };

      // Conflict detection (unless skipped)
      if (!options.skipConflictDetection) {
        const conflict = this.detectConflict(nodeId, update);
        if (conflict) {
          this.handleConflict(conflict);
          return;
        }
      }

      // Apply update optimistically
      const updatedNode: Node = {
        ...existingNode,
        ...changes,
        modifiedAt: new Date().toISOString()
      };

      this.nodes.set(nodeId, updatedNode);
      this.versions.set(nodeId, update.version!);

      // Track pending update for potential rollback
      if (!this.pendingUpdates.has(nodeId)) {
        this.pendingUpdates.set(nodeId, []);
      }
      this.pendingUpdates.get(nodeId)!.push(update);

      // Notify subscribers
      this.notifySubscribers(nodeId, updatedNode, source);

      // Emit event - cast to bypass type checking for now
      // TODO: Update event-types.ts to support source tracking
      eventBus.emit({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: source.type,
        nodeId,
        updateType: 'content',
        newValue: changes
      } as never);

      // Update metrics
      this.metrics.updateCount++;

      // Phase 2.4: Persist to database (unless skipped)
      if (!options.skipPersistence && source.type !== 'database') {
        // Queue database write to prevent concurrent writes
        queueDatabaseWrite(nodeId, async () => {
          try {
            await tauriNodeService.updateNode(nodeId, updatedNode);
            // Mark update as persisted
            this.markUpdatePersisted(nodeId, update);
          } catch (dbError) {
            console.error(`[SharedNodeStore] Database write failed for node ${nodeId}:`, dbError);
            // Rollback the optimistic update
            this.rollbackUpdate(nodeId, update);
          }
        }).catch((err) => {
          // Catch any queueing errors
          console.error(`[SharedNodeStore] Failed to queue database write:`, err);
        });
      }
    } catch (error) {
      console.error(`[SharedNodeStore] Error updating node ${nodeId}:`, error);
      throw error;
    } finally {
      const duration = performance.now() - startTime;
      this.recordMetric(duration);
    }
  }

  /**
   * Batch update multiple nodes
   */
  updateNodes(
    updates: Array<{ nodeId: string; changes: Partial<Node> }>,
    source: UpdateSource,
    options: UpdateOptions = {}
  ): void {
    for (const { nodeId, changes } of updates) {
      this.updateNode(nodeId, changes, source, options);
    }
  }

  /**
   * Set a node (create or replace)
   */
  setNode(node: Node, source: UpdateSource, skipPersistence = false): void {
    this.nodes.set(node.id, node);
    this.versions.set(node.id, this.getNextVersion(node.id));
    this.notifySubscribers(node.id, node, source);

    // Phase 2.4: Persist to database
    if (!skipPersistence && source.type !== 'database') {
      queueDatabaseWrite(node.id, async () => {
        try {
          // Use updateNode for existing nodes (SharedNodeStore manages existence)
          await tauriNodeService.updateNode(node.id, node);
        } catch (dbError) {
          console.error(`[SharedNodeStore] Database write failed for node ${node.id}:`, dbError);
        }
      }).catch((err) => {
        console.error(`[SharedNodeStore] Failed to queue database write:`, err);
      });
    }
  }

  /**
   * Delete a node
   */
  deleteNode(nodeId: string, source: UpdateSource, skipPersistence = false): void {
    const node = this.nodes.get(nodeId);
    if (node) {
      this.nodes.delete(nodeId);
      this.versions.delete(nodeId);
      this.pendingUpdates.delete(nodeId);
      this.notifySubscribers(nodeId, node, source);

      // Emit event - cast to bypass type checking for now
      // TODO: Update event-types.ts to support source tracking
      eventBus.emit({
        type: 'node:deleted',
        namespace: 'lifecycle',
        source: source.type,
        nodeId,
        parentId: node.parentId || undefined
      } as never);

      // Phase 2.4: Persist deletion to database
      if (!skipPersistence && source.type !== 'database') {
        queueDatabaseWrite(nodeId, async () => {
          try {
            await tauriNodeService.deleteNode(nodeId);
          } catch (dbError) {
            console.error(
              `[SharedNodeStore] Database deletion failed for node ${nodeId}:`,
              dbError
            );
          }
        }).catch((err) => {
          console.error(`[SharedNodeStore] Failed to queue database write:`, err);
        });
      }
    }
  }

  /**
   * Clear all nodes (for testing)
   */
  clearAll(): void {
    this.nodes.clear();
    this.versions.clear();
    this.pendingUpdates.clear();
    this.notifyAllSubscribers();
  }

  // ========================================================================
  // Phase 3: External Update Handling (MCP-Ready)
  // ========================================================================

  /**
   * Handle updates from external sources (MCP server, database sync, etc.)
   *
   * This method provides the integration point for future MCP server (#112).
   * It routes external updates through the same conflict detection and
   * synchronization pipeline as local edits.
   *
   * @param source - Source type: 'mcp-server', 'database', or 'external'
   * @param update - The node update to apply
   *
   * @example
   * // Future: When MCP server from #112 is ready
   * mcpServer.on('node:updated', (mcpUpdate) => {
   *   sharedStore.handleExternalUpdate('mcp-server', mcpUpdate);
   * });
   *
   * @example
   * // Current: Simulated MCP update for testing
   * const mcpUpdate = {
   *   nodeId: 'test-node',
   *   changes: { content: 'Updated by AI agent' },
   *   source: { type: 'mcp-server' as const, serverId: 'test-server' },
   *   timestamp: Date.now()
   * };
   * sharedStore.handleExternalUpdate('mcp-server', mcpUpdate);
   */
  handleExternalUpdate(
    sourceType: 'mcp-server' | 'database' | 'external',
    update: NodeUpdate
  ): void {
    // Validate the node exists
    if (!this.nodes.has(update.nodeId)) {
      console.warn(
        `[SharedNodeStore] External update for non-existent node: ${update.nodeId} from ${sourceType}`
      );
      return;
    }

    // Apply the update through standard pipeline
    // This ensures:
    // - Conflict detection happens
    // - All viewers are notified
    // - Metrics are tracked
    // - Events are emitted
    this.updateNode(update.nodeId, update.changes, update.source, {
      // External updates from database should skip persistence to avoid loops
      skipPersistence: sourceType === 'database',
      // MCP server updates should go through conflict detection
      skipConflictDetection: false
    });
  }

  // ========================================================================
  // Conflict Detection and Resolution
  // ========================================================================

  /**
   * Detect if an update conflicts with pending updates
   */
  private detectConflict(nodeId: string, incomingUpdate: NodeUpdate): Conflict | null {
    const pending = this.pendingUpdates.get(nodeId);
    if (!pending || pending.length === 0) {
      return null;
    }

    // Check for version mismatch
    if (
      incomingUpdate.previousVersion !== undefined &&
      this.versions.get(nodeId) !== incomingUpdate.previousVersion
    ) {
      // Version mismatch - concurrent edit detected
      const lastPending = pending[pending.length - 1];
      return {
        nodeId,
        localUpdate: lastPending,
        remoteUpdate: incomingUpdate,
        conflictType: 'version-mismatch',
        detectedAt: Date.now()
      };
    }

    // Check for concurrent edits (same field modified within time window)
    for (const pendingUpdate of pending) {
      const timeDiff = Math.abs(incomingUpdate.timestamp - pendingUpdate.timestamp);
      if (timeDiff < this.conflictWindowMs) {
        // Check if same fields were modified
        const incomingFields = new Set(Object.keys(incomingUpdate.changes));
        const pendingFields = new Set(Object.keys(pendingUpdate.changes));
        const overlap = [...incomingFields].some((field) => pendingFields.has(field));

        if (overlap) {
          return {
            nodeId,
            localUpdate: pendingUpdate,
            remoteUpdate: incomingUpdate,
            conflictType: 'concurrent-edit',
            detectedAt: Date.now()
          };
        }
      }
    }

    return null;
  }

  /**
   * Handle detected conflict
   */
  private handleConflict(conflict: Conflict): void {
    this.metrics.conflictCount++;

    // Get existing node for type-safe resolution
    const existingNode = this.nodes.get(conflict.nodeId);
    if (!existingNode) {
      console.warn(
        `[SharedNodeStore] Cannot resolve conflict for non-existent node: ${conflict.nodeId}`
      );
      return;
    }

    // Resolve using configured strategy
    const resolution = this.conflictResolver.resolve(conflict, existingNode);

    // Apply resolved state
    this.nodes.set(conflict.nodeId, resolution.resolvedNode);
    this.versions.set(conflict.nodeId, this.getNextVersion(conflict.nodeId));

    // Notify about conflict resolution
    eventBus.emit<ConflictResolvedEvent>({
      type: 'node:conflict-resolved',
      namespace: 'lifecycle',
      source: 'shared-node-store',
      nodeId: conflict.nodeId,
      conflictType: conflict.conflictType,
      strategy: resolution.strategy,
      discardedUpdate: resolution.discardedUpdate
    });

    // Notify subscribers
    this.notifySubscribers(conflict.nodeId, resolution.resolvedNode, conflict.remoteUpdate.source);
  }

  /**
   * Set conflict resolver strategy
   */
  setConflictResolver(resolver: ConflictResolver): void {
    this.conflictResolver = resolver;
  }

  /**
   * Get current conflict resolver
   */
  getConflictResolver(): ConflictResolver {
    return this.conflictResolver;
  }

  /**
   * Set conflict detection window (in milliseconds)
   * Updates within this time window are considered potentially concurrent
   */
  setConflictWindow(ms: number): void {
    this.conflictWindowMs = ms;
  }

  // ========================================================================
  // Rollback Support (Optimistic Updates)
  // ========================================================================

  /**
   * Rollback a pending update (e.g., if database write fails)
   */
  rollbackUpdate(nodeId: string, updateToRollback: NodeUpdate): void {
    this.metrics.rollbackCount++;

    const pending = this.pendingUpdates.get(nodeId);
    if (!pending) return;

    // Remove the failed update from pending
    const index = pending.indexOf(updateToRollback);
    if (index > -1) {
      pending.splice(index, 1);
    }

    // Rollback to previous version
    const previousVersion = updateToRollback.previousVersion;
    if (previousVersion !== undefined) {
      this.versions.set(nodeId, previousVersion);
    }

    // Notify subscribers about rollback
    const currentNode = this.nodes.get(nodeId);
    if (currentNode) {
      this.notifySubscribers(nodeId, currentNode, updateToRollback.source);
    }

    // Emit rollback event
    eventBus.emit<UpdateRolledBackEvent>({
      type: 'node:update-rolled-back',
      namespace: 'lifecycle',
      source: 'shared-node-store',
      nodeId,
      reason: 'Database write failed',
      failedUpdate: updateToRollback
    });
  }

  /**
   * Mark an update as persisted (remove from pending)
   */
  markUpdatePersisted(nodeId: string, update: NodeUpdate): void {
    const pending = this.pendingUpdates.get(nodeId);
    if (!pending) return;

    const index = pending.indexOf(update);
    if (index > -1) {
      pending.splice(index, 1);
    }

    // Clean up if no more pending updates
    if (pending.length === 0) {
      this.pendingUpdates.delete(nodeId);
    }
  }

  // ========================================================================
  // Subscription Management (Observer Pattern)
  // ========================================================================

  /**
   * Subscribe to changes for a specific node
   */
  subscribe(nodeId: string, callback: NodeChangeCallback): Unsubscribe {
    const subscription: Subscription = {
      id: `sub_${this.subscriptionIdCounter++}`,
      nodeId,
      callback,
      createdAt: Date.now(),
      callCount: 0
    };

    if (!this.subscriptions.has(nodeId)) {
      this.subscriptions.set(nodeId, new Set());
    }
    this.subscriptions.get(nodeId)!.add(subscription);
    this.metrics.subscriptionCount++;

    // Return unsubscribe function
    return () => {
      const subs = this.subscriptions.get(nodeId);
      if (subs) {
        subs.delete(subscription);
        if (subs.size === 0) {
          this.subscriptions.delete(nodeId);
        }
      }
      this.metrics.subscriptionCount--;
    };
  }

  /**
   * Subscribe to all node changes (wildcard)
   */
  subscribeAll(callback: NodeChangeCallback): Unsubscribe {
    const subscription: Subscription = {
      id: `sub_wildcard_${this.subscriptionIdCounter++}`,
      nodeId: null,
      callback,
      createdAt: Date.now(),
      callCount: 0
    };

    this.wildcardSubscriptions.add(subscription);
    this.metrics.subscriptionCount++;

    return () => {
      this.wildcardSubscriptions.delete(subscription);
      this.metrics.subscriptionCount--;
    };
  }

  /**
   * Notify subscribers of a node change
   */
  private notifySubscribers(nodeId: string, node: Node, source: UpdateSource): void {
    // Notify node-specific subscribers
    const subs = this.subscriptions.get(nodeId);
    if (subs) {
      for (const sub of subs) {
        try {
          sub.callback(node, source);
          sub.callCount++;
        } catch (error) {
          console.error(`[SharedNodeStore] Subscription callback error:`, error);
        }
      }
    }

    // Notify wildcard subscribers
    for (const sub of this.wildcardSubscriptions) {
      try {
        sub.callback(node, source);
        sub.callCount++;
      } catch (error) {
        console.error(`[SharedNodeStore] Wildcard subscription callback error:`, error);
      }
    }
  }

  /**
   * Notify all subscribers (e.g., on clear)
   */
  private notifyAllSubscribers(): void {
    // Notify all node-specific subscribers
    for (const [nodeId, subs] of this.subscriptions) {
      const node = this.nodes.get(nodeId);
      if (node) {
        for (const sub of subs) {
          try {
            sub.callback(node, { type: 'external', source: 'store-cleared' });
          } catch (error) {
            console.error(`[SharedNodeStore] Subscription callback error:`, error);
          }
        }
      }
    }
  }

  // ========================================================================
  // Performance Metrics
  // ========================================================================

  /**
   * Get performance metrics
   */
  getMetrics(): StoreMetrics {
    return { ...this.metrics };
  }

  /**
   * Reset metrics (for testing)
   */
  resetMetrics(): void {
    this.metrics = {
      updateCount: 0,
      avgUpdateTime: 0,
      maxUpdateTime: 0,
      subscriptionCount: this.metrics.subscriptionCount, // Keep subscription count
      conflictCount: 0,
      rollbackCount: 0
    };
  }

  /**
   * Record operation timing
   */
  private recordMetric(duration: number): void {
    const count = this.metrics.updateCount;
    const currentAvg = this.metrics.avgUpdateTime;
    this.metrics.avgUpdateTime = (currentAvg * (count - 1) + duration) / count;
    this.metrics.maxUpdateTime = Math.max(this.metrics.maxUpdateTime, duration);
  }

  // ========================================================================
  // Version Management
  // ========================================================================

  /**
   * Get next version number for a node
   */
  private getNextVersion(nodeId: string): number {
    const current = this.versions.get(nodeId) || 0;
    return current + 1;
  }

  /**
   * Get current version of a node
   */
  getVersion(nodeId: string): number {
    return this.versions.get(nodeId) || 0;
  }
}

// ============================================================================
// Singleton Export
// ============================================================================

/**
 * Singleton instance for application-wide use
 */
export const sharedNodeStore = SharedNodeStore.getInstance();

/**
 * Default export
 */
export default SharedNodeStore;
