/**
 * Test for marked.js integration
 * Validates that marked.js properly handles edge cases that the custom parser struggled with
 */

import { describe, it, expect } from 'vitest';
import { markdownToHtml, htmlToMarkdown } from '$lib/utils/markedConfig.js';

describe('marked.js Integration', () => {
  describe('Edge cases that custom parser failed on', () => {
    it('should handle nested formatting correctly', () => {
      const testCases = [
        {
          markdown: '*__bold__*',
          expected: '<span class="markdown-italic"><span class="markdown-bold">bold</span></span>'
        },
        {
          markdown: '**_italic_**',
          expected: '<span class="markdown-bold"><span class="markdown-italic">italic</span></span>'
        },
        {
          markdown: '_italic with **bold** inside_',
          expected:
            '<span class="markdown-italic">italic with <span class="markdown-bold">bold</span> inside</span>'
        },
        {
          markdown: '**bold with *italic* inside**',
          expected:
            '<span class="markdown-bold">bold with <span class="markdown-italic">italic</span> inside</span>'
        }
      ];

      testCases.forEach(({ markdown, expected }) => {
        const result = markdownToHtml(markdown);
        expect(result).toBe(expected);
      });
    });

    it('should handle mixed marker types correctly', () => {
      const testCases = [
        {
          markdown: '***bold italic***',
          expected:
            '<span class="markdown-italic"><span class="markdown-bold">bold italic</span></span>'
        },
        {
          markdown: '___bold italic___',
          expected:
            '<span class="markdown-italic"><span class="markdown-bold">bold italic</span></span>'
        }
      ];

      testCases.forEach(({ markdown, expected }) => {
        const result = markdownToHtml(markdown);
        expect(result).toBe(expected);
      });
    });

    it('should handle incomplete formatting gracefully', () => {
      const testCases = [
        {
          markdown: '*not closed',
          expected: '*not closed'
        },
        {
          markdown: 'no *formatting here',
          expected: 'no *formatting here'
        },
        {
          markdown: '*empty**content*',
          expected: '<span class="markdown-italic">empty**content</span>'
        }
      ];

      testCases.forEach(({ markdown, expected }) => {
        const result = markdownToHtml(markdown);
        expect(result).toBe(expected);
      });
    });
  });

  describe('Round-trip conversion', () => {
    it('should maintain consistency for simple formatting', () => {
      const testCases = ['**bold**', '*italic*', '***bold italic***'];

      testCases.forEach((markdown) => {
        const html = markdownToHtml(markdown);
        const backToMarkdown = htmlToMarkdown(html);
        expect(backToMarkdown).toBe(markdown);
      });
    });

    it('should normalize equivalent syntax patterns', () => {
      // These different syntaxes should normalize to the same result
      const equivalentPairs = [
        { input: '*__bold__*', normalized: '***bold***' },
        { input: '**_italic_**', normalized: '***italic***' }
      ];

      equivalentPairs.forEach(({ input, normalized }) => {
        const html = markdownToHtml(input);
        const result = htmlToMarkdown(html);
        expect(result).toBe(normalized);
      });
    });
  });

  describe('CSS class preservation', () => {
    it('should use NodeSpace CSS classes instead of standard HTML', () => {
      const testCases = [
        {
          markdown: '**bold**',
          shouldContain: 'class="markdown-bold"',
          shouldNotContain: '<strong>'
        },
        {
          markdown: '*italic*',
          shouldContain: 'class="markdown-italic"',
          shouldNotContain: '<em>'
        }
      ];

      testCases.forEach(({ markdown, shouldContain, shouldNotContain }) => {
        const result = markdownToHtml(markdown);
        expect(result).toContain(shouldContain);
        expect(result).not.toContain(shouldNotContain);
      });
    });
  });

  describe('Header processing (CRITICAL: ensure headers are NOT processed)', () => {
    it('should preserve header syntax as plain text - NodeSpace handles headers separately', () => {
      const headerCases = [
        {
          input: '# Welcome to NodeSpace',
          expected: '# Welcome to NodeSpace',
          description: 'H1 header should remain as plain text'
        },
        {
          input: '## Subheader',
          expected: '## Subheader',
          description: 'H2 header should remain as plain text'
        },
        {
          input: '### Third level',
          expected: '### Third level',
          description: 'H3 header should remain as plain text'
        }
      ];

      headerCases.forEach(({ input, expected, description }) => {
        const result = markdownToHtml(input);
        expect(result, description).toBe(expected);

        // Additional check: should NOT contain HTML header tags
        expect(result).not.toMatch(/<h[1-6][^>]*>/);
        expect(result).not.toMatch(/<\/h[1-6]>/);
      });
    });

    it('should handle headers with inline formatting correctly', () => {
      const mixedCases = [
        {
          input: '# Welcome to **NodeSpace**',
          expected: '# Welcome to <span class="markdown-bold">NodeSpace</span>',
          description: 'Header with bold should only format the bold part'
        },
        {
          input: '## *Italic* header text',
          expected: '## <span class="markdown-italic">Italic</span> header text',
          description: 'Header with italic should only format the italic part'
        }
      ];

      mixedCases.forEach(({ input, expected, description }) => {
        const result = markdownToHtml(input);
        expect(result, description).toBe(expected);

        // Verify header symbols are preserved
        expect(result).toMatch(/^#{1,6}\s/);
        // Verify no HTML header tags are created
        expect(result).not.toMatch(/<h[1-6][^>]*>/);
      });
    });

    it('should maintain header syntax in round-trip conversion', () => {
      const headerWithFormatting = '# Welcome to **NodeSpace** with *style*';

      // Forward conversion
      const html = markdownToHtml(headerWithFormatting);
      expect(html).toBe(
        '# Welcome to <span class="markdown-bold">NodeSpace</span> with <span class="markdown-italic">style</span>'
      );

      // Backward conversion
      const backToMarkdown = htmlToMarkdown(html);
      expect(backToMarkdown).toBe(headerWithFormatting);

      // Should still have # at the beginning
      expect(backToMarkdown).toMatch(/^#\s/);
    });
  });

  describe('Performance and reliability', () => {
    it('should handle large text efficiently', () => {
      const largeText = 'This is **bold** and *italic* text. '.repeat(1000);
      const start = Date.now();
      const result = markdownToHtml(largeText);
      const duration = Date.now() - start;

      // Should complete within reasonable time (less than 100ms for this size)
      expect(duration).toBeLessThan(100);
      expect(result).toContain('class="markdown-bold"');
      expect(result).toContain('class="markdown-italic"');
    });

    it('should handle malformed input without crashing', () => {
      const malformedCases = ['**bold*', '*italic**', '***mixed**', '__**bold*_', ''];

      malformedCases.forEach((input) => {
        // Should not throw errors
        expect(() => markdownToHtml(input)).not.toThrow();
        expect(() => htmlToMarkdown(markdownToHtml(input))).not.toThrow();
      });
    });
  });
});
