/**
 * Integration Tests: TextNode + ContentProcessor
 *
 * Verify that TextNode component successfully integrates with
 * the new ContentProcessor service without breaking existing functionality.
 */

import { describe, it, expect } from 'vitest';
import { contentProcessor } from '../../lib/services/contentProcessor.js';

describe('TextNode + ContentProcessor Integration', () => {
  describe('Header Processing Integration', () => {
    it('should parse header levels correctly for TextNode usage', () => {
      const testCases = [
        ['# Main Header', 1, 'Main Header'],
        ['## Sub Header', 2, 'Sub Header'],
        ['### Section Header', 3, 'Section Header'],
        ['Plain text', 0, 'Plain text'],
        ['', 0, '']
      ];

      for (const [content, expectedLevel, expectedDisplay] of testCases) {
        const level = contentProcessor.parseHeaderLevel(content as string);
        const display = contentProcessor.stripHeaderSyntax(content as string);

        expect(level).toBe(expectedLevel);
        expect(display).toBe(expectedDisplay);
      }
    });

    it('should handle header syntax detection as used in TextNode keyboard handler', () => {
      // Simulate TextNode's header detection logic
      const testInputs = ['#', '##', '###', '####', '#####', '######'];

      for (const input of testInputs) {
        // Simulate adding a space and some content to complete header syntax
        const completed = input + ' test';
        const level = contentProcessor.parseHeaderLevel(completed);

        expect(level).toBe(input.length);
        expect(level).toBeGreaterThan(0);
        expect(level).toBeLessThanOrEqual(6);
      }
    });
  });

  describe('Content Validation Integration', () => {
    it('should validate content as used in TextNode handleContentChange', () => {
      const testContents = [
        'Normal text content',
        '# Header with content',
        '**Bold** and *italic* text',
        '[[Wikilink]] content',
        'Content with <script>alert("bad")</script>'
      ];

      for (const content of testContents) {
        const sanitized = contentProcessor.sanitizeContent(content);
        const validation = contentProcessor.validateContent(sanitized);

        // Should sanitize dangerous content
        expect(sanitized).not.toContain('<script>');

        // Most content should be valid after sanitization
        if (!content.includes('<script>')) {
          expect(validation.isValid).toBe(true);
        }
      }
    });

    it('should handle empty content gracefully as TextNode does', () => {
      const emptyContent = '';

      const level = contentProcessor.parseHeaderLevel(emptyContent);
      const display = contentProcessor.stripHeaderSyntax(emptyContent);
      const sanitized = contentProcessor.sanitizeContent(emptyContent);
      const validation = contentProcessor.validateContent(emptyContent);

      expect(level).toBe(0);
      expect(display).toBe('');
      expect(sanitized).toBe('');
      expect(validation.isValid).toBe(true);
      expect(validation.errors).toHaveLength(0);
    });
  });

  describe('Performance Integration', () => {
    it('should handle typical TextNode content efficiently', () => {
      const typicalContents = [
        '# Document Title',
        'A paragraph of normal text content that might exist in a typical document.',
        '## Section Header',
        'More content with **bold text** and *italic text*.',
        'Content with [[Internal Link]] and `inline code`.',
        '### Subsection',
        'Final paragraph with mixed content and [[Another Link|Display Text]].'
      ];

      const startTime = Date.now();

      for (const content of typicalContents) {
        // Simulate TextNode operations
        const level = contentProcessor.parseHeaderLevel(content);
        const display = contentProcessor.stripHeaderSyntax(content);
        const sanitized = contentProcessor.sanitizeContent(content);
        const validation = contentProcessor.validateContent(sanitized);

        // Basic integrity checks
        expect(typeof level).toBe('number');
        expect(typeof display).toBe('string');
        expect(typeof sanitized).toBe('string');
        expect(typeof validation.isValid).toBe('boolean');
      }

      const processingTime = Date.now() - startTime;
      expect(processingTime).toBeLessThan(10); // Should be very fast for typical content
    });

    it('should handle rapid content changes as in real editing', () => {
      const baseContent = 'Editing content';
      const variations = [
        baseContent,
        '# ' + baseContent,
        '## ' + baseContent,
        baseContent + ' with more text',
        '**' + baseContent + '**',
        '*' + baseContent + '*',
        '[[' + baseContent + ']]',
        '`' + baseContent + '`'
      ];

      const startTime = Date.now();

      // Simulate rapid content changes
      for (let i = 0; i < 100; i++) {
        const content = variations[i % variations.length];

        contentProcessor.parseHeaderLevel(content);
        contentProcessor.stripHeaderSyntax(content);
        contentProcessor.sanitizeContent(content);
      }

      const processingTime = Date.now() - startTime;
      expect(processingTime).toBeLessThan(50); // Should handle 100 operations quickly
    });
  });

  describe('Wikilink Foundation Integration', () => {
    it('should detect wikilinks for future backlinking system', () => {
      const contentWithLinks = 'Document with [[Important Note]] and [[Project Plan|Plan]].';

      const wikiLinks = contentProcessor.detectWikiLinks(contentWithLinks);
      const prepared = contentProcessor.prepareBacklinkSyntax(contentWithLinks);

      expect(wikiLinks).toHaveLength(2);
      expect(wikiLinks[0].target).toBe('Important Note');
      expect(wikiLinks[1].target).toBe('Project Plan');
      expect(wikiLinks[1].displayText).toBe('Plan');

      expect(prepared.wikiLinks).toHaveLength(2);
      expect(prepared.linkPositions.size).toBe(2);
      expect(prepared.originalContent).toBe(contentWithLinks);
    });

    it('should parse wikilinks in AST for future processing', async () => {
      const content = 'Text with [[Linked Content]] here.';

      const ast = contentProcessor.parseMarkdown(content);
      const html = await contentProcessor.renderAST(ast);

      expect(ast.metadata.hasWikiLinks).toBe(true);
      expect(html).toContain('ns-wikilink');
      expect(html).toContain('data-target="Linked Content"');
    });
  });

  describe('Dual-Representation Foundation', () => {
    it('should maintain content integrity through AST round-trip', () => {
      const testContents = [
        '# Header Content',
        'Plain paragraph text',
        'Text with **bold** and *italic*',
        'Content with [[Wikilinks]]',
        '## Header\n\nParagraph content'
      ];

      for (const content of testContents) {
        const ast = contentProcessor.parseMarkdown(content);
        const reconstructed = contentProcessor.astToMarkdown(ast);

        // Should maintain structural integrity
        expect(reconstructed.trim()).toBe(content.trim());
      }
    });

    it('should provide consistent HTML output for display', async () => {
      const content = '# Test Header\n\nContent with **bold** text and [[Link]].';

      const ast = contentProcessor.parseMarkdown(content);
      const html = await contentProcessor.renderAST(ast);

      // Should produce valid HTML with proper CSS classes
      expect(html).toContain('<h1 class="ns-markdown-heading ns-markdown-h1">Test Header</h1>');
      expect(html).toContain('<p class="ns-markdown-paragraph">');
      expect(html).toContain('<strong class="ns-markdown-bold">bold</strong>');
      expect(html).toContain('<span class="ns-wikilink"');
    });
  });

  describe('Error Handling Integration', () => {
    it('should handle malformed content gracefully', () => {
      const malformedContents = [
        '###', // Header without content
        '**', // Unclosed bold
        '[[', // Unclosed wikilink
        '`', // Unclosed code
        null as unknown as string, // Invalid input
        undefined as unknown as string // Invalid input
      ];

      for (const content of malformedContents) {
        expect(() => {
          const safeContent = content || '';
          contentProcessor.parseHeaderLevel(safeContent);
          contentProcessor.stripHeaderSyntax(safeContent);
          contentProcessor.sanitizeContent(safeContent);
          contentProcessor.validateContent(safeContent);
        }).not.toThrow();
      }
    });
  });
});
