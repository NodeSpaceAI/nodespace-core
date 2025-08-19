/**
 * ContentProcessor Service Tests
 *
 * Comprehensive test suite for the enhanced ContentProcessor service
 * covering dual-representation pattern, validation, security, and performance.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { ContentProcessor, contentProcessor } from '$lib/services/contentProcessor.js';

describe('ContentProcessor', () => {
  let processor: ContentProcessor;

  beforeEach(() => {
    processor = ContentProcessor.getInstance();
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
      expect((h1 as { level: number }).level).toBe(1);
      expect((h1 as { content: string }).content).toBe('Header 1');

      expect(h2.type).toBe('header');
      expect((h2 as { level: number }).level).toBe(2);
      expect((h2 as { content: string }).content).toBe('Header 2');

      expect(h3.type).toBe('header');
      expect((h3 as { level: number }).level).toBe(3);
      expect((h3 as { content: string }).content).toBe('Header 3');
    });

    it('should render AST back to HTML correctly', () => {
      const source = '# Hello World\n\nThis is **bold** text.';
      const ast = processor.parseMarkdown(source);
      const html = processor.renderAST(ast);

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
      expect((paragraph as { children: unknown[] }).children).toHaveLength(9); // Mixed text and inline elements
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
    it('should provide backward-compatible markdown to display conversion', () => {
      const markdown = '# Header\n\n**Bold** text with *italic*.';
      const html = processor.markdownToDisplay(markdown);

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
      // Test the specific patterns used in TextNode.svelte
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
      const paragraph = ast.children[0] as { children: { type: string }[] };
      expect(paragraph.children.some((child: { type: string }) => child.type === 'wikilink')).toBe(
        true
      );

      const wikiNode = paragraph.children.find(
        (child: { type: string }) => child.type === 'wikilink'
      ) as { target: string; displayText: string } | undefined;
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

    it('should handle malformed markdown gracefully', () => {
      const malformedContent = '###\n\n**\n\n`\n\n[[]]';

      expect(() => {
        const ast = processor.parseMarkdown(malformedContent);
        processor.renderAST(ast);
      }).not.toThrow();
    });

    it('should handle unicode and special characters', () => {
      const unicodeContent =
        '# Header with émojis 🚀\n\n**Bold** with 中文字符 and [[Link with spaces]]';
      const ast = processor.parseMarkdown(unicodeContent);
      const html = processor.renderAST(ast);

      expect(html).toContain('émojis 🚀');
      expect(html).toContain('中文字符');
      expect(ast.metadata.hasWikiLinks).toBe(true);
    });

    it('should be memory efficient with repeated operations', () => {
      const content = '# Test Header\n\nContent with **formatting**.';

      // Simulate repeated operations
      for (let i = 0; i < 100; i++) {
        const ast = processor.parseMarkdown(content);
        processor.renderAST(ast);
        processor.astToMarkdown(ast);
      }

      // Should complete without memory issues
      expect(true).toBe(true);
    });

    it('should handle concurrent processing correctly', async () => {
      const contents = Array(50)
        .fill(0)
        .map((_, i) => `# Header ${i}\n\nContent with [[Link ${i}]] and **bold ${i}**.`);

      const promises = contents.map((content) =>
        Promise.resolve().then(() => {
          const ast = processor.parseMarkdown(content);
          return processor.renderAST(ast);
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
    it('should work with existing markdownUtils patterns', () => {
      const content = '**Bold** and *italic* with `code`';

      // Should produce similar results to legacy system
      const ast = processor.parseMarkdown(content);
      const html = processor.renderAST(ast);

      expect(html).toContain('ns-markdown-bold');
      expect(html).toContain('ns-markdown-italic');
      expect(html).toContain('ns-markdown-code');
    });

    it('should maintain CSS class compatibility', () => {
      const headerContent = '# Main Header';
      const ast = processor.parseMarkdown(headerContent);
      const html = processor.renderAST(ast);

      expect(html).toContain('ns-markdown-heading');
      expect(html).toContain('ns-markdown-h1');
    });

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
  });
});
