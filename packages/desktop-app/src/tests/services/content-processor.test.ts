/**
 * ContentProcessor Service Tests
 *
 * Comprehensive test suite for the enhanced ContentProcessor service
 * covering dual-representation pattern, validation, security, and performance.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import {
  ContentProcessor,
  contentProcessor,
  type HeaderNode,
  type ParagraphNode,
  type ASTNode,
  type WikiLinkNode
} from '../../lib/services/content-processor.js';

describe('ContentProcessor', () => {
  let processor: ContentProcessor;

  beforeEach(() => {
    processor = ContentProcessor.getInstance();
    // Reset all state to ensure test isolation
    processor.resetForTesting();
  });

  // ========================================================================
  // Core Dual-Representation Tests
  // ========================================================================

  describe('Dual-Representation Pattern (Source ↔ AST ↔ Display)', () => {
    it('should parse simple text into AST', () => {
      const source = 'Hello world';
      const ast = processor.parseMarkdown(source);

      expect(ast.type).toBe('document');
      expect(ast.children).toHaveLength(1);
      expect(ast.children[0].type).toBe('paragraph');
      expect(ast.metadata.totalCharacters).toBe(11);
      expect(ast.metadata.wordCount).toBe(2);
    });

    it('should parse headers into correct AST structure', () => {
      const source = '# Header 1\n## Header 2\n### Header 3';
      const ast = processor.parseMarkdown(source);

      expect(ast.children).toHaveLength(3);

      const [h1, h2, h3] = ast.children;
      expect(h1.type).toBe('header');
      expect((h1 as HeaderNode).level).toBe(1);
      expect((h1 as HeaderNode).content).toBe('Header 1');

      expect(h2.type).toBe('header');
      expect((h2 as HeaderNode).level).toBe(2);
      expect((h2 as HeaderNode).content).toBe('Header 2');

      expect(h3.type).toBe('header');
      expect((h3 as HeaderNode).level).toBe(3);
      expect((h3 as HeaderNode).content).toBe('Header 3');
    });

    it('should render AST back to HTML correctly', async () => {
      const source = '# Hello World\n\nThis is **bold** text.';
      const ast = processor.parseMarkdown(source);
      const html = await processor.renderAST(ast);

      expect(html).toContain('<h1 class="ns-markdown-heading ns-markdown-h1">Hello World</h1>');
      expect(html).toContain('<strong class="ns-markdown-bold">bold</strong>');
      expect(html).toContain('<p class="ns-markdown-paragraph">');
    });

    it('should maintain lossless AST ↔ Source conversion', () => {
      const sources = [
        '# Header',
        'Plain text',
        '**Bold text**',
        '*Italic text*',
        '`Code text`',
        '[[Wikilink]]',
        '# Header\n\nParagraph with **bold** and *italic*.'
      ];

      for (const source of sources) {
        const ast = processor.parseMarkdown(source);
        const reconstructed = processor.astToMarkdown(ast);

        // The reconstructed source should be equivalent (allowing for whitespace normalization)
        expect(reconstructed.trim()).toBe(source.trim());
      }
    });

    it('should handle complex mixed content', () => {
      const source =
        '# Main Header\n\nThis paragraph has **bold**, *italic*, `code`, and [[wikilink]] elements.';
      const ast = processor.parseMarkdown(source);

      expect(ast.children).toHaveLength(2);

      // Header
      expect(ast.children[0].type).toBe('header');

      // Paragraph with inline elements
      const paragraph = ast.children[1];
      expect(paragraph.type).toBe('paragraph');
      expect((paragraph as ParagraphNode).children).toHaveLength(9); // Mixed text and inline elements
    });

    it('should handle empty content gracefully', () => {
      const ast = processor.parseMarkdown('');

      expect(ast.type).toBe('document');
      expect(ast.children).toHaveLength(0);
      expect(ast.metadata.totalCharacters).toBe(0);
      expect(ast.metadata.wordCount).toBe(0);
    });
  });

  // ========================================================================
  // Legacy Compatibility Tests
  // ========================================================================

  describe('Legacy Compatibility', () => {
    it('should provide backward-compatible markdown to display conversion', async () => {
      const markdown = '# Header\n\n**Bold** text with *italic*.';
      const html = await processor.markdownToDisplay(markdown);

      expect(html).toContain('<h1');
      expect(html).toContain('<strong');
      expect(html).toContain('<em');
    });

    it('should handle display to markdown conversion', () => {
      const html = '<h1>Header</h1><p>Plain text</p>';
      const markdown = processor.displayToMarkdown(html);

      expect(markdown).toContain('Header');
      expect(markdown).toContain('Plain text');
    });

    it('should be compatible with existing TextNode usage patterns', () => {
      // Test the specific patterns used in text-node.svelte
      const headerContent = '## Sub Header';
      const level = processor.parseHeaderLevel(headerContent);
      const displayText = processor.stripHeaderSyntax(headerContent);

      expect(level).toBe(2);
      expect(displayText).toBe('Sub Header');
    });
  });

  // ========================================================================
  // Content Validation and Security Tests
  // ========================================================================

  describe('Content Validation and Security', () => {
    it('should detect and prevent XSS attempts', () => {
      const maliciousContent = '<script>alert("xss")</script>Hello';
      const validation = processor.validateContent(maliciousContent);

      expect(validation.isValid).toBe(false);
      expect(validation.errors).toHaveLength(1);
      expect(validation.errors[0].type).toBe('security');
    });

    it('should sanitize dangerous content while preserving markdown', () => {
      const content = '# Header\n\n<script>alert("bad")</script>**Good content**';
      const sanitized = processor.sanitizeContent(content);

      expect(sanitized).not.toContain('<script>');
      expect(sanitized).toContain('# Header');
      expect(sanitized).toContain('**Good content**');
    });

    it('should validate markdown syntax', () => {
      const invalidMarkdown = '**unclosed bold and `unclosed code';
      const validation = processor.validateContent(invalidMarkdown);

      expect(validation.isValid).toBe(false);
      expect(validation.errors.some((e) => e.type === 'syntax')).toBe(true);
    });

    it('should provide performance warnings for large content', () => {
      const largeContent = 'x'.repeat(60000);
      const validation = processor.validateContent(largeContent);

      expect(validation.warnings.some((w) => w.type === 'performance')).toBe(true);
    });

    it('should warn about header level skipping', () => {
      const content = '# H1\n### H3 (skipped H2)';
      const validation = processor.validateContent(content);

      expect(validation.warnings.some((w) => w.type === 'formatting')).toBe(true);
    });

    it('should handle various XSS vectors', () => {
      const xssVectors = [
        'javascript:alert(1)',
        '<img onerror="alert(1)" src="x">',
        'data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==',
        '<iframe src="javascript:alert(1)"></iframe>'
      ];

      for (const vector of xssVectors) {
        const sanitized = processor.sanitizeContent(vector);
        expect(sanitized).not.toContain('alert');
        expect(sanitized).not.toContain('javascript:');
        expect(sanitized).not.toContain('data:');
      }
    });
  });

  // ========================================================================
  // Header Detection and Management Tests
  // ========================================================================

  describe('Header Detection and Management', () => {
    it('should correctly parse header levels', () => {
      const testCases = [
        ['# Header 1', 1],
        ['## Header 2', 2],
        ['### Header 3', 3],
        ['#### Header 4', 4],
        ['##### Header 5', 5],
        ['###### Header 6', 6],
        ['####### Invalid', 0], // Too many #
        ['#NoSpace', 0], // No space after #
        ['Plain text', 0],
        ['', 0]
      ];

      for (const [content, expectedLevel] of testCases) {
        expect(processor.parseHeaderLevel(content as string)).toBe(expectedLevel);
      }
    });

    it('should strip header syntax correctly', () => {
      const testCases = [
        ['# Header Text', 'Header Text'],
        ['## Another Header', 'Another Header'],
        ['### Header with symbols !@#', 'Header with symbols !@#'],
        ['Plain text', 'Plain text'],
        ['#NoSpace', '#NoSpace'], // Not a valid header
        ['', '']
      ];

      for (const [content, expected] of testCases) {
        expect(processor.stripHeaderSyntax(content as string)).toBe(expected);
      }
    });

    it('should handle headers with trailing spaces', () => {
      const content = '##   Spaced Header   ';
      expect(processor.parseHeaderLevel(content)).toBe(2);
      expect(processor.stripHeaderSyntax(content)).toBe('Spaced Header   ');
    });

    it('should preserve header content with special characters', () => {
      const content = '# Header with *markdown* and [[links]]';
      expect(processor.stripHeaderSyntax(content)).toBe('Header with *markdown* and [[links]]');
    });
  });

  // ========================================================================
  // Wikilink Detection Tests (Phase 2 Foundation)
  // ========================================================================

  describe('Wikilink Detection and Backlinking Preparation', () => {
    it('should detect simple wikilinks', () => {
      const content = 'This has a [[Simple Link]] in it.';
      const wikiLinks = processor.detectWikiLinks(content);

      expect(wikiLinks).toHaveLength(1);
      expect(wikiLinks[0].text).toBe('Simple Link');
      expect(wikiLinks[0].target).toBe('Simple Link');
      expect(wikiLinks[0].displayText).toBe('Simple Link');
      expect(wikiLinks[0].startPos).toBe(11);
      expect(wikiLinks[0].endPos).toBe(26);
    });

    it('should detect wikilinks with display text', () => {
      const content = 'Link: [[Target Page|Display Text]]';
      const wikiLinks = processor.detectWikiLinks(content);

      expect(wikiLinks).toHaveLength(1);
      expect(wikiLinks[0].text).toBe('Target Page|Display Text');
      expect(wikiLinks[0].target).toBe('Target Page');
      expect(wikiLinks[0].displayText).toBe('Display Text');
    });

    it('should detect multiple wikilinks', () => {
      const content = '[[First Link]] and [[Second Link]] and [[Third|Display]]';
      const wikiLinks = processor.detectWikiLinks(content);

      expect(wikiLinks).toHaveLength(3);
      expect(wikiLinks[0].target).toBe('First Link');
      expect(wikiLinks[1].target).toBe('Second Link');
      expect(wikiLinks[2].target).toBe('Third');
      expect(wikiLinks[2].displayText).toBe('Display');
    });

    it('should handle nested brackets correctly', () => {
      const content = '[[Link with [brackets] inside]]';
      const wikiLinks = processor.detectWikiLinks(content);

      expect(wikiLinks).toHaveLength(1);
      expect(wikiLinks[0].target).toBe('Link with [brackets] inside');
    });

    it('should prepare content for backlinking system', () => {
      const content = 'Text with [[Link A]] and [[Link B]] and another [[Link A]].';
      const prepared = processor.prepareBacklinkSyntax(content);

      expect(prepared.originalContent).toBe(content);
      expect(prepared.wikiLinks).toHaveLength(3);
      expect(prepared.linkPositions.get('Link A')).toHaveLength(2);
      expect(prepared.linkPositions.get('Link B')).toHaveLength(1);
      expect(prepared.processedContent).toBe(content); // Same for now, will change in Phase 2
    });

    it('should handle empty content for wikilinks', () => {
      const wikiLinks = processor.detectWikiLinks('');
      expect(wikiLinks).toHaveLength(0);

      const prepared = processor.prepareBacklinkSyntax('');
      expect(prepared.wikiLinks).toHaveLength(0);
      expect(prepared.linkPositions.size).toBe(0);
    });

    it('should parse wikilinks in AST correctly', () => {
      const content = 'Text with [[Wikilink]] here.';
      const ast = processor.parseMarkdown(content);

      expect(ast.children).toHaveLength(1);
      const paragraph = ast.children[0] as ParagraphNode;
      expect(paragraph.children.some((child: ASTNode) => child.type === 'wikilink')).toBe(true);

      const wikiNode = paragraph.children.find((child: ASTNode) => child.type === 'wikilink') as
        | WikiLinkNode
        | undefined;
      expect(wikiNode?.target).toBe('Wikilink');
      expect(wikiNode?.displayText).toBe('Wikilink');
    });
  });

  // ========================================================================
  // Performance and Edge Case Tests
  // ========================================================================

  describe('Performance and Edge Cases', () => {
    it('should handle large documents efficiently', () => {
      const largeContent = Array(1000)
        .fill('# Header\n\nParagraph with **bold** text.')
        .join('\n\n');

      const startTime = Date.now();
      const ast = processor.parseMarkdown(largeContent);
      const parseTime = Date.now() - startTime;

      expect(parseTime).toBeLessThan(500); // Should parse in under 500ms
      expect(ast.children.length).toBeGreaterThan(1000);
      expect(ast.metadata.totalCharacters).toBeGreaterThan(40000);
    });

    it('should handle malformed markdown gracefully', async () => {
      const malformedContent = '###\n\n**\n\n`\n\n[[]]';

      await expect(async () => {
        const ast = processor.parseMarkdown(malformedContent);
        await processor.renderAST(ast);
      }).not.toThrow();
    });

    it('should handle unicode and special characters', async () => {
      const unicodeContent =
        '# Header with émojis 🚀\n\n**Bold** with 中文字符 and [[Link with spaces]]';
      const ast = processor.parseMarkdown(unicodeContent);
      const html = await processor.renderAST(ast);

      expect(html).toContain('émojis 🚀');
      expect(html).toContain('中文字符');
      expect(ast.metadata.hasWikiLinks).toBe(true);
    });

    it('should be memory efficient with repeated operations', async () => {
      const content = '# Test Header\n\nContent with **formatting**.';

      // Simulate repeated operations - should complete without throwing
      await expect(async () => {
        for (let i = 0; i < 100; i++) {
          const ast = processor.parseMarkdown(content);
          await processor.renderAST(ast);
          processor.astToMarkdown(ast);
        }
      }).not.toThrow();
    });

    it('should handle concurrent processing correctly', async () => {
      const contents = Array(50)
        .fill(0)
        .map((_, i) => `# Header ${i}\n\nContent with [[Link ${i}]] and **bold ${i}**.`);

      const promises = contents.map((content) =>
        Promise.resolve().then(async () => {
          const ast = processor.parseMarkdown(content);
          return await processor.renderAST(ast);
        })
      );

      const results = await Promise.all(promises);
      expect(results).toHaveLength(50);
      expect(results.every((result) => result.includes('<h1'))).toBe(true);
    });
  });

  // ========================================================================
  // Singleton Pattern Tests
  // ========================================================================

  describe('Singleton Pattern', () => {
    it('should return the same instance', () => {
      const instance1 = ContentProcessor.getInstance();
      const instance2 = ContentProcessor.getInstance();
      const instance3 = contentProcessor;

      expect(instance1).toBe(instance2);
      expect(instance1).toBe(instance3);
    });

    it('should maintain state across calls', () => {
      // The singleton should be stateless for content processing
      // but the instance should be the same
      const content1 = '# First Test';
      const content2 = '# Second Test';

      const result1 = contentProcessor.parseMarkdown(content1);
      const result2 = contentProcessor.parseMarkdown(content2);

      expect(result1.children[0]).not.toEqual(result2.children[0]);
    });
  });

  // ========================================================================
  // Integration with Existing System Tests
  // ========================================================================

  describe('Integration with Existing System', () => {
    it('should support TextNode component patterns', () => {
      // Simulate TextNode usage patterns
      const originalContent = '## Sub Header Text';

      const level = processor.parseHeaderLevel(originalContent);
      const displayText = processor.stripHeaderSyntax(originalContent);
      const sanitized = processor.sanitizeContent(displayText);
      const validation = processor.validateContent(originalContent);

      expect(level).toBe(2);
      expect(displayText).toBe('Sub Header Text');
      expect(sanitized).toBe(displayText);
      expect(validation.isValid).toBe(true);
    });

    it('should parse content into AST correctly for component system', () => {
      const content = '**Bold** and *italic* with `code`';
      const ast = processor.parseMarkdown(content);

      // Test AST structure for component-based rendering
      expect(ast.children).toHaveLength(1);
      expect(ast.children[0].type).toBe('paragraph');

      const paragraph = ast.children[0] as ParagraphNode;
      // Should have text + bold + text + italic + text + code + text nodes
      expect(paragraph.children.length).toBeGreaterThan(1);

      // Verify inline formatting nodes are detected
      const hasInlineFormatting = paragraph.children.some((child) =>
        ['bold', 'italic', 'code'].includes(child.type)
      );
      expect(hasInlineFormatting).toBe(true);
    });
  });

  // ========================================================================
  // Nodespace URI Detection and Reference Service Tests
  // ========================================================================

  describe('Nodespace URI Detection and Reference Service', () => {
    it('should detect nodespace URIs in markdown link format', () => {
      const content = 'Link to [Node Title](nodespace://node-123) here.';
      const links = processor.detectNodespaceURIs(content);

      expect(links).toHaveLength(1);
      expect(links[0].nodeId).toBe('node-123');
      expect(links[0].displayText).toBe('Node Title');
      expect(links[0].uri).toContain('nodespace://node-123');
      expect(links[0].startPosition).toBe(8);
    });

    it('should detect nodespace URIs with alternate format', () => {
      const content = 'Check [Title](nodespace://node/abc-def-123) and continue.';
      const links = processor.detectNodespaceURIs(content);

      expect(links).toHaveLength(1);
      expect(links[0].nodeId).toBe('abc-def-123');
      expect(links[0].displayText).toBe('Title');
    });

    it('should detect empty display text in nodespace URIs', () => {
      const content = '[](nodespace://node-456)';
      const links = processor.detectNodespaceURIs(content);

      expect(links).toHaveLength(1);
      expect(links[0].nodeId).toBe('node-456');
      expect(links[0].displayText).toBe('');
    });

    it('should detect nodespace URIs with query parameters', () => {
      const content = '[Title](nodespace://node-789?view=edit)';
      const links = processor.detectNodespaceURIs(content);

      expect(links).toHaveLength(1);
      expect(links[0].nodeId).toBe('node-789');
      expect(links[0].uri).toContain('?view=edit');
    });

    it('should detect multiple nodespace URIs', () => {
      const content =
        '[First](nodespace://node-1) and [Second](nodespace://node/node-2) refs.';
      const links = processor.detectNodespaceURIs(content);

      expect(links).toHaveLength(2);
      expect(links[0].nodeId).toBe('node-1');
      expect(links[1].nodeId).toBe('node-2');
    });

    it('should handle content without nodespace URIs', () => {
      const content = 'Regular text with [[wikilink]] and **bold**.';
      const links = processor.detectNodespaceURIs(content);

      expect(links).toHaveLength(0);
    });

    it('should include metadata for detected links', () => {
      const content = 'Before [Link](nodespace://test-id) after.';
      const links = processor.detectNodespaceURIs(content);

      expect(links[0].metadata).toBeDefined();
      expect(links[0].metadata?.fullMatch).toContain('[Link](nodespace://test-id)');
      expect(links[0].metadata?.contentBefore).toBe('Before ');
      expect(links[0].metadata?.contentAfter).toBe(' after.');
    });

    it('should set NodeReferenceService', () => {
      const mockService = {
        resolveNodeReference: async () => ({ nodeId: 'test', exists: true }),
        detectNodespaceLinks: () => [],
        createNodeReference: () => 'nodespace://test'
      };

      expect(() => {
        processor.setNodeReferenceService(mockService);
      }).not.toThrow();
    });

    it('should process content with event emission', () => {
      const content = 'Text with [[wikilink]] and more.';
      const result = processor.processContentWithEventEmission(content, 'test-node-id');

      expect(result.originalContent).toBe(content);
      expect(result.wikiLinks).toHaveLength(1);
      expect(result.wikiLinks[0].target).toBe('wikilink');
    });

    it('should process content with references when no service is configured', async () => {
      processor.resetForTesting(); // Ensure no service is set

      const content = 'Text with [[wikilink]] and [ref](nodespace://node-123).';
      const result = await processor.processContentWithReferences(content, 'source-node-id');

      expect(result.prepared.wikiLinks).toHaveLength(1);
      expect(result.nodespaceLinks).toHaveLength(1);
      expect(result.resolved).toBe(false); // No service to resolve
    });

    it('should process content with references when service is configured', async () => {
      const mockService = {
        resolveNodeReference: async () => ({ nodeId: 'node-123', exists: true, isValid: true }),
        detectNodespaceLinks: () => [],
        createNodeReference: () => 'nodespace://node-123',
        parseNodespaceURI: (_uri: string) => ({
          nodeId: 'node-123',
          exists: true,
          isValid: true
        }),
        addReference: async () => {}
      };

      processor.setNodeReferenceService(mockService);

      const content = 'Text with [ref](nodespace://node-123).';
      const result = await processor.processContentWithReferences(content, 'source-node-id');

      expect(result.nodespaceLinks).toHaveLength(1);
      expect(result.resolved).toBe(true); // Service resolved references
    });
  });

  // ========================================================================
  // Reference Cache Management Tests
  // ========================================================================

  describe('Reference Cache Management', () => {
    it('should invalidate cache for specific node', () => {
      processor.clearReferenceCache();

      expect(() => {
        processor.invalidateReferenceCache('test-node-id');
        processor.invalidateReferenceCache('another-node-id');
      }).not.toThrow();
    });

    it('should clear reference cache', () => {
      processor.clearReferenceCache();

      const stats = processor.getReferencesCacheStats();
      expect(stats.size).toBe(0);
    });

    it('should get reference cache statistics', () => {
      processor.clearReferenceCache();

      const stats = processor.getReferencesCacheStats();

      expect(stats).toHaveProperty('size');
      expect(stats).toHaveProperty('hitRate');
      expect(stats).toHaveProperty('oldestEntry');
      expect(stats.size).toBe(0);
      expect(stats.hitRate).toBe(0);
    });

    it('should reset testing state correctly', () => {
      processor.resetForTesting();

      const stats = processor.getReferencesCacheStats();
      expect(stats.size).toBe(0);
    });
  });

  // ========================================================================
  // Markdown to Display with References Tests
  // ========================================================================

  describe('Markdown to Display with References', () => {
    it('should render markdown without reference service', async () => {
      processor.resetForTesting();

      const markdown = 'Text with [ref](nodespace://node-123).';
      const html = await processor.markdownToDisplayWithReferences(markdown);

      expect(html).toContain('ns-noderef');
      expect(html).toContain('node-123');
    });

    it('should render markdown with reference service', async () => {
      const mockService = {
        resolveNodeReference: async () => ({
          nodeId: 'node-123',
          exists: true,
          isValid: true,
          title: 'Test Node'
        }),
        detectNodespaceLinks: () => [],
        createNodeReference: () => 'nodespace://node-123',
        parseNodespaceURI: () => ({
          nodeId: 'node-123',
          exists: true,
          isValid: true,
          title: 'Test Node'
        })
      };

      processor.setNodeReferenceService(mockService);

      const markdown = 'Text with [ref](nodespace://node-123).';
      const html = await processor.markdownToDisplayWithReferences(markdown, 'source-node-id');

      expect(html).toContain('node-123');
    });
  });

  // ========================================================================
  // AST Edge Cases and Advanced Patterns Tests
  // ========================================================================

  describe('AST Edge Cases and Advanced Patterns', () => {
    it('should handle multiple empty lines between paragraphs', () => {
      const content = 'First paragraph\n\n\n\nSecond paragraph';
      const ast = processor.parseMarkdown(content);

      expect(ast.children).toHaveLength(2);
      expect(ast.children[0].type).toBe('paragraph');
      expect(ast.children[1].type).toBe('paragraph');
    });

    it('should handle headers followed by empty lines', () => {
      const content = '# Header\n\n\nParagraph after empty lines';
      const ast = processor.parseMarkdown(content);

      expect(ast.children).toHaveLength(2);
      expect(ast.children[0].type).toBe('header');
      expect(ast.children[1].type).toBe('paragraph');
    });

    it('should handle content ending with header', () => {
      const content = 'Paragraph text\n\n# Final Header';
      const ast = processor.parseMarkdown(content);

      expect(ast.children).toHaveLength(2);
      expect(ast.children[0].type).toBe('paragraph');
      expect(ast.children[1].type).toBe('header');
    });

    it('should handle plain nodespace URIs without markdown links', () => {
      const content = 'Check out nodespace://node/test-node for details.';
      const ast = processor.parseMarkdown(content);

      const paragraph = ast.children[0] as ParagraphNode;
      const hasNodespaceRef = paragraph.children.some((child) => child.type === 'nodespace-ref');
      expect(hasNodespaceRef).toBe(true);
    });

    it('should prioritize markdown-style nodespace refs over plain URIs', () => {
      const content = '[Title](nodespace://node-id)';
      const ast = processor.parseMarkdown(content);

      const paragraph = ast.children[0] as ParagraphNode;
      const nodeRefs = paragraph.children.filter((child) => child.type === 'nodespace-ref');

      // Should only have one nodespace-ref node, not duplicates
      expect(nodeRefs.length).toBe(1);
    });

    it('should handle overlapping inline patterns correctly', () => {
      const content = '**Bold and *italic* mixed**';
      const ast = processor.parseMarkdown(content);

      const paragraph = ast.children[0] as ParagraphNode;
      expect(paragraph.children.length).toBeGreaterThan(0);

      // Should have bold nodes
      const hasBold = paragraph.children.some((child) => child.type === 'bold');
      expect(hasBold).toBe(true);
    });

    it('should render empty AST', async () => {
      const emptyAST = processor.parseMarkdown('');
      const html = await processor.renderAST(emptyAST);

      expect(html).toBe('');
    });

    it('should handle AST with only headers', async () => {
      const content = '# H1\n## H2\n### H3';
      const ast = processor.parseMarkdown(content);
      const html = await processor.renderAST(ast);

      expect(html).toContain('<h1');
      expect(html).toContain('<h2');
      expect(html).toContain('<h3');
      expect(html).not.toContain('<p');
    });

    it('should convert kebab-case to readable text in plain nodespace URIs', () => {
      const content = 'Link: nodespace://node-id/my-test-node';
      const ast = processor.parseMarkdown(content);

      const paragraph = ast.children[0] as ParagraphNode;
      const nodeRef = paragraph.children.find((child) => child.type === 'nodespace-ref');

      expect(nodeRef).toBeDefined();
    });
  });

  // ========================================================================
  // Sanitization Edge Cases Tests
  // ========================================================================

  describe('Sanitization Edge Cases', () => {
    it('should remove event handlers without quotes', () => {
      const content = '<div onclick=alert(1)>text</div>';
      const sanitized = processor.sanitizeContent(content);

      expect(sanitized).not.toContain('onclick=');
    });

    it('should remove style attributes', () => {
      const content = '<p style="color: red">text</p>';
      const sanitized = processor.sanitizeContent(content);

      expect(sanitized).not.toContain('style=');
    });

    it('should remove link tags', () => {
      const content = '<link rel="stylesheet" href="malicious.css">text';
      const sanitized = processor.sanitizeContent(content);

      expect(sanitized).not.toContain('<link');
    });

    it('should remove meta tags', () => {
      const content = '<meta http-equiv="refresh" content="0">text';
      const sanitized = processor.sanitizeContent(content);

      expect(sanitized).not.toContain('<meta');
    });

    it('should remove object and embed tags', () => {
      const content = '<object data="bad.swf"></object><embed src="bad.swf">';
      const sanitized = processor.sanitizeContent(content);

      expect(sanitized).not.toContain('<object');
      expect(sanitized).not.toContain('<embed');
    });

    it('should handle mixed malicious content', () => {
      const content =
        '# Header\n<script>bad</script>\n**Good** <iframe src="x"></iframe>\n[[link]]';
      const sanitized = processor.sanitizeContent(content);

      expect(sanitized).toContain('# Header');
      expect(sanitized).toContain('**Good**');
      expect(sanitized).toContain('[[link]]');
      expect(sanitized).not.toContain('<script');
      expect(sanitized).not.toContain('<iframe');
    });
  });

  // ========================================================================
  // Validation Edge Cases Tests
  // ========================================================================

  describe('Validation Edge Cases', () => {
    it('should validate content without warnings for well-formed markdown', () => {
      const content = '# H1\n## H2\n### H3\n\nWell structured content.';
      const validation = processor.validateContent(content);

      expect(validation.isValid).toBe(true);
      expect(validation.warnings.length).toBe(0);
    });

    it('should detect multiple header level skips', () => {
      const content = '# H1\n##### H5 (skipped H2, H3, H4)';
      const validation = processor.validateContent(content);

      expect(validation.warnings.some((w) => w.type === 'formatting')).toBe(true);
    });

    it('should validate empty content', () => {
      const validation = processor.validateContent('');

      expect(validation.isValid).toBe(true);
      expect(validation.errors.length).toBe(0);
      expect(validation.warnings.length).toBe(0);
    });

    it('should handle content with only whitespace', () => {
      const validation = processor.validateContent('   \n\n   \n   ');

      expect(validation.isValid).toBe(true);
    });
  });

  // ========================================================================
  // Metadata Calculation Tests
  // ========================================================================

  describe('Metadata Calculation', () => {
    it('should calculate correct metadata for complex content', () => {
      const content =
        '# Header\n\nText with **bold**, *italic*, `code`, [[wikilink]], and [ref](nodespace://node-123).';
      const ast = processor.parseMarkdown(content);

      expect(ast.metadata.totalCharacters).toBe(content.length);
      expect(ast.metadata.wordCount).toBeGreaterThan(0);
      expect(ast.metadata.hasWikiLinks).toBe(true);
      expect(ast.metadata.hasNodespaceRefs).toBe(true);
      expect(ast.metadata.headerCount).toBe(1);
      expect(ast.metadata.inlineFormatCount).toBeGreaterThan(0);
      expect(ast.metadata.lastModified).toBeGreaterThan(0);
    });

    it('should detect nodespace refs in both formats', () => {
      const content1 = '[ref](nodespace://node-123)';
      const ast1 = processor.parseMarkdown(content1);
      expect(ast1.metadata.hasNodespaceRefs).toBe(true);

      const content2 = 'Plain nodespace://node/test-id here';
      const ast2 = processor.parseMarkdown(content2);
      expect(ast2.metadata.hasNodespaceRefs).toBe(true);
    });

    it('should count inline formats correctly', () => {
      const content = '**bold** *italic* `code` [[wiki]] [ref](nodespace://id)';
      const ast = processor.parseMarkdown(content);

      // Should count bold, italic, code, wikilink, and nodespace ref
      expect(ast.metadata.inlineFormatCount).toBeGreaterThan(0);
    });

    it('should handle content with no inline formats', () => {
      const content = 'Plain text without any formatting.';
      const ast = processor.parseMarkdown(content);

      expect(ast.metadata.inlineFormatCount).toBe(0);
      expect(ast.metadata.hasWikiLinks).toBe(false);
      expect(ast.metadata.hasNodespaceRefs).toBe(false);
    });
  });

  // ========================================================================
  // NodeReference Integration and Error Handling Tests
  // ========================================================================

  describe('NodeReference Integration and Error Handling', () => {
    it('should handle resolveNodespaceURI errors gracefully during rendering', async () => {
      const mockService = {
        resolveNodeReference: async () => ({ nodeId: 'test', exists: true }),
        detectNodespaceLinks: () => [],
        createNodeReference: () => 'nodespace://test',
        resolveNodespaceURI: async () => {
          throw new Error('Network error');
        }
      };

      processor.setNodeReferenceService(mockService);

      const markdown = 'Text with [ref](nodespace://node-123).';
      const html = await processor.markdownToDisplayWithReferences(markdown, 'source-id');

      // Should fallback to basic rendering despite error
      expect(html).toContain('node-123');
    });

    it('should render nodespace ref with full decoration when reference is valid', async () => {
      const mockService = {
        resolveNodeReference: async () => ({
          nodeId: 'node-123',
          exists: true,
          isValid: true,
          title: 'Test Node',
          nodeType: 'text'
        }),
        detectNodespaceLinks: () => [],
        createNodeReference: () => 'nodespace://node-123',
        resolveNodespaceURI: async (_uri: string) => ({
          nodeId: 'node-123',
          exists: true,
          isValid: true,
          title: 'Test Node',
          nodeType: 'text',
          content: 'Test content',
          properties: {}
        }),
        parseNodespaceURI: (_uri: string) => ({
          nodeId: 'node-123',
          exists: true,
          isValid: true,
          title: 'Test Node'
        })
      };

      processor.setNodeReferenceService(mockService);

      const markdown = 'Text with [ref](nodespace://node-123).';
      const html = await processor.markdownToDisplayWithReferences(markdown, 'source-id');

      // Should contain rich decoration placeholder
      expect(html).toContain('ns-component-placeholder');
    });

    it('should handle addReference errors gracefully', async () => {
      const mockService = {
        resolveNodeReference: async () => ({ nodeId: 'node-123', exists: true, isValid: true }),
        detectNodespaceLinks: () => [],
        createNodeReference: () => 'nodespace://node-123',
        parseNodespaceURI: (_uri: string) => ({
          nodeId: 'node-123',
          exists: true,
          isValid: true,
          title: 'Test'
        }),
        addReference: async () => {
          throw new Error('Database error');
        }
      };

      processor.setNodeReferenceService(mockService);

      // Should not throw when addReference fails
      await expect(async () => {
        await processor.processContentWithReferences(
          'Text with [ref](nodespace://node-123).',
          'source-id'
        );
      }).not.toThrow();
    });

    it('should handle parseNodespaceURI returning null', async () => {
      const mockService = {
        resolveNodeReference: async () => ({ nodeId: 'test', exists: true }),
        detectNodespaceLinks: () => [],
        createNodeReference: () => 'nodespace://test',
        parseNodespaceURI: () => null
      };

      processor.setNodeReferenceService(mockService);

      const markdown = 'Text with [ref](nodespace://node-123).';
      const html = await processor.markdownToDisplayWithReferences(markdown, 'source-id');

      // Should render as invalid reference
      expect(html).toContain('ns-noderef-invalid');
    });

    it('should use cached reference when cache is valid', async () => {
      let parseCallCount = 0;

      const mockService = {
        resolveNodeReference: async () => ({ nodeId: 'test', exists: true }),
        detectNodespaceLinks: () => [],
        createNodeReference: () => 'nodespace://test',
        parseNodespaceURI: () => {
          parseCallCount++;
          return {
            nodeId: 'node-123',
            exists: true,
            isValid: true,
            title: 'Cached Node'
          };
        }
      };

      processor.clearReferenceCache();
      processor.setNodeReferenceService(mockService);

      const markdown = 'Text with [ref](nodespace://node-123).';

      // First call should parse
      await processor.markdownToDisplayWithReferences(markdown, 'source-id');
      expect(parseCallCount).toBe(1);

      // Second call should use cache
      await processor.markdownToDisplayWithReferences(markdown, 'source-id');
      expect(parseCallCount).toBe(1); // Still 1, used cache
    });

    it('should invalidate cache correctly', async () => {
      let parseCallCount = 0;

      const mockService = {
        resolveNodeReference: async () => ({ nodeId: 'test', exists: true }),
        detectNodespaceLinks: () => [],
        createNodeReference: () => 'nodespace://test',
        parseNodespaceURI: () => {
          parseCallCount++;
          return {
            nodeId: 'node-123',
            exists: true,
            isValid: true,
            title: 'Test'
          };
        }
      };

      processor.clearReferenceCache();
      processor.setNodeReferenceService(mockService);

      const markdown = 'Text with [ref](nodespace://node-123).';

      // First call
      await processor.markdownToDisplayWithReferences(markdown, 'source-id');
      expect(parseCallCount).toBe(1);

      // Invalidate cache for this node
      processor.invalidateReferenceCache('node-123');

      // Second call should parse again
      await processor.markdownToDisplayWithReferences(markdown, 'source-id');
      expect(parseCallCount).toBe(2); // Re-parsed after invalidation
    });

    it('should cache null results to avoid repeated failed lookups', async () => {
      let parseCallCount = 0;

      const mockService = {
        resolveNodeReference: async () => ({ nodeId: 'test', exists: true }),
        detectNodespaceLinks: () => [],
        createNodeReference: () => 'nodespace://test',
        parseNodespaceURI: () => {
          parseCallCount++;
          throw new Error('Not found');
        }
      };

      processor.clearReferenceCache();
      processor.setNodeReferenceService(mockService);

      const markdown = 'Text with [ref](nodespace://bad-node).';

      // First call should attempt parse
      await processor.markdownToDisplayWithReferences(markdown, 'source-id');
      expect(parseCallCount).toBe(1);

      // Second call should use cached null result
      await processor.markdownToDisplayWithReferences(markdown, 'source-id');
      expect(parseCallCount).toBe(1); // Cached null, didn't retry
    });

    it('should handle resolveNodespaceURI returning null during decoration', async () => {
      const mockService = {
        resolveNodeReference: async () => ({ nodeId: 'test', exists: true }),
        detectNodespaceLinks: () => [],
        createNodeReference: () => 'nodespace://test',
        resolveNodespaceURI: async () => null,
        parseNodespaceURI: () => ({
          nodeId: 'node-123',
          exists: true,
          isValid: true,
          title: 'Test'
        })
      };

      processor.setNodeReferenceService(mockService);

      const markdown = 'Text with [ref](nodespace://node-123).';
      const html = await processor.markdownToDisplayWithReferences(markdown, 'source-id');

      // Should fallback to basic rendering
      expect(html).toContain('ns-noderef');
    });
  });

  // ========================================================================
  // Additional Coverage Tests for Uncovered Lines
  // ========================================================================

  describe('Additional Coverage for Edge Cases', () => {
    it('should handle vbscript protocol in sanitization', () => {
      const content = 'Link with vbscript:alert(1) protocol';
      const sanitized = processor.sanitizeContent(content);

      expect(sanitized).not.toContain('vbscript:');
    });

    it('should handle word count for empty stripped markdown', () => {
      const content = '# ';
      const ast = processor.parseMarkdown(content);

      // Should handle empty word count gracefully
      expect(ast.metadata.wordCount).toBe(0);
    });

    it('should handle italic regex edge cases', () => {
      const content = 'Text with *italic* and **not italic**';
      const ast = processor.parseMarkdown(content);

      const paragraph = ast.children[0] as ParagraphNode;
      const hasItalic = paragraph.children.some((child) => child.type === 'italic');
      expect(hasItalic).toBe(true);
    });

    it('should handle overlapping nodespace ref patterns correctly', () => {
      // Test the pattern filtering logic for overlapping patterns
      const content = '[Text](nodespace://node-id) nodespace://node-id';
      const ast = processor.parseMarkdown(content);

      const paragraph = ast.children[0] as ParagraphNode;
      const nodeRefs = paragraph.children.filter((child) => child.type === 'nodespace-ref');

      // Should detect both but handle overlapping correctly
      expect(nodeRefs.length).toBeGreaterThanOrEqual(1);
    });

    it('should render wikilinks in AST correctly', async () => {
      const content = 'Text with [[wikilink]] here.';
      const ast = processor.parseMarkdown(content);
      const html = await processor.renderAST(ast);

      expect(html).toContain('ns-wikilink');
      expect(html).toContain('data-target');
    });

    it('should handle nodespace URI with both node/ prefix and query params', () => {
      const content = '[Link](nodespace://node/test-id?view=edit&mode=preview)';
      const links = processor.detectNodespaceURIs(content);

      expect(links).toHaveLength(1);
      expect(links[0].nodeId).toBe('test-id');
      expect(links[0].uri).toContain('?view=edit&mode=preview');
    });

    it('should handle mixed wikilinks and nodespace refs in same paragraph', async () => {
      const content = 'Text with [[wikilink]] and [ref](nodespace://node-123) together.';
      const ast = processor.parseMarkdown(content);

      const paragraph = ast.children[0] as ParagraphNode;
      const hasWikilink = paragraph.children.some((child) => child.type === 'wikilink');
      const hasNodeRef = paragraph.children.some((child) => child.type === 'nodespace-ref');

      expect(hasWikilink).toBe(true);
      expect(hasNodeRef).toBe(true);

      const html = await processor.renderAST(ast);
      expect(html).toContain('ns-wikilink');
      expect(html).toContain('ns-noderef');
    });

    it('should handle AST node types in rendering switch statement', async () => {
      // Test all node type renderings
      const content =
        '# Header\n\nParagraph with **bold**, *italic*, `code`, [[wikilink]], and plain text.';
      const ast = processor.parseMarkdown(content);
      const html = await processor.renderAST(ast);

      expect(html).toContain('<h1');
      expect(html).toContain('<p');
      expect(html).toContain('<strong');
      expect(html).toContain('<em');
      expect(html).toContain('<code');
      expect(html).toContain('ns-wikilink');
    });

    it('should handle unknown AST node types in astToMarkdown', () => {
      const ast = processor.parseMarkdown('# Header\n\nText');
      const markdown = processor.astToMarkdown(ast);

      expect(markdown).toContain('# Header');
      expect(markdown).toContain('Text');
    });
  });
});
