<!--
  BaseNodeViewer - Container that manages a collection of nodes
  Handles node creation, deletion, and organization
  
  Now uses NodeServiceContext to provide @ autocomplete functionality
  to all TextNode components automatically via proper inheritance.
-->

<script lang="ts">
  import { onMount, onDestroy, getContext } from 'svelte';
  import { htmlToMarkdown } from '$lib/utils/markdown.js';
  import { formatTabTitle } from '$lib/utils/text-formatting';
  import { registerChildWithParent } from '$lib/utils/node-hierarchy';
  import { pluginRegistry } from '$lib/components/viewers/index';
  import BaseNode from '$lib/design/components/base-node.svelte';
  import BacklinksPanel from '$lib/design/components/backlinks-panel.svelte';
  import SchemaPropertyForm from '$lib/components/property-forms/schema-property-form.svelte';
  import { getNodeServices } from '$lib/contexts/node-service-context.svelte';
  import { sharedNodeStore } from '$lib/services/shared-node-store';
  import { tauriNodeService } from '$lib/services/tauri-node-service';
  import { PersistenceCoordinator } from '$lib/services/persistence-coordinator.svelte';
  import { focusManager } from '$lib/services/focus-manager.svelte';
  import { NodeExpansionCoordinator } from '$lib/services/node-expansion-coordinator';
  import type { Node } from '$lib/types';
  import type { UpdateSource } from '$lib/types/update-protocol';
  import type { Snippet } from 'svelte';
  import { DEFAULT_PANE_ID } from '$lib/stores/navigation';
  import { getViewerId, saveScrollPosition, getScrollPosition } from '$lib/stores/scroll-state';

  // Get paneId from context (set by PaneContent)
  const paneId = getContext<string>('paneId') ?? DEFAULT_PANE_ID;

  // Props
  let {
    header,
    nodeId = null,
    /**
     * Tab identifier for this viewer instance.
     * Combined with paneId (from context) to create a unique scroll position identifier.
     * Each tab+pane combination maintains independent scroll state, allowing the same
     * document to be viewed in multiple panes with different scroll positions.
     * @default 'default'
     */
    tabId = 'default',
    onTitleChange,
    /**
     * Disable automatic title updates from node content.
     *
     * When true, this component will NOT call onTitleChange based on node content.
     * The parent component is responsible for managing the title completely.
     *
     * Use Cases:
     * - DateNodeViewer: Computes title from date ("Today", "Tomorrow", "Yesterday")
     * - Custom viewers with dynamic titles unrelated to node content
     *
     * @default false - BaseNodeViewer manages title from node.content
     */
    disableTitleUpdates = false
  }: {
    header?: Snippet;
    nodeId?: string | null;
    tabId?: string;
    onTitleChange?: (_title: string) => void;
    onNodeIdChange?: (_nodeId: string) => void; // In type for interface, not used by BaseNodeViewer
    disableTitleUpdates?: boolean;
  } = $props();

  // Get nodeManager from shared context
  const services = getNodeServices();
  if (!services) {
    throw new Error(
      'NodeServices not available. Make sure base-node-viewer is wrapped in NodeServiceContext.'
    );
  }

  const nodeManager = services.nodeManager;

  // Define update source for all BaseNodeViewer operations
  // Using 'viewer' type to indicate updates originating from this UI component
  const VIEWER_SOURCE: UpdateSource = {
    type: 'viewer',
    viewerId: nodeId || 'root'
  };

  // Editable header state (for default header when no custom snippet provided)
  // Use local $state so input binding works and we can update tab title immediately
  let headerContent = $state('');

  // Track last saved content to detect actual changes
  const lastSavedContent = new Map<string, string>();

  // Cancellation flag to prevent database writes after component unmounts
  let isDestroyed = false;

  // Loading flag to prevent watchers from firing during initial load
  // Start as true to prevent watcher from firing before loadChildrenForParent() completes
  let isLoadingInitialNodes = true;

  // Viewer-local placeholder (not in sharedNodeStore until it gets content)
  // This placeholder is only visible to this viewer instance
  let viewerPlaceholder = $state<Node | null>(null);

  // Track the viewed node reactively for schema form display
  let currentViewedNode = $state<Node | null>(null);

  // Scroll position tracking
  // Reference to the scroll container element
  let scrollContainer: HTMLElement | null = null;
  // Generate unique viewer ID for this viewer instance
  const viewerId = getViewerId(tabId, paneId);

  // Set view context, load children, and initialize header content when nodeId changes
  $effect(() => {
    // Removed setViewParentId - now pass nodeId directly to visibleNodes()

    if (nodeId) {
      // Capture disableTitleUpdates at effect creation time (not in async callback)
      // This prevents stale closure issues when component is destroyed before callback fires
      const shouldDisableTitleUpdates = disableTitleUpdates;

      // Load children asynchronously - this will load the parent node first
      loadChildrenForParent(nodeId).then(() => {
        // CRITICAL: Prevent state updates after component destruction
        // This prevents memory leaks and state corruption from stale async callbacks
        if (isDestroyed) {
          return;
        }

        // After loading completes, initialize header content and update tab title
        // This ensures the node is loaded before we try to read its content
        const node = sharedNodeStore.getNode(nodeId);
        headerContent = node?.content || '';

        // Update reactive node reference for schema form
        currentViewedNode = node || null;

        // Update tab title after node is loaded
        // Skip if parent component manages the title (e.g., DateNodeViewer)
        if (!shouldDisableTitleUpdates) {
          updateTabTitle(headerContent);
        }
      });
    } else {
      // Clear when no nodeId
      currentViewedNode = null;
    }
  });

  /**
   * Update tab title from header content
   *
   * Uses shared formatTabTitle utility to ensure consistent tab title formatting
   * across all viewers and navigation. Multi-line content is automatically
   * truncated to first line, and long titles are shortened with ellipsis.
   *
   * @param content - Full node content (may be multi-line)
   */
  function updateTabTitle(content: string) {
    if (onTitleChange) {
      onTitleChange(formatTabTitle(content));
    }
  }

  /**
   * Handle header content changes (for default editable header)
   * Updates local state, tab title, and persists to database
   */
  function handleHeaderInput(newValue: string) {
    // Update local state (since we use one-way binding)
    headerContent = newValue;

    // Update tab title immediately
    updateTabTitle(newValue);

    // Update node content in database if nodeId exists
    // Use the same method as child nodes to ensure consistent behavior
    if (nodeId) {
      try {
        nodeManager.updateNodeContent(nodeId, newValue);
      } catch (error) {
        console.error('[BaseNodeViewer] Failed to update header content:', error);
        // TODO: Show user-facing error notification via event bus
        // eventBus.emit({ type: 'error:node-update-failed', nodeId, error });
      }
    }
  }

  // Track pending content saves for new nodes (keyed by node ID)
  // Structural updates must wait for these to complete to avoid FOREIGN KEY errors
  const pendingContentSavePromises = new Map<string, Promise<void>>();

  // Timeout configuration for promise coordination
  const CONTENT_SAVE_TIMEOUT_MS = 5000; // 5 seconds

  // Explicit coordination: Promise that resolves when content save phase completes
  // This makes the dependency between watchers explicit rather than relying on declaration order
  let contentSavePhasePromise: Promise<void> = Promise.resolve();

  // Explicit coordination: Promise that resolves when structural updates complete
  // The deletion watcher awaits this to ensure children are reassigned before parent deletion
  let pendingStructuralUpdatesPromise: Promise<void> | null = null;

  /**
   * Node types that have structured content and cannot accept arbitrary merges
   * These nodes must maintain specific formatting (e.g., code fences, quote prefixes)
   */
  const STRUCTURED_NODE_TYPES = ['code-block', 'quote-block'] as const;

  /**
   * Check if a node type is a structured node that cannot accept arbitrary merges
   * @param nodeType - The node type to check
   * @returns true if the node type is structured and cannot accept merges
   */
  function isStructuredNode(nodeType: string): boolean {
    return STRUCTURED_NODE_TYPES.includes(nodeType as (typeof STRUCTURED_NODE_TYPES)[number]);
  }

  /**
   * Extract and transform node properties into component-compatible metadata
   *
   * Handles the mismatch between database property storage and component metadata expectations:
   * - Task nodes: Maps properties.task.status → metadata.taskState (for icon rendering)
   * - Other nodes: Returns properties as-is for future extension
   *
   * @param node - Node with properties from database
   * @returns Metadata object compatible with node component expectations
   */
  function extractNodeMetadata(node: {
    nodeType: string;
    properties?: Record<string, unknown>;
  }): Record<string, unknown> {
    const properties = node.properties || {};

    // Task nodes: Map schema status to taskState for icon rendering
    if (node.nodeType === 'task') {
      const taskProps = properties[node.nodeType] as Record<string, unknown> | undefined;
      const status = taskProps?.status || properties.status; // Support both nested and flat formats

      // Map task status to NodeState expected by TaskNode
      let taskState: 'pending' | 'inProgress' | 'completed' = 'pending';
      if (status === 'IN_PROGRESS') {
        taskState = 'inProgress';
      } else if (status === 'DONE') {
        taskState = 'completed';
      } else if (status === 'OPEN') {
        taskState = 'pending';
      }

      return { taskState, ...properties };
    }

    // Default: Return properties as-is
    return properties;
  }

  /**
   * Update a schema field value for a node (schema-aware property update)
   *
   * Follows the same nested format pattern as schema-property-form.svelte:
   * - Builds nested structure: properties[nodeType][fieldName] = value
   * - Handles auto-migration from flat to nested format
   * - Calls sharedNodeStore.updateNode() to persist
   *
   * @param targetNodeId - Node ID to update
   * @param fieldName - Schema field name (e.g., 'status', 'due_date')
   * @param value - New value for the field
   */
  function updateSchemaField(targetNodeId: string, fieldName: string, value: unknown) {
    const targetNode = sharedNodeStore.getNode(targetNodeId);
    if (!targetNode) return;

    // Build nested namespace (properties[nodeType][fieldName])
    const typeNamespace = targetNode.properties?.[targetNode.nodeType];
    const isOldFormat = !typeNamespace || typeof typeNamespace !== 'object';

    let updatedNamespace: Record<string, unknown> = {};

    if (isOldFormat) {
      // Migrate from old flat format - copy ALL existing flat properties into namespace
      updatedNamespace = { ...targetNode.properties };
      // Remove internal fields that shouldn't be in namespace
      delete updatedNamespace._schema_version;
    } else {
      // Already in new format - copy namespace
      updatedNamespace = { ...(typeNamespace as Record<string, unknown>) };
    }

    // Apply the update
    updatedNamespace[fieldName] = value;

    // Build final properties with ONLY the nested namespace
    // CRITICAL: When migrating from old format, ALL flat properties are now in the namespace
    // So we start fresh with ONLY the nested structure, dropping all flat properties
    const updatedProperties = isOldFormat
      ? {
          // Old format: Start fresh with ONLY nested structure (drops ALL flat properties)
          [targetNode.nodeType]: updatedNamespace
        }
      : {
          // New format: Preserve existing properties structure
          ...targetNode.properties,
          [targetNode.nodeType]: updatedNamespace
        };

    // Persist via sharedNodeStore
    sharedNodeStore.updateNode(
      targetNodeId,
      { properties: updatedProperties },
      { type: 'viewer', viewerId: viewerId }
    );
  }

  /**
   * Wait for a pending node save to complete, with timeout and grace period
   * Delegates to SharedNodeStore which tracks pending saves
   * @param nodeIds - Array of node IDs to wait for
   * @returns Set of node IDs that failed to save (should be excluded from updates)
   */
  async function waitForNodeSavesIfPending(nodeIds: string[]): Promise<Set<string>> {
    return await sharedNodeStore.waitForNodeSaves(nodeIds, CONTENT_SAVE_TIMEOUT_MS);
  }

  /**
   * Build error event for persistence failures
   * Extracted for clarity and reusability
   */
  function buildPersistenceErrorEvent(
    failedNodeIds: Set<string>,
    failedUpdates: Array<{
      nodeId: string;
      beforeSiblingId: string | null;
    }>
  ) {
    // Determine failure reason based on what failed
    let failureReason: 'timeout' | 'foreign-key-constraint' | 'database-locked' | 'unknown' =
      'unknown';
    if (failedNodeIds.size > 0) {
      failureReason = 'timeout'; // Nodes that failed to save due to timeout
    } else if (failedUpdates.length > 0) {
      // Check error messages to determine specific reason
      failureReason = 'foreign-key-constraint'; // Most common for structural updates
    }

    // Build detailed operation list
    const affectedOperations = [
      // Failed saves (new nodes that timed out)
      ...Array.from(failedNodeIds).map((nodeId) => ({
        nodeId,
        operation: 'create' as const,
        error: 'Save operation timed out'
      })),
      // Failed updates (structural changes that failed)
      ...failedUpdates.map((update) => ({
        nodeId: update.nodeId,
        operation: 'update' as const,
        error: 'Structural update failed (possible FOREIGN KEY constraint)'
      }))
    ];

    const totalFailed = failedNodeIds.size + failedUpdates.length;

    return {
      type: 'error:persistence-failed',
      namespace: 'error',
      source: 'base-node-viewer',
      message: `Failed to persist ${totalFailed} node(s). Changes have been reverted.`,
      failedNodeIds: Array.from(failedNodeIds),
      failureReason,
      canRetry: failureReason === 'timeout', // Timeouts might succeed on retry
      affectedOperations
    };
  }

  // ============================================================================
  // CONTENT SAVE WATCHER - Initiates content saves for new nodes
  // ============================================================================
  // This watcher creates a promise that the structural watcher awaits, ensuring
  // new nodes are saved to the database before structural updates reference them.
  // This prevents FOREIGN KEY constraint errors.
  // ============================================================================
  $effect.pre(() => {
    if (!nodeId) {
      contentSavePhasePromise = Promise.resolve();
      return;
    }

    // Skip if we're still loading initial nodes from database
    if (isLoadingInitialNodes) {
      contentSavePhasePromise = Promise.resolve();
      return;
    }

    // Create a new promise for this content save phase
    // The structural watcher will await this before processing updates
    let resolvePhase: () => void;
    contentSavePhasePromise = new Promise((resolve) => {
      resolvePhase = resolve;
    });

    const nodes = nodeManager.visibleNodes(nodeId);

    for (const node of nodes) {
      // Skip placeholder nodes
      if (node.isPlaceholder) {
        continue;
      }

      // Only save if content has changed since last save
      const lastContent = lastSavedContent.get(node.id);
      if (node.content.trim() && node.content !== lastContent) {
        // Check if this is a brand new node (never saved before)
        const isNewNode = lastContent === undefined;

        if (isNewNode) {
          // Save immediately without debounce - structural updates may need to reference this node
          // First ensure ancestors are persisted, then save this node
          const savePromise = (async () => {
            // Check if component was destroyed before starting save
            if (isDestroyed) return;

            await ensureAncestorsPersisted(node.id);

            // Check again after async operation
            if (isDestroyed) return;

            // Create/update node immediately (was saveNodeImmediately)
            const fullNode: Node = {
              id: node.id,
              nodeType: node.nodeType,
              content: node.content,
              beforeSiblingId: node.beforeSiblingId,
              createdAt: node.createdAt || new Date().toISOString(),
              modifiedAt: new Date().toISOString(),
              version: node.version || 1, // Use existing version or default to 1 for new nodes
              properties: node.properties || {},
              mentions: node.mentions || []
            };
            sharedNodeStore.setNode(fullNode, VIEWER_SOURCE);
          })();

          // Clean up Map entry when save completes (success or failure)
          savePromise.finally(() => {
            pendingContentSavePromises.delete(node.id);
            lastSavedContent.set(node.id, node.content);
          });

          pendingContentSavePromises.set(node.id, savePromise);
        } else {
          // Existing node - update content (triggers debounced persistence automatically)
          if (!isDestroyed) {
            sharedNodeStore.updateNode(
              node.id,
              { content: node.content, nodeType: node.nodeType },
              VIEWER_SOURCE
            );
            lastSavedContent.set(node.id, node.content);
          }
        }
      }
    }

    // Mark content save phase as complete by resolving the promise
    // Use Promise.resolve().then() to ensure this happens after all synchronous code
    Promise.resolve().then(() => resolvePhase());
  });

  // Track node IDs to detect deletions
  // Use a regular variable, not $state, to avoid infinite loops
  let previousNodeIds = new Set<string>();

  $effect(() => {
    if (!nodeId) return;

    const currentNodeIds = new Set(nodeManager.visibleNodes(nodeId).map((n) => n.id));

    // Skip the first run (when previousNodeIds is empty)
    if (previousNodeIds.size > 0) {
      // Detect deleted nodes by comparing with previous state
      const deletedNodeIds: string[] = [];
      for (const prevId of previousNodeIds) {
        if (!currentNodeIds.has(prevId) && !nodeManager.findNode(prevId)) {
          deletedNodeIds.push(prevId);
        }
      }

      // Delete nodes through the global write queue
      // The queue ensures deletions happen after any pending structural updates
      if (deletedNodeIds.length > 0) {
        // Clean up previousStructure: null out any beforeSiblingId references to deleted nodes
        // This prevents FOREIGN KEY errors when other nodes reference the deleted node
        // Uses reverse index for O(1) lookup instead of O(n) iteration
        for (const deletedId of deletedNodeIds) {
          const affectedNodes = siblingToNodesMap.get(deletedId) || new Set();
          for (const nodeId of affectedNodes) {
            const structure = previousStructure.get(nodeId);
            if (structure) {
              // Use centralized tracking function
              trackStructureChange(nodeId, {
                ...structure,
                beforeSiblingId: null
              });
            }
          }
          // Clean up the deleted node's entry
          siblingToNodesMap.delete(deletedId);
        }

        (async () => {
          // CRITICAL: Await structural updates to complete before deleting
          // This ensures children are reassigned BEFORE parent deletion triggers CASCADE
          if (pendingStructuralUpdatesPromise) {
            try {
              await Promise.race([
                pendingStructuralUpdatesPromise,
                new Promise((_, reject) =>
                  setTimeout(
                    () => reject(new Error('Timeout waiting for structural updates')),
                    CONTENT_SAVE_TIMEOUT_MS
                  )
                )
              ]);
            } catch (error) {
              console.warn(
                '[BaseNodeViewer] Timeout or error waiting for structural updates:',
                error
              );
            }
          }

          // Now safe to delete nodes - children have been reassigned
          // Delegate to SharedNodeStore
          for (const nodeId of deletedNodeIds) {
            sharedNodeStore.deleteNode(nodeId, VIEWER_SOURCE);
          }
        })();
      }
    }

    // Update previous state (mutate the Set instead of replacing it)
    previousNodeIds.clear();
    for (const id of currentNodeIds) {
      previousNodeIds.add(id);
    }
  });

  // ============================================================================
  // STRUCTURAL CHANGE WATCHER - Persists structural changes
  // ============================================================================
  // This watcher awaits contentSavePhasePromise to ensure new nodes are saved
  // before structural updates reference them. This prevents FOREIGN KEY errors.
  //
  // ISSUE #479: Track PERSISTED structure vs in-memory structure
  // When Enter+Tab happens rapidly, node is created with wrong parentId, then
  // immediately indented in SharedNodeStore. The structural watcher must detect
  // this mismatch and persist the correction.
  // ============================================================================
  // previousStructure is updated in three places (all necessary):
  // 1. Deletion watcher (line ~218): Cleans up beforeSiblingId references to deleted nodes
  // 2. This watcher (line ~388): Tracks nodes on first sight
  // 3. This watcher (line ~490): Tracks successfully persisted structural changes (source of truth)
  let previousStructure = new Map<
    string,
    { beforeSiblingId: string | null }
  >();

  // Track what structure was actually persisted to database (#479)
  // This allows detecting when in-memory state differs from persisted state
  let persistedStructure = new Map<
    string,
    { beforeSiblingId: string | null }
  >();

  // Reverse index: Map from beforeSiblingId -> Set of node IDs that reference it
  // This allows O(1) lookup when cleaning up deleted sibling references
  const siblingToNodesMap = new Map<string, Set<string>>();

  /**
   * Update reverse index when tracking structure changes
   */
  function updateSiblingIndex(
    nodeId: string,
    oldBeforeSiblingId: string | null,
    newBeforeSiblingId: string | null
  ): void {
    // Remove from old index entry
    if (oldBeforeSiblingId) {
      const nodes = siblingToNodesMap.get(oldBeforeSiblingId);
      if (nodes) {
        nodes.delete(nodeId);
        if (nodes.size === 0) {
          siblingToNodesMap.delete(oldBeforeSiblingId);
        }
      }
    }

    // Add to new index entry
    if (newBeforeSiblingId) {
      if (!siblingToNodesMap.has(newBeforeSiblingId)) {
        siblingToNodesMap.set(newBeforeSiblingId, new Set());
      }
      siblingToNodesMap.get(newBeforeSiblingId)!.add(nodeId);
    }
  }

  /**
   * Update previousStructure tracking and reverse index
   * Centralizes structure change tracking logic used in three places:
   * 1. Deletion watcher - cleaning up beforeSiblingId references
   * 2. Structural watcher - tracking unchanged nodes
   * 3. Structural watcher - tracking successfully persisted changes
   */
  function trackStructureChange(
    nodeId: string,
    newStructure: { beforeSiblingId: string | null }
  ): void {
    const oldStructure = previousStructure.get(nodeId);
    previousStructure.set(nodeId, newStructure);

    // Update reverse index if beforeSiblingId changed
    if (!oldStructure || oldStructure.beforeSiblingId !== newStructure.beforeSiblingId) {
      updateSiblingIndex(
        nodeId,
        oldStructure?.beforeSiblingId || null,
        newStructure.beforeSiblingId
      );
    }
  }

  $effect.pre(() => {
    if (!nodeId) return;

    const visibleNodes = nodeManager.visibleNodes(nodeId);

    // Collect all structural changes first
    const updates: Array<{
      nodeId: string;
      beforeSiblingId: string | null;
    }> = [];

    for (const node of visibleNodes) {
      // Issue #479: Do NOT skip placeholder nodes in structural watcher
      // isPlaceholder is a UI-only property for viewer styling
      // All structural changes must be persisted regardless of placeholder status

      const currentStructure = {
        beforeSiblingId: node.beforeSiblingId
      };

      const prevStructure = previousStructure.get(node.id);
      const persisted = persistedStructure.get(node.id);

      // Issue #479: Check if current structure differs from what was persisted
      // This catches cases where Tab indent happened after node was created but before structural watcher ran
      const needsPersistenceCorrection =
        persisted &&
        persisted.beforeSiblingId !== currentStructure.beforeSiblingId;

      if (needsPersistenceCorrection) {
        // Current in-memory structure differs from persisted - fix database
        updates.push({
          nodeId: node.id,
          beforeSiblingId: node.beforeSiblingId
        });
      } else if (
        prevStructure &&
        prevStructure.beforeSiblingId !== currentStructure.beforeSiblingId
      ) {
        // Normal structural change detected - queue for persistence
        updates.push({
          nodeId: node.id,
          beforeSiblingId: node.beforeSiblingId
        });
      } else {
        // No change detected OR new node - update tracking to current state
        trackStructureChange(node.id, currentStructure);
        // Issue #479: For nodes loaded from database (first time seeing), query their persisted structure
        // Don't assume current = persisted, as Enter+Tab can create mismatch
        if (!persisted && sharedNodeStore.isNodePersisted(node.id)) {
          // Node exists in database - current structure IS the persisted one (loaded from DB)
          persistedStructure.set(node.id, currentStructure);
        }
        // If node is NOT persisted yet (new node being created), don't track persisted structure
        // until it actually gets persisted (handled in success callback above)
      }
    }

    // Persist updates sequentially to avoid SQLite "database is locked" errors
    // All writes go through the global queue to ensure serialization
    if (updates.length > 0) {
      // Process updates asynchronously - queue ensures proper serialization
      // Track this promise so deletion watcher can await completion
      pendingStructuralUpdatesPromise = (async () => {
        // CRITICAL: Wait for content save phase to complete first
        await contentSavePhasePromise;

        // Wait for any pending content saves to complete
        const nodeIdsToWaitFor: string[] = [];
        for (const update of updates) {
          if (update.beforeSiblingId) nodeIdsToWaitFor.push(update.beforeSiblingId);
        }

        const failedNodeIds = await waitForNodeSavesIfPending(nodeIdsToWaitFor);

        // Filter out updates that reference failed nodes
        const validUpdates = updates.filter(
          (update) =>
            !failedNodeIds.has(update.nodeId) &&
            (!update.beforeSiblingId || !failedNodeIds.has(update.beforeSiblingId))
        );

        // Delegate to SharedNodeStore for validation and persistence
        const result = await sharedNodeStore.updateStructuralChangesValidated(
          validUpdates,
          VIEWER_SOURCE,
          nodeId
        );

        // Update tracking for succeeded updates
        for (const update of result.succeeded) {
          const structure = {
            beforeSiblingId: update.beforeSiblingId
          };
          trackStructureChange(update.nodeId, structure);
          // Issue #479: Track what was actually persisted to database
          persistedStructure.set(update.nodeId, structure);
        }

        // Handle failed updates: rollback in-memory state and emit error event
        const allFailedNodeIds = new Set([...failedNodeIds, ...result.failed.map((u) => u.nodeId)]);

        if (allFailedNodeIds.size > 0) {
          console.warn(
            '[BaseNodeViewer] Failed to persist changes:',
            failedNodeIds.size,
            'save failure(s),',
            result.failed.length,
            'update failure(s)'
          );

          // Rollback in-memory state for failed updates to match last known database state
          for (const update of result.failed) {
            const lastGoodState = previousStructure.get(update.nodeId);
            if (lastGoodState) {
              // Revert node's in-memory structure to last successfully persisted state
              const node = nodeManager.nodes.get(update.nodeId);
              if (node) {
                nodeManager.nodes.set(update.nodeId, {
                  ...node,
                  beforeSiblingId: lastGoodState.beforeSiblingId
                });
              }
            }
          }

          // Emit event for error notification (UI can show toast/banner)
          import('$lib/services/event-bus').then(({ eventBus }) => {
            const event = buildPersistenceErrorEvent(failedNodeIds, result.failed);
            eventBus.emit(event as never);
          });
        }

        // Clear promise after all updates complete
        pendingStructuralUpdatesPromise = null;
      })();
    } else {
      // No updates to process - clear promise immediately
      pendingStructuralUpdatesPromise = null;
    }

    // Clean up tracking for nodes that no longer exist
    const currentNodeIds = new Set(visibleNodes.map((n) => n.id));
    for (const [nodeId] of previousStructure) {
      if (!currentNodeIds.has(nodeId)) {
        previousStructure.delete(nodeId);
      }
    }
  });

  /**
   * Helper function to promote a viewer-local placeholder to a real node
   * Extracts core Node properties without UI state (depth, children, expanded)
   *
   * @param placeholder - The viewer-local placeholder node
   * @param parentNodeId - The parent node ID
   * @param overrides - Content and/or nodeType to override from placeholder
   * @returns Promoted node with core properties only
   */
  function promotePlaceholderToNode(
    placeholder: Node,
    parentNodeId: string,
    overrides: { content?: string; nodeType?: string }
  ): Node & { _parentId?: string } {
    // CRITICAL FIX (Issue #528): Include transient _parentId field
    // This field is NOT part of the Node model (which stores hierarchy as graph edges)
    // but is needed by the backend HTTP API to create the parent-child edge relationship
    // The backend will extract this field and call operations.create_node() with it
    // Note (Issue #533): _containerId removed - backend auto-derives root from parent chain
    return {
      id: placeholder.id,
      nodeType: overrides.nodeType ?? placeholder.nodeType,
      content: overrides.content ?? placeholder.content,
      beforeSiblingId: placeholder.beforeSiblingId,
      version: placeholder.version,
      createdAt: placeholder.createdAt,
      modifiedAt: new Date().toISOString(),
      properties: placeholder.properties,
      mentions: placeholder.mentions || [],
      // Transient field for backend edge creation (not persisted in Node model)
      _parentId: parentNodeId // Root/container auto-derived from parent chain by backend
    };
  }

  async function loadChildrenForParent(nodeId: string, forceRefresh = false) {
    try {
      // Set loading flag to prevent watchers from triggering during initial load
      isLoadingInitialNodes = true;

      // Clear content tracking BEFORE loading to prevent watcher from firing on stale data
      lastSavedContent.clear();

      // CRITICAL FIX: Load the parent node itself first (for header/title display)
      // This is especially important for nodes viewed as pages where we need the node
      // content for the title and properties before loading children.
      // For virtual nodes (like date nodes), getNode returns null which we handle gracefully.
      //
      // SAFETY: Only load if node doesn't already exist in memory
      // This prevents overwriting nodes with pending unsaved changes
      if (!sharedNodeStore.hasNode(nodeId)) {
        const parentNode = await tauriNodeService.getNode(nodeId);
        if (parentNode) {
          // Add parent node to shared store so header can access it
          // Database source type automatically marks node as persisted
          sharedNodeStore.setNode(parentNode, { type: 'database', reason: 'loaded-from-db' });
        }
      }

      // Cache-first loading strategy: Check cache before hitting database (unless force refresh)
      let allNodes: Node[];

      if (!forceRefresh) {
        const cached = sharedNodeStore.getNodesForParent(nodeId);
        if (cached && cached.length > 0) {
          // Cache hit - use immediately (no database call!)
          allNodes = cached;
        } else {
          // Cache miss - fetch from database
          allNodes = await sharedNodeStore.loadChildrenForParent(nodeId);
        }
      } else {
        // Force refresh - bypass cache and fetch from database
        allNodes = await sharedNodeStore.loadChildrenForParent(nodeId);
      }

      // Check if we have any nodes at all (reuse allNodes - no redundant cache check needed)
      if (allNodes.length === 0) {
        // No persisted children - create initial placeholder if needed
        // Note: We already checked cache/DB above, so if allNodes is empty, no persisted children exist

        // Create initial placeholder only if we don't have a viewer placeholder already
        if (!viewerPlaceholder) {
          // No children at all - create initial placeholder
          // Issue #479 Phase 1: Placeholder is completely viewer-local (NOT added to SharedNodeStore)
          // It's rendered via nodesToRender derived state and promoted to real node when user adds content
          const placeholderId = globalThis.crypto.randomUUID();

          viewerPlaceholder = {
            id: placeholderId,
            nodeType: 'text',
            content: '',
            beforeSiblingId: null,
            createdAt: new Date().toISOString(),
            modifiedAt: new Date().toISOString(),
            version: 1,
            properties: {},
            mentions: []
          };

          // DON'T call initializeNodes() - keep placeholder completely viewer-local!
          // It will be rendered by nodesToRender derived state (line 1475-1487)
          // and promoted to real node when user adds content (line 1746-1772)
        }
      } else {
        // Real children exist - clear any viewer placeholder
        viewerPlaceholder = null;

        // Track initial content of ALL loaded nodes BEFORE initializing
        // This prevents the content watcher from thinking these are new nodes
        allNodes.forEach((node) => lastSavedContent.set(node.id, node.content));

        // Initialize with ALL nodes
        nodeManager.initializeNodes(allNodes, {
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0
        });
      }

      // CRITICAL FIX: Register viewer with expansion coordinator AFTER nodes are loaded AND initialized
      // This ensures restoration can find the nodes instead of skipping them all
      // Must be inside try block (not finally) to ensure initializeNodes() has completed
      // Only register once per viewer instance (coordinator handles re-registration gracefully)
      NodeExpansionCoordinator.registerViewer(tabId, nodeManager);
    } catch (error) {
      console.error('[BaseNodeViewer] Failed to load children for parent:', nodeId, error);
    } finally {
      // Clear loading flag after nodes are initialized
      isLoadingInitialNodes = false;
    }
  }

  /**
   * Recursively ensure all placeholder ancestors are persisted before saving a node
   * This handles the case where a user creates nested placeholder nodes, then fills in
   * a child before filling in the parent.
   *
   * NOTE: Ancestor persistence now handled by PersistenceCoordinator dependencies.
   * When a node is saved, coordinator automatically waits for parent dependencies.
   * This function kept for backward compatibility but is now a no-op.
   */
  async function ensureAncestorsPersisted(_nodeId: string): Promise<void> {
    // No-op: PersistenceCoordinator handles dependency ordering automatically
    // via the dependencies array in persist() calls
    return Promise.resolve();
  }

  // Focus handling function with proper cursor positioning using tree walker
  function requestNodeFocus(nodeId: string, position: number) {
    // Use FocusManager as single source of truth for focus management
    // This replaces the old DOM-based focus approach
    focusManager.setEditingNode(nodeId, paneId, position);

    // Force textarea update to ensure merged content is visible immediately
    // Especially important for Safari which doesn't always reactive-update properly
    setTimeout(() => {
      const node = nodeManager.nodes.get(nodeId);
      if (node) {
        const textarea = document.querySelector(
          `textarea[id="textarea-${nodeId}"]`
        ) as HTMLTextAreaElement;
        if (textarea && textarea.value !== node.content) {
          textarea.value = node.content;
          textarea.selectionStart = position;
          textarea.selectionEnd = position;
        }
      }
    }, 10);
  }

  // OLD IMPLEMENTATION - REPLACED BY FOCUSMANAGER
  // function requestNodeFocus(nodeId: string, position: number) {
  //   // Find the target node
  //   const node = nodeManager.findNode(nodeId);
  //   if (!node) {
  //     console.error(`Node ${nodeId} not found for focus request`);
  //     return;
  //   }
  //
  //   // Use DOM API to focus the node directly with cursor positioning
  //   setTimeout(() => {
  //     const nodeElement = document.querySelector(
  //       `[data-node-id="${nodeId}"] [contenteditable]`
  //     ) as HTMLElement;
  //     if (nodeElement) {
  //       nodeElement.focus();
  //
  //       // Set cursor position using tree walker (same approach as controller)
  //       if (position >= 0) {
  //         const selection = window.getSelection();
  //         if (!selection) return;
  //
  //         // Use tree walker to find the correct text node and offset
  //         const walker = document.createTreeWalker(nodeElement, NodeFilter.SHOW_TEXT, null);
  //
  //         let currentOffset = 0;
  //         let currentNode;
  //
  //         while ((currentNode = walker.nextNode())) {
  //           const nodeLength = currentNode.textContent?.length || 0;
  //
  //           if (currentOffset + nodeLength >= position) {
  //             const range = document.createRange();
  //             const offsetInNode = position - currentOffset;
  //             range.setStart(currentNode, Math.min(offsetInNode, nodeLength));
  //             range.setEnd(currentNode, Math.min(offsetInNode, nodeLength));
  //
  //             selection.removeAllRanges();
  //             selection.addRange(range);
  //             return;
  //           }
  //
  //           currentOffset += nodeLength;
  //         }
  //
  //         // If we didn't find the position, position at the end
  //         const range = document.createRange();
  //         range.selectNodeContents(nodeElement);
  //         range.collapse(false);
  //         selection.removeAllRanges();
  //         selection.addRange(range);
  //       }
  //     } else {
  //       console.error(`Could not find contenteditable element for node ${nodeId}`);
  //     }
  //   }, 10);
  // }

  /**
   * Add appropriate formatting syntax to content based on node type
   * Used when creating new nodes from splits to preserve formatting
   *
   * NOTE: Header syntax inheritance is now handled in createNode() in reactiveNodeService.svelte.ts
   * to avoid duplication and ensure consistent behavior.
   */
  function addFormattingSyntax(content: string): string {
    // Header syntax inheritance is now handled in the createNode function
    // to ensure consistent behavior and avoid duplication

    // Return content as-is if no formatting needed
    if (!content) return content;

    // For task nodes: no automatic syntax addition
    // Task checkbox syntax ([ ]) is only added when users type it as a shortcut
    // Splitting a task node preserves the visual task state but not the syntax

    // For other node types, return as-is
    return content;
  }

  // Handle creating new nodes when Enter is pressed
  function handleCreateNewNode(
    event: CustomEvent<{
      afterNodeId: string;
      nodeType: string;
      currentContent?: string;
      newContent?: string;
      originalContent?: string;
      inheritHeaderLevel?: number;
      cursorAtBeginning?: boolean;
      insertAtBeginning?: boolean;
      focusOriginalNode?: boolean;
      newNodeCursorPosition?: number;
    }>
  ) {
    const {
      afterNodeId,
      nodeType,
      currentContent,
      newContent,
      originalContent,
      inheritHeaderLevel,
      insertAtBeginning,
      focusOriginalNode,
      newNodeCursorPosition
    } = event.detail;

    // Validate node creation parameters
    if (!afterNodeId || !nodeType) {
      console.error('Invalid node creation parameters:', { afterNodeId, nodeType });
      return;
    }

    // Verify the target node exists
    if (!nodeManager.nodes.has(afterNodeId)) {
      console.error('Target node does not exist:', afterNodeId);
      return;
    }

    // Update current node content if provided and actually changed
    if (currentContent !== undefined) {
      const existingNode = nodeManager.findNode(afterNodeId);
      if (existingNode && existingNode.content !== currentContent) {
        // Use updateNodeContent for node splitting - with new reactive architecture no forcing needed
        nodeManager.updateNodeContent(afterNodeId, currentContent);
      }
    }

    // Create new node using NodeManager - placeholder if empty, real if has content
    let newNodeId: string;

    // CRITICAL FIX: Use afterNode's parentId as the explicit parent
    // The viewer's nodeId represents the viewer's display context (e.g., date node)
    // but the actual parent is stored in the node's parentId field
    // After indent, the node object's parentId is updated to reflect the new parent
    const afterNode = nodeManager.findNode(afterNodeId);
    const explicitParentId = afterNode?.parentId ?? nodeId ?? null;

    // Add formatting syntax to the new content based on node type and header level
    // (applies to both empty and non-empty content for header inheritance)
    const formattedNewContent = addFormattingSyntax(newContent || '');

    // IMPORTANT: Enter key ALWAYS creates real persisted nodes (even if blank)
    // Only the first viewer-local placeholder uses the placeholder->promotion cycle
    // All subsequent nodes created via Enter are persisted immediately
    newNodeId = nodeManager.createNode(
      afterNodeId,
      formattedNewContent,
      nodeType,
      inheritHeaderLevel,
      insertAtBeginning || false,
      originalContent,
      !focusOriginalNode, // Focus new node when creating splits, original node when creating above
      paneId,
      false, // isInitialPlaceholder (Enter key never creates initial placeholders)
      explicitParentId // Pass viewer's nodeId as parent (e.g., date node for date viewers)
    );

    // Validate that node creation succeeded
    if (!newNodeId || !nodeManager.nodes.has(newNodeId)) {
      console.error('Node creation failed for afterNodeId:', afterNodeId, 'newNodeId:', newNodeId);
      return;
    }

    // Set cursor position using FocusManager (single source of truth)
    if (newNodeCursorPosition !== undefined && !focusOriginalNode) {
      focusManager.setEditingNode(newNodeId, paneId, newNodeCursorPosition);
    }

    // Handle focus direction based on focusOriginalNode parameter
    if (focusOriginalNode) {
      // The hierarchy is correct (new node above, original below)
      // Use the nodeManager's update methods to properly trigger reactivity

      // Use updateNodeContent on original node to trigger focus
      const originalNode = nodeManager.nodes.get(afterNodeId);
      if (originalNode) {
        // Update the original node's content to itself, which should trigger focus
        nodeManager.updateNodeContent(afterNodeId, originalNode.content);
      }
    }

    // Handle HTML formatting conversion if needed
    if (newContent && newContent.includes('<span class="markdown-')) {
      setTimeout(() => {
        const markdownContent = htmlToMarkdown(newContent);
        nodeManager.updateNodeContent(newNodeId, markdownContent);
      }, 100);
    }
  }

  // Handle indenting nodes (Tab key)
  async function handleIndentNode(event: CustomEvent<{ nodeId: string }>) {
    const { nodeId } = event.detail;

    try {
      // Validate node exists before indenting
      if (!nodeManager.nodes.has(nodeId)) {
        console.error('Cannot indent non-existent node:', nodeId);
        return;
      }

      // Store cursor position before DOM changes
      const cursorPosition = saveCursorPosition(nodeId);

      // Use NodeManager to handle indentation
      const success = await nodeManager.indentNode(nodeId);

      if (success) {
        // NodeManager.indentNode() already persists via updateNode()
        // No need for separate saveHierarchyChange() call (was causing double-write)

        // Restore cursor position after DOM update
        setTimeout(() => restoreCursorPosition(nodeId, cursorPosition), 0);
      }
    } catch (error) {
      console.error('Error during node indentation:', error);
    }
  }

  // Cursor position utilities
  function saveCursorPosition(nodeId: string): number {
    const element = document.getElementById(`contenteditable-${nodeId}`);
    if (!element) return 0;

    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return 0;

    const range = selection.getRangeAt(0);
    const preCaretRange = range.cloneRange();
    preCaretRange.selectNodeContents(element);
    preCaretRange.setEnd(range.startContainer, range.startOffset);

    return preCaretRange.toString().length;
  }

  function restoreCursorPosition(nodeId: string, position: number) {
    const element = document.getElementById(`contenteditable-${nodeId}`);
    if (!element) return;

    try {
      const selection = window.getSelection();
      if (!selection) return;

      const range = document.createRange();
      const textNodes = getTextNodes(element);

      let currentOffset = 0;
      for (const textNode of textNodes) {
        const nodeLength = textNode.textContent?.length || 0;
        if (currentOffset + nodeLength >= position) {
          range.setStart(textNode, Math.max(0, position - currentOffset));
          range.collapse(true);
          selection.removeAllRanges();
          selection.addRange(range);
          element.focus();
          return;
        }
        currentOffset += nodeLength;
      }

      // If we couldn\'t find the exact position, place cursor at end
      if (textNodes.length > 0) {
        const lastNode = textNodes[textNodes.length - 1];
        range.setStart(lastNode, lastNode.textContent?.length || 0);
        range.collapse(true);
        selection.removeAllRanges();
        selection.addRange(range);
        element.focus();
      }
    } catch {
      // Silently handle cursor restoration errors
    }
  }

  function getTextNodes(element: Element): Text[] {
    const textNodes: Text[] = [];
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, null);

    let node = walker.nextNode();
    while (node) {
      textNodes.push(node as Text);
      node = walker.nextNode();
    }

    return textNodes;
  }

  // Handle outdenting nodes (Shift+Tab key)
  async function handleOutdentNode(event: CustomEvent<{ nodeId: string }>) {
    const { nodeId } = event.detail;

    try {
      // Validate node exists before outdenting
      if (!nodeManager.nodes.has(nodeId)) {
        console.error('Cannot outdent non-existent node:', nodeId);
        return;
      }

      // Store cursor position before DOM changes
      const cursorPosition = saveCursorPosition(nodeId);

      // Use NodeManager to handle outdentation
      const success = await nodeManager.outdentNode(nodeId);

      if (success) {
        // NodeManager.outdentNode() already persists via updateNode()
        // No need for separate saveHierarchyChange() calls (was causing double-write)
        // Both the outdented node and transferred siblings are persisted automatically

        // Restore cursor position after DOM update
        setTimeout(() => restoreCursorPosition(nodeId, cursorPosition), 0);
      }
    } catch (error) {
      console.error('Error during node outdentation:', error);
    }
  }

  // Handle chevron click to toggle expand/collapse
  function handleToggleExpanded(nodeId: string) {
    // Get the currently focused element before DOM changes
    const activeElement = document.activeElement as HTMLElement;
    const isTextEditor = activeElement && activeElement.id?.startsWith('contenteditable-');
    let focusedNodeId: string | null = null;
    let cursorPosition = 0;

    // Store cursor position if we have an active text editor
    if (isTextEditor) {
      focusedNodeId = activeElement.id.replace('contenteditable-', '');
      cursorPosition = saveCursorPosition(focusedNodeId);
    }

    // Perform the toggle operation
    nodeManager.toggleExpanded(nodeId);

    // Restore focus and cursor position after DOM update
    if (focusedNodeId && isTextEditor) {
      setTimeout(() => {
        const element = document.getElementById(`contenteditable-${focusedNodeId}`);
        if (element && document.body.contains(element)) {
          restoreCursorPosition(focusedNodeId, cursorPosition);
        }
      }, 0);
    }
  }

  /**
   * Navigate to a target node using FocusManager (reactive approach)
   * Passes arrow navigation context to FocusManager for pixel-accurate positioning
   *
   * @param targetNodeId The node to navigate to
   * @param direction Navigation direction ('up' or 'down')
   * @param pixelOffset Horizontal pixel offset to maintain
   */
  function handleNavigateToNode(
    targetNodeId: string,
    direction: 'up' | 'down',
    pixelOffset: number
  ): void {
    // Find the target node
    const targetNode = nodeManager.findNode(targetNodeId);
    if (!targetNode) {
      console.warn(`[Navigation] Target node ${targetNodeId} not found`);
      return;
    }

    // Use FocusManager with arrow navigation context
    // This triggers reactive effects that handle:
    // 1. Switching from view mode to edit mode (isEditing derived value)
    // 2. Focusing the textarea (autoFocus effect in base-node.svelte)
    // 3. Calling controller.enterFromArrowNavigation() with pixel-accurate positioning
    focusManager.setEditingNodeFromArrowNavigation(targetNodeId, direction, pixelOffset, paneId);
  }

  // Handle arrow key navigation between nodes using entry/exit methods
  function handleArrowNavigation(
    event: CustomEvent<{
      nodeId: string;
      direction: 'up' | 'down';
      pixelOffset: number;
    }>
  ) {
    const { nodeId: eventNodeId, direction, pixelOffset } = event.detail;

    // Get visible nodes from NodeManager
    const currentVisibleNodes = nodeManager.visibleNodes(nodeId);
    const currentIndex = currentVisibleNodes.findIndex((n) => n.id === eventNodeId);

    if (currentIndex === -1) return;

    // Find next navigable node that accepts navigation
    let targetIndex = direction === 'up' ? currentIndex - 1 : currentIndex + 1;

    while (targetIndex >= 0 && targetIndex < currentVisibleNodes.length) {
      const candidateNode = currentVisibleNodes[targetIndex];

      // Check if this node accepts navigation (skip if it doesn't)
      // For now, assume all nodes accept navigation (will be refined per node type)
      const acceptsNavigation = true; // candidateNode.navigationMethods?.canAcceptNavigation() ?? true;

      if (acceptsNavigation) {
        // Navigate using reactive approach (FocusManager)
        handleNavigateToNode(candidateNode.id, direction, pixelOffset);
        return;
      }

      // This node doesn't accept navigation - try next one
      targetIndex = direction === 'up' ? targetIndex - 1 : targetIndex + 1;
    }
  }

  // Handle combining current node with previous node (Backspace at start of node)
  // CLEAN DELEGATION: All logic handled by NodeManager
  async function handleCombineWithPrevious(
    event: CustomEvent<{ nodeId: string; currentContent: string }>
  ) {
    try {
      const { nodeId: eventNodeId } = event.detail;

      // Validate node exists before combining
      if (!nodeManager.nodes.has(eventNodeId)) {
        console.error('Cannot combine non-existent node:', eventNodeId);
        return;
      }

      const currentVisibleNodes = nodeManager.visibleNodes(nodeId);
      const currentIndex = currentVisibleNodes.findIndex((n) => n.id === eventNodeId);

      if (currentIndex <= 0) {
        return; // No previous node to combine with
      }

      const previousNode = currentVisibleNodes[currentIndex - 1];

      if (!previousNode || !nodeManager.nodes.has(previousNode.id)) {
        console.error('Previous node not found or invalid:', previousNode?.id);
        return;
      }

      // Prevent merging into structured nodes (code-block, quote-block)
      // These nodes have specific formatting that can't accept arbitrary content
      if (isStructuredNode(previousNode.nodeType)) {
        return; // Silently prevent merge - user can still delete current node if empty
      }

      // Store the original content length before merge (this is where cursor should be positioned)
      const cursorPositionAfterMerge = previousNode.content.length;

      // Always use combineNodes (handles both empty and non-empty nodes with proper child promotion)
      nodeManager.combineNodes(eventNodeId, previousNode.id, paneId);

      // Always request focus at the merge point (end of original previous node content)
      // Use setTimeout to ensure DOM has updated after the merge operation
      // This ensures:
      // 1. Cursor is positioned at the merge point (not at beginning)
      // 2. Textarea updates to show merged content immediately (via forced update)
      // 3. Consistent behavior for both empty and non-empty node merges
      setTimeout(() => {
        requestNodeFocus(previousNode.id, cursorPositionAfterMerge);
      }, 0);
    } catch (error) {
      console.error('Error during node combination:', error);
    }
  }

  // Handle deleting empty node (Backspace at start of empty node)
  async function handleDeleteNode(event: CustomEvent<{ nodeId: string }>) {
    try {
      const { nodeId: eventNodeId } = event.detail;

      // Validate node exists before deletion
      if (!nodeManager.nodes.has(eventNodeId)) {
        console.error('Cannot delete non-existent node:', eventNodeId);
        return;
      }

      const currentVisibleNodes = nodeManager.visibleNodes(nodeId);
      const currentIndex = currentVisibleNodes.findIndex((n) => n.id === eventNodeId);

      if (currentIndex <= 0) return; // No previous node to focus

      const previousNode = currentVisibleNodes[currentIndex - 1];

      if (!previousNode || !nodeManager.nodes.has(previousNode.id)) {
        console.error('Previous node not found for focus after deletion:', previousNode?.id);
        // Can't combine without previous node - this shouldn't happen in normal usage
        return;
      }

      // Prevent merging into structured nodes (code-block, quote-block)
      // These nodes have specific formatting that can't accept arbitrary content
      if (isStructuredNode(previousNode.nodeType)) {
        // Block the action entirely - don't delete, don't merge, don't focus
        // User must manually delete the node (e.g., Cmd+Backspace) or add content first
        return;
      }

      // Use combineNodes even for empty nodes (handles child promotion properly)
      nodeManager.combineNodes(eventNodeId, previousNode.id, paneId);
      requestNodeFocus(previousNode.id, previousNode.content.length);
    } catch (error) {
      console.error('Error during node deletion:', error);
    }
  }

  // Handle icon click events
  // Note: Node-specific components handle their own icon behavior (e.g., TaskNode manages task states)
  // This handler is for any viewer-level icon click coordination if needed in the future
  function handleIconClick(
    _event: CustomEvent<{ nodeId: string; nodeType: string; currentState?: string }>
  ) {
    // Currently a no-op - individual node components handle their own icon clicks
    // This makes the system extensible for future node types that need viewer-level coordination
  }

  // Helper functions removed - NodeManager handles all node operations

  // Simple reactive access - let template handle reactivity directly

  // Dynamic component loading - create stable component mapping for both viewers and nodes
  let loadedViewers = $state(new Map<string, unknown>());
  // Proper Svelte 5 reactivity: use object instead of Map for reactive tracking
  let loadedNodes = $state<Record<string, unknown>>({});

  // Derive the list of nodes to render - either viewer placeholder or real children
  const nodesToRender = $derived(() => {
    const realChildren = nodeManager.visibleNodes(nodeId);

    // If we have real children, render those
    if (realChildren.length > 0) {
      return realChildren;
    }

    // If we have a viewer placeholder, render it (with no parent, viewer-local only)
    if (viewerPlaceholder) {
      // Convert placeholder to renderable format with UI state
      return [
        {
          ...viewerPlaceholder,
          depth: 0,
          children: [],
          expanded: false,
          autoFocus: true,
          inheritHeaderLevel: 0,
          isPlaceholder: true
        }
      ];
    }

    // No children and no placeholder
    return [];
  });

  // Reactive effect: Create placeholder when children drop to zero
  $effect(() => {
    const realChildren = nodeManager.visibleNodes(nodeId);

    // If we have no real children and no placeholder, create one
    if (realChildren.length === 0 && !viewerPlaceholder && nodeId) {
      const placeholderId = globalThis.crypto.randomUUID();
      viewerPlaceholder = {
        id: placeholderId,
        nodeType: 'text',
        content: '',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      // DON'T call initializeNodes() - keep placeholder completely viewer-local!
      // Issue #479: Placeholder should NOT be in SharedNodeStore
      // It will be rendered by nodesToRender derived state (line 1475-1487)
      // and promoted to real node when user adds content (line 1746-1772)
    }
    // Clear placeholder when real children exist
    else if (realChildren.length > 0 && viewerPlaceholder) {
      viewerPlaceholder = null;
    }
  });

  // Calculate minimum depth for relative positioning
  // Children of a container node should start at depth 0 in the viewer
  const minDepth = $derived(() => {
    const nodes = nodesToRender();
    if (nodes.length === 0) return 0;
    return Math.min(...nodes.map((n) => n.depth || 0));
  });

  // Pre-load components when component mounts
  onMount(async () => {
    // NOTE: Viewer registration with NodeExpansionCoordinator moved to loadChildrenForParent()
    // This ensures nodes are loaded BEFORE attempting to restore expansion states
    // Otherwise, all nodes are skipped as "missing" during restoration

    async function preloadComponents() {
      // Pre-load all known types
      const knownTypes = ['text', 'header', 'date', 'task', 'ai-chat'];

      for (const nodeType of knownTypes) {
        // Load viewers
        if (!loadedViewers.has(nodeType)) {
          try {
            const customViewer = await pluginRegistry.getViewer(nodeType);
            if (customViewer) {
              loadedViewers.set(nodeType, customViewer);
            } else {
              // Fallback to BaseNode for unknown types
              loadedViewers.set(nodeType, BaseNode);
            }
          } catch {
            loadedViewers.set(nodeType, BaseNode);
          }
        }

        // Load node components
        if (!(nodeType in loadedNodes)) {
          try {
            const customNode = await pluginRegistry.getNodeComponent(nodeType);
            if (customNode) {
              loadedNodes[nodeType] = customNode;
            } else {
              // Fallback to BaseNode for unknown types
              loadedNodes[nodeType] = BaseNode;
            }
          } catch (error) {
            console.error(`💥 Error loading node component for ${nodeType}:`, error);
            loadedNodes[nodeType] = BaseNode;
          }
        }
      }
    }

    await preloadComponents();
  });

  // Scroll position management: Save scroll position when user scrolls
  $effect(() => {
    if (!scrollContainer) return;

    const handleScroll = () => {
      if (scrollContainer) {
        saveScrollPosition(viewerId, scrollContainer.scrollTop);
      }
    };

    scrollContainer.addEventListener('scroll', handleScroll, { passive: true });

    return () => {
      if (scrollContainer) {
        scrollContainer.removeEventListener('scroll', handleScroll);
      }
    };
  });

  // Scroll position restoration: Restore when viewer becomes active or mounts
  $effect(() => {
    // Restore scroll position when the scroll container is available
    if (scrollContainer && viewerId) {
      const savedPosition = getScrollPosition(viewerId);
      // Capture current container reference to prevent race conditions
      const currentContainer = scrollContainer;
      // Use requestAnimationFrame to ensure DOM is ready
      requestAnimationFrame(() => {
        // Only restore if container hasn't changed (prevents stale updates)
        if (scrollContainer === currentContainer) {
          scrollContainer.scrollTop = savedPosition;
        }
      });
    }
  });

  // Reactively load components when node types change
  $effect(() => {
    const visibleNodes = nodesToRender();

    // Collect all unique node types
    const nodeTypes = new Set(visibleNodes.map((node) => node.nodeType));

    // Load components for any new node types
    for (const nodeType of nodeTypes) {
      if (!(nodeType in loadedNodes)) {
        // Load asynchronously
        (async () => {
          try {
            const customNode = await pluginRegistry.getNodeComponent(nodeType);
            if (customNode) {
              loadedNodes[nodeType] = customNode;
            } else {
              loadedNodes[nodeType] = BaseNode;
            }
          } catch (error) {
            console.error(`💥 Error loading node component for ${nodeType}:`, error);
            loadedNodes[nodeType] = BaseNode;
          }
        })();
      }
    }
  });

  // Clean up on component unmount and flush pending saves
  onDestroy(() => {
    // Unregister this viewer from the expansion coordinator
    NodeExpansionCoordinator.unregisterViewer(tabId);

    // CRITICAL: Commit ALL active batches globally BEFORE flushing
    // This ensures node type conversions (which use batches) are saved
    // We must commit globally because visible nodes might be empty if viewer already unmounted
    console.log('[BaseNodeViewer] onDestroy: Committing all active batches globally');
    sharedNodeStore.commitAllBatches();

    // Flush pending debounced saves to prevent data loss
    PersistenceCoordinator.getInstance().flushPending();

    // Set cancellation flag to prevent stale writes
    isDestroyed = true;

    // Clear pending promise tracking to prevent memory leaks
    pendingContentSavePromises.clear();
    pendingStructuralUpdatesPromise = null;
  });
</script>

<!-- Base Node Viewer: Header + Scrollable Children Area -->
<div class="base-node-viewer">
  <!-- Header Section - Default editable header or custom snippet -->
  {#if header}
    <!-- Custom header provided via snippet (e.g., DateNodeViewer's date navigation) -->
    <div class="viewer-header">
      {@render header()}
    </div>
  {:else}
    <!-- Default editable header (no custom snippet provided) -->
    <div class="viewer-editable-header">
      <input
        type="text"
        class="header-input"
        value={headerContent}
        oninput={(e) => handleHeaderInput(e.currentTarget.value)}
        placeholder="Untitled"
        aria-label="Page title"
      />
    </div>
  {/if}

  <!-- Scrollable Node Content Area (children structure) -->
  <div class="node-content-area" bind:this={scrollContainer}>
    <!-- Schema-Driven Properties Panel - appears after header, before children -->
    {#if currentViewedNode && nodeId}
      <SchemaPropertyForm {nodeId} nodeType={currentViewedNode.nodeType} />
    {/if}

    {#each nodesToRender() as node (node.id)}
      {@const relativeDepth = (node.depth || 0) - minDepth()}
      <div
        class="node-container"
        data-has-children={node.children?.length > 0}
        style="margin-left: {relativeDepth * 2.5}rem"
      >
        <div class="node-content-wrapper">
          <!-- Chevron for parent nodes using design system approach -->
          {#if node.children && node.children.length > 0}
            <button
              class="chevron-icon"
              class:expanded={node.expanded}
              onclick={() => handleToggleExpanded(node.id)}
              onkeydown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  handleToggleExpanded(node.id);
                }
              }}
              aria-label={node.expanded ? 'Collapse node' : 'Expand node'}
              aria-expanded={node.expanded}
            >
              <svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg">
                <path d="M6 3l5 5-5 5-1-1 4-4-4-4 1-1z" />
              </svg>
            </button>
          {/if}

          <!-- Node viewer with stable component references - all nodes use plugin registry -->
          {#if node.nodeType in loadedNodes}
            {#key node.id}
              {@const NodeComponent = loadedNodes[node.nodeType] as typeof BaseNode}
              {@const nodeMetadata = extractNodeMetadata(node)}
              <NodeComponent
                nodeId={node.id}
                nodeType={node.nodeType}
                autoFocus={node.autoFocus}
                content={node.content}
                children={node.children}
                metadata={nodeMetadata}
                editableConfig={{ allowMultiline: true }}
                on:createNewNode={handleCreateNewNode}
                on:indentNode={handleIndentNode}
                on:outdentNode={handleOutdentNode}
                on:navigateArrow={handleArrowNavigation}
                on:contentChanged={(e: CustomEvent<{ content: string }>) => {
                  const content = e.detail.content;

                  // Check if this is the viewer-local placeholder getting its first content
                  if (
                    viewerPlaceholder &&
                    node.id === viewerPlaceholder.id &&
                    content.trim() !== '' &&
                    nodeId
                  ) {
                    // Promote placeholder to real node by assigning parent and adding to store
                    const promotedNode = promotePlaceholderToNode(viewerPlaceholder, nodeId, {
                      content
                    });

                    // Add to shared store (in-memory only, don't persist yet)
                    // Subsequent typing will trigger updateNodeContent() which handles persistence
                    sharedNodeStore.setNode(promotedNode, { type: 'viewer', viewerId }, true);

                    // CRITICAL FIX (Issue #528): Update children cache to establish parent-child relationship
                    // After graph migration (PR #523), parent relationships are stored in:
                    // 1. Backend: SurrealDB has_child edges
                    // 2. Frontend: SharedNodeStore children/parents cache
                    // Without this, the promoted node is orphaned and disappears from the UI
                    registerChildWithParent(nodeId, promotedNode.id);

                    // Clear viewer placeholder - now using real node from store
                    viewerPlaceholder = null;

                    // No need to reload - promoted node is already in shared store
                    // Database query won't find it yet (CREATE is debounced)
                  } else {
                    // Regular node content update (placeholder flag is handled automatically)
                    nodeManager.updateNodeContent(node.id, content);
                  }
                  // Focus management handled by FocusManager (single source of truth)
                }}
                on:nodeTypeChanged={(
                  e: CustomEvent<{
                    nodeType: string;
                    cleanedContent?: string;
                    cursorPosition?: number;
                  }>
                ) => {
                  const newNodeType = e.detail.nodeType;
                  const cleanedContent = e.detail.cleanedContent;
                  // Use cursor position from event (captured by TextareaController)
                  const cursorPosition = e.detail.cursorPosition ?? 0;

                  // CRITICAL: Set editing state BEFORE updating node type
                  // This ensures focus manager state is ready when the new component mounts
                  focusManager.setEditingNodeFromTypeConversion(node.id, cursorPosition, paneId);

                  // Update content if cleanedContent is provided (e.g., from contentTemplate)
                  if (cleanedContent !== undefined) {
                    nodeManager.updateNodeContent(node.id, cleanedContent);
                  }

                  // Update node type through proper API (triggers component re-render)
                  // Uses immediate persistence to ensure type change is saved right away
                  nodeManager.updateNodeType(node.id, newNodeType);
                }}
                on:slashCommandSelected={(
                  e: CustomEvent<{ command: string; nodeType: string; cursorPosition?: number }>
                ) => {
                  // Use cursor position from event (captured by TextareaController)
                  const cursorPosition = e.detail.cursorPosition ?? 0;

                  // CRITICAL: Set editing state BEFORE updating node type
                  // This ensures focus manager state is ready when the new component mounts
                  focusManager.setEditingNodeFromTypeConversion(node.id, cursorPosition, paneId);

                  console.log('[BaseNodeViewer] slashCommandSelected:', {
                    nodeId: node.id,
                    newType: e.detail.nodeType,
                    isPlaceholder: node.isPlaceholder,
                    hasViewerPlaceholder: !!viewerPlaceholder
                  });

                  // CRITICAL FIX: Treat slash commands on placeholders as real node type changes
                  // They must persist to database, not just update locally
                  // Use same batching logic as real nodes to ensure atomic persistence
                  if (
                    node.isPlaceholder &&
                    nodeId &&
                    viewerPlaceholder &&
                    node.id === viewerPlaceholder.id
                  ) {
                    console.log(
                      '[BaseNodeViewer] Promoting placeholder to real node with type:',
                      e.detail.nodeType
                    );
                    // Promote placeholder to real node with the new type
                    const promotedNode = promotePlaceholderToNode(viewerPlaceholder, nodeId, {
                      content: node.content || '',
                      nodeType: e.detail.nodeType
                    });

                    // Add to store and trigger persistence
                    sharedNodeStore.setNode(promotedNode, { type: 'viewer', viewerId }, false);

                    // CRITICAL FIX (Issue #528): Update children cache to establish parent-child relationship
                    registerChildWithParent(nodeId, promotedNode.id);

                    viewerPlaceholder = null;
                  } else {
                    console.log('[BaseNodeViewer] Updating node type for real node');
                    // For real nodes, update node type with full persistence
                    nodeManager.updateNodeType(node.id, e.detail.nodeType);
                  }
                }}
                on:iconClick={handleIconClick}
                on:taskStateChanged={(e) => {
                  const { nodeId, state } = e.detail;

                  // Map UI state to schema enum value
                  let schemaStatus: string;
                  switch (state) {
                    case 'pending':
                      schemaStatus = 'OPEN';
                      break;
                    case 'inProgress':
                      schemaStatus = 'IN_PROGRESS';
                      break;
                    case 'completed':
                      schemaStatus = 'DONE';
                      break;
                    default:
                      schemaStatus = 'OPEN';
                  }

                  // Update using schema-aware helper (handles nested format correctly)
                  updateSchemaField(nodeId, 'status', schemaStatus);
                }}
                on:combineWithPrevious={handleCombineWithPrevious}
                on:deleteNode={handleDeleteNode}
              />
            {/key}
          {:else}
            <!-- Final fallback to BaseNode with key for re-rendering -->
            {#key `${node.id}-${node.nodeType}`}
              <BaseNode
                nodeId={node.id}
                nodeType={node.nodeType}
                autoFocus={node.autoFocus}
                content={node.content}
                children={node.children}
                metadata={node.properties || {}}
                editableConfig={{ allowMultiline: true }}
                on:createNewNode={handleCreateNewNode}
                on:indentNode={handleIndentNode}
                on:outdentNode={handleOutdentNode}
                on:navigateArrow={handleArrowNavigation}
                on:contentChanged={(e: CustomEvent<{ content: string }>) => {
                  const content = e.detail.content;

                  // Update node content (placeholder flag is handled automatically)
                  nodeManager.updateNodeContent(node.id, content);
                }}
                on:slashCommandSelected={(
                  e: CustomEvent<{ command: string; nodeType: string; cursorPosition?: number }>
                ) => {
                  // Use cursor position from event (captured by TextareaController)
                  const cursorPosition = e.detail.cursorPosition ?? 0;

                  // CRITICAL: Set editing state BEFORE updating node type
                  // This ensures focus manager state is ready when the new component mounts
                  focusManager.setEditingNodeFromTypeConversion(node.id, cursorPosition, paneId);

                  console.log('[BaseNodeViewer] slashCommandSelected:', {
                    nodeId: node.id,
                    newType: e.detail.nodeType,
                    isPlaceholder: node.isPlaceholder,
                    hasViewerPlaceholder: !!viewerPlaceholder
                  });

                  // CRITICAL FIX: Treat slash commands on placeholders as real node type changes
                  // They must persist to database, not just update locally
                  // Use same batching logic as real nodes to ensure atomic persistence
                  if (
                    node.isPlaceholder &&
                    nodeId &&
                    viewerPlaceholder &&
                    node.id === viewerPlaceholder.id
                  ) {
                    console.log(
                      '[BaseNodeViewer] Promoting placeholder to real node with type:',
                      e.detail.nodeType
                    );
                    // Promote placeholder to real node with the new type
                    const promotedNode = promotePlaceholderToNode(viewerPlaceholder, nodeId, {
                      content: node.content || '',
                      nodeType: e.detail.nodeType
                    });

                    // Add to store and trigger persistence
                    sharedNodeStore.setNode(promotedNode, { type: 'viewer', viewerId }, false);

                    // CRITICAL FIX (Issue #528): Update children cache to establish parent-child relationship
                    registerChildWithParent(nodeId, promotedNode.id);

                    viewerPlaceholder = null;
                  } else {
                    console.log('[BaseNodeViewer] Updating node type for real node');
                    // For real nodes, update node type with full persistence
                    nodeManager.updateNodeType(node.id, e.detail.nodeType);
                  }
                }}
                on:iconClick={handleIconClick}
                on:taskStateChanged={(e) => {
                  const { nodeId, state } = e.detail;

                  // Map UI state to schema enum value
                  let schemaStatus: string;
                  switch (state) {
                    case 'pending':
                      schemaStatus = 'OPEN';
                      break;
                    case 'inProgress':
                      schemaStatus = 'IN_PROGRESS';
                      break;
                    case 'completed':
                      schemaStatus = 'DONE';
                      break;
                    default:
                      schemaStatus = 'OPEN';
                  }

                  // Update using schema-aware helper (handles nested format correctly)
                  updateSchemaField(nodeId, 'status', schemaStatus);
                }}
                on:combineWithPrevious={handleCombineWithPrevious}
                on:deleteNode={handleDeleteNode}
              />
            {/key}
          {/if}
        </div>
      </div>
    {/each}

    <!-- Backlinks Panel - fixed at bottom of this viewer -->
    {#if nodeId}
      <BacklinksPanel {nodeId} />
    {/if}
  </div>
</div>

<!-- Template structure fixed -->

<style>
  /* Base container - full height layout */
  .base-node-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    width: 100%;
  }

  /* Default editable header section - borderless design */
  .viewer-editable-header {
    flex-shrink: 0;
    padding: 1rem;
    border-bottom: 1px solid hsl(var(--border));
    background: hsl(var(--background));
  }

  .header-input {
    width: 100%;
    font-size: 2rem;
    font-weight: 500;
    color: hsl(var(--muted-foreground));
    background: transparent;
    border: none;
    outline: none;
    padding: 0;
    margin: 0;
    font-family: inherit;
  }

  .header-input::placeholder {
    color: hsl(var(--muted-foreground) / 0.5);
  }

  /* Custom header section - fixed at top, doesn't scroll */
  .viewer-header {
    flex-shrink: 0;
    padding: 1rem;
    border-bottom: 1px solid hsl(var(--border));
    background: hsl(var(--background));
  }

  /* Default header content styling - large, prominent titles */
  .viewer-header :global(h1) {
    font-size: 2rem;
    font-weight: 500;
    color: hsl(var(--muted-foreground));
    margin: 0;
  }

  /* Scrollable node content area for children structure */
  .node-content-area {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    min-height: 0; /* Critical for flex scrolling */
    padding: var(--viewer-padding-vertical) var(--viewer-padding-horizontal);
    padding-bottom: var(
      --viewer-padding-bottom
    ); /* Reduced padding - backlinks panel is now sticky within this container */
    display: flex;
    flex-direction: column;
    gap: 0; /* 0px gap - all spacing from node padding for 8px total */

    /* Autohide scrollbar - only show when scrolling */
    scrollbar-width: thin; /* Firefox */
    scrollbar-color: transparent transparent; /* Firefox - hidden by default */
  }

  /* Show scrollbar on hover or while scrolling */
  .node-content-area:hover,
  .node-content-area:active {
    scrollbar-color: hsl(var(--muted-foreground) / 0.3) transparent; /* Firefox */
  }

  /* WebKit (Chrome, Safari, Edge) scrollbar styling */
  .node-content-area::-webkit-scrollbar {
    width: 8px;
  }

  .node-content-area::-webkit-scrollbar-track {
    background: transparent;
  }

  .node-content-area::-webkit-scrollbar-thumb {
    background: transparent;
    border-radius: 4px;
  }

  /* Show scrollbar on hover or while scrolling */
  .node-content-area:hover::-webkit-scrollbar-thumb,
  .node-content-area:active::-webkit-scrollbar-thumb {
    background: hsl(var(--muted-foreground) / 0.3);
  }

  .node-content-area::-webkit-scrollbar-thumb:hover {
    background: hsl(var(--muted-foreground) / 0.5);
  }

  .base-node-viewer {
    /* Dynamic Circle Positioning System - All values configurable from here */
    --circle-offset: 22px; /* Circle center distance from container left edge - reserves space for chevrons */
    --circle-diameter: 20px; /* Circle size (width and height) */
    --circle-text-gap: 8px; /* Gap between circle edge and text content */
    --node-indent: 2.5rem; /* Indentation distance between parent and child levels */

    /* Default font values for positioning calculations */
    --font-size: 1rem;
    --line-height: 1.6;
    /* Note: --icon-vertical-position is defined globally in app.css */

    /* NodeSpace Extension Colors - Subtle Tint System (Scheme 3) */
    --node-text: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
    --node-task: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
    --node-ai-chat: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
    --node-entity: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
    --node-query: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
  }

  .node-container {
    /* Individual node wrapper - no additional spacing */
    /* Allow chevrons to extend outside container bounds */
    overflow: visible;
  }

  .node-content-wrapper {
    /* Wrapper for chevron + content */
    display: flex;
    align-items: flex-start;
    gap: 0.25rem; /* 4px gap between chevron/spacer and text content */
    position: relative; /* Enable absolute positioning for chevrons */
    width: 100%; /* Ensure wrapper fills parent so flex children can inherit */
  }

  /* Flex children (node wrappers) should fill available space */
  /* Use :global() to apply across component boundaries (TaskNode, CodeBlock, etc. have different scopes) */
  .node-content-wrapper > :global(:not(.chevron-icon):not(.chevron-spacer)) {
    flex: 1;
    min-width: 0; /* Allow flex item to shrink below content size if needed */
  }

  /* 
    Chevron positioning system - matches circle positioning exactly
    
    POSITIONING FORMULA:
    The chevron must be vertically centered with the circles, which use:
    top: calc(0.25rem + (var(--font-size) * var(--line-height) / 2))
    
    Where:
    - 0.25rem = container top padding (.node has padding: 0.25rem)
    - line-height-px = font-size × line-height multiplier
    - This formula positions at the visual center of the first line of text
    
    HORIZONTAL POSITION:
    - Exactly halfway between parent and child circles
    - Parent is at current depth, child is at depth + 2.5rem (--node-indent)
    - Chevron positioned at -1.25rem (half of 2.5rem) from current node
    
    INHERITANCE:
    The --line-height-px variable is inherited from .node-content-wrapper
    which detects the header level of nested content using :has() selector
  */
  .chevron-icon {
    opacity: 0; /* Hidden by default - shows on hover */
    background: none;
    border: none;
    padding: 0.125rem; /* 2px padding for clickable area */
    cursor: pointer;
    border-radius: 0.125rem; /* 2px border radius */
    transition: opacity 0.15s ease-in-out; /* Smooth fade in/out */
    pointer-events: auto; /* Ensure chevron always receives pointer events */
    flex-shrink: 0;
    width: 1.25rem; /* Fixed 20px to match circle size */
    height: 1.25rem; /* Fixed 20px to match circle size */
    display: flex;
    align-items: center;
    justify-content: center;
    /* Position chevron exactly halfway between parent and child circles */
    position: absolute;
    left: calc(
      -1 * var(--node-indent) / 2 + var(--circle-offset)
    ); /* Halfway back to parent + parent circle offset */
    /* Use shared CSS variable from .node - single source of truth for vertical positioning */
    top: var(--icon-vertical-position);
    transform: translate(-50%, -50%); /* Center icon on coordinates, same as circles */
    z-index: 999; /* Very high z-index to ensure clickability over all other elements */
  }

  .chevron-icon svg {
    width: 16px;
    height: 16px;
    fill: hsl(var(--node-text) / 0.5);
    transition: fill 0.15s ease;
  }

  .chevron-icon:focus {
    outline: 2px solid hsl(var(--ring));
    outline-offset: 2px;
    opacity: 1; /* Always visible when focused */
  }

  .chevron-icon:hover svg {
    fill: hsl(var(--node-text) / 0.5);
  }

  /* Show chevron only when hovering directly over this node's content wrapper (not child nodes) */
  .node-content-wrapper:hover > .chevron-icon {
    opacity: 1;
  }

  /* Expanded state: rotate 90 degrees to point down */
  .chevron-icon.expanded {
    transform: translate(-50%, -50%) rotate(90deg);
  }

  /* CSS-first positioning to match base-node.svelte implementation */
  .node-content-wrapper {
    /* Default values for normal text - adjusted for better circle alignment */
    --line-height: 1.875;
    --font-size: 1rem;
  }

  /* Inherit font-size, line-height, and icon positioning from HeaderNode wrapper classes (Issue #311) */
  .node-content-wrapper:has(:global(.header-h1)) {
    --font-size: 2rem;
    --line-height: 1.2;
    --icon-vertical-position: calc(0.25rem + (2rem * 1.2 / 2));
  }

  .node-content-wrapper:has(:global(.header-h2)) {
    --font-size: 1.5rem;
    --line-height: 1.3;
    --icon-vertical-position: calc(0.25rem + (1.5rem * 1.3 / 2));
  }

  .node-content-wrapper:has(:global(.header-h3)) {
    --font-size: 1.25rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (1.25rem * 1.4 / 2));
  }

  .node-content-wrapper:has(:global(.header-h4)) {
    --font-size: 1.125rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (1.125rem * 1.4 / 2));
  }

  .node-content-wrapper:has(:global(.header-h5)) {
    --font-size: 1rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (1rem * 1.4 / 2));
  }

  .node-content-wrapper:has(:global(.header-h6)) {
    --font-size: 0.875rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (0.875rem * 1.4 / 2));
  }

  /* Reset ordered list counter at viewer level */
  .base-node-viewer {
    counter-reset: ordered-list-counter;
  }

  /* Also reset when ordered list sequence is broken by non-list nodes */
  /* Using data attribute for semantic clarity and maintainability */
  .base-node-viewer > *:not([data-node-type='ordered-list']) {
    counter-reset: ordered-list-counter;
  }
</style>
