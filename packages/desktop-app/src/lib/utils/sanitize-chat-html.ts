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

const LINK_ATTRS = ['href', 'xlink:href', 'action', 'formaction'];
const SRC_ATTRS = ['src'];
const DATA_IMAGE = /^data:image\/(png|jpe?g|gif|webp);/i;

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
    for (const attr of SRC_ATTRS) {
      const value = node.getAttribute?.(attr);
      if (value != null && !isSafeLinkHref(value) && !DATA_IMAGE.test(value.trim())) {
        node.removeAttribute(attr);
      }
    }
    // srcset lists several URLs; not worth parsing, so it is not allowed.
    node.removeAttribute?.('srcset');
  });
  return purify.sanitize(html, {
    ADD_ATTR: ['data-node-id', 'data-display-text'],
    ALLOW_UNKNOWN_PROTOCOLS: true
  });
}
