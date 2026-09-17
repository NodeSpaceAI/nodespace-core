/**
 * Guards that the shared Button's filled variants CONSUME the hover tokens.
 *
 * `filled-button-hover.test.ts` proves the `--*-hover` values in app.css obey
 * the rule. That is a necessary but not sufficient condition, and the gap
 * between the two is exactly how this defect shipped: the tokens were correct
 * and the shared Button simply never used them, hovering to `bg-destructive/90`
 * instead and dropping dark-mode destructive to 3.39:1 — below AA, on a
 * "Remove database" control, at the moment of a destructive commitment.
 *
 * So this suite asserts the other half: that each filled variant names its own
 * `--*-hover` token, that no alpha hover survives anywhere in `buttonVariants`,
 * and that the classes it emits resolve through `tailwind.config.js` to values
 * that clear AA and rise on hover in both themes.
 *
 * Reading the variant strings rather than rendering the component is deliberate.
 * The bug is in which classes are emitted, and Happy-DOM applies no Tailwind
 * stylesheet, so a rendered `getComputedStyle` would report an empty background
 * for both the correct and the broken class and pass either way.
 */

import { describe, it, expect } from 'vitest';
import { buttonVariants } from '$lib/components/ui/button/types';
import tailwindConfig from '../../../tailwind.config.js';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const appCss = fs.readFileSync(path.join(packageRoot, 'src/app.css'), 'utf8');

/** WCAG 2.x minimum for normal-size text. */
const AA = 4.5;

type Hsl = { h: number; s: number; l: number };

function themeBlock(source: string, selector: string): string {
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

/** `--token: 345 77% 46%` -> {h,s,l}. Throws on a miss, so a typo fails loudly. */
function readHsl(block: string, token: string): Hsl {
  const pattern = new RegExp(`${token}(?![\\w-])\\s*:\\s*([\\d.]+)\\s+([\\d.]+)%\\s+([\\d.]+)%`);
  const match = pattern.exec(block);
  if (!match) throw new Error(`${token} is not declared as a plain HSL triple in this theme block`);
  return { h: Number(match[1]), s: Number(match[2]), l: Number(match[3]) };
}

function hslToRgb({ h, s, l }: Hsl): [number, number, number] {
  const sat = s / 100;
  const lig = l / 100;
  const k = (n: number) => (n + h / 30) % 12;
  const a = sat * Math.min(lig, 1 - lig);
  const f = (n: number) => lig - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
  return [f(0), f(8), f(4)];
}

/** WCAG relative luminance. Takes 0-1 channels, as hslToRgb returns. */
function luminance(rgb: [number, number, number]): number {
  const [r, g, b] = rgb.map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: Hsl, b: Hsl): number {
  const [hi, lo] = [luminance(hslToRgb(a)), luminance(hslToRgb(b))].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/**
 * The filled variants: solid semantic fill, own foreground, own hover token.
 * `outline`, `ghost` and `link` are not filled — they hover to `accent` — and
 * `secondary` is a neutral surface, so none of them are bound by this rule.
 */
const FILLED_VARIANTS = [
  { variant: 'default' as const, token: '--primary', utility: 'primary' },
  { variant: 'destructive' as const, token: '--destructive', utility: 'destructive' },
];

const THEMES = [
  { name: 'light', selector: ':root' },
  { name: 'dark', selector: '.dark' },
];

/** The class string tailwind-variants emits for a variant, at default size. */
const classesFor = (variant: 'default' | 'destructive') =>
  buttonVariants({ variant }).split(/\s+/).filter(Boolean);

describe('shared Button filled variants', () => {
  for (const { variant, token, utility } of FILLED_VARIANTS) {
    describe(`${variant} variant`, () => {
      const classes = classesFor(variant);

      it(`fills with bg-${utility} and hovers to bg-${utility}-hover`, () => {
        expect(classes).toContain(`bg-${utility}`);
        expect(classes).toContain(`hover:bg-${utility}-hover`);
      });

      it(`labels with text-${utility}-foreground rather than a hardcoded color`, () => {
        // `text-white` was the stock shadcn treatment on destructive. It bypasses
        // `--destructive-foreground`, which carries real per-theme values, and
        // lands at 2.86:1 on the solid dark fill.
        expect(classes).toContain(`text-${utility}-foreground`);
        expect(classes).not.toContain('text-white');
      });

      it(`resolves bg-${utility}-hover through the Tailwind config to the token`, () => {
        // Without this registration the class is simply dropped at build time and
        // the button would keep its rest fill on hover — a silent no-op that the
        // class-name assertion above cannot see.
        const colors = tailwindConfig.theme?.extend?.colors as Record<
          string,
          Record<string, string>
        >;
        expect(colors[utility].hover).toBe(`hsl(var(${token}-hover))`);
      });

      it(`registers ${token}-hover with no alpha channel`, () => {
        // An `<alpha-value>` placeholder would make `hover:bg-x-hover/90` legal
        // again, re-admitting the composited idiom these tokens replaced.
        const colors = tailwindConfig.theme?.extend?.colors as Record<
          string,
          Record<string, string>
        >;
        expect(colors[utility].hover).not.toContain('<alpha-value>');
      });

      for (const theme of THEMES) {
        it(`rises above AA and above rest on hover, ${theme.name} theme`, () => {
          // Deliberately asserts nothing about the REST contrast floor. `--primary`
          // is 3.51:1 light / 4.47:1 dark against its own foreground at rest — both
          // under AA, and both pre-existing properties of the brand color that no
          // hover change can reach. Adding a rest-state floor here would make this
          // suite fail on a defect it does not own and cannot fix. What the hover
          // rule guarantees, and what is asserted, is that hovering never makes the
          // label harder to read than it already was.
          const block = themeBlock(appCss, theme.selector);
          const foreground = readHsl(block, `${token}-foreground`);
          const rest = contrast(readHsl(block, token), foreground);
          const hover = contrast(readHsl(block, `${token}-hover`), foreground);

          expect(hover).toBeGreaterThanOrEqual(AA);
          expect(hover).toBeGreaterThan(rest);
        });
      }
    });
  }

  it('has no alpha-fill hover left on a filled variant', () => {
    // The idiom-level assertion: `hover:bg-primary/90` and `hover:bg-destructive/90`
    // were both present before this change, and this is what stops either coming
    // back under a different token name.
    //
    // Scoped to the FILLED variants, which is the scope of the rule itself.
    // `hover:bg-secondary/80` is left alone deliberately: secondary is a neutral
    // surface, not a semantic fill, and it sits at 16.1:1 light / 14.2:1 dark and
    // GAINS contrast on hover. Alpha is also legitimate elsewhere in the base
    // string — `aria-invalid:ring-destructive/20` is a ring tint, and
    // `dark:hover:bg-accent/50` on ghost/outline is a translucent wash over the
    // page rather than a filled button's solid fill.
    const filledHoverClasses = FILLED_VARIANTS.flatMap(({ variant }) => classesFor(variant)).filter(
      (cls) => /^(dark:)?hover:bg-/.test(cls)
    );

    expect(filledHoverClasses.filter((cls) => cls.includes('/'))).toEqual([]);
  });

  it('leaves no translucent rest fill on the destructive variant', () => {
    // `dark:bg-destructive/60` was stock shadcn. It lightened the dark fill
    // toward the page, which was the only thing keeping `text-white` legible
    // there; with the token foreground the fill should be solid in both themes.
    const classes = classesFor('destructive');
    expect(classes.filter((cls) => /^(dark:)?bg-destructive\//.test(cls))).toEqual([]);
  });
});
