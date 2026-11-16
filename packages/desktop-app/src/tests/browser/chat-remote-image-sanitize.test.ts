import { describe, it, expect } from 'vitest';
import { Marked, Renderer } from 'marked';
import { renderChatImage, REMOTE_IMAGE_PLACEHOLDER_CLASS } from '../../lib/utils/chat-remote-image.js';
import { sanitizeChatHtml } from '../../lib/utils/sanitize-chat-html.js';

// Real Chromium + real DOMPurify, with the chat renderer's image renderer in
// front of the sanitiser exactly as chat-markdown.svelte runs them.
function chat(md: string): HTMLElement {
  const renderer = new Renderer();
  renderer.image = renderChatImage;
  const raw = new Marked().parse(md, { renderer, breaks: true, gfm: true }) as string;
  const root = document.createElement('div');
  root.innerHTML = sanitizeChatHtml(raw);
  return root;
}

describe('a remote image in a chat reply, through the real sanitiser', () => {
  it('survives as a placeholder span with only its three attributes', () => {
    const root = chat('![A chart](https://cdn.example.com/chart.png?id=7)');
    const span = root.querySelector(`.${REMOTE_IMAGE_PLACEHOLDER_CLASS}`);
    expect(span).not.toBeNull();
    expect(span?.getAttributeNames().sort()).toEqual(['class', 'data-alt', 'data-image-url']);
    expect(span?.getAttribute('data-image-url')).toBe('https://cdn.example.com/chart.png?id=7');
    expect(span?.getAttribute('data-alt')).toBe('A chart');
  });

  it('leaves nothing the browser would load: no img element, no src', () => {
    for (const url of ['https://cdn.example.com/a.png', 'http://cdn.example.com/a.png']) {
      const root = chat(`![x](${url})`);
      expect(root.querySelector('img'), url).toBeNull();
      expect(root.innerHTML, url).not.toMatch(/\ssrc\s*=/i);
    }
  });

  it('still drops the remote source of a raw <img> in the message, and adds no placeholder', () => {
    const root = chat('before <img src="https://evil.test/p.png" alt="p"> after');
    expect(root.querySelector('img')?.getAttribute('src') ?? null).toBeNull();
    expect(root.querySelector(`.${REMOTE_IMAGE_PLACEHOLDER_CLASS}`)).toBeNull();
  });

  it('removes a script or an event handler written next to the image', () => {
    const root = chat('![a](https://x.test/p.png) <script>alert(1)</script> <b onclick="alert(2)">b</b>');
    expect(root.querySelector('script')).toBeNull();
    expect(root.querySelector('[onclick]')).toBeNull();
    expect(root.querySelector(`.${REMOTE_IMAGE_PLACEHOLDER_CLASS}`)).not.toBeNull();
  });

  it('keeps an inline data:image and reduces every other address to its alt text', () => {
    expect(chat('![dot](data:image/png;base64,AAAA)').querySelector('img')?.getAttribute('src')).toBe(
      'data:image/png;base64,AAAA'
    );
    for (const url of ['javascript:alert(1)', 'file:///etc/passwd', '//evil.test/x.png']) {
      const root = chat(`![the alt](${url})`);
      expect(root.querySelector('img'), url).toBeNull();
      expect(root.querySelector(`.${REMOTE_IMAGE_PLACEHOLDER_CLASS}`), url).toBeNull();
      expect(root.textContent, url).toContain('the alt');
    }
  });
});
