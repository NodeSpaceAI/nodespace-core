/**
 * Sanitizer for chat markdown output.
 *
 * The chat renderer turns markdown links into anchors itself, but raw HTML in
 * a message (`<a href="//evil/x">`, `<img src="file:///...">`) goes straight
 * through marked. DOMPurify with ALLOW_UNKNOWN_PROTOCOLS (kept so the
 * `nodespace://` placeholders survive) lets protocol-relative, `file:`,
 * `tauri:` and `blob:` URLs through, and the app-shell click handler only
 * intercepts http(s) and nodespace links. A hook therefore applies the same
 * allow-list as every other link renderer to every URL-bearing attribute.
 */

import createDOMPurify, { type Config } from 'dompurify';
import { isSafeLinkHref } from '$lib/utils/safe-link';

const LINK_ATTRS = ['href', 'xlink:href', 'cite'];
// Form submission targets: chat has no use for them, so they are dropped.
const DROPPED_ATTRS = ['action', 'formaction'];
// Attributes that make the browser (or a click) contact a URL without going
// through the link or fetch allow-lists above: `ping` posts to every listed
// URL on click, `longdesc` names a description document, and `data` is the
// resource URL of <object>. None has a use in chat, so they are dropped.
const NETWORK_ATTRS = ['ping', 'longdesc', 'data'];
// Elements whose href is a click target. On any other element (svg image,
// feImage, use, ...) href is fetched by the browser, so it follows
// FETCHED_ATTRS' rule instead of the link allow-list.
const LINK_TAGS = new Set(['a', 'area']);
// Attributes the browser fetches on its own (no click needed). Outside the
// link allow-list on purpose: an absolute http(s) URL would be a tracking
// pixel / IP leak, so only a relative reference qualifies (img and the
// image-valued attributes may also carry an inline data:image/*).
const FETCHED_ATTRS = ['src', 'poster', 'background'];
const DATA_IMAGE = /^data:image\/(png|jpe?g|gif|webp);/i;
const IMAGE_TAGS = new Set(['IMG']);

/** Removes `ping`, `longdesc` and `data` from an element. DOMPurify's default
 *  allow-list already omits them; this keeps them out if the config widens. */
export function stripNetworkAttrs(node: Element): void {
  for (const attr of NETWORK_ATTRS) node.removeAttribute?.(attr);
}

/** True for a schemeless, non-protocol-relative reference (`img/a.png`, `/a`). */
function isRelativeRef(value: string): boolean {
  if (!isSafeLinkHref(value)) return false;
  const stripped = value.replace(/[\p{Cc}\p{Z}\p{Cf}]/gu, '');
  return !/^[^/?#]*[:&]/.test(stripped);
}

/** Sanitizes chat HTML. A fresh DOMPurify instance per call, so the hook is
 *  scoped to this call and never leaks onto the shared singleton. */
export function sanitizeChatHtml(html: string): string {
  return sanitizeWithConfig(html, {});
}

/** Same as sanitizeChatHtml with extra DOMPurify options merged over the
 *  defaults; lets tests widen the allow-list to exercise the hook alone. */
export function sanitizeWithConfig(html: string, extra: Config): string {
  // Bound to the live window each call: the default export is bound to
  // whichever window existed at first import (import-order sensitive).
  const purify = createDOMPurify(document.defaultView ?? undefined);
  purify.addHook('afterSanitizeAttributes', (node) => {
    const isLinkTag = LINK_TAGS.has(node.nodeName.toLowerCase());
    for (const attr of DROPPED_ATTRS) node.removeAttribute?.(attr);
    stripNetworkAttrs(node);
    for (const attr of LINK_ATTRS) {
      const value = node.getAttribute?.(attr);
      if (value == null) continue;
      const clickOnly = attr === 'cite' || isLinkTag;
      if (clickOnly && !isSafeLinkHref(value)) node.removeAttribute(attr);
      else if (!clickOnly && !isRelativeRef(value) && !DATA_IMAGE.test(value.trim())) {
        node.removeAttribute(attr);
      }
    }
    const allowsDataImage = IMAGE_TAGS.has(node.nodeName);
    for (const attr of FETCHED_ATTRS) {
      const value = node.getAttribute?.(attr);
      if (value == null) continue;
      const imageValued = attr !== 'src' || allowsDataImage;
      if (!isRelativeRef(value) && !(imageValued && DATA_IMAGE.test(value.trim()))) {
        node.removeAttribute(attr);
      }
    }
    // srcset lists several URLs; not worth parsing, so it is not allowed.
    node.removeAttribute?.('srcset');
  });
  const { ADD_ATTR: extraAttr = [], ADD_TAGS: extraTags = [], ...rest } = extra;
  return purify.sanitize(html, {
    ...rest,
    ADD_TAGS: extraTags,
    ADD_ATTR: ['data-node-id', 'data-display-text', ...(extraAttr as string[])],
    ALLOW_UNKNOWN_PROTOCOLS: true,
    // Inline style can fetch (background:url(), image-set()) and overlay the
    // app UI (position:fixed). Chat markdown never needs it, so it is dropped
    // rather than filtered.
    FORBID_ATTR: ['style'],
    // A <style> element restyles the whole app and can @import/url() remote
    // CSS, sidestepping the style attribute removal. App styles live in the
    // Svelte component, never in sanitised message HTML.
    // Chat has no use for interactive form controls; a message must not be
    // able to render a form or input fields that look like an app prompt.
    // Their text content is kept.
    FORBID_TAGS: ['style', 'form', 'input', 'textarea', 'select', 'option', 'optgroup', 'button']
  });
}
