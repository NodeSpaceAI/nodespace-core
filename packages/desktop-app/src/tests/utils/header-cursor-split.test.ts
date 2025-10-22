/**
 * Header Cursor Position Splitting Tests
 *
 * Tests the behavior when pressing Enter at different cursor positions within header syntax:
 * - `|# Header` (cursor before header)
 * - `#| Header` (cursor within header syntax)
 * - `# |Header` (cursor after header syntax) ✅ already works
 */

import { describe, it, expect } from 'vitest';
import { patternSplitter } from '../../lib/patterns/splitter';

describe('Header Cursor Position Splitting', () => {
  describe('Fixed Behavior (now working correctly)', () => {
    it('should handle cursor before header syntax |# Header', () => {
      const content = '# Header text';
      const position = 0; // |# Header text

      const result = patternSplitter.split(content, position, 'header');

      console.log('Before header - result:', result);
      // FIXED: Original node keeps header syntax, new node gets full content

      expect(result.beforeContent).toBe('# ');
      expect(result.afterContent).toBe('# Header text');
    });

    it('should handle cursor within header syntax #| Header', () => {
      const content = '# Header text';
      const position = 1; // #| Header text

      const result = patternSplitter.split(content, position, 'header');

      console.log('Within header syntax - result:', result);
      // FIXED: Original node keeps header syntax, new node gets full content

      expect(result.beforeContent).toBe('# '); // Original node keeps header syntax
      expect(result.afterContent).toBe('# Header text'); // New node gets full header
    });

    it('should handle cursor after header syntax # | Header', () => {
      const content = '# Header text';
      const position = 2; // # |Header text

      const result = patternSplitter.split(content, position, 'header');

      console.log('After header syntax - result:', result);
      // FIXED: Both nodes get proper header syntax

      expect(result.beforeContent).toBe('# ');
      expect(result.afterContent).toBe('# Header text'); // Header syntax preserved!
    });

    it('should handle various header levels', () => {
      for (let level = 1; level <= 6; level++) {
        const headerPrefix = '#'.repeat(level) + ' ';
        const content = headerPrefix + 'Header text';

        // Test cursor at different positions within header syntax
        for (let pos = 0; pos < headerPrefix.length; pos++) {
          const result = patternSplitter.split(content, pos, 'header');
          console.log(`Level ${level}, pos ${pos}:`, result);

          // The afterContent should always preserve the full header syntax
          if (pos === 0) {
            // FIXED: Cursor before header should preserve header syntax in beforeContent
            expect(result.beforeContent).toBe(headerPrefix);
            expect(result.afterContent).toBe(content);
          } else if (pos < headerPrefix.length) {
            // Cursor within header syntax: afterContent should preserve header
            expect(result.afterContent).not.toBe(content.substring(pos));
            // We expect it to include the full header syntax
          }
        }
      }
    });
  });

  describe('Expected Behavior (what we want to achieve)', () => {
    it('should preserve header syntax when cursor is before or within header', () => {
      const content = '## Important Header';

      // Case 1: Cursor at position 0 (|## Important Header)
      const result1 = patternSplitter.split(content, 0, 'header');
      expect(result1.beforeContent).toBe('## '); // FIXED: Original node keeps header syntax
      expect(result1.afterContent).toBe('## Important Header');

      // Case 2: Cursor at position 1 (#|# Important Header)
      const result2 = patternSplitter.split(content, 1, 'header');
      expect(result2.beforeContent).toBe('## '); // Original node keeps header syntax
      expect(result2.afterContent).toBe('## Important Header'); // New node gets full header

      // Case 3: Cursor at position 2 (##| Important Header)
      const result3 = patternSplitter.split(content, 2, 'header');
      expect(result3.beforeContent).toBe('## '); // Original node keeps header syntax
      expect(result3.afterContent).toBe('## Important Header'); // New node gets full header

      // Case 4: Cursor at position 3 (## |Important Header)
      const result4 = patternSplitter.split(content, 3, 'header');
      expect(result4.beforeContent).toBe('## '); // Header syntax only
      expect(result4.afterContent).toBe('## Important Header'); // Full header with content
    });

    it('should handle mixed scenarios correctly', () => {
      // Test with content that has text before and after cursor
      const content = '### Section: Overview and Details';

      // Cursor within header syntax should preserve the header for both nodes
      const result = patternSplitter.split(content, 2, 'header'); // ##|# Section...
      expect(result.beforeContent).toBe('### '); // Original node keeps header syntax
      expect(result.afterContent).toBe('### Section: Overview and Details'); // New node gets full content
    });
  });
});
