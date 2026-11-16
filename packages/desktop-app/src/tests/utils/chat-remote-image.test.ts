/**
 * A remote image in a chat reply is rendered as a placeholder that carries the
 * address as data, never as an element the browser would load.
 */
import { describe, it, expect } from 'vitest';
import { Marked, Renderer } from 'marked';
import {
  renderChatImage,
  REMOTE_IMAGE_PLACEHOLDER_CLASS,
  escapeHtmlAttribute
} from '$lib/utils/chat-remote-image';

// The renderer's own output, before sanitising. The sanitiser's behaviour on
// that output is checked in the browser tier (Happy-DOM's DOMPurify is not
// functional), in src/tests/browser/chat-remote-image-sanitize.test.ts.
function chatHtml(markdown: string): string {
  const renderer = new Renderer();
  renderer.image = renderChatImage;
  return new Marked().parse(markdown, { renderer, breaks: true, gfm: true }) as string;
}

function dom(html: string): HTMLElement {
  const root = document.createElement('div');
  root.innerHTML = html;
  return root;
}

describe('renderChatImage', () => {
  it('turns an https image into a placeholder span that carries the address and alt text', () => {
    const root = dom(chatHtml('![A chart](https://cdn.example.com/chart.png?id=7)'));
    const span = root.querySelector(`.${REMOTE_IMAGE_PLACEHOLDER_CLASS}`);
    expect(span).not.toBeNull();
    expect(span?.getAttribute('data-image-url')).toBe('https://cdn.example.com/chart.png?id=7');
    expect(span?.getAttribute('data-alt')).toBe('A chart');
  });

  it('writes no element the browser would load, for either scheme', () => {
    for (const url of ['https://cdn.example.com/a.png', 'http://cdn.example.com/a.png']) {
      const html = chatHtml(`![x](${url})`);
      const root = dom(html);
      expect(root.querySelector('img'), url).toBeNull();
      expect(html, url).not.toMatch(/\ssrc\s*=/i);
      expect(root.querySelector(`.${REMOTE_IMAGE_PLACEHOLDER_CLASS}`), url).not.toBeNull();
    }
  });

  it('keeps an inline data:image as an image', () => {
    const root = dom(chatHtml('![dot](data:image/png;base64,AAAA)'));
    expect(root.querySelector('img')?.getAttribute('src')).toBe('data:image/png;base64,AAAA');
    expect(root.querySelector(`.${REMOTE_IMAGE_PLACEHOLDER_CLASS}`)).toBeNull();
  });

  it('shows only the alt text for any other kind of address', () => {
    for (const url of [
      'javascript:alert(1)',
      'file:///etc/passwd',
      '//evil.test/x.png',
      'data:image/svg+xml;base64,AAAA',
      'ftp://evil.test/x.png',
      'images/local.png'
    ]) {
      const root = dom(chatHtml(`![the alt](${url})`));
      expect(root.querySelector('img'), url).toBeNull();
      expect(root.querySelector(`.${REMOTE_IMAGE_PLACEHOLDER_CLASS}`), url).toBeNull();
      expect(root.textContent, url).toContain('the alt');
    }
  });

  it('keeps a quote or a tag in the alt text or the address out of the markup', () => {
    const token = {
      type: 'image' as const,
      raw: '',
      href: 'https://x.test/p.png?q="><img src=x onerror=alert(1)>',
      title: null,
      text: 'a"b<script>alert(1)</script>c&d',
      tokens: []
    };
    const html = renderChatImage.call(new Renderer(), token);
    const root = dom(html);
    expect(root.children).toHaveLength(1);
    expect(root.querySelector('img')).toBeNull();
    expect(root.querySelector('script')).toBeNull();
    const span = root.firstElementChild as HTMLElement;
    // Whatever the values hold, they are values, not new attributes or elements,
    // and they read back exactly as written.
    expect(span.getAttributeNames().sort()).toEqual(['class', 'data-alt', 'data-image-url']);
    expect(span.getAttribute('data-image-url')).toBe(token.href);
    expect(span.getAttribute('data-alt')).toBe(token.text);
  });

  it('escapes attribute values', () => {
    expect(escapeHtmlAttribute(`a&b"c<d>e`)).toBe('a&amp;b&quot;c&lt;d&gt;e');
  });
});
