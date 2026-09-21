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
 * reason `filled-variants.test.ts` does: Happy-DOM applies no Tailwind
 * stylesheet, so a rendered `getComputedStyle` reports an empty background for
 * a correct class and a forbidden one alike, and would pass either way.
 */

import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { buttonVariants } from '$lib/components/ui/button/types';
import { contrast, readHsl, themeBlock } from '../helpers/wcag-contrast';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const srcRoot = path.join(packageRoot, 'src');
const appCss = fs.readFileSync(path.join(packageRoot, 'src/app.css'), 'utf8');

const THEMES = [
  { name: 'light', selector: ':root' },
  { name: 'dark', selector: '.dark' },
];

/** WCAG 1.4.11 non-text contrast: a UI state must be this distinguishable. */
const NON_TEXT = 3;

/**
 * Every source file that can carry a class string or a CSS rule.
 *
 * Only `src/tests` is excluded, matched by full path rather than by directory
 * name: a bare `name === 'tests'` check would also skip any `tests/` nested
 * inside `lib/`, silently dropping real components from every sweep. The
 * exclusion exists because this suite's own prose names the forbidden utilities
 * and a fixture may legitimately contain one as data.
 */
function sourceFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      return full === path.join(srcRoot, 'tests') ? [] : sourceFiles(full);
    }
    return /\.(svelte|ts|css)$/.test(entry.name) ? [full] : [];
  });
}

/** Read once: four sweeps over ~1000 files is four times the disk work. */
const FILES = sourceFiles(srcRoot).map((file) => ({
  path: path.relative(srcRoot, file),
  text: fs.readFileSync(file, 'utf8'),
}));

/**
 * Tailwind utilities that draw or reserve geometry, in a focus variant.
 *
 * `outline-none` is deliberately absent — it REMOVES the UA outline, which is
 * the whole point, and is the one outline utility that must stay allowed.
 * `focus-visible:outline-none` appears on nearly every control here.
 *
 * Three shapes are easy to leave out and each is a live hole, so they are
 * spelled out rather than left to a `-\d` suffix:
 *
 *  - BARE utilities. `focus-visible:border` is `border-width: 1px`, and it is
 *    the most natural way to write the exact regression this file prevents.
 *    Same for a bare `outline` and a bare `ring`. They are matched by allowing
 *    the utility to end at a word boundary.
 *  - ARBITRARY values. `border-[3px]`, `p-[4px]` — the `[` form bypasses any
 *    pattern that only expects a digit or a named scale step.
 *  - ALIASES that do not name their property. Tailwind's `shadow-*` IS
 *    `box-shadow` and `tracking-*` IS `letter-spacing`; both are banned in the
 *    hand-written-CSS sweep below, so leaving them out here would make the two
 *    halves of the same rule disagree.
 */
const GEOMETRY_UTILITY =
  // rings, including bare `ring` and every ring-offset form
  'ring(?![\\w-])|ring-|' +
  // border WIDTH: bare, numeric, arbitrary, or per-side. `border-<color>` is
  // allowed and must not match, so named colors are excluded by requiring a
  // digit, a bracket, or a side prefix.
  'border(?![\\w-])|border-\\d|border-\\[|border-[xytrbl]-(?:\\d|\\[)|' +
  // padding and margin, numeric or arbitrary
  'p[xytrbl]?-(?:\\d|\\[)|m[xytrbl]?-(?:\\d|\\[)|' +
  // any outline except the suppression
  'outline(?![\\w-])|outline-(?!none)|' +
  // box-shadow under its Tailwind alias, bare or scaled
  'shadow(?![\\w-])|shadow-(?:sm|md|lg|xl|2xl|inner|\\[)|' +
  // transforms and text metrics
  'scale-|translate-|rotate-|skew-|' +
  'font-(?:thin|extralight|light|normal|medium|semibold|bold|extrabold|black)|' +
  'tracking-';

/** The same list under a focus variant, which is how markup spells it. */
const FORBIDDEN_UTILITY = new RegExp(
  `(?:focus|focus-visible|focus-within)(?::[a-z-]+)*:(?:${GEOMETRY_UTILITY})`
);

/**
 * The same list BARE, which is how `@apply` spells it inside a `:focus` block —
 * there the focus state lives in the selector, so the utility carries no prefix.
 * Built from the one list above so the two cannot drift apart.
 */
const FORBIDDEN_APPLIED_UTILITY = new RegExp(`(?:^|\\s)(?:${GEOMETRY_UTILITY})`);

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
const FORBIDDEN_CSS_PROPERTY = new RegExp(
  '^\\s*(' +
    // `border`, `border-width`, `border-top`, and the logical forms
    // (`border-block-width`, `border-inline-start-width`, …). `border-color`
    // and `border-radius` must NOT match, so the branch either ends at the
    // property name or continues into a side/axis followed by `-width`.
    'border(?:-(?:width|top|right|bottom|left|block|inline)(?:-(?:start|end))?(?:-width)?)?|' +
    'padding(?:-(?:top|right|bottom|left|block|inline)(?:-(?:start|end))?)?|' +
    'margin(?:-(?:top|right|bottom|left|block|inline)(?:-(?:start|end))?)?|' +
    'outline(?:-(?:width|offset))?|' +
    'box-shadow|transform|font-weight|letter-spacing' +
    ')\\s*:\\s*([^;]*)'
);

/** `outline: none`, `box-shadow: none` and `transform: none` paint nothing. */
const VALUE_IS_NONE = /^none\b/;

/** `ring-offset-background` sets a ring offset's COLOR, reserving nothing on
 * its own — but it only exists to support a ring, so its presence means one is
 * intended. It is swept unconditionally rather than only in a focus variant. */
const RING_OFFSET_COLOR = /\bring-offset-(?!0\b)[a-z]/;

/**
 * Every `:focus` / `:focus-visible` / `:focus-within` rule body in a stylesheet,
 * with its declarations flattened onto one string.
 *
 * Works on the whole source rather than line by line. A line-oriented scanner
 * needs a special case for each way a rule can be laid out — `{` on the next
 * line, a single-line rule, several declarations sharing a line, a comma-
 * separated selector list broken across lines — and every missing case is a
 * silent hole rather than a failure. Notably the single-line form
 * (`.x:focus { border-width: 2px; }`) is what a formatter produces from a short
 * rule, and it is the collapsed shape of the three defects this change fixed.
 *
 * Comments are stripped first so a commented-out declaration cannot trip it,
 * then each `{...}` body is taken with brace depth tracked across the file so a
 * nested rule inside a focus block is still inside it.
 */
function focusRules(source: string): { selector: string; body: string }[] {
  const css = source.replace(/\/\*[\s\S]*?\*\//g, ' ');
  const rules: { selector: string; body: string }[] = [];

  for (let i = 0; i < css.length; i++) {
    if (css[i] !== '{') continue;

    // The selector is whatever precedes this brace back to the previous
    // delimiter, whitespace collapsed so a multi-line list reads as one.
    const selectorStart = Math.max(
      css.lastIndexOf('}', i),
      css.lastIndexOf('{', i - 1),
      css.lastIndexOf(';', i)
    );
    const selector = css.slice(selectorStart + 1, i).replace(/\s+/g, ' ').trim();

    let depth = 0;
    let end = i;
    for (let j = i; j < css.length; j++) {
      if (css[j] === '{') depth++;
      else if (css[j] === '}' && --depth === 0) {
        end = j;
        break;
      }
    }

    if (/:focus(-visible|-within)?(?![\w-])/.test(selector)) {
      // Only this rule's own declarations: a nested block's contents belong to
      // the nested selector, which this loop reaches on its own iteration.
      //
      // The strip takes each nested block TOGETHER WITH its selector text
      // (`[^;{}]*` before the braces). Removing only `{...}` leaves the
      // selector behind as an orphan, and since declarations are split on `;`
      // that orphan glues onto the front of the next one — which defeats the
      // `^\s*` anchor in FORBIDDEN_CSS_PROPERTY and silently passes it:
      //
      //   .a:focus-visible { .x { color: red } padding: 4px }
      //                                        ^ missed, because the chunk
      //                                          reads ".x  padding: 4px"
      //
      // The bug was order-dependent, which is what made it worth a comment:
      // the same declaration placed BEFORE the nested block was caught.
      rules.push({ selector, body: css.slice(i + 1, end).replace(/[^;{}]*\{[^{}]*\}/g, ' ') });
    }
  }

  return rules;
}

describe('focus treatments are painted-only', () => {
  it('scans a non-trivial number of source files', () => {
    // Guards the guard: a broken path or a too-narrow extension filter would
    // make every assertion below sweep an empty list and pass vacuously.
    expect(FILES.length).toBeGreaterThan(100);
  });

  it('has no focus utility that changes geometry', () => {
    const offenders = FILES.flatMap(({ path: file, text }) =>
      text.split('\n').flatMap((line, i) => {
        const match = FORBIDDEN_UTILITY.exec(line);
        return match ? [`${file}:${i + 1} -> ${match[0]}`] : [];
      })
    );

    expect(offenders).toEqual([]);
  });

  it('has no ring-offset color left behind', () => {
    const offenders = FILES.flatMap(({ path: file, text }) =>
      text.split('\n').flatMap((line, i) => {
        const match = RING_OFFSET_COLOR.exec(line);
        return match ? [`${file}:${i + 1} -> ${match[0]}`] : [];
      })
    );

    expect(offenders).toEqual([]);
  });

  it('has no hand-written :focus rule that changes geometry', () => {
    const offenders: string[] = [];

    for (const { path: file, text } of FILES) {
      for (const { selector, body } of focusRules(text)) {
        for (const declaration of body.split(';')) {
          const trimmed = declaration.trim();

          const match = FORBIDDEN_CSS_PROPERTY.exec(trimmed);
          if (match && !VALUE_IS_NONE.test(match[2].trim())) {
            offenders.push(`${file} -> ${selector} { ${trimmed} }`);
          }

          // `@apply` pulls a Tailwind utility into hand-written CSS, so it
          // slips between the two sweeps: the utility sweep looks for a
          // `focus-visible:` prefix that is not there (the `:focus` is in the
          // selector instead), and the property sweep looks for
          // `property: value`, which `@apply ring-2` is not. Nothing in the
          // codebase uses `@apply` today; this closes the seam rather than
          // waiting for the first one to land in a focus block.
          const applied = /^@apply\s+(.+)/.exec(trimmed);
          if (applied && FORBIDDEN_APPLIED_UTILITY.test(applied[1])) {
            offenders.push(`${file} -> ${selector} { ${trimmed} }`);
          }
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
    const offenders = FILES.flatMap(({ path: file, text }) =>
      text.split('\n').flatMap((line, i) => {
        const matches = line.match(/\bfocus:[a-z][\w:/[\]-]*/g) ?? [];
        const visible = matches.filter((cls) => cls !== 'focus:outline-none');
        return visible.map((cls) => `${file}:${i + 1} -> ${cls}`);
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

  for (const theme of THEMES) {
    it(`states what a filled variant's focus shift actually measures, ${theme.name} theme`, () => {
      // Recorded rather than asserted against 3:1, because the filled variants
      // do NOT clear it and the honest thing is to say so in the place someone
      // will look. `--primary-hover` against `--primary` is 1.38:1 light /
      // 1.30:1 dark; `--destructive-hover` against `--destructive` is 1.24:1 /
      // 1.21:1.
      //
      // That is defensible where the `bg-muted` and `secondary/80` traps were
      // not, and the difference is worth being precise about. A neutral control
      // focusing to `muted` had to be told apart from the PAGE, an unbounded
      // surface with nothing marking where the control ends. A filled button is
      // already a saturated shape against that page, so the shift only has to
      // be told apart from the same button a moment earlier — and it is the
      // identical shift the button makes on hover, which ships and reads fine.
      //
      // What this pins is the direction and the magnitude. A token edit that
      // flattens the shift toward zero fails here, which is the regression that
      // would actually matter.
      const block = themeBlock(appCss, theme.selector);

      for (const token of ['--primary', '--destructive']) {
        const shift = contrast(readHsl(block, `${token}-hover`), readHsl(block, token));
        expect(shift).toBeGreaterThan(1.15);
        expect(shift).toBeLessThan(1.6);
      }
    });
  }
});
