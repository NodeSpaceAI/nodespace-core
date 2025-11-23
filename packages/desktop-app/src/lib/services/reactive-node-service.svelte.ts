/**
 * ReactiveNodeService - Svelte 5 Runes Adapter for SharedNodeStore
 *
 * REFACTORED FOR MULTI-VIEWER SUPPORT (Phase 1.2):
 * - Delegates node data storage to SharedNodeStore (single source of truth)
 * - Maintains per-viewer UI state (expand/collapse, focus, depth)
 * - Provides Svelte 5 reactivity via $state and $derived
 * - Backward compatible - existing API preserved
 *
 * Architecture:
 * - SharedNodeStore: Stores all node data (content, hierarchy, metadata)
 * - ReactiveNodeService: Per-viewer instance with UI-specific state
 * - Subscriptions: Automatically updates when SharedNodeStore changes
 *
 * Key features:
 * - Uses Node from $lib/types (with node_type, properties, etc.)
 * - Separates data (Node in SharedNodeStore) from UI state (NodeUIState)
 * - Computes children on-demand instead of storing arrays
 * - Real-time synchronization across multiple viewers
 */

import { v4 as uuidv4 } from 'uuid';
import { ContentProcessor } from './content-processor';
import { SharedNodeStore } from './shared-node-store';
import { getFocusManager } from './focus-manager.svelte';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import type { Node, NodeUIState } from '$lib/types';
import { createDefaultUIState } from '$lib/types';
import type { UpdateSource } from '$lib/types/update-protocol';
import { DEFAULT_PANE_ID } from '$lib/stores/navigation';
import { schemaService } from './schema-service';
import { moveNode as moveNodeCommand } from './tauri-commands';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';


export interface NodeManagerEvents {
  focusRequested: (nodeId: string, position?: number) => void;
  hierarchyChanged: () => void;
  nodeCreated: (nodeId: string) => void;
  nodeDeleted: (nodeId: string) => void;
}

export function createReactiveNodeService(events: NodeManagerEvents) {
  // ADAPTER PATTERN: Delegate data storage to SharedNodeStore
  // This service now focuses on per-viewer UI state and Svelte reactivity

  // Generate unique viewer ID for this instance
  const viewerId = uuidv4();

  // Update source for this viewer
  const viewerSource: UpdateSource = {
    type: 'viewer',
    viewerId
  };

  // Focus management - single source of truth
  const focusManager = getFocusManager();

  // Get SharedNodeStore instance dynamically (important for tests that call resetInstance())
  const sharedNodeStore = SharedNodeStore.getInstance();

  // UI state only - node data stored in SharedNodeStore
  const _uiState = $state<Record<string, NodeUIState>>({});
  let _rootNodeIds = $state<string[]>([]);
  const _activeNodeId = $state<string | undefined>(undefined);

  // Manual reactivity trigger - incremented when SharedNodeStore updates
  let _updateTrigger = $state(0);

  const contentProcessor = ContentProcessor.getInstance();

  // Subscribe to SharedNodeStore changes for reactive updates
  // Wildcard subscription - updates _updateTrigger when any node changes
  // Note: Unsubscribe handled automatically when service instance is garbage collected
  const _unsubscribe = sharedNodeStore.subscribeAll((node) => {
    // CRITICAL: Initialize UI state for new nodes (e.g., promoted placeholders)
    // If UI state doesn't exist, compute depth and create default state
    if (!_uiState[node.id]) {
      // Compute depth using backend hierarchy queries
      const computeDepth = (nodeId: string, visited = new Set<string>()): number => {
        if (visited.has(nodeId)) return 0; // Prevent infinite recursion
        visited.add(nodeId);

        const parents = sharedNodeStore.getParentsForNode(nodeId);
        if (parents.length === 0) return 0;
        return 1 + computeDepth(parents[0].id, visited);
      };

      const depth = computeDepth(node.id);
      _uiState[node.id] = createDefaultUIState(node.id, { depth });
    }

    _updateTrigger++;
  });

  // Prevent unused variable warning (cleanup would happen here if needed)
  void _unsubscribe;

  // NOTE: Backend now returns children pre-sorted via fractional ordering (ORDER BY order ASC)
  // No frontend sorting needed - we trust the backend's ordering

  function getVisibleNodesRecursive(nodeIds: string[]): (Node & {
    depth: number;
    children: string[];
    expanded: boolean;
    autoFocus: boolean;
    inheritHeaderLevel: number;
    isPlaceholder: boolean;
  })[] {
    const result: (Node & {
      depth: number;
      children: string[];
      expanded: boolean;
      autoFocus: boolean;
      inheritHeaderLevel: number;
      isPlaceholder: boolean;
    })[] = [];

    for (const nodeId of nodeIds) {
      const node = sharedNodeStore.getNode(nodeId);
      const uiState = _uiState[nodeId];
      if (node) {
        // Get children from SharedNodeStore (already sorted by backend via fractional ordering)
        const children = sharedNodeStore.getNodesForParent(nodeId).map((n) => n.id);

        // Derive autoFocus from FocusManager (single source of truth)
        const autoFocus = focusManager.editingNodeId === nodeId;

        // Merge Node with UI state for components
        result.push({
          ...node,
          depth: uiState?.depth || 0,
          children,
          expanded: uiState?.expanded || false,
          autoFocus,
          inheritHeaderLevel: uiState?.inheritHeaderLevel || 0,
          isPlaceholder: uiState?.isPlaceholder || false
        });

        if (uiState?.expanded && children.length > 0) {
          result.push(...getVisibleNodesRecursive(children));
        }
      }
    }

    return result;
  }

  /**
   * Computes visible nodes for a specific parent ID
   * @param viewParentId - The parent node ID to get children for (null = root nodes)
   * @returns Sorted, hierarchical array of visible nodes
   */
  function getVisibleNodesForParent(viewParentId: string | null): (Node & {
    depth: number;
    children: string[];
    expanded: boolean;
    autoFocus: boolean;
    inheritHeaderLevel: number;
    isPlaceholder: boolean;
  })[] {
    void _updateTrigger; // Still track updates for reactivity
    void _rootNodeIds; // Track root node changes

    let viewRoots: string[];
    if (viewParentId !== null) {
      // Get children from SharedNodeStore (already sorted by backend via fractional ordering)
      // NOTE: Parent may not exist yet (e.g., virtual date nodes) - this is OK
      viewRoots = sharedNodeStore.getNodesForParent(viewParentId).map((n) => n.id);
    } else {
      viewRoots = _rootNodeIds;
    }

    return getVisibleNodesRecursive(viewRoots);
  }

  function findNode(nodeId: string): Node | null {
    return sharedNodeStore.getNode(nodeId) || null;
  }

  // DEPRECATED: clearAllAutoFocus() removed
  // Focus is now managed by FocusManager (single source of truth)
  // Use focusManager.setEditingNode(nodeId) or focusManager.clearEditing() instead

  function createNode(
    afterNodeId: string,
    content: string = '',
    nodeType: string = 'text',
    headerLevel?: number,
    insertAtBeginning?: boolean,
    originalNodeContent?: string,
    focusNewNode?: boolean,
    paneId: string = DEFAULT_PANE_ID,
    isInitialPlaceholder: boolean = false,
    parentId?: string | null // Accept parent ID as parameter (not stored in node)
  ): string {
    const afterNode = findNode(afterNodeId);
    if (!afterNode) {
      return '';
    }

    const nodeId = uuidv4();

    // Focus management now handled by FocusManager (single source of truth)
    // No need to clear autoFocus flags - derived from focusManager.editingNodeId
    const afterUIState = _uiState[afterNodeId] || createDefaultUIState(afterNodeId);
    const newDepth = afterUIState.depth;

    // Determine parent from parameter or use afterNode's parentId directly
    // CRITICAL FIX: Don't rely on broken parentsCache - use the node's own parentId field
    let newParentId: string | null;
    if (parentId !== undefined) {
      newParentId = parentId;
    } else if (afterNode.parentId !== undefined) {
      // Use afterNode's parentId directly - this is the authoritative source
      newParentId = afterNode.parentId ?? null;
    } else {
      // Fallback: try cache (for compatibility, though it may be null)
      const parents = sharedNodeStore.getParentsForNode(afterNodeId);
      newParentId = parents.length > 0 ? parents[0].id : null;
    }

    let initialContent = content;
    const sourceContent = originalNodeContent || afterNode.content;

    if (content.trim() === '') {
      const headerMatch = sourceContent.match(/^(#{1,6}\s+)/);
      if (headerMatch) {
        initialContent = headerMatch[1];
      }
    } else {
      const sourceHeaderMatch = sourceContent.match(/^(#{1,6}\s+)/);
      const contentHeaderMatch = content.match(/^(#{1,6}\s+)/);

      if (sourceHeaderMatch && !contentHeaderMatch) {
        initialContent = sourceHeaderMatch[1] + content;
      }
    }

    const shouldFocusNewNode = focusNewNode !== undefined ? focusNewNode : !insertAtBeginning;
    const isPlaceholder = initialContent.trim() === '' || /^#{1,6}\s*$/.test(initialContent.trim());

    // Create Node with unified type system
    // NOTE: beforeSiblingId removed - backend uses fractional ordering on edges
    const newNode: Node = {
      id: nodeId,
      nodeType: nodeType,
      content: initialContent,
      createdAt: new Date().toISOString(),
      parentId: newParentId,
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {},
      embeddingVector: null,
      mentions: []
    };

    // Create UI state (autoFocus removed - now derived from FocusManager)
    const newUIState = createDefaultUIState(nodeId, {
      depth: newDepth,
      expanded: true,
      inheritHeaderLevel: headerLevel !== undefined ? headerLevel : afterUIState.inheritHeaderLevel,
      isPlaceholder
    });

    // Skip persistence ONLY for initial viewer placeholder (when no children exist)
    // All other nodes (including blank nodes created during editing) persist immediately
    // This prevents UNIQUE constraint violations when indenting blank nodes
    const skipPersistence = isInitialPlaceholder;
    sharedNodeStore.setNode(newNode, viewerSource, skipPersistence);
    _uiState[nodeId] = newUIState;

    // CRITICAL FIX: Register parent-child edge in ReactiveStructureTree for browser mode
    // Set focus using FocusManager (single source of truth)
    // This replaces manual autoFocus flag manipulation
    if (shouldFocusNewNode) {
      focusManager.setEditingNode(nodeId, paneId);
    }

    // NOTE: Sibling linked list updates removed - backend handles ordering via fractional ordering

    // Bug 4 fix: Transfer children from expanded nodes
    // When creating a node from a parent with children, transfer those children to the new node
    // This matches pre-migration behavior: new node appears between parent and children
    const children = sharedNodeStore.getNodesForParent(afterNodeId);
    const structureChildren = structureTree.getChildren(afterNodeId);

    console.log('[createNode] Child transfer check:', {
      afterNodeId,
      insertAtBeginning,
      sharedNodeStoreChildren: children.length,
      structureTreeChildren: structureChildren.length,
      children: children.map(c => c.id),
      structureIds: structureChildren
    });

    // IMPORTANT: Always transfer children when not inserting at beginning,
    // regardless of expanded state (expanded state is UI-only, doesn't affect structure)
    if (!insertAtBeginning && children.length > 0) {
        // OPTIMISTIC UI: Update structure tree IMMEDIATELY for instant visual feedback
        // This ensures the UI shows the correct hierarchy without waiting for database
        for (const child of children) {
          structureTree.moveInMemoryRelationship(afterNodeId, nodeId, child.id);
        }

        // BACKGROUND PERSISTENCE: Database sync with error handling and rollback
        // Database updates happen asynchronously without blocking UI
        Promise.resolve().then(async () => {
          try {
            // Wait for newNode to be persisted to database
            // This prevents FOREIGN KEY constraint violations when children reference the new parent
            await sharedNodeStore.waitForNodeSaves([nodeId]);

            // Persist child transfers to database
            // Structure tree already updated above, so UI is already correct
            for (const child of children) {
              // Import moveNode dynamically to avoid circular dependency
              const { moveNode } = await import('$lib/services/tauri-commands');
              // Move child to be under the new node in database
              await moveNode(child.id, nodeId);
              // Note: No need to call structureTree.moveInMemoryRelationship() again
              // It was already done synchronously above for instant UI update
            }
          } catch (error) {
            // ROLLBACK: Revert optimistic UI changes on failure
            console.error('[createNode] Failed to transfer children to database, rolling back:', error);
            for (const child of children) {
              // Move children back to original parent in structure tree
              structureTree.moveInMemoryRelationship(nodeId, afterNodeId, child.id);
            }
            // TODO: Emit error event for UI notification (toast/banner)
            // events.operationFailed?.('transfer_children', error);
          }
        });
    }

    // Handle hierarchy positioning
    // NOTE: Now using backend queries instead of afterNode.parentId
    const afterNodeParents = sharedNodeStore.getParentsForNode(afterNodeId);
    const afterNodeIsRoot = afterNodeParents.length === 0;

    if (insertAtBeginning) {
      if (!afterNodeIsRoot) {
        // Update sibling pointers (backend handles parent relationships)
      } else {
        const afterNodeIndex = _rootNodeIds.indexOf(afterNodeId);
        _rootNodeIds = [
          ..._rootNodeIds.slice(0, afterNodeIndex),
          nodeId,
          ..._rootNodeIds.slice(afterNodeIndex)
        ];
      }
    } else {
      if (!afterNodeIsRoot) {
        // Insert after sibling (backend handles parent relationships)
      } else {
        const afterNodeIndex = _rootNodeIds.indexOf(afterNodeId);
        _rootNodeIds = [
          ..._rootNodeIds.slice(0, afterNodeIndex + 1),
          nodeId,
          ..._rootNodeIds.slice(afterNodeIndex + 1)
        ];
        _updateTrigger++;
      }
    }

    // NOTE: Sorted children cache removed - backend provides pre-sorted children

    events.nodeCreated(nodeId);
    events.hierarchyChanged();

    // If we're not focusing the new node, keep focus on the original node
    if (!shouldFocusNewNode && afterNode) {
      focusManager.setEditingNode(afterNodeId, paneId);
    }

    return nodeId;
  }

  function createPlaceholderNode(
    afterNodeId: string,
    nodeType: string = 'text',
    headerLevel?: number,
    insertAtBeginning: boolean = false,
    originalNodeContent?: string,
    focusNewNode?: boolean,
    paneId: string = DEFAULT_PANE_ID,
    isInitialPlaceholder: boolean = false,
    parentId?: string | null // Accept parent ID as parameter
  ): string {
    return createNode(
      afterNodeId,
      '',
      nodeType,
      headerLevel,
      insertAtBeginning,
      originalNodeContent,
      focusNewNode,
      paneId,
      isInitialPlaceholder,
      parentId // Forward parent ID to createNode
    );
  }

  const debouncedOperations = new Map<
    string,
    {
      fastTimer?: ReturnType<typeof setTimeout>;
      expensiveTimer?: ReturnType<typeof setTimeout>;
      pendingContent?: string;
    }
  >();

  function updateNodeContent(nodeId: string, content: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    const headerLevel = contentProcessor.parseHeaderLevel(content);
    const isPlaceholder = content.trim() === '';

    // Issue #424 FIX: Always include nodeType to preserve slash command conversions
    // Pattern conversions work correctly because they call updateNodeType() separately
    // BEFORE the content update, so both the pattern-detected type and content persist together
    sharedNodeStore.updateNode(nodeId, { content, nodeType: node.nodeType }, viewerSource);

    _uiState[nodeId] = {
      ..._uiState[nodeId],
      isPlaceholder,
      inheritHeaderLevel: headerLevel
    };

    scheduleContentProcessing(nodeId, content);
  }

  function updateNodeType(nodeId: string, nodeType: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    // CRITICAL: Include content with nodeType update to ensure backend persistence works
    // Some backends may not support updating nodeType alone
    const updatePayload: Partial<Node> = { nodeType, content: node.content };

    // Issue #427: Apply schema defaults when converting node types
    // Extract defaults from the new node type's schema and merge with existing properties
    // Using synchronous extractDefaults() which uses the schema cache for immediate results
    try {
      const schemaDefaults = schemaService.extractDefaults(nodeType);

      // Only update properties if there are defaults to apply
      if (Object.keys(schemaDefaults).length > 0) {
        // Deep merge defaults with existing properties (don't overwrite user data)
        // For each namespace (e.g., 'task'), merge the nested objects
        const mergedProperties = { ...node.properties };

        for (const [namespace, defaultFields] of Object.entries(schemaDefaults)) {
          if (typeof defaultFields === 'object' && defaultFields !== null) {
            mergedProperties[namespace] = {
              ...(defaultFields as Record<string, unknown>),
              ...(node.properties[namespace] as Record<string, unknown> | undefined)
            };
          }
        }

        updatePayload.properties = mergedProperties;
      }
    } catch (error) {
      // If schema extraction fails, just proceed without defaults (graceful degradation)
      console.warn(`[updateNodeType] Failed to extract schema defaults for ${nodeType}:`, error);
    }

    // Skip conflict detection for nodeType changes - they are always intentional conversions
    sharedNodeStore.updateNode(nodeId, updatePayload, viewerSource, {
      skipConflictDetection: true
    });

    scheduleContentProcessing(nodeId, node.content);
  }

  function updateNodeMentions(nodeId: string, mentions: string[]): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    // Mentions are computed fields - skip persistence (they're derived from content)
    sharedNodeStore.updateNode(nodeId, { mentions: [...mentions] }, viewerSource, {
      skipPersistence: true
    });

    _updateTrigger++;
  }

  function updateNodeProperties(
    nodeId: string,
    properties: Record<string, unknown>,
    merge: boolean = true
  ): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    sharedNodeStore.updateNode(
      nodeId,
      { properties: merge ? { ...node.properties, ...properties } : { ...properties } },
      viewerSource
    );

    _updateTrigger++;
  }

  function scheduleContentProcessing(nodeId: string, content: string): void {
    const existing = debouncedOperations.get(nodeId);
    if (existing?.fastTimer) clearTimeout(existing.fastTimer);
    if (existing?.expensiveTimer) clearTimeout(existing.expensiveTimer);

    debouncedOperations.set(nodeId, { pendingContent: content });

    const fastTimer = setTimeout(() => {
      processFastContentOperations(nodeId, content);
    }, 300);

    const expensiveTimer = setTimeout(() => {
      processExpensiveContentOperations(nodeId, content);
    }, 2000);

    debouncedOperations.set(nodeId, {
      pendingContent: content,
      fastTimer,
      expensiveTimer
    });
  }

  function processFastContentOperations(nodeId: string, _content: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    const operations = debouncedOperations.get(nodeId);
    if (operations) {
      operations.fastTimer = undefined;
      debouncedOperations.set(nodeId, operations);
    }
  }

  function processExpensiveContentOperations(nodeId: string, _content: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    // Note: Expensive operations (persistence, embeddings, reference propagation)
    // are now handled via LIVE SELECT and backend polling
    debouncedOperations.delete(nodeId);
  }

  function combineNodes(
    currentNodeId: string,
    previousNodeId: string,
    paneId: string = DEFAULT_PANE_ID
  ): void {
    const currentNode = sharedNodeStore.getNode(currentNodeId);
    const previousNode = sharedNodeStore.getNode(previousNodeId);

    if (!currentNode || !previousNode) return;

    const cleanedContent = stripFormattingSyntax(currentNode.content);
    const combinedContent = previousNode.content + cleanedContent;
    const mergePosition = previousNode.content.length;

    // Use viewer source with explicit immediate persistence (no longer need 'external' workaround)
    // The explicit persist option ensures the content update persists immediately
    const contentUpdateSource: UpdateSource = {
      type: 'viewer' as const,
      viewerId: paneId // Use paneId as viewerId since we're in a standalone function
    };
    sharedNodeStore.updateNode(
      previousNodeId,
      { content: combinedContent },
      contentUpdateSource,
      { persist: 'immediate' } // Explicit immediate persistence (replaces 'external' workaround)
    );
    _uiState[previousNodeId] = { ..._uiState[previousNodeId], autoFocus: false };

    // Handle child promotion using shared depth-aware logic
    promoteChildren(currentNodeId, previousNodeId);

    // CRITICAL: Collect dependencies that must persist before deletion
    // This prevents "database is locked" errors and FOREIGN KEY violations
    // PersistenceCoordinator will ensure these operations complete before deletion
    const deletionDependencies = [previousNodeId]; // Content update must persist first
    const children = sharedNodeStore.getNodesForParent(currentNodeId);
    if (children.length > 0) {
      deletionDependencies.push(...children.map((c) => c.id)); // Child promotions must complete
    }

    // NOTE: Sibling chain management removed - backend handles ordering via fractional ordering

    // Delete with dependencies - PersistenceCoordinator ensures correct order
    sharedNodeStore.deleteNode(currentNodeId, viewerSource, false, deletionDependencies);
    delete _uiState[currentNodeId];

    // CRITICAL: Must reassign (not mutate) for Svelte 5 reactivity
    const rootIndex = _rootNodeIds.indexOf(currentNodeId);
    if (rootIndex >= 0) {
      _rootNodeIds = _rootNodeIds.filter((id) => id !== currentNodeId);
    }

    // NOTE: Sorted children cache removed - backend provides pre-sorted children

    // Set focus on the previous node using FocusManager
    focusManager.setEditingNode(previousNodeId, paneId, mergePosition);
    events.focusRequested(previousNodeId, mergePosition);
    events.nodeDeleted(currentNodeId);
    events.hierarchyChanged();
  }

  function stripFormattingSyntax(content: string): string {
    let cleaned = content;
    // Strip header prefixes (# , ## , etc.)
    cleaned = cleaned.replace(/^#{1,6}\s+/, '');
    // Strip task checkbox ([ ] , [x] , etc.)
    cleaned = cleaned.replace(/^\[\s*[x\s]*\]\s*/, '');
    // Strip quote-block prefixes (> ) from all lines
    cleaned = cleaned
      .split('\n')
      .map((line) => line.replace(/^>\s?/, ''))
      .join('\n');
    return cleaned.trim();
  }

  // NOTE: removeFromSiblingChain function removed - backend handles sibling ordering via fractional ordering

  async function indentNode(nodeId: string): Promise<boolean> {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return false;

    // Find previous sibling (indent target)
    const parents = sharedNodeStore.getParentsForNode(nodeId);
    const currentParentId = parents.length > 0 ? parents[0].id : null;

    // Get siblings (already sorted by backend via fractional ordering)
    let siblings: string[];
    if (currentParentId) {
      siblings = sharedNodeStore.getNodesForParent(currentParentId).map((n) => n.id);
    } else {
      siblings = _rootNodeIds;
    }

    const nodeIndex = siblings.indexOf(nodeId);
    if (nodeIndex <= 0) return false; // Can't indent if no previous sibling

    const prevSiblingId = siblings[nodeIndex - 1];
    const prevSibling = sharedNodeStore.getNode(prevSiblingId);
    if (!prevSibling || !pluginRegistry.canHaveChildren(prevSibling.nodeType)) {
      return false; // Can't indent into node that can't have children
    }

    const targetParentId = prevSiblingId;
    const targetParentUIState = _uiState[targetParentId];

    // Save current state for rollback
    const originalUIState = { ..._uiState[nodeId] };
    const originalRootNodeIds = [..._rootNodeIds];

    // Optimistic UI update: Show the move immediately
    _uiState[nodeId] = { ..._uiState[nodeId], depth: (targetParentUIState?.depth || 0) + 1 };
    updateDescendantDepths(nodeId);

    if (!currentParentId) {
      _rootNodeIds = _rootNodeIds.filter((id) => id !== nodeId);
    }

    setExpanded(targetParentId, true);

    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy via LIVE SELECT events
    // Sibling positioning removed (Issue #557) - Backend handles ordering via fractional IDs

    // Update local state and notify (BEFORE backend call for instant UI response)
    // Update node's parentId to move it to the new parent
    // NOTE: beforeSiblingId removed from node - backend handles ordering via fractional ordering
    sharedNodeStore.updateNode(
      nodeId,
      { parentId: targetParentId },
      { type: 'database', reason: 'indent-node' },
      { isComputedField: true }
    );

    // CRITICAL FIX: Update ReactiveStructureTree for browser mode
    // In Tauri mode, LIVE SELECT events update the tree, but in browser mode we must do it manually
    structureTree.moveInMemoryRelationship(currentParentId, targetParentId, nodeId);

    events.hierarchyChanged();
    _updateTrigger++;

    // Fire-and-forget backend persistence (don't await - keep UI responsive!)
    moveNodeCommand(nodeId, targetParentId, null)
      .then(() => {
        // Success - backend persisted successfully
      })
      .catch((error) => {
        // Check if error is ignorable (unit test environment or unpersisted nodes)
        const isIgnorableError =
          error instanceof Error &&
          (error.message.includes('404') ||
            error.message.includes('Not Found') ||
            error.message.includes('ECONNREFUSED') ||
            error.message.includes('fetch failed') ||
            error.message.includes('Failed to execute "fetch()"'));

        if (!isIgnorableError) {
          // Non-ignorable error: rollback optimistic update
          _uiState[nodeId] = originalUIState;
          _rootNodeIds = originalRootNodeIds;
          // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles rollback via LIVE SELECT
          updateDescendantDepths(nodeId);
          if (currentParentId) {
            structureTree.moveInMemoryRelationship(targetParentId, currentParentId, nodeId);
          }
          events.hierarchyChanged();
          _updateTrigger++;

          console.error('[indentNode] Failed to move node, rolled back:', error);
        }
        // Ignorable error: keep UI updates (for unit tests without server)
      });

    return true;
  }

  async function outdentNode(nodeId: string): Promise<boolean> {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return false;

    // Get parent information
    const parents = sharedNodeStore.getParentsForNode(nodeId);
    if (parents.length === 0) return false;

    const parent = parents[0];
    const oldParentId = parent.id;
    const parentParentsQuery = sharedNodeStore.getParentsForNode(parent.id);
    const newParentId = parent.parentId ?? (parentParentsQuery.length > 0 ? parentParentsQuery[0].id : null);

    // Find siblings that come after this node (they will become children)
    // Backend returns children already sorted via fractional ordering
    const siblings = sharedNodeStore.getNodesForParent(oldParentId).map((n) => n.id);
    const nodeIndex = siblings.indexOf(nodeId);
    const siblingsBelow = nodeIndex >= 0 ? siblings.slice(nodeIndex + 1) : [];

    // Save current state for rollback
    const originalUIState = { ..._uiState[nodeId] };
    const originalRootNodeIds = [..._rootNodeIds];

    const newDepth = newParentId ? (_uiState[newParentId]?.depth || 0) + 1 : 0;

    // Optimistic UI updates for main node
    _uiState[nodeId] = { ..._uiState[nodeId], depth: newDepth };
    updateDescendantDepths(nodeId);

    if (!newParentId) {
      const parentIndex = _rootNodeIds.indexOf(parent.id);
      _rootNodeIds = [
        ..._rootNodeIds.slice(0, parentIndex + 1),
        nodeId,
        ..._rootNodeIds.slice(parentIndex + 1)
      ];
    }

    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy via LIVE SELECT events

    // Update local state and transfer siblings (BEFORE backend for instant UI)
    // Update node's parentId to move it to the new parent
    // NOTE: beforeSiblingId removed from node - backend handles ordering via fractional ordering
    sharedNodeStore.updateNode(
      nodeId,
      { parentId: newParentId },
      { type: 'database', reason: 'outdent-node' },
      { isComputedField: true }
    );

    // CRITICAL FIX: Update ReactiveStructureTree for browser mode
    // In Tauri mode, LIVE SELECT events update the tree, but in browser mode we must do it manually
    if (newParentId) {
      structureTree.moveInMemoryRelationship(oldParentId, newParentId, nodeId);
    }

    // Transfer siblings below as children (optimistic UI first)
    if (siblingsBelow.length > 0) {
      // NOTE: Sibling positioning removed (Issue #557) - Backend handles ordering via fractional IDs

      // Transfer each sibling - UI updates first
      for (let i = 0; i < siblingsBelow.length; i++) {
        const siblingId = siblingsBelow[i];
        const sibling = sharedNodeStore.getNode(siblingId);
        if (sibling) {
          // Optimistic UI update
          const siblingDepth = newDepth + 1;
          _uiState[siblingId] = { ..._uiState[siblingId], depth: siblingDepth };
          updateDescendantDepths(siblingId);

          // Update parentId to move sibling to new parent
          // NOTE: beforeSiblingId removed from node - backend handles ordering via fractional ordering
          sharedNodeStore.updateNode(
            siblingId,
            { parentId: nodeId },
            { type: 'database', reason: 'outdent-transfer' },
            { isComputedField: true }
          );

          // NOTE: Sibling chain management removed - backend handles ordering via fractional ordering
        }
      }

      setExpanded(nodeId, true);
    }

    events.hierarchyChanged();
    _updateTrigger++;

    // Fire-and-forget backend persistence (main node + siblings - don't await!)
    const backendPromises = [moveNodeCommand(nodeId, newParentId, null)];

    // Add sibling transfer backend calls
    for (const siblingId of siblingsBelow) {
      backendPromises.push(moveNodeCommand(siblingId, nodeId, null));
    }

    Promise.all(backendPromises)
      .then(() => {
        // Success - all backend operations persisted successfully
      })
      .catch((error) => {
        // Check if error is ignorable
        const isIgnorableError =
          error instanceof Error &&
          (error.message.includes('404') ||
            error.message.includes('Not Found') ||
            error.message.includes('ECONNREFUSED') ||
            error.message.includes('fetch failed') ||
            error.message.includes('Failed to execute "fetch()"'));

        if (!isIgnorableError) {
          // Non-ignorable error: rollback all changes
          _uiState[nodeId] = originalUIState;
          _rootNodeIds = originalRootNodeIds;
          updateDescendantDepths(nodeId);

          // Rollback siblings
          for (const siblingId of siblingsBelow) {
            const sibling = sharedNodeStore.getNode(siblingId);
            if (sibling) {
              _uiState[siblingId] = { ..._uiState[siblingId], depth: (_uiState[oldParentId]?.depth || 0) + 1 };
              updateDescendantDepths(siblingId);
            }
          }

          if (newParentId) {
            structureTree.moveInMemoryRelationship(newParentId, oldParentId, nodeId);
          }
          events.hierarchyChanged();
          _updateTrigger++;

          console.error('[outdentNode] Failed to move node, rolled back:', error);
        }
        // Ignorable error: keep UI updates (for unit tests without server)
      });

    return true;
  }

  /**
   * Promotes children of a node to a new parent using depth-aware selection.
   * Shared logic used by both combineNodes and deleteNode.
   *
   * @param nodeId - The node whose children will be promoted
   * @param previousNodeId - The previous sibling to use for depth-aware parent selection
   */
  function promoteChildren(nodeId: string, previousNodeId: string): void {
    const children = sharedNodeStore.getNodesForParent(nodeId);
    if (children.length === 0) return;

    // Find the nearest ancestor node at the SAME depth as the deleted node
    const _deletedNode = sharedNodeStore.getNode(nodeId);
    const deletedNodeDepth = _uiState[nodeId]?.depth ?? 0;

    // CRITICAL: If deleted node is at root level, promote children to root level too
    // Don't make them children of the previous sibling - maintain the flat root structure
    // NOTE: Now using backend query instead of deletedNode.parentId
    const deletedNodeParents = sharedNodeStore.getParentsForNode(nodeId);
    let newParentForChildren: string | null;
    if (deletedNodeParents.length === 0) {
      newParentForChildren = null; // Promote to root level
    } else {
      // For nested nodes, find a parent at the same depth
      newParentForChildren = previousNodeId;
      let searchNode: string | null = previousNodeId;

      while (searchNode) {
        const searchDepth = _uiState[searchNode]?.depth ?? 0;
        if (searchDepth === deletedNodeDepth) {
          newParentForChildren = searchNode;
          break;
        }
        if (searchDepth < deletedNodeDepth) {
          newParentForChildren = searchNode;
          break;
        }
        const searchNodeParents = sharedNodeStore.getParentsForNode(searchNode);
        searchNode = searchNodeParents.length > 0 ? searchNodeParents[0].id : null;
      }
    }

    // Process each child individually
    // NOTE: beforeSiblingId logic removed - backend handles ordering via fractional ordering
    // Children will be appended in their existing order (already sorted by backend)
    for (const child of children) {
      // NOTE: hierarchy managed by backend via has_child edges
      sharedNodeStore.updateNode(child.id, {}, viewerSource);

      // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy via LIVE SELECT events

      // CRITICAL: If promoting to root level, add to _rootNodeIds
      // Must reassign (not mutate) for Svelte 5 reactivity
      if (newParentForChildren === null && !_rootNodeIds.includes(child.id)) {
        _rootNodeIds = [..._rootNodeIds, child.id];
      }

      // Update depth
      const currentChildDepth = _uiState[child.id]?.depth ?? 0;
      const targetDepth = newParentForChildren
        ? (_uiState[newParentForChildren]?.depth ?? 0) + 1
        : 0;
      const newDepth = Math.min(currentChildDepth, targetDepth);

      _uiState[child.id] = {
        ..._uiState[child.id],
        depth: newDepth
      };

      updateDescendantDepths(child.id);
    }
  }


  /**
   * Deletes a node from storage.
   *
   * **IMPORTANT**: This function does NOT handle child promotion. Children will remain
   * orphaned with their parentId pointing to the deleted node. Use this only when:
   * 1. The node has no children, OR
   * 2. You have already handled child promotion explicitly
   *
   * **For user-facing deletion**: Use `combineNodes()` instead, which properly handles
   * child promotion using depth-aware logic to maintain outline structure.
   *
   * **Use cases for direct deleteNode()**:
   * - Backend operations where child handling is managed separately
   * - Cleanup operations where orphaned children are intentional
   *
   * @param nodeId - The ID of the node to delete
   */
  function deleteNode(nodeId: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    // NOTE: Parent determination no longer needed - cache management removed (Issue #557)
    cleanupDebouncedOperations(nodeId);

    // NOTE: Sibling chain management removed - backend handles ordering via fractional ordering

    // CRITICAL: Update children cache to remove this node from its parent
    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy via LIVE SELECT events

    sharedNodeStore.deleteNode(nodeId, viewerSource);
    delete _uiState[nodeId];

    // CRITICAL: Must reassign (not mutate) for Svelte 5 reactivity
    const rootIndex = _rootNodeIds.indexOf(nodeId);
    if (rootIndex >= 0) {
      _rootNodeIds = _rootNodeIds.filter((id) => id !== nodeId);
    }

    // NOTE: Sorted children cache removed - backend provides pre-sorted children

    events.nodeDeleted(nodeId);
    events.hierarchyChanged();
    _updateTrigger++;
  }

  /**
   * Set the expanded state of a node to a specific value
   * More efficient than toggleExpanded when you know the desired state
   *
   * @param nodeId - The ID of the node to update
   * @param expanded - The desired expanded state
   * @returns True if the state was changed, false if no change was needed or node doesn't exist
   */
  function setExpanded(nodeId: string, expanded: boolean): boolean {
    try {
      // Ensure node exists in SharedNodeStore first
      const node = sharedNodeStore.getNode(nodeId);
      if (!node) return false;

      // Initialize UIState if it doesn't exist (e.g., when indenting creates new parent-child relationship)
      let uiState = _uiState[nodeId];
      if (!uiState) {
        // Compute depth by walking parent chain using backend queries
        let depth = 0;
        let currentNodeId = nodeId;
        const visited = new Set<string>();
        while (!visited.has(currentNodeId)) {
          visited.add(currentNodeId);
          const parents = sharedNodeStore.getParentsForNode(currentNodeId);
          if (parents.length === 0) break;
          depth++;
          currentNodeId = parents[0].id;
        }

        uiState = createDefaultUIState(nodeId, { depth, expanded });
        _uiState[nodeId] = uiState;
        _updateTrigger++;
        events.hierarchyChanged();
        return true;
      }

      // No change needed
      if (uiState.expanded === expanded) return false;

      _uiState[nodeId] = { ...uiState, expanded };

      events.hierarchyChanged();
      _updateTrigger++;

      return true;
    } catch (error) {
      console.error(`[ReactiveNodeService] Error setting expanded state for ${nodeId}:`, error);
      return false;
    }
  }

  /**
   * Batch set expansion states for multiple nodes
   * More efficient than calling setExpanded multiple times as it batches UI updates
   *
   * @param updates - Array of {nodeId, expanded} updates to apply
   * @returns Number of nodes that were actually changed
   */
  function batchSetExpanded(updates: Array<{ nodeId: string; expanded: boolean }>): number {
    try {
      let changedCount = 0;
      const affectedNodes: string[] = [];

      // Apply all updates without triggering events
      for (const { nodeId, expanded } of updates) {
        const uiState = _uiState[nodeId];
        if (!uiState) continue;

        // Skip if no change needed
        if (uiState.expanded === expanded) continue;

        _uiState[nodeId] = { ...uiState, expanded };
        changedCount++;
        affectedNodes.push(nodeId);
      }

      // Only trigger UI update once if anything changed
      if (changedCount > 0) {
        events.hierarchyChanged();
        _updateTrigger++;
      }

      return changedCount;
    } catch (error) {
      console.error('[ReactiveNodeService] Error in batch set expanded:', error);
      return 0;
    }
  }

  function toggleExpanded(nodeId: string): boolean {
    try {
      const uiState = _uiState[nodeId];
      if (!uiState) return false;

      const newExpandedState = !uiState.expanded;
      _uiState[nodeId] = { ...uiState, expanded: newExpandedState };

      events.hierarchyChanged();
      _updateTrigger++;

      return true;
    } catch (error) {
      console.error('Error toggling node expansion:', error);
      return false;
    }
  }

  function cleanupDebouncedOperations(nodeId: string): void {
    const operations = debouncedOperations.get(nodeId);
    if (operations) {
      if (operations.fastTimer) clearTimeout(operations.fastTimer);
      if (operations.expensiveTimer) clearTimeout(operations.expensiveTimer);
      debouncedOperations.delete(nodeId);
    }
  }

  // Helper methods
  function updateDescendantDepths(nodeId: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    const uiState = _uiState[nodeId];
    if (!node || !uiState) return;

    const children = sharedNodeStore.getNodesForParent(nodeId);

    for (const child of children) {
      _uiState[child.id] = {
        ..._uiState[child.id],
        depth: uiState.depth + 1
      };
      updateDescendantDepths(child.id);
    }
  }

  return {
    // Reactive getters
    get nodes() {
      return sharedNodeStore.getAllNodes();
    },
    get rootNodeIds() {
      return _rootNodeIds;
    },
    get activeNodeId() {
      return _activeNodeId;
    },
    /**
     * Get visible nodes for a specific parent
     * @param parentId - Parent node ID (null for root-level nodes)
     * @returns Array of visible nodes sorted by visual hierarchy
     */
    visibleNodes(parentId: string | null): (Node & {
      depth: number;
      children: string[];
      expanded: boolean;
      autoFocus: boolean;
      inheritHeaderLevel: number;
      isPlaceholder: boolean;
    })[] {
      return getVisibleNodesForParent(parentId);
    },
    get _updateTrigger() {
      return _updateTrigger;
    },
    // Direct access to UI state for computed properties
    getUIState(nodeId: string) {
      return _uiState[nodeId];
    },

    // Node operations
    findNode,
    createNode,
    createPlaceholderNode,
    updateNodeContent,
    updateNodeType,
    updateNodeMentions,
    updateNodeProperties,
    combineNodes,
    indentNode,
    outdentNode,
    deleteNode,
    toggleExpanded,
    setExpanded,
    batchSetExpanded,

    // Content processing methods for integration tests
    parseNodeContent(nodeId: string) {
      const node = sharedNodeStore.getNode(nodeId);
      if (!node) return null;
      return contentProcessor.parseMarkdown(node.content);
    },

    async renderNodeAsHTML(nodeId: string): Promise<string> {
      const node = sharedNodeStore.getNode(nodeId);
      if (!node) return '';
      const result = await contentProcessor.markdownToDisplay(node.content);
      return result || '';
    },

    getNodeHeaderLevel(nodeId: string): number {
      const node = sharedNodeStore.getNode(nodeId);
      if (!node) return 0;
      const headerMatch = node.content.match(/^(#{1,6})\s+/);
      return headerMatch ? headerMatch[1].length : 0;
    },

    getNodeDisplayText(nodeId: string): string {
      const node = sharedNodeStore.getNode(nodeId);
      if (!node) return '';
      return contentProcessor
        .displayToMarkdown(node.content)
        .replace(/[#*`[\]()]/g, '')
        .trim();
    },

    updateNodeContentWithProcessing(nodeId: string, content: string): boolean {
      const node = sharedNodeStore.getNode(nodeId);
      if (!node) return false;
      updateNodeContent(nodeId, content);
      return true;
    },

    // Initialize service with nodes (used for loading from database)
    initializeNodes(
      nodes: Array<Node>,
      options?: {
        expanded?: boolean;
        autoFocus?: boolean;
        inheritHeaderLevel?: number;
        isInitialPlaceholder?: boolean;
        /**
         * Optional parent mapping for test scenarios where backend queries aren't available.
         * Maps node ID to parent ID. If not provided, uses backend queries.
         *
         * Example: { 'child-1': 'parent', 'child-2': 'parent' }
         */
        parentMapping?: Record<string, string | null>;
      }
    ): void {
      // NOTE: We no longer cleanup unpersisted nodes here
      // Instead, BaseNodeViewer reuses existing placeholders when database returns 0 nodes
      // This supports multi-tab/multi-pane scenarios where multiple viewers may share the same placeholder
      // See base-node-viewer.svelte loadChildrenForParent() for placeholder reuse logic

      // CRITICAL: Do NOT clear all global _uiState - this would destroy expansion state
      // for nodes being viewed in other panes. Only update state for nodes being initialized.
      // _rootNodeIds is still cleared because it represents the top-level view (single instance)
      _rootNodeIds = [];

      const defaults = {
        expanded: options?.expanded ?? true,
        autoFocus: options?.autoFocus ?? false,
        inheritHeaderLevel: options?.inheritHeaderLevel ?? 0,
        isInitialPlaceholder: options?.isInitialPlaceholder ?? false,
        parentMapping: options?.parentMapping
      };

      // Compute depth for a node based on parent chain using backend queries
      const computeDepth = (nodeId: string, visited = new Set<string>()): number => {
        if (visited.has(nodeId)) return 0; // Prevent infinite recursion
        visited.add(nodeId);

        const parents = sharedNodeStore.getParentsForNode(nodeId);
        if (parents.length === 0) return 0;
        return 1 + computeDepth(parents[0].id, visited);
      };

      // First pass: Add all nodes to SharedNodeStore
      // For initial placeholder: skip persistence (ephemeral until content added)
      // For database nodes: mark as persisted (already in database)
      // For other nodes: persist normally
      const databaseSource = { type: 'database' as const, reason: 'initialization' };
      const viewerSource = {
        type: 'viewer' as const,
        viewerId: 'base-node-viewer',
        reason: 'placeholder-initialization'
      };

      for (const node of nodes) {
        const isPlaceholder = node.nodeType === 'text' && node.content.trim() === '';
        const source = isPlaceholder ? viewerSource : databaseSource;

        // Only skip persistence for initial viewer placeholder (when no children exist)
        // All other nodes (including blank nodes created during editing) persist immediately
        const skipPersistence = defaults.isInitialPlaceholder && isPlaceholder;

        sharedNodeStore.setNode(node, source, skipPersistence);
        _uiState[node.id] = createDefaultUIState(node.id, {
          depth: 0, // Will be computed in second pass
          expanded: defaults.expanded,
          autoFocus: defaults.autoFocus,
          inheritHeaderLevel: defaults.inheritHeaderLevel,
          // Issue #479: Mark as placeholder if it's the initial viewer placeholder
          // This prevents the content watcher from persisting viewer-local placeholders
          isPlaceholder: skipPersistence && isPlaceholder
        });
      }

      // CRITICAL: Build parent-child relationship cache from loaded nodes
      // This populates sharedNodeStore's childrenCache and parentsCache
      //
      // Always build cache from nodes' parentId field
      // This works for both test scenarios and production (backend sets parentId)
      const nodesByParent = new Map<string | null, string[]>();
      for (const node of nodes) {
        const parentKey = defaults.parentMapping?.[node.id] ?? node.parentId ?? null;
        if (!nodesByParent.has(parentKey)) {
          nodesByParent.set(parentKey, []);
        }
        nodesByParent.get(parentKey)!.push(node.id);
      }

      // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy via LIVE SELECT events

      // Second pass: Compute depths and identify roots
      // NOTE: Now using backend queries to determine which nodes are children
      const childIds = new Set<string>();
      for (const node of nodes) {
        const parents = sharedNodeStore.getParentsForNode(node.id);
        if (parents.length > 0) {
          childIds.add(node.id);
        }
      }
      _rootNodeIds = nodes.filter((n) => !childIds.has(n.id)).map((n) => n.id);

      for (const node of nodes) {
        const depth = computeDepth(node.id);
        _uiState[node.id] = { ..._uiState[node.id], depth };
      }

      _updateTrigger++;
      events.hierarchyChanged();
    }
  };
}

export type ReactiveNodeService = ReturnType<typeof createReactiveNodeService>;

// For backward compatibility with existing imports
export { createReactiveNodeService as ReactiveNodeService };
