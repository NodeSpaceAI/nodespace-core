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
import { sharedNodeStore } from './shared-node-store';
import type { Node, NodeUIState } from '$lib/types';
import { createDefaultUIState } from '$lib/types';
import type { UpdateSource } from '$lib/types/update-protocol';

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

  // UI state only - node data stored in SharedNodeStore
  const _uiState = $state<Record<string, NodeUIState>>({});
  let _rootNodeIds = $state<string[]>([]);
  const _activeNodeId = $state<string | undefined>(undefined);

  // View context: which parent are we viewing? (null = viewing global roots)
  let _viewParentId = $state<string | null>(null);

  // Manual reactivity trigger - incremented when SharedNodeStore updates
  let _updateTrigger = $state(0);

  const serviceName = 'ReactiveNodeService';
  const contentProcessor = ContentProcessor.getInstance();

  // Subscribe to SharedNodeStore changes for reactive updates
  // Wildcard subscription - updates _updateTrigger when any node changes
  // Note: Unsubscribe handled automatically when service instance is garbage collected
  const _unsubscribe = sharedNodeStore.subscribeAll(() => {
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

        // Merge Node with UI state for components
        result.push({
          ...node,
          depth: uiState?.depth || 0,
          children,
          expanded: uiState?.expanded || false,
          autoFocus: uiState?.autoFocus || false,
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

  // REACTIVITY FIX: Properly reactive visibleNodes computation using $derived.by
  const _visibleNodes = $derived.by(() => {
    // Track SharedNodeStore updates via _updateTrigger
    void _updateTrigger;
    void _rootNodeIds;
    void _viewParentId;

    // Determine which nodes are "roots" for this view
    // If viewParentId is set, roots are nodes with parent_id === viewParentId
    // If viewParentId is null, roots are nodes with no parent_id
    let viewRoots: string[];
    if (_viewParentId !== null) {
      // Get children from SharedNodeStore
      const childIds = sharedNodeStore.getNodesForParent(_viewParentId).map((n) => n.id);
      // Sort children according to beforeSiblingId linked list
      viewRoots = sortChildrenByBeforeSiblingId(childIds, _viewParentId);
    } else {
      viewRoots = _rootNodeIds;
    }

    return getVisibleNodesRecursive(viewRoots);
  });

  function findNode(nodeId: string): Node | null {
    return sharedNodeStore.getNode(nodeId) || null;
  }

  function clearAllAutoFocus(): void {
    for (const [nodeId, state] of Object.entries(_uiState)) {
      if (state.autoFocus) {
        _uiState[nodeId] = { ...state, autoFocus: false };
      }
    }
  }

  function createNode(
    afterNodeId: string,
    content: string = '',
    nodeType: string = 'text',
    headerLevel?: number,
    insertAtBeginning?: boolean,
    originalNodeContent?: string,
    focusNewNode?: boolean
  ): string {
    const afterNode = findNode(afterNodeId);
    if (!afterNode) {
      return '';
    }

    clearAllAutoFocus();

    const nodeId = uuidv4();
    const afterUIState = _uiState[afterNodeId] || createDefaultUIState(afterNodeId);
    let newDepth: number;
    let newParentId: string | null;

    if (insertAtBeginning) {
      newDepth = afterUIState.depth;
      newParentId = afterNode.parentId;
    } else {
      newDepth = afterUIState.depth;
      newParentId = afterNode.parentId;
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

    /**
     * Determine containerNodeId - tracks which root document this node belongs to
     *
     * Rules:
     * - containerNodeId = null only for root nodes themselves
     * - All other nodes should have containerNodeId pointing to their root document
     *
     * Logic:
     * - If node has explicit parent → inherit containerNodeId from parent (or use parent's ID if parent has none)
     * - If afterNode is a root (containerNodeId = null) → use afterNode's ID as container
     * - Otherwise → inherit afterNode's containerNodeId
     *
     * This ensures proper FOREIGN KEY relationships and allows database queries
     * to efficiently find all nodes belonging to a specific root document.
     */
    let rootId: string | null;
    if (newParentId) {
      // Node has explicit parent - inherit containerNodeId from parent
      const parent = sharedNodeStore.getNode(newParentId);
      rootId = parent?.containerNodeId || newParentId;
    } else {
      // No explicit parent - check if afterNode is a root node or belongs to a root
      if (afterNode.containerNodeId === null) {
        // afterNode IS a root document, so new node belongs to that root
        rootId = afterNodeId;
      } else {
        // afterNode belongs to a root, inherit that root
        rootId = afterNode.containerNodeId;
      }
    }

    // Create Node with unified type system
    const newNode: Node = {
      id: nodeId,
      nodeType: nodeType,
      content: initialContent,
      parentId: newParentId,
      containerNodeId: rootId,
      beforeSiblingId: beforeSiblingId,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {},
      mentions: []
    };

    // Create UI state
    const newUIState = createDefaultUIState(nodeId, {
      depth: newDepth,
      expanded: true,
      autoFocus: shouldFocusNewNode,
      inheritHeaderLevel: headerLevel !== undefined ? headerLevel : afterUIState.inheritHeaderLevel,
      isPlaceholder
    });

    // Find next sibling BEFORE adding new node to prevent interference
    const siblings = sharedNodeStore.getNodesForParent(newParentId);
    const nextSibling = !insertAtBeginning
      ? siblings.find((n) => n.beforeSiblingId === afterNodeId)
      : null;

    sharedNodeStore.setNode(newNode, viewerSource);
    _uiState[nodeId] = newUIState;

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
          // IMPORTANT: Update both parentId AND containerNodeId
          // Rule: containerNodeId = parentId for direct children of root nodes
          for (const child of children) {
            sharedNodeStore.updateNode(
              child.id,
              { parentId: nodeId, containerNodeId: newNode.containerNodeId || nodeId },
              viewerSource
            );
          }
        });
      }
    }

    // Handle hierarchy positioning
    if (insertAtBeginning) {
      if (afterNode.parentId) {
        // const siblings = Object.values(_nodes)
        //   .filter(n => n.parentId === afterNode.parentId)
        //   .map(n => n.id);
        // const afterNodeIndex = siblings.indexOf(afterNodeId);
        // Update sibling pointers (simplified for now)
      } else {
        const afterNodeIndex = _rootNodeIds.indexOf(afterNodeId);
        _rootNodeIds = [
          ..._rootNodeIds.slice(0, afterNodeIndex),
          nodeId,
          ..._rootNodeIds.slice(afterNodeIndex)
        ];
      }
    } else {
      if (afterNode.parentId) {
        // Insert after sibling
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

    if (!shouldFocusNewNode && afterNode) {
      _uiState[afterNodeId] = { ..._uiState[afterNodeId], autoFocus: true };
    }

    return nodeId;
  }

  function createPlaceholderNode(
    afterNodeId: string,
    nodeType: string = 'text',
    headerLevel?: number,
    insertAtBeginning: boolean = false,
    originalNodeContent?: string,
    focusNewNode?: boolean
  ): string {
    return createNode(
      afterNodeId,
      '',
      nodeType,
      headerLevel,
      insertAtBeginning,
      originalNodeContent,
      focusNewNode
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

    sharedNodeStore.updateNode(nodeId, { content }, viewerSource);

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

    sharedNodeStore.updateNode(nodeId, { nodeType }, viewerSource);
    _uiState[nodeId] = { ..._uiState[nodeId], autoFocus: true };

    _updateTrigger++;

    emitNodeUpdated(nodeId, 'nodeType', nodeType);

    setTimeout(() => {
      const state = _uiState[nodeId];
      if (state?.autoFocus) {
        _uiState[nodeId] = { ...state, autoFocus: false };
      }
    }, 250);

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

  function combineNodes(currentNodeId: string, previousNodeId: string): void {
    const currentNode = sharedNodeStore.getNode(currentNodeId);
    const previousNode = sharedNodeStore.getNode(previousNodeId);

    if (!currentNode || !previousNode) return;

    const cleanedContent = stripFormattingSyntax(currentNode.content);
    const combinedContent = previousNode.content + cleanedContent;
    const mergePosition = previousNode.content.length;

    // Use external source to ensure content update persists
    // (using viewerSource would skip persistence for content-only changes per line 254 in shared-node-store.ts)
    const contentUpdateSource = {
      type: 'external' as const,
      source: 'ReactiveNodeService',
      description: 'combineNodes content merge'
    };
    sharedNodeStore.updateNode(previousNodeId, { content: combinedContent }, contentUpdateSource);
    _uiState[previousNodeId] = { ..._uiState[previousNodeId], autoFocus: false };

    // Handle child promotion using shared depth-aware logic
    promoteChildren(currentNodeId, previousNodeId);

    // CRITICAL: Collect dependencies that must persist before deletion
    // This prevents "database is locked" errors and FOREIGN KEY violations
    // PersistenceCoordinator will ensure these operations complete before deletion
    const deletionDependencies = [previousNodeId]; // Content update must persist first
    const siblings = sharedNodeStore.getNodesForParent(currentNode.parentId);
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
    invalidateSortedChildrenCache(currentNode.parentId);
    invalidateSortedChildrenCache(currentNodeId);

    clearAllAutoFocus();
    events.focusRequested(previousNodeId, mergePosition);
    events.nodeDeleted(currentNodeId);
    events.hierarchyChanged();
  }

  function stripFormattingSyntax(content: string): string {
    let cleaned = content;
    cleaned = cleaned.replace(/^#{1,6}\s+/, '');
    cleaned = cleaned.replace(/^\[\s*[x\s]*\]\s*/, '');
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
    const siblings = sharedNodeStore.getNodesForParent(node.parentId);
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
    let siblings: string[];
    if (node.parentId) {
      const unsortedSiblings = sharedNodeStore.getNodesForParent(node.parentId).map((n) => n.id);
      siblings = sortChildrenByBeforeSiblingId(unsortedSiblings, node.parentId);
    } else {
      siblings = sortChildrenByBeforeSiblingId(_rootNodeIds, null);
    }

    const nodeIndex = siblings.indexOf(nodeId);
    if (nodeIndex <= 0) return false; // Can't indent first child or if not found

    const prevSiblingId = siblings[nodeIndex - 1];
    const prevSibling = sharedNodeStore.getNode(prevSiblingId);
    if (!prevSibling) return false;

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
    // Set containerNodeId to the root container when indenting
    // This is required for FOREIGN KEY validation and proper nesting
    let containerNodeId: string | null = null;
    if (targetParentId) {
      // Walk up to find the root container
      let current = sharedNodeStore.getNode(targetParentId);
      while (current) {
        if (!current.parentId) {
          // Reached root level - this node is the container
          containerNodeId = current.id;
          break;
        }
        if (current.containerNodeId) {
          // This node has an explicit container - use it
          containerNodeId = current.containerNodeId;
          break;
        }
        // Move up the chain
        current = current.parentId ? sharedNodeStore.getNode(current.parentId) : undefined;
      }
    }

    sharedNodeStore.updateNode(
      nodeId,
      {
        parentId: targetParentId,
        containerNodeId: containerNodeId,
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
    if (!node.parentId) {
      const rootIndex = _rootNodeIds.indexOf(nodeId);
      if (rootIndex >= 0) {
        _rootNodeIds = _rootNodeIds.filter((id) => id !== nodeId);
      }
    }

    // Invalidate sorted children cache for both old and new parents
    invalidateSortedChildrenCache(node.parentId); // Old parent
    invalidateSortedChildrenCache(targetParentId); // New parent

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
    if (!node || !node.parentId) return false;

    const parent = sharedNodeStore.getNode(node.parentId);
    if (!parent) return false;

    const oldParentId = node.parentId;

    // Find siblings that come after this node (they will become children)
    const siblings = sharedNodeStore.getNodesForParent(oldParentId).map((n) => n.id);
    const sortedSiblings = sortChildrenByBeforeSiblingId(siblings, oldParentId);
    const nodeIndex = sortedSiblings.indexOf(nodeId);
    const siblingsBelow = nodeIndex >= 0 ? sortedSiblings.slice(nodeIndex + 1) : [];

    // Step 1: Prepare for parent change
    // Remove node from current sibling chain to maintain chain integrity
    const updatedSiblingFromRemoval = removeFromSiblingChain(nodeId);

    // Step 2: Calculate new position in parent hierarchy
    const newParentId = parent.parentId || null;
    const uiState = _uiState[nodeId];
    const newDepth = newParentId ? (_uiState[newParentId]?.depth || 0) + 1 : 0;

    // Find where to position the outdented node in its new parent's child list
    // It should come right after its old parent (which is now a sibling)
    let positionBeforeSibling: string | null = oldParentId;

    // Check if old parent has a valid position in the new parent's context
    const oldParentNode = sharedNodeStore.getNode(oldParentId);
    if (oldParentNode && oldParentNode.parentId === newParentId) {
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
    sharedNodeStore.updateNode(
      nodeId,
      {
        parentId: newParentId,
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
          sharedNodeStore.updateNode(
            siblingId,
            {
              parentId: nodeId,
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
    const deletedNode = sharedNodeStore.getNode(nodeId);
    const deletedNodeDepth = _uiState[nodeId]?.depth ?? 0;

    // CRITICAL: If deleted node is at root level (parentId=null), promote children to root level too
    // Don't make them children of the previous sibling - maintain the flat root structure
    let newParentForChildren: string | null;
    if (deletedNode?.parentId === null) {
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
        const searchNodeData = sharedNodeStore.getNode(searchNode);
        searchNode = searchNodeData?.parentId ?? null;
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
      // Determine the new parent node to check its containerNodeId
      const newParent = newParentForChildren ? sharedNodeStore.getNode(newParentForChildren) : null;

      const updates: Partial<Node> = {
        parentId: newParentForChildren,
        // CRITICAL: Update containerNodeId to prevent FOREIGN KEY violations when old container is deleted
        // Container should match the new parent's container (for nested nodes) or null (for root-level)
        containerNodeId: newParent?.containerNodeId ?? null
      };

      if (child.id === firstChildId) {
        updates.beforeSiblingId = lastSiblingId;
      }

      sharedNodeStore.updateNode(child.id, updates, viewerSource);

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

      // CRITICAL: Recursively fix containerNodeId for all descendants
      // When promoting a child, all of its descendants still have containerNodeId pointing to the old deleted node
      // This would cause FOREIGN KEY violations when the old container is deleted
      const newContainerId = newParent?.containerNodeId ?? null;
      fixDescendantContainerIds(child.id, nodeId, newContainerId);
    }
  }

  /**
   * Recursively updates containerNodeId for all descendants that reference an old container
   * This prevents FOREIGN KEY violations when deleting a node that was a container for nested descendants
   *
   * @param rootId - The root node whose descendants to update
   * @param oldContainerId - The old container ID to replace
   * @param newContainerId - The new container ID to use
   */
  function fixDescendantContainerIds(
    rootId: string,
    oldContainerId: string,
    newContainerId: string | null
  ): void {
    const descendants = sharedNodeStore.getNodesForParent(rootId);

    for (const descendant of descendants) {
      // Only update if this descendant currently references the old container
      if (descendant.containerNodeId === oldContainerId) {
        sharedNodeStore.updateNode(
          descendant.id,
          { containerNodeId: newContainerId },
          viewerSource
        );
      }

      // Recurse to fix deeper descendants
      fixDescendantContainerIds(descendant.id, oldContainerId, newContainerId);
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

    cleanupDebouncedOperations(nodeId);

    // Remove node from sibling chain BEFORE deletion to prevent orphans
    removeFromSiblingChain(nodeId);

    sharedNodeStore.deleteNode(nodeId, viewerSource);
    delete _uiState[nodeId];

    // CRITICAL: Must reassign (not mutate) for Svelte 5 reactivity
    const rootIndex = _rootNodeIds.indexOf(nodeId);
    if (rootIndex >= 0) {
      _rootNodeIds = _rootNodeIds.filter((id) => id !== nodeId);
    }

    // Invalidate sorted children cache for parent
    invalidateSortedChildrenCache(node.parentId);
    // Also invalidate cache for the deleted node itself (in case it's still referenced)
    invalidateSortedChildrenCache(nodeId);

    events.nodeDeleted(nodeId);
    events.hierarchyChanged();
    _updateTrigger++;

    eventBus.emit<import('./event-types').NodeDeletedEvent>({
      type: 'node:deleted',
      namespace: 'lifecycle',
      source: serviceName,
      nodeId,
      parentId: node.parentId || undefined
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
    get visibleNodes() {
      return _visibleNodes;
    },
    get viewParentId() {
      return _viewParentId;
    },
    get _updateTrigger() {
      return _updateTrigger;
    },
    // Direct access to UI state for computed properties
    getUIState(nodeId: string) {
      return _uiState[nodeId];
    },

    // View context control
    setViewParentId(parentId: string | null) {
      _viewParentId = parentId;
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
      }
    ): void {
      // Clear existing unpersisted placeholder nodes for the current view parent from SharedNodeStore
      // This prevents duplicate placeholder nodes when navigating between different parent contexts
      // Only deletes empty text placeholders (not persisted to database) to maintain multi-tab safety
      // TODO: Implement proper reference counting for full multi-tab/multi-pane support (see Issue #206)
      if (_viewParentId !== null) {
        const existingNodes = sharedNodeStore.getNodesForParent(_viewParentId);
        const databaseSource = { type: 'database' as const, reason: 'view-cleanup' };
        for (const node of existingNodes) {
          // Only delete unpersisted placeholders (empty text nodes)
          // Persisted nodes will be reloaded from database if needed
          if (node.content === '' && node.nodeType === 'text') {
            sharedNodeStore.deleteNode(node.id, databaseSource);
          }
        }
      }

      // Clear existing state
      Object.keys(_uiState).forEach((id) => delete _uiState[id]);
      _rootNodeIds = [];

      const defaults = {
        expanded: options?.expanded ?? true,
        autoFocus: options?.autoFocus ?? false,
        inheritHeaderLevel: options?.inheritHeaderLevel ?? 0
      };

      // Compute depth for a node based on parent chain
      const computeDepth = (nodeId: string, visited = new Set<string>()): number => {
        if (visited.has(nodeId)) return 0; // Prevent infinite recursion
        visited.add(nodeId);

        const node = sharedNodeStore.getNode(nodeId);
        if (!node || !node.parentId) return 0;
        return 1 + computeDepth(node.parentId, visited);
      };

      // First pass: Add all nodes to SharedNodeStore
      // Use database source to prevent write-back (nodes are already in database)
      const databaseSource = { type: 'database' as const, reason: 'initialization' };
      for (const node of nodes) {
        sharedNodeStore.setNode(node, databaseSource);
        _uiState[node.id] = createDefaultUIState(node.id, {
          depth: 0, // Will be computed in second pass
          expanded: defaults.expanded,
          autoFocus: defaults.autoFocus,
          inheritHeaderLevel: defaults.inheritHeaderLevel,
          isPlaceholder: false
        });
      }

      // Second pass: Compute depths and identify roots
      const childIds = new Set(nodes.filter((n) => n.parentId).map((n) => n.id));
      _rootNodeIds = nodes.filter((n) => !n.parentId || !childIds.has(n.id)).map((n) => n.id);

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
