/**
 * Prefix Inheritance Strategy Tests
 *
 * Tests splitting behavior for patterns that inherit prefixes:
 * - Headers (# , ## , ### , etc.)
 * - Ordered lists (1. )
 * - Quote blocks (> )
 */

import { describe, it, expect } from 'vitest';
import { PrefixInheritanceStrategy } from '../../../lib/patterns/splitter/strategies/prefix-inheritance';
import type { PatternTemplate } from '../../../lib/patterns/types';

describe('PrefixInheritanceStrategy', () => {
  const strategy = new PrefixInheritanceStrategy();

  describe('Header Splitting', () => {
    const headerPattern: PatternTemplate = {
      regex: /^(#{1,6})\s+/,
      nodeType: 'header',
      priority: 10,
      splittingStrategy: 'prefix-inheritance',
      prefixToInherit: '# ',
      cursorPlacement: 'after-prefix'
    };

    it('should create empty node above when cursor at start', () => {
      const content = '# Header text';
      const position = 0; // |# Header text

      const result = strategy.split(content, position, headerPattern);

      expect(result.beforeContent).toBe('# ');
      expect(result.afterContent).toBe('# Header text');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should create empty node above when cursor within syntax', () => {
      const content = '# Header text';
      const position = 1; // #| Header text

      const result = strategy.split(content, position, headerPattern);

      expect(result.beforeContent).toBe('# ');
      expect(result.afterContent).toBe('# Header text');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should split and inherit when cursor after syntax', () => {
      const content = '# Header text';
      const position = 2; // # |Header text

      const result = strategy.split(content, position, headerPattern);

      expect(result.beforeContent).toBe('# ');
      expect(result.afterContent).toBe('# Header text');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should split middle of content and inherit prefix', () => {
      const content = '# Header text';
      const position = 10; // # Header t|ext

      const result = strategy.split(content, position, headerPattern);

      expect(result.beforeContent).toBe('# Header t');
      expect(result.afterContent).toBe('# ext');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should handle various header levels', () => {
      for (let level = 1; level <= 6; level++) {
        const prefix = '#'.repeat(level) + ' ';
        const content = prefix + 'Header Level ' + level;
        const position = 5; // Within header text

        const levelPattern: PatternTemplate = {
          regex: new RegExp(`^#{${level}}\\s+`),
          nodeType: 'header',
          priority: 10,
          splittingStrategy: 'prefix-inheritance',
          prefixToInherit: prefix,
          cursorPlacement: 'after-prefix'
        };

        const result = strategy.split(content, position, levelPattern);

        expect(result.beforeContent).toContain(prefix);
        expect(result.afterContent).toContain(prefix);
      }
    });
  });

  describe('Ordered List Splitting', () => {
    const listPattern: PatternTemplate = {
      regex: /^1\.\s+/,
      nodeType: 'ordered-list',
      priority: 10,
      splittingStrategy: 'prefix-inheritance',
      prefixToInherit: '1. ',
      cursorPlacement: 'after-prefix'
    };

    it('should create empty node above when cursor at start', () => {
      const content = '1. First item';
      const position = 0; // |1. First item

      const result = strategy.split(content, position, listPattern);

      expect(result.beforeContent).toBe('1. ');
      expect(result.afterContent).toBe('1. First item');
      expect(result.newNodeCursorPosition).toBe(3);
    });

    it('should create empty node above when cursor within syntax', () => {
      const content = '1. First item';
      const position = 2; // 1.| First item

      const result = strategy.split(content, position, listPattern);

      expect(result.beforeContent).toBe('1. ');
      expect(result.afterContent).toBe('1. First item');
      expect(result.newNodeCursorPosition).toBe(3);
    });

    it('should split and inherit when cursor after syntax', () => {
      const content = '1. First item';
      const position = 9; // 1. First |item

      const result = strategy.split(content, position, listPattern);

      expect(result.beforeContent).toBe('1. First ');
      expect(result.afterContent).toBe('1. item');
      expect(result.newNodeCursorPosition).toBe(3);
    });

    it('should handle cursor at end of content', () => {
      const content = '1. First item';
      const position = 13; // 1. First item|

      const result = strategy.split(content, position, listPattern);

      expect(result.beforeContent).toBe('1. First item');
      expect(result.afterContent).toBe('1. ');
      expect(result.newNodeCursorPosition).toBe(3);
    });
  });

  describe('Quote Block Splitting', () => {
    const quotePattern: PatternTemplate = {
      regex: /^>\s+/,
      nodeType: 'quote-block',
      priority: 10,
      splittingStrategy: 'prefix-inheritance',
      prefixToInherit: '> ',
      cursorPlacement: 'after-prefix'
    };

    it('should create empty node above when cursor at start', () => {
      const content = '> A quote';
      const position = 0; // |> A quote

      const result = strategy.split(content, position, quotePattern);

      expect(result.beforeContent).toBe('> ');
      expect(result.afterContent).toBe('> A quote');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should create empty node above when cursor within syntax', () => {
      const content = '> A quote';
      const position = 1; // >| A quote

      const result = strategy.split(content, position, quotePattern);

      expect(result.beforeContent).toBe('> ');
      expect(result.afterContent).toBe('> A quote');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should split and inherit when cursor after syntax', () => {
      const content = '> A quote';
      const position = 5; // > A q|uote

      const result = strategy.split(content, position, quotePattern);

      expect(result.beforeContent).toBe('> A q');
      expect(result.afterContent).toBe('> uote');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should handle multi-line quote content', () => {
      const content = '> First line\n> Second line';
      const position = 12; // > First line|

      const result = strategy.split(content, position, quotePattern);

      expect(result.beforeContent).toBe('> First line');
      expect(result.afterContent).toBe('> \n> Second line');
      expect(result.newNodeCursorPosition).toBe(2);
    });
  });

  describe('Fallback Prefix Extraction', () => {
    it('should extract prefix from regex match when not explicitly provided', () => {
      const pattern: PatternTemplate = {
        regex: /^(#+)\s+/,
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        // No prefixToInherit provided - should extract from regex
        cursorPlacement: 'after-prefix'
      };

      const content = '### Header';
      const position = 4; // ### |Header

      const result = strategy.split(content, position, pattern);

      // Should extract "### " from the regex match
      expect(result.beforeContent).toBe('### ');
      expect(result.afterContent).toContain('###');
    });
  });

  describe('Edge Cases', () => {
    const pattern: PatternTemplate = {
      regex: /^# /,
      nodeType: 'header',
      priority: 10,
      splittingStrategy: 'prefix-inheritance',
      prefixToInherit: '# ',
      cursorPlacement: 'after-prefix'
    };

    it('should handle empty content', () => {
      const content = '';
      const position = 0;

      const result = strategy.split(content, position, pattern);

      expect(result.beforeContent).toBe('# ');
      expect(result.afterContent).toBe('');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should handle cursor beyond content length', () => {
      const content = '# Header';
      const position = 100; // Way beyond content

      const result = strategy.split(content, position, pattern);

      expect(result.beforeContent).toBe('# Header');
      expect(result.afterContent).toBe('# ');
    });

    it('should handle negative position as zero', () => {
      const content = '# Header';
      const position = -5;

      const result = strategy.split(content, position, pattern);

      expect(result.beforeContent).toBe('# ');
      expect(result.afterContent).toBe('# Header');
    });
  });

  describe('Cursor Positioning', () => {
    it('should position cursor after prefix in new node', () => {
      const pattern: PatternTemplate = {
        regex: /^# /,
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      const content = '# Header text';
      const position = 5;

      const result = strategy.split(content, position, pattern);

      expect(result.newNodeCursorPosition).toBe(2); // After "# "
    });

    it('should position cursor correctly for different prefix lengths', () => {
      for (let level = 1; level <= 6; level++) {
        const prefix = '#'.repeat(level) + ' ';

        const pattern: PatternTemplate = {
          regex: new RegExp(`^#{${level}}\\s+`),
          nodeType: 'header',
          priority: 10,
          splittingStrategy: 'prefix-inheritance',
          prefixToInherit: prefix,
          cursorPlacement: 'after-prefix'
        };

        const content = prefix + 'Content';
        const position = 5;

        const result = strategy.split(content, position, pattern);

        expect(result.newNodeCursorPosition).toBe(prefix.length);
      }
    });
  });
});
