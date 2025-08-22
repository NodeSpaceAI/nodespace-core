/**
 * Cursor Positioning Algorithm Tests
 * 
 * Tests for the core cursor positioning algorithms without requiring DOM.
 * These tests validate the character mapping and position calculation logic.
 */

import { describe, it, expect } from 'vitest';

// Test the core algorithm logic by creating mock controller methods
describe('Cursor Positioning Algorithm Core Logic', () => {
  
  // Mock implementation of buildCharacterMapping algorithm
  function buildCharacterMapping(htmlText: string, markdownText: string): number[] {
    const mapping: number[] = [];
    let htmlIndex = 0;
    let markdownIndex = 0;
    
    while (htmlIndex < htmlText.length && markdownIndex < markdownText.length) {
      const htmlChar = htmlText[htmlIndex];
      const markdownChar = markdownText[markdownIndex];
      
      if (htmlChar === markdownChar) {
        // Characters match - direct mapping
        mapping[htmlIndex] = markdownIndex;
        htmlIndex++;
        markdownIndex++;
      } else {
        // Characters don't match - likely markdown syntax
        // Skip the markdown syntax character(s)
        markdownIndex++;
      }
    }
    
    // Handle remaining HTML characters (map to end)
    while (htmlIndex < htmlText.length) {
      mapping[htmlIndex] = markdownText.length;
      htmlIndex++;
    }
    
    return mapping;
  }

  // Mock implementation of mapHtmlPositionToMarkdown algorithm
  function mapHtmlPositionToMarkdown(
    htmlPosition: number,
    htmlContent: string,
    markdownContent: string
  ): number {
    // Extract plain text from HTML for comparison
    const htmlText = extractTextFromHtml(htmlContent);

    // Simple case: content matches exactly (no markdown syntax)
    if (htmlText === markdownContent) {
      return Math.min(htmlPosition, markdownContent.length);
    }

    // Check for header syntax at the beginning of markdown content
    const headerMatch = markdownContent.match(/^(#{1,6}\s+)/);
    const headerSyntaxLength = headerMatch ? headerMatch[1].length : 0;

    // Extract content after header syntax for comparison
    const markdownContentWithoutHeader = headerMatch
      ? markdownContent.substring(headerSyntaxLength)
      : markdownContent;

    // For headers, if HTML text matches content after header syntax, simple offset
    if (htmlText === markdownContentWithoutHeader) {
      // Direct mapping: add the header syntax offset to the HTML position
      const mappedPosition = Math.min(htmlPosition + headerSyntaxLength, markdownContent.length);
      return mappedPosition;
    }

    // For content with inline formatting, use character mapping
    const mapping = buildCharacterMapping(htmlText, markdownContent.substring(headerSyntaxLength));
    
    // Get the markdown position from our mapping
    const markdownPosition =
      mapping[htmlPosition] !== undefined
        ? headerSyntaxLength + mapping[htmlPosition]
        : markdownContent.length;

    return Math.min(markdownPosition, markdownContent.length);
  }

  // Mock implementation of extractTextFromHtml
  function extractTextFromHtml(htmlContent: string): string {
    // Simple text extraction for testing - removes HTML tags
    return htmlContent.replace(/<[^>]*>/g, '');
  }

  describe('Character Mapping Algorithm', () => {
    it('should create correct mapping for identical text', () => {
      const mapping = buildCharacterMapping('Hello World', 'Hello World');
      expect(mapping).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    });

    it('should handle header syntax mapping', () => {
      const mapping = buildCharacterMapping('Hello World', '# Hello World');
      // HTML: H e l l o   W o r l d
      // MD:   # _ H e l l o   W o r l d
      //       0 1 2 3 4 5 6 7 8 9 10 11 12
      expect(mapping).toEqual([2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    });

    it('should handle bold formatting mapping', () => {
      const mapping = buildCharacterMapping('Hello World', '**Hello** World');
      // HTML: H e l l o   W o r l d
      // MD:   * * H e l l o * *   W o r l d
      //       0 1 2 3 4 5 6 7 8 9 10 11 12 13 14
      expect(mapping).toEqual([2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14]);
    });

    it('should handle italic formatting mapping', () => {
      const mapping = buildCharacterMapping('Hello World', '*Hello* World');
      // HTML: H e l l o   W o r l d
      // MD:   * H e l l o *   W o r l d
      //       0 1 2 3 4 5 6 7 8 9 10 11 12
      expect(mapping).toEqual([1, 2, 3, 4, 5, 7, 8, 9, 10, 11, 12]);
    });

    it('should handle nested formatting mapping', () => {
      const mapping = buildCharacterMapping('Hello', '***Hello***');
      // HTML: H e l l o
      // MD:   * * * H e l l o * * *
      //       0 1 2 3 4 5 6 7 8 9 10
      expect(mapping).toEqual([3, 4, 5, 6, 7]);
    });

    it('should map remaining characters to end position', () => {
      const mapping = buildCharacterMapping('Hello World', 'Hello');
      // HTML has more characters than markdown
      expect(mapping).toEqual([0, 1, 2, 3, 4, 5, 5, 5, 5, 5, 5]);
    });

    it('should handle empty content', () => {
      const mapping = buildCharacterMapping('', '');
      expect(mapping).toEqual([]);
    });

    it('should handle content with only markdown syntax', () => {
      const mapping = buildCharacterMapping('', '**');
      expect(mapping).toEqual([]);
    });
  });

  describe('HTML to Markdown Position Mapping', () => {
    it('should handle identical content', () => {
      const position = mapHtmlPositionToMarkdown(5, 'Hello World', 'Hello World');
      expect(position).toBe(5);
    });

    it('should handle header syntax offset', () => {
      const position = mapHtmlPositionToMarkdown(5, 'Hello World', '# Hello World');
      // Position 5 in "Hello World" -> position 7 in "# Hello World"
      expect(position).toBe(7);
    });

    it('should handle multiple header levels', () => {
      const h2Position = mapHtmlPositionToMarkdown(5, 'Hello World', '## Hello World');
      expect(h2Position).toBe(8); // "## " = 3 characters

      const h3Position = mapHtmlPositionToMarkdown(5, 'Hello World', '### Hello World');
      expect(h3Position).toBe(9); // "### " = 4 characters
    });

    it('should clamp position to content length', () => {
      const position = mapHtmlPositionToMarkdown(100, 'Hello', '# Hi');
      expect(position).toBe(4); // Length of "# Hi"
    });

    it('should handle bold formatting with character mapping', () => {
      const position = mapHtmlPositionToMarkdown(5, 'Hello World', '**Hello** World');
      // Position 5 in "Hello World" should map to position after "**Hello**"
      expect(position).toBe(9); // After the closing "**"
    });

    it('should handle italic formatting with character mapping', () => {
      const position = mapHtmlPositionToMarkdown(5, 'Hello World', '*Hello* World');
      // Position 5 in "Hello World" should map to position after "*Hello*"
      expect(position).toBe(7); // After the closing "*"
    });
  });

  describe('Edge Cases', () => {
    it('should handle Unicode characters', () => {
      const mapping = buildCharacterMapping('Hello 🌟 World', 'Hello 🌟 World');
      // Unicode emoji may have different character counts, so just check basic structure
      expect(mapping[0]).toBe(0); // First character maps correctly
      expect(mapping[mapping.length - 1]).toBe(mapping.length - 1); // Last character maps correctly
      expect(mapping.length).toBe('Hello 🌟 World'.length); // Mapping covers all characters
    });

    it('should handle very long content efficiently', () => {
      const longText = 'a'.repeat(1000);
      const longMarkdown = '# ' + longText;

      const start = performance.now();
      const position = mapHtmlPositionToMarkdown(500, longText, longMarkdown);
      const end = performance.now();

      expect(position).toBe(502); // 500 + 2 for "# "
      expect(end - start).toBeLessThan(50); // Should complete in <50ms
    });

    it('should handle mixed header and formatting', () => {
      const position = mapHtmlPositionToMarkdown(5, 'Hello World', '## **Hello** World');
      // "## " = 3 chars, "**Hello**" = 9 chars, position 5 should be after "**Hello**"
      expect(position).toBe(12); // 3 (header) + 9 (formatted text)
    });

    it('should handle multiple bold sections', () => {
      const mapping = buildCharacterMapping('Hello Bold World', '**Hello** **Bold** World');
      // HTML: H e l l o   B o l d   W o r l d
      // MD:   * * H e l l o * *   * * B o l d * *   W o r l d
      //       0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22
      expect(mapping[0]).toBe(2); // H -> position after **
      expect(mapping[6]).toBe(12); // B -> position after ** and space in second bold section
    });
  });

  describe('Performance Requirements', () => {
    it('should perform character mapping within performance requirements', () => {
      const text = 'a'.repeat(100);
      const markdown = '# ' + text;

      const start = performance.now();
      const mapping = buildCharacterMapping(text, markdown);
      const end = performance.now();

      expect(mapping.length).toBe(text.length);
      expect(end - start).toBeLessThan(10); // Should complete in <10ms for 100 chars
    });

    it('should perform position mapping within performance requirements', () => {
      const text = 'a'.repeat(100);
      const markdown = '# ' + text;

      const start = performance.now();
      for (let i = 0; i < 10; i++) {
        mapHtmlPositionToMarkdown(50, text, markdown);
      }
      const end = performance.now();

      expect(end - start).toBeLessThan(50); // 10 operations in <50ms
    });
  });

  describe('Text Extraction', () => {
    it('should extract plain text from HTML content', () => {
      expect(extractTextFromHtml('Hello World')).toBe('Hello World');
      expect(extractTextFromHtml('<span>Hello</span> World')).toBe('Hello World');
      expect(extractTextFromHtml('<b>Bold</b> and <i>italic</i>')).toBe('Bold and italic');
      expect(extractTextFromHtml('')).toBe('');
      expect(extractTextFromHtml('<div><p>Nested <span>content</span></p></div>')).toBe('Nested content');
    });
  });
});