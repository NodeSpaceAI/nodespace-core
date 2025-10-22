/**
 * Simple Split Strategy Tests
 *
 * Tests splitting behavior for patterns without special prefixes:
 * - Text nodes
 * - Task nodes
 * - Inline formatting preservation
 */

import { describe, it, expect } from 'vitest';
import { SimpleSplitStrategy } from '../../../lib/patterns/splitter/strategies/simple-split';
import type { PatternTemplate } from '../../../lib/patterns/types';

describe('SimpleSplitStrategy', () => {
  const strategy = new SimpleSplitStrategy();

  const textPattern: PatternTemplate = {
    regex: /^/,
    nodeType: 'text',
    priority: 1,
    splittingStrategy: 'simple-split',
    cursorPlacement: 'start'
  };

  describe('Basic Splitting', () => {
    it('should split plain text without formatting', () => {
      const content = 'plain text content';
      const position = 6; // plain |text

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('plain ');
      expect(result.afterContent).toBe('text content');
      expect(result.newNodeCursorPosition).toBe(0);
    });

    it('should handle splitting at start', () => {
      const content = 'text content';
      const position = 0; // |text content

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('');
      expect(result.afterContent).toBe('text content');
      expect(result.newNodeCursorPosition).toBe(0);
    });

    it('should handle splitting at end', () => {
      const content = 'text content';
      const position = 12; // text content|

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('text content');
      expect(result.afterContent).toBe('');
      expect(result.newNodeCursorPosition).toBe(0);
    });

    it('should handle empty content', () => {
      const content = '';
      const position = 0;

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('');
      expect(result.afterContent).toBe('');
      expect(result.newNodeCursorPosition).toBe(0);
    });
  });

  describe('Bold Formatting Preservation', () => {
    it('should preserve **bold** formatting on both sides', () => {
      const content = '**bold text**';
      const position = 5; // **bol|d text**

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('**bol**');
      expect(result.afterContent).toBe('**d text**');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should preserve __bold__ formatting', () => {
      const content = '__bold text__';
      const position = 5; // __bol|d text__

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('__bol__');
      expect(result.afterContent).toBe('__d text__');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should handle bold at beginning', () => {
      const content = '**bold**';
      const position = 2; // **|bold**

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('****');
      expect(result.afterContent).toBe('**bold**');
    });

    it('should handle bold at end', () => {
      const content = '**bold**';
      const position = 8; // **bold**|

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('**bold**');
      expect(result.afterContent).toBe('');
    });
  });

  describe('Italic Formatting Preservation', () => {
    it('should preserve *italic* formatting', () => {
      const content = '*italic text*';
      const position = 7; // *italic| text*

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('*italic*');
      expect(result.afterContent).toBe('* text*');
      expect(result.newNodeCursorPosition).toBe(1);
    });

    it('should preserve _italic_ formatting', () => {
      const content = '_italic text_';
      const position = 7; // _italic| text_

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('_italic_');
      expect(result.afterContent).toBe('_ text_');
      expect(result.newNodeCursorPosition).toBe(1);
    });
  });

  describe('Code Formatting Preservation', () => {
    it('should preserve `code` formatting', () => {
      const content = '`code snippet`';
      const position = 5; // `code| snippet`

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('`code`');
      expect(result.afterContent).toBe('` snippet`');
      expect(result.newNodeCursorPosition).toBe(1);
    });

    it('should handle code in function name', () => {
      const content = '`getUserById()`';
      const position = 8; // `getUser|ById()`

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('`getUser`');
      expect(result.afterContent).toBe('`ById()`');
      expect(result.newNodeCursorPosition).toBe(1);
    });
  });

  describe('Strikethrough Formatting Preservation', () => {
    it('should preserve ~~strikethrough~~ formatting', () => {
      const content = '~~struck out~~';
      const position = 8; // ~~struck| out~~

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('~~struck~~');
      expect(result.afterContent).toBe('~~ out~~');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should preserve ~strikethrough~ formatting', () => {
      const content = '~struck out~';
      const position = 7; // ~struck| out~

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('~struck~');
      expect(result.afterContent).toBe('~ out~');
      expect(result.newNodeCursorPosition).toBe(1);
    });
  });

  describe('Mixed Formatting', () => {
    it('should handle ***bold-italic***', () => {
      const content = '***important***';
      const position = 8; // ***impor|tant***

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('***impor***');
      expect(result.afterContent).toBe('***tant***');
      expect(result.newNodeCursorPosition).toBe(3);
    });

    it('should handle **_nested formatting_**', () => {
      const content = '**_text_**';
      const position = 6; // **_tex|t_**

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('**_tex_**');
      expect(result.afterContent).toBe('**_t_**');
    });

    it('should handle multiple formatting regions', () => {
      const content = '**bold** and *italic*';
      const position = 17; // **bold** and *ita|lic*

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('**bold** and *ita*');
      expect(result.afterContent).toBe('*lic*');
      expect(result.newNodeCursorPosition).toBe(1);
    });

    it('should handle bold and code together', () => {
      const content = 'Use **bold** and `code`';
      const position = 12; // Use **bold**| and `code`

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('Use **bold**');
      expect(result.afterContent).toBe(' and `code`');
      expect(result.newNodeCursorPosition).toBe(0);
    });
  });

  describe('Real-world Scenarios', () => {
    it('should handle documentation comment', () => {
      const content = 'Use `Array.prototype.map()` to transform';
      const position = 20; // Use `Array.prototype|.map()` to transform

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('Use `Array.prototype`');
      expect(result.afterContent).toBe('`.map()` to transform');
    });

    it('should handle warning text', () => {
      const content = '**WARNING:** Do not use in production';
      const position = 12; // **WARNING:**| Do not use

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('**WARNING:**');
      expect(result.afterContent).toBe(' Do not use in production');
      expect(result.newNodeCursorPosition).toBe(0);
    });

    it('should handle important action', () => {
      const content = '**Important:** Remember to save your work';
      const position = 17; // **Important:** Re|member to save

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('**Important:** Re');
      expect(result.afterContent).toBe('member to save your work');
      expect(result.newNodeCursorPosition).toBe(0);
    });
  });

  describe('Cursor Positioning', () => {
    it('should position cursor after opening markers for bold', () => {
      const content = '**text**';
      const position = 4; // **te|xt**

      const result = strategy.split(content, position, textPattern);

      expect(result.newNodeCursorPosition).toBe(2); // After **
    });

    it('should position cursor after opening markers for italic', () => {
      const content = '*text*';
      const position = 3; // *te|xt*

      const result = strategy.split(content, position, textPattern);

      expect(result.newNodeCursorPosition).toBe(1); // After *
    });

    it('should position cursor at start when no formatting', () => {
      const content = 'plain text';
      const position = 6; // plain |text

      const result = strategy.split(content, position, textPattern);

      expect(result.newNodeCursorPosition).toBe(0);
    });
  });

  describe('Boundary Cases', () => {
    it('should handle position at negative value', () => {
      const content = 'text';
      const position = -5;

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('');
      expect(result.afterContent).toBe('text');
    });

    it('should handle position beyond content length', () => {
      const content = 'text';
      const position = 100;

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('text');
      expect(result.afterContent).toBe('');
    });

    it('should handle content with only formatting markers', () => {
      const content = '**';
      const position = 1; // *|*

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('**');
      expect(result.afterContent).toBe('**');
    });
  });

  describe('Nested and Complex Formatting', () => {
    it('should handle deeply nested formatting', () => {
      const content = '**bold with *italic inside* bold**';
      const position = 17; // **bold with *italic|

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toContain('**');
      expect(result.afterContent).toContain('**');
    });

    it('should handle overlapping formatting', () => {
      const content = '**bold** and *italic*';
      const position = 8; // **bold**| and *italic*

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('**bold**');
      expect(result.afterContent).toBe(' and *italic*');
    });

    it('should handle unbalanced formatting', () => {
      const content = '**bold without closing';
      const position = 9; // **bold wi|thout

      const result = strategy.split(content, position, textPattern);

      expect(result.beforeContent).toBe('**bold wi**');
      expect(result.afterContent).toBe('**thout closing');
    });
  });
});
