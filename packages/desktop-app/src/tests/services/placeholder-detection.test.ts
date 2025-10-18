/**
 * Placeholder Node Detection Tests
 *
 * Tests for the isPlaceholderNode() utility that prevents
 * empty/prefix-only nodes from persisting to the database.
 *
 * Architecture: Frontend manages placeholder nodes in memory until user adds
 * actual content, then persists to database with backend validation.
 */

import { describe, it, expect } from 'vitest';
import { isPlaceholderNode } from '$lib/utils/placeholder-detection';

describe('Placeholder Node Detection', () => {
  describe('Text Node Placeholders', () => {
    it('should detect empty text node as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'text', content: '' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'text', content: '   ' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'text', content: '\n\n' })).toBe(true);
    });

    it('should detect text node with only quote pattern as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'text', content: '>' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'text', content: '> ' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'text', content: '>  ' })).toBe(true);
    });

    it('should detect text node with only header pattern as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'text', content: '# ' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'text', content: '## ' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'text', content: '### ' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'text', content: '#### ' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'text', content: '##### ' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'text', content: '###### ' })).toBe(true);
    });

    it('should detect text node with only code-block pattern as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'text', content: '```' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'text', content: '``` ' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'text', content: '```js' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'text', content: '```python ' })).toBe(true);
    });

    it('should NOT detect text node with actual content as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'text', content: 'Hello' })).toBe(false);
      expect(isPlaceholderNode({ nodeType: 'text', content: '> Hello' })).toBe(false);
      expect(isPlaceholderNode({ nodeType: 'text', content: '# Hello' })).toBe(false);
      expect(isPlaceholderNode({ nodeType: 'text', content: '```js\ncode' })).toBe(false);
    });
  });

  describe('Quote-Block Node Placeholders', () => {
    it('should detect quote-block with only "> " as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'quote-block', content: '> ' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'quote-block', content: '>' })).toBe(true);
    });

    it('should detect multiline quote-block with only prefixes as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'quote-block', content: '> \n> \n> ' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'quote-block', content: '>\n>\n>' })).toBe(true);
    });

    it('should NOT detect quote-block with actual content as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'quote-block', content: '> Hello' })).toBe(false);
      expect(isPlaceholderNode({ nodeType: 'quote-block', content: '> Line1\n> Line2' })).toBe(
        false
      );
      expect(isPlaceholderNode({ nodeType: 'quote-block', content: '> Test content' })).toBe(false);
    });

    it('should handle quote-block with mixed empty and non-empty lines', () => {
      // If ANY line has content, it's not a placeholder
      expect(isPlaceholderNode({ nodeType: 'quote-block', content: '> \n> Content\n> ' })).toBe(
        false
      );
      expect(isPlaceholderNode({ nodeType: 'quote-block', content: '> Hello\n> \n> ' })).toBe(
        false
      );
    });
  });

  describe('Header Node Placeholders', () => {
    it('should detect header with only "# " as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'header', content: '# ' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'header', content: '## ' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'header', content: '### ' })).toBe(true);
    });

    it('should detect header with only hashtags (no space) as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'header', content: '#' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'header', content: '##' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'header', content: '###' })).toBe(true);
    });

    it('should NOT detect header with actual content as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'header', content: '# Hello' })).toBe(false);
      expect(isPlaceholderNode({ nodeType: 'header', content: '## World' })).toBe(false);
      expect(isPlaceholderNode({ nodeType: 'header', content: '### Test' })).toBe(false);
    });
  });

  describe('Code-Block Node Placeholders', () => {
    it('should detect code-block with only ``` as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'code-block', content: '```' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'code-block', content: '```\n```' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'code-block', content: '```js\n```' })).toBe(true);
    });

    it('should NOT detect code-block with actual code as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'code-block', content: '```\ncode\n```' })).toBe(false);
      expect(isPlaceholderNode({ nodeType: 'code-block', content: '```js\nconsole.log()' })).toBe(
        false
      );
    });
  });

  describe('Task Node Placeholders', () => {
    it('should detect empty task as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'task', content: '' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'task', content: '   ' })).toBe(true);
    });

    it('should NOT detect task with description as placeholder', () => {
      expect(isPlaceholderNode({ nodeType: 'task', content: 'Task description' })).toBe(false);
    });
  });

  describe('Unknown Node Types', () => {
    it('should use simple empty check for unknown types', () => {
      expect(isPlaceholderNode({ nodeType: 'unknown', content: '' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'unknown', content: '   ' })).toBe(true);
      expect(isPlaceholderNode({ nodeType: 'custom', content: 'content' })).toBe(false);
    });
  });
});
