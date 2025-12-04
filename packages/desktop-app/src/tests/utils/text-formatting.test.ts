/**
 * Unit tests for text-formatting utility
 *
 * Tests the formatTabTitle function which is used across BaseNodeViewer
 * and NavigationService to ensure consistent tab title formatting.
 */
import { describe, it, expect } from 'vitest';
import { formatTabTitle, MAX_TAB_TITLE_LENGTH } from '$lib/utils/text-formatting';

describe('text-formatting utils', () => {
  describe('formatTabTitle', () => {
    describe('short titles', () => {
      it('should return short title as-is', () => {
        const result = formatTabTitle('Short title');
        expect(result).toBe('Short title');
      });

      it('should trim whitespace from short titles', () => {
        const result = formatTabTitle('  Trimmed  ');
        expect(result).toBe('Trimmed');
      });

      it('should handle empty string with default fallback', () => {
        const result = formatTabTitle('');
        expect(result).toBe('Untitled');
      });

      it('should handle whitespace-only string with default fallback', () => {
        const result = formatTabTitle('   ');
        expect(result).toBe('Untitled');
      });

      it('should use custom fallback when provided', () => {
        const result = formatTabTitle('', 'Custom Fallback');
        expect(result).toBe('Custom Fallback');
      });
    });

    describe('long titles', () => {
      it('should truncate title exceeding MAX_TAB_TITLE_LENGTH', () => {
        const longTitle =
          'This is a very long title that exceeds the maximum length allowed for tab titles';
        const result = formatTabTitle(longTitle);

        expect(result.length).toBe(MAX_TAB_TITLE_LENGTH);
        expect(result).toBe('This is a very long title that exceed...');
        expect(result.endsWith('...')).toBe(true);
      });

      it('should truncate at exactly MAX_TAB_TITLE_LENGTH characters', () => {
        const longTitle = 'a'.repeat(100);
        const result = formatTabTitle(longTitle);

        expect(result.length).toBe(MAX_TAB_TITLE_LENGTH);
        expect(result.endsWith('...')).toBe(true);
      });

      it('should not truncate title at MAX_TAB_TITLE_LENGTH boundary', () => {
        const exactLength = 'a'.repeat(MAX_TAB_TITLE_LENGTH);
        const result = formatTabTitle(exactLength);

        expect(result).toBe(exactLength);
        expect(result.endsWith('...')).toBe(false);
      });

      it('should truncate title one character over MAX_TAB_TITLE_LENGTH', () => {
        const oneOverLength = 'a'.repeat(MAX_TAB_TITLE_LENGTH + 1);
        const result = formatTabTitle(oneOverLength);

        expect(result.length).toBe(MAX_TAB_TITLE_LENGTH);
        expect(result.endsWith('...')).toBe(true);
      });
    });

    describe('multi-line content', () => {
      it('should extract first line from multi-line content', () => {
        const multiLine = 'First line\nSecond line\nThird line';
        const result = formatTabTitle(multiLine);

        expect(result).toBe('First line');
      });

      it('should extract and truncate long first line from multi-line content', () => {
        const longFirstLine = 'a'.repeat(60) + '\nSecond line';
        const result = formatTabTitle(longFirstLine);

        expect(result.length).toBe(MAX_TAB_TITLE_LENGTH);
        expect(result.endsWith('...')).toBe(true);
      });

      it('should handle empty first line with default fallback', () => {
        const emptyFirstLine = '\nSecond line\nThird line';
        const result = formatTabTitle(emptyFirstLine);

        expect(result).toBe('Untitled');
      });

      it('should trim whitespace from extracted first line', () => {
        const paddedFirstLine = '  First line  \nSecond line';
        const result = formatTabTitle(paddedFirstLine);

        expect(result).toBe('First line');
      });
    });

    describe('edge cases', () => {
      it('should handle unicode characters correctly', () => {
        const unicode = '日本語のタイトル';
        const result = formatTabTitle(unicode);

        expect(result).toBe(unicode);
      });

      it('should handle emojis correctly', () => {
        const emoji = '📝 My Notes 📋';
        const result = formatTabTitle(emoji);

        expect(result).toBe(emoji);
      });

      it('should truncate long unicode content', () => {
        const longUnicode = '日'.repeat(50);
        const result = formatTabTitle(longUnicode);

        expect(result.length).toBe(MAX_TAB_TITLE_LENGTH);
        expect(result.endsWith('...')).toBe(true);
      });

      it('should handle special characters', () => {
        const special = 'Title with <html> & "quotes" \' and more';
        const result = formatTabTitle(special);

        expect(result).toBe(special);
      });

      it('should handle newline variations (CRLF)', () => {
        const crlf = 'First line\r\nSecond line';
        const result = formatTabTitle(crlf);

        expect(result).toBe('First line');
      });
    });

    describe('consistency with MAX_TAB_TITLE_LENGTH constant', () => {
      it('should export MAX_TAB_TITLE_LENGTH = 40', () => {
        expect(MAX_TAB_TITLE_LENGTH).toBe(40);
      });

      it('should use MAX_TAB_TITLE_LENGTH for truncation', () => {
        const longTitle = 'a'.repeat(100);
        const result = formatTabTitle(longTitle);

        // Verify the result respects the constant
        expect(result.length).toBe(MAX_TAB_TITLE_LENGTH);

        // Verify ellipsis is included in the length
        const contentLength = result.length - 3; // subtract ellipsis
        expect(contentLength).toBe(MAX_TAB_TITLE_LENGTH - 3);
      });
    });

    describe('real-world scenarios', () => {
      it('should format typical note title', () => {
        const note = 'Meeting notes from Oct 31, 2025';
        const result = formatTabTitle(note);

        expect(result).toBe(note);
      });

      it('should format and truncate long document title', () => {
        const doc =
          'Project Specification Document for Q4 2025 Roadmap Planning and Implementation Strategy';
        const result = formatTabTitle(doc);

        expect(result).toBe('Project Specification Document for Q4...');
        expect(result.length).toBe(MAX_TAB_TITLE_LENGTH);
      });

      it('should extract first line from markdown content and strip header syntax', () => {
        const markdown = '# Header Title\n\nSome content here\n\nMore content';
        const result = formatTabTitle(markdown);

        // Markdown header syntax (# symbols) are stripped for cleaner tab titles
        expect(result).toBe('Header Title');
      });

      it('should handle node content with prefixes', () => {
        const taskContent = '- [ ] Complete code review';
        const result = formatTabTitle(taskContent);

        expect(result).toBe('- [ ] Complete code review');
      });
    });
  });
});
