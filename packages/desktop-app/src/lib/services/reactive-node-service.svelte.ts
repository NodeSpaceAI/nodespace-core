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
import { eventBus } from './event-bus';
import { SharedNodeStore } from './shared-node-store';
import { getFocusManager } from './focus-manager.svelte';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import type { Node, NodeUIState } from '$lib/types';
import { createDefaultUIState } from '$lib/types';
import type { UpdateSource } from '$lib/types/update-protocol';
import { DEFAULT_PANE_ID } from '$lib/stores/navigation';

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

  const serviceName = 'ReactiveNodeService';
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

  // Performance optimization: Cache sorted children to avoid O(n) sorting on every render
  // Invalidate cache only when hierarchy changes (child added/removed/reordered)
  const _sortedChildrenCache = new Map<
    string | null,
    {
      childIds: string[]; // Original unsorted child IDs (for comparison)
      sorted: string[]; // Cached sorted result
    }
  >();

  // Helper function to check if two arrays contain the same elements in the same order
  function arraysEqual(a: string[], b: string[]): boolean {
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) {
      if (a[i] !== b[i]) return false;
    }
    return true;
  }

  // Helper function to invalidate sorted children cache for a parent
  function invalidateSortedChildrenCache(parentId: string | null): void {
    _sortedChildrenCache.delete(parentId);
  }

  // Helper function to sort children according to beforeSiblingId linked list
  // CRITICAL: This ensures nodes appear in correct visual order, not insertion order
  // Performance: Uses memoization to avoid O(n) sorting on every render
  function sortChildrenByBeforeSiblingId(childIds: string[], parentId?: string | null): string[] {
    if (childIds.length === 0) return [];

    // Check cache first - O(1) lookup
    const cacheKey = parentId ?? null;
    const cached = _sortedChildrenCache.get(cacheKey);
    if (cached && arraysEqual(cached.childIds, childIds)) {
      // Cache hit: return pre-sorted result without recomputing
      return cached.sorted;
    }

    // Cache miss: perform sorting (O(n) operation)

    // Build a map of beforeSiblingId -> nodeId for quick lookup
    const beforeSiblingMap = new Map<string | null, string>();
    for (const childId of childIds) {
      const child = sharedNodeStore.getNode(childId);
      if (child) {
        beforeSiblingMap.set(child.beforeSiblingId, childId);
      }
    }

    // Find ALL candidates for first child to detect data corruption
    const firstChildCandidates: string[] = [];
    for (const childId of childIds) {
      const child = sharedNodeStore.getNode(childId);
      if (child) {
        // A node is first if its beforeSiblingId is null or not in the sibling set
        if (child.beforeSiblingId === null || !childIds.includes(child.beforeSiblingId)) {
          firstChildCandidates.push(childId);
        }
      }
    }

    // Validate linked list integrity: should have exactly one first child
    if (firstChildCandidates.length === 0) {
      console.error(`[ReactiveNodeService] Cannot determine first child - returning unsorted`, {
        parentId: parentId || 'root',
        childCount: childIds.length,
        childPointers: childIds.map((id) => ({
          id,
          beforeSiblingId: sharedNodeStore.getNode(id)?.beforeSiblingId
        }))
      });
      return childIds;
    }

    if (firstChildCandidates.length > 1) {
      console.error(
        `[ReactiveNodeService] Multiple first children detected - data corruption in beforeSiblingId chain`,
        {
          parentId: parentId || 'root',
          candidates: firstChildCandidates,
          childPointers: childIds.map((id) => ({
            id,
            beforeSiblingId: sharedNodeStore.getNode(id)?.beforeSiblingId
          }))
        }
      );
      // Use first candidate but log the issue
    }

    const firstChildId = firstChildCandidates[0];

    // Build sorted list by following the linked list
    const sorted: string[] = [];
    let currentId: string | null = firstChildId;
    const visited = new Set<string>();

    // Safety: Stop if we've visited all children (prevents infinite loop on corrupted data)
    while (currentId && visited.size < childIds.length) {
      if (visited.has(currentId)) {
        // Circular reference detected - this is a data integrity violation
        console.error(
          `[ReactiveNodeService] Circular reference detected in beforeSiblingId chain`,
          {
            parentId: parentId || 'root',
            circularNodeId: currentId,
            visitedNodes: Array.from(visited),
            allChildIds: childIds,
            chainSoFar: sorted
          }
        );

        eventBus.emit<import('./event-types').CacheInvalidateEvent>({
          type: 'cache:invalidate',
          namespace: 'coordination',
          source: serviceName,
          cacheKey: 'all',
          scope: 'global',
          reason: 'circular-reference-detected'
        });

        break;
      }
      visited.add(currentId);
      sorted.push(currentId);

      // Find next sibling (the one whose beforeSiblingId points to currentId)
      const nextId = beforeSiblingMap.get(currentId);
      currentId = nextId || null;
    }

    // Add any remaining children that weren't in the linked list (orphaned nodes)
    const orphanedNodes: string[] = [];
    for (const childId of childIds) {
      if (!visited.has(childId)) {
        orphanedNodes.push(childId);
        sorted.push(childId);
      }
    }

    if (orphanedNodes.length > 0) {
      console.warn(
        `[ReactiveNodeService] Found orphaned nodes not in beforeSiblingId chain - appending to end`,
        {
          parentId: parentId || 'root',
          orphanedNodes,
          orphanedPointers: orphanedNodes.map((id) => ({
            id,
            beforeSiblingId: sharedNodeStore.getNode(id)?.beforeSiblingId
          }))
        }
      );
    }

    // Cache the sorted result for future renders (performance optimization)
    _sortedChildrenCache.set(cacheKey, {
      childIds: [...childIds], // Clone to avoid mutation issues
      sorted
    });

    return sorted;
  }

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
        // Get children from SharedNodeStore
        const childIds = sharedNodeStore.getNodesForParent(nodeId).map((n) => n.id);

        // Sort children according to beforeSiblingId linked list
        const children = sortChildrenByBeforeSiblingId(childIds, nodeId);

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
      // Get children from SharedNodeStore
      // NOTE: Parent may not exist yet (e.g., virtual date nodes) - this is OK
      const nodesFromStore = sharedNodeStore.getNodesForParent(viewParentId);
      const childIds = nodesFromStore.map((n) => n.id);
      // Sort children according to beforeSiblingId linked list
      viewRoots = sortChildrenByBeforeSiblingId(childIds, viewParentId);
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

    // Determine parent from parameter or use backend query
    let newParentId: string | null;
    if (parentId !== undefined) {
      newParentId = parentId;
    } else {
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

    // Determine before_sibling_id based on insertion position
    let beforeSiblingId: string | null = null;
    if (insertAtBeginning) {
      // New node takes the place of afterNode, so it gets afterNode's before_sibling_id
      beforeSiblingId = afterNode.beforeSiblingId;
    } else {
      // New node comes after afterNode, so afterNode becomes its before_sibling
      beforeSiblingId = afterNodeId;
    }

    // Create Node with unified type system
    // NOTE: parentId and containerNodeId removed - hierarchy managed by backend
    const newNode: Node = {
      id: nodeId,
      nodeType: nodeType,
      content: initialContent,
      beforeSiblingId: beforeSiblingId,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {},
      mentions: []
    };

    // Create UI state (autoFocus removed - now derived from FocusManager)
    const newUIState = createDefaultUIState(nodeId, {
      depth: newDepth,
      expanded: true,
      inheritHeaderLevel: headerLevel !== undefined ? headerLevel : afterUIState.inheritHeaderLevel,
      isPlaceholder
    });

    // Find next sibling BEFORE adding new node to prevent interference
    const siblings = sharedNodeStore.getNodesForParent(newParentId);
    const nextSibling = !insertAtBeginning
      ? siblings.find((n) => n.beforeSiblingId === afterNodeId)
      : null;

    // Skip persistence ONLY for initial viewer placeholder (when no children exist)
    // All other nodes (including blank nodes created during editing) persist immediately
    // This prevents UNIQUE constraint violations when indenting blank nodes
    const skipPersistence = isInitialPlaceholder;
    sharedNodeStore.setNode(newNode, viewerSource, skipPersistence);
    _uiState[nodeId] = newUIState;

    // CRITICAL: Update children cache when creating a new node
    // This keeps the cache synchronized with the actual hierarchy
    sharedNodeStore.addChildToCache(newParentId, nodeId);

    // Set focus using FocusManager (single source of truth)
    // This replaces manual autoFocus flag manipulation
    if (shouldFocusNewNode) {
      focusManager.setEditingNode(nodeId, paneId);
    }

    // Update sibling linked list
    if (insertAtBeginning) {
      // New node takes afterNode's place, so afterNode now comes after newNode
      sharedNodeStore.updateNode(afterNodeId, { beforeSiblingId: nodeId }, viewerSource);
    } else {
      // New node inserted after afterNode - update next sibling to point to new node
      if (nextSibling) {
        sharedNodeStore.updateNode(nextSibling.id, { beforeSiblingId: nodeId }, viewerSource);
      }
    }

    // Bug 4 fix: Transfer children from expanded nodes
    // If afterNode is expanded and has children, transfer them to the new node
    if (!insertAtBeginning && afterUIState.expanded) {
      const children = sharedNodeStore.getNodesForParent(afterNodeId);

      if (children.length > 0) {
        // CRITICAL: Wait for newNode to be persisted before transferring children
        // This prevents FOREIGN KEY constraint violations when children reference the new parent
        // Use Promise to defer execution until after current synchronous stack completes
        Promise.resolve().then(async () => {
          // Wait for newNode to be persisted to database
          await sharedNodeStore.waitForNodeSaves([nodeId]);

          // Now safe to transfer children - newNode exists in database
          // NOTE: parentId removed - hierarchy managed by backend via parent_of edges
          for (const child of children) {
            // Backend will handle parent relationship through parent_of edges
            sharedNodeStore.updateNode(child.id, {}, viewerSource);
          }
        });
      }
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

    // Invalidate sorted children cache for parent (hierarchy changed)
    invalidateSortedChildrenCache(newParentId);
    // If children were transferred to new node, invalidate that cache too
    if (!insertAtBeginning && afterUIState.expanded) {
      invalidateSortedChildrenCache(nodeId);
    }

    events.nodeCreated(nodeId);
    events.hierarchyChanged();

    eventBus.emit<import('./event-types').NodeCreatedEvent>({
      type: 'node:created',
      namespace: 'lifecycle',
      source: serviceName,
      nodeId,
      nodeType,
      metadata: {}
    });

    eventBus.emit<import('./event-types').HierarchyChangedEvent>({
      type: 'hierarchy:changed',
      namespace: 'lifecycle',
      source: serviceName,
      changeType: 'create',
      affectedNodes: [nodeId]
    });

    eventBus.emit<import('./event-types').CacheInvalidateEvent>({
      type: 'cache:invalidate',
      namespace: 'coordination',
      source: serviceName,
      cacheKey: `node:${nodeId}`,
      scope: 'node',
      nodeId,
      reason: 'node-created'
    });

    emitReferenceUpdateNeeded(nodeId);

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
    isInitialPlaceholder: boolean = false
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
      isInitialPlaceholder
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

    emitNodeUpdated(nodeId, 'content', content);
    emitReferenceUpdateNeeded(nodeId);
    scheduleContentProcessing(nodeId, content);
  }

  function updateNodeType(nodeId: string, nodeType: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    // CRITICAL: Include content with nodeType update to ensure backend persistence works
    // Some backends may not support updating nodeType alone
    const updatePayload = { nodeType, content: node.content };
    // Skip conflict detection for nodeType changes - they are always intentional conversions
    sharedNodeStore.updateNode(nodeId, updatePayload, viewerSource, {
      skipConflictDetection: true
    });

    // Focus is managed by BaseNodeViewer during conversions
    // Do not override cursor position here

    emitNodeUpdated(nodeId, 'nodeType', nodeType);
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
    emitNodeUpdated(nodeId, 'mentions', mentions);
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
    emitNodeUpdated(nodeId, 'properties', properties);
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

    emitReferenceUpdateNeeded(nodeId);

    const operations = debouncedOperations.get(nodeId);
    if (operations) {
      operations.fastTimer = undefined;
      debouncedOperations.set(nodeId, operations);
    }
  }

  function processExpensiveContentOperations(nodeId: string, content: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    emitExpensivePersistenceNeeded(nodeId, content);
    emitVectorEmbeddingNeeded(nodeId, content);
    emitReferencePropagatationNeeded(nodeId, content);

    debouncedOperations.delete(nodeId);
  }

  function emitReferenceUpdateNeeded(nodeId: string): void {
    eventBus.emit<import('./event-types').ReferencesUpdateNeededEvent>({
      type: 'references:update-needed',
      namespace: 'coordination',
      source: serviceName,
      nodeId,
      updateType: 'content',
      metadata: {}
    });
  }

  function emitExpensivePersistenceNeeded(nodeId: string, content: string): void {
    eventBus.emit<import('./event-types').NodePersistenceEvent>({
      type: 'node:persistence-needed',
      namespace: 'backend',
      source: serviceName,
      nodeId,
      content,
      metadata: {}
    });
  }

  function emitVectorEmbeddingNeeded(nodeId: string, content: string): void {
    eventBus.emit<import('./event-types').NodeEmbeddingEvent>({
      type: 'node:embedding-needed',
      namespace: 'ai',
      source: serviceName,
      nodeId,
      content,
      metadata: {}
    });
  }

  function emitReferencePropagatationNeeded(nodeId: string, content: string): void {
    eventBus.emit<import('./event-types').NodeReferencePropagationEvent>({
      type: 'node:reference-propagation-needed',
      namespace: 'references',
      source: serviceName,
      nodeId,
      content,
      metadata: {}
    });
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
    const currentNodeParents = sharedNodeStore.getParentsForNode(currentNodeId);
    const currentParentId = currentNodeParents.length > 0 ? currentNodeParents[0].id : null;
    const siblings = sharedNodeStore.getNodesForParent(currentParentId);
    const nextSibling = siblings.find((n) => n.beforeSiblingId === currentNodeId);
    if (nextSibling) {
      deletionDependencies.push(nextSibling.id); // Sibling chain repair must complete
    }
    const children = sharedNodeStore.getNodesForParent(currentNodeId);
    if (children.length > 0) {
      deletionDependencies.push(...children.map((c) => c.id)); // Child promotions must complete
    }

    // Remove node from sibling chain BEFORE deletion to prevent orphans
    removeFromSiblingChain(currentNodeId);

    // Delete with dependencies - PersistenceCoordinator ensures correct order
    sharedNodeStore.deleteNode(currentNodeId, viewerSource, false, deletionDependencies);
    delete _uiState[currentNodeId];

    // CRITICAL: Must reassign (not mutate) for Svelte 5 reactivity
    const rootIndex = _rootNodeIds.indexOf(currentNodeId);
    if (rootIndex >= 0) {
      _rootNodeIds = _rootNodeIds.filter((id) => id !== currentNodeId);
    }

    // Invalidate sorted children cache for both old parent and new parent
    invalidateSortedChildrenCache(currentParentId);
    invalidateSortedChildrenCache(currentNodeId);

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

  /**
   * PERSISTENCE DEPENDENCY PATTERN: Sibling Chain Updates
   *
   * CRITICAL ARCHITECTURAL DECISION for indent/outdent operations:
   *
   * DO NOT add the updatedSiblingId returned from removeFromSiblingChain() as a
   * persistence dependency in indent/outdent operations.
   *
   * Rationale:
   * 1. Consecutive indent/outdent operations can update each other as siblings,
   *    creating circular dependency chains (e.g., node-2 waits for node-3,
   *    node-3 waits for node-2, resulting in deadlock)
   * 2. Sibling chain updates from removeFromSiblingChain are independent operations
   *    that can execute in parallel with the main update without violating database
   *    constraints
   * 3. SharedNodeStore's automatic dependency system (lines 297-302) ensures that
   *    beforeSiblingId references wait for node persistence (handles FOREIGN KEY
   *    constraints automatically)
   * 4. Removing this dependency eliminates circular deadlocks without sacrificing
   *    database integrity guarantees
   *
   * @see SharedNodeStore lines 297-302 for automatic dependency injection
   * @see indentNode() for implementation example
   * @see outdentNode() for implementation example
   */

  /**
   * Removes a node from its current sibling chain by updating the next sibling's beforeSiblingId.
   * This prevents orphaned nodes when a node is moved (indent/outdent) or deleted.
   *
   * @param nodeId - The node being removed from its current parent
   */
  function removeFromSiblingChain(nodeId: string): string | null {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return null;

    // Find the next sibling (the node that points to this one via beforeSiblingId)
    // NOTE: Now using backend query instead of node.parentId
    const parents = sharedNodeStore.getParentsForNode(nodeId);
    const parentId = parents.length > 0 ? parents[0].id : null;
    const siblings = sharedNodeStore.getNodesForParent(parentId);
    const nextSibling = siblings.find((n) => n.beforeSiblingId === nodeId);

    if (nextSibling) {
      // Update next sibling to point to our predecessor, "splicing us out" of the chain
      sharedNodeStore.updateNode(
        nextSibling.id,
        { beforeSiblingId: node.beforeSiblingId },
        viewerSource,
        { skipConflictDetection: true } // Sequential structural updates
      );
      return nextSibling.id; // Return the ID of the updated sibling
    }
    return null;
  }

  function indentNode(nodeId: string): boolean {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return false;

    // Get SORTED siblings according to beforeSiblingId chain
    // NOTE: Now using backend query instead of node.parentId
    const parents = sharedNodeStore.getParentsForNode(nodeId);
    const currentParentId = parents.length > 0 ? parents[0].id : null;

    let siblings: string[];
    if (currentParentId) {
      const unsortedSiblings = sharedNodeStore.getNodesForParent(currentParentId).map((n) => n.id);
      siblings = sortChildrenByBeforeSiblingId(unsortedSiblings, currentParentId);
    } else {
      siblings = sortChildrenByBeforeSiblingId(_rootNodeIds, null);
    }

    const nodeIndex = siblings.indexOf(nodeId);
    if (nodeIndex <= 0) return false; // Can't indent first child or if not found

    const prevSiblingId = siblings[nodeIndex - 1];
    const prevSibling = sharedNodeStore.getNode(prevSiblingId);
    if (!prevSibling) return false;

    // Check if previous sibling can have children
    if (!pluginRegistry.canHaveChildren(prevSibling.nodeType)) {
      return false; // Silently prevent indent (matches merge prevention UX)
    }

    // Indent target is ALWAYS the previous sibling directly
    // The node becomes a child of the previous sibling, positioned after any existing children
    //
    // Examples:
    // - Simple indent: A B → A[B] (B becomes child of A)
    // - With existing children: A[existing] B → A[existing, B] (B becomes sibling of existing)
    // - Multiple indents: A B C → indent B → A[B] C → indent C → A[B, C] (both siblings under A)
    //
    // This matches natural outliner behavior where Tab makes a node a child of the item above it
    const targetParentId = prevSiblingId;

    // Step 1: Remove node from current sibling chain BEFORE changing parent
    // This operation updates the next sibling (if any) to maintain chain integrity
    const updatedSiblingId = removeFromSiblingChain(nodeId);

    // The node becomes a child of the target parent
    // It should be appended to the end of the target parent's children
    const existingChildren = sharedNodeStore.getNodesForParent(targetParentId).map((n) => n.id);

    let beforeSiblingId: string | null = null;
    if (existingChildren.length > 0) {
      // Get sorted children and append after the last one
      const sortedChildren = sortChildrenByBeforeSiblingId(existingChildren, targetParentId);
      beforeSiblingId = sortedChildren[sortedChildren.length - 1]; // Insert after last child
    }

    const uiState = _uiState[nodeId];
    const targetParentUIState = _uiState[targetParentId];

    // Step 2: Build dependency list for persistence sequencing
    const persistenceDependencies: string[] = [];

    // Defensive validation: Ensure parent exists before adding dependency
    if (!targetParentId) {
      console.error('[indentNode] Invalid state: targetParentId is null/undefined');
      return false; // Early return to prevent corruption
    }

    // Ensure the target parent is persisted before making this node its child
    persistenceDependencies.push(targetParentId);

    // If positioning after an existing child, ensure that child is persisted
    if (beforeSiblingId) {
      persistenceDependencies.push(beforeSiblingId);
    }

    // If we updated a sibling during removal, ensure it's persisted first
    // This prevents FOREIGN KEY violations when the updated sibling references this node
    if (updatedSiblingId) {
      persistenceDependencies.push(updatedSiblingId);
    }

    // Step 3: Update the main node with persistence dependencies
    // NOTE: parentId and containerNodeId removed - hierarchy managed by backend via parent_of edges
    sharedNodeStore.updateNode(
      nodeId,
      {
        beforeSiblingId: beforeSiblingId
      },
      viewerSource,
      {
        persistenceDependencies,
        // Skip conflict detection for sequential viewer operations
        // These are coordinated structural changes, not concurrent edits
        skipConflictDetection: true
      }
    );

    _uiState[nodeId] = { ...uiState, depth: (targetParentUIState?.depth || 0) + 1 };

    // Recalculate depths for all descendants
    updateDescendantDepths(nodeId);

    // If node was a root node, remove from _rootNodeIds array
    // CRITICAL: Must reassign (not mutate) for Svelte 5 reactivity
    // NOTE: Now using backend query instead of node.parentId
    if (!currentParentId) {
      const rootIndex = _rootNodeIds.indexOf(nodeId);
      if (rootIndex >= 0) {
        _rootNodeIds = _rootNodeIds.filter((id) => id !== nodeId);
      }
    }

    // Invalidate sorted children cache for both old and new parents
    invalidateSortedChildrenCache(currentParentId); // Old parent
    invalidateSortedChildrenCache(targetParentId); // New parent

    // CRITICAL: Update children cache when moving node to new parent
    // Remove from old parent, add to new parent
    sharedNodeStore.removeChildFromCache(currentParentId, nodeId);
    sharedNodeStore.addChildToCache(targetParentId, nodeId);

    // Ensure the target parent is expanded to show the newly indented child
    // This prevents the parent from appearing collapsed after indent
    setExpanded(targetParentId, true);

    events.hierarchyChanged();
    _updateTrigger++;

    eventBus.emit<import('./event-types').HierarchyChangedEvent>({
      type: 'hierarchy:changed',
      namespace: 'lifecycle',
      source: serviceName,
      changeType: 'indent',
      affectedNodes: [nodeId]
    });

    eventBus.emit<import('./event-types').ReferencesUpdateNeededEvent>({
      type: 'references:update-needed',
      namespace: 'coordination',
      source: serviceName,
      nodeId,
      updateType: 'hierarchy',
      affectedReferences: []
    });

    return true;
  }

  function outdentNode(nodeId: string): boolean {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return false;

    // NOTE: Now using backend query instead of node.parentId
    const parents = sharedNodeStore.getParentsForNode(nodeId);
    if (parents.length === 0) return false;

    const parent = parents[0];
    const oldParentId = parent.id;

    // Find siblings that come after this node (they will become children)
    const siblings = sharedNodeStore.getNodesForParent(oldParentId).map((n) => n.id);
    const sortedSiblings = sortChildrenByBeforeSiblingId(siblings, oldParentId);
    const nodeIndex = sortedSiblings.indexOf(nodeId);
    const siblingsBelow = nodeIndex >= 0 ? sortedSiblings.slice(nodeIndex + 1) : [];

    // Step 1: Prepare for parent change
    // Remove node from current sibling chain to maintain chain integrity
    const updatedSiblingFromRemoval = removeFromSiblingChain(nodeId);

    // Step 2: Calculate new position in parent hierarchy
    // NOTE: Now using backend query instead of parent.parentId
    const parentParents = sharedNodeStore.getParentsForNode(parent.id);
    const newParentId = parentParents.length > 0 ? parentParents[0].id : null;
    const uiState = _uiState[nodeId];
    const newDepth = newParentId ? (_uiState[newParentId]?.depth || 0) + 1 : 0;

    // Find where to position the outdented node in its new parent's child list
    // It should come right after its old parent (which is now a sibling)
    let positionBeforeSibling: string | null = oldParentId;

    // Check if old parent has a valid position in the new parent's context
    const oldParentParents = sharedNodeStore.getParentsForNode(oldParentId);
    const oldParentCurrentParentId = oldParentParents.length > 0 ? oldParentParents[0].id : null;
    if (oldParentCurrentParentId === newParentId) {
      // Old parent is a valid sibling in the new context
      positionBeforeSibling = oldParentId;
    } else {
      // Old parent is not in the same parent context (shouldn't happen in normal outdent)
      // Position at the end by finding the last sibling
      const siblings = sharedNodeStore
        .getNodesForParent(newParentId)
        .filter((n) => n.id !== nodeId)
        .map((n) => n.id);
      if (siblings.length > 0) {
        const sortedSiblings = sortChildrenByBeforeSiblingId(siblings, newParentId);
        positionBeforeSibling = sortedSiblings[sortedSiblings.length - 1];
      } else {
        positionBeforeSibling = null;
      }
    }

    // Step 3: Build persistence dependencies
    const mainNodeDeps: string[] = [];

    // Defensive validation: Ensure old parent exists before adding dependency
    if (!oldParentId) {
      console.error('[outdentNode] Invalid state: oldParentId is null/undefined');
      return false; // Early return to prevent corruption
    }

    mainNodeDeps.push(oldParentId);
    if (positionBeforeSibling && positionBeforeSibling !== oldParentId) {
      mainNodeDeps.push(positionBeforeSibling);
    }

    // If we updated a sibling during removal from old parent, ensure it's persisted first
    // This prevents FOREIGN KEY violations when the updated sibling references this node
    if (updatedSiblingFromRemoval) {
      mainNodeDeps.push(updatedSiblingFromRemoval);
    }

    // Step 4: Execute main node update
    // NOTE: parentId removed - hierarchy managed by backend via parent_of edges
    sharedNodeStore.updateNode(
      nodeId,
      {
        beforeSiblingId: positionBeforeSibling
      },
      viewerSource,
      {
        persistenceDependencies: mainNodeDeps,
        // Skip conflict detection for sequential viewer operations
        skipConflictDetection: true
      }
    );

    _uiState[nodeId] = { ...uiState, depth: newDepth };

    // Step 5: Repair sibling chain in new parent
    // Find any sibling in the new parent that currently points to positionBeforeSibling
    // and update it to point to the outdented node instead
    if (positionBeforeSibling !== null) {
      // Optimization: Only check direct siblings, not all nodes
      const allSiblings = sharedNodeStore.getNodesForParent(newParentId);

      for (const sibling of allSiblings) {
        if (sibling.id !== nodeId && sibling.beforeSiblingId === positionBeforeSibling) {
          sharedNodeStore.updateNode(sibling.id, { beforeSiblingId: nodeId }, viewerSource, {
            persistenceDependencies: [nodeId], // Wait for main node
            skipConflictDetection: true // Sequential structural updates
          });
          break; // Only one sibling can point to positionBeforeSibling
        }
      }
    }

    // Step 6: Transfer affected siblings as children
    // Need to rebuild their before_sibling_id chain to be valid in new parent context
    if (siblingsBelow.length > 0) {
      // Find existing children of the outdented node to append after them
      const existingChildren = sharedNodeStore
        .getNodesForParent(nodeId)
        .filter((n) => !siblingsBelow.includes(n.id))
        .map((n) => n.id);

      let lastSiblingId: string | null = null;
      if (existingChildren.length > 0) {
        const sortedChildren = sortChildrenByBeforeSiblingId(existingChildren, nodeId);
        lastSiblingId = sortedChildren[sortedChildren.length - 1];
      }

      // Transfer each sibling, updating their before_sibling_id chain
      // Each sibling must wait for the previous one to complete
      for (let i = 0; i < siblingsBelow.length; i++) {
        const siblingId = siblingsBelow[i];
        const sibling = sharedNodeStore.getNode(siblingId);
        if (sibling) {
          // Remove from current sibling chain BEFORE updating beforeSiblingId
          const removedSiblingId = removeFromSiblingChain(siblingId);

          // First transferred sibling points to last existing child (or null)
          // Subsequent siblings point to the previous transferred sibling
          const beforeSiblingId = i === 0 ? lastSiblingId : siblingsBelow[i - 1];

          // Build dependency chain
          const deps: string[] = [
            nodeId, // Wait for main node (the new parent)
            ...(removedSiblingId ? [removedSiblingId] : []), // Wait for sibling chain removal
            ...(beforeSiblingId ? [beforeSiblingId] : []) // Wait for beforeSibling
          ];

          // Update the sibling
          // NOTE: parentId removed - hierarchy managed by backend via parent_of edges
          sharedNodeStore.updateNode(
            siblingId,
            {
              beforeSiblingId
            },
            viewerSource,
            {
              persistenceDependencies: deps,
              skipConflictDetection: true // Sequential structural updates
            }
          );

          // Update depth for transferred sibling
          const siblingUIState = _uiState[siblingId];
          if (siblingUIState) {
            _uiState[siblingId] = { ...siblingUIState, depth: newDepth + 1 };
          }
          // Recalculate depths for descendants of transferred sibling
          updateDescendantDepths(siblingId);
        }
      }
    }

    // Recalculate depths for all descendants
    updateDescendantDepths(nodeId);

    // CRITICAL: Must reassign (not mutate) for Svelte 5 reactivity
    if (!newParentId) {
      const parentIndex = _rootNodeIds.indexOf(parent.id);
      _rootNodeIds = [
        ..._rootNodeIds.slice(0, parentIndex + 1),
        nodeId,
        ..._rootNodeIds.slice(parentIndex + 1)
      ];
    }

    // Invalidate sorted children cache for old parent, new parent, and outdented node
    invalidateSortedChildrenCache(oldParentId); // Old parent
    invalidateSortedChildrenCache(newParentId); // New parent
    invalidateSortedChildrenCache(nodeId); // Outdented node (now has new children)

    // CRITICAL: Update children cache when outdenting
    // Remove from old parent, add to new parent
    sharedNodeStore.removeChildFromCache(oldParentId, nodeId);
    sharedNodeStore.addChildToCache(newParentId, nodeId);

    // Also update cache for siblings that became children of the outdented node
    if (siblingsBelow.length > 0) {
      for (const siblingId of siblingsBelow) {
        sharedNodeStore.removeChildFromCache(oldParentId, siblingId);
        sharedNodeStore.addChildToCache(nodeId, siblingId);
      }
      setExpanded(nodeId, true);
    }

    events.hierarchyChanged();
    _updateTrigger++;

    eventBus.emit<import('./event-types').HierarchyChangedEvent>({
      type: 'hierarchy:changed',
      namespace: 'lifecycle',
      source: serviceName,
      changeType: 'outdent',
      affectedNodes: [nodeId]
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

    // Find existing children of the new parent to append after them
    const existingChildren = sharedNodeStore
      .getNodesForParent(newParentForChildren)
      .filter((n) => !children.find((c) => c.id === n.id))
      .map((n) => n.id);

    let lastSiblingId: string | null = null;
    if (existingChildren.length > 0) {
      const sortedChildren = sortChildrenByBeforeSiblingId(existingChildren, newParentForChildren);
      lastSiblingId = sortedChildren[sortedChildren.length - 1];
    }

    const sortedDeletedChildren = sortChildrenByBeforeSiblingId(
      children.map((c) => c.id),
      nodeId
    );
    const firstChildId = sortedDeletedChildren[0];

    // Process each child individually
    // Note: We could optimize this with a batch update API, but typical nodes have <10 children
    // and PersistenceCoordinator already handles batching at a lower level
    for (const child of children) {
      const updates: Partial<Node> = {};

      if (child.id === firstChildId) {
        updates.beforeSiblingId = lastSiblingId;
      }

      // NOTE: parentId and containerNodeId removed - hierarchy managed by backend via parent_of edges
      sharedNodeStore.updateNode(child.id, updates, viewerSource);

      // CRITICAL: Update children cache when promoting children to new parent
      sharedNodeStore.removeChildFromCache(nodeId, child.id);
      sharedNodeStore.addChildToCache(newParentForChildren, child.id);

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
   * Deletes a node from storage with sibling chain repair.
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
   * - Testing sibling chain repair logic in isolation
   * - Backend operations where child handling is managed separately
   * - Cleanup operations where orphaned children are intentional
   *
   * @param nodeId - The ID of the node to delete
   */
  function deleteNode(nodeId: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    // Determine parent BEFORE deleting the node
    const parents = sharedNodeStore.getParentsForNode(nodeId);
    const parentId = parents.length > 0 ? parents[0].id : null;

    cleanupDebouncedOperations(nodeId);

    // Remove node from sibling chain BEFORE deletion to prevent orphans
    removeFromSiblingChain(nodeId);

    // CRITICAL: Update children cache to remove this node from its parent
    sharedNodeStore.removeChildFromCache(parentId, nodeId);

    sharedNodeStore.deleteNode(nodeId, viewerSource);
    delete _uiState[nodeId];

    // CRITICAL: Must reassign (not mutate) for Svelte 5 reactivity
    const rootIndex = _rootNodeIds.indexOf(nodeId);
    if (rootIndex >= 0) {
      _rootNodeIds = _rootNodeIds.filter((id) => id !== nodeId);
    }

    // Invalidate sorted children cache for this node and its children
    invalidateSortedChildrenCache(nodeId);

    events.nodeDeleted(nodeId);
    events.hierarchyChanged();
    _updateTrigger++;

    eventBus.emit<import('./event-types').NodeDeletedEvent>({
      type: 'node:deleted',
      namespace: 'lifecycle',
      source: serviceName,
      nodeId
    });

    eventBus.emit<import('./event-types').HierarchyChangedEvent>({
      type: 'hierarchy:changed',
      namespace: 'lifecycle',
      source: serviceName,
      changeType: 'delete',
      affectedNodes: [nodeId]
    });

    eventBus.emit<import('./event-types').CacheInvalidateEvent>({
      type: 'cache:invalidate',
      namespace: 'coordination',
      source: serviceName,
      cacheKey: 'all',
      scope: 'global',
      reason: 'node-deleted'
    });

    eventBus.emit<import('./event-types').ReferencesUpdateNeededEvent>({
      type: 'references:update-needed',
      namespace: 'coordination',
      source: serviceName,
      nodeId,
      updateType: 'deletion',
      metadata: {}
    });
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

      const status: import('./event-types').NodeStatus = expanded ? 'expanded' : 'collapsed';
      const changeType: import('./event-types').HierarchyChangedEvent['changeType'] = expanded
        ? 'expand'
        : 'collapse';

      eventBus.emit<import('./event-types').NodeStatusChangedEvent>({
        type: 'node:status-changed',
        namespace: 'coordination',
        source: serviceName,
        nodeId,
        status
      });

      eventBus.emit<import('./event-types').HierarchyChangedEvent>({
        type: 'hierarchy:changed',
        namespace: 'lifecycle',
        source: serviceName,
        changeType,
        affectedNodes: [nodeId],
        metadata: {}
      });

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

        // Emit individual status change events
        const status: import('./event-types').NodeStatus = expanded ? 'expanded' : 'collapsed';
        eventBus.emit<import('./event-types').NodeStatusChangedEvent>({
          type: 'node:status-changed',
          namespace: 'coordination',
          source: serviceName,
          nodeId,
          status
        });
      }

      // Only trigger UI update once if anything changed
      if (changedCount > 0) {
        events.hierarchyChanged();
        _updateTrigger++;

        // Emit single hierarchy changed event for all changes
        eventBus.emit<import('./event-types').HierarchyChangedEvent>({
          type: 'hierarchy:changed',
          namespace: 'lifecycle',
          source: serviceName,
          changeType: 'expand', // Simplified - batch operations are expansion-focused
          affectedNodes,
          metadata: { batchSize: changedCount }
        });
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

      const status: import('./event-types').NodeStatus = newExpandedState
        ? 'expanded'
        : 'collapsed';
      const changeType: import('./event-types').HierarchyChangedEvent['changeType'] =
        newExpandedState ? 'expand' : 'collapse';

      eventBus.emit<import('./event-types').NodeStatusChangedEvent>({
        type: 'node:status-changed',
        namespace: 'coordination',
        source: serviceName,
        nodeId,
        status
      });

      eventBus.emit<import('./event-types').HierarchyChangedEvent>({
        type: 'hierarchy:changed',
        namespace: 'lifecycle',
        source: serviceName,
        changeType,
        affectedNodes: [nodeId]
      });

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

  function emitNodeUpdated(nodeId: string, updateType: string, newValue: unknown): void {
    eventBus.emit<import('./event-types').NodeUpdatedEvent>({
      type: 'node:updated',
      namespace: 'lifecycle',
      source: serviceName,
      nodeId,
      updateType: updateType as import('./event-types').NodeUpdatedEvent['updateType'],
      newValue
    });

    if (updateType === 'content') {
      eventBus.emit<import('./event-types').DecorationUpdateNeededEvent>({
        type: 'decoration:update-needed',
        namespace: 'interaction',
        source: serviceName,
        nodeId,
        decorationType: 'content',
        reason: 'content-changed',
        metadata: {}
      });

      eventBus.emit<import('./event-types').CacheInvalidateEvent>({
        type: 'cache:invalidate',
        namespace: 'coordination',
        source: serviceName,
        cacheKey: `node:${nodeId}`,
        scope: 'node',
        nodeId,
        reason: 'content-updated'
      });
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
        isInitialPlaceholder: options?.isInitialPlaceholder ?? false
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
