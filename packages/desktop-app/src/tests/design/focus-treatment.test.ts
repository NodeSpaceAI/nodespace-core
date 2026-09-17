/**
 * Guards the painted-only focus rule across the whole frontend.
 *
 * A focus treatment in this codebase may change what a control is PAINTED WITH
 * and nothing else. It may not change what space the control occupies, and it
 * may not draw anything outside or inside its edge. The reason is a concrete
 * complaint rather than a principle: a focus style that grows a control shoves
 * every sibling after it, and tabbing through a settings pane makes the whole
 * form twitch line by line.
 *
 * Three families are forbidden, and they fail for three different reasons —
 * worth stating, because only the first is an actual reflow and a reader who
 * knows that might assume the other two are safe:
 *
 *   - `border-width` / `padding` genuinely reflow. `border: 0` -> `border: 2px`
 *     adds 4px to the box and moves everything after it. This is the one that
 *     actually jerks the layout.
 *   - `outline` with a negative offset does NOT reflow, but paints inside the
 *     element's edge, so the control reads as having suddenly grown a thick
 *     inner border. This is the idiom that was removed app-wide once already.
 *   - `ring-offset-*` does NOT reflow either, but paints a gap plus a ring
 *     around the control, visually enlarging its footprint — indistinguishable
 *     from movement to the person looking at it.
 *
 * All three are banned together so that nobody has to re-derive which kind of
 * "doesn't move" a given property offers.
 *
 * This suite scans source text rather than rendering components, for the same
 * reason `filled-button-variants.test.ts` does: Happy-DOM applies no Tailwind
 * stylesheet, so a rendered `getComputedStyle` reports an empty background for
 * a correct class and a forbidden one alike, and would pass either way.
 */

import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { buttonVariants } from '$lib/components/ui/button/types';
import { AA, contrast, readHsl, themeBlock } from '../helpers/wcag-contrast';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const srcRoot = path.join(packageRoot, 'src');
const appCss = fs.readFileSync(path.join(packageRoot, 'src/app.css'), 'utf8');

const THEMES = [
  { name: 'light', selector: ':root' },
  { name: 'dark', selector: '.dark' },
];

/** WCAG 1.4.11 non-text contrast: a UI state must be this distinguishable. */
const NON_TEXT = 3;

/** Every source file that can carry a class string or a CSS rule. */
function sourceFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      // `tests/` is excluded: this suite's own prose names the forbidden
      // utilities, and a test fixture may legitimately contain one as data.
      return entry.name === 'tests' ? [] : sourceFiles(full);
    }
    return /\.(svelte|ts|css)$/.test(entry.name) ? [full] : [];
  });
}

const FILES = sourceFiles(srcRoot);

/**
 * Tailwind utilities that draw or reserve geometry, in a focus variant.
 *
 * `outline-none` is deliberately absent — it REMOVES the UA outline, which is
 * the whole point, and is the one outline utility that must stay allowed.
 * `focus-visible:outline-none` appears on nearly every control here.
 */
const FORBIDDEN_UTILITY =
  /(?:focus|focus-visible|focus-within)(?::[a-z-]+)*:(?:ring(?:-offset)?(?:-|\b)|border-\d|border-[xytrbl]-|p[xytrbl]?-\d|m[xytrbl]?-\d|outline-(?!none)|scale-|translate-|font-(?:bold|semibold|medium|light))/;

/**
 * The same rule in hand-written CSS: `:focus { ... }` blocks in <style>.
 *
 * `border-color` is absent on purpose — it repaints an existing border without
 * changing its width, so it moves nothing. The shorthand `border:` and
 * `border-width:` ARE caught, since either can change the width.
 *
 * `outline: none` is likewise allowed: suppressing the UA outline is the point,
 * and it paints nothing. Any other outline value is forbidden.
 *
 * The allowed-value check is written as a separate `VALUE_IS_NONE` test rather
 * than an inline `(?!none)` lookahead. An inline one does not work here: with
 * `\s*` before it the engine can match zero spaces, which puts the lookahead on
 * the space rather than on `none`, and `outline: none` slips through as a
 * violation. Splitting the property match from the value match removes the
 * backtracking entirely.
 */
const FORBIDDEN_CSS_PROPERTY =
  /^\s*(border(?:-(?:width|top|right|bottom|left))?|padding(?:-[a-z]+)?|margin(?:-[a-z]+)?|outline(?:-(?:width|offset))?|box-shadow|transform|font-weight|letter-spacing)\s*:\s*([^;]*)/;

/** `outline: none`, `box-shadow: none` and `transform: none` paint nothing. */
const VALUE_IS_NONE = /^none\b/;

/** `ring-offset-background` sets a ring offset's COLOR, reserving nothing on
 * its own — but it only exists to support a ring, so its presence means one is
 * intended. It is swept unconditionally rather than only in a focus variant. */
const RING_OFFSET_COLOR = /\bring-offset-(?!0\b)[a-z]/;

describe('focus treatments are painted-only', () => {
  it('scans a non-trivial number of source files', () => {
    // Guards the guard: a broken path or a too-narrow extension filter would
    // make every assertion below sweep an empty list and pass vacuously.
    expect(FILES.length).toBeGreaterThan(100);
  });

  it('has no focus utility that changes geometry', () => {
    const offenders = FILES.flatMap((file) =>
      fs
        .readFileSync(file, 'utf8')
        .split('\n')
        .flatMap((line, i) => {
          const match = FORBIDDEN_UTILITY.exec(line);
          return match ? [`${path.relative(srcRoot, file)}:${i + 1} -> ${match[0]}`] : [];
        })
    );

    expect(offenders).toEqual([]);
  });

  it('has no ring-offset color left behind', () => {
    const offenders = FILES.flatMap((file) =>
      fs
        .readFileSync(file, 'utf8')
        .split('\n')
        .flatMap((line, i) => {
          const match = RING_OFFSET_COLOR.exec(line);
          return match ? [`${path.relative(srcRoot, file)}:${i + 1} -> ${match[0]}`] : [];
        })
    );

    expect(offenders).toEqual([]);
  });

  it('has no hand-written :focus rule that changes geometry', () => {
    const offenders: string[] = [];

    for (const file of FILES) {
      const lines = fs.readFileSync(file, 'utf8').split('\n');
      let depth = 0;
      let inFocusRule = false;

      for (const [i, line] of lines.entries()) {
        // A selector line mentioning :focus opens a block we care about. Nested
        // braces inside it are counted so the block ends where it really ends.
        if (!inFocusRule && /:focus(-visible|-within)?\b/.test(line) && line.includes('{')) {
          inFocusRule = true;
          depth = 0;
        }

        if (inFocusRule) {
          const declaration = FORBIDDEN_CSS_PROPERTY.exec(line);
          if (declaration && !VALUE_IS_NONE.test(declaration[2].trim())) {
            offenders.push(`${path.relative(srcRoot, file)}:${i + 1} -> ${line.trim()}`);
          }
          depth += (line.match(/\{/g) ?? []).length;
          depth -= (line.match(/\}/g) ?? []).length;
          if (depth <= 0) inFocusRule = false;
        }
      }
    }

    expect(offenders).toEqual([]);
  });

  it('uses focus-visible rather than bare focus for visible treatments', () => {
    // Bare `:focus` fires on mouse clicks too, which leaves the treatment
    // sitting on a control the user just pressed with the pointer — a focus
    // style on the resting UI, which is the thing this design avoids.
    //
    // `focus:outline-none` is exempt: suppressing the UA outline for mouse
    // users as well is intentional and paints nothing.
    const offenders = FILES.flatMap((file) =>
      fs
        .readFileSync(file, 'utf8')
        .split('\n')
        .flatMap((line, i) => {
          const matches = line.match(/\bfocus:[a-z][\w:/[\]-]*/g) ?? [];
          const visible = matches.filter((cls) => cls !== 'focus:outline-none');
          return visible.map((cls) => `${path.relative(srcRoot, file)}:${i + 1} -> ${cls}`);
        })
    );

    // dropdown-menu's `focus:bg-accent` is the documented exception: bits-ui
    // drives those items with roving focus from keyboard navigation, so `:focus`
    // there IS the keyboard highlight and never fires from a resting pointer.
    const unexpected = offenders.filter((o) => !o.startsWith('lib/components/ui/dropdown-menu/'));

    expect(unexpected).toEqual([]);
  });
});

describe('the neutral focus fill is actually visible', () => {
  /**
   * The painted-only rule creates a trap that a geometry ban cannot catch: a
   * focus fill that moves nothing but is also indistinguishable from the page
   * satisfies every rule above and still shows the user nothing.
   *
   * This is not hypothetical. The neutral treatment was first written as
   * `focus-visible:bg-muted`, matching how neutral hovers are done elsewhere.
   * `--muted` is 1.10:1 against `--background` in light mode — a focus state
   * nobody could see. `--accent` is the only non-brand token that clears the
   * non-text floor in both themes, which is why the chrome surface uses it.
   */
  for (const theme of THEMES) {
    it(`separates the focus fill from the page background, ${theme.name} theme`, () => {
      const block = themeBlock(appCss, theme.selector);
      const ratio = contrast(readHsl(block, '--accent'), readHsl(block, '--background'));

      expect(ratio).toBeGreaterThanOrEqual(NON_TEXT);
    });
  }

  it('gives every Button variant a focus fill that is not an alpha wash', () => {
    // The same trap in its second form. `secondary` was first given
    // `focus-visible:bg-secondary/80` to mirror its hover — consistent, and
    // 1.08:1 against the page once composited. Hover can afford a subtle shift
    // because the pointer is already on the control; focus cannot, because it is
    // the only thing telling a keyboard user where they are.
    //
    // Asserting "no alpha" rather than computing each ratio: an alpha focus fill
    // composites against an unknown surface, so its real contrast is not
    // knowable from the token alone. Requiring opacity is the checkable form of
    // the rule, and it is what every visible variant now does.
    const variants = ['default', 'destructive', 'outline', 'secondary', 'ghost', 'link'] as const;

    const alphaFocusFills = variants.flatMap((variant) =>
      buttonVariants({ variant })
        .split(/\s+/)
        .filter((cls) => /^focus-visible:bg-.*\//.test(cls))
        .map((cls) => `${variant} -> ${cls}`)
    );

    expect(alphaFocusFills).toEqual([]);
  });

  it('gives every filled or surface Button variant some focus treatment', () => {
    // `link` is the one variant with no fill: it renders as text, so it takes an
    // underline instead. Every other variant must repaint something, or a
    // keyboard user has no way to tell which control Enter will activate — the
    // exact defect on the delete-confirmation dialog that started this work.
    const withFocusTreatment = (['default', 'destructive', 'outline', 'secondary', 'ghost'] as const)
      .filter((variant) => /focus-visible:bg-/.test(buttonVariants({ variant })));

    expect(withFocusTreatment).toHaveLength(5);
    expect(buttonVariants({ variant: 'link' })).toContain('focus-visible:underline');
  });

  it('keeps the light-mode accent label above its dark-mode counterpart', () => {
    // NOT an AA assertion, deliberately. `--accent-foreground` on `--accent` is
    // 3.40:1 in light mode, below the 4.5:1 text floor — a PRE-EXISTING property
    // of the accent pair, already shipped on seven `hover:bg-accent` sites, that
    // this suite does not own and cannot fix by choosing a different focus fill.
    //
    // Asserting AA here would fail the suite on someone else's defect. What is
    // asserted instead is that the value does not silently DROP further: the
    // figure is pinned, so a token edit that worsens it fails loudly and a token
    // edit that fixes it fails too, prompting this comment to be deleted.
    const light = themeBlock(appCss, ':root');
    const ratio = contrast(readHsl(light, '--accent-foreground'), readHsl(light, '--accent'));

    expect(ratio).toBeGreaterThan(3.3);
    expect(ratio).toBeLessThan(3.5);
    expect(ratio).toBeLessThan(AA); // the known gap, stated rather than implied
  });
});
