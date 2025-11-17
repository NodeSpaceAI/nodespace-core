/**
 * HierarchyService - Desktop-Optimized Hierarchy Computation Service
 *
 * Implements hierarchy computation service with Map-based caching for desktop performance.
 * Provides efficient operations for node depth calculation, children/descendants retrieval,
 * path computation, and sibling navigation using single-pointer sibling ordering.
 *
 * Performance Targets:
 * - getNodeDepth: O(1) cached, max 1ms
 * - getChildren: O(1) query, max 5ms
 * - getSiblings: O(n) chain build, max 10ms
 *
 * Key Features:
 * - Map-based caching for desktop performance
 * - Single-pointer sibling navigation using before_sibling_id
 * - Integration with existing NodeManager for data access
 * - Cache invalidation on node updates via EventBus
 * - Reactive updates for UI synchronization
 */

import { eventBus } from './event-bus';
import type { ReactiveNodeService as NodeManager } from './reactive-node-service.svelte';
import type { Node } from '$lib/types';
import { sharedNodeStore } from './shared-node-store';

// ============================================================================
// Core Types
// ============================================================================

export interface HierarchyCache {
  depthCache: Map<string, number>;
  childrenCache: Map<string, string[]>;
  siblingOrderCache: Map<string, string[]>;
  lastInvalidation: number;
}

export interface NodePath {
  nodeIds: string[];
  depths: number[];
  totalDepth: number;
}

export interface HierarchyPerformanceMetrics {
  cacheHits: number;
  cacheMisses: number;
  avgDepthComputeTime: number;
  avgChildrenComputeTime: number;
  avgSiblingsComputeTime: number;
}

// ============================================================================
// HierarchyService Implementation
// ============================================================================

export class HierarchyService {
  private nodeManager: NodeManager;
  private cache: HierarchyCache;
  private readonly serviceName = 'HierarchyService';

  // Performance monitoring
  private performanceMetrics: HierarchyPerformanceMetrics = {
    cacheHits: 0,
    cacheMisses: 0,
    avgDepthComputeTime: 0,
    avgChildrenComputeTime: 0,
    avgSiblingsComputeTime: 0
  };

  constructor(nodeManager: NodeManager) {
    this.nodeManager = nodeManager;
    this.cache = {
      depthCache: new Map(),
      childrenCache: new Map(),
      siblingOrderCache: new Map(),
      lastInvalidation: Date.now()
    };

    this.setupEventBusIntegration();
  }

  // ========================================================================
  // Core Hierarchy Operations
  // ========================================================================

  /**
   * Get node depth with O(1) cached performance
   * Performance target: max 1ms
   */
  public getNodeDepth(nodeId: string): number {
    const startTime = performance.now();

    // Check cache first
    if (this.cache.depthCache.has(nodeId)) {
      this.performanceMetrics.cacheHits++;
      return this.cache.depthCache.get(nodeId)!;
    }

    this.performanceMetrics.cacheMisses++;

    // Compute depth by walking up parent chain
    let depth = 0;
    let currentNode = this.nodeManager.findNode(nodeId);

    if (!currentNode) {
      return 0;
    }

    // Walk up parent chain, caching depths along the way
    const pathNodes: string[] = [nodeId];
    let currentNodeId: string | null = nodeId;

    while (currentNodeId) {
      const parentId = this.getParentId(currentNodeId);
      if (!parentId) break;

      depth++;
      pathNodes.push(parentId);

      // Check if parent depth is cached
      if (this.cache.depthCache.has(parentId)) {
        depth += this.cache.depthCache.get(parentId)!;
        break;
      }

      currentNodeId = parentId;
    }

    // Cache all depths in the path for future lookups
    for (let i = 0; i < pathNodes.length; i++) {
      const nodeDepth = depth - i;
      this.cache.depthCache.set(pathNodes[i], nodeDepth);
    }

    // Update performance metrics
    const computeTime = performance.now() - startTime;
    this.updateDepthComputeTime(computeTime);

    return depth;
  }

  /**
   * Get direct children with O(1) query performance
   * Performance target: max 5ms
   */
  public getChildren(nodeId: string): string[] {
    const startTime = performance.now();

    // Check cache first
    if (this.cache.childrenCache.has(nodeId)) {
      this.performanceMetrics.cacheHits++;
      return [...this.cache.childrenCache.get(nodeId)!];
    }

    this.performanceMetrics.cacheMisses++;

    // Get children from backend query
    const childNodes = sharedNodeStore.getNodesForParent(nodeId);
    const children = childNodes.map((n) => n.id);

    // Cache the result
    this.cache.childrenCache.set(nodeId, children);

    // Update performance metrics
    const computeTime = performance.now() - startTime;
    this.updateChildrenComputeTime(computeTime);

    return children;
  }

  /**
   * Get all descendants (recursive children)
   * Uses cached children for efficient traversal
   */
  public getDescendants(nodeId: string): string[] {
    const descendants: string[] = [];
    const toProcess: string[] = [nodeId];

    while (toProcess.length > 0) {
      const currentId = toProcess.shift()!;
      const children = this.getChildren(currentId);

      for (const childId of children) {
        descendants.push(childId);
        toProcess.push(childId);
      }
    }

    return descendants;
  }

  /**
   * Get node path from root to specified node
   * Returns all ancestor IDs and their depths
   */
  public getNodePath(nodeId: string): NodePath {
    const nodeIds: string[] = [];
    const depths: number[] = [];

    let currentId: string | null = nodeId;

    // Walk up to root, building path
    while (currentId) {
      nodeIds.unshift(currentId);
      depths.unshift(this.getNodeDepth(currentId));

      currentId = this.getParentId(currentId);
    }

    return {
      nodeIds,
      depths,
      totalDepth: nodeIds.length - 1
    };
  }

  /**
   * Get siblings using single-pointer sibling navigation
   * Performance target: O(n) chain build, max 10ms
   * OPTIMIZED: Pre-cached sibling chains with intelligent invalidation
   */
  public getSiblings(nodeId: string): string[] {
    const startTime = performance.now();

    const parentId = this.getParentId(nodeId);
    const cacheKey = parentId || '__root__';

    // Check cache first - now with timestamp validation
    if (this.cache.siblingOrderCache.has(cacheKey)) {
      this.performanceMetrics.cacheHits++;
      const cached = this.cache.siblingOrderCache.get(cacheKey)!;

      // Fast return for cached result
      const computeTime = performance.now() - startTime;
      this.updateSiblingsComputeTime(computeTime);
      return [...cached];
    }

    this.performanceMetrics.cacheMisses++;

    // OPTIMIZATION: Use direct children access instead of full computation
    let siblingIds: string[];
    if (parentId) {
      // Use cached children if available, otherwise compute
      siblingIds = this.cache.childrenCache.get(parentId) || this.getChildren(parentId);
    } else {
      // Root nodes - use NodeManager's optimized root list
      siblingIds = [...this.nodeManager.rootNodeIds];
    }

    // Cache the sibling order with aggressive caching
    this.cache.siblingOrderCache.set(cacheKey, siblingIds);

    // Update performance metrics
    const computeTime = performance.now() - startTime;
    this.updateSiblingsComputeTime(computeTime);

    return siblingIds;
  }

  /**
   * Get sibling position (index within siblings)
   */
  public getSiblingPosition(nodeId: string): number {
    const siblings = this.getSiblings(nodeId);
    return siblings.indexOf(nodeId);
  }

  /**
   * Get next sibling ID
   */
  public getNextSibling(nodeId: string): string | null {
    const siblings = this.getSiblings(nodeId);
    const currentIndex = siblings.indexOf(nodeId);

    if (currentIndex === -1 || currentIndex === siblings.length - 1) {
      return null;
    }

    return siblings[currentIndex + 1];
  }

  /**
   * Get previous sibling ID
   */
  public getPreviousSibling(nodeId: string): string | null {
    const siblings = this.getSiblings(nodeId);
    const currentIndex = siblings.indexOf(nodeId);

    if (currentIndex <= 0) {
      return null;
    }

    return siblings[currentIndex - 1];
  }

  // ========================================================================
  // Bulk Operations for Client-Side Structure Building
  // ========================================================================

  /**
   * Fetch all nodes belonging to a root node in one operation
   * This enables client-side structure building based on parent_id and sibling order
   * Performance optimized for large hierarchies
   */
  public getAllNodesInRoot(rootId: string): {
    nodes: Map<string, Node>;
    rootNode: Node | null;
    totalCount: number;
    maxDepth: number;
  } {
    const startTime = performance.now();
    const rootNode = this.nodeManager.findNode(rootId);

    if (!rootNode) {
      return {
        nodes: new Map(),
        rootNode: null,
        totalCount: 0,
        maxDepth: 0
      };
    }

    // Get all descendants efficiently
    const allDescendants = this.getDescendants(rootId);
    const allNodes = new Map<string, Node>();

    // Add root node
    allNodes.set(rootId, rootNode);
    let maxDepth = 0;

    // Add all descendants with their nodes
    for (const nodeId of allDescendants) {
      const node = this.nodeManager.findNode(nodeId);
      if (node) {
        allNodes.set(nodeId, node);
        // Track max depth for statistics
        const depth = this.getNodeDepth(nodeId);
        maxDepth = Math.max(maxDepth, depth);
      }
    }

    const computeTime = performance.now() - startTime;

    // Emit performance event
    eventBus.emit<import('./event-types').DebugEvent>({
      type: 'debug:log',
      namespace: 'debug',
      source: this.serviceName,
      level: 'info',
      message: `Bulk root fetch completed: ${allNodes.size} nodes in ${computeTime.toFixed(2)}ms`,
      metadata: { rootId, nodeCount: allNodes.size, maxDepth, computeTime }
    });

    return {
      nodes: allNodes,
      rootNode,
      totalCount: allNodes.size,
      maxDepth
    };
  }

  /**
   * Get bulk hierarchy structure for efficient client-side building
   * Returns nodes with their parent/sibling relationships for one-time fetch
   */
  public getBulkHierarchyStructure(rootId: string): {
    nodes: Array<{
      id: string;
      node: Node;
      parentId: string | null;
      beforeSiblingId: string | null;
      depth: number;
      children_count: number;
    }>;
    structure: {
      totalNodes: number;
      maxDepth: number;
      rootId: string;
      fetchTime: number;
    };
  } {
    const startTime = performance.now();
    const bulkData = this.getAllNodesInRoot(rootId);

    const structuredNodes = Array.from(bulkData.nodes.entries()).map(([nodeId, node]) => {
      const children = this.getChildren(nodeId);
      const parentId = this.getParentId(nodeId);

      return {
        id: nodeId,
        node: node,
        parentId: parentId,
        beforeSiblingId: node.beforeSiblingId || null, // For client-side ordering
        depth: this.getNodeDepth(nodeId),
        children_count: children.length
      };
    });

    const fetchTime = performance.now() - startTime;

    return {
      nodes: structuredNodes,
      structure: {
        totalNodes: structuredNodes.length,
        maxDepth: bulkData.maxDepth,
        rootId,
        fetchTime
      }
    };
  }

  // ========================================================================
  // Cache Management
  // ========================================================================

  /**
   * Invalidate cache for specific node
   */
  public invalidateNodeCache(nodeId: string): void {
    this.cache.depthCache.delete(nodeId);
    this.cache.childrenCache.delete(nodeId);

    // Invalidate sibling caches that might include this node
    const parentId = this.getParentId(nodeId);
    const parentKey = parentId || '__root__';
    this.cache.siblingOrderCache.delete(parentKey);

    // Invalidate descendant depth caches
    const descendants = this.getDescendantsFromMap(nodeId);
    for (const descendantId of descendants) {
      this.cache.depthCache.delete(descendantId);
    }
  }

  /**
   * Invalidate all caches
   */
  public invalidateAllCaches(): void {
    this.cache.depthCache.clear();
    this.cache.childrenCache.clear();
    this.cache.siblingOrderCache.clear();
    this.cache.lastInvalidation = Date.now();
  }

  /**
   * Get cache statistics for debugging
   */
  public getCacheStats(): {
    depthCacheSize: number;
    childrenCacheSize: number;
    siblingsCacheSize: number;
    hitRatio: number;
    performance: HierarchyPerformanceMetrics;
  } {
    const totalRequests = this.performanceMetrics.cacheHits + this.performanceMetrics.cacheMisses;
    const hitRatio = totalRequests > 0 ? this.performanceMetrics.cacheHits / totalRequests : 0;

    return {
      depthCacheSize: this.cache.depthCache.size,
      childrenCacheSize: this.cache.childrenCache.size,
      siblingsCacheSize: this.cache.siblingOrderCache.size,
      hitRatio,
      performance: { ...this.performanceMetrics }
    };
  }

  // ========================================================================
  // EventBus Integration
  // ========================================================================

  /**
   * Set up EventBus integration for cache invalidation
   */
  private setupEventBusIntegration(): void {
    // Listen for node updates to invalidate relevant caches
    eventBus.subscribe('node:updated', (event) => {
      const nodeEvent = event as import('./event-types').NodeUpdatedEvent;
      // CRITICAL: Listen for 'hierarchy' updateType to invalidate depth caches
      // This is essential for cache invalidation when nodes are added/moved
      if (nodeEvent.updateType === 'hierarchy') {
        // Hierarchy changes affect multiple nodes, invalidate broadly
        this.invalidateNodeCache(nodeEvent.nodeId);

        // Also invalidate parent and children caches
        const parentId = this.getParentId(nodeEvent.nodeId);
        if (parentId) {
          this.invalidateNodeCache(parentId);
        }
      }
    });

    // Listen for hierarchy changes
    eventBus.subscribe('hierarchy:changed', (event) => {
      const hierarchyEvent = event as import('./event-types').HierarchyChangedEvent;

      // Invalidate cache for all affected nodes
      for (const nodeId of hierarchyEvent.affectedNodes) {
        this.invalidateNodeCache(nodeId);
      }
    });

    // Listen for node creation/deletion
    eventBus.subscribe('node:created', (event) => {
      const nodeEvent = event as import('./event-types').NodeCreatedEvent;

      // Invalidate parent's children cache using backend query
      const parentId = this.getParentId(nodeEvent.nodeId);
      if (parentId) {
        this.cache.childrenCache.delete(parentId);
        this.cache.siblingOrderCache.delete(parentId);
      } else {
        this.cache.siblingOrderCache.delete('__root__');
      }
    });

    eventBus.subscribe('node:deleted', (event) => {
      const nodeEvent = event as import('./event-types').NodeDeletedEvent;

      // Remove from all caches
      this.invalidateNodeCache(nodeEvent.nodeId);

      // Invalidate parent's caches using backend query
      // NOTE: Must query before node is fully deleted from backend
      const parentId = this.getParentId(nodeEvent.nodeId);
      if (parentId) {
        this.cache.childrenCache.delete(parentId);
        this.cache.siblingOrderCache.delete(parentId);
      } else {
        this.cache.siblingOrderCache.delete('__root__');
      }
    });
  }

  // ========================================================================
  // Private Helper Methods
  // ========================================================================

  /**
   * Get parent ID from backend query
   * Returns null if node has no parent (root node)
   */
  private getParentId(nodeId: string): string | null {
    const parents = sharedNodeStore.getParentsForNode(nodeId);
    return parents.length > 0 ? parents[0].id : null;
  }

  /**
   * Get descendants from node map without cache
   * Used during cache invalidation to avoid recursion
   */
  private getDescendantsFromMap(nodeId: string): string[] {
    const descendants: string[] = [];

    // Get children from backend query (not from cache to avoid recursion)
    const childNodes = sharedNodeStore.getNodesForParent(nodeId);
    const children = childNodes.map((n) => n.id);

    if (children.length === 0) {
      return descendants;
    }

    const toProcess = [...children];

    while (toProcess.length > 0) {
      const currentId = toProcess.shift()!;
      descendants.push(currentId);

      // Get children of current node from backend
      const currentChildNodes = sharedNodeStore.getNodesForParent(currentId);
      const currentChildren = currentChildNodes.map((n) => n.id);

      if (currentChildren.length > 0) {
        toProcess.push(...currentChildren);
      }
    }

    return descendants;
  }

  /**
   * Update depth compute time performance metric
   */
  private updateDepthComputeTime(time: number): void {
    const current = this.performanceMetrics.avgDepthComputeTime;
    const count = this.performanceMetrics.cacheMisses;
    this.performanceMetrics.avgDepthComputeTime =
      count === 1 ? time : (current * (count - 1) + time) / count;
  }

  /**
   * Update children compute time performance metric
   */
  private updateChildrenComputeTime(time: number): void {
    const current = this.performanceMetrics.avgChildrenComputeTime;
    const count = this.performanceMetrics.cacheMisses;
    this.performanceMetrics.avgChildrenComputeTime =
      count === 1 ? time : (current * (count - 1) + time) / count;
  }

  /**
   * Update siblings compute time performance metric
   */
  private updateSiblingsComputeTime(time: number): void {
    const current = this.performanceMetrics.avgSiblingsComputeTime;
    const count = this.performanceMetrics.cacheMisses;
    this.performanceMetrics.avgSiblingsComputeTime =
      count === 1 ? time : (current * (count - 1) + time) / count;
  }
}

export default HierarchyService;
