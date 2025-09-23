/**
 * ReactiveNodeService - Svelte Store-Based Reactive Architecture
 *
 * Replaces NodeManager + ReactiveNodeManager + nodeStore with a single,
 * clean reactive service. Uses Svelte stores for automatic reactivity.
 */

import { writable, type Writable, type Readable } from 'svelte/store';
import { v4 as uuidv4 } from 'uuid';
import { ContentProcessor } from './contentProcessor';
import { eventBus } from './eventBus';
import type { NodeStatus } from './eventTypes';

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

export interface DeletionContext {
  type: 'content_merge' | 'empty_removal';
  parentId?: string;
  childrenIds: string[];
  childrenTransferredTo?: string;
  contentLost: string;
  siblingPosition: number;
  mergedIntoNode?: string;
}

export class ReactiveNodeService {
  // Reactive state using Svelte stores
  private _nodes: Map<string, Node> = new Map();
  private _rootNodeIds: string[] = [];
  private _collapsedNodes: Set<string> = new Set();
  private _activeNodeId: string | undefined = undefined;

  // Reactive store for visible nodes
  private _visibleNodesStore: Writable<Node[]> = writable([]);
  public readonly visibleNodes: Readable<Node[]>;

  private events: NodeManagerEvents;
  protected contentProcessor: ContentProcessor;
  protected readonly serviceName: string = 'ReactiveNodeService';

  constructor(events: NodeManagerEvents) {
    this.events = events;
    this.contentProcessor = ContentProcessor.getInstance();

    // Make visibleNodes a readable store
    this.visibleNodes = this._visibleNodesStore;

    // Set up EventBus subscriptions for coordination
    this.setupEventBusIntegration();
  }

  /**
   * Update the visible nodes store to trigger reactivity
   */
  private updateVisibleNodes(): void {
    const visible = this.getVisibleNodesRecursive(this._rootNodeIds);
    this._visibleNodesStore.set(visible);
  }

  /**
   * Set up EventBus integration for dynamic coordination
   */
  private setupEventBusIntegration(): void {
    // Listen for cache invalidation events that might affect nodes
    eventBus.subscribe('cache:invalidate', (event) => {
      const cacheEvent = event as import('./eventTypes').CacheInvalidateEvent;
      if (cacheEvent.scope === 'node' && cacheEvent.nodeId) {
        this.emitNodeStatusChanged(cacheEvent.nodeId, 'processing', 'cache invalidation');
      } else if (cacheEvent.scope === 'global') {
        for (const nodeId of Array.from(this._nodes.keys())) {
          this.emitDecorationUpdateNeeded(nodeId, 'cache-invalidated');
        }
      }
    });

    eventBus.subscribe('reference:resolved', (event) => {
      const refEvent = event as import('./eventTypes').ReferenceResolutionEvent;
      this.emitDecorationUpdateNeeded(refEvent.nodeId, 'reference-updated');
    });
  }

  /**
   * Get all nodes as a Map for efficient lookups
   */
  get nodes(): Map<string, Node> {
    return this._nodes;
  }

  /**
   * Get root node IDs
   */
  get rootNodeIds(): string[] {
    return this._rootNodeIds;
  }

  /**
   * Get collapsed nodes set
   */
  get collapsedNodes(): Set<string> {
    return this._collapsedNodes;
  }

  /**
   * Get/set active node ID
   */
  get activeNodeId(): string | undefined {
    return this._activeNodeId;
  }

  set activeNodeId(nodeId: string | undefined) {
    this._activeNodeId = nodeId;
  }

  // ========================================================================
  // Core Node Operations
  // ========================================================================

  /**
   * Clear autoFocus from all nodes
   */
  private clearAllAutoFocus(): void {
    for (const node of this._nodes.values()) {
      node.autoFocus = false;
    }
  }

  /**
   * Create a new node after the specified node
   */
  createNode(
    afterNodeId: string,
    content: string = '',
    nodeType: string = 'text',
    headerLevel?: number,
    insertAtBeginning: boolean = false
  ): string {
    const afterNode = this.findNode(afterNodeId);
    if (!afterNode) {
      console.warn(`Cannot create node: afterNode ${afterNodeId} not found`);
      return '';
    }

    // Clear autoFocus from all existing nodes to ensure only the new node has focus
    this.clearAllAutoFocus();

    // Determine hierarchy based on afterNode's expansion state and content splitting
    // Per documentation:
    // - Expanded nodes: Split content, new node gets children at same level
    // - Collapsed nodes: Create sibling, original keeps children
    let newDepth = afterNode.depth;
    let newParentId = afterNode.parentId;

    const newNode: Node = {
      id: uuidv4(),
      content,
      nodeType,
      depth: newDepth,
      parentId: newParentId,
      children: [],
      expanded: true,
      autoFocus: true,
      inheritHeaderLevel: headerLevel !== undefined ? headerLevel : afterNode.inheritHeaderLevel,
      metadata: {},
      isPlaceholder: content.trim() === ''
    };

    // Add node to map using assignment pattern
    this._nodes = new Map(this._nodes).set(newNode.id, newNode);

    // Handle hierarchy positioning using assignment patterns
    if (insertAtBeginning) {
      // Cursor at beginning: new node goes ABOVE, takes children if expanded
      if (afterNode.expanded && afterNode.children.length > 0) {
        // Transfer children to new node
        newNode.children = [...afterNode.children];
        afterNode.children.forEach((childId) => {
          const child = this.findNode(childId);
          if (child) {
            child.parentId = newNode.id;
            child.depth = newNode.depth + 1;
          }
        });
        afterNode.children = [];
      }

      // Insert new node before afterNode
      if (afterNode.parentId) {
        const parent = this.findNode(afterNode.parentId);
        if (parent) {
          const siblingIndex = parent.children.indexOf(afterNode.id);
          parent.children = [
            ...parent.children.slice(0, siblingIndex),
            newNode.id,
            ...parent.children.slice(siblingIndex)
          ];
        }
      } else {
        const rootIndex = this._rootNodeIds.indexOf(afterNode.id);
        this._rootNodeIds = [
          ...this._rootNodeIds.slice(0, rootIndex),
          newNode.id,
          ...this._rootNodeIds.slice(rootIndex)
        ];
      }
    } else {
      // Cursor in middle/end: content splitting behavior
      if (afterNode.expanded && afterNode.children.length > 0) {
        // For expanded nodes: new node gets the children
        newNode.children = [...afterNode.children];
        afterNode.children.forEach((childId) => {
          const child = this.findNode(childId);
          if (child) {
            child.parentId = newNode.id;
            child.depth = newNode.depth + 1;
          }
        });
        afterNode.children = [];
      }
      // For collapsed nodes: afterNode keeps children, create sibling

      // Insert new node after afterNode
      if (afterNode.parentId) {
        const parent = this.findNode(afterNode.parentId);
        if (parent) {
          const siblingIndex = parent.children.indexOf(afterNode.id);
          parent.children = [
            ...parent.children.slice(0, siblingIndex + 1),
            newNode.id,
            ...parent.children.slice(siblingIndex + 1)
          ];
        }
      } else {
        const rootIndex = this._rootNodeIds.indexOf(afterNode.id);
        this._rootNodeIds = [
          ...this._rootNodeIds.slice(0, rootIndex + 1),
          newNode.id,
          ...this._rootNodeIds.slice(rootIndex + 1)
        ];
      }
    }

    this.emitNodeCreated(newNode.id);
    this.events.nodeCreated(newNode.id);

    // Update the reactive store
    this.updateVisibleNodes();

    return newNode.id;
  }

  /**
   * Create a placeholder node (empty content, will be filled by user)
   */
  createPlaceholderNode(
    afterNodeId: string,
    nodeType: string = 'text',
    headerLevel?: number
  ): string {
    const afterNode = this.findNode(afterNodeId);
    if (!afterNode) {
      console.warn(`Cannot create placeholder: afterNode ${afterNodeId} not found`);
      return '';
    }

    // Clear autoFocus from all existing nodes to ensure only the new placeholder has focus
    this.clearAllAutoFocus();

    // Same logic as createNode - placeholders follow same hierarchical rules
    let newDepth = afterNode.depth;
    let newParentId = afterNode.parentId;

    const placeholderNode: Node = {
      id: uuidv4(),
      content: '',
      nodeType,
      depth: newDepth,
      parentId: newParentId,
      children: [],
      expanded: true,
      autoFocus: true,
      inheritHeaderLevel: headerLevel !== undefined ? headerLevel : afterNode.inheritHeaderLevel,
      metadata: {},
      isPlaceholder: true
    };

    this._nodes = new Map(this._nodes).set(placeholderNode.id, placeholderNode);

    // Handle hierarchy positioning (same logic as createNode)
    // Placeholders are typically created for Enter key at end of node
    if (afterNode.expanded && afterNode.children.length > 0) {
      // For expanded nodes: new placeholder gets the children
      placeholderNode.children = [...afterNode.children];
      afterNode.children.forEach((childId) => {
        const child = this.findNode(childId);
        if (child) {
          child.parentId = placeholderNode.id;
          child.depth = placeholderNode.depth + 1;
        }
      });
      afterNode.children = [];
    }
    // For collapsed nodes: afterNode keeps children, create sibling

    // Insert new placeholder after afterNode
    if (afterNode.parentId) {
      const parent = this.findNode(afterNode.parentId);
      if (parent) {
        const siblingIndex = parent.children.indexOf(afterNodeId);
        parent.children = [
          ...parent.children.slice(0, siblingIndex + 1),
          placeholderNode.id,
          ...parent.children.slice(siblingIndex + 1)
        ];
      }
    } else {
      const rootIndex = this._rootNodeIds.indexOf(afterNodeId);
      this._rootNodeIds = [
        ...this._rootNodeIds.slice(0, rootIndex + 1),
        placeholderNode.id,
        ...this._rootNodeIds.slice(rootIndex + 1)
      ];
    }

    this.emitNodeCreated(placeholderNode.id);
    this.events.nodeCreated(placeholderNode.id);

    // Update the reactive store
    this.updateVisibleNodes();

    return placeholderNode.id;
  }

  /**
   * Update node content
   */
  updateNodeContent(nodeId: string, content: string): void {
    const node = this.findNode(nodeId);
    if (!node) {
      console.warn(`Cannot update content: node ${nodeId} not found`);
      return;
    }

    const previousContent = node.content;
    node.content = content;
    node.isPlaceholder = content.trim() === '';

    this.emitNodeUpdated(nodeId, 'content', previousContent, content);
    this.emitDecorationUpdateNeeded(nodeId, 'content-changed');
  }

  /**
   * Update node type
   */
  updateNodeType(nodeId: string, nodeType: string): void {
    const node = this.findNode(nodeId);
    if (!node) {
      console.warn(`Cannot update type: node ${nodeId} not found`);
      return;
    }

    const previousType = node.nodeType;
    node.nodeType = nodeType;

    this.emitNodeUpdated(nodeId, 'nodeType', previousType, nodeType);
    this.emitDecorationUpdateNeeded(nodeId, 'nodeType-changed');
  }

  /**
   * Delete a node and handle children transfer
   */
  deleteNode(nodeId: string): void {
    const node = this.findNode(nodeId);
    if (!node) {
      console.warn(`Cannot delete: node ${nodeId} not found`);
      return;
    }

    if (node.parentId) {
      const parent = this.findNode(node.parentId);
      if (parent && node.children.length > 0) {
        const nodeIndex = parent.children.indexOf(nodeId);
        parent.children.splice(nodeIndex, 1, ...node.children);

        node.children.forEach((childId) => {
          const child = this.findNode(childId);
          if (child) {
            child.parentId = parent.id;
            child.depth = parent.depth + 1;
          }
        });
      } else if (parent) {
        const nodeIndex = parent.children.indexOf(nodeId);
        parent.children.splice(nodeIndex, 1);
      }
    } else {
      const rootIndex = this._rootNodeIds.indexOf(nodeId);
      if (rootIndex >= 0) {
        if (node.children.length > 0) {
          this._rootNodeIds.splice(rootIndex, 1, ...node.children);
          node.children.forEach((childId) => {
            const child = this.findNode(childId);
            if (child) {
              child.parentId = undefined;
              child.depth = 0;
            }
          });
        } else {
          this._rootNodeIds.splice(rootIndex, 1);
        }
      }
    }

    this._nodes.delete(nodeId);

    this.emitNodeDeleted(nodeId);
    this.events.nodeDeleted(nodeId);

    // Update the reactive store
    this.updateVisibleNodes();
  }

  /**
   * Find a node by ID
   */
  findNode(nodeId: string): Node | null {
    return this._nodes.get(nodeId) || null;
  }

  /**
   * Get siblings of a node
   */
  getSiblings(nodeId: string): string[] {
    const node = this.findNode(nodeId);
    if (!node) return [];

    return node.parentId ? this.findNode(node.parentId)?.children || [] : this._rootNodeIds;
  }

  /**
   * Get previous sibling of a node
   */
  getPreviousSibling(nodeId: string): Node | null {
    const siblings = this.getSiblings(nodeId);
    const currentIndex = siblings.indexOf(nodeId);
    const previousSiblingId = currentIndex > 0 ? siblings[currentIndex - 1] : null;
    return previousSiblingId ? this.findNode(previousSiblingId) : null;
  }

  // ========================================================================
  // Hierarchy Operations
  // ========================================================================

  /**
   * Indent a node (make it a child of previous sibling)
   */
  indentNode(nodeId: string): boolean {
    const node = this.findNode(nodeId);
    if (!node) return false;

    const previousSibling = this.getPreviousSibling(nodeId);
    if (!previousSibling) return false;

    if (node.parentId) {
      const parent = this.findNode(node.parentId);
      if (parent) {
        const index = parent.children.indexOf(nodeId);
        parent.children.splice(index, 1);
      }
    } else {
      const index = this._rootNodeIds.indexOf(nodeId);
      this._rootNodeIds.splice(index, 1);
    }

    previousSibling.children.push(nodeId);
    node.parentId = previousSibling.id;
    node.depth = previousSibling.depth + 1;

    this.updateDescendantDepths(node);

    this.emitHierarchyChanged([nodeId, previousSibling.id], 'indent');
    this.events.hierarchyChanged();

    // Update the reactive store
    this.updateVisibleNodes();

    return true;
  }

  /**
   * Outdent a node (move it up one level) with proper repositioning
   * The node moves left in hierarchy while maintaining visual position
   * Following siblings are repositioned based on their depth relationships
   */
  outdentNode(nodeId: string): boolean {
    const node = this.findNode(nodeId);
    if (!node || !node.parentId) return false;

    const parent = this.findNode(node.parentId);
    if (!parent) return false;

    // Find the position of this node among its siblings
    const nodeIndexInParent = parent.children.indexOf(nodeId);
    if (nodeIndexInParent === -1) return false;

    // Collect siblings that come after this node (they may need repositioning)
    const followingSiblings = parent.children.slice(nodeIndexInParent + 1);

    // Remove the node and all following siblings from parent
    parent.children = parent.children.slice(0, nodeIndexInParent);

    // Determine where to place the outdented node
    let newParentId: string | undefined;
    let insertionTarget: { isRoot: boolean; parentId?: string; insertIndex: number };

    if (parent.parentId) {
      // Parent has a parent (grandparent exists)
      const grandparent = this.findNode(parent.parentId);
      if (!grandparent) return false;

      const parentIndexInGrandparent = grandparent.children.indexOf(parent.id);
      newParentId = parent.parentId;
      insertionTarget = {
        isRoot: false,
        parentId: parent.parentId,
        insertIndex: parentIndexInGrandparent + 1
      };
    } else {
      // Parent is a root node, so outdented node becomes root
      const parentIndexInRoot = this._rootNodeIds.indexOf(parent.id);
      newParentId = undefined;
      insertionTarget = {
        isRoot: true,
        insertIndex: parentIndexInRoot + 1
      };
    }

    // Update the outdented node
    node.parentId = newParentId;
    node.depth = newParentId ? (this.findNode(newParentId)?.depth ?? 0) + 1 : 0;

    // Insert the outdented node at its new position
    if (insertionTarget.isRoot) {
      this._rootNodeIds.splice(insertionTarget.insertIndex, 0, nodeId);
    } else {
      const newParent = this.findNode(insertionTarget.parentId!);
      if (newParent) {
        newParent.children.splice(insertionTarget.insertIndex, 0, nodeId);
      }
    }

    // Update depths for the outdented node and its children
    this.updateDescendantDepths(node);

    // Now handle following siblings - they need to be repositioned
    // based on their depth relative to the outdented node
    this.repositionFollowingSiblings(nodeId, followingSiblings, node.depth);

    this.emitHierarchyChanged([nodeId, parent.id, ...followingSiblings], 'outdent');
    this.events.hierarchyChanged();

    // Update the reactive store
    this.updateVisibleNodes();

    return true;
  }

  /**
   * Reposition siblings that came after an outdented node
   * They become children of the outdented node or siblings based on depth
   */
  private repositionFollowingSiblings(
    outdentedNodeId: string,
    followingSiblings: string[],
    outdentedDepth: number
  ): void {
    const outdentedNode = this.findNode(outdentedNodeId);
    if (!outdentedNode) return;

    for (const siblingId of followingSiblings) {
      const sibling = this.findNode(siblingId);
      if (!sibling) continue;

      // Determine the new relationship based on depth
      if (sibling.depth > outdentedDepth) {
        // Sibling should become a child of the outdented node
        sibling.parentId = outdentedNodeId;
        outdentedNode.children.push(siblingId);
      } else {
        // Sibling should become a sibling of the outdented node
        sibling.parentId = outdentedNode.parentId;

        if (outdentedNode.parentId) {
          const newParent = this.findNode(outdentedNode.parentId);
          if (newParent) {
            newParent.children.push(siblingId);
          }
        } else {
          this._rootNodeIds.push(siblingId);
        }
      }

      // Update depths for the repositioned sibling and its descendants
      this.updateDescendantDepths(sibling);
    }
  }

  /**
   * Toggle node expansion
   */
  toggleNodeExpansion(nodeId: string): boolean {
    const node = this.findNode(nodeId);
    if (!node) return false;

    node.expanded = !node.expanded;

    if (node.expanded) {
      this._collapsedNodes.delete(nodeId);
    } else {
      this._collapsedNodes.add(nodeId);
    }

    this.emitHierarchyChanged([nodeId], node.expanded ? 'expand' : 'collapse');

    // Update the reactive store
    this.updateVisibleNodes();

    return true;
  }

  /**
   * Toggle expanded state of a node
   */
  toggleExpanded(nodeId: string): boolean {
    const node = this._nodes.get(nodeId);
    if (!node) return false;

    node.expanded = !node.expanded;

    if (node.expanded) {
      this._collapsedNodes.delete(nodeId);
    } else {
      this._collapsedNodes.add(nodeId);
    }

    this.emitHierarchyChanged([nodeId], node.expanded ? 'expand' : 'collapse');

    // Update the reactive store
    this.updateVisibleNodes();

    return true;
  }

  /**
   * Convert placeholder node to real node
   */
  convertPlaceholderToReal(nodeId: string, content: string): boolean {
    const node = this._nodes.get(nodeId);
    if (!node || !node.isPlaceholder) return false;

    node.content = content;
    node.isPlaceholder = false;

    this.emitContentChanged(nodeId, content);
    return true;
  }

  /**
   * Add external node (e.g., from node reference service)
   */
  addExternalNode(nodeData: {
    id: string;
    content: string;
    nodeType: string;
    parentId?: string;
    depth?: number;
    metadata?: Record<string, unknown>;
  }): boolean {
    const existingNode = this._nodes.get(nodeData.id);
    if (existingNode) return false;

    const newNode: Node = {
      id: nodeData.id,
      content: nodeData.content,
      nodeType: nodeData.nodeType,
      depth: nodeData.depth ?? 0,
      parentId: nodeData.parentId ?? undefined,
      children: [],
      expanded: true,
      autoFocus: false,
      inheritHeaderLevel: 0,
      metadata: nodeData.metadata ?? {}
    };

    this._nodes.set(nodeData.id, newNode);

    if (!nodeData.parentId) {
      this._rootNodeIds.push(nodeData.id);
    } else {
      const parent = this._nodes.get(nodeData.parentId);
      if (parent) {
        parent.children.push(nodeData.id);
      }
    }

    this.emitContentChanged(nodeData.id, nodeData.content);
    return true;
  }

  /**
   * Initialize from legacy data (for tests and migration)
   */
  initializeFromLegacyData(nodes: Node[]): void {
    this._nodes.clear();
    this._rootNodeIds.length = 0;
    this._collapsedNodes.clear();

    for (const node of nodes) {
      this._nodes.set(node.id, node);
      if (node.depth === 0 || !node.parentId) {
        this._rootNodeIds.push(node.id);
      }
    }

    console.log(`ReactiveNodeService initialized with ${nodes.length} legacy nodes`);
  }

  /**
   * Initialize with rich demo data
   */
  initializeWithSampleData(): void {
    const sampleNodes: Node[] = [
      {
        id: 'demo-1',
        content: '# Welcome to NodeSpace',
        nodeType: 'text',
        depth: 0,
        children: ['demo-2', 'demo-3', 'demo-4'],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 1,
        metadata: {}
      },
      {
        id: 'demo-2',
        content: '## This is a child node',
        nodeType: 'text',
        depth: 1,
        parentId: 'demo-1',
        children: ['demo-5'],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 2,
        metadata: {}
      },
      {
        id: 'demo-5',
        content: 'Test *__bold__* and **_italic_** text, plus __bold__ and _italic_ for testing',
        nodeType: 'text',
        depth: 2,
        parentId: 'demo-2',
        children: [],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: {}
      },
      {
        id: 'demo-3',
        content: 'This is another child node',
        nodeType: 'text',
        depth: 1,
        parentId: 'demo-1',
        children: [],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: {}
      },
      {
        id: 'demo-4',
        content: 'This is a sample task node - try the /task slash command to create more!',
        nodeType: 'task',
        depth: 1,
        parentId: 'demo-1',
        children: [],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: { taskState: 'pending' }
      }
    ];

    this._nodes.clear();
    this._rootNodeIds.length = 0;
    this._collapsedNodes.clear();

    for (const node of sampleNodes) {
      this._nodes.set(node.id, node);
      if (node.depth === 0) {
        this._rootNodeIds.push(node.id);
      }
    }

    // Update the reactive store with initial data
    this.updateVisibleNodes();

    console.log('ReactiveNodeService initialized with rich demo data');
  }

  /**
   * Combine current node with previous node (for backspace behavior)
   */
  combineNodes(currentNodeId: string, previousNodeId: string): void {
    const currentNode = this.findNode(currentNodeId);
    const previousNode = this.findNode(previousNodeId);

    if (!currentNode || !previousNode) {
      console.warn('Cannot combine nodes: one or both nodes not found');
      return;
    }

    const currentContent = currentNode.content;
    const previousContent = previousNode.content;
    const isEmpty = currentContent.trim() === '';

    if (isEmpty) {
      // Empty node - just delete and focus previous
      this.deleteNode(currentNodeId);
      this.events.focusRequested(previousNodeId, previousContent.length);
    } else {
      // Atomic merge operation - do all changes in a single update
      const junctionPosition = previousContent.length;

      // Strip formatting syntax from current node content since it will inherit target node's format
      const cleanedCurrentContent = this.stripFormattingSyntax(currentContent);
      const mergedContent = previousContent + cleanedCurrentContent;

      // Create new nodes map with all changes applied atomically
      const newNodesMap = new Map(this._nodes);

      // Update previous node content
      const updatedPreviousNode = { ...previousNode, content: mergedContent };

      // Handle children based on relationship between current and previous nodes
      // Check if previous node is the parent of current node
      const isChildToParent = currentNode.parentId === previousNodeId;
      console.log('combineNodes debug:', {
        currentNodeId,
        previousNodeId,
        currentParentId: currentNode.parentId,
        isChildToParent,
        currentNodeChildrenCount: currentNode.children.length,
        previousNodeChildrenCount: previousNode.children.length
      });

      if (isChildToParent) {
        // Child-to-parent merge: promote children to same level as deleted node
        // Find the position of the current node in parent's children
        const currentNodeIndex = previousNode.children.indexOf(currentNodeId);
        if (currentNodeIndex >= 0) {
          // Remove current node and insert its children in its place
          const beforeChildren = previousNode.children.slice(0, currentNodeIndex);
          const afterChildren = previousNode.children.slice(currentNodeIndex + 1);
          updatedPreviousNode.children = [
            ...beforeChildren,
            ...currentNode.children,
            ...afterChildren
          ];
        }

        // Update parent references for promoted children (they keep same parent)
        currentNode.children.forEach((childId) => {
          const child = this.findNode(childId);
          if (child) {
            const updatedChild = { ...child, parentId: previousNodeId, depth: child.depth - 1 };
            newNodesMap.set(childId, updatedChild);
          }
        });
      } else if (currentNode.children.length > 0) {
        // Sibling-to-sibling merge: transfer children to the previous node
        updatedPreviousNode.children = [...previousNode.children, ...currentNode.children];

        // Update parent references for transferred children
        currentNode.children.forEach((childId) => {
          const child = this.findNode(childId);
          if (child) {
            const updatedChild = { ...child, parentId: previousNodeId };
            newNodesMap.set(childId, updatedChild);
          }
        });
      }

      // Update the previous node
      newNodesMap.set(previousNodeId, updatedPreviousNode);

      // Remove current node from parent's children array or root array
      // BUT skip this if we already handled child promotion above (child-to-parent case)
      if (currentNode.parentId && !isChildToParent) {
        // Only update parent if it's NOT the same as previousNode (avoid overwriting our promotion logic)
        const parent = this.findNode(currentNode.parentId);
        if (parent) {
          const nodeIndex = parent.children.indexOf(currentNodeId);
          if (nodeIndex >= 0) {
            const updatedParent = {
              ...parent,
              children: parent.children.filter((id) => id !== currentNodeId)
            };
            newNodesMap.set(parent.id, updatedParent);
          }
        }
      } else if (!currentNode.parentId) {
        // Handle root-level node removal
        const rootIndex = this._rootNodeIds.indexOf(currentNodeId);
        if (rootIndex >= 0) {
          this._rootNodeIds = [...this._rootNodeIds];
          this._rootNodeIds.splice(rootIndex, 1);
        }
      }

      // Remove current node from the map
      newNodesMap.delete(currentNodeId);

      // Apply all changes atomically
      this._nodes = newNodesMap;

      // Emit events
      this.emitNodeDeleted(currentNodeId);
      this.events.nodeDeleted(currentNodeId);

      // Update visible nodes once after all changes
      this.updateVisibleNodes();

      // Focus at junction point
      this.events.focusRequested(previousNodeId, junctionPosition);
    }
  }

  /**
   * Set active node
   */
  setActiveNode(nodeId: string | undefined): void {
    this._activeNodeId = nodeId;
  }

  // ========================================================================
  // Helper Methods
  // ========================================================================

  /**
   * Strip formatting syntax from content when merging nodes
   * Removes markdown headers so content inherits target node's format
   * Note: Task checkbox syntax is only present when typed as shortcut, not in node content
   */
  private stripFormattingSyntax(content: string): string {
    if (!content) return content;

    let cleaned = content;

    // Remove markdown headers (# ## ###)
    cleaned = cleaned.replace(/^#{1,6}\s+/, '');

    // Remove task checkbox syntax ([ ] [x]) - though this should be rare
    // since task nodes typically don't store this syntax in content
    cleaned = cleaned.replace(/^\[\s*[x\s]*\]\s*/, '');

    // Remove leading/trailing whitespace but preserve internal spacing
    cleaned = cleaned.trim();

    return cleaned;
  }

  /**
   * Get visible nodes recursively respecting expansion state
   */
  private getVisibleNodesRecursive(nodeIds: string[]): Node[] {
    const result: Node[] = [];

    for (const nodeId of nodeIds) {
      const node = this.findNode(nodeId);
      if (node) {
        result.push(node);

        if (node.expanded && node.children.length > 0) {
          result.push(...this.getVisibleNodesRecursive(node.children));
        }
      }
    }

    return result;
  }

  /**
   * Update depths of all descendants recursively
   */
  private updateDescendantDepths(node: Node): void {
    for (const childId of node.children) {
      const child = this.findNode(childId);
      if (child) {
        child.depth = node.depth + 1;
        this.updateDescendantDepths(child);
      }
    }
  }

  // ========================================================================
  // Event Emission Methods
  // ========================================================================

  private emitNodeStatusChanged(nodeId: string, status: NodeStatus, reason?: string): void {
    const statusEvent: Omit<import('./eventTypes').NodeStatusChangedEvent, 'timestamp'> = {
      type: 'node:status-changed',
      namespace: 'coordination',
      source: this.serviceName,
      nodeId,
      status,
      metadata: reason ? { reason } : undefined
    };

    eventBus.emit(statusEvent);
  }

  private emitDecorationUpdateNeeded(
    nodeId: string,
    reason:
      | 'content-changed'
      | 'status-changed'
      | 'reference-updated'
      | 'cache-invalidated'
      | 'nodeType-changed'
  ): void {
    const decorationUpdateEvent: Omit<
      import('./eventTypes').DecorationUpdateNeededEvent,
      'timestamp'
    > = {
      type: 'decoration:update-needed',
      namespace: 'interaction',
      source: this.serviceName,
      nodeId,
      decorationType: 'all',
      reason,
      metadata: {}
    };

    eventBus.emit(decorationUpdateEvent);
  }

  private emitNodeCreated(nodeId: string): void {
    const nodeCreatedEvent: Omit<import('./eventTypes').NodeCreatedEvent, 'timestamp'> = {
      type: 'node:created',
      namespace: 'lifecycle',
      source: this.serviceName,
      nodeId,
      nodeType: this.findNode(nodeId)?.nodeType || 'unknown'
    };

    eventBus.emit(nodeCreatedEvent);
  }

  private emitNodeUpdated(
    nodeId: string,
    updateType: 'content' | 'hierarchy' | 'status' | 'metadata' | 'nodeType',
    previousValue?: unknown,
    newValue?: unknown
  ): void {
    const nodeUpdatedEvent: Omit<import('./eventTypes').NodeUpdatedEvent, 'timestamp'> = {
      type: 'node:updated',
      namespace: 'lifecycle',
      source: this.serviceName,
      nodeId,
      updateType,
      previousValue,
      newValue
    };

    eventBus.emit(nodeUpdatedEvent);
  }

  private emitNodeDeleted(nodeId: string): void {
    const nodeDeletedEvent: Omit<import('./eventTypes').NodeDeletedEvent, 'timestamp'> = {
      type: 'node:deleted',
      namespace: 'lifecycle',
      source: this.serviceName,
      nodeId
    };

    eventBus.emit(nodeDeletedEvent);
  }

  private emitContentChanged(nodeId: string, content: string): void {
    const nodeUpdatedEvent: Omit<import('./eventTypes').NodeUpdatedEvent, 'timestamp'> = {
      type: 'node:updated',
      namespace: 'lifecycle',
      source: this.serviceName,
      nodeId,
      updateType: 'content',
      newValue: content
    };

    eventBus.emit(nodeUpdatedEvent);
  }

  private emitHierarchyChanged(
    affectedNodes: string[],
    changeType: 'indent' | 'outdent' | 'move' | 'expand' | 'collapse'
  ): void {
    const hierarchyChangedEvent: Omit<import('./eventTypes').HierarchyChangedEvent, 'timestamp'> = {
      type: 'hierarchy:changed',
      namespace: 'lifecycle',
      source: this.serviceName,
      affectedNodes,
      changeType
    };

    eventBus.emit(hierarchyChangedEvent);
  }
}
