/**
 * Tests for OrderedList Node Type-Safe Wrapper
 */

import { describe, it, expect } from 'vitest';
import type { Node } from '$lib/types/node';
import {
  type OrderedListNode,
  isOrderedListNode,
  OrderedListNodeHelpers
} from '$lib/types/ordered-list-node';

describe('OrderedListNode Type Guard', () => {
  it('identifies ordered list nodes correctly', () => {
    const orderedListNode: Node = {
      id: 'test-1',
      nodeType: 'ordered-list',
      content: 'First item',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isOrderedListNode(orderedListNode)).toBe(true);
  });

  it('rejects non-ordered-list nodes', () => {
    const textNode: Node = {
      id: 'test-2',
      nodeType: 'text',
      content: 'Regular text',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isOrderedListNode(textNode)).toBe(false);
  });
});

describe('OrderedListNodeHelpers', () => {
  describe('createOrderedListItem', () => {
    it('creates a new ordered list item with content', () => {
      const item = OrderedListNodeHelpers.createOrderedListItem('First step in the process');

      expect(item.nodeType).toBe('ordered-list');
      expect(item.content).toBe('First step in the process');
      expect(item.properties).toEqual({});
    });

    it('creates an ordered list item with content (parent relationship via backend)', () => {
      const item = OrderedListNodeHelpers.createOrderedListItem('A list item');

      expect(item.content).toBe('A list item');
      // Note: Parent relationships managed via backend graph queries
    });

    it('generates unique IDs', () => {
      const item1 = OrderedListNodeHelpers.createOrderedListItem('Item 1');
      const item2 = OrderedListNodeHelpers.createOrderedListItem('Item 2');

      expect(item1.id).not.toBe(item2.id);
    });

    it('sets timestamps', () => {
      const item = OrderedListNodeHelpers.createOrderedListItem('An item');

      expect(item.createdAt).toBeDefined();
      expect(item.modifiedAt).toBeDefined();
      expect(new Date(item.createdAt).getTime()).toBeGreaterThan(0);
    });
  });

  describe('isMultiline', () => {
    it('returns true for multiline list items', () => {
      const multilineItem: OrderedListNode = {
        id: 'test-3',
        nodeType: 'ordered-list',
        content: 'First line\nSecond line\nThird line',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(OrderedListNodeHelpers.isMultiline(multilineItem)).toBe(true);
    });

    it('returns false for single line list items', () => {
      const singleLineItem: OrderedListNode = {
        id: 'test-4',
        nodeType: 'ordered-list',
        content: 'A single line item',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(OrderedListNodeHelpers.isMultiline(singleLineItem)).toBe(false);
    });
  });

  describe('getFirstLine', () => {
    it('returns first line from multiline item', () => {
      const item: OrderedListNode = {
        id: 'test-5',
        nodeType: 'ordered-list',
        content: 'First line\nSecond line\nThird line',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(OrderedListNodeHelpers.getFirstLine(item)).toBe('First line');
    });

    it('returns content for single line item', () => {
      const item: OrderedListNode = {
        id: 'test-6',
        nodeType: 'ordered-list',
        content: 'Single line',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(OrderedListNodeHelpers.getFirstLine(item)).toBe('Single line');
    });

    it('returns empty string for empty item', () => {
      const item: OrderedListNode = {
        id: 'test-7',
        nodeType: 'ordered-list',
        content: '',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(OrderedListNodeHelpers.getFirstLine(item)).toBe('');
    });
  });

  describe('getLineCount', () => {
    it('returns correct count for multiline items', () => {
      const item: OrderedListNode = {
        id: 'test-8',
        nodeType: 'ordered-list',
        content: 'Line 1\nLine 2\nLine 3',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(OrderedListNodeHelpers.getLineCount(item)).toBe(3);
    });

    it('returns 1 for single line items', () => {
      const item: OrderedListNode = {
        id: 'test-9',
        nodeType: 'ordered-list',
        content: 'Single line',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(OrderedListNodeHelpers.getLineCount(item)).toBe(1);
    });

    it('returns 0 for empty items', () => {
      const item: OrderedListNode = {
        id: 'test-10',
        nodeType: 'ordered-list',
        content: '',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(OrderedListNodeHelpers.getLineCount(item)).toBe(0);
    });
  });

  describe('extractNumber', () => {
    it('extracts number from numbered items', () => {
      expect(OrderedListNodeHelpers.extractNumber('1. First item')).toBe(1);
      expect(OrderedListNodeHelpers.extractNumber('42. Answer')).toBe(42);
      expect(OrderedListNodeHelpers.extractNumber('100. Hundredth')).toBe(100);
    });

    it('returns null for items without number prefix', () => {
      expect(OrderedListNodeHelpers.extractNumber('No number here')).toBeNull();
      expect(OrderedListNodeHelpers.extractNumber('Just text')).toBeNull();
      expect(OrderedListNodeHelpers.extractNumber('1.No space')).toBeNull();
    });

    it('handles edge cases', () => {
      expect(OrderedListNodeHelpers.extractNumber('0. Zero')).toBe(0);
      expect(OrderedListNodeHelpers.extractNumber('999. Large number')).toBe(999);
      expect(OrderedListNodeHelpers.extractNumber('')).toBeNull();
    });
  });

  describe('hasNumberPrefix', () => {
    it('returns true for items with number prefix', () => {
      const item: OrderedListNode = {
        id: 'test-11',
        nodeType: 'ordered-list',
        content: '1. First item',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(OrderedListNodeHelpers.hasNumberPrefix(item)).toBe(true);
    });

    it('returns false for items without number prefix', () => {
      const item: OrderedListNode = {
        id: 'test-12',
        nodeType: 'ordered-list',
        content: 'No prefix here',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(OrderedListNodeHelpers.hasNumberPrefix(item)).toBe(false);
    });
  });

  describe('removeNumberPrefix', () => {
    it('removes number prefix from content', () => {
      expect(OrderedListNodeHelpers.removeNumberPrefix('1. First item')).toBe('First item');
      expect(OrderedListNodeHelpers.removeNumberPrefix('42. Answer')).toBe('Answer');
      expect(OrderedListNodeHelpers.removeNumberPrefix('100. Item')).toBe('Item');
    });

    it('returns original content if no prefix', () => {
      expect(OrderedListNodeHelpers.removeNumberPrefix('No prefix')).toBe('No prefix');
      expect(OrderedListNodeHelpers.removeNumberPrefix('Just text')).toBe('Just text');
    });

    it('preserves content after prefix removal', () => {
      expect(OrderedListNodeHelpers.removeNumberPrefix('1. Item with 2. inside')).toBe(
        'Item with 2. inside'
      );
    });
  });
});

describe('Integration', () => {
  it('works with type guard and helpers', () => {
    const node: Node = {
      id: 'test-13',
      nodeType: 'ordered-list',
      content: '1. First step: gather requirements\nSub-point a\nSub-point b',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    if (isOrderedListNode(node)) {
      expect(OrderedListNodeHelpers.isMultiline(node)).toBe(true);
      expect(OrderedListNodeHelpers.getLineCount(node)).toBe(3);
      expect(OrderedListNodeHelpers.hasNumberPrefix(node)).toBe(true);
      expect(OrderedListNodeHelpers.extractNumber(node.content)).toBe(1);
      expect(OrderedListNodeHelpers.getFirstLine(node)).toBe('1. First step: gather requirements');
    }
  });

  it('handles various list scenarios', () => {
    const scenarios = [
      {
        content: '1. First item',
        expectedLines: 1,
        expectedMultiline: false,
        expectedNumber: 1,
        expectedHasPrefix: true
      },
      {
        content: 'No number prefix',
        expectedLines: 1,
        expectedMultiline: false,
        expectedNumber: null,
        expectedHasPrefix: false
      },
      {
        content: '42. Multi\nline\nitem',
        expectedLines: 3,
        expectedMultiline: true,
        expectedNumber: 42,
        expectedHasPrefix: true
      }
    ];

    scenarios.forEach(
      ({ content, expectedLines, expectedMultiline, expectedNumber, expectedHasPrefix }) => {
        const item: OrderedListNode = {
          id: `test-${Date.now()}-${Math.random().toString(36).substring(2, 9)}`,
          nodeType: 'ordered-list',
          content,
              createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          version: 1,
          properties: {}
        };

        expect(OrderedListNodeHelpers.getLineCount(item)).toBe(expectedLines);
        expect(OrderedListNodeHelpers.isMultiline(item)).toBe(expectedMultiline);
        expect(OrderedListNodeHelpers.extractNumber(content)).toBe(expectedNumber);
        expect(OrderedListNodeHelpers.hasNumberPrefix(item)).toBe(expectedHasPrefix);
      }
    );
  });

  it('preserves extra properties', () => {
    const node: OrderedListNode = {
      id: 'test-14',
      nodeType: 'ordered-list',
      content: '1. An item',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: { priority: 'high', tags: ['important'] }
    };

    expect(node.properties.priority).toBe('high');
    expect(node.properties.tags).toEqual(['important']);
    expect(isOrderedListNode(node)).toBe(true);
  });
});
