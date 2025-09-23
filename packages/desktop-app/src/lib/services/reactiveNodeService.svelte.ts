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

  // Pure reactive computed visible nodes - with logging to debug reactivity
  const visibleNodes = $derived.by(() => {
    // Force reactivity by accessing the trigger
    _updateTrigger;
    const result = getVisibleNodesRecursive(_rootNodeIds);
    console.log(`🔄 visibleNodes updated: ${result.length} nodes from ${_rootNodeIds.length} roots`);
    return result;
  });

  function getVisibleNodes(): Node[] {
    return visibleNodes;
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
    if (nodeId === 'todo-section' && node) {
      console.log(`🔍 DEBUG findNode for todo-section:`, {
        id: node.id,
        children: node.children,
        childrenType: typeof node.children,
        fullNode: node
      });
    }
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
      console.log(`🌳 Taking ELSE branch - node goes AFTER as sibling`);
      console.log(`🌳 AfterNode children count: ${afterNode.children?.length || 0}`);
      if (afterNode.children && afterNode.children.length > 0) {
        console.log(`🌳 Transferring ${afterNode.children.length} children to new node`);
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
        console.log(`🌳 Cleared afterNode children, transferred to new node`);
      } else {
        console.log(`🌳 AfterNode has no children to transfer`);
      }

      // Insert new node as sibling after afterNode
      console.log(`🌳 Inserting new node after afterNode. AfterNode parentId: ${afterNode.parentId}`);
      if (afterNode.parentId) {
        console.log(`🌳 AfterNode has parent, inserting in parent's children`);
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
        console.log(`🌳 AfterNode is a root node, inserting in _rootNodeIds`);
        const afterNodeIndex = _rootNodeIds.indexOf(afterNodeId);
        console.log(`🌳 AfterNode index in roots: ${afterNodeIndex}, current _rootNodeIds: ${_rootNodeIds.length}`);
        _rootNodeIds = [
          ..._rootNodeIds.slice(0, afterNodeIndex + 1),
          nodeId,
          ..._rootNodeIds.slice(afterNodeIndex + 1)
        ];
        console.log(`🌳 Reassigned _rootNodeIds array, new length: ${_rootNodeIds.length}`);

        // Manually trigger reactivity
        _updateTrigger++;
        console.log(`🔄 Incremented trigger to: ${_updateTrigger}`);
      }
    }

    console.log(`🎉 About to dispatch nodeCreated event for ${nodeId}`);
    events.nodeCreated(nodeId);
    console.log(`✅ nodeCreated event dispatched, returning nodeId: ${nodeId}`);
    return nodeId;
  }

  function createPlaceholderNode(
    afterNodeId: string,
    nodeType: string = 'text',
    headerLevel?: number
  ): string {
    return createNode(afterNodeId, '', nodeType, headerLevel, false);
  }

  // Debounce timers for expensive operations
  const debouncedOperations = new Map<
    string,
    {
      fastTimer?: number;
      expensiveTimer?: number;
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
    console.log('🔧 Rebuilding children arrays from parent relationships');

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

    console.log('🔧 Children arrays rebuilt successfully');
  }

  function updateDescendantDepths(node: Node): void {
    const expectedDepth = node.parentId ? (findNode(node.parentId)?.depth ?? 0) + 1 : 0;

    console.log(`🔧 updateDescendantDepths for ${node.id}: current depth=${node.depth}, expected depth=${expectedDepth}`);

    if (node.depth !== expectedDepth) {
      const depthDiff = expectedDepth - node.depth;
      console.log(`📏 Updating ${node.id} depth: ${node.depth} -> ${expectedDepth} (diff: ${depthDiff})`);

      // Use assignment-based reactivity - reassign the entire object
      _nodes[node.id] = { ...node, depth: expectedDepth };
    } else {
      console.log(`✅ ${node.id} depth already correct (${node.depth})`);
    }

    // ALWAYS update children depths regardless of whether parent depth changed
    // Children should have depth = parent.depth + 1
    const expectedChildDepth = node.depth + 1;
    for (const childId of node.children) {
      const child = _nodes[childId];
      if (child) {
        const oldChildDepth = child.depth;
        if (child.depth !== expectedChildDepth) {
          console.log(`📏 Updating child ${childId} depth: ${oldChildDepth} -> ${expectedChildDepth}`);
          // Use assignment-based reactivity - reassign the entire object
          _nodes[childId] = { ...child, depth: expectedChildDepth };
        } else {
          console.log(`✅ Child ${childId} depth already correct (${child.depth})`);
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

    // Handle child-to-parent merging
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

        // Update children's parent reference and depths
        for (const childId of currentNode.children) {
          const child = newNodesRecord[childId];
          if (child) {
            // Update parent reference and recalculate depth
            const newDepth = previousNode.depth + 1; // Children should be one level deeper than the merged parent
            newNodesRecord[childId] = { ...child, parentId: previousNodeId, depth: newDepth };
          }
        }
      }
    }

    // Update previous node and remove current node atomically
    newNodesRecord[previousNodeId] = updatedPreviousNode;
    delete newNodesRecord[currentNodeId];

    console.log(`🎯 combineNodes: Combining "${currentNode.content}" into "${previousNode.content}"`);
    console.log(`🔄 combineNodes: Combined content will be: "${combinedContent}"`);
    console.log(`📍 combineNodes: Cursor will be positioned at index ${mergePosition} in node ${previousNodeId}`);

    // Apply atomic changes
    Object.keys(_nodes).forEach(key => delete _nodes[key]);
    Object.entries(newNodesRecord).forEach(([id, node]) => {
      _nodes[id] = node;
    });

    // Update depths for all descendants of promoted children (if child-to-parent merge)
    if (isChildToParent && currentNode.children.length > 0) {
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
    console.log(`🎯 combineNodes: Requesting focus on ${previousNodeId} at position ${mergePosition}`);
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
    console.log(`🔧 indentNode called for node: ${nodeId}`);
    const node = findNode(nodeId);
    if (!node) {
      console.log(`❌ indentNode: node not found: ${nodeId}`);
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

    return true;
  }

  function collectAllFollowingNodes(targetNodeId: string, currentDepth: number): string[] {
    const result: string[] = [];
    const visibleNodes = getVisibleNodes();

    // Find the index of the target node in the visible list
    const targetIndex = visibleNodes.findIndex(node => node.id === targetNodeId);
    if (targetIndex === -1) return result;

    // Collect all nodes after the target in tree order
    for (let i = targetIndex + 1; i < visibleNodes.length; i++) {
      const node = visibleNodes[i];

      // Stop if we encounter a node at the same or lesser depth than current depth
      // (this means we've moved out of the scope that should be affected)
      if (node.depth <= currentDepth) {
        break;
      }

      result.push(node.id);
    }

    // console.log(`🔍 collectAllFollowingNodes for ${targetNodeId} (depth ${currentDepth}):`, result);
    return result;
  }

  function outdentNode(nodeId: string): boolean {
    const node = findNode(nodeId);

    if (!node || !node.parentId) {
      console.log(`❌ outdentNode: node not found or no parent: ${nodeId}, parentId: ${node?.parentId}`);
      return false;
    }

    console.log(`🔍 Looking for parent: ${node.parentId}`);
    const parent = findNode(node.parentId);
    console.log(`🔍 Parent found:`, {
      found: !!parent,
      id: parent?.id,
      children: parent?.children
    });

    if (!parent) {
      console.log(`❌ outdentNode: parent not found: ${node.parentId}`);
      return false;
    }

    // WORKAROUND: Rebuild children arrays from parent relationships if corrupted
    if (!parent.children || !Array.isArray(parent.children) || parent.children.indexOf(nodeId) === -1) {
      console.log(`🔧 WORKAROUND: rebuilding children arrays from parent relationships`);
      rebuildChildrenArrays();

      // Re-fetch parent after rebuilding
      const rebuiltParent = findNode(node.parentId);
      if (!rebuiltParent || !rebuiltParent.children || rebuiltParent.children.indexOf(nodeId) === -1) {
        console.log(`❌ outdentNode: still can't find node in parent's children after rebuild`);
        return false;
      }

      // Update parent reference
      Object.assign(parent, rebuiltParent);
    }

    // Find the position of this node among its siblings
    const nodeIndexInParent = parent.children.indexOf(nodeId);
    console.log(`🔍 nodeIndexInParent: ${nodeIndexInParent}, parent.children:`, parent.children);
    if (nodeIndexInParent === -1) {
      console.log(`❌ outdentNode: node ${nodeId} not found in parent's children`);
      return false;
    }

    // Collect siblings that come after this node - they will become children of the outdented node
    const followingSiblings = parent.children.slice(nodeIndexInParent + 1);
    console.log(`🔍 Following siblings that will become children:`, followingSiblings);

    // Remove the outdented node AND all following siblings from the parent
    parent.children = parent.children.slice(0, nodeIndexInParent);
    console.log(`🔍 After removing node and following siblings, parent.children:`, parent.children);

    // Determine where to place the outdented node
    console.log(`🔍 Determining placement for outdented node ${nodeId}`);
    let newParentId: string | undefined;
    let insertionTarget: { isRoot: boolean; parentId?: string; insertIndex: number };

    if (parent.parentId) {
      // Parent has a parent (grandparent exists)
      console.log(`🔍 Parent has grandparent: ${parent.parentId}`);
      const grandparent = findNode(parent.parentId);
      if (!grandparent) {
        console.log(`❌ Grandparent not found: ${parent.parentId}`);
        return false;
      }

      const parentIndexInGrandparent = grandparent.children.indexOf(parent.id);
      console.log(`🔍 Parent index in grandparent: ${parentIndexInGrandparent}`);
      newParentId = parent.parentId;
      insertionTarget = {
        isRoot: false,
        parentId: parent.parentId,
        insertIndex: parentIndexInGrandparent + 1
      };
      console.log(`🔍 Will insert as child of grandparent at index ${insertionTarget.insertIndex}`);
    } else {
      // Parent is a root node, so outdented node becomes root
      console.log(`🔍 Parent is root node, outdented node will become root`);
      const parentIndexInRoot = _rootNodeIds.indexOf(parent.id);
      console.log(`🔍 Parent index in root: ${parentIndexInRoot}`);
      newParentId = undefined;
      insertionTarget = {
        isRoot: true,
        insertIndex: parentIndexInRoot + 1
      };
      console.log(`🔍 Will insert as root at index ${insertionTarget.insertIndex}`);
    }

    // Update the outdented node with assignment-based reactivity
    const newDepth = newParentId ? (findNode(newParentId)?.depth ?? 0) + 1 : 0;
    console.log(`🔧 Updating outdented node ${nodeId}: parentId ${node.parentId} -> ${newParentId}, depth ${node.depth} -> ${newDepth}`);

    _nodes[nodeId] = { ...node, parentId: newParentId, depth: newDepth };
    // Update our local reference to the updated node
    const updatedNode = _nodes[nodeId];
    console.log(`✅ Node updated: parentId=${updatedNode.parentId}, depth=${updatedNode.depth}`);

    // Insert the outdented node at its new position
    console.log(`🔧 Inserting node at position: isRoot=${insertionTarget.isRoot}, parentId=${insertionTarget.parentId}, index=${insertionTarget.insertIndex}`);
    if (insertionTarget.isRoot) {
      console.log(`🔧 Inserting ${nodeId} as root at index ${insertionTarget.insertIndex}`);
      _rootNodeIds.splice(insertionTarget.insertIndex, 0, nodeId);
      console.log(`✅ Root node inserted. New root IDs:`, $state.snapshot(_rootNodeIds));
    } else {
      console.log(`🔧 Looking for new parent: ${insertionTarget.parentId}`);
      const newParent = findNode(insertionTarget.parentId!);
      if (newParent) {
        console.log(`🔧 Inserting ${nodeId} into parent ${insertionTarget.parentId} children at index ${insertionTarget.insertIndex}`);
        console.log(`🔍 Parent children before:`, $state.snapshot(newParent.children));
        newParent.children.splice(insertionTarget.insertIndex, 0, nodeId);
        console.log(`✅ Node inserted. Parent children after:`, $state.snapshot(newParent.children));
      } else {
        console.error(`❌ Could not find new parent: ${insertionTarget.parentId}`);
      }
    }

    // Add the following siblings as children of the outdented node (after existing children)
    if (followingSiblings.length > 0) {
      console.log(`🔧 Adding ${followingSiblings.length} following siblings as children of outdented node`);

      // Update their parent references in the reactive storage
      for (const siblingId of followingSiblings) {
        const siblingNode = findNode(siblingId);
        if (siblingNode) {
          console.log(`📦 Before: ${siblingId} parentId=${siblingNode.parentId}, depth=${siblingNode.depth}`);

          // Create updated node and save back to reactive storage
          const updatedSibling = { ...siblingNode, parentId: nodeId };
          _nodes[siblingId] = updatedSibling;

          console.log(`📦 After: ${siblingId} parentId=${updatedSibling.parentId}`);
        } else {
          console.error(`❌ Could not find sibling node: ${siblingId}`);
        }
      }

      // Add them to the outdented node's children (after existing children)
      const newChildren = [...updatedNode.children, ...followingSiblings];
      _nodes[nodeId] = { ...updatedNode, children: newChildren };
      console.log(`✅ Outdented node children updated:`, newChildren);
      console.log(`✅ Outdented node saved to reactive storage`);
    }

    // Update depths for the outdented node and all its children (including the new ones)
    console.log(`🔧 About to update depths for outdented node and all descendants`);
    const finalUpdatedNode = _nodes[nodeId]; // Get the latest version with updated children
    updateDescendantDepths(finalUpdatedNode);
    console.log(`✅ Depths updated for outdented node and all descendants`);

    console.log(`🔧 Triggering hierarchy change event`);
    events.hierarchyChanged();

    console.log(`✅ Outdent operation completed successfully for node: ${nodeId}`);
    return true;
  }

  function repositionFollowingNodes(
    outdentedNodeId: string,
    followingNodes: string[],
    outdentedDepth: number
  ): void {
    const outdentedNode = findNode(outdentedNodeId);
    if (!outdentedNode) return;

    // First, remove all following nodes from their current parents
    const nodesToReposition: { node: Node; id: string }[] = [];
    for (const nodeId of followingNodes) {
      const followingNode = findNode(nodeId);
      if (!followingNode) continue;

      // Remove from current parent
      if (followingNode.parentId) {
        const currentParent = findNode(followingNode.parentId);
        if (currentParent) {
          currentParent.children = currentParent.children.filter(id => id !== nodeId);
        }
      } else {
        // Remove from root nodes
        const rootIndex = _rootNodeIds.indexOf(nodeId);
        if (rootIndex !== -1) {
          _rootNodeIds.splice(rootIndex, 1);
        }
      }

      nodesToReposition.push({ node: followingNode, id: nodeId });
    }

    // Group nodes by their new parent and insert them in order
    const childrenForOutdentedNode: string[] = [];
    const siblingsForNewParent: string[] = [];

    // Separate nodes based on where they should go
    for (const { node: followingNode, id: nodeId } of nodesToReposition) {
      // Update parent reference first
      if (followingNode.depth > outdentedDepth) {
        // Following node should become a child of the outdented node
        followingNode.parentId = outdentedNodeId;
        childrenForOutdentedNode.push(nodeId);
        console.log(`📦 Moving ${nodeId} (depth ${followingNode.depth}) as child of outdented node ${outdentedNodeId}`);
      } else {
        // Following node should become a sibling of the outdented node
        followingNode.parentId = outdentedNode.parentId;
        siblingsForNewParent.push(nodeId);
        console.log(`📦 Moving ${nodeId} (depth ${followingNode.depth}) as sibling of outdented node ${outdentedNodeId}`);
      }

      // Update depths for the repositioned node and its descendants
      updateDescendantDepths(followingNode);
    }

    // Insert children into the outdented node (at the end since they maintain their internal order)
    outdentedNode.children.push(...childrenForOutdentedNode);

    // Insert siblings into the new parent right after the outdented node
    if (siblingsForNewParent.length > 0) {
      if (outdentedNode.parentId) {
        const newParent = findNode(outdentedNode.parentId);
        if (newParent) {
          const outdentedNodeIndex = newParent.children.indexOf(outdentedNodeId);
          if (outdentedNodeIndex !== -1) {
            // Insert right after the outdented node to maintain order
            newParent.children.splice(outdentedNodeIndex + 1, 0, ...siblingsForNewParent);
          } else {
            // Fallback: add at the end
            newParent.children.push(...siblingsForNewParent);
          }
        }
      } else {
        // Outdented node is at root level, insert siblings after it in root
        const outdentedNodeIndex = _rootNodeIds.indexOf(outdentedNodeId);
        if (outdentedNodeIndex !== -1) {
          _rootNodeIds.splice(outdentedNodeIndex + 1, 0, ...siblingsForNewParent);
        } else {
          _rootNodeIds.push(...siblingsForNewParent);
        }
      }
    }
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

      // Toggle the expanded state using assignment-based reactivity
      const newExpandedState = !node.expanded;
      _nodes[nodeId] = { ...node, expanded: newExpandedState };

      console.log(`✅ Toggled node expansion: ${nodeId} -> ${newExpandedState ? 'expanded' : 'collapsed'}`);
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


  function initializeWithRichDemoData(): void {
    const today = new Date();
    const todayStr = today.toISOString().split('T')[0]; // YYYY-MM-DD format

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
    Object.keys(_nodes).forEach(key => delete _nodes[key]);
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

      console.log(`🔧 Adding node ${preservedNode.id} with children:`, preservedNode.children);
      _nodes[preservedNode.id] = preservedNode;

      // Debug: Immediately check what was stored
      const storedNode = _nodes[preservedNode.id];
      console.log(`🔍 Stored node ${preservedNode.id} children:`, storedNode?.children);

      if (preservedNode.depth === 0) {
        _rootNodeIds = [..._rootNodeIds, preservedNode.id];
      }
    }

    console.log('ReactiveNodeService initialized with rich demo data');

    // Debug: Check node relationships after initialization
    console.log('🔍 Node relationships after init:');
    for (const [nodeId, node] of Object.entries(_nodes)) {
      if (node.parentId) {
        console.log(`  ${nodeId} -> parent: ${node.parentId}, depth: ${node.depth}`);
      } else {
        console.log(`  ${nodeId} -> ROOT (no parent), depth: ${node.depth}`);
      }
    }

    // Debug: Check if children are still intact after the loop
    const todoAfterLoop = _nodes['todo-section'];
    console.log('🔍 todo-section children after debug loop:', todoAfterLoop?.children);
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
      return visibleNodes;
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
    initializeWithRichDemoData
  };
}

export type ReactiveNodeService = ReturnType<typeof createReactiveNodeService>;

// For backward compatibility with existing imports - export both
export const ReactiveNodeService = createReactiveNodeService;
