/**
 * Dark-mode tokens that carry their own alpha (`--border`, `--input`) must not
 * be wrapped in Tailwind's stock `hsl(var(--x) / <alpha-value>)` form.
 *
 * That form expands to `hsl(0 0% 100% / 0.15 / 1)`, which is invalid CSS. The
 * declaration is dropped and `border-input` falls back to `currentColor`, so
 * every dark-mode input border rendered near-white while `dark:bg-input/30`
 * fills silently disappeared. Happy-DOM applies no stylesheet, so this reads
 * the config and the token file rather than a computed style.
 */

import { describe, it, expect } from 'vitest';
import tailwindConfig from '../../../tailwind.config.js';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const appCss = fs.readFileSync(path.join(packageRoot, 'src/app.css'), 'utf8');

type ColorEntry = string | ((_opts: { opacityValue?: string }) => string) | Record<string, unknown>;
const colors = tailwindConfig.theme?.extend?.colors as unknown as Record<string, ColorEntry>;

/** Every `--token` whose declared value embeds an alpha (`H S% L% / A`). */
const alphaTokens = [...appCss.matchAll(/^\s*--([a-z-]+):\s*[^;/]*\/\s*[\d.]+\s*;/gm)].map(
  (m) => m[1]
);

describe('alpha-carrying tokens in Tailwind colors', () => {
  it('finds the dark-mode border and input tokens', () => {
    expect(alphaTokens).toEqual(expect.arrayContaining(['border', 'input']));
  });

  it.each(['border', 'input'])('%s is not the stock <alpha-value> string', (name) => {
    const entry = colors[name];
    expect(typeof entry).toBe('function');
    const fn = entry as (_opts: { opacityValue?: string }) => string;
    expect(fn({})).toBe(`hsl(var(--${name}))`);
    const scaled = fn({ opacityValue: '0.3' });
    expect(scaled).toContain(`hsl(var(--${name}))`);
    expect(scaled).toContain('color-mix');
    expect(scaled).not.toContain('<alpha-value>');
  });

  it('no Tailwind color wraps an alpha-carrying token in <alpha-value>', () => {
    const offenders: string[] = [];
    const walk = (prefix: string, entry: unknown) => {
      if (typeof entry === 'string') {
        for (const token of alphaTokens) {
          if (entry.includes(`var(--${token})`) && entry.includes('<alpha-value>')) {
            offenders.push(`${prefix}: ${entry}`);
          }
        }
      } else if (entry && typeof entry === 'object') {
        for (const [k, v] of Object.entries(entry)) walk(`${prefix}.${k}`, v);
      }
    };
    walk('colors', colors);
    expect(offenders).toEqual([]);
  });
});
