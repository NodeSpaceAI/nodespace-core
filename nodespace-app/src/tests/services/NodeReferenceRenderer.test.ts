/**
 * NodeReferenceRenderer Tests
 *
 * Tests for the performance-optimized reference decoration rendering system
 * that coordinates the BaseNode decoration classes.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  NodeReferenceRenderer,
  initializeNodeReferenceRenderer
} from '$lib/services/NodeReferenceRenderer';
import { NodeReferenceService } from '$lib/services/NodeReferenceService';
import { EnhancedNodeManager } from '$lib/services/EnhancedNodeManager';
import { HierarchyService } from '$lib/services/HierarchyService';
import { NodeOperationsService } from '$lib/services/NodeOperationsService';
import { MockDatabaseService, type NodeSpaceNode } from '$lib/services/MockDatabaseService';

// Type definitions for mock DOM elements
interface MockElement {
  tagName: string;
  innerHTML: string;
  classList: {
    add: ReturnType<typeof vi.fn>;
    remove: ReturnType<typeof vi.fn>;
    contains: ReturnType<typeof vi.fn>;
    toggle: ReturnType<typeof vi.fn>;
  };
  dataset: Record<string, string>;
  setAttribute: ReturnType<typeof vi.fn>;
  addEventListener: ReturnType<typeof vi.fn>;
  removeEventListener: ReturnType<typeof vi.fn>;
}

interface MockTreeWalker {
  nextNode: ReturnType<typeof vi.fn>;
}

interface MockDocument {
  createElement: ReturnType<typeof vi.fn>;
  createTreeWalker: ReturnType<typeof vi.fn>;
  querySelectorAll: ReturnType<typeof vi.fn>;
}

interface MockWindow {
  IntersectionObserver: ReturnType<typeof vi.fn>;
  MutationObserver: ReturnType<typeof vi.fn>;
}

describe('NodeReferenceRenderer', () => {
  let renderer: NodeReferenceRenderer;
  let nodeReferenceService: NodeReferenceService;
  let databaseService: MockDatabaseService;
  let nodeManager: EnhancedNodeManager;
  let hierarchyService: HierarchyService;
  let nodeOperationsService: NodeOperationsService;

  beforeEach(async () => {
    // Initialize services
    databaseService = new MockDatabaseService();
    nodeManager = new EnhancedNodeManager(databaseService);
    hierarchyService = new HierarchyService(nodeManager, databaseService);
    nodeOperationsService = new NodeOperationsService(
      nodeManager,
      hierarchyService,
      databaseService
    );
    nodeReferenceService = new NodeReferenceService(
      nodeManager,
      hierarchyService,
      nodeOperationsService,
      databaseService
    );

    renderer = initializeNodeReferenceRenderer(nodeReferenceService);

    // Mock DOM environment
    globalThis.document = {
      createElement: vi.fn(
        (tagName: string): MockElement => ({
          tagName: tagName.toUpperCase(),
          innerHTML: '',
          classList: {
            add: vi.fn(),
            remove: vi.fn(),
            contains: vi.fn(() => false),
            toggle: vi.fn()
          },
          dataset: {},
          setAttribute: vi.fn(),
          addEventListener: vi.fn(),
          removeEventListener: vi.fn()
        })
      ),
      createTreeWalker: vi.fn(
        (): MockTreeWalker => ({
          nextNode: vi.fn(() => null)
        })
      ),
      querySelectorAll: vi.fn(() => [])
    } as MockDocument;

    globalThis.window = {
      IntersectionObserver: vi.fn(),
      MutationObserver: vi.fn()
    } as MockWindow;
  });

  afterEach(() => {
    if (renderer) {
      renderer.cleanup();
    }
    vi.clearAllMocks();
  });

  // ========================================================================
  // Initialization Tests
  // ========================================================================

  describe('Initialization', () => {
    it('should initialize successfully with NodeReferenceService', () => {
      expect(renderer).toBeDefined();
      expect(renderer.getMetrics).toBeDefined();
      expect(renderer.renderReference).toBeDefined();
      expect(renderer.renderContainer).toBeDefined();
    });

    it('should provide initial metrics', () => {
      const metrics = renderer.getMetrics();

      expect(metrics).toEqual({
        totalReferences: 0,
        renderedReferences: 0,
        viewportReferences: 0,
        cacheMisses: 0,
        renderTime: 0,
        lastRender: 0
      });
    });
  });

  // ========================================================================
  // Single Reference Rendering
  // ========================================================================

  describe('Single Reference Rendering', () => {
    let testNode: NodeSpaceNode;
    let mockElement: HTMLElement;

    beforeEach(async () => {
      // Create a test node
      testNode = await nodeReferenceService.createNode(
        'task',
        'Test Task\nstatus: pending\npriority: high\n\nTest task description'
      );

      mockElement = {
        innerHTML: '',
        classList: {
          add: vi.fn(),
          remove: vi.fn(),
          contains: vi.fn(() => false),
          toggle: vi.fn()
        },
        dataset: {},
        setAttribute: vi.fn(),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn()
      } as unknown as HTMLElement;
    });

    it('should render a single task reference correctly', async () => {
      await renderer.renderReference(mockElement, testNode.id, 'inline');

      expect(mockElement.innerHTML).toContain('ns-noderef--task');
      expect(mockElement.innerHTML).toContain('Test Task');
      expect(mockElement.setAttribute).toHaveBeenCalledWith(
        'aria-label',
        expect.stringContaining('Task')
      );
      expect(mockElement.setAttribute).toHaveBeenCalledWith('role', 'button');
    });

    it('should handle different display contexts', async () => {
      const contexts: Array<'inline' | 'popup' | 'preview'> = ['inline', 'popup', 'preview'];

      for (const context of contexts) {
        const element = { ...mockElement };
        await renderer.renderReference(element, testNode.id, context);

        expect(element.dataset.context).toBe(context);
      }
    });

    it('should handle non-existent nodes gracefully', async () => {
      await renderer.renderReference(mockElement, 'non-existent-id', 'inline');

      expect(mockElement.innerHTML).toContain('ns-noderef--error');
      expect(mockElement.innerHTML).toContain('Reference Error');
    });

    it('should use cache on subsequent renders', async () => {
      // First render
      await renderer.renderReference(mockElement, testNode.id, 'inline');
      const firstMetrics = renderer.getMetrics();

      // Second render with same parameters
      const secondElement = { ...mockElement };
      await renderer.renderReference(secondElement, testNode.id, 'inline');
      const secondMetrics = renderer.getMetrics();

      // Cache should be used (no additional cache miss)
      expect(secondMetrics.cacheMisses).toBe(firstMetrics.cacheMisses);
    });

    it('should force refresh when requested', async () => {
      // First render
      await renderer.renderReference(mockElement, testNode.id, 'inline');
      const firstMetrics = renderer.getMetrics();

      // Force refresh
      const secondElement = { ...mockElement };
      await renderer.renderReference(secondElement, testNode.id, 'inline', { force: true });
      const secondMetrics = renderer.getMetrics();

      // Should have additional cache miss due to forced refresh
      expect(secondMetrics.cacheMisses).toBeGreaterThan(firstMetrics.cacheMisses);
    });
  });

  // ========================================================================
  // Container Rendering
  // ========================================================================

  describe('Container Rendering', () => {
    let mockContainer: HTMLElement;
    let testNodes: NodeSpaceNode[];

    beforeEach(async () => {
      // Create test nodes
      testNodes = [
        await nodeReferenceService.createNode('task', 'Task 1\nstatus: pending'),
        await nodeReferenceService.createNode('user', 'John Doe\nrole: admin'),
        await nodeReferenceService.createNode('date', '2024-12-31\nProject deadline')
      ];

      // Ensure nodes are available for testing
      expect(testNodes).toHaveLength(3);

      mockContainer = {
        innerHTML: '',
        classList: { add: vi.fn(), remove: vi.fn(), contains: vi.fn(() => false) },
        dataset: {},
        setAttribute: vi.fn(),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn()
      } as unknown as HTMLElement;
    });

    it('should render container without viewport optimization', async () => {
      await renderer.renderContainer(mockContainer, {
        viewportOptimization: false,
        displayContext: 'inline',
        batchSize: 10,
        debounceMs: 0
      });

      const metrics = renderer.getMetrics();
      expect(metrics.lastRender).toBeGreaterThan(0);
      expect(metrics.renderTime).toBeGreaterThan(0);
    });

    it('should handle different display contexts for container', async () => {
      const contexts: Array<'inline' | 'popup' | 'preview'> = ['inline', 'popup', 'preview'];

      for (const context of contexts) {
        await renderer.renderContainer(mockContainer, {
          displayContext: context,
          viewportOptimization: false
        });

        // Should complete without errors
        const metrics = renderer.getMetrics();
        expect(metrics.lastRender).toBeGreaterThan(0);
      }
    });
  });

  // ========================================================================
  // Cache Management
  // ========================================================================

  describe('Cache Management', () => {
    let testNode: NodeSpaceNode;

    beforeEach(async () => {
      testNode = await nodeReferenceService.createNode('task', 'Cached Task\nstatus: pending');
    });

    it('should clear cache when requested', async () => {
      const mockElement = {
        innerHTML: '',
        classList: { add: vi.fn(), remove: vi.fn(), contains: vi.fn(() => false) },
        dataset: {},
        setAttribute: vi.fn(),
        addEventListener: vi.fn()
      } as unknown as HTMLElement;

      // Render to populate cache
      await renderer.renderReference(mockElement, testNode.id, 'inline');
      const beforeClear = renderer.getMetrics();

      // Clear cache
      renderer.clearCache();

      // Render again should cause cache miss
      await renderer.renderReference(mockElement, testNode.id, 'inline');
      const afterClear = renderer.getMetrics();

      expect(afterClear.cacheMisses).toBeGreaterThan(beforeClear.cacheMisses);
    });

    it('should update decorations when node content changes', async () => {
      const mockElement = {
        innerHTML: '',
        classList: { add: vi.fn(), remove: vi.fn(), contains: vi.fn(() => false) },
        dataset: {},
        setAttribute: vi.fn(),
        addEventListener: vi.fn()
      } as unknown as HTMLElement;

      // Initial render
      await renderer.renderReference(mockElement, testNode.id, 'inline');
      expect(mockElement.innerHTML).toContain('Cached Task');

      // Simulate decoration update
      await renderer.updateDecoration(testNode.id, 'content-changed');

      // Should handle update without errors
      const metrics = renderer.getMetrics();
      expect(metrics).toBeDefined();
    });
  });

  // ========================================================================
  // Error Handling
  // ========================================================================

  describe('Error Handling', () => {
    it('should handle rendering errors gracefully', async () => {
      const mockElement = {
        innerHTML: '',
        classList: { add: vi.fn(), remove: vi.fn(), contains: vi.fn(() => false) },
        dataset: {},
        setAttribute: vi.fn(() => {
          throw new Error('Mock error');
        }),
        addEventListener: vi.fn()
      } as unknown as HTMLElement;

      // Should not throw, should render error state
      await expect(
        renderer.renderReference(mockElement, 'some-id', 'inline')
      ).resolves.not.toThrow();
    });

    it('should handle service errors during rendering', async () => {
      // Mock service to throw error
      const errorService = {
        ...nodeReferenceService,
        resolveNodespaceURI: vi.fn().mockRejectedValue(new Error('Service error'))
      } as unknown as NodeReferenceService;

      const errorRenderer = new (NodeReferenceRenderer as typeof NodeReferenceRenderer)(
        errorService as NodeReferenceService
      );

      const mockElement = {
        innerHTML: '',
        classList: { add: vi.fn(), remove: vi.fn(), contains: vi.fn(() => false) },
        dataset: {},
        setAttribute: vi.fn(),
        addEventListener: vi.fn()
      } as unknown as HTMLElement;

      await errorRenderer.renderReference(mockElement, 'test-id', 'inline');

      // Should render error state
      expect(mockElement.innerHTML).toContain('ns-noderef--error');
    });
  });

  // ========================================================================
  // Performance Tests
  // ========================================================================

  describe('Performance', () => {
    it('should provide performance metrics', () => {
      const metrics = renderer.getMetrics();

      expect(metrics).toHaveProperty('totalReferences');
      expect(metrics).toHaveProperty('renderedReferences');
      expect(metrics).toHaveProperty('viewportReferences');
      expect(metrics).toHaveProperty('cacheMisses');
      expect(metrics).toHaveProperty('renderTime');
      expect(metrics).toHaveProperty('lastRender');

      expect(typeof metrics.totalReferences).toBe('number');
      expect(typeof metrics.renderedReferences).toBe('number');
      expect(typeof metrics.viewportReferences).toBe('number');
      expect(typeof metrics.cacheMisses).toBe('number');
      expect(typeof metrics.renderTime).toBe('number');
      expect(typeof metrics.lastRender).toBe('number');
    });

    it('should track rendering metrics correctly', async () => {
      const testNode = await nodeReferenceService.createNode('task', 'Performance Test Task');
      const mockElement = {
        innerHTML: '',
        classList: { add: vi.fn(), remove: vi.fn(), contains: vi.fn(() => false) },
        dataset: {},
        setAttribute: vi.fn(),
        addEventListener: vi.fn()
      } as unknown as HTMLElement;

      const beforeMetrics = renderer.getMetrics();

      await renderer.renderReference(mockElement, testNode.id, 'inline');

      const afterMetrics = renderer.getMetrics();

      expect(afterMetrics.renderedReferences).toBeGreaterThan(beforeMetrics.renderedReferences);
      expect(afterMetrics.renderTime).toBeGreaterThanOrEqual(0);
    });
  });

  // ========================================================================
  // Cleanup
  // ========================================================================

  describe('Cleanup', () => {
    it('should cleanup resources properly', () => {
      expect(() => renderer.cleanup()).not.toThrow();

      // After cleanup, metrics should still be accessible
      const metrics = renderer.getMetrics();
      expect(metrics).toBeDefined();
    });
  });
});
