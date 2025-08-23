/**
 * Component-Based Decoration System Integration Test
 *
 * This test validates the complete component-based decoration system:
 * 1. ContentProcessor renders nodespace references with component placeholders
 * 2. ComponentHydrationSystem can mount Svelte components from placeholders
 * 3. Plugin architecture is ready for future node types
 * 4. End-to-end component rendering pipeline works correctly
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { JSDOM } from 'jsdom';

// Set up DOM environment for testing
const dom = new JSDOM('<!DOCTYPE html><html><body></body></html>', {
  url: 'http://localhost',
  pretendToBeVisual: true,
  resources: 'usable'
});

// Make DOM globally available
globalThis.document = dom.window.document;
globalThis.window = dom.window as unknown as Window & typeof globalThis;
globalThis.HTMLElement = dom.window.HTMLElement;
globalThis.Element = dom.window.Element;
globalThis.Node = dom.window.Node;

import { contentProcessor } from '../../lib/services/contentProcessor';
import { componentHydrationSystem } from '../../lib/services/ComponentHydrationSystem';
import { NodeDecoratorFactory } from '../../lib/services/BaseNodeDecoration';
import {
  NODE_REFERENCE_COMPONENTS,
  getNodeReferenceComponent
} from '../../lib/components/references';
import { MockDatabaseService } from '../../lib/services/MockDatabaseService';
import { NodeManager } from '../../lib/services/NodeManager';
import { HierarchyService } from '../../lib/services/HierarchyService';
import { NodeOperationsService } from '../../lib/services/NodeOperationsService';
import { NodeReferenceService } from '../../lib/services/NodeReferenceService';

describe('Component-Based Decoration System', () => {
  let mockDb: MockDatabaseService;
  let nodeManager: NodeManager;
  let hierarchyService: HierarchyService;
  let nodeOperationsService: NodeOperationsService;
  let nodeReferenceService: NodeReferenceService;

  beforeEach(async () => {
    // Initialize services for testing
    mockDb = new MockDatabaseService();

    // Create mock NodeManagerEvents
    const mockEvents = {
      focusRequested: () => {},
      hierarchyChanged: () => {},
      nodeCreated: () => {},
      nodeDeleted: () => {}
    };

    nodeManager = new NodeManager(mockEvents);
    hierarchyService = new HierarchyService(nodeManager);
    nodeOperationsService = new NodeOperationsService(nodeManager, hierarchyService);
    nodeReferenceService = new NodeReferenceService(
      nodeManager,
      hierarchyService,
      nodeOperationsService,
      mockDb
    );
  });

  describe('Component Registry', () => {
    it('should have core node reference component registered', () => {
      // Should have BaseNodeReference available
      expect(NODE_REFERENCE_COMPONENTS['BaseNodeReference']).toBeDefined();
      expect(NODE_REFERENCE_COMPONENTS['base']).toBeDefined();

      // All node types should map to BaseNodeReference for now
      expect(getNodeReferenceComponent('text')).toBe(NODE_REFERENCE_COMPONENTS.base);
      expect(getNodeReferenceComponent('task')).toBe(NODE_REFERENCE_COMPONENTS.base);
      expect(getNodeReferenceComponent('user')).toBe(NODE_REFERENCE_COMPONENTS.base);
      expect(getNodeReferenceComponent('document')).toBe(NODE_REFERENCE_COMPONENTS.base);
    });

    it('should resolve components by node type', () => {
      // Test component resolution for different node types
      expect(getNodeReferenceComponent('text')).toBeDefined();
      expect(getNodeReferenceComponent('task')).toBeDefined();
      expect(getNodeReferenceComponent('user')).toBeDefined();
      expect(getNodeReferenceComponent('document')).toBeDefined();
      expect(getNodeReferenceComponent('unknown')).toBeDefined(); // Should fall back to base
    });
  });

  describe('NodeDecoratorFactory', () => {
    it('should create component decorations for different node types', () => {
      const factory = new NodeDecoratorFactory(nodeReferenceService);
      const context = {
        nodeId: 'test-node',
        nodeType: 'text',
        title: 'Test Node',
        content: 'Test content',
        uri: 'nodespace://workspace/test-node',
        metadata: {},
        targetElement: document.createElement('span'),
        displayContext: 'inline' as const
      };

      const decoration = factory.decorateReference(context);

      expect(decoration).toBeDefined();
      expect(decoration.component).toBeDefined();
      expect(decoration.props).toBeDefined();
      expect(decoration.props.nodeId).toBe('test-node');
      expect(decoration.props.nodeType).toBe('text');
      expect(decoration.props.title).toBe('Test Node');
    });

    it('should handle different node types appropriately', () => {
      const factory = new NodeDecoratorFactory(nodeReferenceService);
      const nodeTypes = ['text', 'task', 'user', 'document', 'ai_chat'];

      nodeTypes.forEach((nodeType) => {
        const context = {
          nodeId: `test-${nodeType}`,
          nodeType,
          title: `Test ${nodeType}`,
          content: `Test ${nodeType} content`,
          uri: `nodespace://workspace/test-${nodeType}`,
          metadata: {},
          targetElement: document.createElement('span'),
          displayContext: 'inline' as const
        };

        const decoration = factory.decorateReference(context);

        expect(decoration).toBeDefined();
        expect(decoration.component).toBeDefined();
        expect(decoration.props.nodeType).toBe(nodeType);

        // All should use BaseNodeReference for now
        expect(decoration.component).toBe(NODE_REFERENCE_COMPONENTS.base);
      });
    });
  });

  describe('ContentProcessor Integration', () => {
    beforeEach(() => {
      // Initialize ContentProcessor with our services
      contentProcessor.setNodeReferenceService(nodeReferenceService);
    });

    it('should render component placeholders for nodespace references', async () => {
      // Create the node that will be referenced using the correct method
      const createdNode = await nodeReferenceService.createNode('text', 'Test node content');

      const markdown = `Check this [reference](nodespace://node/${createdNode.id}) for details.`;

      const html = await contentProcessor.markdownToDisplayWithReferences(markdown);

      expect(html).toBeDefined();
      expect(html).toContain('data-component="BaseNodeReference"');
      expect(html).toContain('data-node-id');
    });

    it('should include proper component data in placeholders', async () => {
      // Create the node that will be referenced
      const createdNode = await nodeReferenceService.createNode('text', 'Example node content');

      const markdown = `Reference: nodespace://node/${createdNode.id}`;

      const html = await contentProcessor.markdownToDisplayWithReferences(markdown);

      expect(html).toContain('data-component="BaseNodeReference"');
      expect(html).toContain('data-props');

      // Should include proper JSON-encoded props
      const parser = new dom.window.DOMParser();
      const doc = parser.parseFromString(html, 'text/html');
      const placeholder = doc.querySelector('[data-component]');

      expect(placeholder).toBeTruthy();
      if (placeholder) {
        const propsString = placeholder.getAttribute('data-props') || '{}';
        const props = JSON.parse(propsString);
        expect(props.nodeId).toBeDefined();
        expect(props.nodeType).toBeDefined();
      }
    });
  });

  describe('ComponentHydrationSystem', () => {
    let container: HTMLElement;

    beforeEach(() => {
      container = document.createElement('div');
      document.body.appendChild(container);
    });

    it('should hydrate component placeholders', async () => {
      // Create a placeholder element matching what ContentProcessor generates
      const placeholder = document.createElement('span');
      placeholder.className = 'ns-component-placeholder';
      placeholder.setAttribute('data-component', 'BaseNodeReference');
      placeholder.setAttribute('data-node-type', 'text');
      placeholder.setAttribute(
        'data-props',
        JSON.stringify({
          nodeId: 'test-node',
          nodeType: 'text',
          title: 'Test Node',
          content: 'Test content',
          href: '/node/test-node'
        })
      );
      placeholder.setAttribute('data-metadata', '{}');
      placeholder.setAttribute('data-hydrate', 'pending');
      container.appendChild(placeholder);

      const result = await componentHydrationSystem.hydrate({ container });

      expect(result.hydrated).toBe(1);
      expect(result.failed).toBe(0);
      expect(result.hydrated + result.failed).toBe(1);
    });

    it('should handle hydration errors gracefully', async () => {
      // Create a placeholder with invalid JSON
      const placeholder = document.createElement('span');
      placeholder.className = 'ns-component-placeholder';
      placeholder.setAttribute('data-component', 'BaseNodeReference');
      placeholder.setAttribute('data-node-type', 'text');
      placeholder.setAttribute('data-props', 'invalid-json');
      placeholder.setAttribute('data-hydrate', 'pending');
      container.appendChild(placeholder);

      const result = await componentHydrationSystem.hydrate({ container });

      expect(result.hydrated).toBe(0);
      expect(result.failed).toBe(1);
      expect(result.hydrated + result.failed).toBe(1);
    });

    it('should provide accurate statistics', async () => {
      // Create multiple placeholders matching what ContentProcessor generates
      for (let i = 0; i < 3; i++) {
        const placeholder = document.createElement('span');
        placeholder.className = 'ns-component-placeholder';
        placeholder.setAttribute('data-component', 'BaseNodeReference');
        placeholder.setAttribute('data-node-type', 'text');
        placeholder.setAttribute(
          'data-props',
          JSON.stringify({
            nodeId: `test-node-${i}`,
            nodeType: 'text',
            title: `Test Node ${i}`,
            content: `Test content ${i}`,
            href: `/node/test-node-${i}`
          })
        );
        placeholder.setAttribute('data-metadata', '{}');
        placeholder.setAttribute('data-hydrate', 'pending');
        container.appendChild(placeholder);
      }

      const result = await componentHydrationSystem.hydrate({ container });
      const stats = componentHydrationSystem.getStats();

      expect(result.hydrated).toBe(3);
      expect(result.failed).toBe(0);
      expect(result.hydrated + result.failed).toBe(3);
      expect(stats.totalMounted).toBeGreaterThanOrEqual(3);
    });

    it('should cleanup components properly', async () => {
      // Create and hydrate a component
      const placeholder = document.createElement('span');
      placeholder.setAttribute('data-component', 'BaseNodeReference');
      placeholder.setAttribute(
        'data-props',
        JSON.stringify({
          nodeId: 'test-node',
          nodeType: 'text',
          title: 'Test Node',
          content: 'Test content',
          href: '/node/test-node'
        })
      );
      container.appendChild(placeholder);

      await componentHydrationSystem.hydrate({ container });

      // Cleanup should work without errors
      expect(() => {
        componentHydrationSystem.cleanup(container);
      }).not.toThrow();
    });
  });

  describe('Plugin Architecture Readiness', () => {
    it('should support plugin component registration pattern', () => {
      // The current architecture should support future plugin registration
      expect(NODE_REFERENCE_COMPONENTS).toBeDefined();
      expect(typeof getNodeReferenceComponent).toBe('function');

      // Should handle unknown component types gracefully
      const unknownComponent = getNodeReferenceComponent('unknown-plugin-type');
      expect(unknownComponent).toBeDefined();
      expect(unknownComponent).toBe(NODE_REFERENCE_COMPONENTS.base); // Fallback to base
    });
  });

  describe('End-to-End Integration', () => {
    beforeEach(() => {
      contentProcessor.setNodeReferenceService(nodeReferenceService);
    });

    it('should complete the full pipeline: markdown → components', async () => {
      // Create the node that will be referenced
      const createdNode = await nodeReferenceService.createNode('text', 'Important note content');

      const markdown = `See this [important note](nodespace://node/${createdNode.id}) for context.`;

      // Step 1: Process markdown with ContentProcessor
      const processedHtml = await contentProcessor.markdownToDisplayWithReferences(markdown);
      expect(processedHtml).toContain('data-component="BaseNodeReference"');

      // Step 2: Create DOM from processed HTML
      const parser = new dom.window.DOMParser();
      const doc = parser.parseFromString(processedHtml, 'text/html');
      const container = doc.body;

      // Step 3: Hydrate components
      const hydrateResult = await componentHydrationSystem.hydrate({
        container: container as HTMLElement
      });

      expect(hydrateResult.hydrated).toBe(1);
      expect(hydrateResult.failed).toBe(0);
      expect(hydrateResult.hydrated + hydrateResult.failed).toBe(1);

      // The full pipeline should work end-to-end
      expect(hydrateResult.hydrated).toBeGreaterThan(0);
    });
  });
});
