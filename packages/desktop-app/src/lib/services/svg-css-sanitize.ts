/* global CSSStyleDeclaration, CSSStyleSheet, CSSStyleRule */
/**
 * CSS allow-list for sanitised SVG (mermaid output).
 *
 * A <style> element inside inline SVG applies to the whole document, so its
 * text is parsed with the browser's CSS parser and rebuilt from scratch:
 * only top-level style rules survive, each selector must stay under the
 * diagram's root id, and each declaration is re-serialised after checks.
 * Every at-rule (@media, @keyframes, @scope, @import, ...) and every nested
 * rule is dropped because none of them can be scoped safely. Without a CSS
 * parser the style element is dropped (fail closed).
 */

const ROOT_ID = /^[A-Za-z_][\w-]*$/;
const IDENT = '-?[A-Za-z_][\\w-]*';
const SIMPLE =
  `(?:\\.${IDENT}|\\[${IDENT}(?:[$^*]?=(?:"[\\w .#-]*"|${IDENT}|[\\w.-]+))?\\]` +
  `|:(?:hover|first-child|last-child|nth-child\\(\\s*[\\w+\\- ]+\\s*\\)))`;
const COMPOUND = `(?:(?:${IDENT})${SIMPLE}*|${SIMPLE}+)`;
const REMAINDER = new RegExp(`^(?:(?:\\s*>\\s*|\\s+)${COMPOUND})*$`);

const BLOCKED_PROPS =
  /^(?:position|inset(?:-.*)?|top|right|bottom|left|z-index|pointer-events|behavior|-moz-binding|animation(?:-.*)?|transition(?:-.*)?)$/i;
// Extra properties dropped on a rule that targets the diagram root itself
// (no descendant combinator): they would move, resize, clip or blend the
// whole diagram over the surrounding page.
const ROOT_BLOCKED_PROPS =
  /^(?:transform(?:-.*)?|translate|scale|rotate|offset(?:-.*)?|margin(?:-.*)?|float|overflow(?:-.*)?|width|height|min-(?:width|height)|max-height|clip(?:-.*)?|opacity|mix-blend-mode|filter|backdrop-filter|display|contain(?:-.*)?|isolation|will-change|perspective(?:-.*)?)$/i;
const PROP_NAME = /^-?[a-z][a-z0-9-]*$/;
const BAD_VALUE =
  /image-set|image\(|src\(|attr\(|cross-fade|expression|javascript:|vbscript:|var\(|\\|\/\*|[{};<]/i;
const FRAGMENT_URL = /url\(\s*(?:"#[\w.:-]+"|'#[\w.:-]+'|#[\w.:-]+)\s*\)/gi;

/** True when a CSS value is safe: no remote url(), no escapes, no var(). */
export function isSafeCssValue(value: string): boolean {
  if (!value || BAD_VALUE.test(value)) return false;
  return !/u\s*r\s*l\s*\(/i.test(value.replace(FRAGMENT_URL, ''));
}

/** True when an attribute value carries no CSS escape, comment or non-fragment url(). */
export function isSafeAttrValue(value: string): boolean {
  if (value.includes('\\') || value.includes('/*')) return false;
  return !/u\s*r\s*l\s*\(/i.test(value.replace(FRAGMENT_URL, ''));
}

function declarationsOf(style: CSSStyleDeclaration, rootRule: boolean): string {
  const out: string[] = [];
  for (let i = 0; i < style.length; i++) {
    const name = style[i];
    if (!PROP_NAME.test(name) || BLOCKED_PROPS.test(name)) continue;
    if (rootRule && ROOT_BLOCKED_PROPS.test(name)) continue;
    const value = style.getPropertyValue(name).trim();
    if (!isSafeCssValue(value)) continue;
    const important = style.getPropertyPriority(name) === 'important' ? ' !important' : '';
    out.push(`${name}:${value}${important}`);
  }
  return out.join(';');
}

function splitSelectors(text: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let quote = '';
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (quote) {
      if (ch === quote) quote = '';
    } else if (ch === '"' || ch === "'") quote = ch;
    else if (ch === '(' || ch === '[') depth++;
    else if (ch === ')' || ch === ']') depth--;
    else if (ch === ',' && depth === 0) {
      parts.push(text.slice(start, i));
      start = i + 1;
    }
  }
  parts.push(text.slice(start));
  return parts.map((p) => p.trim());
}

/** True when the selector has no descendant combinator, i.e. it styles the root itself. */
function targetsRoot(selector: string, rootId: string): boolean {
  return !/[\s>]/.test(selector.slice(rootId.length + 1));
}

/** True when the selector is `#rootId` followed only by allowed descendants. */
export function isScopedSelector(selector: string, rootId: string): boolean {
  if (!ROOT_ID.test(rootId) || !selector.startsWith(`#${rootId}`)) return false;
  const rest = selector.slice(rootId.length + 1);
  if (rest.length > 0 && /^[\w-]/.test(rest)) return false;
  const rootExtras = rest.match(new RegExp(`^(?:${SIMPLE})*`))?.[0] ?? '';
  return REMAINDER.test(rest.slice(rootExtras.length));
}

/**
 * Parses and rebuilds a <style> text. Returns null when the text cannot be
 * parsed safely (no CSSStyleSheet support) or the root id is unusable.
 */
export function sanitizeStyleSheetText(text: string, rootId: string): string | null {
  if (!ROOT_ID.test(rootId)) return null;
  if (typeof CSSStyleSheet === 'undefined') return null;
  let sheet: CSSStyleSheet;
  try {
    sheet = new CSSStyleSheet();
    if (typeof sheet.replaceSync !== 'function') return null;
    sheet.replaceSync(text);
  } catch {
    return null;
  }
  const rules: string[] = [];
  for (const rule of Array.from(sheet.cssRules)) {
    // 1 = CSSRule.STYLE_RULE; every at-rule has another type.
    if (rule.type !== 1) continue;
    const styleRule = rule as CSSStyleRule;
    if (styleRule.cssRules && styleRule.cssRules.length > 0) continue;
    const selectors = splitSelectors(styleRule.selectorText).filter((sel) =>
      isScopedSelector(sel, rootId)
    );
    if (selectors.length === 0) continue;
    const body = declarationsOf(
      styleRule.style,
      selectors.some((sel) => targetsRoot(sel, rootId))
    );
    if (body) rules.push(`${selectors.join(',')}{${body}}`);
  }
  return rules.join('\n');
}

/** Re-serialises a style attribute through the CSS parser; null drops it. */
export function sanitizeStyleAttr(value: string, rootElement = false): string | null {
  if (typeof document === 'undefined') return null;
  const probe = document.createElement('div');
  if (!probe.style || typeof probe.style.setProperty !== 'function') return null;
  probe.style.cssText = value;
  const out = declarationsOf(probe.style, rootElement);
  return out || null;
}

/** Containment applied to the outermost svg after all filtering. */
// Inline !important beats any author !important rule.
export const ROOT_CONTAINMENT =
  'overflow:hidden !important;contain:paint !important;isolation:isolate !important';
