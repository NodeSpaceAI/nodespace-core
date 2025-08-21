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
import { ContentProcessor } from './ContentProcessor';

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
}

export interface NodeManagerEvents {
  focusRequested: (nodeId: string, position?: number) => void;
  hierarchyChanged: () => void;
  nodeCreated: (nodeId: string) => void;
  nodeDeleted: (nodeId: string) => void;
}

export class NodeManager {
  private _nodes: Map<string, Node>;
  private _rootNodeIds: string[];
  private events: NodeManagerEvents;
  protected contentProcessor: ContentProcessor;

  constructor(events: NodeManagerEvents) {
    this._nodes = new Map<string, Node>();
    this._rootNodeIds = [];
    this.events = events;
    this.contentProcessor = ContentProcessor.getInstance();
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
  renderNodeAsHTML(nodeId: string): string {
    const node = this.findNode(nodeId);
    if (!node) return '';
    const ast = this.contentProcessor.parseMarkdown(node.content);
    return this.contentProcessor.renderAST(ast);
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
   * CRITICAL METHOD - Fixes backspace combination bug
   * Combines currentNode with previousNode, preserving content and hierarchy
   */
  combineNodes(currentNodeId: string, previousNodeId: string): void {
    const currentNode = this.findNode(currentNodeId);
    const previousNode = this.findNode(previousNodeId);

    if (!currentNode || !previousNode) {
      return;
    }

    const junctionPosition = previousNode.content.length;

    // Combine content
    previousNode.content = previousNode.content + currentNode.content;

    // Transfer children if any
    if (currentNode.children.length > 0) {
      previousNode.children.push(...currentNode.children);
      // Update parent references
      currentNode.children.forEach((childId) => {
        const child = this.findNode(childId);
        if (child) child.parentId = previousNode.id;
      });
    }

    // Remove current node from tree
    this.deleteNode(currentNodeId);

    // Notify UI for cursor positioning
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
      if (!legacyNode || typeof legacyNode !== 'object' || !legacyNode.id) {
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
   * Create new node after specified node
   */
  createNode(
    afterNodeId: string,
    content: string = '',
    nodeType: string = 'text',
    inheritHeaderLevel?: number
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

    // Insert into appropriate location
    if (afterNode.parentId) {
      const parent = this.findNode(afterNode.parentId);
      if (parent) {
        const afterIndex = parent.children.indexOf(afterNodeId);
        parent.children.splice(afterIndex + 1, 0, newId);
      }
    } else {
      // Insert at root level
      const afterIndex = this._rootNodeIds.indexOf(afterNodeId);
      this._rootNodeIds.splice(afterIndex + 1, 0, newId);
    }

    this.events.nodeCreated(newId);
    this.events.hierarchyChanged();

    // Request focus with cursor positioned after inherited syntax for new nodes
    if (finalContent.length > 0) {
      const cursorPosition = this.calculateOptimalCursorPosition(finalContent, finalHeaderLevel);
      if (cursorPosition > 0) {
        // Use setTimeout to ensure DOM has been updated before focusing
        setTimeout(() => {
          this.events.focusRequested(newId, cursorPosition);
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
      node.content = content;
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

    deleteRecursive(nodeId);

    this.events.nodeDeleted(nodeId);
    this.events.hierarchyChanged();
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

    this.events.hierarchyChanged();
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

    this.events.hierarchyChanged();
    return true;
  }

  /**
   * Toggle node expanded state
   */
  toggleExpanded(nodeId: string): boolean {
    const node = this.findNode(nodeId);
    if (!node) return false;

    node.expanded = !node.expanded;
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
    for (const node of this._nodes.values()) {
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
    const remainingContent = content.substring(position);
    
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
}
