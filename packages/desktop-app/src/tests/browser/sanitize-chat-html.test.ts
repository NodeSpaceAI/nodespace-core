import { describe, it, expect } from 'vitest';
import { marked } from 'marked';
import { sanitizeChatHtml } from '../../lib/utils/sanitize-chat-html.js';

// Real Chromium + real DOMPurify (Happy-DOM's DOM drops anchors under DOMPurify),
// with raw HTML passed through marked exactly as the chat renderer does.
const sanitized = (md: string) => sanitizeChatHtml(marked.parse(md, { gfm: true, breaks: true }) as string);

describe('sanitizeChatHtml', () => {
  it('drops unsafe URLs from raw anchors but keeps the text', () => {
    for (const href of [
      '//evil.test/x',
      'file:///etc/passwd',
      'tauri://localhost/x',
      '\\\\evil',
      'blob:https://a.test/1',
      'javascript&#58;alert(1)'
    ]) {
      const out = sanitized(`<a href="${href}">go</a>`);
      expect(out, href).toContain('go');
      expect(out, href).not.toContain('href');
    }
  });

  it('drops unsafe img sources and any srcset', () => {
    for (const src of ['//evil/t.png', 'file:///etc/passwd', 'tauri://localhost/x', 'blob:https://a.test/1']) {
      expect(sanitized(`<img src="${src}">`), src).not.toContain('src=');
    }
    expect(sanitized('<img src="https://a.test/t.png" srcset="//evil/t.png 2x">')).not.toContain('srcset');
  });

  it('keeps safe anchors, images and node-card placeholders', () => {
    expect(sanitized('<a href="https://a.test/x">go</a>')).toContain('href="https://a.test/x"');
    expect(sanitized('<a href="nodespace://abc">go</a>')).toContain('href="nodespace://abc"');
    expect(sanitized('<img src="https://a.test/t.png">')).toContain('src="https://a.test/t.png"');
    expect(sanitized('<img src="data:image/png;base64,AAAA">')).toContain('src="data:image/png');
    expect(sanitized('<span data-node-id="abc" data-display-text="T"></span>')).toContain('data-node-id="abc"');
  });
});
