/**
 * NodeReferenceRenderer - Performance-Optimized Reference Decoration System
 *
 * Coordinates the rendering of rich node reference decorations using the BaseNode
 * decoration system. Provides viewport-based optimization, accessibility support,
 * and integration with the existing NodeReferenceService and DecorationCoordinator.
 *
 * Key Features:
 * - Viewport-based processing for large documents
 * - XSS-safe content rendering with sanitization
 * - Performance-optimized caching and batch processing
 * - Full accessibility support with ARIA labels
 * - Integration with existing EventBus coordination
 * - Support for different display contexts (inline, popup, preview)
 */

import { eventBus } from './EventBus';
import { decorationCoordinator } from './DecorationCoordinator';
import { NodeDecoratorFactory } from './BaseNodeDecoration';
import type { DecorationContext, DecorationResult } from './BaseNodeDecoration';
import type { ComponentDecoration } from '../types/ComponentDecoration';
import type { NodeReferenceService } from './NodeReferenceService';
import type { NodeSpaceNode } from './MockDatabaseService';

// ============================================================================
// Core Types and Interfaces
// ============================================================================

export interface RenderContext {
  containerElement: HTMLElement;
  displayContext: 'inline' | 'popup' | 'preview';
  viewportOptimization: boolean;
  batchSize: number;
  debounceMs: number;
}

export interface RenderOptions {
  force?: boolean;
  skipCache?: boolean;
  targetNodes?: string[];
  async?: boolean;
}

export interface ViewportInfo {
  top: number;
  bottom: number;
  left: number;
  right: number;
}

export interface RenderMetrics {
  totalReferences: number;
  renderedReferences: number;
  viewportReferences: number;
  cacheMisses: number;
  renderTime: number;
  lastRender: number;
}

// ============================================================================
// NodeReferenceRenderer Service
// ============================================================================

export class NodeReferenceRenderer {
  private static instance: NodeReferenceRenderer;
  private readonly serviceName = 'NodeReferenceRenderer';

  private nodeReferenceService: NodeReferenceService;
  private decoratorFactory: NodeDecoratorFactory;
  private renderCache = new Map<
    string,
    { result: ComponentDecoration | DecorationResult; timestamp: number; element?: HTMLElement }
  >();
  private intersectionObserver: IntersectionObserver | null = null;
  private mutationObserver: MutationObserver | null = null;

  // Performance optimization
  private pendingRenders = new Set<string>();
  private renderQueue: Array<{ nodeId: string; element: HTMLElement; context: DecorationContext }> =
    [];
  private renderTimeout: number | null = null;
  private readonly cacheTimeout = 300000; // 5 minutes
  private readonly batchSize = 50;
  private readonly debounceMs = 100;

  // Metrics tracking
  private metrics: RenderMetrics = {
    totalReferences: 0,
    renderedReferences: 0,
    viewportReferences: 0,
    cacheMisses: 0,
    renderTime: 0,
    lastRender: 0
  };

  public static getInstance(nodeReferenceService?: NodeReferenceService): NodeReferenceRenderer {
    if (!NodeReferenceRenderer.instance && nodeReferenceService) {
      NodeReferenceRenderer.instance = new NodeReferenceRenderer(nodeReferenceService);
    }
    return NodeReferenceRenderer.instance;
  }

  private constructor(nodeReferenceService: NodeReferenceService) {
    this.nodeReferenceService = nodeReferenceService;
    this.decoratorFactory = new NodeDecoratorFactory(nodeReferenceService);

    this.setupEventBusIntegration();
    this.setupViewportOptimization();
    this.setupMutationObserving();
  }

  // ========================================================================
  // Public API
  // ========================================================================

  /**
   * Render all node references in a container
   */
  public async renderContainer(
    container: HTMLElement,
    context: Partial<RenderContext> = {},
    options: RenderOptions = {}
  ): Promise<void> {
    const startTime = performance.now();

    const renderContext: RenderContext = {
      containerElement: container,
      displayContext: 'inline',
      viewportOptimization: true,
      batchSize: this.batchSize,
      debounceMs: this.debounceMs,
      ...context
    };

    try {
      // Find all nodespace:// references in the container
      const references = this.findNodeReferences(container);
      this.metrics.totalReferences = references.length;

      if (renderContext.viewportOptimization) {
        // Only render references in viewport
        await this.renderViewportReferences(references, renderContext, options);
      } else {
        // Render all references
        await this.renderAllReferences(references, renderContext, options);
      }

      // Update metrics
      const renderTime = performance.now() - startTime;
      this.metrics.renderTime = renderTime;
      this.metrics.lastRender = Date.now();

      // Emit completion event
      (eventBus.emit as (event: unknown) => void)({
        type: 'references:rendered',
        namespace: 'coordination',
        source: this.serviceName,
        timestamp: Date.now(),
        containerElement: container,
        referencesCount: references.length,
        renderTime,
        metadata: { context: renderContext, options }
      });
    } catch (error) {
      console.error('NodeReferenceRenderer: Error rendering container', {
        error,
        container,
        context,
        options
      });
      throw error;
    }
  }

  /**
   * Render a single node reference
   */
  public async renderReference(
    element: HTMLElement,
    nodeId: string,
    displayContext: 'inline' | 'popup' | 'preview' = 'inline',
    options: RenderOptions = {}
  ): Promise<void> {
    try {
      // Get node data
      const node = await this.resolveNode(nodeId);
      if (!node) {
        this.renderErrorReference(element, nodeId, 'Node not found');
        return;
      }

      // Create decoration context
      const decorationContext: DecorationContext = {
        nodeId: node.id,
        nodeType: node.type,
        title: this.extractNodeTitle(node),
        content: node.content,
        uri: this.nodeReferenceService.createNodespaceURI(nodeId),
        metadata: node.metadata || {},
        targetElement: element,
        displayContext
      };

      // Check cache if not forcing or skipping cache
      if (!options.force && !options.skipCache) {
        const cached = this.getCachedDecoration(nodeId, displayContext);
        if (cached) {
          this.applyDecoration(element, cached.result, decorationContext);
          return;
        }
      }

      // Generate decoration
      const decoration = this.decoratorFactory.decorateReference(decorationContext);

      // Cache the result
      this.cacheDecoration(nodeId, displayContext, decoration, element);

      // Apply decoration to element
      this.applyDecoration(element, decoration, decorationContext);

      // Register with decoration coordinator
      decorationCoordinator.registerDecoration({
        nodeId,
        decorationType: node.type,
        target: nodeId,
        element,
        isActive: false,
        lastUpdate: Date.now(),
        metadata: decorationContext.metadata
      });

      this.metrics.renderedReferences++;
    } catch (error) {
      console.error('NodeReferenceRenderer: Error rendering reference', {
        error,
        element,
        nodeId,
        displayContext
      });
      this.renderErrorReference(element, nodeId, 'Rendering error');
    }
  }

  /**
   * Update decoration for a specific node
   */
  public async updateDecoration(
    nodeId: string,
    reason: 'content-changed' | 'status-changed' | 'metadata-changed' = 'content-changed'
  ): Promise<void> {
    try {
      // Invalidate cache for this node
      this.invalidateNodeCache(nodeId);

      // Find all rendered instances of this node
      const elements = this.findRenderedElements(nodeId);

      // Re-render each instance
      for (const element of elements) {
        const displayContext = this.getElementDisplayContext(element);
        await this.renderReference(element, nodeId, displayContext, { force: true });
      }

      // Emit update event
      (eventBus.emit as (event: unknown) => void)({
        type: 'decoration:updated',
        namespace: 'coordination',
        source: this.serviceName,
        timestamp: Date.now(),
        nodeId,
        updateReason: reason,
        affectedElements: elements.length,
        metadata: { reason }
      });
    } catch (error) {
      console.error('NodeReferenceRenderer: Error updating decoration', { error, nodeId, reason });
    }
  }

  /**
   * Get rendering metrics
   */
  public getMetrics(): RenderMetrics {
    return { ...this.metrics };
  }

  /**
   * Clear all caches
   */
  public clearCache(): void {
    this.renderCache.clear();
    this.metrics.cacheMisses = 0;
  }

  // ========================================================================
  // Event Bus Integration
  // ========================================================================

  private setupEventBusIntegration(): void {
    // Listen for node updates
    eventBus.subscribe('node:updated', (event) => {
      const nodeEvent = event as import('./EventTypes').NodeUpdatedEvent;
      this.updateDecoration(nodeEvent.nodeId, 'content-changed');
    });

    // Listen for node status changes
    eventBus.subscribe('node:status-changed', (event) => {
      const statusEvent = event as import('./EventTypes').NodeStatusChangedEvent;
      this.updateDecoration(statusEvent.nodeId, 'status-changed');
    });

    // Listen for cache invalidation
    eventBus.subscribe('cache:invalidate', (event) => {
      const cacheEvent = event as import('./EventTypes').CacheInvalidateEvent;
      if (cacheEvent.scope === 'global') {
        this.clearCache();
      } else if (cacheEvent.scope === 'node' && cacheEvent.nodeId) {
        this.invalidateNodeCache(cacheEvent.nodeId);
      }
    });

    // Listen for reference update requests
    eventBus.subscribe('references:update-needed', (event) => {
      const updateEvent = event as import('./EventTypes').ReferencesUpdateNeededEvent;
      this.updateDecoration(updateEvent.nodeId, 'metadata-changed');
    });
  }

  // ========================================================================
  // Viewport Optimization
  // ========================================================================

  private setupViewportOptimization(): void {
    // Check for IntersectionObserver support
    if (
      typeof window === 'undefined' ||
      typeof window.IntersectionObserver === 'undefined' ||
      !window.IntersectionObserver
    ) {
      console.debug(
        'NodeReferenceRenderer: IntersectionObserver not available, viewport optimization disabled'
      );
      return;
    }

    this.intersectionObserver = new window.IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const element = entry.target as HTMLElement;
          const nodeId = element.dataset.nodeId;

          if (!nodeId) continue;

          if (entry.isIntersecting) {
            // Element entered viewport - render if not already rendered
            if (!element.classList.contains('ns-noderef--rendered')) {
              const displayContext = this.getElementDisplayContext(element);
              this.renderReference(element, nodeId, displayContext);
            }
            this.metrics.viewportReferences++;
          }
        }
      },
      {
        rootMargin: '100px', // Start rendering 100px before element enters viewport
        threshold: 0.1
      }
    );
  }

  private setupMutationObserving(): void {
    // Check for MutationObserver support
    if (
      typeof window === 'undefined' ||
      typeof window.MutationObserver === 'undefined' ||
      !window.MutationObserver
    ) {
      console.debug(
        'NodeReferenceRenderer: MutationObserver not available, automatic DOM change detection disabled'
      );
      return;
    }

    this.mutationObserver = new window.MutationObserver((mutations) => {
      for (const mutation of mutations) {
        if (mutation.type === 'childList') {
          // Check for new node references added to the DOM
          for (const node of Array.from(mutation.addedNodes)) {
            if (node.nodeType === Node.ELEMENT_NODE) {
              const element = node as HTMLElement;
              this.scanForNewReferences(element);
            }
          }
        }
      }
    });
  }

  // ========================================================================
  // Reference Discovery and Processing
  // ========================================================================

  private findNodeReferences(
    container: HTMLElement
  ): Array<{ element: HTMLElement; nodeId: string; uri: string }> {
    const references: Array<{ element: HTMLElement; nodeId: string; uri: string }> = [];

    // Find elements with nodespace:// URIs
    const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);

    let textNode;
    while ((textNode = walker.nextNode())) {
      const text = textNode.textContent || '';
      const nodespaceLinkRegex = /nodespace:\/\/node\/([a-zA-Z0-9_-]+)(?:\?[^)\s]*)?\)/g;

      let match;
      while ((match = nodespaceLinkRegex.exec(text)) !== null) {
        const uri = match[0];
        const nodeId = match[1];

        // Create a placeholder element for the reference
        const referenceElement = document.createElement('span');
        referenceElement.className = 'ns-noderef-placeholder';
        referenceElement.dataset.nodeId = nodeId;
        referenceElement.dataset.uri = uri;

        references.push({ element: referenceElement, nodeId, uri });
      }
    }

    // Also find existing rendered references
    const existingRefs = container.querySelectorAll('[data-node-id]');
    for (const element of Array.from(existingRefs)) {
      const nodeId = (element as HTMLElement).dataset.nodeId;
      const uri = (element as HTMLElement).dataset.uri;
      if (nodeId && uri) {
        references.push({ element: element as HTMLElement, nodeId, uri });
      }
    }

    return references;
  }

  private async renderViewportReferences(
    references: Array<{ element: HTMLElement; nodeId: string; uri: string }>,
    context: RenderContext,
    options: RenderOptions
  ): Promise<void> {
    if (!this.intersectionObserver) {
      // Fallback to render all if no intersection observer
      return this.renderAllReferences(references, context, options);
    }

    // Set up intersection observing for viewport-based rendering
    for (const ref of references) {
      this.intersectionObserver.observe(ref.element);
    }
  }

  private async renderAllReferences(
    references: Array<{ element: HTMLElement; nodeId: string; uri: string }>,
    context: RenderContext,
    options: RenderOptions
  ): Promise<void> {
    // Batch process references for performance
    const batches = this.chunkArray(references, context.batchSize);

    for (const batch of batches) {
      await Promise.all(
        batch.map((ref) =>
          this.renderReference(ref.element, ref.nodeId, context.displayContext, options)
        )
      );

      // Small delay between batches to prevent blocking
      if (batches.length > 1) {
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
    }
  }

  private scanForNewReferences(element: HTMLElement): void {
    const references = this.findNodeReferences(element);

    for (const ref of references) {
      if (!ref.element.classList.contains('ns-noderef--rendered')) {
        const displayContext = this.getElementDisplayContext(ref.element);
        this.renderReference(ref.element, ref.nodeId, displayContext);
      }
    }
  }

  // ========================================================================
  // Decoration Application and Caching
  // ========================================================================

  private applyDecoration(
    element: HTMLElement,
    decoration: ComponentDecoration | DecorationResult,
    context: DecorationContext
  ): void {
    try {
      // Handle ComponentDecoration vs DecorationResult
      if ('component' in decoration) {
        // This is a ComponentDecoration - create component placeholder for hydration
        const componentName = decoration.component.name;
        const propsJSON = JSON.stringify(decoration.props).replace(/"/g, '&quot;');
        const metadataJSON = decoration.metadata ? JSON.stringify(decoration.metadata).replace(/"/g, '&quot;') : '';
        
        element.innerHTML = `<div class="ns-component-placeholder" 
          data-hydrate="pending" 
          data-component="${componentName}" 
          data-node-type="${context.nodeType}" 
          data-props="${propsJSON}" 
          data-metadata="${metadataJSON}"></div>`;
      } else {
        // This is a legacy DecorationResult - set HTML content (already sanitized by decorator)
        element.innerHTML = decoration.html;
      }

      // Apply CSS classes and accessibility attributes
      if ('component' in decoration) {
        // ComponentDecoration - use default classes
        const nodeType = context.nodeType;
        element.className = `ns-noderef ns-noderef--${nodeType}`;
        element.setAttribute('aria-label', decoration.props.ariaLabel as string || `Reference to ${nodeType}`);
        element.setAttribute('role', 'button'); // Component decorations are interactive
      } else {
        // DecorationResult - use provided classes and labels
        element.className = decoration.cssClasses.join(' ');
        element.setAttribute('aria-label', decoration.ariaLabel);
        element.setAttribute('role', 'text'); // DecorationResult doesn't have interactive property
      }

      // Set data attributes
      element.dataset.nodeId = context.nodeId;
      element.dataset.uri = context.uri;
      element.dataset.context = context.displayContext;

      // Mark as rendered
      element.classList.add('ns-noderef--rendered');

      // Set up keyboard accessibility if interactive
      const isInteractive = 'component' in decoration ? true : false; // DecorationResult doesn't have interactive property
      if (isInteractive) {
        element.setAttribute('tabindex', '0');
        this.setupKeyboardHandlers(element, context);
      }
    } catch (error) {
      console.error('NodeReferenceRenderer: Error applying decoration', {
        error,
        element,
        decoration,
        context
      });
      this.renderErrorReference(element, context.nodeId, 'Application error');
    }
  }

  private setupKeyboardHandlers(element: HTMLElement, context: DecorationContext): void {
    const keydownHandler = (event: KeyboardEvent) => {
      if (event.key === 'Enter' || event.key === ' ') {
        event.preventDefault();

        // Emit click event
        eventBus.emit<import('./EventTypes').DecorationClickedEvent>({
          type: 'decoration:clicked',
          namespace: 'interaction',
          source: this.serviceName,
          nodeId: context.nodeId,
          decorationType: context.nodeType,
          target: context.nodeId,
          clickPosition: { x: 0, y: 0 }, // Keyboard activation
          metadata: { ...context.metadata, keyboardActivated: true }
        });
      }
    };

    element.addEventListener('keydown', keydownHandler);

    // Store handler for cleanup
    (element as HTMLElement & { _keydownHandler?: (event: KeyboardEvent) => void })._keydownHandler = keydownHandler;
  }

  private getCachedDecoration(
    nodeId: string,
    displayContext: string
  ): { result: ComponentDecoration | DecorationResult; element?: HTMLElement } | null {
    const key = `${nodeId}:${displayContext}`;
    const cached = this.renderCache.get(key);

    if (cached && Date.now() - cached.timestamp < this.cacheTimeout) {
      return cached;
    }

    if (cached) {
      this.renderCache.delete(key);
    }

    this.metrics.cacheMisses++;
    return null;
  }

  private cacheDecoration(
    nodeId: string,
    displayContext: string,
    decoration: ComponentDecoration | DecorationResult,
    element?: HTMLElement
  ): void {
    const key = `${nodeId}:${displayContext}`;
    this.renderCache.set(key, {
      result: decoration,
      timestamp: Date.now(),
      element
    });
  }

  private renderErrorReference(element: HTMLElement, nodeId: string, message: string): void {
    element.innerHTML = `
      <span class="ns-noderef ns-noderef--error" 
            data-node-id="${nodeId}"
            role="text"
            aria-label="Reference error: ${message}">
        <span class="ns-noderef__icon">⚠️</span>
        <span class="ns-noderef__title">Reference Error</span>
      </span>
    `;
    element.classList.add('ns-noderef--rendered', 'ns-noderef--error');
  }

  // ========================================================================
  // Utility Methods
  // ========================================================================

  private async resolveNode(nodeId: string): Promise<NodeSpaceNode | null> {
    try {
      const uri = this.nodeReferenceService.createNodespaceURI(nodeId);
      return await this.nodeReferenceService.resolveNodespaceURI(uri);
    } catch (error) {
      console.error('NodeReferenceRenderer: Error resolving node', { error, nodeId });
      return null;
    }
  }

  private extractNodeTitle(node: NodeSpaceNode): string {
    if (!node.content) return 'Untitled';

    const lines = node.content.split('\n');
    const firstLine = lines[0].trim();

    const headerMatch = firstLine.match(/^#{1,6}\s*(.*)$/);
    if (headerMatch) {
      return headerMatch[1].trim() || 'Untitled';
    }

    return firstLine.substring(0, 100) || 'Untitled';
  }

  private invalidateNodeCache(nodeId: string): void {
    const keysToDelete: string[] = [];
    for (const key of Array.from(this.renderCache.keys())) {
      if (key.startsWith(`${nodeId}:`)) {
        keysToDelete.push(key);
      }
    }
    for (const key of keysToDelete) {
      this.renderCache.delete(key);
    }
  }

  private findRenderedElements(nodeId: string): HTMLElement[] {
    return Array.from(document.querySelectorAll(`[data-node-id="${nodeId}"]`)) as HTMLElement[];
  }

  private getElementDisplayContext(element: HTMLElement): 'inline' | 'popup' | 'preview' {
    return (element.dataset.context as 'inline' | 'popup' | 'preview') || 'inline';
  }

  private chunkArray<T>(array: T[], chunkSize: number): T[][] {
    const chunks: T[][] = [];
    for (let i = 0; i < array.length; i += chunkSize) {
      chunks.push(array.slice(i, i + chunkSize));
    }
    return chunks;
  }

  // ========================================================================
  // Cleanup
  // ========================================================================

  public cleanup(): void {
    if (this.intersectionObserver) {
      this.intersectionObserver.disconnect();
      this.intersectionObserver = null;
    }

    if (this.mutationObserver) {
      this.mutationObserver.disconnect();
      this.mutationObserver = null;
    }

    if (this.renderTimeout) {
      if (typeof window !== 'undefined' && window.clearTimeout) {
        window.clearTimeout(this.renderTimeout);
      } else if (typeof clearTimeout !== 'undefined') {
        clearTimeout(this.renderTimeout);
      }
      this.renderTimeout = null;
    }

    // Clean up keyboard handlers
    for (const cached of Array.from(this.renderCache.values())) {
      const elementWithHandler = cached.element as HTMLElement & {
        _keydownHandler?: EventListener;
      };
      if (cached.element && elementWithHandler._keydownHandler) {
        cached.element.removeEventListener('keydown', elementWithHandler._keydownHandler);
        delete elementWithHandler._keydownHandler;
      }
    }

    this.renderCache.clear();
    this.renderQueue.length = 0;
    this.pendingRenders.clear();
  }
}

// ============================================================================
// Singleton Export and Convenience Functions
// ============================================================================

let rendererInstance: NodeReferenceRenderer | null = null;

export function getNodeReferenceRenderer(
  nodeReferenceService?: NodeReferenceService
): NodeReferenceRenderer {
  if (!rendererInstance && nodeReferenceService) {
    rendererInstance = NodeReferenceRenderer.getInstance(nodeReferenceService);
  }
  return rendererInstance!;
}

export function initializeNodeReferenceRenderer(
  nodeReferenceService: NodeReferenceService
): NodeReferenceRenderer {
  rendererInstance = NodeReferenceRenderer.getInstance(nodeReferenceService);
  return rendererInstance;
}

export default NodeReferenceRenderer;
