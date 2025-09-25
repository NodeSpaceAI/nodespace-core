/**
 * ReactiveNodeService - Pure Svelte 5 Runes Reactive Architecture (Function-based)
 *
 * Alternative implementation using function-based approach for better Svelte 5 compatibility
 */

import { v4 as uuidv4 } from 'uuid';
import { ContentProcessor } from './contentProcessor';
import { eventBus } from './eventBus';

export interface Node {
  id: string;
  content: string;
  nodeType: string;
  depth: number;
  parentId?: string;
  children: string[];
  expanded: boolean;
  autoFocus: boolean;
  inheritHeaderLevel: number;
  metadata: Record<string, unknown>;
  mentions?: string[];
  before_sibling_id?: string;
  isPlaceholder?: boolean;
}

export interface NodeManagerEvents {
  focusRequested: (nodeId: string, position?: number) => void;
  hierarchyChanged: () => void;
  nodeCreated: (nodeId: string) => void;
  nodeDeleted: (nodeId: string) => void;
}

export function createReactiveNodeService(events: NodeManagerEvents) {
  // Pure Svelte 5 reactive state - using Record instead of Map to avoid Svelte 5 reactivity issues
  const _nodes = $state<Record<string, Node>>({});
  let _rootNodeIds = $state<string[]>([]);
  const _collapsedNodes = $state<Set<string>>(new Set());
  const _activeNodeId = $state<string | undefined>(undefined);

  // Manual reactivity trigger for debugging
  let _updateTrigger = $state(0);

  // REACTIVITY FIX: Properly reactive visibleNodes computation using $derived.by
  // This ensures the template re-renders when nodes change
  const _visibleNodes = $derived.by(() => {
    // Force reactivity by accessing all node IDs and their nodeTypes
    const allNodes = Object.values(_nodes);
    // Touch each node's nodeType to track changes
    for (const node of allNodes) {
      void node.nodeType; // Access to trigger reactivity
    }
    // Touch the _updateTrigger to ensure reactivity when needed
    void _updateTrigger;
    // Touch _rootNodeIds to ensure reactivity on root changes
    void _rootNodeIds;
    return getVisibleNodesRecursive(_rootNodeIds);
  });

  const serviceName = 'ReactiveNodeService';

  // ContentProcessor instance for content processing methods
  const contentProcessor = ContentProcessor.getInstance();

  function getVisibleNodesRecursive(nodeIds: string[]): Node[] {
    const result: Node[] = [];

    for (const nodeId of nodeIds) {
      const node = _nodes[nodeId];
      if (node) {
        result.push(node);

        // Add children if node is expanded
        if (node.expanded && !_collapsedNodes.has(nodeId) && node.children.length > 0) {
          result.push(...getVisibleNodesRecursive(node.children));
        }
      }
    }

    return result;
  }

  function findNode(nodeId: string): Node | null {
    const node = _nodes[nodeId] || null;
    return node;
  }

  function clearAllAutoFocus(): void {
    for (const [nodeId, node] of Object.entries(_nodes)) {
      if (node.autoFocus) {
        const updatedNode = { ...node, autoFocus: false };
        _nodes[nodeId] = updatedNode;
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

    // Clear autoFocus from all existing nodes
    clearAllAutoFocus();

    const nodeId = uuidv4();

    // Determine depth and parent based on insertion strategy
    let newDepth: number;
    let newParentId: string | undefined;

    if (insertAtBeginning) {
      // Node goes above afterNode with same parent and depth
      newDepth = afterNode.depth;
      newParentId = afterNode.parentId;
    } else {
      // Node goes after afterNode - always create as sibling
      newDepth = afterNode.depth;
      newParentId = afterNode.parentId;
    }

    // Generate initial content with header syntax if needed
    let initialContent = content;
    const sourceContent = originalNodeContent || afterNode.content; // Use original content if provided

    if (content.trim() === '') {
      // For empty content, extract and inherit actual header syntax from the source content
      const headerMatch = sourceContent.match(/^(#{1,6}\s+)/);
      if (headerMatch) {
        initialContent = headerMatch[1]; // This includes the # symbols and the space
      }
    } else {
      // For non-empty content (e.g., from Enter key splits), check if we should inherit header syntax
      // This handles the case where pressing Enter after "# |" should create a new node above with "# " syntax
      const sourceHeaderMatch = sourceContent.match(/^(#{1,6}\s+)/);
      const contentHeaderMatch = content.match(/^(#{1,6}\s+)/);

      // If the source content has header syntax but the new content doesn't, inherit it
      if (sourceHeaderMatch && !contentHeaderMatch) {
        initialContent = sourceHeaderMatch[1] + content;
      }
    }

    // Determine which node should receive focus based on insertion strategy
    const shouldFocusNewNode = focusNewNode !== undefined ? focusNewNode : !insertAtBeginning;

    const newNode: Node = {
      id: nodeId,
      content: initialContent,
      nodeType,
      depth: newDepth,
      parentId: newParentId,
      children: [],
      expanded: true,
      autoFocus: shouldFocusNewNode,
      inheritHeaderLevel: headerLevel !== undefined ? headerLevel : afterNode.inheritHeaderLevel,
      metadata: {},
      isPlaceholder: initialContent.trim() === '' || /^#{1,6}\s*$/.test(initialContent.trim())
    };

    // Add node to record using assignment pattern
    _nodes[nodeId] = newNode;

    // Handle hierarchy positioning using assignment patterns
    if (insertAtBeginning) {
      // Cursor at beginning: new node goes ABOVE (shifts current node down)
      // The new node is empty, current node keeps its children

      // Insert new node before afterNode
      if (afterNode.parentId) {
        const parent = _nodes[afterNode.parentId];
        if (parent) {
          const afterNodeIndex = parent.children.indexOf(afterNodeId);
          parent.children = [
            ...parent.children.slice(0, afterNodeIndex),
            nodeId,
            ...parent.children.slice(afterNodeIndex)
          ];
        }
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
      if (afterNode.children && afterNode.children.length > 0) {
        newNode.children = [...afterNode.children];
        // Update children's parent reference to point to new node
        for (const childId of afterNode.children) {
          const child = _nodes[childId];
          if (child) {
            _nodes[childId] = { ...child, parentId: nodeId };
          }
        }
        // Clear afterNode's children since they now belong to newNode
        _nodes[afterNodeId] = { ...afterNode, children: [] };
      }

      // Insert new node as sibling after afterNode
      if (afterNode.parentId) {
        const parent = _nodes[afterNode.parentId];
        if (parent) {
          const afterNodeIndex = parent.children.indexOf(afterNodeId);
          parent.children = [
            ...parent.children.slice(0, afterNodeIndex + 1),
            nodeId,
            ...parent.children.slice(afterNodeIndex + 1)
          ];
        }
      } else {
        const afterNodeIndex = _rootNodeIds.indexOf(afterNodeId);
        _rootNodeIds = [
          ..._rootNodeIds.slice(0, afterNodeIndex + 1),
          nodeId,
          ..._rootNodeIds.slice(afterNodeIndex + 1)
        ];

        // Manually trigger reactivity
        _updateTrigger++;
      }
    }

    events.nodeCreated(nodeId);
    events.hierarchyChanged();

    // Emit EventBus event for integration tests
    eventBus.emit<import('./eventTypes').NodeCreatedEvent>({
      type: 'node:created' as const,
      namespace: 'lifecycle' as const,
      source: 'ReactiveNodeService',
      nodeId,
      nodeType,
      metadata: {}
    });

    // Also emit hierarchy changed event for integration tests
    eventBus.emit<import('./eventTypes').HierarchyChangedEvent>({
      type: 'hierarchy:changed' as const,
      namespace: 'lifecycle' as const,
      source: 'ReactiveNodeService',
      affectedNodes: [nodeId],
      changeType: 'move', // Using 'move' as the closest fit since node is inserted into hierarchy
      metadata: {}
    });

    // Emit cache invalidation event for integration tests
    eventBus.emit<import('./eventTypes').CacheInvalidateEvent>({
      type: 'cache:invalidate' as const,
      namespace: 'coordination' as const,
      source: 'ReactiveNodeService',
      cacheKey: `node:${nodeId}`,
      scope: 'node',
      reason: 'node-created',
      metadata: {}
    });

    // Set focus on original node if new node doesn't receive focus
    if (!shouldFocusNewNode) {
      _nodes[afterNodeId] = { ..._nodes[afterNodeId], autoFocus: true };
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

    // IMMEDIATE: Update local state for responsive UI using assignment-based reactivity
    const headerLevel = contentProcessor.parseHeaderLevel(content);
    _nodes[nodeId] = {
      ...node,
      content,
      isPlaceholder: content.trim() === '',
      inheritHeaderLevel: headerLevel
    };

    // IMMEDIATE: Emit for immediate UI updates (live formatting, etc.)
    emitNodeUpdated(nodeId, 'content', content);

    // IMMEDIATE: Emit reference update needed (tests expect this immediately)
    emitReferenceUpdateNeeded(nodeId);

    // DEBOUNCED: Schedule expensive operations
    scheduleContentProcessing(nodeId, content);
  }

  function updateNodeType(nodeId: string, nodeType: string): void {
    const node = _nodes[nodeId];
    if (!node) {
      return;
    }

    // IMMEDIATE: Update node type for responsive UI using assignment-based reactivity
    // Also set autoFocus temporarily to restore focus when component switches
    _nodes[nodeId] = { ...node, nodeType, autoFocus: true };

    // REACTIVITY FIX: Trigger reactive computation update
    _updateTrigger++;

    // IMMEDIATE: Emit for immediate UI updates
    emitNodeUpdated(nodeId, 'nodeType', nodeType);

    // Clear autoFocus after sufficient delay for component switch and focus to complete
    setTimeout(() => {
      const currentNode = _nodes[nodeId];
      if (currentNode && currentNode.autoFocus) {
        const updatedNode = { ...currentNode, autoFocus: false };
        _nodes[nodeId] = updatedNode;
      }
    }, 250); // Sufficient time for component switch and focus

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
      type: 'references:update-needed' as const,
      namespace: 'coordination' as const,
      source: serviceName,
      nodeId,
      updateType: 'content',
      metadata: {}
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
      metadata: {}
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
      metadata: {}
    };
    eventBus.emit(embeddingEvent);
  }

  function emitReferencePropagatationNeeded(nodeId: string, content: string): void {
    const propagationEvent = {
      type: 'node:reference-propagation-needed' as const,
      namespace: 'coordination' as const,
      source: serviceName,
      nodeId,
      content,
      metadata: {}
    };
    eventBus.emit(propagationEvent);
  }

  function rebuildChildrenArrays(): void {
    // Clear all children arrays
    for (const node of Object.values(_nodes)) {
      node.children = [];
    }

    // Rebuild children arrays by iterating through all nodes and adding them to their parent's children
    for (const node of Object.values(_nodes)) {
      if (node.parentId) {
        const parent = _nodes[node.parentId];
        if (parent) {
          parent.children.push(node.id);
        }
      }
    }
  }

  function updateDescendantDepths(node: Node): void {
    const expectedDepth = node.parentId ? (findNode(node.parentId)?.depth ?? 0) + 1 : 0;

    if (node.depth !== expectedDepth) {
      // Use assignment-based reactivity - reassign the entire object
      _nodes[node.id] = { ...node, depth: expectedDepth };
    }

    // ALWAYS update children depths regardless of whether parent depth changed
    // Children should have depth = parent.depth + 1
    const expectedChildDepth = node.depth + 1;
    for (const childId of node.children) {
      const child = _nodes[childId];
      if (child) {
        if (child.depth !== expectedChildDepth) {
          // Use assignment-based reactivity - reassign the entire object
          _nodes[childId] = { ...child, depth: expectedChildDepth };
        }
        // Recursively update the child's descendants
        updateDescendantDepths(child);
      }
    }
  }

  function combineNodes(currentNodeId: string, previousNodeId: string): void {
    const currentNode = _nodes[currentNodeId];
    const previousNode = _nodes[previousNodeId];

    if (!currentNode || !previousNode) {
      return;
    }

    // Check if this is child-to-parent merging
    const isChildToParent = currentNode.parentId === previousNodeId;

    // Strip formatting syntax from current node content when merging
    const cleanedContent = stripFormattingSyntax(currentNode.content);
    const combinedContent = previousNode.content + cleanedContent;

    // Calculate cursor position for focus (at the merge point between old and new content)
    // Position should be at the end of the original previousNode content
    const mergePosition = previousNode.content.length;

    // Create atomic update - all changes in one operation
    const newNodesRecord = { ..._nodes };
    const updatedPreviousNode = { ...previousNode, content: combinedContent };

    // Handle child-to-parent merging OR any merge where current node has children
    if (isChildToParent || currentNode.children.length > 0) {
      if (isChildToParent) {
        // Find current node's position in parent's children
        const currentNodeIndex = previousNode.children.indexOf(currentNodeId);

        if (currentNodeIndex !== -1) {
          // Remove current node and insert its children in its place
          const beforeChildren = previousNode.children.slice(0, currentNodeIndex);
          const afterChildren = previousNode.children.slice(currentNodeIndex + 1);
          updatedPreviousNode.children = [
            ...beforeChildren,
            ...currentNode.children,
            ...afterChildren
          ];
        }
      } else {
        // Handle sibling merge: children will be assigned to correct parents in the loop below
        // First, collect the new parent IDs for each child
        const childToNewParent = new Map();

        for (const childId of currentNode.children) {
          const child = newNodesRecord[childId];
          if (child) {
            // Find the correct parent: the node that is one level above the child's current depth
            const childDepth = child.depth;
            let newParentId = null;

            // Check if the merged node (previousNode) can be the parent
            if (previousNode.depth === childDepth - 1) {
              newParentId = previousNodeId;
            } else {
              // Walk up from previousNode to find a node at the appropriate parent depth
              let currentParentId = previousNode.parentId;
              while (currentParentId && newNodesRecord[currentParentId]) {
                const parentNode = newNodesRecord[currentParentId];
                if (parentNode.depth === childDepth - 1) {
                  // Found appropriate parent
                  newParentId = currentParentId;
                  break;
                }
                currentParentId = parentNode.parentId;
              }
            }

            childToNewParent.set(childId, newParentId);
          }
        }

        // Add children to their new parents' children lists
        for (const [childId, newParentId] of childToNewParent) {
          if (newParentId && newNodesRecord[newParentId]) {
            const newParent = newNodesRecord[newParentId];
            if (!newParent.children.includes(childId)) {
              newNodesRecord[newParentId] = {
                ...newParent,
                children: [...newParent.children, childId]
              };
            }
          } else {
            // If no suitable parent found, make it a root node
            _rootNodeIds.push(childId);
          }
        }

        // Update children's parent reference and depth using the parent assignments from above
        for (const childId of currentNode.children) {
          const child = newNodesRecord[childId];
          if (child) {
            // Children maintain their current depth - they don't change level during merge
            const targetDepth = child.depth;
            const newParentId = childToNewParent.get(childId);

            // Update child with correct parent but preserve existing depth
            newNodesRecord[childId] = {
              ...child,
              parentId: newParentId,
              depth: targetDepth
            };
          }
        }
      }
    }

    // Update the target node with merged content and disable autoFocus for precise cursor positioning
    const finalPreviousNode = newNodesRecord[previousNodeId] || previousNode;
    newNodesRecord[previousNodeId] = {
      ...finalPreviousNode,
      content: combinedContent,
      autoFocus: false
    };
    delete newNodesRecord[currentNodeId];

    // Apply atomic changes
    Object.keys(_nodes).forEach((key) => delete _nodes[key]);
    Object.entries(newNodesRecord).forEach(([id, node]) => {
      _nodes[id] = node;
    });

    // Update depths for all descendants of promoted children (for any merge with children)
    if (currentNode.children.length > 0) {
      for (const childId of currentNode.children) {
        const promotedChild = _nodes[childId];
        if (promotedChild) {
          updateDescendantDepths(promotedChild);
        }
      }
    }

    // Remove from parent's children if not child-to-parent
    if (!isChildToParent && currentNode.parentId) {
      const parent = _nodes[currentNode.parentId];
      if (parent) {
        parent.children = parent.children.filter((id) => id !== currentNodeId);
      }
    }

    // Remove from root nodes if needed
    const rootIndex = _rootNodeIds.indexOf(currentNodeId);
    if (rootIndex >= 0) {
      _rootNodeIds.splice(rootIndex, 1);
    }

    // Prepare for precise cursor positioning
    clearAllAutoFocus();

    // Emit focus request to position cursor at merge point
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
    const node = findNode(nodeId);
    if (!node) {
      return false;
    }

    let siblings: string[];
    let nodeIndex: number;

    if (node.parentId) {
      // Node has a parent - get siblings from parent's children
      const parent = findNode(node.parentId);
      if (!parent) return false;
      siblings = parent.children;
      nodeIndex = siblings.indexOf(nodeId);
    } else {
      // Root node - get siblings from root node IDs
      siblings = _rootNodeIds;
      nodeIndex = siblings.indexOf(nodeId);
    }

    if (nodeIndex === 0) return false; // Can't indent if it's the first sibling

    const prevSiblingId = siblings[nodeIndex - 1];
    const prevSibling = findNode(prevSiblingId);
    if (!prevSibling) return false;

    // Remove from current location
    if (node.parentId) {
      const parent = findNode(node.parentId);
      if (parent) {
        parent.children = parent.children.filter((id) => id !== nodeId);
      }
    } else {
      _rootNodeIds.splice(nodeIndex, 1);
    }

    // Add to previous sibling as child
    prevSibling.children.push(nodeId);

    // Use assignment-based reactivity for node updates
    _nodes[nodeId] = { ...node, parentId: prevSiblingId, depth: prevSibling.depth + 1 };

    updateDescendantDepths(node);
    events.hierarchyChanged();

    // Trigger reactivity for visibleNodes (hierarchy changed)
    _updateTrigger++;

    // Emit EventBus events for integration tests
    eventBus.emit<import('./eventTypes').HierarchyChangedEvent>({
      type: 'hierarchy:changed' as const,
      namespace: 'lifecycle' as const,
      source: 'ReactiveNodeService',
      changeType: 'indent',
      affectedNodes: [nodeId]
    });

    eventBus.emit<import('./eventTypes').ReferencesUpdateNeededEvent>({
      type: 'references:update-needed' as const,
      namespace: 'coordination' as const,
      source: 'ReactiveNodeService',
      nodeId,
      updateType: 'hierarchy',
      metadata: {}
    });

    return true;
  }

  function outdentNode(nodeId: string): boolean {
    const node = findNode(nodeId);

    if (!node || !node.parentId) {
      return false;
    }

    const parent = findNode(node.parentId);

    if (!parent) {
      return false;
    }

    // WORKAROUND: Rebuild children arrays from parent relationships if corrupted
    if (
      !parent.children ||
      !Array.isArray(parent.children) ||
      parent.children.indexOf(nodeId) === -1
    ) {
      rebuildChildrenArrays();

      // Re-fetch parent after rebuilding
      const rebuiltParent = findNode(node.parentId);
      if (
        !rebuiltParent ||
        !rebuiltParent.children ||
        rebuiltParent.children.indexOf(nodeId) === -1
      ) {
        return false;
      }

      // Update parent reference
      Object.assign(parent, rebuiltParent);
    }

    // Find the position of this node among its siblings
    const nodeIndexInParent = parent.children.indexOf(nodeId);
    if (nodeIndexInParent === -1) {
      return false;
    }

    // Collect siblings that come after this node - they will become children of the outdented node
    const followingSiblings = parent.children.slice(nodeIndexInParent + 1);

    // Remove the outdented node AND all following siblings from the parent
    parent.children = parent.children.slice(0, nodeIndexInParent);

    // Determine where to place the outdented node
    let newParentId: string | undefined;
    let insertionTarget: { isRoot: boolean; parentId?: string; insertIndex: number };

    if (parent.parentId) {
      // Parent has a parent (grandparent exists)
      const grandparent = findNode(parent.parentId);
      if (!grandparent) {
        return false;
      }

      const parentIndexInGrandparent = grandparent.children.indexOf(parent.id);
      newParentId = parent.parentId;
      insertionTarget = {
        isRoot: false,
        parentId: parent.parentId,
        insertIndex: parentIndexInGrandparent + 1
      };
    } else {
      // Parent is a root node, so outdented node becomes root
      const parentIndexInRoot = _rootNodeIds.indexOf(parent.id);
      newParentId = undefined;
      insertionTarget = {
        isRoot: true,
        insertIndex: parentIndexInRoot + 1
      };
    }

    // Update the outdented node with assignment-based reactivity
    const newDepth = newParentId ? (findNode(newParentId)?.depth ?? 0) + 1 : 0;

    _nodes[nodeId] = { ...node, parentId: newParentId, depth: newDepth };
    // Update our local reference to the updated node
    const updatedNode = _nodes[nodeId];

    // Insert the outdented node at its new position
    if (insertionTarget.isRoot) {
      _rootNodeIds.splice(insertionTarget.insertIndex, 0, nodeId);
    } else {
      const newParent = findNode(insertionTarget.parentId!);
      if (newParent) {
        newParent.children.splice(insertionTarget.insertIndex, 0, nodeId);
      } else {
        console.error(`❌ Could not find new parent: ${insertionTarget.parentId}`);
      }
    }

    // Add the following siblings as children of the outdented node (after existing children)
    if (followingSiblings.length > 0) {
      // Update their parent references in the reactive storage
      for (const siblingId of followingSiblings) {
        const siblingNode = findNode(siblingId);
        if (siblingNode) {
          // Create updated node and save back to reactive storage
          const updatedSibling = { ...siblingNode, parentId: nodeId };
          _nodes[siblingId] = updatedSibling;
        } else {
          console.error(`❌ Could not find sibling node: ${siblingId}`);
        }
      }

      // Add them to the outdented node's children (after existing children)
      const newChildren = [...updatedNode.children, ...followingSiblings];
      _nodes[nodeId] = { ...updatedNode, children: newChildren };
    }

    // Update depths for the outdented node and all its children (including the new ones)
    const finalUpdatedNode = _nodes[nodeId]; // Get the latest version with updated children
    updateDescendantDepths(finalUpdatedNode);

    events.hierarchyChanged();

    // Trigger reactivity for visibleNodes (hierarchy changed)
    _updateTrigger++;

    // Emit EventBus event for integration tests
    eventBus.emit<import('./eventTypes').HierarchyChangedEvent>({
      type: 'hierarchy:changed' as const,
      namespace: 'lifecycle' as const,
      source: 'ReactiveNodeService',
      changeType: 'outdent',
      affectedNodes: [nodeId]
    });

    return true;
  }

  function deleteNode(nodeId: string): void {
    const node = findNode(nodeId);
    if (!node) return;

    // CLEANUP: Cancel any pending debounced operations for this node
    cleanupDebouncedOperations(nodeId);

    // Move children to parent or root
    if (node.parentId) {
      const parent = findNode(node.parentId);
      if (parent && node.children.length > 0) {
        const nodeIndex = parent.children.indexOf(nodeId);
        parent.children = [
          ...parent.children.slice(0, nodeIndex),
          ...node.children,
          ...parent.children.slice(nodeIndex + 1)
        ];

        // Update children's parent reference
        for (const childId of node.children) {
          const child = findNode(childId);
          if (child) {
            child.parentId = node.parentId;
            updateDescendantDepths(child);
          }
        }
      }

      // Remove from parent's children
      if (parent) {
        parent.children = parent.children.filter((id) => id !== nodeId);
      }
    } else {
      // Root node - move children to root
      const rootIndex = _rootNodeIds.indexOf(nodeId);
      if (rootIndex >= 0) {
        if (node.children.length > 0) {
          _rootNodeIds.splice(rootIndex, 1, ...node.children);
          // Update children's parent reference
          for (const childId of node.children) {
            const child = findNode(childId);
            if (child) {
              child.parentId = undefined;
              child.depth = 0;
              updateDescendantDepths(child);
            }
          }
        } else {
          _rootNodeIds.splice(rootIndex, 1);
        }
      }
    }

    delete _nodes[nodeId];
    events.nodeDeleted(nodeId);

    // Trigger reactivity for visibleNodes
    _updateTrigger++;

    // Emit node deleted event
    eventBus.emit<import('./eventTypes').NodeDeletedEvent>({
      type: 'node:deleted' as const,
      namespace: 'lifecycle' as const,
      source: 'ReactiveNodeService',
      nodeId,
      parentId: node.parentId
    });

    // Emit hierarchy change event for deletion
    eventBus.emit<import('./eventTypes').HierarchyChangedEvent>({
      type: 'hierarchy:changed' as const,
      namespace: 'lifecycle' as const,
      source: 'ReactiveNodeService',
      changeType: 'collapse',
      affectedNodes: [nodeId]
    });

    // Emit references update needed for deletion
    eventBus.emit<import('./eventTypes').ReferencesUpdateNeededEvent>({
      type: 'references:update-needed' as const,
      namespace: 'coordination' as const,
      source: 'ReactiveNodeService',
      nodeId,
      updateType: 'deletion',
      metadata: {}
    });

    // Emit global cache invalidation for deletion
    eventBus.emit<import('./eventTypes').CacheInvalidateEvent>({
      type: 'cache:invalidate' as const,
      namespace: 'coordination' as const,
      source: 'ReactiveNodeService',
      cacheKey: `global:hierarchy`,
      scope: 'global',
      reason: 'node-deleted'
    });
  }

  function toggleExpanded(nodeId: string): boolean {
    try {
      const node = _nodes[nodeId];
      if (!node) {
        console.warn(`Cannot toggle expansion of non-existent node: ${nodeId}`);
        return false;
      }

      // Toggle the expanded state
      const newExpandedState = !node.expanded;
      // Mutate existing node object to preserve references, then replace in store
      node.expanded = newExpandedState;
      _nodes[nodeId] = { ...node };

      // Emit hierarchy changed event since expansion affects visibility
      events.hierarchyChanged();

      // Trigger reactivity for visibleNodes (expansion affects visibility)
      _updateTrigger++;

      // Emit EventBus events for integration tests
      const status = newExpandedState ? 'expanded' : 'collapsed';
      const changeType = newExpandedState ? 'expand' : 'collapse';

      eventBus.emit<import('./eventTypes').NodeStatusChangedEvent>({
        type: 'node:status-changed' as const,
        namespace: 'coordination' as const,
        source: 'ReactiveNodeService',
        nodeId,
        status
      });

      eventBus.emit<import('./eventTypes').HierarchyChangedEvent>({
        type: 'hierarchy:changed' as const,
        namespace: 'lifecycle' as const,
        source: 'ReactiveNodeService',
        changeType,
        affectedNodes: [nodeId]
      });

      return true;
    } catch (error) {
      console.error('❌ Error toggling node expansion:', error);
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

    // Also emit decoration update needed for content updates
    if (updateType === 'content') {
      eventBus.emit<import('./eventTypes').DecorationUpdateNeededEvent>({
        type: 'decoration:update-needed' as const,
        namespace: 'interaction',
        source: 'ReactiveNodeService',
        nodeId,
        decorationType: 'content',
        reason: 'content-changed',
        metadata: {}
      });

      // Emit cache invalidation for content updates
      eventBus.emit<import('./eventTypes').CacheInvalidateEvent>({
        type: 'cache:invalidate' as const,
        namespace: 'coordination' as const,
        source: 'ReactiveNodeService',
        cacheKey: `node:${nodeId}`,
        scope: 'node',
        nodeId,
        reason: 'content-updated'
      });
    }
  }

  function initializeWithRichDemoData(): void {
    const demoNodes: Node[] = [
      {
        id: 'welcome-root',
        content: '# Welcome to NodeSpace',
        nodeType: 'text',
        depth: 0,
        children: ['features-section', 'formatting-section', 'tasks-section'],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 1,
        metadata: { type: 'welcome', version: '1.0' }
      },
      {
        id: 'features-section',
        content: '## Key Features',
        nodeType: 'text',
        depth: 1,
        parentId: 'welcome-root',
        children: ['feature-1', 'feature-2', 'feature-3'],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 2,
        metadata: {}
      },
      {
        id: 'feature-1',
        content: 'AI-native knowledge management with **powerful** search',
        nodeType: 'text',
        depth: 2,
        parentId: 'features-section',
        children: [],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: {}
      },
      {
        id: 'feature-2',
        content: 'Hierarchical note organization with *infinite* nesting',
        nodeType: 'text',
        depth: 2,
        parentId: 'features-section',
        children: [],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: {}
      },
      {
        id: 'feature-3',
        content: 'Real-time collaboration and `code syntax` highlighting',
        nodeType: 'text',
        depth: 2,
        parentId: 'features-section',
        children: [],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: {}
      },
      {
        id: 'formatting-section',
        content: '## Formatting Examples',
        nodeType: 'text',
        depth: 1,
        parentId: 'welcome-root',
        children: ['markdown-text', 'code-example', 'math-example'],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 2,
        metadata: {}
      },
      {
        id: 'markdown-text',
        content: 'This text shows **bold**, *italic*, and ~~strikethrough~~ formatting',
        nodeType: 'text',
        depth: 2,
        parentId: 'formatting-section',
        children: [],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: {}
      },
      {
        id: 'code-example',
        content: 'Inline code: `const x = 42;` and keyboard shortcuts like `Ctrl+C`',
        nodeType: 'text',
        depth: 2,
        parentId: 'formatting-section',
        children: [],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: {}
      },
      {
        id: 'math-example',
        content: 'Math expressions: E = mc² and quadratic formula x = (-b ± √(b²-4ac))/2a',
        nodeType: 'text',
        depth: 2,
        parentId: 'formatting-section',
        children: [],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: {}
      },
      {
        id: 'tasks-section',
        content: '## Sample Tasks',
        nodeType: 'text',
        depth: 1,
        parentId: 'welcome-root',
        children: ['task-1', 'task-2', 'task-3'],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 2,
        metadata: {}
      },
      {
        id: 'task-1',
        content: 'Try keyboard navigation (Tab to indent, Shift+Tab to outdent)',
        nodeType: 'task',
        depth: 2,
        parentId: 'tasks-section',
        children: ['task-1-subtask'],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: { taskState: 'completed' }
      },
      {
        id: 'task-1-subtask',
        content: 'Press Enter to create new nodes, Backspace to merge with previous',
        nodeType: 'task',
        depth: 3,
        parentId: 'task-1',
        children: [],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: { taskState: 'inProgress' }
      },
      {
        id: 'task-2',
        content: 'Explore the node hierarchy by expanding and collapsing sections',
        nodeType: 'task',
        depth: 2,
        parentId: 'tasks-section',
        children: [],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: { taskState: 'pending' }
      },
      {
        id: 'task-3',
        content: 'Try typing with **markdown** and `code` formatting!',
        nodeType: 'task',
        depth: 2,
        parentId: 'tasks-section',
        children: [],
        expanded: true,
        autoFocus: true, // Focus on this task
        inheritHeaderLevel: 0,
        metadata: { taskState: 'completed' }
      }
    ];

    // Clear existing data
    Object.keys(_nodes).forEach((key) => delete _nodes[key]);
    _rootNodeIds = [];
    _collapsedNodes.clear();

    // Add all demo nodes - ensure children arrays are properly preserved
    for (const node of demoNodes) {
      // Ensure children array is never undefined and create a proper copy
      const preservedNode: Node = {
        id: node.id,
        content: node.content,
        nodeType: node.nodeType,
        depth: node.depth,
        parentId: node.parentId,
        children: Array.isArray(node.children) ? [...node.children] : [],
        expanded: node.expanded,
        autoFocus: node.autoFocus,
        inheritHeaderLevel: node.inheritHeaderLevel,
        metadata: { ...node.metadata }
      };

      _nodes[preservedNode.id] = preservedNode;

      if (preservedNode.depth === 0) {
        _rootNodeIds = [..._rootNodeIds, preservedNode.id];
      }
    }
  }

  return {
    // Reactive getters
    get nodes() {
      // Return a Map for backward compatibility with existing code that expects .has(), .get(), etc.
      return new Map(Object.entries(_nodes));
    },
    get rootNodeIds() {
      return _rootNodeIds;
    },
    get activeNodeId() {
      return _activeNodeId;
    },
    get visibleNodes() {
      // Return the properly reactive derived value
      return _visibleNodes;
    },
    get _updateTrigger() {
      return _updateTrigger;
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
    initializeWithSampleData: initializeWithRichDemoData,
    initializeWithRichDemoData,

    // Content processing methods for integration tests
    parseNodeContent(nodeId: string) {
      const node = findNode(nodeId);
      if (!node) return null;
      return contentProcessor.parseMarkdown(node.content);
    },

    async renderNodeAsHTML(nodeId: string): Promise<string> {
      const node = findNode(nodeId);
      if (!node) return '';
      const result = await contentProcessor.markdownToDisplay(node.content);
      return result || '';
    },

    getNodeHeaderLevel(nodeId: string): number {
      const node = findNode(nodeId);
      if (!node) return 0;
      const headerMatch = node.content.match(/^(#{1,6})\s+/);
      return headerMatch ? headerMatch[1].length : 0;
    },

    getNodeDisplayText(nodeId: string): string {
      const node = findNode(nodeId);
      if (!node) return '';
      return contentProcessor
        .displayToMarkdown(node.content)
        .replace(/[#*`[\]()]/g, '')
        .trim();
    },

    updateNodeContentWithProcessing(nodeId: string, content: string): boolean {
      const node = findNode(nodeId);
      if (!node) return false;
      updateNodeContent(nodeId, content);
      // The header level is computed dynamically, so no separate update needed
      return true;
    },

    // Add legacy data initialization method for tests
    initializeFromLegacyData(
      legacyData: Array<{
        id: string;
        type?: string;
        nodeType?: string;
        content: string;
        inheritHeaderLevel: number;
        children: string[];
        expanded: boolean;
        autoFocus: boolean;
        metadata?: Record<string, unknown>;
        parentId?: string;
      }>
    ): void {
      // Clear existing data
      Object.keys(_nodes).forEach((id) => delete _nodes[id]);
      _rootNodeIds = [];

      // Filter out invalid entries
      const validLegacyData = legacyData.filter(
        (node): node is NonNullable<typeof node> =>
          node !== null &&
          node !== undefined &&
          typeof node === 'object' &&
          'id' in node &&
          Boolean(node.id)
      );

      // Convert legacy data to new format
      for (const legacyNode of validLegacyData) {
        const node: Node = {
          id: legacyNode.id,
          content: legacyNode.content || '',
          nodeType: legacyNode.type || legacyNode.nodeType || 'text',
          depth: 0, // Will be calculated based on hierarchy
          parentId: undefined, // Will be set based on children relationships
          children: Array.isArray(legacyNode.children) ? [...legacyNode.children] : [],
          expanded: legacyNode.expanded ?? true,
          autoFocus: legacyNode.autoFocus ?? false,
          inheritHeaderLevel: legacyNode.inheritHeaderLevel ?? 0,
          metadata: legacyNode.metadata || {}
        };
        _nodes[legacyNode.id] = node;
      }

      // Set root nodes (nodes not referenced as children)
      const allChildIds = new Set(
        validLegacyData.flatMap((n) => (Array.isArray(n.children) ? n.children : []))
      );
      _rootNodeIds = validLegacyData.filter((n) => !allChildIds.has(n.id)).map((n) => n.id);

      // Update parent references and depths
      for (const node of Object.values(_nodes)) {
        for (const childId of node.children) {
          const child = _nodes[childId];
          if (child) {
            child.parentId = node.id;
            child.depth = node.depth + 1;
          }
        }
      }

      // Trigger reactivity
      _updateTrigger++;

      // Emit hierarchy changed event since we've rebuilt the entire hierarchy
      events.hierarchyChanged();
    }
  };
}

export type ReactiveNodeService = ReturnType<typeof createReactiveNodeService>;

// For backward compatibility with existing imports
export { createReactiveNodeService as ReactiveNodeService };
