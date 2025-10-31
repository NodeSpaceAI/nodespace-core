<!--
  BaseNodeViewer - Container that manages a collection of nodes
  Handles node creation, deletion, and organization
  
  Now uses NodeServiceContext to provide @ autocomplete functionality
  to all TextNode components automatically via proper inheritance.
-->

<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { htmlToMarkdown } from '$lib/utils/markdown.js';
  import { pluginRegistry } from '$lib/components/viewers/index';
  import BaseNode from '$lib/design/components/base-node.svelte';
  import BacklinksPanel from '$lib/design/components/backlinks-panel.svelte';
  import { getNodeServices } from '$lib/contexts/node-service-context.svelte';
  import { sharedNodeStore } from '$lib/services/shared-node-store';
  import { PersistenceCoordinator } from '$lib/services/persistence-coordinator.svelte';
  import { focusManager } from '$lib/services/focus-manager.svelte';
  import type { Node } from '$lib/types';
  import type { UpdateSource } from '$lib/types/update-protocol';
  import type { Snippet } from 'svelte';

  // Props
  let {
    header,
    nodeId = null,
    onTitleChange
  }: {
    header?: Snippet;
    nodeId?: string | null;
    onTitleChange?: (_title: string) => void;
    onNodeIdChange?: (_nodeId: string) => void; // In type for interface, not used by BaseNodeViewer
  } = $props();

  // Editable header state
  let headerContent = $state('');

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

  // Track last saved content to detect actual changes
  const lastSavedContent = new Map<string, string>();

  // Cancellation flag to prevent database writes after component unmounts
  let isDestroyed = false;

  // Loading flag to prevent watchers from firing during initial load
  // Start as true to prevent watcher from firing before loadChildrenForParent() completes
  let isLoadingInitialNodes = true;

  // Set view context and load children when nodeId changes
  $effect(() => {
    nodeManager.setViewParentId(nodeId);

    if (nodeId) {
      loadChildrenForParent(nodeId);

      // Load header content from node
      const node = sharedNodeStore.getNode(nodeId);
      if (node) {
        headerContent = node.content || '';
      }

      // Set tab title to node content (first line)
      if (onTitleChange) {
        const firstLine = headerContent.split('\n')[0].trim();
        const title = firstLine.length > 40 ? firstLine.substring(0, 37) + '...' : firstLine;
        onTitleChange(title || 'Untitled');
      }
    }
  });

  // Update tab title when header content changes
  $effect(() => {
    if (onTitleChange && headerContent !== undefined) {
      const firstLine = headerContent.split('\n')[0].trim();
      const title = firstLine.length > 40 ? firstLine.substring(0, 37) + '...' : firstLine;
      onTitleChange(title || 'Untitled');
    }
  });

  /**
   * Handle header content changes
   * Updates both the local state and the node's content in the database
   */
  function handleHeaderInput(newValue: string) {
    // Update local state
    headerContent = newValue;

    // Update node content in database if nodeId exists
    if (nodeId) {
      sharedNodeStore.updateNode(nodeId, { content: newValue }, VIEWER_SOURCE);
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
      parentId: string | null;
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

    const nodes = nodeManager.visibleNodes;

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
              parentId: node.parentId || nodeId,
              containerNodeId: node.containerNodeId || nodeId!,
              beforeSiblingId: node.beforeSiblingId,
              createdAt: node.createdAt || new Date().toISOString(),
              modifiedAt: new Date().toISOString(),
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

    const currentNodeIds = new Set(nodeManager.visibleNodes.map((n) => n.id));

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
  // IMPORTANT: saveNodeWithParent() saves BOTH content AND structure
  // This means new nodes don't need a separate structural update - they're already persisted
  // This watcher only handles structural changes to EXISTING nodes (indent, outdent, reorder)
  // ============================================================================
  // previousStructure is updated in three places (all necessary):
  // 1. Deletion watcher (line ~218): Cleans up beforeSiblingId references to deleted nodes
  // 2. This watcher (line ~388): Tracks unchanged nodes to prevent redundant updates
  // 3. This watcher (line ~490): Tracks successfully persisted structural changes (source of truth)
  let previousStructure = new Map<
    string,
    { parentId: string | null; beforeSiblingId: string | null }
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
    newStructure: { parentId: string | null; beforeSiblingId: string | null }
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

    const visibleNodes = nodeManager.visibleNodes;

    // Collect all structural changes first
    const updates: Array<{
      nodeId: string;
      parentId: string | null;
      beforeSiblingId: string | null;
    }> = [];

    for (const node of visibleNodes) {
      // Skip placeholder nodes
      if (node.isPlaceholder) {
        continue;
      }

      const currentStructure = {
        parentId: node.parentId,
        beforeSiblingId: node.beforeSiblingId
      };

      const prevStructure = previousStructure.get(node.id);

      if (
        prevStructure &&
        (prevStructure.parentId !== currentStructure.parentId ||
          prevStructure.beforeSiblingId !== currentStructure.beforeSiblingId)
      ) {
        // Structural change detected - queue for persistence
        updates.push({
          nodeId: node.id,
          parentId: node.parentId,
          beforeSiblingId: node.beforeSiblingId
        });
      } else {
        // No change detected OR new node - update tracking to current state
        // New nodes have their structure saved via saveNodeWithParent() in content save watcher
        // So we can safely track them here to avoid redundant structural updates
        trackStructureChange(node.id, currentStructure);
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
          if (update.parentId) nodeIdsToWaitFor.push(update.parentId);
          if (update.beforeSiblingId) nodeIdsToWaitFor.push(update.beforeSiblingId);
        }

        const failedNodeIds = await waitForNodeSavesIfPending(nodeIdsToWaitFor);

        // Filter out updates that reference failed nodes
        const validUpdates = updates.filter(
          (update) =>
            !failedNodeIds.has(update.nodeId) &&
            (!update.parentId || !failedNodeIds.has(update.parentId)) &&
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
          trackStructureChange(update.nodeId, {
            parentId: update.parentId,
            beforeSiblingId: update.beforeSiblingId
          });
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
                  parentId: lastGoodState.parentId,
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

  async function loadChildrenForParent(nodeId: string) {
    try {
      // Set loading flag to prevent watchers from triggering during initial load
      isLoadingInitialNodes = true;

      // Clear content tracking BEFORE loading to prevent watcher from firing on stale data
      lastSavedContent.clear();

      const allNodes = await sharedNodeStore.loadChildrenForParent(nodeId);

      // Check if we have any nodes at all
      if (allNodes.length === 0) {
        // Check if placeholder already exists (reuse for multi-tab support)
        const existingNodes = sharedNodeStore.getNodesForParent(nodeId);

        if (existingNodes.length === 0) {
          // No placeholder exists - create one
          const placeholderId = globalThis.crypto.randomUUID();
          const placeholder: Node = {
            id: placeholderId,
            nodeType: 'text',
            content: '',
            parentId: nodeId,
            containerNodeId: nodeId,
            beforeSiblingId: null,
            createdAt: new Date().toISOString(),
            modifiedAt: new Date().toISOString(),
            properties: {},
            mentions: []
          };

          // Initialize with placeholder
          // initializeNodes() will auto-detect this is a placeholder and use viewer source
          nodeManager.initializeNodes([placeholder], {
            expanded: true,
            autoFocus: true,
            inheritHeaderLevel: 0
          });
        } else {
          // Placeholder exists - initialize UI with existing placeholder
          // initializeNodes() will auto-detect placeholders and use viewer source
          nodeManager.initializeNodes(existingNodes, {
            expanded: true,
            autoFocus: true,
            inheritHeaderLevel: 0
          });
        }
      } else {
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

  /**
   * Save hierarchy changes (parent_id, before_sibling_id) after indent/outdent operations
   * Updates immediately without debouncing since these are explicit user actions
   *
   * Delegates to SharedNodeStore for persistence.
   * Skips placeholder nodes - they should not be persisted yet
   */
  async function saveHierarchyChange(childNodeId: string) {
    if (!nodeId) return;

    try {
      const node = nodeManager.findNode(childNodeId);
      if (!node) {
        console.error('[BaseNodeViewer] Cannot save hierarchy - node not found:', childNodeId);
        return;
      }

      // Check if node is a placeholder by looking at visibleNodes which includes UI state
      const visibleNode = nodeManager.visibleNodes.find((n) => n.id === childNodeId);
      const isPlaceholder = visibleNode?.isPlaceholder || false;

      // Skip placeholder nodes - they should not be persisted yet
      if (isPlaceholder) {
        return;
      }

      // Update node with structural changes (was saveNodeImmediately)
      const fullNode: Node = {
        id: childNodeId,
        nodeType: node.nodeType,
        content: node.content,
        parentId: node.parentId || nodeId,
        containerNodeId: node.containerNodeId || nodeId,
        beforeSiblingId: node.beforeSiblingId,
        createdAt: node.createdAt || new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: node.properties || {},
        mentions: node.mentions || []
      };
      sharedNodeStore.setNode(fullNode, VIEWER_SOURCE);
    } catch (error) {
      console.error('[BaseNodeViewer] Failed to save hierarchy change:', childNodeId, error);
    }
  }

  // Focus handling function with proper cursor positioning using tree walker
  function requestNodeFocus(nodeId: string, position: number) {
    // Use FocusManager as single source of truth for focus management
    // This replaces the old DOM-based focus approach
    focusManager.setEditingNode(nodeId, position);

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

    // Update current node content if provided
    if (currentContent !== undefined) {
      // Use updateNodeContent for node splitting - with new reactive architecture no forcing needed
      nodeManager.updateNodeContent(afterNodeId, currentContent);
    }

    // Create new node using NodeManager - placeholder if empty, real if has content
    let newNodeId: string;

    if (!newContent || newContent.trim() === '') {
      // Create placeholder node for empty content (Enter key without splitting)
      newNodeId = nodeManager.createPlaceholderNode(
        afterNodeId,
        nodeType,
        inheritHeaderLevel,
        insertAtBeginning || false,
        originalContent,
        !focusOriginalNode // Focus new node when creating splits, original node when creating above
      );
    } else {
      // Create real node when splitting existing content
      // Add formatting syntax to the new content based on node type and header level
      const formattedNewContent = addFormattingSyntax(newContent);

      newNodeId = nodeManager.createNode(
        afterNodeId,
        formattedNewContent,
        nodeType,
        inheritHeaderLevel,
        insertAtBeginning || false,
        originalContent,
        !focusOriginalNode // Focus new node when creating splits, original node when creating above
      );
    }

    // Validate that node creation succeeded
    if (!newNodeId || !nodeManager.nodes.has(newNodeId)) {
      console.error('Node creation failed for afterNodeId:', afterNodeId, 'newNodeId:', newNodeId);
      return;
    }

    // Set cursor position using FocusManager (single source of truth)
    if (newNodeCursorPosition !== undefined && !focusOriginalNode) {
      focusManager.setEditingNode(newNodeId, newNodeCursorPosition);
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
      const success = nodeManager.indentNode(nodeId);

      if (success) {
        // Persist hierarchy change - AWAIT to ensure it completes
        await saveHierarchyChange(nodeId);

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

      // Get existing children of the node BEFORE outdenting
      const childrenBefore = Array.from(nodeManager.nodes.values())
        .filter((n) => n.parentId === nodeId)
        .map((n) => n.id);

      // Use NodeManager to handle outdentation
      const success = nodeManager.outdentNode(nodeId);

      if (success) {
        // Get children of outdented node AFTER outdenting (includes transferred siblings)
        const childrenAfter = Array.from(nodeManager.nodes.values())
          .filter((n) => n.parentId === nodeId)
          .map((n) => n.id);

        // Find the transferred siblings (nodes that are now children but weren't before)
        const transferredSiblings = childrenAfter.filter((id) => !childrenBefore.includes(id));

        // Persist hierarchy change for the outdented node
        await saveHierarchyChange(nodeId);

        // Persist hierarchy changes for all transferred siblings
        for (const siblingId of transferredSiblings) {
          await saveHierarchyChange(siblingId);
        }

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
    focusManager.setEditingNodeFromArrowNavigation(targetNodeId, direction, pixelOffset);
  }

  // Handle arrow key navigation between nodes using entry/exit methods
  function handleArrowNavigation(
    event: CustomEvent<{
      nodeId: string;
      direction: 'up' | 'down';
      pixelOffset: number;
    }>
  ) {
    const { nodeId, direction, pixelOffset } = event.detail;

    // Get visible nodes from NodeManager
    const currentVisibleNodes = nodeManager.visibleNodes;
    const currentIndex = currentVisibleNodes.findIndex((n) => n.id === nodeId);

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
      const { nodeId } = event.detail;

      // Validate node exists before combining
      if (!nodeManager.nodes.has(nodeId)) {
        console.error('Cannot combine non-existent node:', nodeId);
        return;
      }

      const currentVisibleNodes = nodeManager.visibleNodes;
      const currentIndex = currentVisibleNodes.findIndex((n) => n.id === nodeId);

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
      nodeManager.combineNodes(nodeId, previousNode.id);

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
      const { nodeId } = event.detail;

      // Validate node exists before deletion
      if (!nodeManager.nodes.has(nodeId)) {
        console.error('Cannot delete non-existent node:', nodeId);
        return;
      }

      const currentVisibleNodes = nodeManager.visibleNodes;
      const currentIndex = currentVisibleNodes.findIndex((n) => n.id === nodeId);

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
      nodeManager.combineNodes(nodeId, previousNode.id);
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

  // Calculate minimum depth for relative positioning
  // Children of a container node should start at depth 0 in the viewer
  const minDepth = $derived(() => {
    const nodes = nodeManager.visibleNodes;
    if (nodes.length === 0) return 0;
    return Math.min(...nodes.map((n) => n.depth || 0));
  });

  // Pre-load components when component mounts
  onMount(async () => {
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

  // Reactively load components when node types change
  $effect(() => {
    const visibleNodes = nodeManager.visibleNodes;

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
  <!-- Editable Header Section (always visible) -->
  <div class="viewer-editable-header">
    <input
      type="text"
      class="header-input"
      bind:value={headerContent}
      oninput={(e) => handleHeaderInput(e.currentTarget.value)}
      placeholder="Untitled"
      aria-label="Page title"
    />
  </div>

  <!-- Custom Header Section (can be customized via snippet) -->
  {#if header}
    <div class="viewer-header">
      {@render header()}
    </div>
  {/if}

  <!-- Scrollable Node Content Area (children structure) -->
  <div class="node-content-area">
    {#each nodeManager.visibleNodes as node (node.id)}
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
              <NodeComponent
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
                  focusManager.setEditingNodeFromTypeConversion(node.id, cursorPosition);

                  // ATOMIC UPDATE: Use batch to ensure content + nodeType persist together
                  // This prevents race conditions where content persists before nodeType changes
                  // Timeout auto-resets on each keystroke - only commits after true inactivity
                  // Batch will auto-commit after timeout (default 2s) or when user navigates away
                  sharedNodeStore.startBatch(node.id);

                  // Update content if cleanedContent is provided (e.g., from contentTemplate)
                  if (cleanedContent !== undefined) {
                    nodeManager.updateNodeContent(node.id, cleanedContent);
                  }

                  // Update node type through proper API (triggers component re-render)
                  nodeManager.updateNodeType(node.id, newNodeType);

                  // Don't commit immediately - let user finish typing
                  // Batch will auto-commit after timeout or can be manually committed later
                }}
                on:slashCommandSelected={(
                  e: CustomEvent<{ command: string; nodeType: string; cursorPosition?: number }>
                ) => {
                  // Use cursor position from event (captured by TextareaController)
                  const cursorPosition = e.detail.cursorPosition ?? 0;

                  // CRITICAL: Set editing state BEFORE updating node type
                  // This ensures focus manager state is ready when the new component mounts
                  focusManager.setEditingNodeFromTypeConversion(node.id, cursorPosition);

                  if (node.isPlaceholder) {
                    // For placeholder nodes, just update the nodeType locally
                    if ('updatePlaceholderNodeType' in nodeManager) {
                      (nodeManager as any).updatePlaceholderNodeType(node.id, e.detail.nodeType);
                    }
                  } else {
                    // For real nodes, update node type with full persistence
                    nodeManager.updateNodeType(node.id, e.detail.nodeType);
                  }
                }}
                on:iconClick={handleIconClick}
                on:taskStateChanged={(e) => {
                  const { nodeId, state } = e.detail;
                  const node = nodeManager.nodes.get(nodeId);
                  if (node) {
                    node.properties = { ...node.properties, taskState: state };
                    // Trigger sync to persist the change
                    nodeManager.updateNodeContent(nodeId, node.content);
                  }
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
                  focusManager.setEditingNodeFromTypeConversion(node.id, cursorPosition);

                  if (node.isPlaceholder) {
                    // For placeholder nodes, just update the nodeType locally
                    if ('updatePlaceholderNodeType' in nodeManager) {
                      (nodeManager as any).updatePlaceholderNodeType(node.id, e.detail.nodeType);
                    }
                  } else {
                    // For real nodes, update node type with full persistence
                    nodeManager.updateNodeType(node.id, e.detail.nodeType);
                  }
                }}
                on:iconClick={handleIconClick}
                on:taskStateChanged={(e) => {
                  const { nodeId, state } = e.detail;
                  const node = nodeManager.nodes.get(nodeId);
                  if (node) {
                    node.properties = { ...node.properties, taskState: state };
                    // Trigger sync to persist the change
                    nodeManager.updateNodeContent(nodeId, node.content);
                  }
                }}
                on:combineWithPrevious={handleCombineWithPrevious}
                on:deleteNode={handleDeleteNode}
              />
            {/key}
          {/if}
        </div>
      </div>
    {/each}

    <!-- Backlinks Panel - shows which containers mention this node -->
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

  /* Editable header section - always visible, borderless design */
  .viewer-editable-header {
    flex-shrink: 0;
    padding: 1rem;
    padding-bottom: 0.5rem;
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
    padding: 1.5rem;
    padding-bottom: 3rem; /* Extra bottom padding to see last line fully */
    display: flex;
    flex-direction: column;
    gap: 0; /* 0px gap - all spacing from node padding for 8px total */

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
