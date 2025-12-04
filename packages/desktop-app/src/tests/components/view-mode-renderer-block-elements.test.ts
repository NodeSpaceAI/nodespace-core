/**
 * Tests for ViewModeRenderer with Block Elements
 *
 * Tests the enableBlockElements prop functionality used by quote-blocks
 * to render headings, lists, and paragraphs as proper HTML elements.
 */

import { describe, it, expect } from 'vitest';
import { marked } from 'marked';
import type { Tokens } from 'marked';

// Test the parsing logic directly (component rendering tested via integration tests)
describe('ViewModeRenderer Block Elements Parsing', () => {
  /**
   * Simulate the ViewModeRenderer's parsing logic for block elements
   * This mirrors the actual component logic to test token processing
   */
  function parseWithBlockElements(content: string): { type: string; data?: unknown }[] {
    const tokens = marked.lexer(content);
    const results: { type: string; data?: unknown }[] = [];

    for (const token of tokens) {
      if (token.type === 'heading') {
        const heading = token as Tokens.Heading;
        results.push({
          type: 'heading',
          data: { level: heading.depth, text: heading.text }
        });
      } else if (token.type === 'list') {
        const list = token as Tokens.List;
        results.push({
          type: 'list',
          data: {
            ordered: list.ordered,
            items: list.items.map((item) => item.text)
          }
        });
      } else if (token.type === 'paragraph') {
        const para = token as Tokens.Paragraph;
        results.push({
          type: 'paragraph',
          data: { text: para.text }
        });
      }
    }

    return results;
  }

  describe('Heading Parsing', () => {
    it('parses h2 headings', () => {
      const content = '## Hello World';
      const results = parseWithBlockElements(content);

      expect(results).toHaveLength(1);
      expect(results[0].type).toBe('heading');
      expect((results[0].data as { level: number }).level).toBe(2);
      expect((results[0].data as { text: string }).text).toBe('Hello World');
    });

    it('parses h1 through h6 headings', () => {
      const headings = [
        { md: '# H1', level: 1 },
        { md: '## H2', level: 2 },
        { md: '### H3', level: 3 },
        { md: '#### H4', level: 4 },
        { md: '##### H5', level: 5 },
        { md: '###### H6', level: 6 }
      ];

      for (const { md, level } of headings) {
        const results = parseWithBlockElements(md);
        expect(results[0].type).toBe('heading');
        expect((results[0].data as { level: number }).level).toBe(level);
      }
    });

    it('parses heading with inline bold', () => {
      const content = '## **IMPORTANT** Notice';
      const results = parseWithBlockElements(content);

      expect(results).toHaveLength(1);
      expect(results[0].type).toBe('heading');
      // Marked includes the ** in raw but strips for text
      expect((results[0].data as { text: string }).text).toContain('IMPORTANT');
    });
  });

  describe('List Parsing', () => {
    it('parses unordered lists', () => {
      const content = '- Item 1\n- Item 2\n- Item 3';
      const results = parseWithBlockElements(content);

      expect(results).toHaveLength(1);
      expect(results[0].type).toBe('list');
      expect((results[0].data as { ordered: boolean }).ordered).toBe(false);
      expect((results[0].data as { items: string[] }).items).toHaveLength(3);
    });

    it('parses ordered lists', () => {
      const content = '1. First\n2. Second\n3. Third';
      const results = parseWithBlockElements(content);

      expect(results).toHaveLength(1);
      expect(results[0].type).toBe('list');
      expect((results[0].data as { ordered: boolean }).ordered).toBe(true);
      expect((results[0].data as { items: string[] }).items).toHaveLength(3);
    });

    it('parses list items with inline formatting', () => {
      const content = '- **Bold** item\n- *Italic* item\n- `code` item';
      const results = parseWithBlockElements(content);

      expect(results).toHaveLength(1);
      expect(results[0].type).toBe('list');
      const items = (results[0].data as { items: string[] }).items;
      expect(items).toHaveLength(3);
    });
  });

  describe('Mixed Content Parsing', () => {
    it('parses heading followed by list', () => {
      const content = '## Section Title\n\n- Item A\n- Item B';
      const results = parseWithBlockElements(content);

      expect(results.length).toBeGreaterThanOrEqual(2);
      expect(results[0].type).toBe('heading');
      expect(results[1].type).toBe('list');
    });

    it('parses complex quote-like content', () => {
      const content = `## UNIVERSAL PROCESS NOTICE

**This process applies EQUALLY to:**
- AI Agents (Claude, GPT, custom agents)
- Human Engineers (frontend, backend, full-stack)
- Human Architects (senior, principal, staff)

**NO EXCEPTIONS**`;

      const results = parseWithBlockElements(content);

      // Should have heading, paragraph, list, paragraph
      const headings = results.filter((r) => r.type === 'heading');
      const lists = results.filter((r) => r.type === 'list');
      const paragraphs = results.filter((r) => r.type === 'paragraph');

      expect(headings.length).toBeGreaterThanOrEqual(1);
      expect(lists.length).toBeGreaterThanOrEqual(1);
      expect(paragraphs.length).toBeGreaterThanOrEqual(1);
    });
  });

  describe('Paragraph Parsing', () => {
    it('parses paragraphs with inline formatting', () => {
      const content = '**Bold text** and *italic text*';
      const results = parseWithBlockElements(content);

      expect(results).toHaveLength(1);
      expect(results[0].type).toBe('paragraph');
    });

    it('parses multiple paragraphs', () => {
      const content = 'First paragraph.\n\nSecond paragraph.';
      const results = parseWithBlockElements(content);

      expect(results.length).toBe(2);
      expect(results.every((r) => r.type === 'paragraph')).toBe(true);
    });
  });
});

describe('Quote Block Display Content', () => {
  /**
   * Simulate the quote-block's displayContent extraction
   * (stripping > prefixes)
   */
  function extractQuoteForDisplay(content: string): string {
    const lines = content.split('\n');
    const strippedLines = lines.map((line) => line.replace(/^>\s?/, ''));
    return strippedLines.join('\n');
  }

  it('strips > prefix from single line', () => {
    const content = '> Hello world';
    expect(extractQuoteForDisplay(content)).toBe('Hello world');
  });

  it('strips > prefix from multiline content', () => {
    const content = '> ## Heading\n> \n> - Item 1\n> - Item 2';
    const expected = '## Heading\n\n- Item 1\n- Item 2';
    expect(extractQuoteForDisplay(content)).toBe(expected);
  });

  it('handles content without > prefix', () => {
    const content = 'No prefix here';
    expect(extractQuoteForDisplay(content)).toBe('No prefix here');
  });

  it('preserves inline formatting after stripping', () => {
    const content = '> **Bold** and *italic*';
    expect(extractQuoteForDisplay(content)).toBe('**Bold** and *italic*');
  });

  it('processes complex quote content correctly', () => {
    const rawContent = `> ## NOTICE
>
> **Important:**
> - Point 1
> - Point 2`;

    const displayContent = extractQuoteForDisplay(rawContent);

    // Verify the display content can be parsed as valid markdown
    const tokens = marked.lexer(displayContent);
    const tokenTypes = tokens.map((t) => t.type);

    expect(tokenTypes).toContain('heading');
    expect(tokenTypes).toContain('list');
  });
});
