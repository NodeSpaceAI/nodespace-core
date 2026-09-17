/**
 * WCAG contrast math and app.css token reading, shared by the design-token
 * suites that assert arithmetic relationships between color tokens.
 *
 * These live here rather than in either suite because two files need them and
 * the `readHsl` lesson below is worth having in exactly one place: when it was
 * duplicated, the comment recording it survived in only one copy.
 */

import { expect } from 'vitest';

export type Hsl = { h: number; s: number; l: number };

/**
 * Extracts one theme block's body from app.css.
 *
 * The `:root` (light) and `.dark` blocks must be read separately, because every
 * semantic token is declared in both and a whole-file scan would silently
 * compare a light fill against a dark foreground.
 *
 * Brace-counting rather than a lazy `[^}]*` match: both blocks contain nested
 * rules, so stopping at the first `}` would truncate the light block partway
 * through and lose the state colors declared after it.
 */
export function themeBlock(source: string, selector: string): string {
  const start = source.indexOf(selector);
  expect(start, `${selector} block not found in app.css`).toBeGreaterThan(-1);

  const open = source.indexOf('{', start);
  let depth = 0;
  for (let i = open; i < source.length; i++) {
    if (source[i] === '{') depth++;
    else if (source[i] === '}' && --depth === 0) return source.slice(open + 1, i);
  }
  throw new Error(`Unbalanced braces after ${selector} in app.css`);
}

/**
 * `--token: 345 77% 46%` -> {h,s,l}.
 *
 * Throws rather than returning null on a miss. A token that cannot be read is a
 * broken test, not a passing one — an earlier draft returned null and let the
 * callers skip, which turned a typo in this regex into 16 vacuous passes.
 *
 * `(?![\w-])` stops `--primary` from matching `--primary-hover`, which shares
 * its prefix and is declared three lines away.
 */
export function readHsl(block: string, token: string): Hsl {
  const pattern = new RegExp(`${token}(?![\\w-])\\s*:\\s*([\\d.]+)\\s+([\\d.]+)%\\s+([\\d.]+)%`);
  const match = pattern.exec(block);
  if (!match) throw new Error(`${token} is not declared as a plain HSL triple in this theme block`);
  return { h: Number(match[1]), s: Number(match[2]), l: Number(match[3]) };
}

/**
 * Returns 0-1 channels, as `luminance` expects.
 *
 * Exported alongside `contrast` rather than kept private even though both
 * current callers only want the ratio: these are the two WCAG primitives the
 * ratio is built from, and an assertion about a single color — "is this surface
 * light or dark", a non-ratio threshold — needs the luminance without the
 * comparison. Keeping them named and documented here is the point of the
 * module; hiding them would only push the next such test back to a local copy,
 * which is what this file exists to stop.
 */
export function hslToRgb({ h, s, l }: Hsl): [number, number, number] {
  const sat = s / 100;
  const lig = l / 100;
  const k = (n: number) => (n + h / 30) % 12;
  const a = sat * Math.min(lig, 1 - lig);
  const f = (n: number) => lig - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
  return [f(0), f(8), f(4)];
}

/** WCAG relative luminance. Takes 0-1 channels, as hslToRgb returns. */
export function luminance(rgb: [number, number, number]): number {
  const [r, g, b] = rgb.map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast ratio between two opaque colors, order-independent. */
export function contrast(a: Hsl, b: Hsl): number {
  const [hi, lo] = [luminance(hslToRgb(a)), luminance(hslToRgb(b))].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** WCAG 2.x minimum contrast for normal-size text. */
export const AA = 4.5;
