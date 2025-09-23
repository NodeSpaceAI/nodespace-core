/**
 * NodeManager + ContentProcessor Integration Tests
 * Validates dual-representation functionality and content processing integration
 */

import { describe, test, expect, beforeEach } from 'vitest';
import {
  createReactiveNodeService,
  ReactiveNodeService as NodeManager,
  type NodeManagerEvents
} from '../../lib/services/reactiveNodeService.svelte.js';

describe('NodeManager + ContentProcessor Integration', () => {
  let nodeManager: NodeManager;
  let mockEvents: NodeManagerEvents;

  beforeEach(() => {
    mockEvents = {
      focusRequested: () => {},
      hierarchyChanged: () => {},
      nodeCreated: () => {},
      nodeDeleted: () => {}
    };
    nodeManager = createReactiveNodeService(mockEvents);
  });

  describe('Dual-Representation Methods', () => {
    test('parseNodeContent should parse markdown content to AST', () => {
      // Initialize with test data including our target node
      nodeManager.initializeFromLegacyData([
        {
          id: 'test-node',
          type: 'text',
          content: '# Test Header\n\nSome **bold** text',
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          autoFocus: false
        }
      ]);

      // Parse the content
      const ast = nodeManager.parseNodeContent('test-node');

      expect(ast).toBeDefined();
      expect(ast?.type).toBe('document');
      expect(ast?.children).toHaveLength(2); // Header + paragraph
    });

    test('renderNodeAsHTML should convert markdown to HTML', async () => {
      // Initialize with test data
      nodeManager.initializeFromLegacyData([
        {
          id: 'html-test-node',
          type: 'text',
          content: '**Bold text** with *italics*',
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          autoFocus: false
        }
      ]);

      // Render as HTML
      const html = await nodeManager.renderNodeAsHTML('html-test-node');

      expect(html).toContain('<strong class="ns-markdown-bold">Bold text</strong>');
      expect(html).toContain('<em class="ns-markdown-italic">italics</em>');
    });

    test('getNodeHeaderLevel should detect header levels', () => {
      nodeManager.initializeFromLegacyData([
        {
          id: 'h1-node',
          type: 'text',
          content: '# Header 1',
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          autoFocus: false
        },
        {
          id: 'h2-node',
          type: 'text',
          content: '## Header 2',
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          autoFocus: false
        },
        {
          id: 'text-node',
          type: 'text',
          content: 'Regular text',
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          autoFocus: false
        }
      ]);

      expect(nodeManager.getNodeHeaderLevel('h1-node')).toBe(1);
      expect(nodeManager.getNodeHeaderLevel('h2-node')).toBe(2);
      expect(nodeManager.getNodeHeaderLevel('text-node')).toBe(0);
    });

    test('getNodeDisplayText should strip markdown syntax', () => {
      nodeManager.initializeFromLegacyData([
        {
          id: 'header-node',
          type: 'text',
          content: '## This is a header',
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          autoFocus: false
        }
      ]);

      const displayText = nodeManager.getNodeDisplayText('header-node');

      expect(displayText).toBe('This is a header');
    });

    test('updateNodeContentWithProcessing should update content and header level', () => {
      nodeManager.initializeFromLegacyData([
        {
          id: 'update-test-node',
          type: 'text',
          content: 'Regular text',
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          autoFocus: false
        }
      ]);

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

      nodeManager.initializeFromLegacyData([
        {
          id: 'complex-node',
          type: 'text',
          content: complexContent,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          autoFocus: false
        }
      ]);

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

        testNodes.push({
          id: `perf-node-${i}`,
          type: 'text',
          content: content,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          autoFocus: false
        });
      }

      nodeManager.initializeFromLegacyData(testNodes);
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
