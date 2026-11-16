/**
 * Markdown image rendering for AI chat replies.
 *
 * A remote image never loads on its own: the content security policy keeps
 * `img-src` to the app, `asset:` and `data:`, and the sanitiser drops every
 * network source. An image whose address is `http(s)` is therefore written as
 * a placeholder span that carries the address and the alt text as data
 * attributes. The chat renderer mounts `RemoteImagePlaceholder` on it after
 * sanitising, which shows the address and a "Load image" button; nothing is
 * requested until the person presses it.
 *
 * Every other kind of image address is not a remote image: an inline
 * `data:image/*` goes through marked's own renderer (the sanitiser still
 * filters it), and anything else (`file:`, `javascript:`, a protocol-relative
 * or relative address) is shown as its alt text only.
 */

import { Renderer, type Tokens } from 'marked';

/** Class of the span that a placeholder component is mounted on. */
export const REMOTE_IMAGE_PLACEHOLDER_CLASS = 'ns-remote-image-placeholder';

const REMOTE_URL = /^https?:\/\//i;
const DATA_IMAGE = /^data:image\/(png|jpe?g|gif|webp);/i;

/** Escapes text for use inside a double-quoted HTML attribute value. */
export function escapeHtmlAttribute(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/"/g, '&quot;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

/** Escapes text for use as HTML element content. */
function escapeHtmlText(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

/** The alt text of an image token as plain text, without inline markup. */
function altText(this: Renderer, token: Tokens.Image): string {
  const parsed = token.tokens?.length
    ? this.parser.parseInline(token.tokens, this.parser.textRenderer)
    : token.text;
  return parsed ?? '';
}

/**
 * `renderer.image` for the chat Marked instance. Called with the renderer as
 * `this`, which is how marked calls it.
 */
export function renderChatImage(this: Renderer, token: Tokens.Image): string {
  const href = (token.href ?? '').trim();
  const alt = altText.call(this, token);
  if (REMOTE_URL.test(href)) {
    return `<span class="${REMOTE_IMAGE_PLACEHOLDER_CLASS}" data-image-url="${escapeHtmlAttribute(href)}" data-alt="${escapeHtmlAttribute(alt)}"></span>`;
  }
  if (DATA_IMAGE.test(href)) {
    return Renderer.prototype.image.call(this, token);
  }
  return escapeHtmlText(alt);
}
