/**
 * Mock Text Service
 *
 * Provides simulated CRUD operations for TextNode components.
 * Includes auto-save functionality and realistic network delays.
 *
 * This mock service enables independent TextNode development
 * and will be replaced with real data store integration later.
 */

export interface TextNodeData {
  id: string;
  content: string;
  title: string;
  createdAt: Date;
  updatedAt: Date;
  parentId: string | null;
  depth: number;
  expanded: boolean;
  metadata: {
    wordCount: number;
    lastEditedBy: string;
    version: number;
    hasChildren: boolean;
    childrenIds: string[];
  };
}

export interface TextSaveResult {
  success: boolean;
  id: string;
  timestamp: Date;
  error?: string;
}

export interface HierarchicalTextNode {
  id: string;
  title: string;
  content: string;
  nodeType: 'text' | 'task' | 'ai-chat' | 'entity' | 'query';
  depth: number;
  parentId: string | null;
  children: HierarchicalTextNode[];
  expanded: boolean;
  hasChildren: boolean;
  createdAt: Date;
  updatedAt: Date;
  metadata: {
    wordCount: number;
    lastEditedBy: string;
    version: number;
  };
}

export class MockTextService {
  private static instance: MockTextService;
  private textNodes = new Map<string, TextNodeData>();
  private autoSaveTimeouts = new Map<string, NodeJS.Timeout>();

  static getInstance(): MockTextService {
    if (!MockTextService.instance) {
      MockTextService.instance = new MockTextService();
    }
    return MockTextService.instance;
  }

  constructor() {
    // Initialize with some sample data
    this.initializeSampleData();
  }

  private initializeSampleData(): void {
    const rootNode: TextNodeData = {
      id: 'text-root-1',
      content:
        'This is the **root node** of our hierarchical structure.\n\nIt contains several child nodes demonstrating tree patterns.',
      title: 'Project Root',
      parentId: null,
      depth: 0,
      expanded: true,
      createdAt: new Date(Date.now() - 86400000),
      updatedAt: new Date(Date.now() - 3600000),
      metadata: {
        wordCount: 18,
        lastEditedBy: 'user',
        version: 1,
        hasChildren: true,
        childrenIds: ['text-child-1', 'text-child-2']
      }
    };

    const childNode1: TextNodeData = {
      id: 'text-child-1',
      content:
        'This is the **first child node**.\n\nIt has its own children to demonstrate multi-level hierarchy.',
      title: 'Chapter 1: Introduction',
      parentId: 'text-root-1',
      depth: 1,
      expanded: true,
      createdAt: new Date(Date.now() - 82800000),
      updatedAt: new Date(Date.now() - 7200000),
      metadata: {
        wordCount: 16,
        lastEditedBy: 'user',
        version: 2,
        hasChildren: true,
        childrenIds: ['text-grandchild-1', 'text-grandchild-2']
      }
    };

    const childNode2: TextNodeData = {
      id: 'text-child-2',
      content: "This is the **second child node**.\n\nIt's a leaf node with no children.",
      title: 'Chapter 2: Conclusion',
      parentId: 'text-root-1',
      depth: 1,
      expanded: false,
      createdAt: new Date(Date.now() - 79200000),
      updatedAt: new Date(Date.now() - 3600000),
      metadata: {
        wordCount: 14,
        lastEditedBy: 'user',
        version: 1,
        hasChildren: false,
        childrenIds: []
      }
    };

    const grandchildNode1: TextNodeData = {
      id: 'text-grandchild-1',
      content:
        'This is a **grandchild node** at depth 2.\n\n- Demonstrates deeper hierarchy\n- Shows indentation patterns',
      title: 'Section 1.1: Core Concepts',
      parentId: 'text-child-1',
      depth: 2,
      expanded: false,
      createdAt: new Date(Date.now() - 75600000),
      updatedAt: new Date(Date.now() - 1800000),
      metadata: {
        wordCount: 15,
        lastEditedBy: 'user',
        version: 1,
        hasChildren: false,
        childrenIds: []
      }
    };

    const grandchildNode2: TextNodeData = {
      id: 'text-grandchild-2',
      content:
        'Another **grandchild node** showing sibling relationships.\n\nThis completes our hierarchy example.',
      title: 'Section 1.2: Advanced Features',
      parentId: 'text-child-1',
      depth: 2,
      expanded: false,
      createdAt: new Date(Date.now() - 72000000),
      updatedAt: new Date(Date.now() - 900000),
      metadata: {
        wordCount: 13,
        lastEditedBy: 'user',
        version: 1,
        hasChildren: false,
        childrenIds: []
      }
    };

    // Store all nodes
    this.textNodes.set(rootNode.id, rootNode);
    this.textNodes.set(childNode1.id, childNode1);
    this.textNodes.set(childNode2.id, childNode2);
    this.textNodes.set(grandchildNode1.id, grandchildNode1);
    this.textNodes.set(grandchildNode2.id, grandchildNode2);
  }

  /**
   * Save text node content with auto-save simulation
   */
  async saveTextNode(id: string, content: string, title?: string): Promise<TextSaveResult> {
    // Simulate network delay
    await new Promise((resolve) => setTimeout(resolve, 200 + Math.random() * 300));

    try {
      const now = new Date();
      const existingNode = this.textNodes.get(id);

      const nodeData: TextNodeData = {
        id,
        content,
        title: title || existingNode?.title || 'Untitled',
        parentId: existingNode?.parentId || null,
        depth: existingNode?.depth || 0,
        expanded: existingNode?.expanded ?? true,
        createdAt: existingNode?.createdAt || now,
        updatedAt: now,
        metadata: {
          wordCount: this.calculateWordCount(content),
          lastEditedBy: 'user',
          version: (existingNode?.metadata.version || 0) + 1,
          hasChildren: existingNode?.metadata.hasChildren || false,
          childrenIds: existingNode?.metadata.childrenIds || []
        }
      };

      this.textNodes.set(id, nodeData);

      return {
        success: true,
        id,
        timestamp: now
      };
    } catch (error) {
      return {
        success: false,
        id,
        timestamp: new Date(),
        error: error instanceof Error ? error.message : 'Unknown error'
      };
    }
  }

  /**
   * Load text node by ID
   */
  async loadTextNode(id: string): Promise<TextNodeData | null> {
    // Simulate network delay
    await new Promise((resolve) => setTimeout(resolve, 100 + Math.random() * 200));

    return this.textNodes.get(id) || null;
  }

  /**
   * Create new text node
   */
  async createTextNode(
    content: string = '',
    title: string = 'New Text Node'
  ): Promise<TextNodeData> {
    await new Promise((resolve) => setTimeout(resolve, 150));

    const id = this.generateId();
    const now = new Date();

    const nodeData: TextNodeData = {
      id,
      content,
      title,
      parentId: null,
      depth: 0,
      expanded: true,
      createdAt: now,
      updatedAt: now,
      metadata: {
        wordCount: this.calculateWordCount(content),
        lastEditedBy: 'user',
        version: 1,
        hasChildren: false,
        childrenIds: []
      }
    };

    this.textNodes.set(id, nodeData);
    return nodeData;
  }

  /**
   * Delete text node
   */
  async deleteTextNode(id: string): Promise<boolean> {
    await new Promise((resolve) => setTimeout(resolve, 100));

    // Clear any pending auto-save
    const timeout = this.autoSaveTimeouts.get(id);
    if (timeout) {
      clearTimeout(timeout);
      this.autoSaveTimeouts.delete(id);
    }

    return this.textNodes.delete(id);
  }

  /**
   * Auto-save with debouncing
   */
  scheduleAutoSave(
    id: string,
    content: string,
    title?: string,
    delay: number = 2000
  ): Promise<TextSaveResult> {
    return new Promise((resolve) => {
      // Clear existing timeout for this node
      const existingTimeout = this.autoSaveTimeouts.get(id);
      if (existingTimeout) {
        clearTimeout(existingTimeout);
      }

      // Set new timeout
      const timeout = setTimeout(async () => {
        const result = await this.saveTextNode(id, content, title);
        this.autoSaveTimeouts.delete(id);
        resolve(result);
      }, delay);

      this.autoSaveTimeouts.set(id, timeout);
    });
  }

  /**
   * Cancel pending auto-save for a node
   */
  cancelAutoSave(id: string): void {
    const timeout = this.autoSaveTimeouts.get(id);
    if (timeout) {
      clearTimeout(timeout);
      this.autoSaveTimeouts.delete(id);
    }
  }

  /**
   * Get all text nodes (for listing/search)
   */
  async getAllTextNodes(): Promise<TextNodeData[]> {
    await new Promise((resolve) => setTimeout(resolve, 150));

    return Array.from(this.textNodes.values()).sort(
      (a, b) => b.updatedAt.getTime() - a.updatedAt.getTime()
    );
  }

  /**
   * Search text nodes by content or title
   */
  async searchTextNodes(query: string): Promise<TextNodeData[]> {
    await new Promise((resolve) => setTimeout(resolve, 200));

    if (!query.trim()) {
      return this.getAllTextNodes();
    }

    const searchLower = query.toLowerCase();

    return Array.from(this.textNodes.values())
      .filter(
        (node) =>
          node.title.toLowerCase().includes(searchLower) ||
          node.content.toLowerCase().includes(searchLower)
      )
      .sort((a, b) => b.updatedAt.getTime() - a.updatedAt.getTime());
  }

  /**
   * Generate unique node ID
   */
  private generateId(): string {
    return `text-${Date.now()}-${Math.random().toString(36).substring(2, 9)}`;
  }

  /**
   * Calculate word count for content
   */
  private calculateWordCount(content: string): number {
    return content
      .trim()
      .split(/\s+/)
      .filter((word) => word.length > 0).length;
  }

  /**
   * Get hierarchical tree structure for UI rendering
   */
  async getHierarchicalNodes(): Promise<HierarchicalTextNode[]> {
    await new Promise((resolve) => setTimeout(resolve, 100));

    const allNodes = Array.from(this.textNodes.values());
    const nodeMap = new Map<string, HierarchicalTextNode>();

    // Convert to hierarchical format
    for (const node of allNodes) {
      nodeMap.set(node.id, {
        id: node.id,
        title: node.title,
        content: node.content,
        nodeType: 'text',
        depth: node.depth,
        parentId: node.parentId,
        children: [],
        expanded: node.expanded,
        hasChildren: node.metadata.hasChildren,
        createdAt: node.createdAt,
        updatedAt: node.updatedAt,
        metadata: {
          wordCount: node.metadata.wordCount,
          lastEditedBy: node.metadata.lastEditedBy,
          version: node.metadata.version
        }
      });
    }

    // Build tree structure
    const rootNodes: HierarchicalTextNode[] = [];
    for (const hierarchicalNode of nodeMap.values()) {
      if (hierarchicalNode.parentId === null) {
        rootNodes.push(hierarchicalNode);
      } else {
        const parent = nodeMap.get(hierarchicalNode.parentId);
        if (parent) {
          parent.children.push(hierarchicalNode);
        }
      }
    }

    // Sort children by creation date
    function sortChildren(nodes: HierarchicalTextNode[]) {
      nodes.sort((a, b) => a.createdAt.getTime() - b.createdAt.getTime());
      nodes.forEach((node) => sortChildren(node.children));
    }
    sortChildren(rootNodes);

    return rootNodes;
  }

  /**
   * Create a child node under a parent
   */
  async createChildNode(
    parentId: string,
    content: string = '',
    title: string = 'New Child Node'
  ): Promise<TextNodeData | null> {
    await new Promise((resolve) => setTimeout(resolve, 150));

    const parent = this.textNodes.get(parentId);
    if (!parent) return null;

    const id = this.generateId();
    const now = new Date();

    const childNode: TextNodeData = {
      id,
      content,
      title,
      parentId,
      depth: parent.depth + 1,
      expanded: false,
      createdAt: now,
      updatedAt: now,
      metadata: {
        wordCount: this.calculateWordCount(content),
        lastEditedBy: 'user',
        version: 1,
        hasChildren: false,
        childrenIds: []
      }
    };

    // Update parent to show it has children
    parent.metadata.hasChildren = true;
    parent.metadata.childrenIds.push(id);
    parent.updatedAt = now;

    this.textNodes.set(id, childNode);
    this.textNodes.set(parentId, parent);

    return childNode;
  }

  /**
   * Move a node to a different parent (or make it root-level)
   */
  async moveNode(nodeId: string, newParentId: string | null): Promise<boolean> {
    await new Promise((resolve) => setTimeout(resolve, 200));

    const node = this.textNodes.get(nodeId);
    if (!node) return false;

    const oldParentId = node.parentId;

    // Remove from old parent's children list
    if (oldParentId) {
      const oldParent = this.textNodes.get(oldParentId);
      if (oldParent) {
        oldParent.metadata.childrenIds = oldParent.metadata.childrenIds.filter(
          (id) => id !== nodeId
        );
        oldParent.metadata.hasChildren = oldParent.metadata.childrenIds.length > 0;
        oldParent.updatedAt = new Date();
        this.textNodes.set(oldParentId, oldParent);
      }
    }

    // Add to new parent or make root-level
    if (newParentId) {
      const newParent = this.textNodes.get(newParentId);
      if (!newParent) return false;

      node.parentId = newParentId;
      node.depth = newParent.depth + 1;

      newParent.metadata.hasChildren = true;
      newParent.metadata.childrenIds.push(nodeId);
      newParent.updatedAt = new Date();
      this.textNodes.set(newParentId, newParent);
    } else {
      node.parentId = null;
      node.depth = 0;
    }

    // Update depths of all descendant nodes
    this.updateDescendantDepths(nodeId, node.depth);

    node.updatedAt = new Date();
    this.textNodes.set(nodeId, node);

    return true;
  }

  /**
   * Toggle expansion state of a node
   */
  async toggleNodeExpansion(nodeId: string): Promise<boolean> {
    await new Promise((resolve) => setTimeout(resolve, 50));

    const node = this.textNodes.get(nodeId);
    if (!node || !node.metadata.hasChildren) return false;

    node.expanded = !node.expanded;
    node.updatedAt = new Date();
    this.textNodes.set(nodeId, node);

    return node.expanded;
  }

  /**
   * Get children of a specific node
   */
  async getChildren(parentId: string): Promise<TextNodeData[]> {
    await new Promise((resolve) => setTimeout(resolve, 75));

    const parent = this.textNodes.get(parentId);
    if (!parent) return [];

    return parent.metadata.childrenIds
      .map((id) => this.textNodes.get(id))
      .filter((node) => node !== undefined)
      .sort((a, b) => a!.createdAt.getTime() - b!.createdAt.getTime()) as TextNodeData[];
  }

  /**
   * Update depths of all descendant nodes recursively
   */
  private updateDescendantDepths(nodeId: string, newDepth: number): void {
    const node = this.textNodes.get(nodeId);
    if (!node) return;

    node.depth = newDepth;

    for (const childId of node.metadata.childrenIds) {
      this.updateDescendantDepths(childId, newDepth + 1);
    }
  }

  /**
   * Get service statistics (for debugging/monitoring)
   */
  getStats() {
    const nodes = Array.from(this.textNodes.values());
    const rootNodes = nodes.filter((node) => node.parentId === null);
    const maxDepth = Math.max(...nodes.map((node) => node.depth), 0);

    return {
      totalNodes: this.textNodes.size,
      rootNodes: rootNodes.length,
      maxDepth,
      nodesWithChildren: nodes.filter((node) => node.metadata.hasChildren).length,
      pendingAutoSaves: this.autoSaveTimeouts.size,
      lastActivity: new Date()
    };
  }
}

// Export singleton instance
export const mockTextService = MockTextService.getInstance();

// Types are already exported above - no need to re-export
