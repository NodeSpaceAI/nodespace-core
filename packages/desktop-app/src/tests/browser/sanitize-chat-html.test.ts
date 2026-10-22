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
    expect(sanitized('<img src="img/t.png">')).toContain('src="img/t.png"');
    expect(sanitized('<img src="/assets/t.png">')).toContain('src="/assets/t.png"');
    expect(sanitized('<img src="data:image/png;base64,AAAA">')).toContain('src="data:image/png');
    expect(sanitized('<span data-node-id="abc" data-display-text="T"></span>')).toContain('data-node-id="abc"');
  });

  it('drops URL-bearing poster, background and cite attributes', () => {
    expect(sanitized('<video poster="//evil/p.png"></video>')).not.toContain('poster');
    expect(sanitized('<video poster="https://evil.test/p.png"></video>')).not.toContain('poster');
    expect(sanitized('<table background="//evil/b.png"><tr><td background="//evil/b.png">x</td></tr></table>')).not.toContain('background');
    expect(sanitized('<blockquote cite="//evil/c">q</blockquote>')).not.toContain('cite');
    expect(sanitized('<blockquote cite="javascript&#58;alert(1)">q</blockquote>')).not.toContain('cite');
    expect(sanitized('<blockquote cite="https://a.test/c">q</blockquote>')).toContain('cite="https://a.test/c"');
    expect(sanitized('<video poster="img/p.png"></video>')).toContain('poster="img/p.png"');
  });

  it('drops inline style but keeps the element and its text', () => {
    for (const style of [
      'background:url(//evil/x.png)',
      'background-image:image-set(url(//evil/x.png) 1x)',
      'position:fixed;top:0;left:0;width:100%;height:100%'
    ]) {
      const out = sanitized(`<div style="${style}">hello</div>`);
      expect(out, style).toContain('hello');
      expect(out, style).not.toContain('style');
    }
  });

  it('drops <style> elements, in HTML and inside svg', () => {
    for (const html of [
      '<div>a</div><style>*{background:url(//evil.test/x)}</style>',
      '<svg><style>@import "//evil.test/a.css"</style></svg>'
    ]) {
      const out = sanitized(html);
      expect(out, html).not.toMatch(/<style/i);
      expect(out, html).not.toContain('evil.test');
    }
  });

  it('keeps only relative or data:image sources for fetched media', () => {
    for (const html of [
      '<img src="https://evil.test/t.png">',
      '<img src="http://evil.test/t.png">',
      '<audio src="https://evil.test/a.mp3"></audio>',
      '<video src="https://evil.test/v.mp4"></video>',
      '<audio><source src="https://evil.test/a.mp3"></audio>',
      '<video><track src="https://evil.test/t.vtt"></video>'
    ]) {
      expect(sanitized(html), html).not.toContain('src=');
    }
    // Regression guards: already dropped before this change (svg data, entity scheme, non-image data).
    for (const html of [
      '<img src="data:image/svg+xml;base64,AAAA">',
      '<img src="java&#115;cript:alert(1)">',
      '<audio src="data:image/png;base64,AAAA"></audio>'
    ]) {
      expect(sanitized(html), html).not.toContain('src=');
    }
    expect(sanitized('<audio src="media/a.mp3"></audio>')).toContain('src="media/a.mp3"');
    expect(sanitized('<video><source src="media/v.mp4"></video>')).toContain('src="media/v.mp4"');
  });

  it('applies the fetched-URL rule to href on non-anchor elements (svg image, feImage, use)', () => {
    for (const html of [
      '<svg><image href="https://evil.test/x.png"/></svg>',
      '<svg><image xlink:href="https://evil.test/x.png"/></svg>',
      '<svg><filter><feImage href="https://evil.test/x.png"/></filter></svg>',
      '<svg><use href="https://evil.test/x.svg#a"/></svg>',
      '<svg><image href="//evil.test/x.png"/></svg>'
    ]) {
      expect(sanitized(html), html).not.toContain('evil.test');
    }
    expect(sanitized('<svg><image href="img/x.png"/></svg>')).toContain('href="img/x.png"');
    // Anchors, including svg anchors, keep the link allow-list.
    expect(sanitized('<svg><a href="https://a.test/x"><text>t</text></a></svg>')).toContain('href="https://a.test/x"');
  });

  it('drops form submission targets', () => {
    expect(sanitized('<form action="https://evil.test/s"><button formaction="https://evil.test/t">x</button></form>')).not.toContain('evil.test');
  });

  it('leaves link hrefs on the link allow-list', () => {
    expect(sanitized('<a href="https://a.test/x">go</a>')).toContain('href="https://a.test/x"');
    expect(sanitized('<a href="mailto:a@b.test">go</a>')).toContain('href="mailto:a@b.test"');
  });
});
