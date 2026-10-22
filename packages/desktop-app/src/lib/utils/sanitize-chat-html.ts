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

import createDOMPurify from 'dompurify';
import { isSafeLinkHref } from '$lib/utils/safe-link';

const LINK_ATTRS = ['href', 'xlink:href', 'action', 'formaction', 'cite'];
// Attributes the browser fetches on its own (no click needed). Outside the
// link allow-list on purpose: an absolute http(s) URL would be a tracking
// pixel / IP leak, so only a relative reference qualifies (img and the
// image-valued attributes may also carry an inline data:image/*).
const FETCHED_ATTRS = ['src', 'poster', 'background'];
const DATA_IMAGE = /^data:image\/(png|jpe?g|gif|webp);/i;
const IMAGE_TAGS = new Set(['IMG']);

/** True for a schemeless, non-protocol-relative reference (`img/a.png`, `/a`). */
function isRelativeRef(value: string): boolean {
  if (!isSafeLinkHref(value)) return false;
  const stripped = value.replace(/[\p{Cc}\p{Z}\p{Cf}]/gu, '');
  return !/^[^/?#]*[:&]/.test(stripped);
}

/** Sanitizes chat HTML. A fresh DOMPurify instance per call, so the hook is
 *  scoped to this call and never leaks onto the shared singleton. */
export function sanitizeChatHtml(html: string): string {
  // Bound to the live window each call: the default export is bound to
  // whichever window existed at first import (import-order sensitive).
  const purify = createDOMPurify(document.defaultView ?? undefined);
  purify.addHook('afterSanitizeAttributes', (node) => {
    for (const attr of LINK_ATTRS) {
      const value = node.getAttribute?.(attr);
      if (value != null && !isSafeLinkHref(value)) node.removeAttribute(attr);
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
  return purify.sanitize(html, {
    ADD_ATTR: ['data-node-id', 'data-display-text'],
    ALLOW_UNKNOWN_PROTOCOLS: true,
    // Inline style can fetch (background:url(), image-set()) and overlay the
    // app UI (position:fixed). Chat markdown never needs it, so it is dropped
    // rather than filtered.
    FORBID_ATTR: ['style']
  });
}
