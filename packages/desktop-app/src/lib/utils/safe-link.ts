/**
 * Link scheme allow-list shared by every renderer that turns markdown into an
 * anchor. Node content is synced between workspace members, so a link such as
 * `[x](javascript:alert(1))` must never become a clickable anchor.
 */

const SAFE_SCHEMES = new Set(['http', 'https', 'mailto', 'nodespace']);

/** Named character references that can appear inside a scheme or its colon. */
const NAMED_ENTITIES: Record<string, string> = {
  colon: ':',
  tab: '\t',
  newline: '\n',
  amp: '&',
  lt: '<',
  gt: '>',
  quot: '"',
  apos: "'",
  lpar: '(',
  rpar: ')',
  sol: '/',
  bsol: '\\',
  period: '.',
  comma: ',',
  semi: ';',
  num: '#',
  percnt: '%',
  excl: '!',
  quest: '?'
};

/**
 * Decodes HTML character references the way a browser does when it reads an
 * attribute value: numeric ones (`&#58;`, `&#x3A;`, with or without the
 * trailing semicolon) and the named ones above. A named reference not in the
 * table is left as-is; `isSafeLinkHref` refuses a scheme that still has `&`.
 */
function decodeEntities(value: string): string {
  return value.replace(/&(?:#(\d+)|#[xX]([0-9a-fA-F]+)|([a-zA-Z][a-zA-Z0-9]*));?/g, (m, dec, hex, name) => {
    if (name !== undefined) return NAMED_ENTITIES[name.toLowerCase()] ?? m;
    const code = dec !== undefined ? parseInt(dec, 10) : parseInt(hex, 16);
    if (!Number.isFinite(code) || code > 0x10ffff) return m;
    try {
      return String.fromCodePoint(code);
    } catch {
      return m;
    }
  });
}

/**
 * True when `href` may be rendered as a clickable link: it has no scheme (a
 * relative reference or fragment) or one of http, https, mailto, nodespace.
 *
 * Character references are decoded first (a browser decodes them in an
 * attribute value, so `javascript&#58;` is `javascript:`), then ASCII
 * control characters, spaces and invisible format characters are removed
 * (browsers ignore leading ones and tabs/newlines anywhere). Scheme names are
 * case-insensitive. Protocol-relative (`//host`) and backslash-led
 * references are refused: they leave the workspace to an arbitrary host.
 */
export function isSafeLinkHref(href: string | null | undefined): boolean {
  if (!href) return true;
  // Decode twice so a double-encoded `&amp;#58;` cannot survive one pass.
  const decoded = decodeEntities(decodeEntities(href));
  const normalized = decoded.replace(/[\p{Cc}\p{Z}\p{Cf}]/gu, '');
  if (/^[/\\]{2}/.test(normalized) || normalized.startsWith('\\')) return false;
  const match = normalized.match(/^([a-z][a-z0-9+.-]*):/i);
  if (!match) {
    // A colon before any '/', '?' or '#' that is not a valid scheme (e.g. a
    // leading digit) is not a relative path either; refuse it. So is a
    // leftover '&' there: an undecoded reference could still spell a scheme.
    return !/^[^/?#]*[:&]/.test(normalized);
  }
  return SAFE_SCHEMES.has(match[1].toLowerCase());
}
