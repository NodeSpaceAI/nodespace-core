/**
 * Tests for the custom marked.js renderer configuration (marked-config.ts).
 *
 * marked-config.ts registers renderer overrides at import time via `marked.use({...})`.
 * Importing it here for its side effect wires those overrides into the shared `marked`
 * singleton, then we assert on `marked.parse`/`marked.parseInline` output.
 */
import { describe, it, expect } from 'vitest';
import { marked } from 'marked';
import '$lib/utils/marked-config';

describe('marked-config custom renderers', () => {
  it('renders **bold** with the markdown-bold span class', () => {
    const html = marked.parseInline('**bold text**') as string;
    expect(html).toContain('<span class="markdown-bold">bold text</span>');
  });

  it('renders *italic* with the markdown-italic span class', () => {
    const html = marked.parseInline('*italic text*') as string;
    expect(html).toContain('<span class="markdown-italic">italic text</span>');
  });

  it('renders `code` with the markdown-code-inline class', () => {
    const html = marked.parseInline('`code`') as string;
    expect(html).toContain('<code class="markdown-code-inline">code</code>');
  });

  it('does not wrap a paragraph in <p> tags', () => {
    const html = marked.parse('Just a plain paragraph of text.') as string;
    expect(html).not.toContain('<p>');
    expect(html).not.toContain('</p>');
    expect(html).toContain('Just a plain paragraph of text.');
  });

  it('preserves "# Header" as literal plain text, not an <h1>', () => {
    const html = marked.parse('# Header') as string;
    expect(html).not.toContain('<h1>');
    expect(html).toContain('# Header');
  });

  it('preserves headers of other levels as literal "##"/"###" text', () => {
    const html2 = marked.parse('## Subheader') as string;
    expect(html2).not.toContain('<h2>');
    expect(html2).toContain('## Subheader');

    const html3 = marked.parse('### Sub-subheader') as string;
    expect(html3).not.toContain('<h3>');
    expect(html3).toContain('### Sub-subheader');
  });

  it('preserves an ordered list as plain text lines, not <ol><li>', () => {
    const html = marked.parse('1. Item\n2. Item2') as string;
    expect(html).not.toContain('<ol>');
    expect(html).not.toContain('<li>');
    expect(html).toContain('1. Item');
    expect(html).toContain('2. Item2');
  });

  it('renders an unordered list with a "- " marker, not <ul><li>', () => {
    const html = marked.parse('- Alpha\n- Beta') as string;
    expect(html).not.toContain('<ul>');
    expect(html).not.toContain('<li>');
    expect(html).toContain('- Alpha');
    expect(html).toContain('- Beta');
  });

  it('preserves task-list checkbox markers for unchecked and checked items', () => {
    const html = marked.parse('- [ ] todo\n- [x] done') as string;
    expect(html).toContain('[ ] todo');
    expect(html).toContain('[x] done');
  });

  it('resolves nested formatting through recursive parseInline (bold containing italic)', () => {
    const html = marked.parseInline('**bold *italic* text**') as string;
    expect(html).toContain('<span class="markdown-bold">');
    expect(html).toContain('<span class="markdown-italic">italic</span>');
    expect(html).toContain('bold');
    expect(html).toContain('text');
  });

  it('resolves nested formatting through the recursive list-item parser (bold inside a list item)', () => {
    const html = marked.parse('- Item with **bold** text') as string;
    expect(html).toContain('- Item with');
    expect(html).toContain('<span class="markdown-bold">bold</span>');
  });

  it('renders an ordered list starting at a custom start number', () => {
    const html = marked.parse('5. Fifth\n6. Sixth') as string;
    expect(html).toContain('5. Fifth');
    expect(html).toContain('6. Sixth');
  });

  it('falls back to a start of 1 when the list token has a falsy start (defensive `|| 1` branch)', () => {
    // marked's own lexer always populates `start` with a real number for ordered
    // lists, so this branch is unreachable through the public parse API. Exercise
    // it directly via the registered renderer, using a hand-built token whose
    // `start` is falsy (0) to prove the fallback itself is correct.
    const renderer = marked.defaults.renderer as unknown as {
      list(token: {
        ordered: boolean;
        start: number;
        items: Array<{ tokens: unknown[]; task: boolean; checked?: boolean }>;
      }): string;
      parser: { parse(tokens: unknown[]): string };
    };
    const token = {
      ordered: true,
      start: 0,
      items: [
        { tokens: [{ type: 'text', raw: 'First', text: 'First' }], task: false },
        { tokens: [{ type: 'text', raw: 'Second', text: 'Second' }], task: false }
      ]
    };

    const text = renderer.list.call(renderer, token);

    expect(text).toContain('1. First');
    expect(text).toContain('2. Second');
  });
});

describe('marked-config link scheme allow-list', () => {
  it('keeps safe links as anchors', () => {
    for (const url of ['https://a.test/x', 'http://a.test', 'mailto:a@b.test', 'nodespace://abc', 'rel/path', '#frag']) {
      expect(marked.parseInline(`[t](${url})`) as string).toContain('<a href=');
    }
  });

  it('renders unsafe schemes as plain label text', () => {
    for (const url of [
      'javascript:alert(1)',
      'JaVaScRiPt:alert(1)',
      ' javascript:alert(1)',
      'java%0Ascript:alert(1)',
      'data:text/html,x',
      'vbscript:msgbox(1)',
      'file:///etc/passwd'
    ]) {
      const html = marked.parseInline(`[click](<${url}>)`) as string;
      expect(html, url).not.toContain('<a');
      expect(html, url).toContain('click');
    }
  });

  it('renders entity-encoded and protocol-relative hrefs as plain label text', () => {
    for (const url of ['javascript&#58;alert(1)', 'javascript&colon;alert(1)', 'javascript&#x3A;alert(1)', 'java&#9;script:alert(1)', 'java&Tab;script:alert(1)', 'data&colon;text/html,x', 'javascript&#58alert(1)', 'javascript&amp;#58;alert(1)', 'jav&#x61;script:alert(1)', 'javascript&unknownref;:alert(1)'].concat(['//evil.test/x'])) {
      const html = marked.parseInline(`[click](${url})`) as string;
      expect(html, url).not.toContain('<a');
      expect(html, url).toContain('click');
    }
  });
});
