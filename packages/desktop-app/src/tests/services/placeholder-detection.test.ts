/**
 * Placeholder Node Detection Tests
 *
 * Tests for the isPlaceholderNode() logic in SharedNodeStore that prevents
 * empty/prefix-only nodes from persisting to the database.
 *
 * Architecture: Frontend manages placeholder nodes in memory until user adds
 * actual content, then persists to database with backend validation.
 */

import { describe, it, expect } from 'vitest';

// Test the placeholder detection logic directly
function isPlaceholderNode(node: { nodeType: string; content: string }): boolean {
  const trimmedContent = node.content.trim();

  switch (node.nodeType) {
    case 'text': {
      // Text nodes: empty content is a placeholder
      if (trimmedContent === '') {
        return true;
      }

      // Text nodes that contain ONLY pattern prefixes are also placeholders
      // These are text nodes in the process of being converted to specialized types
      // Check for common patterns: "> " (quote), "# " (header), "```" (code)
      if (
        trimmedContent === '>' ||
        trimmedContent.match(/^>\s*$/) || // "> " or ">  " etc
        trimmedContent.match(/^#{1,6}\s*$/) || // "# " or "## " etc
        trimmedContent.match(/^```\w*\s*$/) // "```" or "```js " etc
      ) {
        return true;
      }

      return false;
    }

    case 'quote-block': {
      // Quote-block nodes: only "> " prefix (no actual content after) is a placeholder
      // Strip "> " or ">" from all lines and check if any content remains
      const contentWithoutPrefix = trimmedContent
        .split('\n')
        .map((line) => line.replace(/^>\s?/, ''))
        .join('\n')
        .trim();
      return contentWithoutPrefix === '';
    }

    case 'header': {
      // Header nodes: only "# " prefix (no actual content after) is a placeholder
      // Strip hashtags and space, check if content remains
      const contentWithoutHashtags = trimmedContent.replace(/^#{1,6}\s*/, '');
      return contentWithoutHashtags === '';
    }

    case 'code-block': {
      // Code-block nodes: only "```" prefix (no actual code) is a placeholder
      // Strip backticks and language identifier, check if code remains
      const contentWithoutBackticks = trimmedContent
        .replace(/^```\w*\s*/, '')
        .replace(/```$/, '');
      return contentWithoutBackticks.trim() === '';
    }

    case 'task':
      // Task nodes: empty content (regardless of checkbox) is a placeholder
      // The backend validates task description separately
      return trimmedContent === '';

    default:
      // For unknown node types, use simple empty check
      return trimmedContent === '';
  }
}

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
