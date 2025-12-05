/**
 * Tests for TextNode Type-Safe Wrapper
 *
 * Comprehensive test coverage for text node type guards, helpers, and utilities.
 */

import { describe, it, expect } from 'vitest';
import type { Node } from '$lib/types/node';
import { type TextNode, isTextNode, TextNodeHelpers } from '$lib/types/text-node';

describe('TextNode Type Guard', () => {
  it('identifies text nodes correctly', () => {
    const textNode: TextNode = {
      id: 'test-1',
      nodeType: 'text',
      content: 'This is a text node',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isTextNode(textNode)).toBe(true);
  });

  it('rejects non-text nodes', () => {
    const taskNode: Node = {
      id: 'test-2',
      nodeType: 'task',
      content: 'This is a task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isTextNode(taskNode)).toBe(false);
  });

  it('rejects date nodes', () => {
    const dateNode: Node = {
      id: 'test-3',
      nodeType: 'date',
      content: '2025-12-05',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isTextNode(dateNode)).toBe(false);
  });

  it('works with type narrowing', () => {
    const node: Node = {
      id: 'test-4',
      nodeType: 'text',
      content: 'Sample text',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    if (isTextNode(node)) {
      // TypeScript should narrow the type here
      expect(node.nodeType).toBe('text');
      expect(node.content).toBe('Sample text');
    }
  });
});

describe('TextNodeHelpers.createTextNode', () => {
  it('creates a text node with the given content', () => {
    const content = 'Hello, world!';
    const textNode = TextNodeHelpers.createTextNode(content);

    expect(textNode.content).toBe(content);
    expect(textNode.nodeType).toBe('text');
  });

  it('generates a unique ID', () => {
    const node1 = TextNodeHelpers.createTextNode('First');
    const node2 = TextNodeHelpers.createTextNode('Second');

    expect(node1.id).not.toBe(node2.id);
    expect(node1.id).toMatch(/^text-/);
    expect(node2.id).toMatch(/^text-/);
  });

  it('sets timestamps', () => {
    const before = Date.now();
    const textNode = TextNodeHelpers.createTextNode('Test');
    const after = Date.now();

    expect(textNode.createdAt).toBeDefined();
    expect(textNode.modifiedAt).toBeDefined();

    // Parse ISO strings to timestamps for comparison
    const createdTime = new Date(textNode.createdAt).getTime();
    expect(createdTime).toBeGreaterThanOrEqual(before);
    expect(createdTime).toBeLessThanOrEqual(after);
    expect(textNode.modifiedAt).toBe(textNode.createdAt);
  });

  it('sets version to 1', () => {
    const textNode = TextNodeHelpers.createTextNode('Test');
    expect(textNode.version).toBe(1);
  });

  it('initializes properties as empty object', () => {
    const textNode = TextNodeHelpers.createTextNode('Test');
    expect(textNode.properties).toEqual({});
  });

  it('handles empty string content', () => {
    const textNode = TextNodeHelpers.createTextNode('');
    expect(textNode.content).toBe('');
    expect(textNode.nodeType).toBe('text');
  });

  it('handles whitespace-only content', () => {
    const textNode = TextNodeHelpers.createTextNode('   ');
    expect(textNode.content).toBe('   ');
  });

  it('handles multiline content', () => {
    const multiline = 'Line 1\nLine 2\nLine 3';
    const textNode = TextNodeHelpers.createTextNode(multiline);
    expect(textNode.content).toBe(multiline);
  });

  it('handles special characters', () => {
    const special = 'Special chars: !@#$%^&*()_+-=[]{}|;:,.<>?';
    const textNode = TextNodeHelpers.createTextNode(special);
    expect(textNode.content).toBe(special);
  });

  it('handles unicode characters', () => {
    const unicode = 'Unicode: 你好世界 🌍🚀✨';
    const textNode = TextNodeHelpers.createTextNode(unicode);
    expect(textNode.content).toBe(unicode);
  });
});

describe('TextNodeHelpers.isEmpty', () => {
  it('returns true for empty string', () => {
    const node = TextNodeHelpers.createTextNode('');
    expect(TextNodeHelpers.isEmpty(node)).toBe(true);
  });

  it('returns true for whitespace-only string', () => {
    const node1 = TextNodeHelpers.createTextNode('   ');
    const node2 = TextNodeHelpers.createTextNode('\t\t');
    const node3 = TextNodeHelpers.createTextNode('\n\n');
    const node4 = TextNodeHelpers.createTextNode('  \t\n  ');

    expect(TextNodeHelpers.isEmpty(node1)).toBe(true);
    expect(TextNodeHelpers.isEmpty(node2)).toBe(true);
    expect(TextNodeHelpers.isEmpty(node3)).toBe(true);
    expect(TextNodeHelpers.isEmpty(node4)).toBe(true);
  });

  it('returns false for non-empty string', () => {
    const node = TextNodeHelpers.createTextNode('Hello');
    expect(TextNodeHelpers.isEmpty(node)).toBe(false);
  });

  it('returns false for string with leading/trailing whitespace', () => {
    const node = TextNodeHelpers.createTextNode('  Hello  ');
    expect(TextNodeHelpers.isEmpty(node)).toBe(false);
  });

  it('returns false for single character', () => {
    const node = TextNodeHelpers.createTextNode('a');
    expect(TextNodeHelpers.isEmpty(node)).toBe(false);
  });

  it('returns false for multiline content', () => {
    const node = TextNodeHelpers.createTextNode('Line 1\nLine 2');
    expect(TextNodeHelpers.isEmpty(node)).toBe(false);
  });
});

describe('TextNodeHelpers.getWordCount', () => {
  it('returns 0 for empty string', () => {
    const node = TextNodeHelpers.createTextNode('');
    expect(TextNodeHelpers.getWordCount(node)).toBe(0);
  });

  it('returns 0 for whitespace-only string', () => {
    const node1 = TextNodeHelpers.createTextNode('   ');
    const node2 = TextNodeHelpers.createTextNode('\t\t');
    const node3 = TextNodeHelpers.createTextNode('\n\n');

    expect(TextNodeHelpers.getWordCount(node1)).toBe(0);
    expect(TextNodeHelpers.getWordCount(node2)).toBe(0);
    expect(TextNodeHelpers.getWordCount(node3)).toBe(0);
  });

  it('counts single word', () => {
    const node = TextNodeHelpers.createTextNode('Hello');
    expect(TextNodeHelpers.getWordCount(node)).toBe(1);
  });

  it('counts multiple words separated by spaces', () => {
    const node = TextNodeHelpers.createTextNode('Hello world test');
    expect(TextNodeHelpers.getWordCount(node)).toBe(3);
  });

  it('handles multiple spaces between words', () => {
    const node = TextNodeHelpers.createTextNode('Hello    world');
    expect(TextNodeHelpers.getWordCount(node)).toBe(2);
  });

  it('handles leading and trailing whitespace', () => {
    const node = TextNodeHelpers.createTextNode('  Hello world  ');
    expect(TextNodeHelpers.getWordCount(node)).toBe(2);
  });

  it('handles tabs as word separators', () => {
    const node = TextNodeHelpers.createTextNode('Hello\tworld\ttest');
    expect(TextNodeHelpers.getWordCount(node)).toBe(3);
  });

  it('handles newlines as word separators', () => {
    const node = TextNodeHelpers.createTextNode('Hello\nworld\ntest');
    expect(TextNodeHelpers.getWordCount(node)).toBe(3);
  });

  it('handles mixed whitespace', () => {
    const node = TextNodeHelpers.createTextNode('Hello  \t\n  world');
    expect(TextNodeHelpers.getWordCount(node)).toBe(2);
  });

  it('counts hyphenated words as single words', () => {
    const node = TextNodeHelpers.createTextNode('test-driven development');
    expect(TextNodeHelpers.getWordCount(node)).toBe(2);
  });

  it('handles punctuation within text', () => {
    const node = TextNodeHelpers.createTextNode("Hello, world! How's it going?");
    expect(TextNodeHelpers.getWordCount(node)).toBe(5);
  });

  it('handles multiline paragraphs', () => {
    const node = TextNodeHelpers.createTextNode(
      'First paragraph here.\n\nSecond paragraph with more words.'
    );
    expect(TextNodeHelpers.getWordCount(node)).toBe(8);
  });

  it('handles unicode words', () => {
    const node = TextNodeHelpers.createTextNode('你好 世界 test');
    expect(TextNodeHelpers.getWordCount(node)).toBe(3);
  });
});

describe('TextNodeHelpers.isTextNode (namespace method)', () => {
  it('matches the standalone isTextNode function', () => {
    const textNode: TextNode = {
      id: 'test-5',
      nodeType: 'text',
      content: 'Test',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(TextNodeHelpers.isTextNode(textNode)).toBe(true);
    expect(isTextNode(textNode)).toBe(true);
    expect(TextNodeHelpers.isTextNode(textNode)).toBe(isTextNode(textNode));
  });

  it('rejects non-text nodes consistently', () => {
    const taskNode: Node = {
      id: 'test-6',
      nodeType: 'task',
      content: 'Task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(TextNodeHelpers.isTextNode(taskNode)).toBe(false);
    expect(isTextNode(taskNode)).toBe(false);
  });
});

describe('Integration', () => {
  it('creates and validates text nodes', () => {
    const content = 'Integration test content';
    const node = TextNodeHelpers.createTextNode(content);

    expect(isTextNode(node)).toBe(true);
    expect(TextNodeHelpers.isTextNode(node)).toBe(true);
    expect(node.content).toBe(content);
  });

  it('creates non-empty nodes', () => {
    const node = TextNodeHelpers.createTextNode('Hello world');

    expect(TextNodeHelpers.isEmpty(node)).toBe(false);
    expect(TextNodeHelpers.getWordCount(node)).toBe(2);
  });

  it('creates empty nodes', () => {
    const node = TextNodeHelpers.createTextNode('');

    expect(TextNodeHelpers.isEmpty(node)).toBe(true);
    expect(TextNodeHelpers.getWordCount(node)).toBe(0);
  });

  it('works with type guard in conditional', () => {
    const node: Node = TextNodeHelpers.createTextNode('Sample text with multiple words');

    if (isTextNode(node)) {
      // TypeScript should narrow the type
      expect(node.nodeType).toBe('text');
      expect(TextNodeHelpers.getWordCount(node)).toBe(5);
      expect(TextNodeHelpers.isEmpty(node)).toBe(false);
    } else {
      throw new Error('Should be a text node');
    }
  });

  it('handles various text scenarios', () => {
    const scenarios = [
      { content: '', isEmpty: true, wordCount: 0 },
      { content: '   ', isEmpty: true, wordCount: 0 },
      { content: 'Hello', isEmpty: false, wordCount: 1 },
      { content: 'Hello world', isEmpty: false, wordCount: 2 },
      { content: '  Hello  world  ', isEmpty: false, wordCount: 2 },
      { content: 'One\nTwo\nThree', isEmpty: false, wordCount: 3 },
      {
        content: 'A longer sentence with multiple words.',
        isEmpty: false,
        wordCount: 6
      }
    ];

    scenarios.forEach(({ content, isEmpty, wordCount }) => {
      const node = TextNodeHelpers.createTextNode(content);

      expect(TextNodeHelpers.isEmpty(node)).toBe(isEmpty);
      expect(TextNodeHelpers.getWordCount(node)).toBe(wordCount);
      expect(node.nodeType).toBe('text');
      expect(node.content).toBe(content);
    });
  });

  it('maintains node structure integrity', () => {
    const node = TextNodeHelpers.createTextNode('Test content');

    // Verify all required fields exist
    expect(node).toHaveProperty('id');
    expect(node).toHaveProperty('nodeType');
    expect(node).toHaveProperty('content');
    expect(node).toHaveProperty('createdAt');
    expect(node).toHaveProperty('modifiedAt');
    expect(node).toHaveProperty('version');
    expect(node).toHaveProperty('properties');

    // Verify field types
    expect(typeof node.id).toBe('string');
    expect(node.nodeType).toBe('text');
    expect(typeof node.content).toBe('string');
    expect(typeof node.createdAt).toBe('string');
    expect(typeof node.modifiedAt).toBe('string');
    expect(typeof node.version).toBe('number');
    expect(typeof node.properties).toBe('object');
  });
});
