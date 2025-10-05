/**
 * NodeManager + ContentProcessor Integration Tests
 * Validates dual-representation functionality and content processing integration
 */

import { describe, test, expect, beforeEach } from 'vitest';
import {
  createMockReactiveNodeService,
  type MockReactiveNodeService,
  type NodeManagerEvents
} from '../mocks/mock-reactive-node-service';

// Helper to create unified Node objects for tests
function createNode(
  id: string,
  content: string,
  nodeType: string = 'text',
  parentId: string | null = null,
  properties: Record<string, unknown> = {}
) {
  return {
    id,
    nodeType: nodeType,
    content,
    parentId: parentId,
    originNodeId: null,
    beforeSiblingId: null,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    mentions: [] as string[],
    properties
  };
}

describe('NodeManager + ContentProcessor Integration', () => {
  let nodeManager: MockReactiveNodeService;
  let mockEvents: NodeManagerEvents;

  beforeEach(() => {
    mockEvents = {
      focusRequested: () => {},
      hierarchyChanged: () => {},
      nodeCreated: () => {},
      nodeDeleted: () => {}
    };
    nodeManager = createMockReactiveNodeService(mockEvents);
  });

  describe('Dual-Representation Methods', () => {
    test('parseNodeContent should parse markdown content to AST', () => {
      // Initialize with test data including our target node
      nodeManager.initializeNodes(
        [createNode('test-node', '# Test Header\n\nSome **bold** text')],
        {
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0
        }
      );

      // Parse the content
      const ast = nodeManager.parseNodeContent('test-node');

      expect(ast).toBeDefined();
      expect(ast?.type).toBe('document');
      expect(ast?.children).toHaveLength(2); // Header + paragraph
    });

    test('renderNodeAsHTML should convert markdown to HTML', async () => {
      // Initialize with test data
      nodeManager.initializeNodes([createNode('html-test-node', '**Bold text** with *italics*')], {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      // Render as HTML
      const html = await nodeManager.renderNodeAsHTML('html-test-node');

      expect(html).toContain('<strong class="ns-markdown-bold">Bold text</strong>');
      expect(html).toContain('<em class="ns-markdown-italic">italics</em>');
    });

    test('getNodeHeaderLevel should detect header levels', () => {
      nodeManager.initializeNodes(
        [
          createNode('h1-node', '# Header 1'),
          createNode('h2-node', '## Header 2'),
          createNode('text-node', 'Regular text')
        ],
        {
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0
        }
      );

      expect(nodeManager.getNodeHeaderLevel('h1-node')).toBe(1);
      expect(nodeManager.getNodeHeaderLevel('h2-node')).toBe(2);
      expect(nodeManager.getNodeHeaderLevel('text-node')).toBe(0);
    });

    test('getNodeDisplayText should strip markdown syntax', () => {
      nodeManager.initializeNodes([createNode('header-node', '## This is a header')], {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      const displayText = nodeManager.getNodeDisplayText('header-node');

      expect(displayText).toBe('This is a header');
    });

    test('updateNodeContentWithProcessing should update content and header level', () => {
      nodeManager.initializeNodes([createNode('update-test-node', 'Regular text')], {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      // Update to header content
      const success = nodeManager.updateNodeContentWithProcessing(
        'update-test-node',
        '### New Header'
      );

      expect(success).toBe(true);
      expect(nodeManager.getNodeHeaderLevel('update-test-node')).toBe(3);
      expect(nodeManager.getNodeDisplayText('update-test-node')).toBe('New Header');
    });
  });

  describe('Content Processing Integration', () => {
    test('should handle complex markdown content', async () => {
      const complexContent = `# Main Header

This is a paragraph with **bold** and *italic* text.

## Sub Header

- List item 1
- List item 2

\`code block\``;

      nodeManager.initializeNodes([createNode('complex-node', complexContent)], {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      // Test parsing
      const ast = nodeManager.parseNodeContent('complex-node');
      expect(ast?.children?.length).toBeGreaterThan(1);

      // Test HTML rendering
      const html = await nodeManager.renderNodeAsHTML('complex-node');
      expect(html).toContain('<h1 class="ns-markdown-heading ns-markdown-h1">');
      expect(html).toContain('<h2 class="ns-markdown-heading ns-markdown-h2">');
      expect(html).toContain('<strong class="ns-markdown-bold">');
      expect(html).toContain('<em class="ns-markdown-italic">');
    });

    test('should handle empty and invalid content gracefully', async () => {
      const emptyNodeId = nodeManager.createNode('', '');

      expect(nodeManager.parseNodeContent(emptyNodeId)).toBeDefined();
      expect(await nodeManager.renderNodeAsHTML(emptyNodeId)).toBe('');
      expect(nodeManager.getNodeHeaderLevel(emptyNodeId)).toBe(0);
      expect(nodeManager.getNodeDisplayText(emptyNodeId)).toBe('');
    });

    test('should handle non-existent nodes gracefully', async () => {
      const nonExistentId = 'does-not-exist';

      expect(nodeManager.parseNodeContent(nonExistentId)).toBeNull();
      expect(await nodeManager.renderNodeAsHTML(nonExistentId)).toBe('');
      expect(nodeManager.getNodeHeaderLevel(nonExistentId)).toBe(0);
      expect(nodeManager.getNodeDisplayText(nonExistentId)).toBe('');
      expect(nodeManager.updateNodeContentWithProcessing(nonExistentId, 'test')).toBe(false);
    });
  });

  describe('Performance with ContentProcessor', () => {
    test('should handle content processing efficiently for many nodes', async () => {
      // Create multiple nodes with different content types
      const testNodes = [];
      for (let i = 0; i < 100; i++) {
        const content =
          i % 3 === 0
            ? `# Header ${i}`
            : i % 3 === 1
              ? `**Bold text ${i}** with *italic*`
              : `Regular paragraph text for node ${i}`;

        testNodes.push(createNode(`perf-node-${i}`, content));
      }

      nodeManager.initializeNodes(testNodes, {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });
      const nodes = testNodes.map((n) => n.id);

      const startTime = performance.now();

      // Process all nodes
      for (const nodeId of nodes) {
        nodeManager.parseNodeContent(nodeId);
        await nodeManager.renderNodeAsHTML(nodeId);
        nodeManager.getNodeHeaderLevel(nodeId);
        nodeManager.getNodeDisplayText(nodeId);
      }

      const endTime = performance.now();
      const duration = endTime - startTime;

      expect(duration).toBeLessThan(1000); // Should be reasonably fast (increased due to async)
    });
  });
});
