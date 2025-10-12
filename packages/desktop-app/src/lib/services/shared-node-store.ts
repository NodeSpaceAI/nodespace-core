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

/**
 * Module-level write queue for coordinated database persistence
 *
 * Prevents concurrent writes to the same node by ensuring writes are
 * executed sequentially per node. The Map key is the node ID, and the
 * value is the Promise representing the pending write operation.
 *
 * This coordination mechanism ensures database consistency and prevents
 * race conditions across all SharedNodeStore instances.
 */
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

  // Track which nodes have been persisted to database
  // Avoids querying database on every update to check existence
  private persistedNodeIds = new Set<string>();

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

  // Test error tracking (populated only in NODE_ENV='test', cleared between tests)
  private testErrors: Error[] = [];

  // Debouncing for content updates (500ms)
  private contentDebounceTimers = new Map<string, ReturnType<typeof setTimeout>>();

  // Queue for structural updates (processed serially)
  private structuralUpdateQueue: Array<() => Promise<void>> = [];
  private isProcessingStructuralQueue = false;

  // Track pending content saves (for FOREIGN KEY coordination)
  private pendingContentSaves = new Map<string, Promise<void>>();

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
        // Skip persisting empty text nodes - they exist in UI but not in database
        const isEmptyTextNode =
          updatedNode.nodeType === 'text' && updatedNode.content.trim() === '';

        if (!isEmptyTextNode) {
          // Queue database write to prevent concurrent writes
          queueDatabaseWrite(nodeId, async () => {
            try {
              // Check if node has been persisted - use in-memory tracking to avoid database query
              const isPersistedToDatabase = this.persistedNodeIds.has(nodeId);
              if (isPersistedToDatabase) {
                await tauriNodeService.updateNode(nodeId, updatedNode);
              } else {
                // Node doesn't exist yet (was a placeholder or new node)
                await tauriNodeService.createNode(updatedNode);
                this.persistedNodeIds.add(nodeId); // Track as persisted
              }
              // Mark update as persisted
              this.markUpdatePersisted(nodeId, update);
            } catch (dbError) {
              const error = dbError instanceof Error ? dbError : new Error(String(dbError));
              console.error(`[SharedNodeStore] Database write failed for node ${nodeId}:`, error);

              // Track error in test environment for test verification
              if (typeof process !== 'undefined' && process.env.NODE_ENV === 'test') {
                this.testErrors.push(error);
              }

              // Rollback the optimistic update
              this.rollbackUpdate(nodeId, update);
            }
          }).catch((err) => {
            // Catch any queueing errors
            const error = err instanceof Error ? err : new Error(String(err));
            console.error(`[SharedNodeStore] Failed to queue database write:`, error);

            // Track error in test environment for test verification
            if (typeof process !== 'undefined' && process.env.NODE_ENV === 'test') {
              this.testErrors.push(error);
            }
          });
        }
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

    // If source is database, mark node as already persisted
    if (source.type === 'database') {
      this.persistedNodeIds.add(node.id);
    }

    // Phase 2.4: Persist to database
    if (!skipPersistence && source.type !== 'database') {
      // Skip persisting empty text nodes - they exist in UI but not in database
      // until user adds content (backend validation requires non-empty content)
      const isEmptyTextNode = node.nodeType === 'text' && node.content.trim() === '';

      if (!isEmptyTextNode) {
        queueDatabaseWrite(node.id, async () => {
          try {
            // Check if node has been persisted - use in-memory tracking to avoid database query
            const isPersistedToDatabase = this.persistedNodeIds.has(node.id);
            if (isPersistedToDatabase) {
              await tauriNodeService.updateNode(node.id, node);
            } else {
              await tauriNodeService.createNode(node);
              this.persistedNodeIds.add(node.id); // Track as persisted
            }
          } catch (dbError) {
            const error = dbError instanceof Error ? dbError : new Error(String(dbError));
            console.error(`[SharedNodeStore] Database write failed for node ${node.id}:`, error);

            // Track error in test environment for test verification
            if (typeof process !== 'undefined' && process.env.NODE_ENV === 'test') {
              this.testErrors.push(error);
            }
          }
        }).catch((err) => {
          const error = err instanceof Error ? err : new Error(String(err));
          console.error(`[SharedNodeStore] Failed to queue database write:`, error);

          // Track error in test environment for test verification
          if (typeof process !== 'undefined' && process.env.NODE_ENV === 'test') {
            this.testErrors.push(error);
          }
        });
      }
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
      this.persistedNodeIds.delete(nodeId); // Remove from tracking set
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
            const error = dbError instanceof Error ? dbError : new Error(String(dbError));
            console.error(`[SharedNodeStore] Database deletion failed for node ${nodeId}:`, error);

            // Track error in test environment for test verification
            if (typeof process !== 'undefined' && process.env.NODE_ENV === 'test') {
              this.testErrors.push(error);
            }
          }
        }).catch((err) => {
          const error = err instanceof Error ? err : new Error(String(err));
          console.error(`[SharedNodeStore] Failed to queue database write:`, error);

          // Track error in test environment for test verification
          if (typeof process !== 'undefined' && process.env.NODE_ENV === 'test') {
            this.testErrors.push(error);
          }
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
    this.persistedNodeIds.clear();
    this.notifyAllSubscribers();
  }

  // ========================================================================
  // New Methods for BaseNodeViewer Migration (Issue #237)
  // ========================================================================

  /**
   * Load child nodes from database for a parent
   * Replaces BaseNodeViewer's direct databaseService.getNodesByOriginId() call
   *
   * @param parentId - The parent/container node ID
   * @returns Array of child nodes loaded from database
   */
  async loadChildrenForParent(parentId: string): Promise<Node[]> {
    try {
      const nodes = await tauriNodeService.getNodesByOriginId(parentId);

      // Add nodes to store with database source (skips persistence)
      const databaseSource = { type: 'database' as const, reason: 'loaded-from-db' };
      for (const node of nodes) {
        this.setNode(node, databaseSource, true); // skipPersistence = true
        this.persistedNodeIds.add(node.id); // Mark as already persisted
      }

      return nodes;
    } catch (error) {
      console.error(`[SharedNodeStore] Failed to load children for parent ${parentId}:`, error);
      throw error;
    }
  }

  /**
   * Update node content with debouncing (500ms for typing)
   * Replaces BaseNodeViewer's debounceSave() logic
   *
   * @param nodeId - The node to update
   * @param content - New content
   * @param nodeType - Node type
   * @param isPlaceholder - Whether this is a placeholder node
   * @param source - Update source
   */
  updateNodeContentDebounced(
    nodeId: string,
    content: string,
    nodeType: string,
    isPlaceholder: boolean,
    source: UpdateSource
  ): void {
    // Clear existing timer
    const existing = this.contentDebounceTimers.get(nodeId);
    if (existing) {
      clearTimeout(existing);
    }

    // Skip persisting placeholders
    if (isPlaceholder) {
      // Update in-memory only
      this.updateNode(nodeId, { content }, source, { skipPersistence: true });
      return;
    }

    // Debounce content updates (500ms)
    const timer = setTimeout(() => {
      this.updateNode(nodeId, { content, nodeType }, source);
      this.contentDebounceTimers.delete(nodeId);
    }, 500);

    this.contentDebounceTimers.set(nodeId, timer);
  }

  /**
   * Update node content immediately (for new nodes)
   * Replaces BaseNodeViewer's saveNode() for immediate saves
   * Tracks pending saves for FOREIGN KEY coordination
   *
   * @param nodeId - The node to update
   * @param content - New content
   * @param nodeType - Node type
   * @param parentId - Parent node ID
   * @param containerNodeId - Container/origin node ID
   * @param beforeSiblingId - Sibling ordering
   * @param isPlaceholder - Whether this is a placeholder node
   * @param source - Update source
   */
  async saveNodeImmediately(
    nodeId: string,
    content: string,
    nodeType: string,
    parentId: string | null,
    containerNodeId: string,
    beforeSiblingId: string | null,
    isPlaceholder: boolean,
    source: UpdateSource
  ): Promise<void> {
    // Skip persisting placeholders
    if (isPlaceholder) {
      return;
    }

    const savePromise = (async () => {
      const node: Node = {
        id: nodeId,
        nodeType,
        content,
        parentId,
        containerNodeId,
        beforeSiblingId,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {}
        // Note: mentions is computed, not persisted
      };

      // Check if node already exists
      const existingNode = this.getNode(nodeId);
      if (existingNode) {
        // Update existing node
        this.updateNode(
          nodeId,
          { content, nodeType, parentId, containerNodeId, beforeSiblingId },
          source
        );
      } else {
        // Create new node
        this.setNode(node, source);
      }
    })();

    // Track pending save for FOREIGN KEY coordination
    this.pendingContentSaves.set(nodeId, savePromise);

    try {
      await savePromise;
    } finally {
      this.pendingContentSaves.delete(nodeId);
    }
  }

  /**
   * Check if a node has a pending save operation
   * Provides single source of truth for pending save status
   *
   * @param nodeId - Node ID to check
   * @returns True if save is pending
   */
  hasPendingSave(nodeId: string): boolean {
    return this.pendingContentSaves.has(nodeId);
  }

  /**
   * Wait for pending node saves to complete with timeout
   * Replaces BaseNodeViewer's waitForNodeSavesIfPending() logic
   *
   * @param nodeIds - Array of node IDs to wait for
   * @param timeoutMs - Timeout in milliseconds (default 5000)
   * @returns Set of node IDs that failed to save
   */
  async waitForNodeSaves(nodeIds: string[], timeoutMs = 5000): Promise<Set<string>> {
    const failedNodeIds = new Set<string>();
    const nodeSavePromises = new Map<string, Promise<void>>();

    // Collect relevant pending saves
    for (const nodeId of nodeIds) {
      const savePromise = this.pendingContentSaves.get(nodeId);
      if (savePromise) {
        nodeSavePromises.set(nodeId, savePromise);
      }
    }

    if (nodeSavePromises.size === 0) return failedNodeIds;

    try {
      await Promise.race([
        Promise.all(nodeSavePromises.values()),
        new Promise((_, reject) =>
          setTimeout(() => reject(new Error('Timeout waiting for saves')), timeoutMs)
        )
      ]);
    } catch (error) {
      console.error('[SharedNodeStore] Timeout waiting for saves:', error);

      // Grace period to allow in-flight saves to complete
      await new Promise((resolve) => setTimeout(resolve, 50));

      // After grace period, check each promise individually to see if it actually failed
      for (const [nodeId, savePromise] of nodeSavePromises) {
        try {
          // Try to await the promise - if it resolved during grace period, it succeeds
          await Promise.race([
            savePromise,
            new Promise((_, reject) => setTimeout(() => reject(new Error('Grace timeout')), 0))
          ]);
          // Promise resolved - node saved successfully
        } catch {
          // Promise rejected or still pending after grace period - mark as failed
          console.error('[SharedNodeStore] Node save failed or timed out:', nodeId);
          failedNodeIds.add(nodeId);
        }
      }
    }

    return failedNodeIds;
  }

  /**
   * Recursively ensure all placeholder ancestors are persisted
   * Replaces BaseNodeViewer's ensureAncestorsPersisted() logic
   *
   * @param nodeId - Node ID to check ancestors for
   * @param checkIsPlaceholder - Function to check if a node is a placeholder (UI state)
   */
  async ensureAncestorChainPersisted(
    nodeId: string,
    checkIsPlaceholder: (nodeId: string) => boolean
  ): Promise<void> {
    const node = this.getNode(nodeId);
    if (!node || !node.parentId) return;

    const parent = this.getNode(node.parentId);
    if (!parent) return;

    const isParentPlaceholder = checkIsPlaceholder(parent.id);

    if (isParentPlaceholder) {
      // Recursively persist grandparents first
      await this.ensureAncestorChainPersisted(parent.id, checkIsPlaceholder);

      // Persist placeholder parent with empty content
      await this.saveNodeImmediately(
        parent.id,
        '', // empty content for placeholder
        parent.nodeType,
        parent.parentId,
        parent.containerNodeId || parent.parentId || parent.id,
        parent.beforeSiblingId,
        false, // no longer a placeholder after persisting
        { type: 'viewer', viewerId: 'ancestor-chain' }
      );
    }
  }

  /**
   * Validate FOREIGN KEY references before structural update
   * Replaces BaseNodeViewer's inline validation logic
   *
   * @param nodeId - Node to validate
   * @param parentId - Proposed parent ID
   * @param beforeSiblingId - Proposed sibling ID
   * @param viewerParentId - Special case: viewer's parent exists but isn't loaded in store
   * @returns Validated references and any errors
   */
  async validateNodeReferences(
    nodeId: string,
    parentId: string | null,
    beforeSiblingId: string | null,
    viewerParentId: string | null
  ): Promise<{
    validatedParentId: string | null;
    validatedBeforeSiblingId: string | null;
    errors: string[];
  }> {
    const errors: string[] = [];
    let validatedParentId = parentId;
    let validatedBeforeSiblingId = beforeSiblingId;

    // Validate node still exists
    if (!this.hasNode(nodeId)) {
      errors.push(`Node ${nodeId} not found (may have been deleted)`);
      return { validatedParentId, validatedBeforeSiblingId, errors };
    }

    // Validate parentId exists
    if (validatedParentId) {
      // Special case: viewer's parent exists in database but not loaded in store
      const isViewerParent = validatedParentId === viewerParentId;
      if (!isViewerParent && !this.hasNode(validatedParentId)) {
        errors.push(`Parent ${validatedParentId} not found (may have been deleted)`);
      }
    }

    // Validate beforeSiblingId exists
    if (validatedBeforeSiblingId) {
      if (!this.hasNode(validatedBeforeSiblingId)) {
        console.warn(
          `[SharedNodeStore] beforeSiblingId ${validatedBeforeSiblingId} not found, null-ing reference`
        );
        validatedBeforeSiblingId = null;
      }
    }

    return { validatedParentId, validatedBeforeSiblingId, errors };
  }

  /**
   * Update structural changes with validation and serial processing
   * Replaces BaseNodeViewer's complex structural watcher logic
   *
   * @param updates - Array of structural updates
   * @param source - Update source
   * @param viewerParentId - Viewer's parent ID (for reference validation)
   * @returns Results with succeeded/failed updates
   */
  async updateStructuralChangesValidated(
    updates: Array<{
      nodeId: string;
      parentId: string | null;
      beforeSiblingId: string | null;
    }>,
    source: UpdateSource,
    viewerParentId: string | null
  ): Promise<{
    succeeded: typeof updates;
    failed: typeof updates;
    errors: Map<string, Error>;
  }> {
    const succeeded: typeof updates = [];
    const failed: typeof updates = [];
    const errors = new Map<string, Error>();

    // Process updates serially to prevent race conditions
    for (const update of updates) {
      try {
        // Validate FOREIGN KEY references
        const validation = await this.validateNodeReferences(
          update.nodeId,
          update.parentId,
          update.beforeSiblingId,
          viewerParentId
        );

        if (validation.errors.length > 0) {
          failed.push(update);
          errors.set(update.nodeId, new Error(validation.errors.join('; ')));
          continue;
        }

        // Apply validated update
        this.updateNode(
          update.nodeId,
          {
            parentId: validation.validatedParentId,
            beforeSiblingId: validation.validatedBeforeSiblingId
          },
          source
        );

        succeeded.push({
          ...update,
          parentId: validation.validatedParentId,
          beforeSiblingId: validation.validatedBeforeSiblingId
        });
      } catch (error) {
        const err = error instanceof Error ? error : new Error(String(error));
        console.error('[SharedNodeStore] Structural update failed:', update.nodeId, err);
        failed.push(update);
        errors.set(update.nodeId, err);
      }
    }

    return { succeeded, failed, errors };
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

  // ========================================================================
  // Test Utilities
  // ========================================================================

  /**
   * Check if there are pending database writes
   * Used by tests to wait for all writes to complete
   */
  hasPendingWrites(): boolean {
    return pendingDatabaseWrites.size > 0;
  }

  /**
   * Get test errors (only populated in test environment)
   * Used by tests to verify database operations succeeded
   */
  getTestErrors(): Error[] {
    return [...this.testErrors];
  }

  /**
   * Clear test errors
   * Should be called at the start of each test for isolation
   */
  clearTestErrors(): void {
    this.testErrors = [];
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
