/**
 * Test-compatible version of ReactiveNodeService
 * Provides the same API without Svelte 5 runes dependency
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

// Type guard and interface for legacy node data
interface LegacyNodeData {
  id: string;
  content?: string;
  nodeType?: string;
  depth?: number;
  parentId?: string;
  children?: unknown[];
  expanded?: boolean;
  autoFocus?: boolean;
  inheritHeaderLevel?: number;
  metadata?: Record<string, unknown>;
  mentions?: string[];
  before_sibling_id?: string;
  isPlaceholder?: boolean;
}

// Type guard function to check if unknown data looks like a legacy node
function isLegacyNodeData(data: unknown): data is LegacyNodeData {
  return (
    data !== null &&
    typeof data === 'object' &&
    'id' in data &&
    typeof (data as unknown as { id: unknown }).id === 'string'
  );
}

export interface NodeManagerEvents {
  focusRequested: (nodeId: string, position?: number) => void;
  hierarchyChanged: () => void;
  nodeCreated: (nodeId: string) => void;
  nodeDeleted: (nodeId: string) => void;
}

export function createReactiveNodeService(events: NodeManagerEvents) {
  // Plain JavaScript state management for tests - no Svelte runes
  const _nodes: Record<string, Node> = {};
  let _rootNodeIds: string[] = [];
  const _collapsedNodes = new Set<string>();
  let _activeNodeId: string | undefined = undefined;

  // Manual reactivity trigger for debugging
  let _updateTrigger = 0;

  // Computed visible nodes - without Svelte reactivity
  function getVisibleNodes(): Node[] {
    return getVisibleNodesRecursive(_rootNodeIds);
  }

  const contentProcessor = ContentProcessor.getInstance();
  const serviceName = 'ReactiveNodeService';

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
    insertAtBeginning?: boolean
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
    if (content.trim() === '') {
      // For empty content, extract and inherit actual header syntax from the original node
      const headerMatch = afterNode.content.match(/^(#{1,6}\s+)/);
      if (headerMatch) {
        initialContent = headerMatch[1]; // This includes the # symbols and the space
      }
    } else {
      // For non-empty content (e.g., from Enter key splits), check if we should inherit header syntax
      // This handles the case where pressing Enter after "# |" should create a new node above with "# " syntax
      const afterNodeHeaderMatch = afterNode.content.match(/^(#{1,6}\s+)/);
      const contentHeaderMatch = content.match(/^(#{1,6}\s+)/);

      // If the afterNode has header syntax but the new content doesn't, inherit it
      if (afterNodeHeaderMatch && !contentHeaderMatch) {
        initialContent = afterNodeHeaderMatch[1] + content;
      }
    }

    const newNode: Node = {
      id: nodeId,
      content: initialContent,
      nodeType,
      depth: newDepth,
      parentId: newParentId,
      children: [],
      expanded: true,
      autoFocus: true,
      inheritHeaderLevel: headerLevel !== undefined ? headerLevel : afterNode.inheritHeaderLevel,
      metadata: {},
      isPlaceholder: initialContent.trim() === '' || /^#{1,6}\s*$/.test(initialContent.trim())
    };

    // Add node to record
    _nodes[nodeId] = newNode;

    // Handle hierarchy positioning
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
    _nodes[nodeId] = {
      ...node,
      content,
      isPlaceholder: content.trim() === '',
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
    // Also set autoFocus temporarily to restore focus when component switches
    _nodes[nodeId] = { ...node, nodeType, autoFocus: true };

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

  function processFastContentOperations(nodeId: string, content: string): void {
    const node = _nodes[nodeId];
    if (!node) return;

    // Process mentions and references (placeholder - implement when needed)
    // const processedContent = contentProcessor.processContent(content);
    // if (processedContent.mentions) {
    //   node.mentions = processedContent.mentions;
    // }

    // Update any immediate reference links in other nodes
    emitReferenceUpdateNeeded(nodeId, content);

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

  function emitReferenceUpdateNeeded(nodeId: string, content: string): void {
    const referenceUpdateEvent = {
      type: 'node:reference-update-needed' as const,
      namespace: 'content' as const,
      source: serviceName,
      nodeId,
      content
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

  function rebuildChildrenArrays(): void {
    // Clear all children arrays first
    for (const nodeId of Object.keys(_nodes)) {
      _nodes[nodeId] = { ..._nodes[nodeId], children: [] };
    }

    // Rebuild children arrays by iterating through all nodes and adding them to their parent's children
    for (const node of Object.values(_nodes)) {
      if (node.parentId && _nodes[node.parentId]) {
        const parent = _nodes[node.parentId];
        const updatedParent = { ...parent, children: [...parent.children, node.id] };
        _nodes[node.parentId] = updatedParent;
      }
    }
  }

  function updateDescendantDepths(node: Node): void {
    const expectedDepth = node.parentId ? (findNode(node.parentId)?.depth ?? 0) + 1 : 0;

    if (node.depth !== expectedDepth) {
      // Update the node object directly
      _nodes[node.id] = { ...node, depth: expectedDepth };
    }

    // ALWAYS update children depths regardless of whether parent depth changed
    // Children should have depth = parent.depth + 1
    const expectedChildDepth = node.depth + 1;
    for (const childId of node.children) {
      const child = _nodes[childId];
      if (child) {
        if (child.depth !== expectedChildDepth) {
          // Update the child object directly
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
        // Handle sibling merge: children of current node become children of the previous node
        // Transfer all children from current node to previous node
        updatedPreviousNode.children = [...previousNode.children, ...currentNode.children];

        // Update children's parent reference to point to the previous node
        for (const childId of currentNode.children) {
          const child = newNodesRecord[childId];
          if (child) {
            // Children become children of the merged node (previous node)
            newNodesRecord[childId] = { ...child, parentId: previousNodeId };
          }
        }
      }
    }

    // Update previous node and remove current node atomically
    newNodesRecord[previousNodeId] = updatedPreviousNode;
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

    // Update node properties
    _nodes[nodeId] = { ...node, parentId: prevSiblingId, depth: prevSibling.depth + 1 };

    updateDescendantDepths(node);
    events.hierarchyChanged();

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

    // Update the outdented node
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
      // Update their parent references in the storage
      for (const siblingId of followingSiblings) {
        const siblingNode = findNode(siblingId);
        if (siblingNode) {
          // Create updated node and save back to storage
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
  }

  function toggleExpanded(nodeId: string): boolean {
    try {
      const node = _nodes[nodeId];
      if (!node) {
        console.warn(`Cannot toggle expansion of non-existent node: ${nodeId}`);
        return false;
      }

      // Toggle the expanded state by mutating the object in place
      // This maintains existing object references that tests might hold
      node.expanded = !node.expanded;

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
  }

  function initializeFromLegacyData(legacyNodes: unknown[]): void {
    // Clear existing data
    Object.keys(_nodes).forEach((key) => delete _nodes[key]);
    _rootNodeIds = [];
    _collapsedNodes.clear();

    // First pass: check if this uses flat structure (nodes with children as IDs) or nested structure
    const hasNestedChildren = legacyNodes.some(
      (node) =>
        isLegacyNodeData(node) &&
        Array.isArray(node.children) &&
        node.children.length > 0 &&
        typeof node.children[0] === 'object' &&
        node.children[0] !== null &&
        'id' in node.children[0]
    );

    // Handle nested structure recursively
    const processLegacyNode = (legacyNode: unknown, parentId?: string, depth: number = 0): void => {
      if (!isLegacyNodeData(legacyNode)) {
        return;
      }

      const node: Node = {
        id: legacyNode.id,
        content: legacyNode.content || '',
        nodeType: legacyNode.nodeType || 'text',
        depth: depth,
        parentId: parentId,
        children: [],
        expanded: legacyNode.expanded !== false,
        autoFocus: legacyNode.autoFocus || false,
        inheritHeaderLevel: legacyNode.inheritHeaderLevel || 0,
        metadata: legacyNode.metadata || {},
        mentions: legacyNode.mentions,
        before_sibling_id: legacyNode.before_sibling_id,
        isPlaceholder: legacyNode.isPlaceholder || false
      };

      _nodes[node.id] = node;

      if (!parentId) {
        _rootNodeIds.push(node.id);
      }

      if (Array.isArray(legacyNode.children)) {
        const childIds: string[] = [];
        for (const childNode of legacyNode.children) {
          if (isLegacyNodeData(childNode)) {
            childIds.push(childNode.id);
            processLegacyNode(childNode, node.id, depth + 1);
          }
        }
        _nodes[node.id] = { ..._nodes[node.id], children: childIds };
      }
    };

    if (hasNestedChildren) {
      for (const legacyNode of legacyNodes) {
        processLegacyNode(legacyNode);
      }
    } else {
      // Handle flat structure - create all nodes first, then establish relationships
      for (const legacyNode of legacyNodes) {
        if (!isLegacyNodeData(legacyNode)) {
          continue;
        }

        const node: Node = {
          id: legacyNode.id,
          content: legacyNode.content || '',
          nodeType: legacyNode.nodeType || 'text',
          depth: legacyNode.depth || 0,
          parentId: legacyNode.parentId,
          children: Array.isArray(legacyNode.children)
            ? [...(legacyNode.children as string[])]
            : [],
          expanded: legacyNode.expanded !== false,
          autoFocus: legacyNode.autoFocus || false,
          inheritHeaderLevel: legacyNode.inheritHeaderLevel || 0,
          metadata: legacyNode.metadata || {},
          mentions: legacyNode.mentions,
          before_sibling_id: legacyNode.before_sibling_id,
          isPlaceholder: legacyNode.isPlaceholder || false
        };

        _nodes[node.id] = node;

        if (!node.parentId) {
          _rootNodeIds.push(node.id);
        }
      }
    }

    // Emit hierarchy changed event
    events.hierarchyChanged();
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
        metadata: { completed: true }
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
        metadata: { completed: false }
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
        metadata: { completed: false }
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
        metadata: { completed: false }
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
      return getVisibleNodes();
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
    getVisibleNodes,

    // Initialization
    initializeFromLegacyData,
    initializeWithSampleData: initializeWithRichDemoData,
    initializeWithRichDemoData
  };
}

export type ReactiveNodeService = ReturnType<typeof createReactiveNodeService>;

// For backward compatibility with existing imports
export { createReactiveNodeService as ReactiveNodeService };
