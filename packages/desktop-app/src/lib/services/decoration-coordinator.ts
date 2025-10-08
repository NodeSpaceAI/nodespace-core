/**
 * DecorationCoordinator - Interactive Decoration Handling Service
 *
 * Coordinates interactive decoration events for the NodeSpace reference system.
 * Builds on the existing decorateReference() architecture to provide dynamic
 * click handling and real-time updates.
 *
 * Key Features:
 * - Interactive click handling for decorations
 * - Real-time decoration updates based on node status changes
 * - Hover state management
 * - Foundation for advanced interaction patterns
 */

import { eventBus } from './event-bus';
import type {
  DecorationClickedEvent,
  DecorationHoverEvent,
  NodeStatusChangedEvent,
  CacheInvalidateEvent
} from './event-types';

// ============================================================================
// Types and Interfaces
// ============================================================================

export interface DecorationInfo {
  nodeId: string;
  decorationType: string;
  target: string;
  element: HTMLElement;
  isActive: boolean;
  lastUpdate: number;
  metadata?: Record<string, unknown>;
}

export interface DecorationClickHandler {
  decorationType: string;
  handler: (event: DecorationClickedEvent) => void | Promise<void>;
  priority?: number;
}

export interface DecorationHoverHandler {
  decorationType: string;
  handler: (event: DecorationHoverEvent) => void | Promise<void>;
  priority?: number;
}

// ============================================================================
// DecorationCoordinator Service
// ============================================================================

export class DecorationCoordinator {
  private static instance: DecorationCoordinator;
  private readonly serviceName = 'DecorationCoordinator';

  private activeDecorations = new Map<string, DecorationInfo>();
  private clickHandlers: DecorationClickHandler[] = [];
  private hoverHandlers: DecorationHoverHandler[] = [];
  private hoverTimeouts = new Map<string, NodeJS.Timeout>();

  public static getInstance(): DecorationCoordinator {
    if (!DecorationCoordinator.instance) {
      DecorationCoordinator.instance = new DecorationCoordinator();
    }
    return DecorationCoordinator.instance;
  }

  private constructor() {
    this.setupEventBusIntegration();
  }

  // ========================================================================
  // Event Bus Integration
  // ========================================================================

  private setupEventBusIntegration(): void {
    // Listen for decoration update needed events
    eventBus.subscribe('decoration:update-needed', (event) => {
      this.handleDecorationUpdateNeeded(event as { nodeId: string; decorationType: string });
    });

    // Listen for node status changes to update decorations
    eventBus.subscribe('node:status-changed', (event) => {
      this.handleNodeStatusChanged(event as import('./event-types').NodeStatusChangedEvent);
    });

    // Listen for cache invalidation to refresh decorations
    eventBus.subscribe('cache:invalidate', (event) => {
      this.handleCacheInvalidation(event as import('./event-types').CacheInvalidateEvent);
    });

    // Listen for decoration click events
    eventBus.subscribe('decoration:clicked', (event) => {
      this.handleDecorationClicked(event as import('./event-types').DecorationClickedEvent);
    });

    // Listen for decoration hover events
    eventBus.subscribe('decoration:hover', (event) => {
      this.handleDecorationHover(event as import('./event-types').DecorationHoverEvent);
    });
  }

  // ========================================================================
  // Decoration Management
  // ========================================================================

  /**
   * Register a decoration for coordination
   */
  public registerDecoration(decorationInfo: DecorationInfo): void {
    const key = this.getDecorationKey(decorationInfo.nodeId, decorationInfo.target);
    this.activeDecorations.set(key, {
      ...decorationInfo,
      lastUpdate: Date.now()
    });

    // Set up click listeners
    this.setupDecorationListeners(decorationInfo);
  }

  /**
   * Unregister a decoration
   */
  public unregisterDecoration(nodeId: string, target: string): void {
    const key = this.getDecorationKey(nodeId, target);
    const decoration = this.activeDecorations.get(key);

    if (decoration) {
      this.cleanupDecorationListeners(decoration);
      this.activeDecorations.delete(key);
    }
  }

  /**
   * Update decoration status
   */
  public updateDecoration(nodeId: string, target: string, updates: Partial<DecorationInfo>): void {
    const key = this.getDecorationKey(nodeId, target);
    const decoration = this.activeDecorations.get(key);

    if (decoration) {
      Object.assign(decoration, updates, { lastUpdate: Date.now() });
      this.refreshDecorationDisplay(decoration);
    }
  }

  /**
   * Get all active decorations for a node
   */
  public getNodeDecorations(nodeId: string): DecorationInfo[] {
    const decorations: DecorationInfo[] = [];

    for (const decoration of this.activeDecorations.values()) {
      if (decoration.nodeId === nodeId) {
        decorations.push(decoration);
      }
    }

    return decorations;
  }

  // ========================================================================
  // Click and Hover Handler Registration
  // ========================================================================

  /**
   * Register a click handler for specific decoration types
   */
  public registerClickHandler(handler: DecorationClickHandler): () => void {
    this.clickHandlers.push(handler);
    this.clickHandlers.sort((a, b) => (b.priority || 0) - (a.priority || 0));

    return () => {
      const index = this.clickHandlers.indexOf(handler);
      if (index !== -1) {
        this.clickHandlers.splice(index, 1);
      }
    };
  }

  /**
   * Register a hover handler for specific decoration types
   */
  public registerHoverHandler(handler: DecorationHoverHandler): () => void {
    this.hoverHandlers.push(handler);
    this.hoverHandlers.sort((a, b) => (b.priority || 0) - (a.priority || 0));

    return () => {
      const index = this.hoverHandlers.indexOf(handler);
      if (index !== -1) {
        this.hoverHandlers.splice(index, 1);
      }
    };
  }

  // ========================================================================
  // Event Handlers
  // ========================================================================

  private handleDecorationUpdateNeeded(event: { nodeId: string; decorationType: string }): void {
    const decorations = this.getNodeDecorations(event.nodeId);

    for (const decoration of decorations) {
      if (decoration.decorationType === event.decorationType || event.decorationType === 'all') {
        this.refreshDecorationDisplay(decoration);
      }
    }
  }

  private handleNodeStatusChanged(event: NodeStatusChangedEvent): void {
    const decorations = this.getNodeDecorations(event.nodeId);

    for (const decoration of decorations) {
      // Update decoration based on node status
      this.updateDecorationForStatus(decoration, event.status);
    }
  }

  private handleCacheInvalidation(event: CacheInvalidateEvent): void {
    if (event.scope === 'global') {
      // Refresh all decorations
      for (const decoration of this.activeDecorations.values()) {
        this.refreshDecorationDisplay(decoration);
      }
    } else if (event.scope === 'node' && event.nodeId) {
      // Refresh decorations for specific node
      const decorations = this.getNodeDecorations(event.nodeId);
      for (const decoration of decorations) {
        this.refreshDecorationDisplay(decoration);
      }
    }
  }

  private handleDecorationClicked(event: DecorationClickedEvent): void {
    for (const handler of this.clickHandlers) {
      if (handler.decorationType === event.decorationType || handler.decorationType === '*') {
        try {
          const result = handler.handler(event);
          if (result instanceof Promise) {
            result.catch((error) => {
              console.error('DecorationCoordinator: Click handler error', {
                handler,
                event,
                error
              });
            });
          }
        } catch (error) {
          console.error('DecorationCoordinator: Click handler error', { handler, event, error });
        }
      }
    }
  }

  private handleDecorationHover(event: DecorationHoverEvent): void {
    const key = this.getDecorationKey(event.nodeId, event.target);

    // Handle hover timeout for leave events
    if (event.hoverState === 'leave') {
      const timeoutId = setTimeout(() => {
        this.processHoverEvent(event);
        this.hoverTimeouts.delete(key);
      }, 100); // Small delay to handle quick mouse movements

      // Clear any existing timeout
      if (this.hoverTimeouts.has(key)) {
        clearTimeout(this.hoverTimeouts.get(key)!);
      }
      this.hoverTimeouts.set(key, timeoutId);
    } else {
      // Clear leave timeout on enter
      if (this.hoverTimeouts.has(key)) {
        clearTimeout(this.hoverTimeouts.get(key)!);
        this.hoverTimeouts.delete(key);
      }
      this.processHoverEvent(event);
    }
  }

  private processHoverEvent(event: DecorationHoverEvent): void {
    for (const handler of this.hoverHandlers) {
      if (handler.decorationType === event.decorationType || handler.decorationType === '*') {
        try {
          const result = handler.handler(event);
          if (result instanceof Promise) {
            result.catch((error) => {
              console.error('DecorationCoordinator: Hover handler error', {
                handler,
                event,
                error
              });
            });
          }
        } catch (error) {
          console.error('DecorationCoordinator: Hover handler error', { handler, event, error });
        }
      }
    }
  }

  // ========================================================================
  // Decoration Display and Interaction
  // ========================================================================

  private setupDecorationListeners(decoration: DecorationInfo): void {
    const element = decoration.element;

    // Click listener
    const clickHandler = (domEvent: MouseEvent) => {
      domEvent.preventDefault();
      domEvent.stopPropagation();

      const clickEvent: Omit<import('./event-types').DecorationClickedEvent, 'timestamp'> = {
        type: 'decoration:clicked',
        namespace: 'interaction',
        source: this.serviceName,
        nodeId: decoration.nodeId,
        decorationType: decoration.decorationType,
        target: decoration.target,
        clickPosition: { x: domEvent.clientX, y: domEvent.clientY },
        metadata: decoration.metadata
      };
      eventBus.emit(clickEvent);
    };

    // Hover listeners
    const mouseEnterHandler = (_domEvent: MouseEvent) => {
      const hoverEvent: Omit<import('./event-types').DecorationHoverEvent, 'timestamp'> = {
        type: 'decoration:hover',
        namespace: 'interaction',
        source: this.serviceName,
        nodeId: decoration.nodeId,
        decorationType: decoration.decorationType,
        target: decoration.target,
        hoverState: 'enter',
        metadata: decoration.metadata
      };
      eventBus.emit(hoverEvent);
    };

    const mouseLeaveHandler = (_domEvent: MouseEvent) => {
      const hoverEvent: Omit<import('./event-types').DecorationHoverEvent, 'timestamp'> = {
        type: 'decoration:hover',
        namespace: 'interaction',
        source: this.serviceName,
        nodeId: decoration.nodeId,
        decorationType: decoration.decorationType,
        target: decoration.target,
        hoverState: 'leave',
        metadata: decoration.metadata
      };
      eventBus.emit(hoverEvent);
    };

    // Attach listeners
    element.addEventListener('click', clickHandler);
    element.addEventListener('mouseenter', mouseEnterHandler);
    element.addEventListener('mouseleave', mouseLeaveHandler);

    // Store handlers for cleanup
    (element as unknown as { _decorationHandlers: unknown })._decorationHandlers = {
      click: clickHandler,
      mouseenter: mouseEnterHandler,
      mouseleave: mouseLeaveHandler
    };
  }

  private cleanupDecorationListeners(decoration: DecorationInfo): void {
    const element = decoration.element;
    const elementWithHandlers = element as unknown as {
      _decorationHandlers?: {
        click: (event: Event) => void;
        mouseenter: (event: Event) => void;
        mouseleave: (event: Event) => void;
      };
    };
    const handlers = elementWithHandlers._decorationHandlers;

    if (handlers) {
      element.removeEventListener('click', handlers.click);
      element.removeEventListener('mouseenter', handlers.mouseenter);
      element.removeEventListener('mouseleave', handlers.mouseleave);
      delete elementWithHandlers._decorationHandlers;
    }
  }

  private refreshDecorationDisplay(decoration: DecorationInfo): void {
    // Update decoration visual state based on current status
    const element = decoration.element;

    // Add/remove CSS classes based on status
    element.classList.toggle('ns-decoration-active', decoration.isActive);
    element.classList.add('ns-decoration-updated');

    // Remove update class after animation
    setTimeout(() => {
      element.classList.remove('ns-decoration-updated');
    }, 200);

    decoration.lastUpdate = Date.now();
  }

  private updateDecorationForStatus(decoration: DecorationInfo, status: string): void {
    // Update decoration based on node status
    switch (status) {
      case 'focused':
      case 'editing':
        decoration.isActive = true;
        break;
      case 'active':
        decoration.isActive = true;
        break;
      default:
        decoration.isActive = false;
        break;
    }

    this.refreshDecorationDisplay(decoration);
  }

  // ========================================================================
  // Utility Methods
  // ========================================================================

  private getDecorationKey(nodeId: string, target: string): string {
    return `${nodeId}:${target}`;
  }

  /**
   * Get performance metrics
   */
  public getMetrics(): {
    activeDecorations: number;
    clickHandlers: number;
    hoverHandlers: number;
    pendingHovers: number;
  } {
    return {
      activeDecorations: this.activeDecorations.size,
      clickHandlers: this.clickHandlers.length,
      hoverHandlers: this.hoverHandlers.length,
      pendingHovers: this.hoverTimeouts.size
    };
  }

  /**
   * Clean up all resources
   */
  public cleanup(): void {
    // Clean up all decoration listeners
    for (const decoration of this.activeDecorations.values()) {
      this.cleanupDecorationListeners(decoration);
    }

    // Clear all timeouts
    for (const timeoutId of this.hoverTimeouts.values()) {
      clearTimeout(timeoutId);
    }

    // Clear collections
    this.activeDecorations.clear();
    this.clickHandlers.length = 0;
    this.hoverHandlers.length = 0;
    this.hoverTimeouts.clear();
  }
}

// ============================================================================
// Singleton Export
// ============================================================================

export const decorationCoordinator = DecorationCoordinator.getInstance();

export default DecorationCoordinator;
