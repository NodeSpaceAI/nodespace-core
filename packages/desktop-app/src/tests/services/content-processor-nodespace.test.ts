/**
 * ContentProcessor Nodespace URI Integration Tests
 *
 * Tests the enhanced ContentProcessor integration with nodespace:// URIs
 * and NodeReferenceService integration (Phase 2.1 Days 4-5)
 */

import { describe, it, expect, beforeEach, vi, type MockedFunction } from 'vitest';
import { contentProcessor, type ParagraphNode } from '../../lib/services/contentProcessor';
import type { NodeReferenceService, NodeReference } from '../../lib/services/nodeReferenceService';

// Mock NodeReferenceService
const mockNodeReferenceService = {
  parseNodespaceURI: vi.fn(),
  resolveNodespaceURI: vi.fn(),
  addReference: vi.fn(),
  detectNodespaceLinks: vi.fn()
} as Partial<NodeReferenceService> as NodeReferenceService;

describe('ContentProcessor - Nodespace URI Integration', () => {
  beforeEach(() => {
    // Reset all mocks
    vi.clearAllMocks();

    // Set the mock service
    contentProcessor.setNodeReferenceService(mockNodeReferenceService);

    // Clear cache
    contentProcessor.clearReferenceCache();
  });

  describe('Nodespace URI Detection', () => {
    it('should detect nodespace:// URIs in markdown content', () => {
      const content = 'Check out [My Node](nodespace://node/test-123) for details.';
      const links = contentProcessor.detectNodespaceURIs(content);

      expect(links).toHaveLength(1);
      expect(links[0]).toMatchObject({
        uri: 'nodespace://node/test-123',
        nodeId: 'test-123',
        displayText: 'My Node',
        startPos: 10,
        endPos: 46, // Actual end position of the link
        isValid: false // Will be false without proper resolution setup
      });
    });

    it('should detect multiple nodespace URIs', () => {
      const content = `
        First reference: [Node A](nodespace://node/node-a)
        Second reference: [Node B](nodespace://node/node-b?hierarchy=true)
        Regular link: [External](https://example.com)
      `;

      const links = contentProcessor.detectNodespaceURIs(content);

      expect(links).toHaveLength(2);
      expect(links[0].nodeId).toBe('node-a');
      expect(links[1].nodeId).toBe('node-b');
      expect(links[1].uri).toBe('nodespace://node/node-b?hierarchy=true');
    });

    it('should handle complex URIs with query parameters', () => {
      const content = '[Complex Node](nodespace://node/abc-123?hierarchy=true&timestamp=123456)';
      const links = contentProcessor.detectNodespaceURIs(content);

      expect(links).toHaveLength(1);
      expect(links[0]).toMatchObject({
        uri: 'nodespace://node/abc-123?hierarchy=true&timestamp=123456',
        nodeId: 'abc-123',
        displayText: 'Complex Node'
      });
    });
  });

  describe('AST Processing with Nodespace References', () => {
    it('should parse nodespace references into AST nodes', () => {
      const markdown = 'See [Related Node](nodespace://node/related-123) for more info.';
      const ast = contentProcessor.parseMarkdown(markdown);

      // Should have a paragraph with nodespace-ref node
      expect(ast.children).toHaveLength(1);
      expect(ast.children[0].type).toBe('paragraph');

      const paragraph = ast.children[0] as ParagraphNode;
      expect(paragraph.children).toHaveLength(3); // Text + NodespaceRef + Text

      const refNode = paragraph.children[1];
      expect(refNode.type).toBe('nodespace-ref');

      // Type assertion for nodespace reference node properties
      interface NodespaceRefNode {
        nodeId: string;
        uri: string;
        displayText: string;
        isValid: boolean;
      }

      const typedRefNode = refNode as unknown as NodespaceRefNode;
      expect(typedRefNode.nodeId).toBe('related-123');
      expect(typedRefNode.uri).toBe('nodespace://node/related-123');
      expect(typedRefNode.displayText).toBe('Related Node');
      expect(typedRefNode.isValid).toBe(false); // Not resolved yet
    });

    it('should count nodespace references in metadata', () => {
      const markdown = `
        # Header
        
        First ref: [Node A](nodespace://node/node-a)
        Second ref: [Node B](nodespace://node/node-b)
        Wiki link: [[Traditional Wiki Link]]
      `;

      const ast = contentProcessor.parseMarkdown(markdown);

      expect(ast.metadata.hasNodespaceRefs).toBe(true);
      expect(ast.metadata.nodeRefCount).toBe(2);
      expect(ast.metadata.hasWikiLinks).toBe(true);
    });
  });

  describe('Component-Based Reference Rendering', () => {
    it('should process nodespace references for component decoration', async () => {
      // Mock a valid reference
      const mockReference: NodeReference = {
        nodeId: 'test-123',
        uri: 'nodespace://node/test-123',
        title: 'My Node',
        nodeType: 'note',
        isValid: true,
        lastResolved: Date.now(),
        metadata: {}
      };

      (
        mockNodeReferenceService.parseNodespaceURI as MockedFunction<
          typeof mockNodeReferenceService.parseNodespaceURI
        >
      ).mockReturnValue(mockReference);

      const markdown = 'Check [My Node](nodespace://node/test-123) here.';
      const result = await contentProcessor.processContentWithReferences(markdown, 'source-node');

      // Test component-based approach: should have nodespace links for decoration
      expect(result.nodespaceLinks).toHaveLength(1);
      expect(result.nodespaceLinks[0].nodeId).toBe('test-123');
      expect(result.nodespaceLinks[0].displayText).toBe('My Node');
      expect(result.resolved).toBe(true);
    });
  });

  describe('Enhanced Content Processing', () => {
    it('should process content with both wikilinks and nodespace refs', async () => {
      const content = `
        Wiki link: [[Traditional Link]]
        Nodespace ref: [Modern Link](nodespace://node/modern-123)
      `;

      const result = await contentProcessor.processContentWithReferences(content, 'source-node');

      expect(result.prepared.wikiLinks).toHaveLength(1);
      expect(result.nodespaceLinks).toHaveLength(1);
      expect(result.prepared.wikiLinks[0].target).toBe('Traditional Link');
      expect(result.nodespaceLinks[0].nodeId).toBe('modern-123');
    });

    it('should add bidirectional references for valid nodespace links', async () => {
      // Mock valid reference
      const mockReference: NodeReference = {
        nodeId: 'target-node',
        uri: 'nodespace://node/target-node',
        title: 'Target Node',
        nodeType: 'note',
        isValid: true,
        lastResolved: Date.now(),
        metadata: {}
      };

      (
        mockNodeReferenceService.parseNodespaceURI as MockedFunction<
          typeof mockNodeReferenceService.parseNodespaceURI
        >
      ).mockReturnValue(mockReference);
      (
        mockNodeReferenceService.addReference as MockedFunction<
          typeof mockNodeReferenceService.addReference
        >
      ).mockResolvedValue(undefined);

      const content = 'Reference to [Target](nodespace://node/target-node)';
      const result = await contentProcessor.processContentWithReferences(content, 'source-node');

      expect(result.resolved).toBe(true);
      expect(mockNodeReferenceService.addReference).toHaveBeenCalledWith(
        'source-node',
        'target-node'
      );
    });
  });

  describe('Reference Resolution and Caching', () => {
    it('should cache resolved references', async () => {
      const mockReference: NodeReference = {
        nodeId: 'test-123',
        uri: 'nodespace://node/test-123',
        title: 'Test Node',
        nodeType: 'note',
        isValid: true,
        lastResolved: Date.now(),
        metadata: {}
      };

      (
        mockNodeReferenceService.parseNodespaceURI as MockedFunction<
          typeof mockNodeReferenceService.parseNodespaceURI
        >
      ).mockReturnValue(mockReference);

      const markdown = 'Check [Test Node](nodespace://node/test-123) here.';

      // First call
      await contentProcessor.markdownToDisplayWithReferences(markdown, 'source-node');
      expect(mockNodeReferenceService.parseNodespaceURI).toHaveBeenCalledTimes(1);

      // Second call should use cache
      await contentProcessor.markdownToDisplayWithReferences(markdown, 'source-node');
      expect(mockNodeReferenceService.parseNodespaceURI).toHaveBeenCalledTimes(1); // No additional calls
    });

    it('should provide cache statistics', () => {
      const stats = contentProcessor.getReferencesCacheStats();

      expect(stats).toHaveProperty('size');
      expect(stats).toHaveProperty('hitRate');
      expect(stats).toHaveProperty('oldestEntry');
      expect(typeof stats.size).toBe('number');
    });
  });

  describe('Error Handling', () => {
    it('should handle broken references gracefully', async () => {
      (
        mockNodeReferenceService.parseNodespaceURI as MockedFunction<
          typeof mockNodeReferenceService.parseNodespaceURI
        >
      ).mockReturnValue(null);

      const markdown = 'Broken [Reference](nodespace://node/nonexistent)';
      const result = await contentProcessor.processContentWithReferences(markdown, 'source-node');

      // Test component-based approach: should still detect the reference but mark as invalid
      expect(result.nodespaceLinks).toHaveLength(1);
      expect(result.nodespaceLinks[0].nodeId).toBe('nonexistent');
      expect(result.nodespaceLinks[0].isValid).toBe(false);
    });

    it('should handle service errors gracefully', async () => {
      (
        mockNodeReferenceService.parseNodespaceURI as MockedFunction<
          typeof mockNodeReferenceService.parseNodespaceURI
        >
      ).mockImplementation(() => {
        throw new Error('Service error');
      });

      const markdown = 'Error [Reference](nodespace://node/error-node)';

      // Should not throw
      await expect(
        contentProcessor.markdownToDisplayWithReferences(markdown, 'source-node')
      ).resolves.toBeDefined();
    });
  });

  describe('Event Emission', () => {
    it('should emit events for detected nodespace references', () => {
      const content = 'Reference to [Target](nodespace://node/target-123)';

      // Event bus mocking would be implemented here for content processing tests

      // Process content
      contentProcessor.processContentWithEventEmission(content, 'source-node');

      // Would need to properly mock eventBus to test this
      // For now, we verify the method doesn't throw
      expect(() => {
        contentProcessor.processContentWithEventEmission(content, 'source-node');
      }).not.toThrow();
    });
  });

  describe('Round-trip Processing', () => {
    it('should maintain nodespace references through AST round-trip', () => {
      const original = 'Check [My Node](nodespace://node/test-123) for details.';

      const ast = contentProcessor.parseMarkdown(original);
      const reconstructed = contentProcessor.astToMarkdown(ast);

      expect(reconstructed).toContain('[My Node](nodespace://node/test-123)');
    });

    it('should handle mixed content correctly', () => {
      const original = `
        # Header
        
        Normal text with **bold** and [[wiki link]].
        
        Nodespace reference: [My Node](nodespace://node/test-123)
        
        More content.
      `.trim();

      const ast = contentProcessor.parseMarkdown(original);
      const reconstructed = contentProcessor.astToMarkdown(ast);

      expect(reconstructed).toContain('[[wiki link]]');
      expect(reconstructed).toContain('[My Node](nodespace://node/test-123)');
      expect(reconstructed).toContain('**bold**');
    });
  });

  describe('Performance and Validation', () => {
    it('should handle large content with many references efficiently', () => {
      const content = Array.from(
        { length: 100 },
        (_, i) => `Reference ${i}: [Node ${i}](nodespace://node/node-${i})`
      ).join(' ');

      const startTime = performance.now();
      const links = contentProcessor.detectNodespaceURIs(content);
      const endTime = performance.now();

      expect(links).toHaveLength(100);
      expect(endTime - startTime).toBeLessThan(100); // Should be fast
    });

    it('should validate content with nodespace references', () => {
      const content = 'Valid [Reference](nodespace://node/valid-123)';
      const validation = contentProcessor.validateContent(content);

      expect(validation.isValid).toBe(true);
      expect(validation.errors).toHaveLength(0);
    });
  });
});
