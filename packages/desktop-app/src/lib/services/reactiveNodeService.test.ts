/**
 * Test-compatible version of ReactiveNodeService
 * Provides the same API without Svelte 5 runes dependency
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
  // Separated data and UI state - matching unified type system
  const _nodes: Record<string, Node> = {};
  const _uiState: Record<string, NodeUIState> = {};
  let _rootNodeIds: string[] = [];
  let _activeNodeId: string | undefined = undefined;

  // Manual reactivity trigger for debugging
  let _updateTrigger = 0;

  // Computed visible nodes - without Svelte reactivity
  function getVisibleNodes(): (Node & {
    depth: number;
    children: string[];
    expanded: boolean;
    autoFocus: boolean;
    inheritHeaderLevel: number;
    isPlaceholder: boolean;
  })[] {
    return getVisibleNodesRecursive(_rootNodeIds);
  }

  const contentProcessor = ContentProcessor.getInstance();
  const serviceName = 'ReactiveNodeService';

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
        // Compute children on-demand from parent_id relationships
        const children = Object.values(_nodes)
          .filter((n) => n.parentId === nodeId)
          .map((n) => n.id);

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

        // Add children if node is expanded
        if (uiState?.expanded && children.length > 0) {
          result.push(...getVisibleNodesRecursive(children));
        }
      }
    }

    return result;
  }

  function findNode(nodeId: string):
    | (Node & {
        depth: number;
        children: string[];
        expanded: boolean;
        autoFocus: boolean;
        inheritHeaderLevel: number;
        isPlaceholder: boolean;
      })
    | null {
    const node = _nodes[nodeId];
    if (!node) return null;

    const uiState = _uiState[nodeId] || createDefaultUIState(nodeId);
    const children = Object.values(_nodes)
      .filter((n) => n.parentId === nodeId)
      .map((n) => n.id);

    // Return node merged with UI state
    return {
      ...node,
      depth: uiState.depth,
      children,
      expanded: uiState.expanded,
      autoFocus: uiState.autoFocus,
      inheritHeaderLevel: uiState.inheritHeaderLevel,
      isPlaceholder: uiState.isPlaceholder
    };
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
    insertAtBeginning?: boolean
  ): string {
    const afterNode = findNode(afterNodeId);
    if (!afterNode) {
      return '';
    }

    clearAllAutoFocus();

    const nodeId = uuidv4();
    const afterUIState = _uiState[afterNodeId] || createDefaultUIState(afterNodeId);

    // Determine depth and parent based on insertion strategy
    let newDepth: number;
    let newParentId: string | null;

    if (insertAtBeginning) {
      newDepth = afterUIState.depth;
      newParentId = afterNode.parentId;
    } else {
      newDepth = afterUIState.depth;
      newParentId = afterNode.parentId;
    }

    // Generate initial content with header syntax if needed
    let initialContent = content;
    if (content.trim() === '') {
      const headerMatch = afterNode.content.match(/^(#{1,6}\s+)/);
      if (headerMatch) {
        initialContent = headerMatch[1];
      }
    } else {
      const afterNodeHeaderMatch = afterNode.content.match(/^(#{1,6}\s+)/);
      const contentHeaderMatch = content.match(/^(#{1,6}\s+)/);

      if (afterNodeHeaderMatch && !contentHeaderMatch) {
        initialContent = afterNodeHeaderMatch[1] + content;
      }
    }

    const isPlaceholder = initialContent.trim() === '' || /^#{1,6}\s*$/.test(initialContent.trim());

    // Create Node with unified type system
    const newNode: Node = {
      id: nodeId,
      nodeType: nodeType,
      content: initialContent,
      parentId: newParentId,
      originNodeId: newParentId || nodeId, // Root ID logic
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {},
      mentions: []
    };

    // Create UI state
    const newUIState = createDefaultUIState(nodeId, {
      depth: newDepth,
      expanded: true,
      autoFocus: true,
      inheritHeaderLevel: headerLevel !== undefined ? headerLevel : afterUIState.inheritHeaderLevel,
      isPlaceholder
    });

    _nodes[nodeId] = newNode;
    _uiState[nodeId] = newUIState;

    // Handle hierarchy positioning
    if (insertAtBeginning) {
      // Cursor at beginning: new node goes ABOVE (shifts current node down)
      if (afterNode.parentId) {
        // Insert before sibling (simplified for test - no sibling pointer updates)
      } else {
        const afterNodeIndex = _rootNodeIds.indexOf(afterNodeId);
        _rootNodeIds = [
          ..._rootNodeIds.slice(0, afterNodeIndex),
          nodeId,
          ..._rootNodeIds.slice(afterNodeIndex)
        ];
      }
    } else {
      // Cursor not at beginning: new node goes AFTER as sibling
      // Transfer all children from afterNode to the new node
      const afterNodeChildren = Object.values(_nodes)
        .filter((n) => n.parentId === afterNodeId)
        .map((n) => n.id);

      if (afterNodeChildren.length > 0) {
        // Update children's parent reference to point to new node
        for (const childId of afterNodeChildren) {
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

      // Insert new node as sibling after afterNode
      if (afterNode.parentId) {
        // Insert after sibling (simplified for test)
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
    return nodeId;
  }

  function createPlaceholderNode(
    afterNodeId: string,
    nodeType: string = 'text',
    headerLevel?: number,
    insertAtBeginning: boolean = false
  ): string {
    return createNode(afterNodeId, '', nodeType, headerLevel, insertAtBeginning);
  }

  // Debounce timers for expensive operations
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
    if (!node) {
      return;
    }

    // IMMEDIATE: Update local state for responsive UI
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

    // IMMEDIATE: Emit for immediate UI updates (live formatting, etc.)
    emitNodeUpdated(nodeId, 'content', content);

    // DEBOUNCED: Schedule expensive operations
    scheduleContentProcessing(nodeId, content);
  }

  function updateNodeType(nodeId: string, nodeType: string): void {
    const node = _nodes[nodeId];
    if (!node) {
      return;
    }

    // IMMEDIATE: Update node type for responsive UI
    _nodes[nodeId] = {
      ...node,
      nodeType: nodeType,
      modifiedAt: new Date().toISOString()
    };
    _uiState[nodeId] = { ..._uiState[nodeId], autoFocus: true };

    // IMMEDIATE: Emit for immediate UI updates
    emitNodeUpdated(nodeId, 'nodeType', nodeType);

    // Clear autoFocus after sufficient delay for component switch and focus to complete
    setTimeout(() => {
      const state = _uiState[nodeId];
      if (state?.autoFocus) {
        _uiState[nodeId] = { ...state, autoFocus: false };
      }
    }, 250);

    // DEBOUNCED: Schedule save to storage (reuse existing debouncing)
    scheduleContentProcessing(nodeId, node.content);
  }

  function scheduleContentProcessing(nodeId: string, content: string): void {
    // Clear existing timers for this node
    const existing = debouncedOperations.get(nodeId);
    if (existing?.fastTimer) clearTimeout(existing.fastTimer);
    if (existing?.expensiveTimer) clearTimeout(existing.expensiveTimer);

    // Update pending content
    debouncedOperations.set(nodeId, { pendingContent: content });

    // FAST DEBOUNCE (300ms): Mentions, references, validation
    const fastTimer = setTimeout(() => {
      processFastContentOperations(nodeId, content);
    }, 300);

    // EXPENSIVE DEBOUNCE (2000ms): Backend save, vectors, reference propagation
    const expensiveTimer = setTimeout(() => {
      processExpensiveContentOperations(nodeId, content);
    }, 2000);

    // Store timers
    debouncedOperations.set(nodeId, {
      pendingContent: content,
      fastTimer,
      expensiveTimer
    });
  }

  function processFastContentOperations(nodeId: string, _content: string): void {
    const node = _nodes[nodeId];
    if (!node) return;

    // Process mentions and references (placeholder - implement when needed)
    // const processedContent = contentProcessor.processContent(content);
    // if (processedContent.mentions) {
    //   node.mentions = processedContent.mentions;
    // }

    // Update any immediate reference links in other nodes
    emitReferenceUpdateNeeded(nodeId);

    // Clear fast timer
    const operations = debouncedOperations.get(nodeId);
    if (operations) {
      operations.fastTimer = undefined;
      debouncedOperations.set(nodeId, operations);
    }
  }

  function processExpensiveContentOperations(nodeId: string, content: string): void {
    const node = _nodes[nodeId];
    if (!node) return;

    // Emit expensive operations (backend will handle these)
    emitExpensivePersistenceNeeded(nodeId, content);
    emitVectorEmbeddingNeeded(nodeId, content);
    emitReferencePropagatationNeeded(nodeId, content);

    // Clear both timers
    debouncedOperations.delete(nodeId);
  }

  function emitReferenceUpdateNeeded(nodeId: string): void {
    const referenceUpdateEvent = {
      type: 'node:reference-update-needed' as const,
      namespace: 'content' as const,
      source: serviceName,
      nodeId
    };
    eventBus.emit(referenceUpdateEvent);
  }

  function emitExpensivePersistenceNeeded(nodeId: string, content: string): void {
    const persistenceEvent = {
      type: 'node:persistence-needed' as const,
      namespace: 'backend' as const,
      source: serviceName,
      nodeId,
      content,
      timestamp: Date.now()
    };
    eventBus.emit(persistenceEvent);
  }

  function emitVectorEmbeddingNeeded(nodeId: string, content: string): void {
    const embeddingEvent = {
      type: 'node:embedding-needed' as const,
      namespace: 'ai' as const,
      source: serviceName,
      nodeId,
      content,
      timestamp: Date.now()
    };
    eventBus.emit(embeddingEvent);
  }

  function emitReferencePropagatationNeeded(nodeId: string, content: string): void {
    const propagationEvent = {
      type: 'node:reference-propagation-needed' as const,
      namespace: 'references' as const,
      source: serviceName,
      nodeId,
      content,
      timestamp: Date.now()
    };
    eventBus.emit(propagationEvent);
  }

  function updateDescendantDepths(nodeId: string): void {
    const node = _nodes[nodeId];
    const uiState = _uiState[nodeId];
    if (!node || !uiState) return;

    // Compute children from parent_id relationships
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

  function combineNodes(currentNodeId: string, previousNodeId: string): void {
    const currentNode = _nodes[currentNodeId];
    const previousNode = _nodes[previousNodeId];

    if (!currentNode || !previousNode) return;

    // Strip formatting syntax from current node content when merging
    const cleanedContent = stripFormattingSyntax(currentNode.content);
    const combinedContent = previousNode.content + cleanedContent;
    const mergePosition = previousNode.content.length;

    _nodes[previousNodeId] = {
      ...previousNode,
      content: combinedContent,
      modifiedAt: new Date().toISOString()
    };
    _uiState[previousNodeId] = { ..._uiState[previousNodeId], autoFocus: false };

    // Handle child promotion
    const currentChildren = Object.values(_nodes)
      .filter((n) => n.parentId === currentNodeId)
      .map((n) => n.id);

    // Determine where children should go
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
    cleaned = cleaned.replace(/^#{1,6}\s+/, ''); // Remove headers
    cleaned = cleaned.replace(/^\[\s*[x\s]*\]\s*/, ''); // Remove task syntax
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
      modifiedAt: new Date().toISOString()
    };
    _uiState[nodeId] = { ...uiState, depth: (prevSiblingUIState?.depth || 0) + 1 };

    updateDescendantDepths(nodeId);

    if (!node.parentId) {
      _rootNodeIds.splice(nodeIndex, 1);
    }

    events.hierarchyChanged();
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
      modifiedAt: new Date().toISOString()
    };
    _uiState[nodeId] = { ...uiState, depth: newDepth };

    updateDescendantDepths(nodeId);

    if (!newParentId) {
      const parentIndex = _rootNodeIds.indexOf(parent.id);
      _rootNodeIds.splice(parentIndex + 1, 0, nodeId);
    }

    events.hierarchyChanged();
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
  }

  function toggleExpanded(nodeId: string): boolean {
    try {
      const uiState = _uiState[nodeId];
      if (!uiState) return false;

      _uiState[nodeId] = { ...uiState, expanded: !uiState.expanded };
      return true;
    } catch (error) {
      console.error('Error toggling node expansion:', error);
      return false;
    }
  }

  function cleanupDebouncedOperations(nodeId: string): void {
    const operations = debouncedOperations.get(nodeId);
    if (operations) {
      if (operations.fastTimer) {
        clearTimeout(operations.fastTimer);
      }
      if (operations.expensiveTimer) {
        clearTimeout(operations.expensiveTimer);
      }
      debouncedOperations.delete(nodeId);
    }
  }

  function emitNodeUpdated(nodeId: string, updateType: string, newValue: unknown): void {
    const nodeUpdatedEvent = {
      type: 'node:updated' as const,
      namespace: 'lifecycle' as const,
      source: serviceName,
      nodeId,
      updateType,
      newValue
    };

    eventBus.emit(nodeUpdatedEvent);
  }

  function initializeNodes(
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
      return getVisibleNodes();
    },
    get _updateTrigger() {
      return _updateTrigger;
    },
    getUIState(nodeId: string) {
      return _uiState[nodeId];
    },

    // Node operations
    findNode,
    createNode,
    createPlaceholderNode,
    updateNodeContent,
    updateNodeType,
    combineNodes,
    indentNode,
    outdentNode,
    deleteNode,
    toggleExpanded,

    // Initialization
    initializeNodes
  };
}

export type ReactiveNodeService = ReturnType<typeof createReactiveNodeService>;

// For backward compatibility with existing imports
export { createReactiveNodeService as ReactiveNodeService };
