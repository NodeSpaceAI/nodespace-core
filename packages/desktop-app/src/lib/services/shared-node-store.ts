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
import { PersistenceCoordinator, OperationCancelledError } from './persistence-coordinator.svelte';
import { isPlaceholderNode, requiresAtomicBatching } from '$lib/utils/placeholder-detection';
import { shouldLogDatabaseErrors, isTestEnvironment } from '$lib/utils/test-environment';
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
// NOTE: Database write coordination is now handled by PersistenceCoordinator
// All persistence operations delegate to PersistenceCoordinator.getInstance().persist()

// ============================================================================
// Constants
// ============================================================================

/**
 * Default timeout for batch updates (milliseconds)
 * Batches auto-commit after this duration of inactivity
 * Timer resets on each change, so batch only commits after true inactivity
 */
const DEFAULT_BATCH_TIMEOUT_MS = 2000; // 2 seconds

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
 * Batch structure for atomic multi-property updates
 * Used for pattern conversions where content + nodeType must persist together
 */
interface ActiveBatch {
  nodeId: string;
  changes: Partial<Node>;
  batchId: string;
  createdAt: number;
  timeout: ReturnType<typeof setTimeout>;
  timeoutMs: number;
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

  // Batch ID counter for unique batch identification
  private batchIdCounter = 0;

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

  // Batch update tracking for atomic multi-property updates
  // Used for pattern conversions where content + nodeType must persist together
  private activeBatches = new Map<string, ActiveBatch>();

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
  // Persistence Control (Phase 1 of UpdateSource Refactor)
  // ========================================================================

  /**
   * Determine persistence behavior from explicit options or legacy source type.
   *
   * This helper implements the new explicit persistence API while maintaining
   * backward compatibility with the legacy `source.type === 'database'` checks.
   *
   * Priority (highest to lowest):
   * 1. options.markAsPersistedOnly - Mark as persisted without re-persisting
   * 2. options.skipPersistence - Skip persistence
   * 3. options.persist - Explicit persistence control
   * 4. Legacy: source.type === 'database' - Auto-skip persistence
   * 5. Default: Auto-determine based on source type and changes
   *
   * @returns Object with shouldPersist and shouldMarkAsPersisted flags
   */
  private determinePersistenceBehavior(
    source: UpdateSource,
    options: UpdateOptions,
    _changes?: Partial<Node>
  ): { shouldPersist: boolean; shouldMarkAsPersisted: boolean } {
    // Priority 1: Explicit mark-as-persisted-only (no actual persistence)
    if (options.markAsPersistedOnly) {
      return { shouldPersist: false, shouldMarkAsPersisted: true };
    }

    // Priority 2: Skip persistence flag (legacy compatibility)
    if (options.skipPersistence) {
      return { shouldPersist: false, shouldMarkAsPersisted: false };
    }

    // Priority 3: Explicit persist option (new API)
    if (options.persist !== undefined) {
      if (options.persist === false) {
        // Explicitly skip persistence
        return { shouldPersist: false, shouldMarkAsPersisted: false };
      }
      // persist === true, 'debounced', or 'immediate' - all trigger persistence
      // (Mode selection handled by PersistenceCoordinator)
      return { shouldPersist: true, shouldMarkAsPersisted: false };
    }

    // Priority 4: Legacy source.type === 'database' behavior
    // Database sources mean "loaded from backend, already persisted"
    if (source.type === 'database') {
      return { shouldPersist: false, shouldMarkAsPersisted: true };
    }

    // Priority 5: Default auto-determination
    // External sources and MCP sources trigger persistence
    // Viewer sources depend on context (handled by caller)
    return { shouldPersist: true, shouldMarkAsPersisted: false };
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
   * Note: Mention relationships are automatically synced by the backend when content changes.
   * The Rust backend extracts nodespace:// mentions and maintains the node_mentions table.
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

    // Handle isComputedField flag - automatically set skipPersistence and skipConflictDetection
    if (options.isComputedField) {
      options = {
        ...options,
        skipPersistence: true,
        skipConflictDetection: true
      };
    }

    // ========================================================================
    // Batch Handling - Route updates through batch system if active
    // ========================================================================

    // Check if batch is active for this node
    if (this.activeBatches.has(nodeId)) {
      // Route through batch system
      this.addToBatch(nodeId, changes);

      // Auto-commit if requested
      if (options.batch?.commitImmediately) {
        this.commitBatch(nodeId);
      }
      return;
    }

    // Check if this update should create a new batch
    if (options.batch?.autoBatch) {
      this.startBatch(nodeId, options.batch.batchTimeout);
      this.addToBatch(nodeId, changes);

      // Auto-commit if requested
      if (options.batch.commitImmediately) {
        this.commitBatch(nodeId);
      }
      return;
    }

    // CRITICAL: Auto-restart batch for pattern-converted node types
    // After a batch commits, subsequent edits should ALSO be batched to maintain consistency
    // This prevents falling back to old debounced path which can cause partial content loss
    // IMPORTANT: Respect UpdateOptions - don't batch if caller explicitly skipped persistence
    const existingNode = this.nodes.get(nodeId);
    const nodeRequiresBatching = existingNode && requiresAtomicBatching(existingNode.nodeType);

    if (nodeRequiresBatching && changes.content !== undefined && !options.skipPersistence) {
      this.startBatch(nodeId, DEFAULT_BATCH_TIMEOUT_MS);
      this.addToBatch(nodeId, changes);
      return;
    }

    // ========================================================================
    // Normal Update Flow (No Batching)
    // ========================================================================

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
        updateType: this.determineUpdateType(changes),
        newValue: changes
      } as never);

      // Update metrics
      this.metrics.updateCount++;

      // Phase 2.4: Persist to database (unless skipped)
      // IMPORTANT: For viewer-sourced updates:
      // - Structural changes (parentId, beforeSiblingId, containerNodeId) persist immediately
      // - Content changes persist in debounced mode
      // This ensures hierarchy operations work while debouncing rapid typing
      const persistBehavior = this.determinePersistenceBehavior(source, options, changes);
      if (persistBehavior.shouldPersist) {
        // GUARD: Check if beforeSiblingId or parentId reference unpersisted placeholders
        // Placeholders use skipPersistence, so there's no operation to wait for
        // Remove these references to avoid FOREIGN KEY constraint violations
        //
        // Type casting explanation: TypeScript doesn't allow deleting properties from Partial<Node>
        // because it's readonly. We create MutablePartialNode to allow deletion, which is safe
        // because we're modifying a local changes object, not the original node in the store.
        type MutablePartialNode = { [K in keyof Partial<Node>]: Partial<Node>[K] };
        const coordinator = PersistenceCoordinator.getInstance();

        // Check beforeSiblingId
        if ('beforeSiblingId' in changes && changes.beforeSiblingId) {
          const isPersisted = this.persistedNodeIds.has(changes.beforeSiblingId);
          const hasPending = coordinator.isPending(changes.beforeSiblingId);
          if (!isPersisted && !hasPending) {
            // Check if it's a placeholder node
            const siblingNode = this.nodes.get(changes.beforeSiblingId);
            if (siblingNode && isPlaceholderNode(siblingNode)) {
              // beforeSiblingId points to unpersisted placeholder - defer until placeholder persists
              delete (changes as MutablePartialNode).beforeSiblingId;
            }
          }
        }

        // Check parentId
        if ('parentId' in changes && changes.parentId) {
          const isPersisted = this.persistedNodeIds.has(changes.parentId);
          const hasPending = coordinator.isPending(changes.parentId);
          if (!isPersisted && !hasPending) {
            // Check if it's a placeholder node
            const parentNode = this.nodes.get(changes.parentId);
            if (parentNode && isPlaceholderNode(parentNode)) {
              // parentId points to unpersisted placeholder - defer until placeholder persists
              delete (changes as MutablePartialNode).parentId;
            }
          }
        }

        const isStructuralChange =
          'parentId' in changes || 'beforeSiblingId' in changes || 'containerNodeId' in changes;
        const isContentChange = 'content' in changes;
        const isNodeTypeChange = 'nodeType' in changes;
        const isPropertyChange = 'properties' in changes;
        const shouldPersist =
          source.type !== 'viewer' ||
          isStructuralChange ||
          isContentChange ||
          isNodeTypeChange ||
          isPropertyChange;

        // Skip persisting placeholder nodes - they exist in UI but not in database
        // Placeholders are nodes with only type-specific prefixes and no actual content
        // They will be persisted once user adds real content
        const isPlaceholder = isPlaceholderNode(updatedNode);

        // CRITICAL: Skip persistence if batch is active for this node
        // The batch will handle persistence atomically when committed
        const hasBatchActive = this.activeBatches.has(nodeId);

        if (shouldPersist && !isPlaceholder && !hasBatchActive) {
          // Delegate to PersistenceCoordinator for coordinated persistence
          // Use debounced mode for content changes (typing), immediate for structural changes
          const dependencies: Array<string | (() => Promise<void>)> = [];

          // For structural changes, ensure ENTIRE ancestor chain is persisted (FOREIGN KEY)
          if (isStructuralChange && updatedNode.parentId) {
            dependencies.push(async () => {
              await this.ensureAncestorChainPersisted(updatedNode.parentId!);
            });
          }

          /**
           * Ensure containerNodeId is persisted (FOREIGN KEY constraint)
           *
           * Backend validates that containerNodeId must reference an existing node.
           * Add dependency when:
           * - containerNodeId is set
           * - Different from parentId (avoids duplicate dependency)
           * - NOT already persisted to database
           */
          if (
            updatedNode.containerNodeId &&
            updatedNode.containerNodeId !== updatedNode.parentId &&
            !this.persistedNodeIds.has(updatedNode.containerNodeId)
          ) {
            dependencies.push(updatedNode.containerNodeId);
          }

          /**
           * Ensure beforeSiblingId is persisted (FOREIGN KEY constraint)
           *
           * Backend validates that beforeSiblingId must reference an existing node.
           * Add dependency when:
           * - beforeSiblingId is set
           * - NOT already persisted to database
           *
           * NOTE: Placeholder check is done earlier (line 263-274) to avoid FOREIGN KEY violations
           */
          if (
            updatedNode.beforeSiblingId &&
            !this.persistedNodeIds.has(updatedNode.beforeSiblingId)
          ) {
            dependencies.push(updatedNode.beforeSiblingId);
          }

          // Add any additional dependencies from options
          if (options.persistenceDependencies) {
            dependencies.push(...options.persistenceDependencies);
          }

          // Capture handle to catch cancellation errors
          const handle = PersistenceCoordinator.getInstance().persist(
            nodeId,
            async () => {
              try {
                // GUARD: Re-check placeholder status at persistence execution time
                // Prevents persisting empty nodes when structural changes trigger persistence
                //
                // EDGE CASE: Eventual consistency in rapid typing scenarios
                // If user adds content between queueing and execution (due to debouncing),
                // the node might have valid content by execution time but still be skipped.
                // This is acceptable because the content update will trigger a new persistence
                // operation that will succeed. The system is eventually consistent.
                const currentNode = this.nodes.get(nodeId);
                if (currentNode && isPlaceholderNode(currentNode)) {
                  return; // Skip persistence - will retry when content is added
                }

                // Check if node has been persisted - use in-memory tracking to avoid database query
                const isPersistedToDatabase = this.persistedNodeIds.has(nodeId);

                if (isPersistedToDatabase) {
                  // IMPORTANT: For UPDATE, only send the changes (Partial<Node>), not the full node
                  // The backend expects NodeUpdate which is a partial update of specific fields
                  // Convert nullable fields properly (undefined = don't update, null = set to null)
                  const updatePayload = { ...changes };

                  // Get current node version for optimistic concurrency control
                  const currentNode = this.nodes.get(nodeId);
                  const currentVersion = currentNode?.version ?? 1;

                  try {
                    await tauriNodeService.updateNode(nodeId, currentVersion, updatePayload);
                  } catch (updateError) {
                    // If UPDATE fails because node doesn't exist, try CREATE instead
                    // This handles cases where persistedNodeIds is out of sync (page reload, database reset)
                    // Match both "NodeNotFound" and "does not exist" error messages
                    if (
                      updateError instanceof Error &&
                      (updateError.message.includes('NodeNotFound') ||
                        updateError.message.includes('does not exist'))
                    ) {
                      console.warn(
                        `[SharedNodeStore] Node ${nodeId} not found in database, creating instead of updating`
                      );
                      await tauriNodeService.createNode(updatedNode);
                      this.persistedNodeIds.add(nodeId); // Now it's persisted
                    } else {
                      // Re-throw other errors
                      throw updateError;
                    }
                  }
                } else {
                  // Node doesn't exist yet (was a placeholder or new node)
                  await tauriNodeService.createNode(updatedNode);
                  this.persistedNodeIds.add(nodeId); // Track as persisted

                  // CRITICAL: After persisting a placeholder that now has content,
                  // update any nodes that reference this node in their beforeSiblingId
                  // (these updates were skipped earlier to avoid FOREIGN KEY violations)
                  this.updateDeferredSiblingReferences(nodeId);
                }
                // Mark update as persisted
                this.markUpdatePersisted(nodeId, update);
              } catch (dbError) {
                const error = dbError instanceof Error ? dbError : new Error(String(dbError));

                // Suppress expected errors in in-memory test mode
                if (shouldLogDatabaseErrors()) {
                  console.error(
                    `[SharedNodeStore] Database write failed for node ${nodeId}:`,
                    error
                  );
                }

                // Always track errors in test environment for verification
                this.trackErrorIfTesting(error);

                // Rollback the optimistic update
                this.rollbackUpdate(nodeId, update);
                throw error; // Re-throw to mark operation as failed in coordinator
              }
            },
            {
              mode: isStructuralChange ? 'immediate' : 'debounce',
              dependencies: dependencies.length > 0 ? dependencies : undefined
            }
          );

          // Handle cancellation errors (expected when operations are superseded)
          handle.promise.catch((err) => {
            if (err instanceof OperationCancelledError) {
              // Operation was cancelled by a newer operation - this is expected
              return;
            }
            // Real errors are logged by PersistenceCoordinator
            // Re-throw would create unhandled rejection, so we silently handle
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
    const isNewNode = !this.persistedNodeIds.has(node.id);
    this.nodes.set(node.id, node);
    this.versions.set(node.id, this.getNextVersion(node.id));
    this.notifySubscribers(node.id, node, source);

    // Determine persistence behavior using new explicit API
    const options: UpdateOptions = { skipPersistence };
    const { shouldMarkAsPersisted } = this.determinePersistenceBehavior(source, options);

    // Mark as persisted if explicitly requested or loaded from backend
    if (shouldMarkAsPersisted) {
      this.persistedNodeIds.add(node.id);
    }

    // Check if node is a placeholder (node with only type-specific prefix, no actual content)
    const isPlaceholder = isPlaceholderNode(node);
    const isPlaceholderFromViewer = isPlaceholder && source.type === 'viewer' && isNewNode;

    // Phase 2.4: Persist to database
    // IMPORTANT: For NEW nodes from viewer, persist immediately (including empty ones!)
    // For UPDATES from viewer, skip persistence - BaseNodeViewer handles with debouncing
    // This ensures createNode() persistence works while avoiding duplicate writes on updates
    //
    // EXCEPTION: Placeholders should persist nodeType but NOT content (to avoid backend validation errors)
    const persistBehavior = this.determinePersistenceBehavior(source, options);
    if (!isPlaceholderFromViewer && persistBehavior.shouldPersist) {
      const shouldPersist = source.type !== 'viewer' || isNewNode;

      if (shouldPersist) {
        // Delegate to PersistenceCoordinator
        const dependencies: Array<string | (() => Promise<void>)> = [];

        // Ensure ENTIRE ancestor chain is persisted (FOREIGN KEY)
        if (node.parentId) {
          dependencies.push(async () => {
            await this.ensureAncestorChainPersisted(node.parentId!);
          });
        }

        /**
         * Ensure containerNodeId is persisted (FOREIGN KEY constraint)
         *
         * Backend validates that containerNodeId must reference an existing node.
         * Add dependency when:
         * - containerNodeId is set
         * - Different from parentId (avoids duplicate dependency)
         * - NOT already persisted to database
         */
        if (
          node.containerNodeId &&
          node.containerNodeId !== node.parentId &&
          !this.persistedNodeIds.has(node.containerNodeId)
        ) {
          dependencies.push(node.containerNodeId);
        }

        /**
         * Ensure beforeSiblingId is persisted (FOREIGN KEY constraint)
         *
         * Backend validates that beforeSiblingId must reference an existing node.
         * Add dependency when:
         * - beforeSiblingId is set
         * - NOT already persisted to database
         */
        if (node.beforeSiblingId && !this.persistedNodeIds.has(node.beforeSiblingId)) {
          dependencies.push(node.beforeSiblingId);
        }

        // For placeholders with specialized nodeType, filter out content to avoid backend validation
        let nodeToPersist = node;
        if (isPlaceholder && node.nodeType !== 'text') {
          // Extract and discard content field (using underscore prefix to indicate intentionally unused)
          // Content field is omitted to avoid backend validation errors for placeholder nodes
          const { content: _content, ...nodeWithoutContent } = node;
          // Type assertion required: backend expects Node but we're omitting content for placeholders
          // This is safe because backend validation allows missing content for non-text placeholder nodes
          nodeToPersist = nodeWithoutContent as Node;
        }

        // Capture handle to catch cancellation errors
        const handle = PersistenceCoordinator.getInstance().persist(
          node.id,
          async () => {
            try {
              // Check if node has been persisted - use in-memory tracking to avoid database query
              const isPersistedToDatabase = this.persistedNodeIds.has(node.id);
              if (isPersistedToDatabase) {
                try {
                  // Get current version for optimistic concurrency control
                  const currentVersion = node.version ?? 1;
                  await tauriNodeService.updateNode(node.id, currentVersion, nodeToPersist);
                } catch (updateError) {
                  // If UPDATE fails because node doesn't exist, try CREATE instead
                  const errorMessage =
                    updateError instanceof Error
                      ? updateError.message.toLowerCase()
                      : String(updateError).toLowerCase();
                  const isNodeNotFound =
                    errorMessage.includes('nodenotfound') ||
                    errorMessage.includes('not found') ||
                    errorMessage.includes('does not exist');

                  if (isNodeNotFound) {
                    console.warn(
                      `[SharedNodeStore] Node ${node.id} not found in database, creating instead of updating (error: ${errorMessage})`
                    );
                    await tauriNodeService.createNode(nodeToPersist);
                    this.persistedNodeIds.add(node.id);
                  } else {
                    throw updateError;
                  }
                }
              } else {
                await tauriNodeService.createNode(nodeToPersist);
                this.persistedNodeIds.add(node.id); // Track as persisted
              }
            } catch (dbError) {
              const error = dbError instanceof Error ? dbError : new Error(String(dbError));

              // Suppress expected errors in in-memory test mode
              if (shouldLogDatabaseErrors()) {
                console.error(
                  `[SharedNodeStore] Database write failed for node ${node.id}:`,
                  error
                );
              }

              // Always track errors in test environment for verification
              this.trackErrorIfTesting(error);

              throw error; // Re-throw to mark operation as failed in coordinator
            }
          },
          {
            mode: 'immediate',
            dependencies: dependencies.length > 0 ? dependencies : undefined
          }
        );

        // Handle cancellation errors (expected when operations are superseded)
        handle.promise.catch((err) => {
          if (err instanceof OperationCancelledError) {
            // Operation was cancelled by a newer operation - this is expected
            return;
          }
          // Real errors are logged by PersistenceCoordinator
          // Re-throw would create unhandled rejection, so we silently handle
        });
      }
    }
  }

  /**
   * Delete a node
   *
   * @param nodeId - ID of node to delete
   * @param source - Source of the deletion
   * @param skipPersistence - Skip database persistence (default: false)
   * @param dependencies - Node IDs that must be persisted before deletion (prevents FOREIGN KEY violations)
   */
  deleteNode(
    nodeId: string,
    source: UpdateSource,
    skipPersistence = false,
    dependencies: string[] = []
  ): void {
    // Cancel any active batch before deletion
    this.cancelBatch(nodeId);

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
      const persistBehavior = this.determinePersistenceBehavior(source, { skipPersistence });
      if (persistBehavior.shouldPersist) {
        // Filter dependencies to only include nodes with pending persistence operations
        const pendingDeps = dependencies.filter((depId) =>
          PersistenceCoordinator.getInstance().isPending(depId)
        );

        // Capture handle to catch cancellation errors
        const handle = PersistenceCoordinator.getInstance().persist(
          nodeId,
          async () => {
            try {
              // Get current version for optimistic concurrency control
              // Note: node has already been removed from this.nodes, so we use the captured node variable
              const currentVersion = node.version ?? 1;
              await tauriNodeService.deleteNode(nodeId, currentVersion);
            } catch (dbError) {
              const error = dbError instanceof Error ? dbError : new Error(String(dbError));

              // Suppress expected errors in in-memory test mode
              if (shouldLogDatabaseErrors()) {
                console.error(
                  `[SharedNodeStore] Database deletion failed for node ${nodeId}:`,
                  error
                );
              }

              // Always track errors in test environment for verification
              this.trackErrorIfTesting(error);

              throw error; // Re-throw to mark operation as failed in coordinator
            }
          },
          {
            mode: 'immediate',
            dependencies: pendingDeps.length > 0 ? pendingDeps : undefined
          }
        );

        // Handle cancellation errors (expected when operations are superseded)
        handle.promise.catch((err) => {
          if (err instanceof OperationCancelledError) {
            // Operation was cancelled by a newer operation - this is expected
            return;
          }
          // Real errors are logged by PersistenceCoordinator
          // Re-throw would create unhandled rejection, so we silently handle
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
   * Replaces BaseNodeViewer's direct databaseService.getNodesByContainerId() call
   *
   * @param parentId - The parent/container node ID
   * @returns Array of child nodes loaded from database
   */
  async loadChildrenForParent(parentId: string): Promise<Node[]> {
    try {
      const nodes = await tauriNodeService.getNodesByContainerId(parentId);

      // Add nodes to store with database source (skips persistence)
      // skipPersistence=true will automatically mark nodes as persisted in setNode()
      const databaseSource = { type: 'database' as const, reason: 'loaded-from-db' };
      for (const node of nodes) {
        this.setNode(node, databaseSource, true); // skipPersistence = true
      }

      return nodes;
    } catch (error) {
      // Suppress expected errors in in-memory test mode
      if (shouldLogDatabaseErrors()) {
        console.error(`[SharedNodeStore] Failed to load children for parent ${parentId}:`, error);
      }

      throw error;
    }
  }

  /**
   * Check if a node has been persisted to the database
   * @param nodeId - Node ID to check
   * @returns True if node exists in database, false if only in memory
   */
  isNodePersisted(nodeId: string): boolean {
    return this.persistedNodeIds.has(nodeId);
  }

  /**
   * Check if a node has a pending save operation
   * Delegates to PersistenceCoordinator
   *
   * @param nodeId - Node ID to check
   * @returns True if save is pending
   */
  hasPendingSave(nodeId: string): boolean {
    return PersistenceCoordinator.getInstance().isPending(nodeId);
  }

  /**
   * Wait for pending node saves to complete with timeout
   * Delegates to PersistenceCoordinator
   *
   * @param nodeIds - Array of node IDs to wait for
   * @param timeoutMs - Timeout in milliseconds (default 5000)
   * @returns Set of node IDs that failed to save
   */
  async waitForNodeSaves(nodeIds: string[], timeoutMs = 5000): Promise<Set<string>> {
    return PersistenceCoordinator.getInstance().waitForPersistence(nodeIds, timeoutMs);
  }

  /**
   * Ensure entire ancestor chain is persisted before persisting a child node
   * Recursively walks up the parent chain and waits for each ancestor to be persisted
   *
   * This prevents FOREIGN KEY constraint violations when creating deeply nested
   * placeholder nodes (e.g., Grandparent → Parent → Child where all are placeholders)
   *
   * @param nodeId - Starting node ID to walk ancestors from
   */
  private async ensureAncestorChainPersisted(nodeId: string): Promise<void> {
    const node = this.nodes.get(nodeId);
    if (!node) {
      // Node doesn't exist in store, nothing to persist
      return;
    }

    // Skip date nodes - backend handles their persistence (virtual or real)
    // Frontend should never try to persist date nodes - that's backend's responsibility
    if (node.nodeType === 'date') {
      return;
    }

    // If node has a parent, recursively ensure parent chain is persisted first
    if (node.parentId) {
      await this.ensureAncestorChainPersisted(node.parentId);
    }

    // Check if node needs to be persisted
    if (!this.persistedNodeIds.has(nodeId)) {
      // GUARD: Skip placeholder nodes - they should not be force-persisted
      // Placeholder nodes will be persisted when they receive actual content
      // This prevents invalid nodes from being persisted during ancestor chain resolution
      if (isPlaceholderNode(node)) {
        return; // Don't persist placeholders, even in ancestor chain
      }

      // Node was never persisted (e.g., placeholder node created with skipPersistence)
      // Force persist it now before any child operations that reference it

      // Trigger persistence via PersistenceCoordinator
      const handle = PersistenceCoordinator.getInstance().persist(
        nodeId,
        async () => {
          await tauriNodeService.createNode(node);
        },
        { mode: 'immediate' } // Use immediate mode for structural operations
      );

      try {
        await handle.promise;
        // Mark as persisted on success
        this.persistedNodeIds.add(nodeId);
      } catch (error) {
        console.error(`[SharedNodeStore] Failed to persist node ${nodeId}:`, error);
        throw error;
      }
    } else if (this.hasPendingSave(nodeId)) {
      // Node has a pending save operation, wait for it
      await this.waitForNodeSaves([nodeId]);
    }
  }

  /**
   * Update deferred sibling references after a placeholder is persisted
   *
   * ARCHITECTURAL DECISION:
   * Placeholders use skipPersistence to avoid backend validation errors (empty content).
   * When they're later persisted (user adds content), we must manually update any
   * sibling nodes that reference them in beforeSiblingId. These updates were deferred
   * earlier to avoid FOREIGN KEY constraint violations.
   *
   * This is simpler than conditional persistence and avoids circular dependencies.
   * Alternative approaches (conditional persistence, polling) add complexity without
   * significant benefit. This manual reconciliation is a proven pattern.
   *
   * PERFORMANCE CHARACTERISTICS:
   * - Complexity: O(n) where n = total nodes in memory
   * - Typical case: 1-5 nodes need reconciliation (user rarely creates many placeholders)
   * - Large documents: With 1000+ nodes, this could add ~1-5ms per placeholder persistence
   * - Mitigation: Placeholders persist on content addition (user typing), not bulk operations
   * - Future optimization: Could maintain explicit deferred reference map if needed
   *
   * @param newlyPersistedNodeId - ID of the node that was just persisted
   */
  private updateDeferredSiblingReferences(newlyPersistedNodeId: string): void {
    // Find all nodes in memory that have this node as their beforeSiblingId OR parentId
    // but haven't persisted that reference yet (due to FOREIGN KEY constraints)
    const nodesToUpdate: Array<{ nodeId: string; changes: Partial<Node> }> = [];

    for (const [id, node] of this.nodes.entries()) {
      if (!this.persistedNodeIds.has(id)) {
        continue; // Skip unpersisted nodes
      }

      const changes: Partial<Node> = {};

      // Check if this node has the newly-persisted node as beforeSiblingId
      if (node.beforeSiblingId === newlyPersistedNodeId) {
        changes.beforeSiblingId = newlyPersistedNodeId;
      }

      // Check if this node has the newly-persisted node as parentId
      if (node.parentId === newlyPersistedNodeId) {
        changes.parentId = newlyPersistedNodeId;
      }

      if (Object.keys(changes).length > 0) {
        nodesToUpdate.push({ nodeId: id, changes });
      }
    }

    if (nodesToUpdate.length === 0) {
      return; // No deferred updates needed
    }

    // Update each node through normal updateNode flow
    // Use 'database' source with explicit immediate persistence
    const reconciliationSource: UpdateSource = {
      type: 'database',
      reason: 'deferred-reference-reconciliation'
    };

    for (const { nodeId, changes } of nodesToUpdate) {
      this.updateNode(nodeId, changes, reconciliationSource, {
        skipConflictDetection: true, // Skip conflict detection for reconciliation
        persist: 'immediate' // Explicit immediate persistence
      });
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
          // SPECIAL CASE: NodeType changes with content are not conflicts - they're pattern-triggered conversions
          // Allow nodeType updates to proceed even if content overlaps with pending update
          const isNodeTypeConversion = 'nodeType' in incomingUpdate.changes;
          if (isNodeTypeConversion) {
            // Allow this update - it's a pattern-triggered conversion, not a conflict
            continue;
          }

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
            sub.callback(node, { type: 'database', reason: 'store-cleared' });
          } catch (error) {
            console.error(`[SharedNodeStore] Subscription callback error:`, error);
          }
        }
      }
    }
  }

  /**
   * Determine the type of update based on which fields changed
   *
   * @param changes - Partial node data representing the changes
   * @returns 'structure' for hierarchy changes, 'metadata' for computed fields, 'content' otherwise
   */
  private determineUpdateType(changes: Partial<Node>): 'content' | 'structure' | 'metadata' {
    // Structural changes take precedence
    if ('parentId' in changes || 'beforeSiblingId' in changes || 'containerNodeId' in changes) {
      return 'structure';
    }

    // Metadata-only changes (computed fields that don't affect content)
    if (this.isMetadataOnlyUpdate(changes)) {
      return 'metadata';
    }

    return 'content';
  }

  /**
   * Check if an update only modifies metadata (computed fields)
   *
   * @param changes - Partial node data representing the changes
   * @returns true if only computed/derived fields changed
   */
  private isMetadataOnlyUpdate(changes: Partial<Node>): boolean {
    // Currently only mentions are metadata-only (computed from content)
    // Future: Could include other computed fields (tags, backlinks, etc.)
    return 'mentions' in changes && Object.keys(changes).length === 1;
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
  // Atomic Batch Updates
  // ========================================================================

  /**
   * Start an atomic batch update for a node
   * All subsequent updates for this nodeId will be accumulated until commitBatch()
   *
   * Use this for pattern conversions where content + nodeType must persist together:
   * - Quote blocks: content change + nodeType change must be atomic
   * - Code blocks: content change + nodeType change must be atomic
   * - Ordered lists: content change + nodeType change must be atomic
   *
   * @param nodeId - Node to batch updates for
   * @param timeoutMs - Auto-commit timeout in ms (default: DEFAULT_BATCH_TIMEOUT_MS = 2000ms)
   * @returns Batch ID for tracking
   *
   * @example
   * ```typescript
   * const batchId = store.startBatch(nodeId);
   * store.addToBatch(nodeId, { content: '> Quote text' });
   * store.addToBatch(nodeId, { nodeType: 'quote-block' });
   * store.commitBatch(nodeId); // Atomically persists both changes
   * ```
   */
  startBatch(nodeId: string, timeoutMs = DEFAULT_BATCH_TIMEOUT_MS): string {
    // Cancel existing batch if any (ensures clean state)
    this.cancelBatch(nodeId);

    // CRITICAL: Cancel any pending non-batched persistence operations
    // This prevents race between old debounced updates and new batch
    PersistenceCoordinator.getInstance().cancelPending(nodeId);

    // Use counter-based batch ID to prevent timing collisions
    // (Date.now() can return same value for rapid successive calls)
    const batchId = `batch-${nodeId}-${this.batchIdCounter++}`;
    const createdAt = Date.now();

    // Auto-commit after timeout to prevent abandoned batches
    const timeout = setTimeout(() => {
      console.warn('[SharedNodeStore] Auto-committing batch after inactivity timeout', {
        batchId,
        nodeId,
        timeoutMs,
        age: Date.now() - createdAt
      });
      this.commitBatch(nodeId);
    }, timeoutMs);

    this.activeBatches.set(nodeId, {
      nodeId,
      changes: {},
      batchId,
      createdAt,
      timeout,
      timeoutMs
    });

    return batchId;
  }

  /**
   * Add changes to the active batch for a node
   * Changes are accumulated and merged (later changes override earlier ones)
   * Updates in-memory state immediately (optimistic update)
   *
   * @param nodeId - Node to update
   * @param changes - Partial node changes to add to batch
   *
   * @example
   * ```typescript
   * store.startBatch(nodeId);
   * store.addToBatch(nodeId, { content: '1. ' });         // First change
   * store.addToBatch(nodeId, { nodeType: 'ordered-list' }); // Second change
   * store.commitBatch(nodeId); // Both persist atomically
   * ```
   */
  addToBatch(nodeId: string, changes: Partial<Node>): void {
    const batch = this.activeBatches.get(nodeId);
    if (!batch) {
      console.warn('[SharedNodeStore] Attempted to add to non-existent batch', {
        nodeId,
        changes: Object.keys(changes)
      });
      return;
    }

    // Merge changes into batch (later changes override)
    Object.assign(batch.changes, changes);

    // Update in-memory state immediately (optimistic)
    const currentNode = this.nodes.get(nodeId);
    if (currentNode) {
      const updatedNode = { ...currentNode, ...changes };
      this.nodes.set(nodeId, updatedNode);

      // Notify subscribers of optimistic update
      this.notifySubscribers(nodeId, updatedNode, { type: 'viewer', viewerId: 'batch' });
    }

    // Reset timeout to extend batch lifetime while user is actively making changes
    // This ensures batch only commits after true inactivity (no changes for N seconds)
    this.resetBatchTimeout(nodeId);
  }

  /**
   * Commit an active batch atomically
   * Runs placeholder detection on final state and persists if not a placeholder
   *
   * Edge case handling:
   * - If node was previously persisted and becomes a placeholder (user deleted content),
   *   still persist to update database with empty/placeholder state
   *
   * @param nodeId - Node whose batch to commit
   */
  commitBatch(nodeId: string): void {
    const batch = this.activeBatches.get(nodeId);
    if (!batch) {
      return; // No batch active, nothing to commit
    }

    // CRITICAL: Clear timeout and remove batch FIRST (ensures cleanup even on error)
    // This prevents memory leaks if persistBatchedChanges() throws
    clearTimeout(batch.timeout);
    this.activeBatches.delete(nodeId);

    try {
      // Get final node state after all batch changes
      const finalNode = this.nodes.get(nodeId);
      if (!finalNode) {
        console.warn('[SharedNodeStore] Batch commit aborted - node not found', {
          nodeId,
          batchId: batch.batchId
        });
        return;
      }

      // Nothing to persist if batch has no changes
      if (Object.keys(batch.changes).length === 0) {
        return;
      }

      // Check if final state is a placeholder
      const isPlaceholder = isPlaceholderNode(finalNode);
      const wasPersisted = this.persistedNodeIds.has(nodeId);

      // Persist if:
      // 1. Node has real content (not a placeholder), OR
      // 2. Node became a placeholder but was already persisted (need to update DB)
      //    Example: User typed "> Hello" (persisted), then deleted back to "> " (placeholder)
      const shouldPersist = !isPlaceholder || wasPersisted;

      if (shouldPersist) {
        // Always persist with full batched changes
        // This handles both new nodes and fixing race conditions where old path persisted first
        this.persistBatchedChanges(nodeId, batch.changes, finalNode);
      }
    } catch (error) {
      console.error('[SharedNodeStore] Batch commit error', {
        nodeId,
        batchId: batch.batchId,
        error
      });
      // Re-throw to surface to caller, but cleanup is already done
      throw error;
    }
  }

  /**
   * Cancel an active batch without persisting
   * Used when batch should be abandoned (e.g., node deleted during batch)
   *
   * @param nodeId - Node whose batch to cancel
   */
  cancelBatch(nodeId: string): void {
    const batch = this.activeBatches.get(nodeId);
    if (batch) {
      clearTimeout(batch.timeout);
      this.activeBatches.delete(nodeId);
    }
  }

  /**
   * Commit all active batches globally
   * Used when component unmounts to ensure all pending batched changes are saved
   */
  commitAllBatches(): void {
    const nodeIds = Array.from(this.activeBatches.keys());
    console.log('[SharedNodeStore] Committing all batches:', nodeIds.length, 'active');
    for (const nodeId of nodeIds) {
      this.commitBatch(nodeId);
    }
  }

  /**
   * Reset the auto-commit timeout for an active batch
   * Extends the batch lifetime when user continues making changes
   *
   * This implements "true inactivity" timeout:
   * - Timer resets on every change (content, nodeType, metadata, etc.)
   * - Batch only commits after N seconds of NO activity
   * - Prevents premature commits while user is actively typing
   *
   * @param nodeId - Node whose batch timeout to reset
   *
   * @example
   * ```typescript
   * store.startBatch(nodeId); // Start with default timeout (2s)
   * // ... user types ...
   * store.addToBatch(nodeId, { content: 'new' }); // Resets timeout to 2s
   * // ... user types more ...
   * store.addToBatch(nodeId, { content: 'newer' }); // Resets timeout to 2s again
   * // ... after 2s of no activity, auto-commit fires
   * ```
   */
  private resetBatchTimeout(nodeId: string): void {
    const batch = this.activeBatches.get(nodeId);
    if (!batch) {
      return; // No batch active
    }

    // Clear existing timeout
    clearTimeout(batch.timeout);

    // Create new timeout with same duration
    const timeout = setTimeout(() => {
      this.commitBatch(nodeId);
    }, batch.timeoutMs);

    // Update batch with new timeout (keep other properties)
    batch.timeout = timeout;
  }

  /**
   * Persist batched changes atomically
   * Delegates to existing persistence infrastructure
   *
   * @param nodeId - Node to persist
   * @param changes - Accumulated changes from batch
   * @param finalNode - Final node state after batch
   */
  private persistBatchedChanges(nodeId: string, changes: Partial<Node>, finalNode: Node): void {
    const isPersistedToDatabase = this.persistedNodeIds.has(nodeId);

    // Use PersistenceCoordinator for coordinated persistence
    const dependencies: Array<string | (() => Promise<void>)> = [];

    // Ensure parent chain is persisted (FOREIGN KEY)
    if (finalNode.parentId) {
      dependencies.push(async () => {
        await this.ensureAncestorChainPersisted(finalNode.parentId!);
      });
    }

    // Ensure containerNodeId is persisted (FOREIGN KEY)
    if (
      finalNode.containerNodeId &&
      finalNode.containerNodeId !== finalNode.parentId &&
      !this.persistedNodeIds.has(finalNode.containerNodeId)
    ) {
      dependencies.push(finalNode.containerNodeId);
    }

    // Ensure beforeSiblingId is persisted (FOREIGN KEY)
    if (finalNode.beforeSiblingId && !this.persistedNodeIds.has(finalNode.beforeSiblingId)) {
      dependencies.push(finalNode.beforeSiblingId);
    }

    // Persist with immediate mode (batches should not be debounced)
    PersistenceCoordinator.getInstance().persist(
      nodeId,
      async () => {
        try {
          // RACE CONDITION HANDLING:
          // ========================
          // SCENARIO: User types "> text" in text node
          // t=0ms:    Content "> " queued for debounced persistence (500ms delay)
          // t=200ms:  Pattern detected → startBatch() called
          // t=300ms:  User continues typing → batched updates accumulate
          // t=500ms:  Debounced persistence fires → node persisted via old path (race!)
          // t=2200ms: Batch commits → tries CREATE but node already exists
          //
          // SOLUTION: Try CREATE first (standard case), but if it fails with UNIQUE constraint,
          // fall back to UPDATE with batched changes to fix the race
          //
          // STRATEGY: Try UPDATE first if we know node is persisted, otherwise CREATE
          if (isPersistedToDatabase) {
            // Get current version for optimistic concurrency control
            const currentVersion = finalNode.version ?? 1;
            await tauriNodeService.updateNode(nodeId, currentVersion, changes);
          } else {
            // Try CREATE, but handle race condition where old path persisted first
            try {
              await tauriNodeService.createNode(finalNode);
              this.persistedNodeIds.add(nodeId);

              // Update deferred sibling references
              this.updateDeferredSiblingReferences(nodeId);
            } catch (createError) {
              // If CREATE fails (node already exists from race), try UPDATE with batched changes
              if (
                createError instanceof Error &&
                (createError.message.includes('UNIQUE constraint') ||
                  createError.message.includes('already exists'))
              ) {
                // Race detected: Old debounced path persisted before batch started
                // Update with batched changes to fix inconsistent state
                const currentVersion = finalNode.version ?? 1;
                await tauriNodeService.updateNode(nodeId, currentVersion, changes);
                this.persistedNodeIds.add(nodeId);
              } else {
                throw createError;
              }
            }
          }
        } catch (dbError) {
          const error = dbError instanceof Error ? dbError : new Error(String(dbError));

          // Suppress expected errors in in-memory test mode
          if (shouldLogDatabaseErrors()) {
            console.error(`[SharedNodeStore] Batch persistence failed for node ${nodeId}:`, error);
          }

          // Always track errors in test environment for verification
          this.trackErrorIfTesting(error);

          throw error;
        }
      },
      {
        mode: 'immediate', // Batches are already accumulated, persist immediately
        dependencies: dependencies.length > 0 ? dependencies : undefined
      }
    );
  }

  // ========================================================================
  // Test Utilities
  // ========================================================================

  /**
   * Check if there are pending database writes
   * Used by tests to wait for all writes to complete
   * Delegates to PersistenceCoordinator
   */
  hasPendingWrites(): boolean {
    const metrics = PersistenceCoordinator.getInstance().getMetrics();
    return metrics.pendingOperations > 0;
  }

  /**
   * Get test errors (only populated in test environment)
   * Used by tests to verify database operations succeeded
   */
  getTestErrors(): Error[] {
    return [...this.testErrors];
  }

  /**
   * Track error in test environment for verification
   * Only adds errors when NODE_ENV='test'
   *
   * @param error - Error to track for test verification
   * @private
   */
  private trackErrorIfTesting(error: Error): void {
    if (isTestEnvironment()) {
      this.testErrors.push(error);
    }
  }

  /**
   * Clear test errors
   * Should be called at the start of each test for isolation
   */
  clearTestErrors(): void {
    this.testErrors = [];
  }

  /**
   * Reset store state (for testing only)
   * @internal
   */
  __resetForTesting(): void {
    this.nodes.clear();
    this.persistedNodeIds.clear();
    this.subscriptions.clear();
    this.wildcardSubscriptions.clear();
    this.pendingUpdates.clear();
    this.versions.clear();
    this.testErrors = [];

    // Cancel all active batches
    for (const [nodeId] of this.activeBatches) {
      this.cancelBatch(nodeId);
    }
    this.activeBatches.clear();

    this.metrics = {
      updateCount: 0,
      avgUpdateTime: 0,
      maxUpdateTime: 0,
      subscriptionCount: 0,
      conflictCount: 0,
      rollbackCount: 0
    };
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
