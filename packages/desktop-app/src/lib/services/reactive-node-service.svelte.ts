/**
 * ReactiveNodeService - Pure Svelte 5 Runes Reactive Architecture
 *
 * Uses unified Node type from $lib/types with separated UI state.
 *
 * Key features:
 * - Uses Node from $lib/types (with node_type, properties, etc.)
 * - Separates data (Node) from UI state (NodeUIState)
 * - Computes children on-demand instead of storing arrays
 * - Direct access to Node objects for mutations
 */

import { v4 as uuidv4 } from 'uuid';
import { ContentProcessor } from './content-processor';
import { eventBus } from './event-bus';
import type { Node, NodeUIState } from '$lib/types';
import { createDefaultUIState } from '$lib/types';

export interface NodeManagerEvents {
  focusRequested: (nodeId: string, position?: number) => void;
  hierarchyChanged: () => void;
  nodeCreated: (nodeId: string) => void;
  nodeDeleted: (nodeId: string) => void;
}

export function createReactiveNodeService(events: NodeManagerEvents) {
  // Separated data and UI state - following unified type system
  const _nodes = $state<Record<string, Node>>({});
  const _uiState = $state<Record<string, NodeUIState>>({});
  let _rootNodeIds = $state<string[]>([]);
  const _activeNodeId = $state<string | undefined>(undefined);

  // View context: which parent are we viewing? (null = viewing global roots)
  let _viewParentId = $state<string | null>(null);

  // Manual reactivity trigger for debugging
  let _updateTrigger = $state(0);

  const serviceName = 'ReactiveNodeService';
  const contentProcessor = ContentProcessor.getInstance();

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
      const child = _nodes[childId];
      if (child) {
        beforeSiblingMap.set(child.beforeSiblingId, childId);
      }
    }

    // Find ALL candidates for first child to detect data corruption
    const firstChildCandidates: string[] = [];
    for (const childId of childIds) {
      const child = _nodes[childId];
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
          beforeSiblingId: _nodes[id]?.beforeSiblingId
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
            beforeSiblingId: _nodes[id]?.beforeSiblingId
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
            beforeSiblingId: _nodes[id]?.beforeSiblingId
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
      const node = _nodes[nodeId];
      const uiState = _uiState[nodeId];
      if (node) {
        const childIds = Object.values(_nodes)
          .filter((n) => n.parentId === nodeId)
          .map((n) => n.id);

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
    const allNodes = Object.values(_nodes);
    for (const node of allNodes) {
      void node.nodeType;
    }
    void _updateTrigger;
    void _rootNodeIds;
    void _viewParentId;

    // Determine which nodes are "roots" for this view
    // If viewParentId is set, roots are nodes with parent_id === viewParentId
    // If viewParentId is null, roots are nodes with no parent_id
    let viewRoots: string[];
    if (_viewParentId !== null) {
      const childIds = Object.values(_nodes)
        .filter((n) => n.parentId === _viewParentId)
        .map((n) => n.id);
      // Sort children according to beforeSiblingId linked list
      viewRoots = sortChildrenByBeforeSiblingId(childIds, _viewParentId);
    } else {
      viewRoots = _rootNodeIds;
    }

    return getVisibleNodesRecursive(viewRoots);
  });

  function findNode(nodeId: string): Node | null {
    return _nodes[nodeId] || null;
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

    // Determine origin_node_id - inherit from parent or use own id if no parent
    let rootId: string;
    if (newParentId) {
      const parent = _nodes[newParentId];
      // Inherit origin_node_id from parent, or use parent's id if parent has no origin_node_id
      rootId = parent?.containerNodeId || newParentId;
    } else {
      // No parent means this node is the root
      rootId = nodeId;
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
    const siblings = Object.values(_nodes).filter((n) => n.parentId === newParentId);
    const nextSibling = !insertAtBeginning
      ? siblings.find((n) => n.beforeSiblingId === afterNodeId)
      : null;

    _nodes[nodeId] = newNode;
    _uiState[nodeId] = newUIState;

    // Update sibling linked list
    if (insertAtBeginning) {
      // New node takes afterNode's place, so afterNode now comes after newNode
      _nodes[afterNodeId] = {
        ...afterNode,
        beforeSiblingId: nodeId,
        modifiedAt: new Date().toISOString()
      };
    } else {
      // New node inserted after afterNode - update next sibling to point to new node
      if (nextSibling) {
        _nodes[nextSibling.id] = {
          ...nextSibling,
          beforeSiblingId: nodeId,
          modifiedAt: new Date().toISOString()
        };
      }
    }

    // Bug 4 fix: Transfer children from expanded nodes
    // If afterNode is expanded and has children, transfer them to the new node
    if (!insertAtBeginning && afterUIState.expanded) {
      const children = Object.values(_nodes)
        .filter((n) => n.parentId === afterNodeId)
        .map((n) => n.id);

      if (children.length > 0) {
        // Transfer children to new node
        for (const childId of children) {
          const child = _nodes[childId];
          if (child) {
            _nodes[childId] = {
              ...child,
              parentId: nodeId,
              modifiedAt: new Date().toISOString()
            };
          }
        }
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
    const node = _nodes[nodeId];
    if (!node) return;

    const headerLevel = contentProcessor.parseHeaderLevel(content);
    const isPlaceholder = content.trim() === '';

    _nodes[nodeId] = {
      ...node,
      content,
      modifiedAt: new Date().toISOString()
    };

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
    const node = _nodes[nodeId];
    if (!node) return;

    _nodes[nodeId] = {
      ...node,
      nodeType: nodeType,
      modifiedAt: new Date().toISOString()
    };
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
    const node = _nodes[nodeId];
    if (!node) return;

    _nodes[nodeId] = {
      ...node,
      mentions: [...mentions],
      modifiedAt: new Date().toISOString()
    };

    _updateTrigger++;
    emitNodeUpdated(nodeId, 'mentions', mentions);
  }

  function updateNodeProperties(
    nodeId: string,
    properties: Record<string, unknown>,
    merge: boolean = true
  ): void {
    const node = _nodes[nodeId];
    if (!node) return;

    _nodes[nodeId] = {
      ...node,
      properties: merge ? { ...node.properties, ...properties } : { ...properties },
      modifiedAt: new Date().toISOString()
    };

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
    const node = _nodes[nodeId];
    if (!node) return;

    emitReferenceUpdateNeeded(nodeId);

    const operations = debouncedOperations.get(nodeId);
    if (operations) {
      operations.fastTimer = undefined;
      debouncedOperations.set(nodeId, operations);
    }
  }

  function processExpensiveContentOperations(nodeId: string, content: string): void {
    const node = _nodes[nodeId];
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
    const currentNode = _nodes[currentNodeId];
    const previousNode = _nodes[previousNodeId];

    if (!currentNode || !previousNode) return;

    const cleanedContent = stripFormattingSyntax(currentNode.content);
    const combinedContent = previousNode.content + cleanedContent;
    const mergePosition = previousNode.content.length;

    _nodes[previousNodeId] = {
      ...previousNode,
      content: combinedContent,
      modifiedAt: new Date().toISOString()
    };
    _uiState[previousNodeId] = { ..._uiState[previousNodeId], autoFocus: false };

    // Handle child promotion: children shift up while maintaining outline structure
    // Get all children of the node being deleted
    const currentChildren = Object.values(_nodes)
      .filter((n) => n.parentId === currentNodeId)
      .map((n) => n.id);

    if (currentChildren.length > 0) {
      // Find the nearest ancestor node above the deleted node at the SAME depth,
      // walking up until we reach a node that is one or more levels higher
      // (because there are no other nodes at the same level)
      const deletedNodeDepth = _uiState[currentNodeId]?.depth ?? 0;
      let newParentForChildren = previousNodeId;
      let searchNode: string | null = previousNodeId;

      while (searchNode) {
        const searchDepth = _uiState[searchNode]?.depth ?? 0;
        if (searchDepth === deletedNodeDepth) {
          // Found a node at the same level as the deleted node
          newParentForChildren = searchNode;
          break;
        }
        if (searchDepth < deletedNodeDepth) {
          // Reached a node at a higher level (shallower), stop here
          newParentForChildren = searchNode;
          break;
        }
        // Keep walking up the tree
        const parentId: string | null | undefined = _nodes[searchNode]?.parentId;
        searchNode = parentId ?? null;
      }

      // Find existing children of the new parent to append after them
      const existingChildren = Object.values(_nodes)
        .filter((n) => n.parentId === newParentForChildren && !currentChildren.includes(n.id))
        .map((n) => n.id);

      let lastSiblingId: string | null = null;
      if (existingChildren.length > 0) {
        // Get sorted children to find the last one
        const sortedChildren = sortChildrenByBeforeSiblingId(
          existingChildren,
          newParentForChildren
        );
        lastSiblingId = sortedChildren[sortedChildren.length - 1];
      }

      // Find the first child in the deleted node's children (the one with beforeSiblingId pointing outside or null)
      const sortedDeletedChildren = sortChildrenByBeforeSiblingId(currentChildren, currentNodeId);
      const firstChildId = sortedDeletedChildren[0];

      // Process each child
      for (const childId of currentChildren) {
        const child = _nodes[childId];
        if (child) {
          // Only update the first child's beforeSiblingId to append after existing children
          // All other children keep their existing beforeSiblingId to maintain their chain
          const updates: Partial<typeof child> = {
            parentId: newParentForChildren,
            modifiedAt: new Date().toISOString()
          };

          if (childId === firstChildId) {
            updates.beforeSiblingId = lastSiblingId;
          }

          _nodes[childId] = {
            ...child,
            ...updates
          };

          // Note: Database persistence is handled by UI layer's $effect watchers
          // in base-node-viewer.svelte, which detects structural changes and persists them

          // Update depth: children maintain their relative position in the outline
          // Calculate target depth based on new parent
          const currentChildDepth = _uiState[childId]?.depth ?? 0;
          const targetDepth = newParentForChildren
            ? (_uiState[newParentForChildren]?.depth ?? 0) + 1
            : 0;

          // Preserve depth: never increase, only maintain or decrease
          // This ensures nodes shift UP in the outline, not down
          const newDepth = Math.min(currentChildDepth, targetDepth);

          _uiState[childId] = {
            ..._uiState[childId],
            depth: newDepth
          };

          // Recursively update descendant depths
          updateDescendantDepths(childId);
        }
      }
    }

    // Remove node from sibling chain BEFORE deletion to prevent orphans
    removeFromSiblingChain(currentNodeId);

    delete _nodes[currentNodeId];
    delete _uiState[currentNodeId];

    const rootIndex = _rootNodeIds.indexOf(currentNodeId);
    if (rootIndex >= 0) {
      _rootNodeIds.splice(rootIndex, 1);
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
   * Removes a node from its current sibling chain by updating the next sibling's beforeSiblingId.
   * This prevents orphaned nodes when a node is moved (indent/outdent) or deleted.
   *
   * @param nodeId - The node being removed from its current parent
   */
  function removeFromSiblingChain(nodeId: string): void {
    const node = _nodes[nodeId];
    if (!node) return;

    // Find the next sibling (the node that points to this one via beforeSiblingId)
    const siblings = Object.values(_nodes).filter((n) => n.parentId === node.parentId);
    const nextSibling = siblings.find((n) => n.beforeSiblingId === nodeId);

    if (nextSibling) {
      // Update next sibling to point to our predecessor, "splicing us out" of the chain
      _nodes[nextSibling.id] = {
        ...nextSibling,
        beforeSiblingId: node.beforeSiblingId,
        modifiedAt: new Date().toISOString()
      };
    }
  }

  function indentNode(nodeId: string): boolean {
    const node = _nodes[nodeId];
    if (!node) return false;

    // Get SORTED siblings according to beforeSiblingId chain
    let siblings: string[];
    if (node.parentId) {
      const unsortedSiblings = Object.values(_nodes)
        .filter((n) => n.parentId === node.parentId)
        .map((n) => n.id);
      siblings = sortChildrenByBeforeSiblingId(unsortedSiblings, node.parentId);
    } else {
      siblings = sortChildrenByBeforeSiblingId(_rootNodeIds, null);
    }

    const nodeIndex = siblings.indexOf(nodeId);
    if (nodeIndex <= 0) return false; // Can't indent first child or if not found

    const prevSiblingId = siblings[nodeIndex - 1];
    const prevSibling = _nodes[prevSiblingId];
    if (!prevSibling) return false;

    // Remove node from current sibling chain BEFORE changing parent
    removeFromSiblingChain(nodeId);

    // Find the last child of the new parent to insert after
    const existingChildren = Object.values(_nodes)
      .filter((n) => n.parentId === prevSiblingId)
      .map((n) => n.id);

    let beforeSiblingId: string | null = null;
    if (existingChildren.length > 0) {
      // Get sorted children and append after the last one
      const sortedChildren = sortChildrenByBeforeSiblingId(existingChildren, prevSiblingId);
      beforeSiblingId = sortedChildren[sortedChildren.length - 1]; // Insert after last child
    }

    const uiState = _uiState[nodeId];
    const prevSiblingUIState = _uiState[prevSiblingId];

    _nodes[nodeId] = {
      ...node,
      parentId: prevSiblingId,
      beforeSiblingId: beforeSiblingId, // Insert after last existing child (or null if no children)
      modifiedAt: new Date().toISOString()
    };
    _uiState[nodeId] = { ...uiState, depth: (prevSiblingUIState?.depth || 0) + 1 };

    // Recalculate depths for all descendants
    updateDescendantDepths(nodeId);

    // If node was a root node, remove from _rootNodeIds array
    if (!node.parentId) {
      const rootIndex = _rootNodeIds.indexOf(nodeId);
      if (rootIndex >= 0) {
        _rootNodeIds.splice(rootIndex, 1);
      }
    }

    // Invalidate sorted children cache for both old and new parents
    invalidateSortedChildrenCache(node.parentId); // Old parent
    invalidateSortedChildrenCache(prevSiblingId); // New parent

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
    const node = _nodes[nodeId];
    if (!node || !node.parentId) return false;

    const parent = _nodes[node.parentId];
    if (!parent) return false;

    const oldParentId = node.parentId;

    // Find siblings that come after this node (they will become children)
    const siblings = Object.values(_nodes)
      .filter((n) => n.parentId === oldParentId)
      .map((n) => n.id);
    const sortedSiblings = sortChildrenByBeforeSiblingId(siblings, oldParentId);
    const nodeIndex = sortedSiblings.indexOf(nodeId);
    const siblingsBelow = nodeIndex >= 0 ? sortedSiblings.slice(nodeIndex + 1) : [];

    // Remove node from current sibling chain BEFORE changing parent
    removeFromSiblingChain(nodeId);

    const newParentId = parent.parentId || null;
    const uiState = _uiState[nodeId];
    const newDepth = newParentId ? (_uiState[newParentId]?.depth || 0) + 1 : 0;

    // Find where to position the outdented node in its new parent's child list
    // It should come right after its old parent (which is now a sibling)
    let positionBeforeSibling: string | null = oldParentId;

    // Check if old parent has a valid position in the new parent's context
    const oldParentNode = _nodes[oldParentId];
    if (oldParentNode && oldParentNode.parentId === newParentId) {
      // Old parent is a valid sibling in the new context
      positionBeforeSibling = oldParentId;
    } else {
      // Old parent is not in the same parent context (shouldn't happen in normal outdent)
      // Position at the end by finding the last sibling
      const siblings = Object.values(_nodes)
        .filter((n) => n.parentId === newParentId && n.id !== nodeId)
        .map((n) => n.id);
      if (siblings.length > 0) {
        const sortedSiblings = sortChildrenByBeforeSiblingId(siblings, newParentId);
        positionBeforeSibling = sortedSiblings[sortedSiblings.length - 1];
      } else {
        positionBeforeSibling = null;
      }
    }

    _nodes[nodeId] = {
      ...node,
      parentId: newParentId,
      beforeSiblingId: positionBeforeSibling,
      modifiedAt: new Date().toISOString()
    };
    _uiState[nodeId] = { ...uiState, depth: newDepth };

    // Insert the outdented node into the new parent's sibling chain
    // Find any sibling in the new parent that currently points to positionBeforeSibling
    // and update it to point to the outdented node instead
    if (positionBeforeSibling !== null) {
      // Optimization: Only check direct siblings, not all nodes
      const siblingIds = Object.keys(_nodes).filter(
        (id) => id !== nodeId && _nodes[id].parentId === newParentId
      );

      for (const siblingId of siblingIds) {
        const sibling = _nodes[siblingId];
        if (sibling.beforeSiblingId === positionBeforeSibling) {
          _nodes[siblingId] = {
            ...sibling,
            beforeSiblingId: nodeId,
            modifiedAt: new Date().toISOString()
          };
          break; // Only one sibling can point to positionBeforeSibling
        }
      }
    }

    // Transfer siblings below the outdented node as its children
    // Need to rebuild their before_sibling_id chain to be valid in new parent context
    if (siblingsBelow.length > 0) {
      // Find existing children of the outdented node to append after them
      const existingChildren = Object.values(_nodes)
        .filter((n) => n.parentId === nodeId && !siblingsBelow.includes(n.id))
        .map((n) => n.id);

      let lastSiblingId: string | null = null;
      if (existingChildren.length > 0) {
        const sortedChildren = sortChildrenByBeforeSiblingId(existingChildren, nodeId);
        lastSiblingId = sortedChildren[sortedChildren.length - 1];
      }

      // Transfer each sibling, updating their before_sibling_id chain
      for (let i = 0; i < siblingsBelow.length; i++) {
        const siblingId = siblingsBelow[i];
        const sibling = _nodes[siblingId];
        if (sibling) {
          // Remove from current sibling chain BEFORE updating beforeSiblingId
          removeFromSiblingChain(siblingId);

          // First transferred sibling points to last existing child (or null)
          // Subsequent siblings point to the previous transferred sibling
          const beforeSiblingId = i === 0 ? lastSiblingId : siblingsBelow[i - 1];

          _nodes[siblingId] = {
            ...sibling,
            parentId: nodeId,
            beforeSiblingId,
            modifiedAt: new Date().toISOString()
          };
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

    if (!newParentId) {
      const parentIndex = _rootNodeIds.indexOf(parent.id);
      _rootNodeIds.splice(parentIndex + 1, 0, nodeId);
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

  function deleteNode(nodeId: string): void {
    const node = _nodes[nodeId];
    if (!node) return;

    cleanupDebouncedOperations(nodeId);

    // Remove node from sibling chain BEFORE deletion to prevent orphans
    removeFromSiblingChain(nodeId);

    // Promote children
    const children = Object.values(_nodes)
      .filter((n) => n.parentId === nodeId)
      .map((n) => n.id);

    for (const childId of children) {
      const child = _nodes[childId];
      if (child) {
        _nodes[childId] = {
          ...child,
          parentId: node.parentId,
          modifiedAt: new Date().toISOString()
        };
      }
    }

    delete _nodes[nodeId];
    delete _uiState[nodeId];

    const rootIndex = _rootNodeIds.indexOf(nodeId);
    if (rootIndex >= 0) {
      _rootNodeIds.splice(rootIndex, 1);
    }

    // Invalidate sorted children cache for parent (node removed, children promoted)
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
      affectedNodes: [nodeId, ...children]
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
    const node = _nodes[nodeId];
    const uiState = _uiState[nodeId];
    if (!node || !uiState) return;

    const children = Object.values(_nodes)
      .filter((n) => n.parentId === nodeId)
      .map((n) => n.id);

    for (const childId of children) {
      _uiState[childId] = {
        ..._uiState[childId],
        depth: uiState.depth + 1
      };
      updateDescendantDepths(childId);
    }
  }

  return {
    // Reactive getters
    get nodes() {
      return new Map(Object.entries(_nodes));
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
      const node = _nodes[nodeId];
      if (!node) return null;
      return contentProcessor.parseMarkdown(node.content);
    },

    async renderNodeAsHTML(nodeId: string): Promise<string> {
      const node = _nodes[nodeId];
      if (!node) return '';
      const result = await contentProcessor.markdownToDisplay(node.content);
      return result || '';
    },

    getNodeHeaderLevel(nodeId: string): number {
      const node = _nodes[nodeId];
      if (!node) return 0;
      const headerMatch = node.content.match(/^(#{1,6})\s+/);
      return headerMatch ? headerMatch[1].length : 0;
    },

    getNodeDisplayText(nodeId: string): string {
      const node = _nodes[nodeId];
      if (!node) return '';
      return contentProcessor
        .displayToMarkdown(node.content)
        .replace(/[#*`[\]()]/g, '')
        .trim();
    },

    updateNodeContentWithProcessing(nodeId: string, content: string): boolean {
      const node = _nodes[nodeId];
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
      // Clear existing state
      Object.keys(_nodes).forEach((id) => delete _nodes[id]);
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

        const node = _nodes[nodeId];
        if (!node || !node.parentId) return 0;
        return 1 + computeDepth(node.parentId, visited);
      };

      // First pass: Add all nodes
      for (const node of nodes) {
        _nodes[node.id] = node;
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

      for (const nodeId of Object.keys(_nodes)) {
        const depth = computeDepth(nodeId);
        _uiState[nodeId] = { ..._uiState[nodeId], depth };
      }

      _updateTrigger++;
      events.hierarchyChanged();
    }
  };
}

export type ReactiveNodeService = ReturnType<typeof createReactiveNodeService>;

// For backward compatibility with existing imports
export { createReactiveNodeService as ReactiveNodeService };
