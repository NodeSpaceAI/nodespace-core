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
import { ContentProcessor } from './contentProcessor';
import { eventBus } from './eventBus';
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

  // Helper function to sort children according to beforeSiblingId linked list
  function sortChildrenByBeforeSiblingId(childIds: string[]): string[] {
    if (childIds.length === 0) return [];

    // Build a map of beforeSiblingId -> nodeId for quick lookup
    const beforeSiblingMap = new Map<string | null, string>();
    for (const childId of childIds) {
      const child = _nodes[childId];
      if (child) {
        beforeSiblingMap.set(child.beforeSiblingId, childId);
      }
    }

    // Find the first child (the one with no beforeSiblingId or whose beforeSiblingId is not in this sibling set)
    let firstChildId: string | null = null;
    for (const childId of childIds) {
      const child = _nodes[childId];
      if (child) {
        // A node is first if its beforeSiblingId is null or not in the sibling set
        if (child.beforeSiblingId === null || !childIds.includes(child.beforeSiblingId)) {
          firstChildId = childId;
          break;
        }
      }
    }

    if (!firstChildId) {
      // Fallback: return unsorted if we can't find a first child
      return childIds;
    }

    // Build sorted list by following the linked list
    const sorted: string[] = [];
    let currentId: string | null = firstChildId;
    const visited = new Set<string>();

    while (currentId && visited.size < childIds.length) {
      if (visited.has(currentId)) {
        // Circular reference detected, break
        break;
      }
      visited.add(currentId);
      sorted.push(currentId);

      // Find next sibling (the one whose beforeSiblingId points to currentId)
      const nextId = beforeSiblingMap.get(currentId);
      currentId = nextId || null;
    }

    // Add any remaining children that weren't in the linked list
    for (const childId of childIds) {
      if (!visited.has(childId)) {
        sorted.push(childId);
      }
    }

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
        const children = sortChildrenByBeforeSiblingId(childIds);

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
      viewRoots = sortChildrenByBeforeSiblingId(childIds);
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
      rootId = parent?.originNodeId || newParentId;
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
      originNodeId: rootId,
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

    events.nodeCreated(nodeId);
    events.hierarchyChanged();

    eventBus.emit<import('./eventTypes').NodeCreatedEvent>({
      type: 'node:created',
      namespace: 'lifecycle',
      source: serviceName,
      nodeId,
      nodeType,
      metadata: {}
    });

    eventBus.emit<import('./eventTypes').HierarchyChangedEvent>({
      type: 'hierarchy:changed',
      namespace: 'lifecycle',
      source: serviceName,
      changeType: 'create',
      affectedNodes: [nodeId]
    });

    eventBus.emit<import('./eventTypes').CacheInvalidateEvent>({
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
    eventBus.emit<import('./eventTypes').ReferencesUpdateNeededEvent>({
      type: 'references:update-needed',
      namespace: 'coordination',
      source: serviceName,
      nodeId,
      updateType: 'content',
      metadata: {}
    });
  }

  function emitExpensivePersistenceNeeded(nodeId: string, content: string): void {
    eventBus.emit<import('./eventTypes').NodePersistenceEvent>({
      type: 'node:persistence-needed',
      namespace: 'backend',
      source: serviceName,
      nodeId,
      content,
      metadata: {}
    });
  }

  function emitVectorEmbeddingNeeded(nodeId: string, content: string): void {
    eventBus.emit<import('./eventTypes').NodeEmbeddingEvent>({
      type: 'node:embedding-needed',
      namespace: 'ai',
      source: serviceName,
      nodeId,
      content,
      metadata: {}
    });
  }

  function emitReferencePropagatationNeeded(nodeId: string, content: string): void {
    eventBus.emit<import('./eventTypes').NodeReferencePropagationEvent>({
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

    // const isChildToParent = currentNode.parentId === previousNodeId;
    const cleanedContent = stripFormattingSyntax(currentNode.content);
    const combinedContent = previousNode.content + cleanedContent;
    const mergePosition = previousNode.content.length;

    _nodes[previousNodeId] = {
      ...previousNode,
      content: combinedContent,
      modifiedAt: new Date().toISOString()
    };
    _uiState[previousNodeId] = { ..._uiState[previousNodeId], autoFocus: false };

    // Handle child promotion if needed
    const currentChildren = Object.values(_nodes)
      .filter((n) => n.parentId === currentNodeId)
      .map((n) => n.id);

    // Determine where children should go:
    // - If nodes are at same depth/level, transfer to target (previousNode)
    // - If there's a depth mismatch, promote to source's parent
    const currentUIState = _uiState[currentNodeId];
    const previousUIState = _uiState[previousNodeId];
    const sameDepthLevel = currentUIState?.depth === previousUIState?.depth;

    const newParentForChildren = sameDepthLevel ? previousNodeId : currentNode.parentId;

    for (const childId of currentChildren) {
      const child = _nodes[childId];
      if (child) {
        _nodes[childId] = {
          ...child,
          parentId: newParentForChildren,
          modifiedAt: new Date().toISOString()
        };

        // Update depth based on new parent
        const newParentUIState = newParentForChildren ? _uiState[newParentForChildren] : null;
        const newDepth = newParentUIState ? newParentUIState.depth + 1 : 0;
        _uiState[childId] = {
          ..._uiState[childId],
          depth: newDepth
        };

        // Recursively update descendant depths
        updateDescendantDepths(childId);
      }
    }

    delete _nodes[currentNodeId];
    delete _uiState[currentNodeId];

    const rootIndex = _rootNodeIds.indexOf(currentNodeId);
    if (rootIndex >= 0) {
      _rootNodeIds.splice(rootIndex, 1);
    }

    clearAllAutoFocus();
    events.focusRequested(previousNodeId, mergePosition);
    events.hierarchyChanged();
  }

  function stripFormattingSyntax(content: string): string {
    let cleaned = content;
    cleaned = cleaned.replace(/^#{1,6}\s+/, '');
    cleaned = cleaned.replace(/^\[\s*[x\s]*\]\s*/, '');
    return cleaned.trim();
  }

  function indentNode(nodeId: string): boolean {
    const node = _nodes[nodeId];
    if (!node) return false;

    let siblings: string[];
    let nodeIndex: number;

    if (node.parentId) {
      siblings = Object.values(_nodes)
        .filter((n) => n.parentId === node.parentId)
        .map((n) => n.id);
      nodeIndex = siblings.indexOf(nodeId);
    } else {
      siblings = _rootNodeIds;
      nodeIndex = siblings.indexOf(nodeId);
    }

    if (nodeIndex === 0) return false;

    const prevSiblingId = siblings[nodeIndex - 1];
    const prevSibling = _nodes[prevSiblingId];
    if (!prevSibling) return false;

    const uiState = _uiState[nodeId];
    const prevSiblingUIState = _uiState[prevSiblingId];

    _nodes[nodeId] = {
      ...node,
      parentId: prevSiblingId,
      beforeSiblingId: null, // Becomes first child of new parent
      modifiedAt: new Date().toISOString()
    };
    _uiState[nodeId] = { ...uiState, depth: (prevSiblingUIState?.depth || 0) + 1 };

    // Recalculate depths for all descendants
    updateDescendantDepths(nodeId);

    if (node.parentId) {
      // Remove from old parent's children list
    } else {
      _rootNodeIds.splice(nodeIndex, 1);
    }

    events.hierarchyChanged();
    _updateTrigger++;

    eventBus.emit<import('./eventTypes').HierarchyChangedEvent>({
      type: 'hierarchy:changed',
      namespace: 'lifecycle',
      source: serviceName,
      changeType: 'indent',
      affectedNodes: [nodeId]
    });

    eventBus.emit<import('./eventTypes').ReferencesUpdateNeededEvent>({
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

    const newParentId = parent.parentId || null;
    const uiState = _uiState[nodeId];
    const newDepth = newParentId ? (_uiState[newParentId]?.depth || 0) + 1 : 0;

    _nodes[nodeId] = {
      ...node,
      parentId: newParentId,
      beforeSiblingId: node.parentId, // Positioned right after old parent
      modifiedAt: new Date().toISOString()
    };
    _uiState[nodeId] = { ...uiState, depth: newDepth };

    // Recalculate depths for all descendants
    updateDescendantDepths(nodeId);

    if (!newParentId) {
      const parentIndex = _rootNodeIds.indexOf(parent.id);
      _rootNodeIds.splice(parentIndex + 1, 0, nodeId);
    }

    events.hierarchyChanged();
    _updateTrigger++;

    eventBus.emit<import('./eventTypes').HierarchyChangedEvent>({
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

    events.nodeDeleted(nodeId);
    events.hierarchyChanged();
    _updateTrigger++;

    eventBus.emit<import('./eventTypes').NodeDeletedEvent>({
      type: 'node:deleted',
      namespace: 'lifecycle',
      source: serviceName,
      nodeId,
      parentId: node.parentId || undefined
    });

    eventBus.emit<import('./eventTypes').HierarchyChangedEvent>({
      type: 'hierarchy:changed',
      namespace: 'lifecycle',
      source: serviceName,
      changeType: 'delete',
      affectedNodes: [nodeId, ...children]
    });

    eventBus.emit<import('./eventTypes').CacheInvalidateEvent>({
      type: 'cache:invalidate',
      namespace: 'coordination',
      source: serviceName,
      cacheKey: 'all',
      scope: 'global',
      reason: 'node-deleted'
    });

    eventBus.emit<import('./eventTypes').ReferencesUpdateNeededEvent>({
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

      const status: import('./eventTypes').NodeStatus = newExpandedState ? 'expanded' : 'collapsed';
      const changeType: import('./eventTypes').HierarchyChangedEvent['changeType'] =
        newExpandedState ? 'expand' : 'collapse';

      eventBus.emit<import('./eventTypes').NodeStatusChangedEvent>({
        type: 'node:status-changed',
        namespace: 'coordination',
        source: serviceName,
        nodeId,
        status
      });

      eventBus.emit<import('./eventTypes').HierarchyChangedEvent>({
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
    eventBus.emit<import('./eventTypes').NodeUpdatedEvent>({
      type: 'node:updated',
      namespace: 'lifecycle',
      source: serviceName,
      nodeId,
      updateType: updateType as import('./eventTypes').NodeUpdatedEvent['updateType'],
      newValue
    });

    if (updateType === 'content') {
      eventBus.emit<import('./eventTypes').DecorationUpdateNeededEvent>({
        type: 'decoration:update-needed',
        namespace: 'interaction',
        source: serviceName,
        nodeId,
        decorationType: 'content',
        reason: 'content-changed',
        metadata: {}
      });

      eventBus.emit<import('./eventTypes').CacheInvalidateEvent>({
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
