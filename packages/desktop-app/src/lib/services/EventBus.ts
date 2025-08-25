/**
 * EventBus - Type-safe Event Bus for Dynamic Coordination
 *
 * High-performance, type-safe EventBus for coordinating between NodeSpace services.
 * Designed to complement, not replace, the existing decorateReference() system.
 *
 * Key Features:
 * - Type-safe event handling with full TypeScript support
 * - Namespace separation to prevent collisions
 * - Performance optimizations (debouncing, batching)
 * - Memory leak prevention
 * - Comprehensive error handling
 * - Foundation for Phase 2+ features
 */

import type {
  NodeSpaceEvent,
  EventHandler,
  EventFilter,
  EventSubscriptionOptions,
  BatchedEvent,
  BatchingConfig
} from './EventTypes';

// ============================================================================
// Event Bus Implementation
// ============================================================================

export class EventBus {
  private static instance: EventBus;
  private subscribers = new Map<string, Set<EventSubscription>>();
  private wildcardSubscribers = new Set<EventSubscription>();
  private eventHistory: NodeSpaceEvent[] = [];
  private batchingConfig: BatchingConfig;
  private pendingBatches = new Map<string, NodeSpaceEvent[]>();
  private batchTimeouts = new Map<string, NodeJS.Timeout>();
  private debounceTimeouts = new Map<string, NodeJS.Timeout>();
  private maxHistorySize = 1000;
  private isEnabled = true;
  private performanceMetrics = {
    totalEvents: 0,
    totalHandlers: 0,
    averageProcessingTime: 0,
    errorCount: 0
  };

  constructor() {
    this.batchingConfig = {
      maxBatchSize: 10,
      timeWindowMs: 16, // ~60 FPS
      enableForTypes: [
        // Batching is disabled by default for testing
        // Can be enabled via configureBatching() method
      ]
    };
  }

  public static getInstance(): EventBus {
    if (!EventBus.instance) {
      EventBus.instance = new EventBus();
    }
    return EventBus.instance;
  }

  // ========================================================================
  // Core Event Publishing
  // ========================================================================

  /**
   * Emit an event to all matching subscribers
   */
  public emit<T extends NodeSpaceEvent>(event: Omit<T, 'timestamp'>): void {
    if (!this.isEnabled) return;

    const startTime = performance.now();

    // Add timestamp if not present
    const fullEvent: T = {
      ...event,
      timestamp: Date.now()
    } as T;

    this.performanceMetrics.totalEvents++;

    try {
      // Add to history for debugging
      this.addToHistory(fullEvent);

      // Check if this event type should be batched
      if (this.shouldBatch(fullEvent.type)) {
        this.handleBatchedEvent(fullEvent);
        return;
      }

      // Process immediately
      this.processEvent(fullEvent);
    } catch (error) {
      this.performanceMetrics.errorCount++;
      console.error('EventBus: Error emitting event', { event, error });
    } finally {
      // Update performance metrics
      const processingTime = performance.now() - startTime;
      this.updateProcessingTime(processingTime);
    }
  }

  /**
   * Emit multiple events in sequence
   */
  public emitBatch(events: Omit<NodeSpaceEvent, 'timestamp'>[]): void {
    for (const event of events) {
      this.emit(event);
    }
  }

  // ========================================================================
  // Event Subscription
  // ========================================================================

  /**
   * Subscribe to events with optional filtering and options
   */
  public subscribe<T extends NodeSpaceEvent>(
    eventType: T['type'] | '*',
    handler: EventHandler<T>,
    options: EventSubscriptionOptions = {}
  ): () => void {
    const subscription: EventSubscription = {
      id: this.generateSubscriptionId(),
      eventType,
      handler: handler as EventHandler,
      options,
      createdAt: Date.now(),
      callCount: 0,
      lastCalled: 0
    };

    this.performanceMetrics.totalHandlers++;

    // Handle wildcard subscriptions
    if (eventType === '*') {
      this.wildcardSubscribers.add(subscription);
    } else {
      if (!this.subscribers.has(eventType)) {
        this.subscribers.set(eventType, new Set());
      }
      this.subscribers.get(eventType)!.add(subscription);
    }

    // Return unsubscribe function
    return () => this.unsubscribe(subscription.id);
  }

  /**
   * Subscribe to multiple event types with same handler
   */
  public subscribeMultiple(
    eventTypes: string[],
    handler: EventHandler,
    options: EventSubscriptionOptions = {}
  ): () => void {
    const unsubscribers = eventTypes.map((type) =>
      this.subscribe(type as NodeSpaceEvent['type'], handler, options)
    );

    return () => {
      unsubscribers.forEach((unsub) => unsub());
    };
  }

  /**
   * Subscribe once - automatically unsubscribes after first event
   */
  public once<T extends NodeSpaceEvent>(
    eventType: T['type'],
    handler: EventHandler<T>
  ): Promise<T> {
    return new Promise((resolve) => {
      this.subscribe(
        eventType,
        (event) => {
          handler(event);
          resolve(event);
        },
        { once: true }
      );
    });
  }

  // ========================================================================
  // Event Filtering and Querying
  // ========================================================================

  /**
   * Get recent events matching filter
   */
  public getRecentEvents(filter?: EventFilter, limit: number = 50): NodeSpaceEvent[] {
    let events = [...this.eventHistory].reverse();

    if (filter) {
      events = events.filter((event) => this.matchesFilter(event, filter));
    }

    return events.slice(0, limit);
  }

  /**
   * Wait for specific event matching criteria
   */
  public waitFor<T extends NodeSpaceEvent>(
    eventType: T['type'],
    filter?: EventFilter,
    timeoutMs: number = 5000
  ): Promise<T> {
    return new Promise((resolve, reject) => {
      const timeoutId = setTimeout(() => {
        reject(new Error(`EventBus: Timeout waiting for event ${eventType}`));
      }, timeoutMs);

      const unsubscribe = this.subscribe(eventType, (event) => {
        if (!filter || this.matchesFilter(event, filter)) {
          clearTimeout(timeoutId);
          unsubscribe();
          resolve(event);
        }
      });
    });
  }

  // ========================================================================
  // Performance and Configuration
  // ========================================================================

  /**
   * Configure batching settings
   */
  public configureBatching(config: Partial<BatchingConfig>): void {
    this.batchingConfig = { ...this.batchingConfig, ...config };
  }

  /**
   * Enable/disable the event bus
   */
  public setEnabled(enabled: boolean): void {
    this.isEnabled = enabled;
  }

  /**
   * Get performance metrics
   */
  public getMetrics(): typeof this.performanceMetrics {
    return { ...this.performanceMetrics };
  }

  /**
   * Clear event history and reset metrics
   */
  public reset(): void {
    this.eventHistory.length = 0;

    // Clear all subscribers
    this.subscribers.clear();
    this.wildcardSubscribers.clear();

    this.performanceMetrics = {
      totalEvents: 0,
      totalHandlers: 0,
      averageProcessingTime: 0,
      errorCount: 0
    };

    // Clear all pending batches
    for (const timeoutId of this.batchTimeouts.values()) {
      clearTimeout(timeoutId);
    }
    this.pendingBatches.clear();
    this.batchTimeouts.clear();

    // Clear all debounce timeouts
    for (const timeoutId of this.debounceTimeouts.values()) {
      clearTimeout(timeoutId);
    }
    this.debounceTimeouts.clear();
  }

  // ========================================================================
  // Debug and Introspection
  // ========================================================================

  /**
   * Get current subscribers count by event type
   */
  public getSubscriberCounts(): Record<string, number> {
    const counts: Record<string, number> = {};

    for (const [eventType, subscribers] of this.subscribers) {
      counts[eventType] = subscribers.size;
    }

    counts['*'] = this.wildcardSubscribers.size;

    return counts;
  }

  /**
   * Get detailed subscription information
   */
  public getSubscriptionDetails(): SubscriptionDetails[] {
    const details: SubscriptionDetails[] = [];

    for (const [eventType, subscribers] of this.subscribers) {
      for (const sub of subscribers) {
        details.push({
          id: sub.id,
          eventType,
          createdAt: sub.createdAt,
          callCount: sub.callCount,
          lastCalled: sub.lastCalled,
          options: sub.options
        });
      }
    }

    return details;
  }

  // ========================================================================
  // Private Implementation
  // ========================================================================

  private processEvent(event: NodeSpaceEvent): void {
    const matchingSubscriptions = new Set<EventSubscription>();

    // Get direct type subscribers
    const typeSubscribers = this.subscribers.get(event.type);
    if (typeSubscribers) {
      for (const sub of typeSubscribers) {
        matchingSubscriptions.add(sub);
      }
    }

    // Add wildcard subscribers
    for (const sub of this.wildcardSubscribers) {
      matchingSubscriptions.add(sub);
    }

    // Process all matching subscriptions
    const subscriptionsToRemove: EventSubscription[] = [];

    for (const subscription of matchingSubscriptions) {
      try {
        // Apply filters
        if (!this.shouldHandleEvent(event, subscription)) {
          continue;
        }

        // Apply debouncing if configured
        if (subscription.options.debounceMs) {
          this.handleDebouncedEvent(event, subscription);
          continue;
        }

        // Execute handler
        this.executeHandler(event, subscription);

        // Handle once subscriptions
        if (subscription.options.once) {
          subscriptionsToRemove.push(subscription);
        }
      } catch (error) {
        console.error('EventBus: Error in event handler', {
          eventType: event.type,
          subscriptionId: subscription.id,
          error
        });
      }
    }

    // Remove once subscriptions
    for (const sub of subscriptionsToRemove) {
      this.removeSubscription(sub);
    }
  }

  private executeHandler(event: NodeSpaceEvent, subscription: EventSubscription): void {
    subscription.callCount++;
    subscription.lastCalled = Date.now();

    const result = subscription.handler(event);

    // Handle async handlers
    if (result instanceof Promise) {
      result.catch((error) => {
        console.error('EventBus: Async handler error', {
          eventType: event.type,
          subscriptionId: subscription.id,
          error
        });
      });
    }
  }

  private shouldHandleEvent(event: NodeSpaceEvent, subscription: EventSubscription): boolean {
    if (!subscription.options.filter) return true;
    return this.matchesFilter(event, subscription.options.filter);
  }

  private matchesFilter(event: NodeSpaceEvent, filter: EventFilter): boolean {
    // Type filter
    if (filter.type) {
      const types = Array.isArray(filter.type) ? filter.type : [filter.type];
      if (!types.includes(event.type)) return false;
    }

    // Namespace filter
    if (filter.namespace) {
      const namespaces = Array.isArray(filter.namespace) ? filter.namespace : [filter.namespace];
      if (!namespaces.includes(event.namespace)) return false;
    }

    // Source filter
    if (filter.source) {
      const sources = Array.isArray(filter.source) ? filter.source : [filter.source];
      if (!sources.includes(event.source)) return false;
    }

    // Node ID filter
    if (filter.nodeId && 'nodeId' in event) {
      if (event.nodeId !== filter.nodeId) return false;
    }

    // User ID filter
    if (filter.userId && 'userId' in event) {
      if (event.userId !== filter.userId) return false;
    }

    return true;
  }

  private shouldBatch(eventType: string): boolean {
    return this.batchingConfig.enableForTypes.includes(eventType);
  }

  private handleBatchedEvent(event: NodeSpaceEvent): void {
    const batchKey = `${event.type}:${event.namespace}`;

    if (!this.pendingBatches.has(batchKey)) {
      this.pendingBatches.set(batchKey, []);
    }

    const batch = this.pendingBatches.get(batchKey)!;
    batch.push(event);

    // Check if batch is full
    if (batch.length >= this.batchingConfig.maxBatchSize) {
      this.processBatch(batchKey);
      return;
    }

    // Set or reset timeout for this batch
    if (this.batchTimeouts.has(batchKey)) {
      clearTimeout(this.batchTimeouts.get(batchKey)!);
    }

    const timeoutId = setTimeout(() => {
      this.processBatch(batchKey);
    }, this.batchingConfig.timeWindowMs);

    this.batchTimeouts.set(batchKey, timeoutId);
  }

  private processBatch(batchKey: string): void {
    const events = this.pendingBatches.get(batchKey);
    if (!events || events.length === 0) return;

    // Clear timeout and batch
    const timeoutId = this.batchTimeouts.get(batchKey);
    if (timeoutId) {
      clearTimeout(timeoutId);
      this.batchTimeouts.delete(batchKey);
    }
    this.pendingBatches.delete(batchKey);

    // Create batched event
    const batchedEvent: BatchedEvent = {
      events: [...events],
      batchId: this.generateBatchId(),
      batchSize: events.length,
      timeWindow: this.batchingConfig.timeWindowMs
    };

    // Process each event in the batch
    for (const event of events) {
      this.processEvent(event);
    }

    // Emit batch completed event for subscribers who care about batching
    const debugEvent: import('./EventTypes').DebugEvent = {
      type: 'debug:log',
      namespace: 'debug',
      source: 'EventBus',
      timestamp: Date.now(),
      level: 'debug',
      message: `Processed batch ${batchKey} with ${events.length} events`,
      metadata: { batchedEvent }
    };
    this.emit(debugEvent);
  }

  private handleDebouncedEvent(event: NodeSpaceEvent, subscription: EventSubscription): void {
    const debounceKey = `${subscription.id}:${event.type}`;

    // Clear existing timeout
    if (this.debounceTimeouts.has(debounceKey)) {
      clearTimeout(this.debounceTimeouts.get(debounceKey)!);
    }

    // Set new timeout
    const timeoutId = setTimeout(() => {
      this.debounceTimeouts.delete(debounceKey);
      this.executeHandler(event, subscription);
    }, subscription.options.debounceMs!);

    this.debounceTimeouts.set(debounceKey, timeoutId);
  }

  private unsubscribe(subscriptionId: string): void {
    let found = false;

    // Check type-specific subscribers
    for (const [eventType, subscribers] of this.subscribers) {
      for (const sub of subscribers) {
        if (sub.id === subscriptionId) {
          subscribers.delete(sub);
          if (subscribers.size === 0) {
            this.subscribers.delete(eventType);
          }
          found = true;
          break;
        }
      }
      if (found) break;
    }

    // Check wildcard subscribers
    if (!found) {
      for (const sub of this.wildcardSubscribers) {
        if (sub.id === subscriptionId) {
          this.wildcardSubscribers.delete(sub);
          found = true;
          break;
        }
      }
    }

    if (found) {
      this.performanceMetrics.totalHandlers--;
    }
  }

  private removeSubscription(subscription: EventSubscription): void {
    this.unsubscribe(subscription.id);
  }

  private addToHistory(event: NodeSpaceEvent): void {
    this.eventHistory.push(event);

    // Trim history if too large
    if (this.eventHistory.length > this.maxHistorySize) {
      this.eventHistory.shift();
    }
  }

  private updateProcessingTime(processingTime: number): void {
    const total = this.performanceMetrics.totalEvents;
    const currentAvg = this.performanceMetrics.averageProcessingTime;
    this.performanceMetrics.averageProcessingTime =
      (currentAvg * (total - 1) + processingTime) / total;
  }

  private generateSubscriptionId(): string {
    return `sub_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
  }

  private generateBatchId(): string {
    return `batch_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
  }
}

// ============================================================================
// Supporting Types
// ============================================================================

interface EventSubscription {
  id: string;
  eventType: string;
  handler: EventHandler;
  options: EventSubscriptionOptions;
  createdAt: number;
  callCount: number;
  lastCalled: number;
}

interface SubscriptionDetails {
  id: string;
  eventType: string;
  createdAt: number;
  callCount: number;
  lastCalled: number;
  options: EventSubscriptionOptions;
}

// ============================================================================
// Singleton Export
// ============================================================================

/**
 * Singleton instance for application-wide use
 */
export const eventBus = EventBus.getInstance();

// ============================================================================
// Default Export
// ============================================================================

export default EventBus;
