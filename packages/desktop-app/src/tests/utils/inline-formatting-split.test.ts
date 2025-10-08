/**
 * Inline Formatting Split Tests
 *
 * Tests the behavior when pressing Enter/Shift+Enter within inline formatted text.
 * Ensures that markdown formatting is preserved on both lines after splitting:
 * - **bold** text
 * - *italic* text
 * - `code` text
 * - ~~strikethrough~~ text
 * - Mixed formatting like ***bold-italic***
 */

import { describe, it, expect } from 'vitest';
import { splitMarkdownContent } from '../../lib/utils/markdownSplitter';

describe('Inline Formatting Split', () => {
  describe('Bold formatting (**text**)', () => {
    it('should split bold text in the middle and preserve formatting on both lines', () => {
      const content = '**bold text**';
      const position = 5; // **bol|d text** (position 5 is after '**bol')

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('**bol**'); // First line gets closing **
      expect(result.afterContent).toBe('**d text**'); // Second line gets opening **
      expect(result.newNodeCursorPosition).toBe(2); // Cursor after opening **
    });

    it('should split bold text at the beginning of word', () => {
      const content = '**bold text**';
      const position = 2; // **|bold text**

      const result = splitMarkdownContent(content, position);

      // When splitting right after opening markers, the implementation adds closing markers
      expect(result.beforeContent).toBe('****'); // Opening markers + closing markers
      expect(result.afterContent).toBe('**bold text**'); // Full bold text
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should split bold text at the end of word', () => {
      const content = '**bold text**';
      const position = 11; // **bold text|**

      const result = splitMarkdownContent(content, position);

      // When splitting right before closing markers, the implementation adds opening markers to second line
      expect(result.beforeContent).toBe('**bold text**'); // Complete bold text
      expect(result.afterContent).toBe('****'); // Opening markers + closing markers
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should handle underscore bold (__text__)', () => {
      const content = '__bold text__';
      const position = 5; // __bol|d text__

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('__bol__');
      expect(result.afterContent).toBe('__d text__');
      expect(result.newNodeCursorPosition).toBe(2);
    });
  });

  describe('Italic formatting (*text* and _text_)', () => {
    it('should split italic text in the middle with asterisks', () => {
      const content = '*italic text*';
      const position = 7; // *italic| text*

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('*italic*');
      expect(result.afterContent).toBe('* text*');
      expect(result.newNodeCursorPosition).toBe(1);
    });

    it('should split italic text in the middle with underscores', () => {
      const content = '_italic text_';
      const position = 7; // _italic| text_

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('_italic_');
      expect(result.afterContent).toBe('_ text_');
      expect(result.newNodeCursorPosition).toBe(1);
    });
  });

  describe('Code formatting (`text`)', () => {
    it('should split code text in the middle', () => {
      const content = '`code snippet`';
      const position = 5; // `code| snippet`

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('`code`');
      expect(result.afterContent).toBe('` snippet`');
      expect(result.newNodeCursorPosition).toBe(1);
    });

    it('should handle splitting within function call', () => {
      const content = '`getUserById()`';
      const position = 8; // `getUser|ById()`

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('`getUser`');
      expect(result.afterContent).toBe('`ById()`');
      expect(result.newNodeCursorPosition).toBe(1);
    });
  });

  describe('Strikethrough formatting (~~text~~ and ~text~)', () => {
    it('should split double tilde strikethrough text in the middle', () => {
      const content = '~~crossed out~~';
      const position = 9; // ~~crossed| out~~

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('~~crossed~~');
      expect(result.afterContent).toBe('~~ out~~');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should split single tilde strikethrough text in the middle', () => {
      const content = '~strikethrough~';
      const position = 7; // ~strike|through~

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('~strike~');
      expect(result.afterContent).toBe('~through~');
      expect(result.newNodeCursorPosition).toBe(1);
    });

    it('should handle single tilde in mixed content', () => {
      const content = 'Testing some in-line **bold**, *italic*, and ~strikethrough~';
      const position = 52; // Testing some in-line **bold**, *italic*, and ~strike|through~

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('Testing some in-line **bold**, *italic*, and ~strike~');
      expect(result.afterContent).toBe('~through~');
      expect(result.newNodeCursorPosition).toBe(1);
    });
  });

  describe('Bold-Italic formatting (***text***)', () => {
    it('should split bold-italic text in the middle', () => {
      const content = '***important***';
      const position = 8; // ***impor|tant***

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('***impor***');
      expect(result.afterContent).toBe('***tant***');
      expect(result.newNodeCursorPosition).toBe(3);
    });

    it('should handle mixed bold-italic syntax (**_text_**)', () => {
      const content = '**_important_**';
      const position = 8; // **_impor|tant_**

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('**_impor_**');
      expect(result.afterContent).toBe('**_tant_**');
      expect(result.newNodeCursorPosition).toBe(3);
    });
  });

  describe('Mixed content (plain text + formatting)', () => {
    it('should split text with bold in the middle of sentence', () => {
      const content = 'This is **bold** text';
      const position = 12; // This is **bo|ld** text

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('This is **bo**');
      expect(result.afterContent).toBe('**ld** text');
      expect(result.newNodeCursorPosition).toBe(2);
    });

    it('should split plain text outside of formatting', () => {
      const content = 'This is **bold** text';
      const position = 5; // This |is **bold** text

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('This ');
      expect(result.afterContent).toBe('is **bold** text');
      expect(result.newNodeCursorPosition).toBe(0); // No formatting, cursor at start
    });

    it('should handle multiple formatted sections', () => {
      const content = '**bold** and *italic* text';
      const position = 17; // **bold** and *ita|lic* text

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('**bold** and *ita*');
      expect(result.afterContent).toBe('*lic* text');
      expect(result.newNodeCursorPosition).toBe(1);
    });
  });

  describe('Edge cases', () => {
    it('should handle splitting at start of content', () => {
      const content = '**bold**';
      const position = 0; // |**bold**

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('');
      expect(result.afterContent).toBe('**bold**');
      expect(result.newNodeCursorPosition).toBe(0);
    });

    it('should handle splitting at end of content', () => {
      const content = '**bold**';
      const position = 8; // **bold**|

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('**bold**');
      expect(result.afterContent).toBe('');
      expect(result.newNodeCursorPosition).toBe(0);
    });

    it('should handle empty content', () => {
      const content = '';
      const position = 0;

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('');
      expect(result.afterContent).toBe('');
      expect(result.newNodeCursorPosition).toBe(0);
    });

    it('should preserve nested formatting', () => {
      const content = '**bold _italic_ text**';
      const position = 15; // **bold _italic| _ text**

      const result = splitMarkdownContent(content, position);

      // This is a complex case - the implementation should preserve both bold and italic
      expect(result.beforeContent).toContain('_italic_');
      expect(result.beforeContent).toContain('**');
    });
  });

  describe('Real-world scenarios', () => {
    it('should handle splitting in code documentation', () => {
      const content = 'Use `Array.prototype.map()` to transform arrays';
      const position = 20; // Use `Array.prototype|.map()` to transform arrays

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('Use `Array.prototype`');
      expect(result.afterContent).toBe('`.map()` to transform arrays');
      expect(result.newNodeCursorPosition).toBe(1);
    });

    it('should handle splitting in emphasized warning', () => {
      const content = '**WARNING:** Do not use this in production';
      const position = 12; // **WARNING:**| Do not use this in production

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('**WARNING:**');
      expect(result.afterContent).toBe(' Do not use this in production');
      expect(result.newNodeCursorPosition).toBe(0);
    });

    it('should handle splitting in formatted list item', () => {
      const content = '**Important:** Remember to save your work';
      const position = 17; // **Important:** Re|member to save your work

      const result = splitMarkdownContent(content, position);

      expect(result.beforeContent).toBe('**Important:** Re');
      expect(result.afterContent).toBe('member to save your work');
      expect(result.newNodeCursorPosition).toBe(0);
    });
  });

  describe('Cursor positioning', () => {
    it('should position cursor after opening markers in new line for bold', () => {
      const content = '**text**';
      const position = 4; // **te|xt**

      const result = splitMarkdownContent(content, position);

      expect(result.newNodeCursorPosition).toBe(2); // After **
    });

    it('should position cursor after opening markers in new line for italic', () => {
      const content = '*text*';
      const position = 3; // *te|xt*

      const result = splitMarkdownContent(content, position);

      expect(result.newNodeCursorPosition).toBe(1); // After *
    });

    it('should position cursor after opening markers in new line for code', () => {
      const content = '`code`';
      const position = 3; // `co|de`

      const result = splitMarkdownContent(content, position);

      expect(result.newNodeCursorPosition).toBe(1); // After `
    });

    it('should position cursor at start when no formatting', () => {
      const content = 'plain text';
      const position = 6; // plain |text

      const result = splitMarkdownContent(content, position);

      expect(result.newNodeCursorPosition).toBe(0); // At start
    });
  });
});
