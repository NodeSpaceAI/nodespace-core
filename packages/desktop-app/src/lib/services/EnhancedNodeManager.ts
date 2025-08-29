/**
 * EnhancedNodeManager - Enhanced Node Management with Service Composition
 * 
 * Extends the existing NodeManager using composition to add enhanced methods
 * that leverage HierarchyService and NodeOperationsService while maintaining
 * full backward compatibility with existing EventBus integrations.
 * 
 * Key Features:
 * - Full backward compatibility with existing NodeManager
 * - Enhanced hierarchy operations via HierarchyService
 * - Smart content operations via NodeOperationsService
 * - Improved performance through service caching
 * - Rich node analysis and metadata handling
 * - Seamless integration with existing EventBus patterns
 */

import { NodeManager, type NodeManagerEvents, type Node } from './NodeManager';
import { HierarchyService } from './HierarchyService';
import { NodeOperationsService } from './NodeOperationsService';
import { ContentProcessor } from './ContentProcessor';
import { eventBus } from './EventBus';
// import type { NodeSpaceNode } from './MockDatabaseService';

// ============================================================================
// Enhanced Types
// ============================================================================

export interface EnhancedNode extends Node {
  // Additional properties for enhanced functionality
  mentions?: string[];
  computedDepth?: number;
  siblingPosition?: number;
  rootAncestorId?: string;
  lastAnalyzed?: number;
}

export interface NodeAnalysis {
  nodeId: string;
  contentType: string;
  wordCount: number;
  hasWikiLinks: boolean;
  wikiLinks: string[];
  headerLevel: number;
  formattingComplexity: number;
  mentionsCount: number;
  backlinksCount: number;
  hierarchyDepth: number;
  childrenCount: number;
  descendantsCount: number;
}

export interface BulkOperationResult {
  successCount: number;
  failureCount: number;
  failedNodes: string[];
  affectedNodes: string[];
  operationTime: number;
}

// ============================================================================
// EnhancedNodeManager Implementation
// ============================================================================

export class EnhancedNodeManager extends NodeManager {
  private hierarchyService: HierarchyService;
  private nodeOperationsService: NodeOperationsService;
  private contentProcessor: ContentProcessor;
  private readonly serviceName = 'EnhancedNodeManager';

  // Enhanced caching
  private analysisCache: Map<string, NodeAnalysis> = new Map();
  private lastGlobalAnalysis = 0;

  constructor(events: NodeManagerEvents) {
    super(events);
    
    // Initialize enhanced services
    this.hierarchyService = new HierarchyService(this);
    this.contentProcessor = ContentProcessor.getInstance();
    this.nodeOperationsService = new NodeOperationsService(
      this,
      this.hierarchyService,
      this.contentProcessor
    );

    this.setupEnhancedEventBusIntegration();
  }

  // ========================================================================
  // Enhanced Hierarchy Operations
  // ========================================================================

  /**
   * Get node depth using cached hierarchy service
   * Much faster than walking the parent chain manually
   */
  public getEnhancedNodeDepth(nodeId: string): number {
    return this.hierarchyService.getNodeDepth(nodeId);
  }

  /**
   * Get direct children using cached hierarchy service
   */
  public getEnhancedChildren(nodeId: string): Node[] {
    const childIds = this.hierarchyService.getChildren(nodeId);
    return childIds.map(id => this.findNode(id)).filter(node => node !== null) as Node[];
  }

  /**
   * Get all descendants efficiently
   */
  public getEnhancedDescendants(nodeId: string): Node[] {
    const descendantIds = this.hierarchyService.getDescendants(nodeId);
    return descendantIds.map(id => this.findNode(id)).filter(node => node !== null) as Node[];
  }

  /**
   * Get node path from root with depths
   */
  public getNodePath(nodeId: string): { nodes: Node[]; depths: number[] } {
    const path = this.hierarchyService.getNodePath(nodeId);
    const nodes = path.nodeIds.map(id => this.findNode(id)).filter(node => node !== null) as Node[];
    
    return {
      nodes,
      depths: path.depths
    };
  }

  /**
   * Get sibling nodes with positioning information
   */
  public getEnhancedSiblings(nodeId: string): {
    siblings: Node[];
    currentPosition: number;
    nextSibling: Node | null;
    previousSibling: Node | null;
  } {
    const siblingIds = this.hierarchyService.getSiblings(nodeId);
    const siblings = siblingIds.map(id => this.findNode(id)).filter(node => node !== null) as Node[];
    const currentPosition = this.hierarchyService.getSiblingPosition(nodeId);
    
    const nextSiblingId = this.hierarchyService.getNextSibling(nodeId);
    const previousSiblingId = this.hierarchyService.getPreviousSibling(nodeId);
    
    return {
      siblings,
      currentPosition,
      nextSibling: nextSiblingId ? this.findNode(nextSiblingId) : null,
      previousSibling: previousSiblingId ? this.findNode(previousSiblingId) : null
    };
  }

  // ========================================================================
  // Enhanced Content Operations
  // ========================================================================

  /**
   * Create node with enhanced content processing
   */
  public createEnhancedNode(
    afterNodeId: string,
    content: string = '',
    options: {
      nodeType?: string;
      inheritHeaderLevel?: number;
      cursorAtBeginning?: boolean;
      metadata?: Record<string, unknown>;
      mentions?: string[];
    } = {}
  ): string {
    // Use parent's creation method first
    const nodeId = this.createNode(
      afterNodeId,
      content,
      options.nodeType || 'text',
      options.inheritHeaderLevel,
      options.cursorAtBeginning || false
    );

    // Add enhanced properties
    const node = this.findNode(nodeId);
    if (node && options.metadata) {
      node.metadata = { ...node.metadata, ...options.metadata };
    }

    // Set up mentions if provided
    if (options.mentions && options.mentions.length > 0) {
      this.updateNodeMentions(nodeId, options.mentions);
    }

    // Invalidate analysis cache
    this.invalidateAnalysisCache(nodeId);

    return nodeId;
  }

  /**
   * Update node with enhanced processing
   */
  public updateEnhancedNode(
    nodeId: string,
    updates: {
      content?: string;
      metadata?: Record<string, unknown>;
      mentions?: string[];
      nodeType?: string;
    }
  ): boolean {
    const node = this.findNode(nodeId);
    if (!node) return false;

    // Update content
    if (updates.content !== undefined) {
      this.updateNodeContent(nodeId, updates.content);
    }

    // Update metadata
    if (updates.metadata) {
      node.metadata = { ...node.metadata, ...updates.metadata };
    }

    // Update node type
    if (updates.nodeType) {
      node.nodeType = updates.nodeType;
    }

    // Update mentions
    if (updates.mentions) {
      this.updateNodeMentions(nodeId, updates.mentions);
    }

    // Invalidate analysis cache
    this.invalidateAnalysisCache(nodeId);

    return true;
  }

  /**
   * Update node mentions with bidirectional consistency
   */
  public updateNodeMentions(nodeId: string, mentions: string[]): void {
    const node = this.findNode(nodeId);
    if (!node) return;

    // Add mentions property to node if not exists
    if (!node.mentions) {
      node.mentions = [];
    }

    const oldMentions = [...node.mentions];
    node.mentions = [...mentions];

    // Use NodeOperationsService for bidirectional consistency
    this.nodeOperationsService.updateNodeMentions(nodeId, mentions)
      .catch(error => {
        console.error('Failed to update node mentions:', error);
        // Rollback on failure
        node.mentions = oldMentions;
      });

    // Emit events
    eventBus.emit({
      type: 'node:updated',
      namespace: 'lifecycle',
      source: this.serviceName,
      timestamp: Date.now(),
      nodeId,
      updateType: 'metadata',
      previousValue: oldMentions,
      newValue: mentions
    });
  }

  // ========================================================================
  // Node Analysis and Intelligence
  // ========================================================================

  /**
   * Analyze a node for comprehensive insights
   */
  public analyzeNode(nodeId: string, useCache = true): NodeAnalysis | null {
    // Check cache first
    if (useCache && this.analysisCache.has(nodeId)) {
      const cached = this.analysisCache.get(nodeId)!;
      // Return cached if less than 5 minutes old
      if (Date.now() - cached.lastAnalyzed! < 300000) {
        return cached;
      }
    }

    const node = this.findNode(nodeId);
    if (!node) return null;

    // Analyze content
    const contentResult = this.nodeOperationsService.extractContentWithContext({
      content: node.content,
      type: node.nodeType,
      metadata: node.metadata
    });

    // Get hierarchy information
    const depth = this.hierarchyService.getNodeDepth(nodeId);
    const children = this.hierarchyService.getChildren(nodeId);
    const descendants = this.hierarchyService.getDescendants(nodeId);

    // Create analysis
    const analysis: NodeAnalysis = {
      nodeId,
      contentType: contentResult.ast.metadata.hasWikiLinks ? 'linked' : node.nodeType,
      wordCount: contentResult.wordCount,
      hasWikiLinks: contentResult.wikiLinks.length > 0,
      wikiLinks: contentResult.wikiLinks.map(link => link.target),
      headerLevel: contentResult.headerLevel,
      formattingComplexity: contentResult.hasFormatting ? 
        contentResult.ast.metadata.inlineFormatCount : 0,
      mentionsCount: node.mentions?.length || 0,
      backlinksCount: this.getNodeBacklinks(nodeId).length,
      hierarchyDepth: depth,
      childrenCount: children.length,
      descendantsCount: descendants.length
    };

    // Cache the result
    const enhancedAnalysis = { ...analysis, lastAnalyzed: Date.now() };
    this.analysisCache.set(nodeId, enhancedAnalysis);

    return analysis;
  }

  /**
   * Analyze all nodes for insights
   */
  public analyzeAllNodes(): {
    totalNodes: number;
    byType: Record<string, number>;
    avgDepth: number;
    avgWordCount: number;
    totalMentions: number;
    mostLinkedNodes: { nodeId: string; links: number }[];
  } {
    const analyses = Array.from(this.nodes.keys())
      .map(id => this.analyzeNode(id))
      .filter(analysis => analysis !== null) as NodeAnalysis[];

    const byType: Record<string, number> = {};
    let totalDepth = 0;
    let totalWordCount = 0;
    let totalMentions = 0;

    const linkCounts = new Map<string, number>();

    for (const analysis of analyses) {
      byType[analysis.contentType] = (byType[analysis.contentType] || 0) + 1;
      totalDepth += analysis.hierarchyDepth;
      totalWordCount += analysis.wordCount;
      totalMentions += analysis.mentionsCount;
      linkCounts.set(analysis.nodeId, analysis.mentionsCount + analysis.backlinksCount);
    }

    const mostLinkedNodes = Array.from(linkCounts.entries())
      .sort((a, b) => b[1] - a[1])
      .slice(0, 10)
      .map(([nodeId, links]) => ({ nodeId, links }));

    return {
      totalNodes: analyses.length,
      byType,
      avgDepth: analyses.length > 0 ? totalDepth / analyses.length : 0,
      avgWordCount: analyses.length > 0 ? totalWordCount / analyses.length : 0,
      totalMentions,
      mostLinkedNodes
    };
  }

  // ========================================================================
  // Bulk Operations
  // ========================================================================

  /**
   * Perform bulk operations on multiple nodes
   */
  public async bulkUpdateNodes(
    nodeIds: string[],
    updates: Partial<{
      content: string;
      metadata: Record<string, unknown>;
      nodeType: string;
    }>
  ): Promise<BulkOperationResult> {
    const startTime = performance.now();
    const result: BulkOperationResult = {
      successCount: 0,
      failureCount: 0,
      failedNodes: [],
      affectedNodes: [],
      operationTime: 0
    };

    for (const nodeId of nodeIds) {
      try {
        const success = this.updateEnhancedNode(nodeId, updates);
        if (success) {
          result.successCount++;
          result.affectedNodes.push(nodeId);
        } else {
          result.failureCount++;
          result.failedNodes.push(nodeId);
        }
      } catch (error) {
        result.failureCount++;
        result.failedNodes.push(nodeId);
        console.error(`Failed to update node ${nodeId}:`, error);
      }
    }

    result.operationTime = performance.now() - startTime;
    
    // Emit bulk operation event
    eventBus.emit({
      type: 'debug:log',
      namespace: 'debug',
      source: this.serviceName,
      timestamp: Date.now(),
      level: 'info',
      message: `Bulk operation completed: ${result.successCount} success, ${result.failureCount} failed`,
      metadata: result
    });

    return result;
  }

  // ========================================================================
  // Search and Filtering
  // ========================================================================

  /**
   * Search nodes with enhanced criteria
   */
  public searchNodes(criteria: {
    content?: string;
    nodeType?: string;
    hasWikiLinks?: boolean;
    mentionsNode?: string;
    minWordCount?: number;
    maxDepth?: number;
    inParent?: string;
  }): Node[] {
    const results: Node[] = [];

    for (const node of this.nodes.values()) {
      let matches = true;

      // Content search
      if (criteria.content) {
        const searchTerm = criteria.content.toLowerCase();
        if (!node.content.toLowerCase().includes(searchTerm)) {
          matches = false;
        }
      }

      // Type filter
      if (criteria.nodeType && node.nodeType !== criteria.nodeType) {
        matches = false;
      }

      // Wiki links filter
      if (criteria.hasWikiLinks !== undefined) {
        const hasLinks = this.contentProcessor.detectWikiLinks(node.content).length > 0;
        if (hasLinks !== criteria.hasWikiLinks) {
          matches = false;
        }
      }

      // Mentions filter
      if (criteria.mentionsNode && node.mentions) {
        if (!node.mentions.includes(criteria.mentionsNode)) {
          matches = false;
        }
      }

      // Word count filter
      if (criteria.minWordCount !== undefined) {
        const wordCount = node.content.split(/\s+/).filter(w => w.length > 0).length;
        if (wordCount < criteria.minWordCount) {
          matches = false;
        }
      }

      // Depth filter
      if (criteria.maxDepth !== undefined) {
        const depth = this.hierarchyService.getNodeDepth(node.id);
        if (depth > criteria.maxDepth) {
          matches = false;
        }
      }

      // Parent filter
      if (criteria.inParent && node.parentId !== criteria.inParent) {
        matches = false;
      }

      if (matches) {
        results.push(node);
      }
    }

    return results;
  }

  // ========================================================================
  // Performance and Statistics
  // ========================================================================

  /**
   * Get enhanced performance statistics
   */
  public getEnhancedStats(): {
    nodeManager: {
      totalNodes: number;
      rootNodes: number;
      collapsedNodes: number;
    };
    hierarchyService: unknown;
    analysisCache: {
      size: number;
      hitRatio: number;
      oldestEntry: number;
    };
    contentAnalysis: {
      totalContentLength: number;
      averageContentLength: number;
      headerDistribution: Record<string, number>;
    };
  } {
    const hierarchyStats = this.hierarchyService.getCacheStats();
    
    return {
      nodeManager: {
        totalNodes: this.nodes.size,
        rootNodes: this.rootNodeIds.length,
        collapsedNodes: this.collapsedNodes.size
      },
      hierarchyService: hierarchyStats,
      analysisCache: {
        size: this.analysisCache.size,
        hitRatio: this.analysisCache.size / this.nodes.size,
        oldestEntry: this.getOldestAnalysisCacheEntry()
      },
      contentAnalysis: this.analyzeAllNodes()
    };
  }

  // ========================================================================
  // Bulk Operations for Client-Side Structure Building
  // ========================================================================

  /**
   * Fetch all nodes in a root hierarchy for client-side structure building
   * This method enables efficient bulk fetching that clients can then organize
   * based on parent_id and sibling ordering relationships
   */
  public getAllNodesInRoot(rootId: string): {
    nodes: Map<string, Node>;
    rootNode: Node | null;
    totalCount: number;
    maxDepth: number;
    structure: Array<{
      id: string;
      node: Node;
      parent_id: string | null;
      before_sibling_id: string | null;
      depth: number;
      children_count: number;
      mentions_count: number;
      backlinks_count: number;
    }>;
  } {
    // Use HierarchyService for efficient bulk fetching
    const hierarchyResult = this.hierarchyService.getAllNodesInRoot(rootId);
    
    // Convert to Node objects and add enhanced metadata
    const nodeMap = new Map<string, Node>();
    const structure: Array<{
      id: string;
      node: Node;
      parent_id: string | null;
      before_sibling_id: string | null;
      depth: number;
      children_count: number;
      mentions_count: number;
      backlinks_count: number;
    }> = [];

    for (const [nodeId, nodeData] of hierarchyResult.nodes.entries()) {
      const node = nodeData as Node;
      nodeMap.set(nodeId, node);

      // Build structure information for client-side organization
      const children = this.hierarchyService.getChildren(nodeId);
      const backlinks = this.getNodeBacklinks(nodeId);

      structure.push({
        id: nodeId,
        node: node,
        parent_id: node.parentId || null,
        before_sibling_id: (node as unknown as { before_sibling_id?: string }).before_sibling_id || null,
        depth: this.hierarchyService.getNodeDepth(nodeId),
        children_count: children.length,
        mentions_count: node.mentions?.length || 0,
        backlinks_count: backlinks.length
      });
    }

    return {
      nodes: nodeMap,
      rootNode: hierarchyResult.rootNode as Node | null,
      totalCount: hierarchyResult.totalCount,
      maxDepth: hierarchyResult.maxDepth,
      structure
    };
  }

  /**
   * Get optimized hierarchy data for client rendering
   * Includes all necessary information for efficient client-side tree building
   */
  public getHierarchyForClient(rootId: string): {
    success: boolean;
    data?: {
      nodes: Node[];
      relationships: Array<{
        nodeId: string;
        parentId: string | null;
        beforeSiblingId: string | null;
        depth: number;
        hasChildren: boolean;
      }>;
      metadata: {
        totalNodes: number;
        maxDepth: number;
        rootId: string;
        fetchTime: number;
      };
    };
    error?: string;
  } {
    try {
      const startTime = performance.now();
      const bulkResult = this.getAllNodesInRoot(rootId);
      
      if (!bulkResult.rootNode) {
        return {
          success: false,
          error: `Root node ${rootId} not found`
        };
      }

      // Convert to client-friendly format
      const nodes = Array.from(bulkResult.nodes.values());
      const relationships = bulkResult.structure.map(item => ({
        nodeId: item.id,
        parentId: item.parent_id,
        beforeSiblingId: item.before_sibling_id,
        depth: item.depth,
        hasChildren: item.children_count > 0
      }));

      const fetchTime = performance.now() - startTime;

      return {
        success: true,
        data: {
          nodes,
          relationships,
          metadata: {
            totalNodes: bulkResult.totalCount,
            maxDepth: bulkResult.maxDepth,
            rootId,
            fetchTime
          }
        }
      };
    } catch (error) {
      return {
        success: false,
        error: error instanceof Error ? error.message : 'Unknown error occurred'
      };
    }
  }

  // ========================================================================
  // Backward Compatibility Maintenance
  // ========================================================================

  /**
   * Get nodes that mention a specific node (backlinks)
   */
  public getNodeBacklinks(nodeId: string): Node[] {
    const backlinks: Node[] = [];
    
    for (const node of this.nodes.values()) {
      if (node.mentions && node.mentions.includes(nodeId)) {
        backlinks.push(node);
      }
    }

    return backlinks;
  }

  // ========================================================================
  // Private Helper Methods
  // ========================================================================

  /**
   * Setup enhanced EventBus integration
   */
  private setupEnhancedEventBusIntegration(): void {
    // Listen for events that should invalidate analysis cache
    eventBus.subscribe('node:updated', (event) => {
      const nodeEvent = event as import('./EventTypes').NodeUpdatedEvent;
      this.invalidateAnalysisCache(nodeEvent.nodeId);
    });

    eventBus.subscribe('hierarchy:changed', (event) => {
      const hierarchyEvent = event as import('./EventTypes').HierarchyChangedEvent;
      
      // Invalidate analysis cache for affected nodes
      for (const nodeId of hierarchyEvent.affectedNodes) {
        this.invalidateAnalysisCache(nodeId);
      }
    });

    eventBus.subscribe('node:deleted', (event) => {
      const nodeEvent = event as import('./EventTypes').NodeDeletedEvent;
      this.analysisCache.delete(nodeEvent.nodeId);
    });
  }

  /**
   * Invalidate analysis cache for a node
   */
  private invalidateAnalysisCache(nodeId: string): void {
    this.analysisCache.delete(nodeId);
    
    // Also invalidate cache for nodes that might be affected
    const node = this.findNode(nodeId);
    if (node?.mentions) {
      for (const mentionedId of node.mentions) {
        this.analysisCache.delete(mentionedId);
      }
    }
  }

  /**
   * Get timestamp of oldest analysis cache entry
   */
  private getOldestAnalysisCacheEntry(): number {
    let oldest = Date.now();
    
    for (const analysis of this.analysisCache.values()) {
      if (analysis.lastAnalyzed && analysis.lastAnalyzed < oldest) {
        oldest = analysis.lastAnalyzed;
      }
    }

    return oldest;
  }
}

export default EnhancedNodeManager;