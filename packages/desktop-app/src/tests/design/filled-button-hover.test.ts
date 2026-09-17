/**
 * Guards the filled-button hover rule for semantic color tokens.
 *
 * DESIGN.md: a filled semantic button hovers to its own `--*-hover` token,
 * which moves lightness AWAY from the surface — darker in light mode, lighter
 * in dark. The point of that direction is the consequence asserted below:
 * contrast against the button's own foreground RISES on hover, so hovering can
 * never make a label harder to read.
 *
 * Stylelint cannot catch a violation. `--destructive-hover: 345 77% 60%` is a
 * well-formed custom property assigned a plain HSL triple in the one file
 * allowed to hold raw values; every design-token rule passes while the button
 * silently fades toward its background on hover. The rule is arithmetic on the
 * values, not a property-shape constraint, so it needs an executable guard.
 *
 * This is not hypothetical. The idioms this replaced did exactly that: the
 * shared shadcn Button's `hover:bg-destructive/90` drops dark-mode destructive
 * to 3.39:1 — below AA, at the moment of a destructive commitment.
 */

import { describe, it, expect } from 'vitest';
import { AA, contrast, readHsl, themeBlock } from '../helpers/wcag-contrast';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const appCssPath = path.join(packageRoot, 'src/app.css');

/**
 * Every semantic token carrying a `--*-hover`. The set is closed: all four
 * state colors are derived by the rule whether or not they have a filled-button
 * use today, so the first such button applies the law rather than inventing a
 * value. Add a token here when you add its hover, and this suite holds it to
 * the same law.
 */
const FILLED_BUTTON_TOKENS = ['--primary', '--destructive', '--success', '--warning'] as const;

/**
 * Which direction "away from the surface" points in each theme. Light-mode
 * surfaces are near-white so hover darkens; dark-mode surfaces are near-black
 * so it lightens.
 */
const THEMES = [
  { name: 'light', selector: ':root', direction: 'darker' as const },
  { name: 'dark', selector: '.dark', direction: 'lighter' as const },
];

const appCss = fs.readFileSync(appCssPath, 'utf8');

describe('filled semantic button hover tokens', () => {
  for (const theme of THEMES) {
    describe(`${theme.name} theme`, () => {
      const block = themeBlock(appCss, theme.selector);

      for (const token of FILLED_BUTTON_TOKENS) {
        // Read inside each `it` so a missing token fails that test with the
        // thrown message, rather than crashing collection for the whole file.
        const base = () => readHsl(block, token);
        const hover = () => readHsl(block, `${token}-hover`);
        const foreground = () => readHsl(block, `${token}-foreground`);

        it(`declares ${token}, ${token}-hover and ${token}-foreground`, () => {
          expect(() => [base(), hover(), foreground()]).not.toThrow();
        });

        it(`moves ${token}-hover ${theme.direction} than ${token}`, () => {
          // Compared on the raw HSL lightness the rule is stated in, not on
          // luminance: the rule is "6 points of HSL lightness away from the
          // surface", and for a hue/saturation-preserving move the two agree
          // in sign anyway.
          if (theme.direction === 'darker') expect(hover().l).toBeLessThan(base().l);
          else expect(hover().l).toBeGreaterThan(base().l);
        });

        it(`keeps ${token}-hover the same hue and saturation as ${token}`, () => {
          // A hover that shifts hue is a different color, not a hover state —
          // and it would slip past the lightness assertion above.
          const { h, s } = hover();
          expect({ h, s }).toEqual({ h: base().h, s: base().s });
        });

        it(`keeps ${token}-hover legible against ${token}-foreground`, () => {
          expect(contrast(hover(), foreground())).toBeGreaterThanOrEqual(AA);
        });

        it(`does not lose contrast hovering ${token}`, () => {
          // The invariant the direction exists to produce. This is the one that
          // catches a regression the AA floor alone would let through: a hover
          // that stays above 4.5:1 while still being harder to read than rest.
          expect(contrast(hover(), foreground())).toBeGreaterThan(contrast(base(), foreground()));
        });
      }
    });
  }

  it('gives every theme-tracking --*-hover token a value in both themes', () => {
    // A token defined only under :root silently keeps its light value in dark
    // mode, which inverts the hover direction rather than failing loudly.
    //
    // --console-* is excluded because it is deliberately theme-invariant: the
    // diagnostic panel stays a dark console in both themes, so app.css defines
    // that surface once under :root and does NOT redefine it under .dark.
    // Requiring a .dark twin here would push app.css toward exactly the
    // duplication that comment argues against.
    const declared = (selector: string) =>
      [...themeBlock(appCss, selector).matchAll(/(--[a-z0-9-]+-hover)\s*:/gi)]
        .map((m) => m[1])
        .filter((name) => !name.startsWith('--console-'))
        .sort();

    expect(declared('.dark')).toEqual(declared(':root'));
  });
});
