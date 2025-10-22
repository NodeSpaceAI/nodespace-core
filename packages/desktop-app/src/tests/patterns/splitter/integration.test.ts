/**
 * PatternSplitter Integration Tests
 *
 * Tests the unified PatternSplitter interface across all node types.
 * Validates integration with registry and strategies.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { PatternSplitter } from '../../../lib/patterns/splitter';
import { PatternRegistry } from '../../../lib/patterns/registry';
import type { PatternTemplate } from '../../../lib/patterns/types';

describe('PatternSplitter Integration', () => {
  let splitter: PatternSplitter;
  let registry: PatternRegistry;

  beforeEach(() => {
    registry = PatternRegistry.getInstance();
    registry.clear();
    splitter = new PatternSplitter(registry);

    // Register common patterns
    const headerPattern: PatternTemplate = {
      regex: /^(#{1,6})\s+/,
      nodeType: 'header',
      priority: 10,
      splittingStrategy: 'prefix-inheritance',
      prefixToInherit: '# ',
      cursorPlacement: 'after-prefix'
    };

    const orderedListPattern: PatternTemplate = {
      regex: /^1\.\s+/,
      nodeType: 'ordered-list',
      priority: 10,
      splittingStrategy: 'prefix-inheritance',
      prefixToInherit: '1. ',
      cursorPlacement: 'after-prefix'
    };

    const quotePattern: PatternTemplate = {
      regex: /^>\s+/,
      nodeType: 'quote-block',
      priority: 10,
      splittingStrategy: 'prefix-inheritance',
      prefixToInherit: '> ',
      cursorPlacement: 'after-prefix'
    };

    const textPattern: PatternTemplate = {
      regex: /^/,
      nodeType: 'text',
      priority: 1,
      splittingStrategy: 'simple-split',
      cursorPlacement: 'start'
    };

    const taskPattern: PatternTemplate = {
      regex: /^- \[/,
      nodeType: 'task',
      priority: 5,
      splittingStrategy: 'simple-split',
      cursorPlacement: 'start'
    };

    registry.register(headerPattern);
    registry.register(orderedListPattern);
    registry.register(quotePattern);
    registry.register(textPattern);
    registry.register(taskPattern);
  });

  afterEach(() => {
    registry.clear();
  });

  describe('Header Splitting', () => {
    it('should split header and inherit prefix in new node', () => {
      const content = '# My Header';
      const position = 5; // # My |Header

      const result = splitter.split(content, position, 'header');

      expect(result.beforeContent).toBe('# My ');
      expect(result.afterContent).toBe('# Header');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should handle H2 header', () => {
      registry.clear();
      const pattern: PatternTemplate = {
        regex: /^##\s+/,
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '## ',
        cursorPlacement: 'after-prefix'
      };
      registry.register(pattern);

      const content = '## Subtitle';
      const position = 5; // ## Su|btitle

      const result = splitter.split(content, position, 'header');

      expect(result.beforeContent).toBe('## Su');
      expect(result.afterContent).toBe('## btitle');
    });
  });

  describe('Ordered List Splitting', () => {
    it('should split ordered list and inherit prefix', () => {
      const content = '1. First item';
      const position = 9; // 1. First |item

      const result = splitter.split(content, position, 'ordered-list');

      expect(result.beforeContent).toBe('1. First ');
      expect(result.afterContent).toBe('1. item');
      expect(result.newNodeCursorPosition).toBe(3);
    });
  });

  describe('Quote Block Splitting', () => {
    it('should split quote block and inherit prefix', () => {
      const content = '> A quote';
      const position = 4; // > A |quote

      const result = splitter.split(content, position, 'quote-block');

      expect(result.beforeContent).toBe('> A ');
      expect(result.afterContent).toBe('> quote');
      expect(result.newNodeCursorPosition).toBe(2);
    });
  });

  describe('Text Node Splitting', () => {
    it('should split plain text without formatting', () => {
      const content = 'Plain text content';
      const position = 6; // Plain |text content

      const result = splitter.split(content, position, 'text');

      expect(result.beforeContent).toBe('Plain ');
      expect(result.afterContent).toBe('text content');
      expect(result.newNodeCursorPosition).toBe(0);
    });

    it('should preserve bold formatting when splitting', () => {
      const content = 'Some **bold** text';
      const position = 8; // Some **b|old** text

      const result = splitter.split(content, position, 'text');

      expect(result.beforeContent).toBe('Some **b**');
      expect(result.afterContent).toBe('**old** text');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should preserve italic formatting when splitting', () => {
      const content = 'Some *italic* text';
      const position = 8; // Some *it|alic* text

      const result = splitter.split(content, position, 'text');

      expect(result.beforeContent).toBe('Some *it*');
      expect(result.afterContent).toBe('*alic* text');
      expect(result.newNodeCursorPosition).toBe(1);
    });

    it('should preserve code formatting when splitting', () => {
      const content = 'Code `const x` here';
      const position = 13; // Code `const x|` here

      const result = splitter.split(content, position, 'text');

      // At position 13, we're inside the code block, so closing marker is added
      expect(result.beforeContent).toBe('Code `const x`');
      expect(result.afterContent).toBe('`` here');
      expect(result.newNodeCursorPosition).toBe(1);
    });

    it('should handle nested formatting', () => {
      const content = 'Text with ***bold-italic*** content';
      const position = 16; // Text with ***bold|-italic*** content

      const result = splitter.split(content, position, 'text');

      expect(result.beforeContent).toContain('***');
      expect(result.afterContent).toContain('***');
    });
  });

  describe('Task Node Splitting', () => {
    it('should split task node with simple split strategy', () => {
      const content = '- [ ] Buy groceries';
      const position = 9; // - [ ] Buy| groceries

      const result = splitter.split(content, position, 'task');

      expect(result.beforeContent).toBe('- [ ] Buy');
      expect(result.afterContent).toBe(' groceries');
      expect(result.newNodeCursorPosition).toBe(0);
    });
  });

  describe('Pattern Auto-Detection', () => {
    it('should detect header pattern without explicit node type', () => {
      const content = '# My Header';
      const position = 5;

      const result = splitter.split(content, position);

      expect(result.beforeContent).toBe('# My ');
      expect(result.afterContent).toBe('# Header');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should detect list pattern without explicit node type', () => {
      const content = '1. First item';
      const position = 9;

      const result = splitter.split(content, position);

      expect(result.beforeContent).toBe('1. First ');
      expect(result.afterContent).toBe('1. item');
      expect(result.newNodeCursorPosition).toBe(3);
    });

    it('should detect quote pattern without explicit node type', () => {
      const content = '> A quote';
      const position = 4;

      const result = splitter.split(content, position);

      expect(result.beforeContent).toBe('> A ');
      expect(result.afterContent).toBe('> quote');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should fall back to simple split for unrecognized content', () => {
      const content = 'Plain text that does not match any pattern';
      const position = 11;

      const result = splitter.split(content, position);

      expect(result.beforeContent).toBe('Plain text ');
      expect(result.afterContent).toBe('that does not match any pattern');
      expect(result.newNodeCursorPosition).toBe(0);
    });
  });

  describe('Priority-Based Detection', () => {
    it('should use higher priority pattern when multiple patterns match', () => {
      registry.clear();

      // Two patterns that could both match "# Header"
      const lowPriority: PatternTemplate = {
        regex: /^#/,
        nodeType: 'text',
        priority: 1,
        splittingStrategy: 'simple-split',
        cursorPlacement: 'start'
      };

      const highPriority: PatternTemplate = {
        regex: /^# /,
        nodeType: 'header',
        priority: 100,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };

      registry.register(lowPriority);
      registry.register(highPriority);

      const result = splitter.split('# Header', 5);

      // Should use header strategy (high priority), not simple split (low priority)
      expect(result.newNodeCursorPosition).toBe(2); // After "# " prefix
    });
  });

  describe('Cursor Positioning', () => {
    it('should position cursor at start for simple split', () => {
      const result = splitter.split('plain text', 5, 'text');
      expect(result.newNodeCursorPosition).toBe(0);
    });

    it('should position cursor after prefix for prefix-inheritance', () => {
      const result = splitter.split('# Header', 5, 'header');
      expect(result.newNodeCursorPosition).toBe(2); // After "# "
    });
  });

  describe('Edge Cases', () => {
    it('should handle empty content', () => {
      const result = splitter.split('', 0, 'text');
      expect(result.beforeContent).toBe('');
      expect(result.afterContent).toBe('');
    });

    it('should handle cursor at position 0', () => {
      const result = splitter.split('Content', 0, 'text');
      expect(result.beforeContent).toBe('');
      expect(result.afterContent).toBe('Content');
    });

    it('should handle cursor at end of content', () => {
      const content = 'Content';
      const result = splitter.split(content, content.length, 'text');
      expect(result.beforeContent).toBe('Content');
      expect(result.afterContent).toBe('');
    });

    it('should handle cursor beyond content length', () => {
      const result = splitter.split('Content', 100, 'text');
      expect(result.beforeContent).toBe('Content');
      expect(result.afterContent).toBe('');
    });

    it('should handle negative cursor position as zero', () => {
      const result = splitter.split('Content', -5, 'text');
      expect(result.beforeContent).toBe('');
      expect(result.afterContent).toBe('Content');
    });
  });

  describe('Strategy Registration', () => {
    it('should allow registering custom strategies', () => {
      const customStrategy = {
        split: (_content: string, _position: number, _pattern: unknown) => ({
          beforeContent: 'custom',
          afterContent: 'split',
          newNodeCursorPosition: 0
        })
      };

      splitter.registerStrategy('custom', customStrategy);
      const retrieved = splitter.getStrategy('custom');

      expect(retrieved).toBe(customStrategy);
    });

    it('should retrieve registered strategy', () => {
      const strategy = splitter.getStrategy('prefix-inheritance');
      expect(strategy).toBeDefined();
      expect(strategy?.split).toBeDefined();
    });

    it('should return undefined for non-existent strategy', () => {
      const strategy = splitter.getStrategy('nonexistent');
      expect(strategy).toBeUndefined();
    });
  });

  describe('Real-World Scenarios', () => {
    it('should handle documentation with mixed formatting', () => {
      const content = 'Example: **const x = 5** creates a constant';
      const position = 20; // After the bold code

      const result = splitter.split(content, position, 'text');

      expect(result.beforeContent).toContain('**');
      expect(result.afterContent).toBeDefined();
    });

    it('should handle header with inline formatting', () => {
      registry.clear();
      const pattern: PatternTemplate = {
        regex: /^# /,
        nodeType: 'header',
        priority: 10,
        splittingStrategy: 'prefix-inheritance',
        prefixToInherit: '# ',
        cursorPlacement: 'after-prefix'
      };
      registry.register(pattern);

      const content = '# Important **Note**';
      const position = 15;

      const result = splitter.split(content, position, 'header');

      expect(result.beforeContent).toContain('# ');
      expect(result.afterContent).toContain('# ');
    });

    it('should handle quote with bold text', () => {
      const content = '> This is **very important**';
      const position = 15;

      const result = splitter.split(content, position, 'quote-block');

      expect(result.beforeContent).toContain('> ');
      expect(result.afterContent).toContain('> ');
    });

    it('should handle list with code', () => {
      const content = '1. Initialize with `const x = 5`';
      const position = 20;

      const result = splitter.split(content, position, 'ordered-list');

      expect(result.beforeContent).toContain('1. ');
      expect(result.afterContent).toContain('1. ');
    });
  });
});
