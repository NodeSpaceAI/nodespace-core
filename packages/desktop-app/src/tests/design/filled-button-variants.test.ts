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
import { AA, contrast, readHsl, themeBlock } from '../helpers/wcag-contrast';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const appCss = fs.readFileSync(path.join(packageRoot, 'src/app.css'), 'utf8');

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
        //
        // The first assertion is the one that bites, and it covers the literal
        // too: `tv()` runs tw-merge, so a literal added after the token silently
        // REPLACES it — `text-destructive-foreground text-white` collapses to
        // `text-white` alone. The second line therefore never fails on its own
        // (whichever color survives, the other is absent by construction). It
        // stays as a statement of the prohibition, not as a second safety net.
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

      it(`registers ${token}-hover in its bare, opaque form`, () => {
        // Asserts the SHAPE of the registration, not an impossibility. Omitting
        // the `<alpha-value>` placeholder does not stop an alpha variant from
        // compiling — Tailwind v3 injects alpha into `hsl(var(--x))` regardless,
        // so `hover:bg-primary-hover/90` still resolves. The placeholder's
        // absence is how a value meant to be used opaquely is written here;
        // what actually forbids an alpha hover is the filled-variant sweep below.
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
