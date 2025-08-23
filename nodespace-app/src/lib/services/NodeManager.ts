/**
 * NodeManager Service
 *
 * Centralized service for managing node operations with Svelte 5 $state() reactivity.
 * Replaces complex inline node management logic in BaseNodeViewer.
 *
 * Key Features:
 * - Map-based node storage for efficient lookups
 * - Reactive node state with automatic UI updates
 * - Event-driven communication with UI components
 * - Data migration from legacy array-based structure
 * - Efficient backspace combination logic
 */

import { v4 as uuidv4 } from 'uuid';
import { ContentProcessor } from './contentProcessor';
import { eventBus } from './EventBus';
import type { NodeStatus } from './EventTypes';

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

export class NodeManager {
  private _nodes: Map<string, Node>;
  private _rootNodeIds: string[];
  private _collapsedNodes: Set<string>;
  private events: NodeManagerEvents;
  protected contentProcessor: ContentProcessor;
  private activeNodeId?: string;
  protected readonly serviceName = 'NodeManager';

  constructor(events: NodeManagerEvents) {
    this._nodes = new Map<string, Node>();
    this._rootNodeIds = [];
    this._collapsedNodes = new Set<string>();
    this.events = events;
    this.contentProcessor = ContentProcessor.getInstance();

    // Set up EventBus subscriptions for coordination
    this.setupEventBusIntegration();
  }

  /**
   * Set up EventBus integration for dynamic coordination
   */
  private setupEventBusIntegration(): void {
    // Listen for cache invalidation events that might affect nodes
    eventBus.subscribe('cache:invalidate', (event) => {
      const cacheEvent = event as import('./EventTypes').CacheInvalidateEvent;
      if (cacheEvent.scope === 'node' && cacheEvent.nodeId) {
        // Node-specific cache invalidation - might need to refresh decorations
        this.emitNodeStatusChanged(cacheEvent.nodeId, 'processing', 'cache invalidation');
      } else if (cacheEvent.scope === 'global') {
        // Global cache invalidation - emit events for all nodes
        for (const nodeId of Array.from(this._nodes.keys())) {
          this.emitDecorationUpdateNeeded(nodeId, 'cache-invalidated');
        }
      }
    });

    // Listen for reference resolution events
    eventBus.subscribe('reference:resolved', (event) => {
      const refEvent = event as import('./EventTypes').ReferenceResolutionEvent;
      // When a reference is resolved, update any decorations that might be affected
      this.emitDecorationUpdateNeeded(refEvent.nodeId, 'reference-updated');
    });
  }

  /**
   * Reactive getter - automatically updates UI when nodes change
   * Returns all visible nodes in hierarchical order respecting expanded state
   */
  get visibleNodes(): Node[] {
    return this.getVisibleNodesRecursive(this._rootNodeIds);
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

  // ========================================================================
  // Dual-Representation Methods (ContentProcessor Integration)
  // ========================================================================

  /**
   * Parse node content as markdown and return AST
   * Enables content analysis and manipulation
   */
  parseNodeContent(nodeId: string) {
    const node = this.findNode(nodeId);
    if (!node) return null;
    return this.contentProcessor.parseMarkdown(node.content);
  }

  /**
   * Render node content as HTML for display
   * Supports formatted content presentation
   */
  async renderNodeAsHTML(nodeId: string): Promise<string> {
    const node = this.findNode(nodeId);
    if (!node) return '';
    const ast = this.contentProcessor.parseMarkdown(node.content);
    return await this.contentProcessor.renderAST(ast);
  }

  /**
   * Get header level for a node using ContentProcessor
   * Replaces manual header parsing throughout the codebase
   */
  getNodeHeaderLevel(nodeId: string): number {
    const node = this.findNode(nodeId);
    if (!node) return 0;
    return this.contentProcessor.parseHeaderLevel(node.content);
  }

  /**
   * Get display text without markdown syntax
   * Useful for navigation and search
   */
  getNodeDisplayText(nodeId: string): string {
    const node = this.findNode(nodeId);
    if (!node) return '';
    return this.contentProcessor.stripHeaderSyntax(node.content);
  }

  /**
   * Update node content with ContentProcessor validation
   * Ensures content integrity and triggers content analysis
   */
  updateNodeContentWithProcessing(nodeId: string, content: string): boolean {
    const node = this.findNode(nodeId);
    if (!node) return false;

    // Use ContentProcessor to validate and analyze content
    this.contentProcessor.parseMarkdown(content);

    // Update the node content
    node.content = content;

    // Update header level if it's a header node
    const headerLevel = this.contentProcessor.parseHeaderLevel(content);
    if (headerLevel > 0) {
      node.inheritHeaderLevel = headerLevel;
    }

    return true;
  }

  /**
   * SOPHISTICATED BACKSPACE - Handle empty node removal or content merging
   * Implements advanced children transfer with depth preservation
   */
  combineNodes(currentNodeId: string, previousNodeId: string): void {
    const currentNode = this.findNode(currentNodeId);
    const previousNode = this.findNode(previousNodeId);

    if (!currentNode || !previousNode) {
      return;
    }

    const currentContent = currentNode.content;
    const isEmpty = currentContent.trim() === '';

    if (isEmpty) {
      // EMPTY NODE REMOVAL LOGIC
      this.handleEmptyNodeRemoval(currentNode, previousNode);
    } else {
      // CONTENT MERGE LOGIC
      this.handleContentMerge(currentNode, previousNode);
    }
  }

  /**
   * Handle removal of empty nodes (backspace on empty content)
   */
  private handleEmptyNodeRemoval(currentNode: Node, previousNode: Node): void {
    const previousContent = previousNode.content;

    // Transfer children using sophisticated depth preservation
    if (currentNode.children.length > 0) {
      this.transferChildrenWithDepthPreservation(currentNode, previousNode);
    }

    // Remove current node from tree
    this.deleteNode(currentNode.id);

    // Focus previous node at end
    this.events.focusRequested(previousNode.id, previousContent.length);
  }

  /**
   * Handle merging of node content (backspace with actual content)
   * SMART FORMAT INHERITANCE: Receiver format takes precedence, strip source format
   */
  private handleContentMerge(currentNode: Node, previousNode: Node): void {
    const previousContent = previousNode.content;
    const currentContent = currentNode.content;
    const junctionPosition = previousContent.length;

    // SMART FORMAT INHERITANCE LOGIC
    const receiverHeaderLevel = this.contentProcessor.parseHeaderLevel(previousContent);
    const sourceHeaderLevel = this.contentProcessor.parseHeaderLevel(currentContent);

    let processedCurrentContent = currentContent;

    if (sourceHeaderLevel > 0) {
      // Strip header syntax from source node being merged
      processedCurrentContent = this.contentProcessor.stripHeaderSyntax(currentContent);
    }

    // Merge content with processed source content
    const mergedContent = previousContent + processedCurrentContent;
    previousNode.content = mergedContent;

    // Update the receiver node's header level (in case it changed)
    if (receiverHeaderLevel > 0) {
      previousNode.inheritHeaderLevel = receiverHeaderLevel;
    }

    // Transfer children using sophisticated depth preservation BEFORE removing node
    if (currentNode.children.length > 0) {
      this.transferChildrenWithDepthPreservation(currentNode, previousNode);
    }

    // Remove current node from tree
    this.deleteNode(currentNode.id);

    // Focus at junction point (after original content, before merged content)
    this.events.focusRequested(previousNode.id, junctionPosition);
  }

  /**
   * DATA MIGRATION - Initialize from legacy BaseNodeViewer array structure
   * Converts nested array structure to Map-based system preserving all properties
   */
  initializeFromLegacyData(legacyNodes: unknown[]): void {
    this._nodes.clear();
    this._rootNodeIds.length = 0;

    // Convert legacy nodes recursively
    const convertNode = (legacyNode: unknown, depth: number = 0, parentId?: string): string => {
      // Handle malformed data
      if (
        !legacyNode ||
        typeof legacyNode !== 'object' ||
        !(legacyNode as Record<string, unknown>).id
      ) {
        return '';
      }

      const legacyNodeObj = legacyNode as Record<string, unknown>;
      const nodeId = legacyNodeObj.id as string;

      // Create new Node from legacy structure
      const node: Node = {
        id: nodeId,
        content: (legacyNodeObj.content as string) || '',
        nodeType: (legacyNodeObj.type as string) || 'text',
        depth: depth,
        parentId: parentId,
        children: [],
        expanded: (legacyNodeObj.expanded as boolean) !== false, // Default to true
        autoFocus: (legacyNodeObj.autoFocus as boolean) || false,
        inheritHeaderLevel: (legacyNodeObj.inheritHeaderLevel as number) || 0,
        metadata: (legacyNodeObj.metadata as Record<string, unknown>) || {}
      };

      // Process children if they exist
      if (legacyNodeObj.children && Array.isArray(legacyNodeObj.children)) {
        node.children = (legacyNodeObj.children as unknown[]).map((child: unknown) =>
          convertNode(child, depth + 1, nodeId)
        );
      }

      // Store in nodes map
      this._nodes.set(nodeId, node);

      // CRITICAL SYNC: Initialize collapsed state based on node.expanded
      if (!node.expanded) {
        this._collapsedNodes.add(nodeId);
      }

      return nodeId;
    };

    // Convert all root level nodes
    for (const legacyNode of legacyNodes) {
      const nodeId = convertNode(legacyNode);
      if (nodeId) {
        // Only add valid node IDs
        this._rootNodeIds.push(nodeId);
      }
    }

    this.events.hierarchyChanged();
  }

  /**
   * Create new node with sophisticated Enter key logic
   * Supports cursor-at-beginning insertion and collapsed state handling
   */
  createNode(
    afterNodeId: string,
    content: string = '',
    nodeType: string = 'text',
    inheritHeaderLevel?: number,
    cursorAtBeginning: boolean = false
  ): string {
    const afterNode = this.findNode(afterNodeId);
    if (!afterNode) return '';

    // Determine the header level for the new node
    const finalHeaderLevel =
      inheritHeaderLevel !== undefined ? inheritHeaderLevel : afterNode.inheritHeaderLevel;

    // Add header syntax if creating a header node with empty content
    let finalContent = content;
    if (finalContent === '' && finalHeaderLevel > 0) {
      const headerPrefix = '#'.repeat(finalHeaderLevel) + ' ';
      finalContent = headerPrefix;
    }

    const newId = uuidv4();
    const newNode: Node = {
      id: newId,
      content: finalContent,
      nodeType: nodeType,
      depth: afterNode.depth,
      parentId: afterNode.parentId,
      children: [],
      expanded: true,
      autoFocus: true,
      inheritHeaderLevel: finalHeaderLevel,
      metadata: {}
    };

    this._nodes.set(newId, newNode);

    // Clear autoFocus from all other nodes
    this.clearAllAutoFocus();
    newNode.autoFocus = true;

    // SOPHISTICATED LOGIC: Handle children transfer based on collapsed state
    if (afterNode.children.length > 0) {
      const isCollapsed = this._collapsedNodes.has(afterNodeId);

      if (isCollapsed) {
        // CRITICAL FIX: When parent is collapsed, children ALWAYS stay with original node
        // regardless of cursor position or content splitting
        // No children transfer needed - they stay put
      } else if (!cursorAtBeginning) {
        // When expanded AND not at beginning, children go to the right (new) node
        newNode.children = [...afterNode.children];
        newNode.children.forEach((childId) => {
          const child = this.findNode(childId);
          if (child) child.parentId = newId;
        });
        afterNode.children = [];
      }
      // Note: When expanded AND at beginning, children stay with original node too
    }

    // Insert into appropriate location
    if (afterNode.parentId) {
      const parent = this.findNode(afterNode.parentId);
      if (parent) {
        const afterIndex = parent.children.indexOf(afterNodeId);
        if (cursorAtBeginning) {
          // Special case: cursor at beginning - insert BEFORE current node
          parent.children.splice(afterIndex, 0, newId);
        } else {
          // Normal case: insert AFTER current node
          parent.children.splice(afterIndex + 1, 0, newId);
        }
      }
    } else {
      // Insert at root level
      const afterIndex = this._rootNodeIds.indexOf(afterNodeId);
      if (cursorAtBeginning) {
        // Special case: cursor at beginning - insert BEFORE current node
        this._rootNodeIds.splice(afterIndex, 0, newId);
      } else {
        // Normal case: insert AFTER current node
        this._rootNodeIds.splice(afterIndex + 1, 0, newId);
      }
    }

    // Emit EventBus events for dynamic coordination
    const nodeCreatedEvent: Omit<import('./EventTypes').NodeCreatedEvent, 'timestamp'> = {
      type: 'node:created',
      namespace: 'lifecycle',
      source: this.serviceName,
      nodeId: newId,
      parentId: afterNode.parentId,
      nodeType,
      metadata: { inheritHeaderLevel: finalHeaderLevel, cursorAtBeginning }
    };
    eventBus.emit(nodeCreatedEvent);

    const hierarchyChangedEvent: Omit<import('./EventTypes').HierarchyChangedEvent, 'timestamp'> = {
      type: 'hierarchy:changed',
      namespace: 'lifecycle',
      source: this.serviceName,
      affectedNodes: [newId, afterNodeId],
      changeType: 'move'
    };
    eventBus.emit(hierarchyChangedEvent);

    // Legacy callback events (maintain compatibility)
    this.events.nodeCreated(newId);
    this.events.hierarchyChanged();

    // Emit cache invalidation for any references to the parent node
    this.emitCacheInvalidate('node', afterNode.parentId || afterNodeId, 'node created');

    // Request focus with cursor positioned after inherited syntax for new nodes
    if (finalContent.length > 0) {
      const cursorPosition = this.calculateOptimalCursorPosition(finalContent, finalHeaderLevel);
      if (cursorPosition > 0) {
        // Use setTimeout to ensure DOM has been updated before focusing
        setTimeout(() => {
          this.events.focusRequested(newId, cursorPosition);
          this.emitFocusRequested(newId, cursorPosition, 'node creation');
        }, 0);
      }
    }

    return newId;
  }

  /**
   * Update node content
   */
  updateNodeContent(nodeId: string, content: string): void {
    const node = this.findNode(nodeId);
    if (node) {
      const previousContent = node.content;
      node.content = content;

      // Emit EventBus events for dynamic coordination
      const nodeUpdatedEvent: Omit<import('./EventTypes').NodeUpdatedEvent, 'timestamp'> = {
        type: 'node:updated',
        namespace: 'lifecycle',
        source: this.serviceName,
        nodeId,
        updateType: 'content',
        previousValue: previousContent,
        newValue: content
      };
      eventBus.emit(nodeUpdatedEvent);

      // Emit decoration update needed for references
      this.emitDecorationUpdateNeeded(nodeId, 'content-changed');

      // Emit references update needed for any nodes that reference this one
      this.emitReferencesUpdateNeeded(nodeId, 'content');

      // Invalidate cache for this specific node
      this.emitCacheInvalidate('node', nodeId, 'content updated');
    }
  }

  /**
   * Delete node and update hierarchy
   */
  deleteNode(nodeId: string): void {
    const node = this.findNode(nodeId);
    if (!node) return;

    // Remove from parent's children or root
    if (node.parentId) {
      const parent = this.findNode(node.parentId);
      if (parent) {
        parent.children = parent.children.filter((id) => id !== nodeId);
      }
    } else {
      this._rootNodeIds = this._rootNodeIds.filter((id) => id !== nodeId);
    }

    // Recursively delete children that are still owned by this node
    const deleteRecursive = (id: string) => {
      const nodeToDelete = this._nodes.get(id);
      if (nodeToDelete) {
        // Only delete children that still belong to this node
        const ownedChildren = nodeToDelete.children.filter((childId) => {
          const child = this._nodes.get(childId);
          return child && child.parentId === id;
        });
        ownedChildren.forEach((childId) => deleteRecursive(childId));
        // Remove from map
        this._nodes.delete(id);
      }
    };

    // Collect children before deletion for event emission
    const childrenTransferred: string[] = [];
    if (node.children.length > 0) {
      childrenTransferred.push(...node.children);
    }

    deleteRecursive(nodeId);

    // Emit EventBus events for dynamic coordination
    const nodeDeletedEvent: Omit<import('./EventTypes').NodeDeletedEvent, 'timestamp'> = {
      type: 'node:deleted',
      namespace: 'lifecycle',
      source: this.serviceName,
      nodeId,
      parentId: node.parentId,
      childrenTransferred
    };
    eventBus.emit(nodeDeletedEvent);

    const hierarchyDeletedEvent: Omit<import('./EventTypes').HierarchyChangedEvent, 'timestamp'> = {
      type: 'hierarchy:changed',
      namespace: 'lifecycle',
      source: this.serviceName,
      affectedNodes: [nodeId, ...(node.parentId ? [node.parentId] : [])],
      changeType: 'move'
    };
    eventBus.emit(hierarchyDeletedEvent);

    // Legacy callback events (maintain compatibility)
    this.events.nodeDeleted(nodeId);
    this.events.hierarchyChanged();

    // Emit references update needed for deletion
    this.emitReferencesUpdateNeeded(nodeId, 'deletion');

    // Invalidate cache globally as references may be broken
    this.emitCacheInvalidate('global', undefined, 'node deleted');
  }

  /**
   * Find node by ID
   */
  findNode(nodeId: string): Node | null {
    return this._nodes.get(nodeId) || null;
  }

  /**
   * Indent node (move to previous sibling's children)
   */
  indentNode(nodeId: string): boolean {
    const node = this.findNode(nodeId);
    if (!node) return false;

    const siblings = node.parentId
      ? this.findNode(node.parentId)?.children || []
      : this._rootNodeIds;

    const nodeIndex = siblings.indexOf(nodeId);
    if (nodeIndex <= 0) return false; // Can't indent if no previous sibling

    const previousSiblingId = siblings[nodeIndex - 1];
    const previousSibling = this.findNode(previousSiblingId);
    if (!previousSibling) return false;

    // Remove from current position
    siblings.splice(nodeIndex, 1);

    // Add to previous sibling's children
    previousSibling.children.push(nodeId);
    node.parentId = previousSiblingId;
    node.depth = previousSibling.depth + 1;

    // Update depth of all descendants
    this.updateDescendantDepths(nodeId);

    // Emit EventBus events for dynamic coordination
    const indentEvent: Omit<import('./EventTypes').HierarchyChangedEvent, 'timestamp'> = {
      type: 'hierarchy:changed',
      namespace: 'lifecycle',
      source: this.serviceName,
      affectedNodes: [nodeId, previousSiblingId],
      changeType: 'indent'
    };
    eventBus.emit(indentEvent);

    // Legacy callback events (maintain compatibility)
    this.events.hierarchyChanged();

    // Emit references update needed for hierarchy change
    this.emitReferencesUpdateNeeded(nodeId, 'hierarchy');

    return true;
  }

  /**
   * Outdent node (move to parent's level)
   */
  outdentNode(nodeId: string): boolean {
    const node = this.findNode(nodeId);
    if (!node || !node.parentId) return false; // Can't outdent root nodes

    const parent = this.findNode(node.parentId);
    if (!parent) return false;

    // Remove from parent's children
    parent.children = parent.children.filter((id) => id !== nodeId);

    // Add to grandparent's children or root
    if (parent.parentId) {
      const grandparent = this.findNode(parent.parentId);
      if (grandparent) {
        const parentIndex = grandparent.children.indexOf(parent.id);
        grandparent.children.splice(parentIndex + 1, 0, nodeId);
        node.parentId = parent.parentId;
        node.depth = grandparent.depth + 1;
      }
    } else {
      // Move to root level
      const parentIndex = this._rootNodeIds.indexOf(parent.id);
      this._rootNodeIds.splice(parentIndex + 1, 0, nodeId);
      node.parentId = undefined;
      node.depth = 0;
    }

    // Update depth of all descendants
    this.updateDescendantDepths(nodeId);

    // Emit EventBus events for dynamic coordination
    const outdentEvent: Omit<import('./EventTypes').HierarchyChangedEvent, 'timestamp'> = {
      type: 'hierarchy:changed',
      namespace: 'lifecycle',
      source: this.serviceName,
      affectedNodes: [nodeId, parent.id],
      changeType: 'outdent'
    };
    eventBus.emit(outdentEvent);

    // Legacy callback events (maintain compatibility)
    this.events.hierarchyChanged();

    // Emit references update needed for hierarchy change
    this.emitReferencesUpdateNeeded(nodeId, 'hierarchy');

    return true;
  }

  /**
   * Toggle node expanded state
   * CRITICAL FIX: Synchronize with collapsed nodes set
   */
  toggleExpanded(nodeId: string): boolean {
    const node = this.findNode(nodeId);
    if (!node) return false;

    node.expanded = !node.expanded;

    // CRITICAL SYNC: Keep _collapsedNodes in sync with node.expanded
    if (node.expanded) {
      this._collapsedNodes.delete(nodeId); // Expanded = not in collapsed set
    } else {
      this._collapsedNodes.add(nodeId); // Collapsed = in collapsed set
    }

    // Emit EventBus events for dynamic coordination
    this.emitNodeStatusChanged(nodeId, node.expanded ? 'expanded' : 'collapsed', 'toggle expanded');

    const expandCollapseEvent: Omit<import('./EventTypes').HierarchyChangedEvent, 'timestamp'> = {
      type: 'hierarchy:changed',
      namespace: 'lifecycle',
      source: this.serviceName,
      affectedNodes: [nodeId],
      changeType: node.expanded ? 'expand' : 'collapse'
    };
    eventBus.emit(expandCollapseEvent);

    // Legacy callback events (maintain compatibility)
    this.events.hierarchyChanged();
    return true;
  }

  /**
   * Get visible nodes respecting hierarchy and expanded states
   */
  getVisibleNodes(): Node[] {
    return this.getVisibleNodesRecursive(this._rootNodeIds);
  }

  // ============================================================================
  // Private Helper Methods
  // ============================================================================

  /**
   * Get visible nodes recursively
   */
  private getVisibleNodesRecursive(nodeIds: string[]): Node[] {
    const result: Node[] = [];

    for (const nodeId of nodeIds) {
      const node = this._nodes.get(nodeId);
      if (node) {
        result.push(node);

        // Include children if node is expanded
        if (node.expanded && node.children.length > 0) {
          result.push(...this.getVisibleNodesRecursive(node.children));
        }
      }
    }

    return result;
  }

  /**
   * Clear autoFocus from all nodes
   */
  private clearAllAutoFocus(): void {
    for (const node of Array.from(this._nodes.values())) {
      node.autoFocus = false;
    }
  }

  /**
   * Update depth of node and all its descendants
   */
  private updateDescendantDepths(nodeId: string): void {
    const node = this.findNode(nodeId);
    if (!node) return;

    // Update children depths recursively
    for (const childId of node.children) {
      const child = this.findNode(childId);
      if (child) {
        child.depth = node.depth + 1;
        this.updateDescendantDepths(childId);
      }
    }
  }

  /**
   * Get node's siblings (nodes at same level)
   */
  private getNodeSiblings(nodeId: string): string[] {
    const node = this.findNode(nodeId);
    if (!node) return [];

    if (node.parentId) {
      const parent = this.findNode(node.parentId);
      return parent?.children || [];
    } else {
      return this._rootNodeIds;
    }
  }

  /**
   * Calculate optimal cursor position for new nodes with inherited syntax
   * Positions cursor after both header syntax and inline formatting syntax
   * Handles complex cases like "# *Wel|come*" → "# *|come*"
   */
  private calculateOptimalCursorPosition(content: string, headerLevel: number): number {
    let position = 0;

    // Step 1: Skip header syntax if present
    if (headerLevel > 0) {
      const headerPrefix = '#'.repeat(headerLevel) + ' ';
      if (content.startsWith(headerPrefix)) {
        position = headerPrefix.length;
      }
    }

    // Step 2: Skip all consecutive opening inline formatting markers
    // This handles cases where multiple formatting markers appear in sequence
    // e.g., "# **__*text*__**" should position after all opening markers

    // Check for formatting markers in order of precedence (longest first to avoid conflicts)
    const formattingMarkers = ['**', '__', '*']; // Bold, underline, italic

    let foundMarker = true;
    while (foundMarker) {
      foundMarker = false;
      const currentRemaining = content.substring(position);

      for (const marker of formattingMarkers) {
        if (currentRemaining.startsWith(marker)) {
          // Check if this marker has a closing counterpart later in the content
          const closingIndex = currentRemaining.indexOf(marker, marker.length);
          if (closingIndex !== -1) {
            // Valid formatting pair found, skip the opening marker
            position += marker.length;
            foundMarker = true;
            break; // Found a marker, restart the search from new position
          }
        }
      }
    }

    return position;
  }

  // ============================================================================
  // Sophisticated Children Transfer Logic (from nodespace-core-ui)
  // ============================================================================

  /**
   * Transfer children from source node with sophisticated depth preservation
   * Handles collapsed state and auto-expansion logic
   */
  private transferChildrenWithDepthPreservation(sourceNode: Node, targetNode: Node): void {
    if (sourceNode.children.length === 0) return;

    // Track which nodes get new children for auto-expansion
    const nodesGettingNewChildren = new Set<Node>();

    const sourceDepth = this.getNodeDepth(sourceNode);

    // Determine the appropriate parent for the transferred children
    let newParent: Node;
    let insertAtBeginning = false;

    if (sourceDepth === 0) {
      // Source is a root node - children should find appropriate level in target hierarchy
      // Use root ancestor approach for root sources
      const targetRootAncestor = this.findRootAncestor(targetNode);
      if (!targetRootAncestor) return;
      newParent = targetRootAncestor;
      insertAtBeginning = this._collapsedNodes.has(targetRootAncestor.id);
    } else {
      // Source is not a root - children should go directly to target
      // Use direct target approach for non-root sources
      newParent = targetNode;
      insertAtBeginning = this._collapsedNodes.has(targetNode.id);
    }

    // Move all direct children of the source to the determined parent
    if (insertAtBeginning) {
      // Target was collapsed - insert new children at the BEGINNING
      const existingChildren = [...newParent.children];
      newParent.children = [...sourceNode.children, ...existingChildren];
    } else {
      // Target was expanded - insert new children at the END
      newParent.children.push(...sourceNode.children);
    }

    // Update parent references for all transferred children
    sourceNode.children.forEach((childId) => {
      const child = this.findNode(childId);
      if (child) child.parentId = newParent.id;
    });

    // Mark parent as getting new children
    nodesGettingNewChildren.add(newParent);

    // Clear the source node's children since they've been moved
    sourceNode.children = [];

    // Auto-expand nodes that received new children
    nodesGettingNewChildren.forEach((node) => {
      if (this._collapsedNodes.has(node.id)) {
        this._collapsedNodes.delete(node.id);
      }
    });
  }

  /**
   * Find the root ancestor (depth 0) of a given node
   */
  private findRootAncestor(node: Node): Node | null {
    let current: Node | null = node;

    // Walk up to find the root
    while (current && current.parentId) {
      current = this.findNode(current.parentId);
    }

    return current;
  }

  /**
   * Get the depth of a node in the hierarchy
   */
  private getNodeDepth(node: Node): number {
    let depth = 0;
    let current = this.findNode(node.parentId || '');
    while (current) {
      depth++;
      current = this.findNode(current.parentId || '');
    }
    return depth;
  }

  /**
   * Find the appropriate parent node at the specified depth
   */
  private findParentAtDepth(startNode: Node, targetDepth: number): Node | null {
    let current: Node | null = startNode;

    // Walk up the tree to find a node at the target depth
    while (current && this.getNodeDepth(current) > targetDepth) {
      current = this.findNode(current.parentId || '');
    }

    // If we found a node at exactly the target depth, return it
    if (current && this.getNodeDepth(current) === targetDepth) {
      return current;
    }

    return null;
  }

  /**
   * Toggle collapsed state of a node
   * CRITICAL FIX: Synchronize with node.expanded property
   */
  toggleCollapsed(nodeId: string): boolean {
    const node = this.findNode(nodeId);
    if (!node) return false;

    if (this._collapsedNodes.has(nodeId)) {
      this._collapsedNodes.delete(nodeId);
      node.expanded = true; // Sync with node property
      return false; // Now expanded
    } else {
      this._collapsedNodes.add(nodeId);
      node.expanded = false; // Sync with node property
      return true; // Now collapsed
    }
  }

  /**
   * Check if a node is collapsed
   */
  isNodeCollapsed(nodeId: string): boolean {
    return this._collapsedNodes.has(nodeId);
  }

  // ============================================================================
  // EventBus Integration Methods
  // ============================================================================

  /**
   * Set the active node and emit status change event
   */
  public setActiveNode(nodeId: string): void {
    const previousNodeId = this.activeNodeId;

    if (previousNodeId) {
      this.emitNodeStatusChanged(previousNodeId, 'active', 'deactivated');
    }

    this.activeNodeId = nodeId;
    this.emitNodeStatusChanged(nodeId, 'focused', 'activated');
  }

  /**
   * Emit node status changed event
   */
  private emitNodeStatusChanged(nodeId: string, status: NodeStatus, reason: string): void {
    const statusEvent: Omit<import('./EventTypes').NodeStatusChangedEvent, 'timestamp'> = {
      type: 'node:status-changed',
      namespace: 'coordination',
      source: this.serviceName,
      nodeId,
      status,
      metadata: { reason }
    };
    eventBus.emit(statusEvent);
  }

  /**
   * Emit decoration update needed event
   */
  private emitDecorationUpdateNeeded(
    nodeId: string,
    reason: 'content-changed' | 'status-changed' | 'reference-updated' | 'cache-invalidated'
  ): void {
    const decorationUpdateEvent: Omit<
      import('./EventTypes').DecorationUpdateNeededEvent,
      'timestamp'
    > = {
      type: 'decoration:update-needed',
      namespace: 'interaction',
      source: this.serviceName,
      nodeId,
      decorationType: 'nodespace-reference',
      reason
    };
    eventBus.emit(decorationUpdateEvent);
  }

  /**
   * Emit references update needed event
   */
  private emitReferencesUpdateNeeded(
    nodeId: string,
    updateType: 'content' | 'status' | 'hierarchy' | 'deletion'
  ): void {
    const referencesUpdateEvent: Omit<
      import('./EventTypes').ReferencesUpdateNeededEvent,
      'timestamp'
    > = {
      type: 'references:update-needed',
      namespace: 'coordination',
      source: this.serviceName,
      nodeId,
      updateType
    };
    eventBus.emit(referencesUpdateEvent);
  }

  /**
   * Emit cache invalidation event
   */
  private emitCacheInvalidate(
    scope: 'single' | 'node' | 'global',
    nodeId?: string,
    reason?: string
  ): void {
    const cacheEvent: Omit<import('./EventTypes').CacheInvalidateEvent, 'timestamp'> = {
      type: 'cache:invalidate',
      namespace: 'coordination',
      source: this.serviceName,
      cacheKey: nodeId ? `node:${nodeId}` : 'global',
      scope,
      nodeId,
      reason: reason || 'node operation'
    };
    eventBus.emit(cacheEvent);
  }

  /**
   * Emit focus requested event through EventBus
   */
  private emitFocusRequested(
    nodeId: string,
    position?: number,
    reason: string = 'programmatic'
  ): void {
    const focusEvent: Omit<import('./EventTypes').FocusRequestedEvent, 'timestamp'> = {
      type: 'focus:requested',
      namespace: 'navigation',
      source: this.serviceName,
      nodeId,
      position,
      reason
    };
    eventBus.emit(focusEvent);
  }

  /**
   * Add a node to NodeManager from external sources (like NodeReferenceService)
   * This allows other services to register nodes with NodeManager
   */
  public addExternalNode(node: {
    id: string;
    content: string;
    nodeType: string;
    parentId?: string;
    metadata?: Record<string, unknown>;
  }): void {
    const managerNode: Node = {
      id: node.id,
      content: node.content,
      nodeType: node.nodeType,
      depth: 0, // Default depth, can be calculated if needed
      parentId: node.parentId || 'root',
      children: [],
      expanded: true,
      autoFocus: false,
      inheritHeaderLevel: 0,
      metadata: node.metadata || {}
    };

    this._nodes.set(node.id, managerNode);

    // If it's a root node, add to root nodes list
    if (!node.parentId || node.parentId === 'root') {
      this._rootNodeIds.push(node.id);
    }

    // Emit events
    this.emitNodeStatusChanged(node.id, 'active', 'added by external service');
    this.events.nodeCreated(node.id);
  }
}
