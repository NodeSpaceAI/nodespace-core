/**
 * Link scheme allow-list shared by every renderer that turns markdown into an
 * anchor. Node content is synced between workspace members, so a link such as
 * `[x](javascript:alert(1))` must never become a clickable anchor.
 */

const SAFE_SCHEMES = new Set(['http', 'https', 'mailto', 'nodespace']);

/**
 * True when `href` may be rendered as a clickable link: it has no scheme (a
 * relative reference or fragment) or one of http, https, mailto, nodespace.
 *
 * Browsers ignore ASCII control characters and spaces at the start of a URL
 * and tabs/newlines anywhere inside it (`java\tscript:`), so those are removed
 * before the scheme is read. Scheme names are case-insensitive.
 */
export function isSafeLinkHref(href: string | null | undefined): boolean {
  if (!href) return true;
  const normalized = href.replace(/[\p{Cc}\p{Z}\p{Cf}]/gu, '');
  const match = normalized.match(/^([a-z][a-z0-9+.-]*):/i);
  if (!match) {
    // A colon before any '/', '?' or '#' that is not a valid scheme (e.g. a
    // leading digit) is not a relative path either; refuse it.
    return !/^[^/?#]*:/.test(normalized);
  }
  return SAFE_SCHEMES.has(match[1].toLowerCase());
}
