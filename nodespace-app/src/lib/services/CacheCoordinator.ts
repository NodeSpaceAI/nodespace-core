/**
 * CacheCoordinator - Cache Invalidation Coordination Service
 *
 * Coordinates cache invalidation across NodeSpace services to ensure
 * data consistency and optimal performance. Handles cache dependencies
 * and cascading invalidations.
 *
 * Key Features:
 * - Cache dependency tracking
 * - Cascading invalidation coordination
 * - Performance-optimized batch invalidation
 * - Cache hit/miss metrics
 * - Foundation for distributed caching (Phase 2+)
 */

import { eventBus } from './EventBus';
import type {
  CacheInvalidateEvent,
  NodeCreatedEvent,
  NodeUpdatedEvent,
  NodeDeletedEvent,
  HierarchyChangedEvent
} from './EventTypes';

// ============================================================================
// Types and Interfaces
// ============================================================================

export interface CacheEntry {
  key: string;
  nodeId?: string;
  lastUpdate: number;
  hitCount: number;
  dependencies: Set<string>;
  metadata?: Record<string, unknown>;
}

export interface CacheDependency {
  key: string;
  dependsOn: string[];
  invalidationStrategy: 'immediate' | 'batch' | 'lazy';
}

export interface CacheMetrics {
  totalEntries: number;
  totalHits: number;
  totalMisses: number;
  invalidationCount: number;
  averageHitRate: number;
  lastCleanup: number;
}

export interface CacheInvalidationStrategy {
  name: string;
  condition: (event: unknown, cache: CacheEntry) => boolean;
  priority: number;
  batchable: boolean;
}

// ============================================================================
// CacheCoordinator Service
// ============================================================================

export class CacheCoordinator {
  private static instance: CacheCoordinator;
  private readonly serviceName = 'CacheCoordinator';

  private cacheRegistry = new Map<string, CacheEntry>();
  private dependencies = new Map<string, CacheDependency>();
  private invalidationStrategies: CacheInvalidationStrategy[] = [];
  private pendingInvalidations = new Set<string>();
  private batchTimeout?: NodeJS.Timeout;
  private metrics: CacheMetrics = {
    totalEntries: 0,
    totalHits: 0,
    totalMisses: 0,
    invalidationCount: 0,
    averageHitRate: 0,
    lastCleanup: Date.now()
  };

  // Configuration
  private readonly BATCH_DELAY_MS = 50;
  private readonly MAX_CACHE_AGE_MS = 5 * 60 * 1000; // 5 minutes
  private readonly CLEANUP_INTERVAL_MS = 30 * 1000; // 30 seconds

  public static getInstance(): CacheCoordinator {
    if (!CacheCoordinator.instance) {
      CacheCoordinator.instance = new CacheCoordinator();
    }
    return CacheCoordinator.instance;
  }

  private constructor() {
    this.setupEventBusIntegration();
    this.setupDefaultInvalidationStrategies();
    this.startCleanupTimer();
  }

  // ========================================================================
  // Event Bus Integration
  // ========================================================================

  private setupEventBusIntegration(): void {
    // Listen for cache invalidation events
    eventBus.subscribe('cache:invalidate', (event) => {
      this.handleCacheInvalidation(event as import('./EventTypes').CacheInvalidateEvent);
    });

    // Listen for node lifecycle events
    eventBus.subscribe('node:created', (event) => {
      this.handleNodeLifecycleEvent(event as import('./EventTypes').NodeCreatedEvent);
    });

    eventBus.subscribe('node:updated', (event) => {
      this.handleNodeLifecycleEvent(event as import('./EventTypes').NodeUpdatedEvent);
    });

    eventBus.subscribe('node:deleted', (event) => {
      this.handleNodeLifecycleEvent(event as import('./EventTypes').NodeDeletedEvent);
    });

    eventBus.subscribe('hierarchy:changed', (event) => {
      this.handleHierarchyChanged(event as import('./EventTypes').HierarchyChangedEvent);
    });

    // Listen for reference events that might affect cache
    eventBus.subscribe('references:update-needed', (event) => {
      const refEvent = event as import('./EventTypes').ReferencesUpdateNeededEvent;
      this.invalidateCacheForNode(refEvent.nodeId, 'reference update');
    });

    // Listen for backlink detection (Phase 2+ preparation)
    eventBus.subscribe('backlink:detected', (event) => {
      const backlinkEvent = event as import('./EventTypes').BacklinkDetectedEvent;
      // Invalidate cache for both source and target nodes
      this.invalidateCacheForNode(backlinkEvent.sourceNodeId, 'backlink detected');
      this.invalidateCacheForNode(backlinkEvent.targetNodeId, 'backlink target');
    });
  }

  // ========================================================================
  // Cache Registration and Management
  // ========================================================================

  /**
   * Register a cache entry for coordination
   */
  public registerCache(cacheEntry: Omit<CacheEntry, 'hitCount' | 'lastUpdate'>): void {
    const entry: CacheEntry = {
      ...cacheEntry,
      hitCount: 0,
      lastUpdate: Date.now(),
      dependencies: new Set(cacheEntry.dependencies || [])
    };

    this.cacheRegistry.set(entry.key, entry);
    this.metrics.totalEntries++;

    // Set up dependencies if any
    if (entry.dependencies.size > 0) {
      this.setupCacheDependencies(entry);
    }
  }

  /**
   * Unregister a cache entry
   */
  public unregisterCache(key: string): void {
    const entry = this.cacheRegistry.get(key);
    if (entry) {
      this.cacheRegistry.delete(key);
      this.dependencies.delete(key);
      this.metrics.totalEntries--;
    }
  }

  /**
   * Record cache hit
   */
  public recordCacheHit(key: string): void {
    const entry = this.cacheRegistry.get(key);
    if (entry) {
      entry.hitCount++;
      entry.lastUpdate = Date.now();
      this.metrics.totalHits++;
      this.updateHitRate();
    }
  }

  /**
   * Record cache miss
   */
  public recordCacheMiss(_key: string): void {
    this.metrics.totalMisses++;
    this.updateHitRate();
  }

  /**
   * Set up cache dependencies
   */
  public addCacheDependency(dependency: CacheDependency): void {
    this.dependencies.set(dependency.key, dependency);
  }

  /**
   * Add invalidation strategy
   */
  public addInvalidationStrategy(strategy: CacheInvalidationStrategy): void {
    this.invalidationStrategies.push(strategy);
    this.invalidationStrategies.sort((a, b) => b.priority - a.priority);
  }

  // ========================================================================
  // Cache Invalidation
  // ========================================================================

  /**
   * Invalidate cache for a specific node
   */
  public invalidateCacheForNode(nodeId: string, reason: string = 'node change'): void {
    const keysToInvalidate = new Set<string>();

    // Find all cache entries for this node
    for (const [key, entry] of this.cacheRegistry) {
      if (entry.nodeId === nodeId) {
        keysToInvalidate.add(key);
      }

      // Check dependencies
      if (entry.dependencies.has(nodeId)) {
        keysToInvalidate.add(key);
      }
    }

    // Process invalidations
    for (const key of keysToInvalidate) {
      this.scheduleInvalidation(key, reason);
    }
  }

  /**
   * Invalidate specific cache key
   */
  public invalidateCache(key: string, reason: string = 'explicit invalidation'): void {
    this.scheduleInvalidation(key, reason);
  }

  /**
   * Invalidate all cache entries
   */
  public invalidateAllCache(reason: string = 'global invalidation'): void {
    for (const key of this.cacheRegistry.keys()) {
      this.scheduleInvalidation(key, reason);
    }
  }

  // ========================================================================
  // Event Handlers
  // ========================================================================

  private handleCacheInvalidation(event: CacheInvalidateEvent): void {
    switch (event.scope) {
      case 'single':
        this.invalidateCache(event.cacheKey, event.reason);
        break;
      case 'node':
        if (event.nodeId) {
          this.invalidateCacheForNode(event.nodeId, event.reason);
        }
        break;
      case 'global':
        this.invalidateAllCache(event.reason);
        break;
    }
  }

  private handleNodeLifecycleEvent(
    event: NodeCreatedEvent | NodeUpdatedEvent | NodeDeletedEvent
  ): void {
    // Apply invalidation strategies
    for (const strategy of this.invalidationStrategies) {
      for (const [key, entry] of this.cacheRegistry) {
        if (strategy.condition(event, entry)) {
          if (strategy.batchable) {
            this.scheduleInvalidation(key, `strategy: ${strategy.name}`);
          } else {
            this.executeInvalidation(key, `strategy: ${strategy.name}`);
          }
        }
      }
    }
  }

  private handleHierarchyChanged(event: HierarchyChangedEvent): void {
    // Invalidate cache for all affected nodes
    for (const nodeId of event.affectedNodes) {
      this.invalidateCacheForNode(nodeId, `hierarchy changed: ${event.changeType}`);
    }
  }

  // ========================================================================
  // Invalidation Scheduling and Execution
  // ========================================================================

  private scheduleInvalidation(key: string, _reason: string): void {
    this.pendingInvalidations.add(key);

    // Set up batch processing
    if (!this.batchTimeout) {
      this.batchTimeout = setTimeout(() => {
        this.executeBatchInvalidation();
      }, this.BATCH_DELAY_MS);
    }
  }

  private executeBatchInvalidation(): void {
    if (this.pendingInvalidations.size === 0) return;

    const keysToInvalidate = Array.from(this.pendingInvalidations);
    this.pendingInvalidations.clear();

    if (this.batchTimeout) {
      clearTimeout(this.batchTimeout);
      this.batchTimeout = undefined;
    }

    // Process all invalidations
    for (const key of keysToInvalidate) {
      this.executeInvalidation(key, 'batch invalidation');
    }

    // Emit batch invalidation complete event
    const debugEvent: import('./EventTypes').DebugEvent = {
      type: 'debug:log',
      namespace: 'debug',
      source: this.serviceName,
      timestamp: Date.now(),
      level: 'debug',
      message: `Batch invalidated ${keysToInvalidate.length} cache entries`,
      metadata: { keys: keysToInvalidate }
    };
    eventBus.emit(debugEvent);
  }

  private executeInvalidation(key: string, reason: string): void {
    const entry = this.cacheRegistry.get(key);
    if (!entry) return;

    // Mark as invalidated
    entry.lastUpdate = 0; // Mark as stale
    this.metrics.invalidationCount++;

    // Emit cache invalidation event for dependent services
    const cacheEvent: import('./EventTypes').CacheInvalidateEvent = {
      type: 'cache:invalidate',
      namespace: 'coordination',
      source: this.serviceName,
      timestamp: Date.now(),
      cacheKey: key,
      scope: 'single',
      nodeId: entry.nodeId,
      reason,
      metadata: { originalEntry: entry }
    };
    eventBus.emit(cacheEvent);

    // Handle cascading invalidations
    this.handleCascadingInvalidation(key, reason);
  }

  private handleCascadingInvalidation(invalidatedKey: string, reason: string): void {
    // Find dependencies that depend on this key
    for (const [key, dependency] of this.dependencies) {
      if (dependency.dependsOn.includes(invalidatedKey)) {
        if (dependency.invalidationStrategy === 'immediate') {
          this.executeInvalidation(key, `cascading: ${reason}`);
        } else if (dependency.invalidationStrategy === 'batch') {
          this.scheduleInvalidation(key, `cascading: ${reason}`);
        }
        // 'lazy' strategy does nothing - will be invalidated on next access
      }
    }
  }

  // ========================================================================
  // Default Invalidation Strategies
  // ========================================================================

  private setupDefaultInvalidationStrategies(): void {
    // Content change strategy
    this.addInvalidationStrategy({
      name: 'content-change',
      condition: (event, cache) => {
        const typedEvent = event as any;
        return (
          typedEvent.type === 'node:updated' &&
          typedEvent.updateType === 'content' &&
          cache.nodeId === typedEvent.nodeId
        );
      },
      priority: 100,
      batchable: false
    });

    // Hierarchy change strategy
    this.addInvalidationStrategy({
      name: 'hierarchy-change',
      condition: (event, cache) => {
        const typedEvent = event as any;
        return (
          typedEvent.type === 'hierarchy:changed' &&
          cache.nodeId &&
          typedEvent.affectedNodes?.includes(cache.nodeId)
        );
      },
      priority: 90,
      batchable: true
    });

    // Node deletion strategy
    this.addInvalidationStrategy({
      name: 'node-deletion',
      condition: (event, cache) => {
        const typedEvent = event as any;
        return (
          typedEvent.type === 'node:deleted' &&
          (cache.nodeId === typedEvent.nodeId || cache.dependencies.has(typedEvent.nodeId))
        );
      },
      priority: 95,
      batchable: false
    });

    // Reference update strategy
    this.addInvalidationStrategy({
      name: 'reference-update',
      condition: (event, cache) => {
        const typedEvent = event as any;
        return typedEvent.type === 'references:update-needed' && cache.nodeId === typedEvent.nodeId;
      },
      priority: 80,
      batchable: true
    });
  }

  // ========================================================================
  // Cache Maintenance
  // ========================================================================

  private startCleanupTimer(): void {
    globalThis.setInterval(() => {
      this.performCacheCleanup();
    }, this.CLEANUP_INTERVAL_MS);
  }

  private performCacheCleanup(): void {
    const now = Date.now();
    const keysToRemove: string[] = [];

    // Find stale cache entries
    for (const [key, entry] of this.cacheRegistry) {
      if (now - entry.lastUpdate > this.MAX_CACHE_AGE_MS) {
        keysToRemove.push(key);
      }
    }

    // Remove stale entries
    for (const key of keysToRemove) {
      this.unregisterCache(key);
    }

    this.metrics.lastCleanup = now;

    if (keysToRemove.length > 0) {
      const debugEvent: import('./EventTypes').DebugEvent = {
        type: 'debug:log',
        namespace: 'debug',
        source: this.serviceName,
        timestamp: Date.now(),
        level: 'debug',
        message: `Cache cleanup removed ${keysToRemove.length} stale entries`,
        metadata: { removedKeys: keysToRemove }
      };
      eventBus.emit(debugEvent);
    }
  }

  // ========================================================================
  // Cache Dependencies Setup
  // ========================================================================

  private setupCacheDependencies(entry: CacheEntry): void {
    // Create dependency mapping
    const dependency: CacheDependency = {
      key: entry.key,
      dependsOn: Array.from(entry.dependencies),
      invalidationStrategy: 'batch' // Default strategy
    };

    this.addCacheDependency(dependency);
  }

  // ========================================================================
  // Metrics and Utilities
  // ========================================================================

  private updateHitRate(): void {
    const total = this.metrics.totalHits + this.metrics.totalMisses;
    this.metrics.averageHitRate = total > 0 ? (this.metrics.totalHits / total) * 100 : 0;
  }

  /**
   * Get cache metrics
   */
  public getMetrics(): CacheMetrics {
    return { ...this.metrics };
  }

  /**
   * Get cache entry info
   */
  public getCacheInfo(key: string): CacheEntry | null {
    return this.cacheRegistry.get(key) || null;
  }

  /**
   * Get all cache entries for a node
   */
  public getNodeCacheEntries(nodeId: string): CacheEntry[] {
    const entries: CacheEntry[] = [];

    for (const entry of this.cacheRegistry.values()) {
      if (entry.nodeId === nodeId) {
        entries.push(entry);
      }
    }

    return entries;
  }

  /**
   * Clear all pending invalidations
   */
  public flushPendingInvalidations(): void {
    if (this.batchTimeout) {
      clearTimeout(this.batchTimeout);
      this.batchTimeout = undefined;
    }
    this.executeBatchInvalidation();
  }

  /**
   * Reset metrics
   */
  public resetMetrics(): void {
    this.metrics = {
      totalEntries: this.cacheRegistry.size,
      totalHits: 0,
      totalMisses: 0,
      invalidationCount: 0,
      averageHitRate: 0,
      lastCleanup: Date.now()
    };
  }

  /**
   * Clean up all resources
   */
  public cleanup(): void {
    if (this.batchTimeout) {
      clearTimeout(this.batchTimeout);
    }

    this.cacheRegistry.clear();
    this.dependencies.clear();
    this.pendingInvalidations.clear();
    this.invalidationStrategies.length = 0;
    this.resetMetrics();
  }
}

// ============================================================================
// Singleton Export
// ============================================================================

export const cacheCoordinator = CacheCoordinator.getInstance();

export default CacheCoordinator;
