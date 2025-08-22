/**
 * MockDatabaseService - Database Schema Implementation for Parallel Development
 *
 * Mock database implementation for parallel development that implements the exact
 * NodeSpaceNode schema. Provides in-memory storage with Map-based collections
 * and supports mentions array and sibling ordering queries.
 *
 * This service mirrors the future LanceDB interface to enable independent development
 * while the database integration is being implemented.
 *
 * Key Features:
 * - Exact NodeSpaceNode schema implementation
 * - In-memory storage with Map-based collections
 * - Support for mentions array (bidirectional backlink system)
 * - Single-pointer sibling ordering via before_sibling_id
 * - Methods mirroring future LanceDB interface
 * - Type-specific metadata handling via JSON field
 */

// ============================================================================
// Database Schema Interface (Exact Implementation Required)
// ============================================================================

export interface NodeSpaceNode {
  id: string;
  type: string;
  content: string;
  parent_id: string | null;
  root_id: string;
  before_sibling_id: string | null; // Single-pointer sibling ordering
  created_at: string;
  mentions: string[]; // Array of node IDs this node references (BACKLINK SYSTEM)
  metadata: Record<string, unknown>; // JSON for type-specific properties
  embedding_vector: Float32Array | null; // For Phase 3
}

export interface NodeQuery {
  id?: string;
  type?: string;
  parent_id?: string | null;
  root_id?: string;
  content_contains?: string;
  mentioned_by?: string; // Find nodes that mention this ID
  mentions?: string; // Find nodes mentioned by this ID
  limit?: number;
  offset?: number;
}

export interface NodeUpdate {
  content?: string;
  type?: string;
  parent_id?: string | null;
  root_id?: string;
  before_sibling_id?: string | null;
  mentions?: string[];
  metadata?: Record<string, unknown>;
}

// ============================================================================
// MockDatabaseService Implementation
// ============================================================================

export class MockDatabaseService {
  private nodes: Map<string, NodeSpaceNode> = new Map();
  private mentionsIndex: Map<string, Set<string>> = new Map(); // target -> Set<source>
  private reverseIndex: Map<string, Set<string>> = new Map(); // source -> Set<target>
  private typeIndex: Map<string, Set<string>> = new Map(); // type -> Set<nodeId>
  private parentIndex: Map<string | null, Set<string>> = new Map(); // parent -> Set<children>
  private rootIndex: Map<string, Set<string>> = new Map(); // root -> Set<nodeIds>

  // ========================================================================
  // Core CRUD Operations
  // ========================================================================

  /**
   * Insert a new node
   */
  async insertNode(node: NodeSpaceNode): Promise<void> {
    if (this.nodes.has(node.id)) {
      throw new Error(`Node with ID ${node.id} already exists`);
    }

    // Validate schema
    this.validateNode(node);

    // Store the node
    this.nodes.set(node.id, { ...node });

    // Update indexes
    this.updateIndexesOnInsert(node);
  }

  /**
   * Update an existing node
   */
  async updateNode(nodeId: string, updates: NodeUpdate): Promise<NodeSpaceNode | null> {
    const existingNode = this.nodes.get(nodeId);
    if (!existingNode) {
      return null;
    }

    // Create updated node
    const updatedNode: NodeSpaceNode = {
      ...existingNode,
      ...updates
    };

    // Validate updated schema
    this.validateNode(updatedNode);

    // Remove old indexes
    this.updateIndexesOnDelete(existingNode);

    // Update the node
    this.nodes.set(nodeId, updatedNode);

    // Add new indexes
    this.updateIndexesOnInsert(updatedNode);

    return { ...updatedNode };
  }

  /**
   * Upsert node (insert or update)
   */
  async upsertNode(node: NodeSpaceNode): Promise<NodeSpaceNode> {
    // Validate node schema
    this.validateNode(node);

    const existingNode = this.nodes.get(node.id);
    if (existingNode) {
      // Remove old indexes
      this.updateIndexesOnDelete(existingNode);
    }

    // Insert/update the node
    this.nodes.set(node.id, { ...node });

    // Add new indexes
    this.updateIndexesOnInsert(node);

    return { ...node };
  }

  /**
   * Get node by ID
   */
  async getNode(nodeId: string): Promise<NodeSpaceNode | null> {
    const node = this.nodes.get(nodeId);
    return node ? { ...node } : null;
  }

  /**
   * Delete a node
   */
  async deleteNode(nodeId: string): Promise<boolean> {
    const node = this.nodes.get(nodeId);
    if (!node) {
      return false;
    }

    // Remove from indexes
    this.updateIndexesOnDelete(node);

    // Remove the node
    this.nodes.delete(nodeId);

    return true;
  }

  // ========================================================================
  // Query Operations
  // ========================================================================

  /**
   * Query nodes with filtering
   */
  async queryNodes(query: NodeQuery = {}): Promise<NodeSpaceNode[]> {
    let results: NodeSpaceNode[] = [];

    if (query.id) {
      // Direct ID lookup
      const node = await this.getNode(query.id);
      results = node ? [node] : [];
    } else {
      // Start with all nodes and filter
      results = Array.from(this.nodes.values());

      // Apply filters
      if (query.type) {
        results = results.filter((node) => node.type === query.type);
      }

      if (query.parent_id !== undefined) {
        results = results.filter((node) => node.parent_id === query.parent_id);
      }

      if (query.root_id) {
        results = results.filter((node) => node.root_id === query.root_id);
      }

      if (query.content_contains) {
        const searchTerm = query.content_contains.toLowerCase();
        results = results.filter((node) => node.content.toLowerCase().includes(searchTerm));
      }

      if (query.mentioned_by) {
        // Find nodes that mention the specified node
        results = results.filter((node) => node.mentions.includes(query.mentioned_by!));
      }

      if (query.mentions) {
        // Find nodes mentioned by the specified node
        const sourceNode = this.nodes.get(query.mentions);
        if (sourceNode) {
          const mentionedIds = new Set(sourceNode.mentions);
          results = results.filter((node) => mentionedIds.has(node.id));
        } else {
          results = [];
        }
      }
    }

    // Apply pagination
    if (query.offset) {
      results = results.slice(query.offset);
    }

    if (query.limit) {
      results = results.slice(0, query.limit);
    }

    // Return deep copies to prevent external mutation
    return results.map((node) => ({ ...node }));
  }

  // ========================================================================
  // Hierarchy Operations
  // ========================================================================

  /**
   * Get direct children of a node
   */
  async getChildren(nodeId: string): Promise<NodeSpaceNode[]> {
    const childrenIds = this.parentIndex.get(nodeId) || new Set();
    const children: NodeSpaceNode[] = [];

    for (const childId of childrenIds) {
      const child = this.nodes.get(childId);
      if (child) {
        children.push({ ...child });
      }
    }

    // Sort by sibling order (before_sibling_id creates linked list)
    return this.sortSiblings(children);
  }

  /**
   * Get all nodes with no parent (root nodes)
   */
  async getRootNodes(): Promise<NodeSpaceNode[]> {
    const rootIds = this.parentIndex.get(null) || new Set();
    const roots: NodeSpaceNode[] = [];

    for (const rootId of rootIds) {
      const root = this.nodes.get(rootId);
      if (root) {
        roots.push({ ...root });
      }
    }

    return this.sortSiblings(roots);
  }

  /**
   * Get all descendants of a node (recursive)
   */
  async getDescendants(nodeId: string): Promise<NodeSpaceNode[]> {
    const descendants: NodeSpaceNode[] = [];
    const toProcess = [nodeId];

    while (toProcess.length > 0) {
      const currentId = toProcess.shift()!;
      const children = await this.getChildren(currentId);

      for (const child of children) {
        descendants.push(child);
        toProcess.push(child.id);
      }
    }

    return descendants;
  }

  // ========================================================================
  // Backlink/Mentions Operations
  // ========================================================================

  /**
   * Get all nodes that mention the specified node (backlinks)
   */
  async getBacklinks(nodeId: string): Promise<NodeSpaceNode[]> {
    const mentioningIds = this.mentionsIndex.get(nodeId) || new Set();
    const backlinks: NodeSpaceNode[] = [];

    for (const mentioningId of mentioningIds) {
      const node = this.nodes.get(mentioningId);
      if (node) {
        backlinks.push({ ...node });
      }
    }

    return backlinks;
  }

  /**
   * Get all nodes mentioned by the specified node (forward links)
   */
  async getMentions(nodeId: string): Promise<NodeSpaceNode[]> {
    const mentionedIds = this.reverseIndex.get(nodeId) || new Set();
    const mentions: NodeSpaceNode[] = [];

    for (const mentionedId of mentionedIds) {
      const node = this.nodes.get(mentionedId);
      if (node) {
        mentions.push({ ...node });
      }
    }

    return mentions;
  }

  /**
   * Update mentions for a node (maintains bidirectional consistency)
   */
  async updateNodeMentions(nodeId: string, newMentions: string[]): Promise<void> {
    const node = this.nodes.get(nodeId);
    if (!node) {
      throw new Error(`Node ${nodeId} not found`);
    }

    // The old mentions will be cleaned up automatically by updateNode

    // Update the node
    await this.updateNode(nodeId, { mentions: [...newMentions] });

    // The indexes are automatically updated by updateNode, but let's verify consistency
    this.verifyMentionsConsistency(nodeId);
  }

  // ========================================================================
  // Utility Operations
  // ========================================================================

  /**
   * Get database statistics
   */
  getStats(): {
    totalNodes: number;
    nodesByType: Record<string, number>;
    totalMentions: number;
    averageMentionsPerNode: number;
  } {
    const nodesByType: Record<string, number> = {};
    let totalMentions = 0;

    for (const node of this.nodes.values()) {
      nodesByType[node.type] = (nodesByType[node.type] || 0) + 1;
      totalMentions += node.mentions.length;
    }

    return {
      totalNodes: this.nodes.size,
      nodesByType,
      totalMentions,
      averageMentionsPerNode: this.nodes.size > 0 ? totalMentions / this.nodes.size : 0
    };
  }

  /**
   * Clear all data (for testing)
   */
  async clear(): Promise<void> {
    this.nodes.clear();
    this.mentionsIndex.clear();
    this.reverseIndex.clear();
    this.typeIndex.clear();
    this.parentIndex.clear();
    this.rootIndex.clear();
  }

  /**
   * Export all data (for debugging/backup)
   */
  exportData(): NodeSpaceNode[] {
    return Array.from(this.nodes.values()).map((node) => ({ ...node }));
  }

  /**
   * Import data (for testing/restore)
   */
  async importData(nodes: NodeSpaceNode[]): Promise<void> {
    await this.clear();

    for (const node of nodes) {
      await this.insertNode(node);
    }
  }

  // ========================================================================
  // Private Implementation
  // ========================================================================

  /**
   * Validate node schema
   */
  private validateNode(node: NodeSpaceNode): void {
    if (!node.id || typeof node.id !== 'string') {
      throw new Error('Node ID must be a non-empty string');
    }

    if (!node.type || typeof node.type !== 'string') {
      throw new Error('Node type must be a non-empty string');
    }

    if (typeof node.content !== 'string') {
      throw new Error('Node content must be a string');
    }

    if (node.parent_id !== null && typeof node.parent_id !== 'string') {
      throw new Error('Node parent_id must be string or null');
    }

    if (!node.root_id || typeof node.root_id !== 'string') {
      throw new Error('Node root_id must be a non-empty string');
    }

    if (node.before_sibling_id !== null && typeof node.before_sibling_id !== 'string') {
      throw new Error('Node before_sibling_id must be string or null');
    }

    if (!Array.isArray(node.mentions)) {
      throw new Error('Node mentions must be an array');
    }

    if (typeof node.metadata !== 'object' || node.metadata === null) {
      throw new Error('Node metadata must be an object');
    }

    if (!node.created_at || typeof node.created_at !== 'string') {
      throw new Error('Node created_at must be a non-empty string');
    }
  }

  /**
   * Update indexes when inserting a node
   */
  private updateIndexesOnInsert(node: NodeSpaceNode): void {
    // Type index
    if (!this.typeIndex.has(node.type)) {
      this.typeIndex.set(node.type, new Set());
    }
    this.typeIndex.get(node.type)!.add(node.id);

    // Parent index
    if (!this.parentIndex.has(node.parent_id)) {
      this.parentIndex.set(node.parent_id, new Set());
    }
    this.parentIndex.get(node.parent_id)!.add(node.id);

    // Root index
    if (!this.rootIndex.has(node.root_id)) {
      this.rootIndex.set(node.root_id, new Set());
    }
    this.rootIndex.get(node.root_id)!.add(node.id);

    // Mentions indexes (bidirectional)
    for (const mentionedId of node.mentions) {
      // Forward index: node -> mentions
      if (!this.reverseIndex.has(node.id)) {
        this.reverseIndex.set(node.id, new Set());
      }
      this.reverseIndex.get(node.id)!.add(mentionedId);

      // Reverse index: mentioned -> mentioning nodes (backlinks)
      if (!this.mentionsIndex.has(mentionedId)) {
        this.mentionsIndex.set(mentionedId, new Set());
      }
      this.mentionsIndex.get(mentionedId)!.add(node.id);
    }
  }

  /**
   * Update indexes when deleting a node
   */
  private updateIndexesOnDelete(node: NodeSpaceNode): void {
    // Type index
    this.typeIndex.get(node.type)?.delete(node.id);
    if (this.typeIndex.get(node.type)?.size === 0) {
      this.typeIndex.delete(node.type);
    }

    // Parent index
    this.parentIndex.get(node.parent_id)?.delete(node.id);
    if (this.parentIndex.get(node.parent_id)?.size === 0) {
      this.parentIndex.delete(node.parent_id);
    }

    // Root index
    this.rootIndex.get(node.root_id)?.delete(node.id);
    if (this.rootIndex.get(node.root_id)?.size === 0) {
      this.rootIndex.delete(node.root_id);
    }

    // Mentions indexes cleanup
    for (const mentionedId of node.mentions) {
      // Remove from forward index
      this.reverseIndex.get(node.id)?.delete(mentionedId);
      if (this.reverseIndex.get(node.id)?.size === 0) {
        this.reverseIndex.delete(node.id);
      }

      // Remove from reverse index (backlinks)
      this.mentionsIndex.get(mentionedId)?.delete(node.id);
      if (this.mentionsIndex.get(mentionedId)?.size === 0) {
        this.mentionsIndex.delete(mentionedId);
      }
    }

    // Clean up any nodes that mentioned this deleted node
    const backlinks = this.mentionsIndex.get(node.id) || new Set();
    for (const backlinkNodeId of backlinks) {
      const backlinkNode = this.nodes.get(backlinkNodeId);
      if (backlinkNode) {
        const updatedMentions = backlinkNode.mentions.filter((id) => id !== node.id);
        // Note: This creates a recursive update, but it's necessary for consistency
        this.updateNode(backlinkNodeId, { mentions: updatedMentions });
      }
    }
  }

  /**
   * Sort siblings according to before_sibling_id linked list order
   */
  private sortSiblings(siblings: NodeSpaceNode[]): NodeSpaceNode[] {
    if (siblings.length <= 1) {
      return siblings;
    }

    // Create a map for quick lookup
    const siblingMap = new Map<string, NodeSpaceNode>();
    const hasNext = new Set<string>(); // Nodes that have a node after them

    for (const sibling of siblings) {
      siblingMap.set(sibling.id, sibling);
      if (sibling.before_sibling_id && siblingMap.has(sibling.before_sibling_id)) {
        hasNext.add(sibling.before_sibling_id);
      }
    }

    // Find the first node (one that is not after any other node)
    const firstNodes = siblings.filter(
      (s) => !s.before_sibling_id || !siblingMap.has(s.before_sibling_id)
    );

    if (firstNodes.length === 0) {
      // Circular reference or broken chain, return original order
      return siblings;
    }

    // Build the ordered chain starting from the first node
    const ordered: NodeSpaceNode[] = [];
    const processed = new Set<string>();
    let current: NodeSpaceNode | null = firstNodes[0]; // Take first valid starting point

    while (current && !processed.has(current.id)) {
      ordered.push(current);
      processed.add(current.id);

      // Find next node (one that comes after current)
      const next = siblings.find(
        (s) => s.before_sibling_id === current!.id && !processed.has(s.id)
      );
      current = next || null;

      // Prevent infinite loops
      if (ordered.length >= siblings.length) {
        break;
      }
    }

    // Add any remaining nodes that weren't in the chain
    for (const sibling of siblings) {
      if (!processed.has(sibling.id)) {
        ordered.push(sibling);
      }
    }

    return ordered;
  }

  /**
   * Verify mentions bidirectional consistency (for debugging)
   */
  private verifyMentionsConsistency(nodeId: string): boolean {
    const node = this.nodes.get(nodeId);
    if (!node) return false;

    // Check forward mentions
    for (const mentionedId of node.mentions) {
      const backlinks = this.mentionsIndex.get(mentionedId) || new Set();
      if (!backlinks.has(nodeId)) {
        console.error(
          `Mentions inconsistency: ${nodeId} mentions ${mentionedId} but backlink missing`
        );
        return false;
      }
    }

    // Check reverse mentions
    const nodeBacklinks = this.mentionsIndex.get(nodeId) || new Set();
    for (const backlinkId of nodeBacklinks) {
      const backlinkNode = this.nodes.get(backlinkId);
      if (!backlinkNode || !backlinkNode.mentions.includes(nodeId)) {
        console.error(`Mentions inconsistency: ${backlinkId} should mention ${nodeId} but doesn't`);
        return false;
      }
    }

    return true;
  }
}

export default MockDatabaseService;
