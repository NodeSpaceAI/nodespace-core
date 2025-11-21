/**
 * Tests for QuoteBlock Node Type-Safe Wrapper
 */

import { describe, it, expect } from 'vitest';
import type { Node } from '$lib/types/node';
import {
  type QuoteBlockNode,
  isQuoteBlockNode,
  QuoteBlockNodeHelpers
} from '$lib/types/quote-block-node';

describe('QuoteBlockNode Type Guard', () => {
  it('identifies quote block nodes correctly', () => {
    const quoteBlockNode: Node = {
      id: 'test-1',
      nodeType: 'quote-block',
      content: 'To be or not to be',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isQuoteBlockNode(quoteBlockNode)).toBe(true);
  });

  it('rejects non-quote-block nodes', () => {
    const textNode: Node = {
      id: 'test-2',
      nodeType: 'text',
      content: 'Regular text',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isQuoteBlockNode(textNode)).toBe(false);
  });
});

describe('QuoteBlockNodeHelpers', () => {
  describe('createQuoteBlock', () => {
    it('creates a new quote block with content', () => {
      const quote = QuoteBlockNodeHelpers.createQuoteBlock(
        'The only thing we have to fear is fear itself'
      );

      expect(quote.nodeType).toBe('quote-block');
      expect(quote.content).toBe('The only thing we have to fear is fear itself');
      expect(quote.properties).toEqual({});
    });

    it('creates a quote block with content (parent relationship via backend)', () => {
      const quote = QuoteBlockNodeHelpers.createQuoteBlock('A quote');

      expect(quote.content).toBe('A quote');
      // Note: Parent relationships managed via backend graph queries
    });

    it('generates unique IDs', () => {
      const quote1 = QuoteBlockNodeHelpers.createQuoteBlock('Quote 1');
      const quote2 = QuoteBlockNodeHelpers.createQuoteBlock('Quote 2');

      expect(quote1.id).not.toBe(quote2.id);
    });

    it('sets timestamps', () => {
      const quote = QuoteBlockNodeHelpers.createQuoteBlock('A quote');

      expect(quote.createdAt).toBeDefined();
      expect(quote.modifiedAt).toBeDefined();
      expect(new Date(quote.createdAt).getTime()).toBeGreaterThan(0);
    });
  });

  describe('isMultiline', () => {
    it('returns true for multiline quotes', () => {
      const multilineQuote: QuoteBlockNode = {
        id: 'test-3',
        nodeType: 'quote-block',
        content: 'First line\nSecond line\nThird line',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(QuoteBlockNodeHelpers.isMultiline(multilineQuote)).toBe(true);
    });

    it('returns false for single line quotes', () => {
      const singleLineQuote: QuoteBlockNode = {
        id: 'test-4',
        nodeType: 'quote-block',
        content: 'A single line quote',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(QuoteBlockNodeHelpers.isMultiline(singleLineQuote)).toBe(false);
    });
  });

  describe('getFirstLine', () => {
    it('returns first line from multiline quote', () => {
      const quote: QuoteBlockNode = {
        id: 'test-5',
        nodeType: 'quote-block',
        content: 'First line\nSecond line\nThird line',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(QuoteBlockNodeHelpers.getFirstLine(quote)).toBe('First line');
    });

    it('returns content for single line quote', () => {
      const quote: QuoteBlockNode = {
        id: 'test-6',
        nodeType: 'quote-block',
        content: 'Single line',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(QuoteBlockNodeHelpers.getFirstLine(quote)).toBe('Single line');
    });

    it('returns empty string for empty quote', () => {
      const quote: QuoteBlockNode = {
        id: 'test-7',
        nodeType: 'quote-block',
        content: '',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(QuoteBlockNodeHelpers.getFirstLine(quote)).toBe('');
    });
  });

  describe('getLineCount', () => {
    it('returns correct count for multiline quotes', () => {
      const quote: QuoteBlockNode = {
        id: 'test-8',
        nodeType: 'quote-block',
        content: 'Line 1\nLine 2\nLine 3',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(QuoteBlockNodeHelpers.getLineCount(quote)).toBe(3);
    });

    it('returns 1 for single line quotes', () => {
      const quote: QuoteBlockNode = {
        id: 'test-9',
        nodeType: 'quote-block',
        content: 'Single line',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(QuoteBlockNodeHelpers.getLineCount(quote)).toBe(1);
    });

    it('returns 0 for empty quotes', () => {
      const quote: QuoteBlockNode = {
        id: 'test-10',
        nodeType: 'quote-block',
        content: '',
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(QuoteBlockNodeHelpers.getLineCount(quote)).toBe(0);
    });
  });
});

describe('Integration', () => {
  it('works with type guard and helpers', () => {
    const node: Node = {
      id: 'test-11',
      nodeType: 'quote-block',
      content: 'Be yourself; everyone else is already taken.\n— Oscar Wilde',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    if (isQuoteBlockNode(node)) {
      expect(QuoteBlockNodeHelpers.isMultiline(node)).toBe(true);
      expect(QuoteBlockNodeHelpers.getLineCount(node)).toBe(2);
      expect(QuoteBlockNodeHelpers.getFirstLine(node)).toBe(
        'Be yourself; everyone else is already taken.'
      );
    }
  });

  it('handles various quote scenarios', () => {
    const scenarios = [
      {
        content: 'In the middle of difficulty lies opportunity.',
        expectedLines: 1,
        expectedMultiline: false
      },
      {
        content:
          'Success is not final, failure is not fatal:\nit is the courage to continue that counts.',
        expectedLines: 2,
        expectedMultiline: true
      },
      {
        content: 'Line 1\nLine 2\nLine 3\nLine 4',
        expectedLines: 4,
        expectedMultiline: true
      }
    ];

    scenarios.forEach(({ content, expectedLines, expectedMultiline }) => {
      const quote: QuoteBlockNode = {
        id: `test-${Date.now()}-${Math.random().toString(36).substring(2, 9)}`,
        nodeType: 'quote-block',
        content,
          createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(QuoteBlockNodeHelpers.getLineCount(quote)).toBe(expectedLines);
      expect(QuoteBlockNodeHelpers.isMultiline(quote)).toBe(expectedMultiline);
    });
  });

  it('preserves extra properties', () => {
    const node: QuoteBlockNode = {
      id: 'test-12',
      nodeType: 'quote-block',
      content: 'A quote',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: { author: 'Anonymous', source: 'Internet' }
    };

    expect(node.properties.author).toBe('Anonymous');
    expect(node.properties.source).toBe('Internet');
    expect(isQuoteBlockNode(node)).toBe(true);
  });
});
