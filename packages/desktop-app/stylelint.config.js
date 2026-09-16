/**
 * Stylelint configuration — design-token enforcement for CSS in Svelte components.
 *
 * ESLint (eslint.config.js) covers JS/TS and Svelte markup, but
 * eslint-plugin-svelte does not parse CSS inside `<style>` blocks — that region
 * is opaque to it. Nearly all design-system drift (hardcoded colors, ad-hoc
 * shadows, unauthorized motion) lives there. This config is the CSS-side gate;
 * postcss-html gives Stylelint a parser for embedded `<style>` blocks.
 *
 * The governing spec is DESIGN.md in the nodespace-docs repo. The rules encode
 * three of its hard constraints:
 *
 *   1. Colors come from CSS custom properties, never raw literals in components.
 *      Raw values belong in the token definitions (app.css) and nowhere else.
 *   2. Box shadows are reserved for floating surfaces (popovers, dropdowns) and
 *      use one specific value. Modal panels get no shadow — the backdrop
 *      provides the separation.
 *   3. Motion is off globally except two approved cases: the sidebar width
 *      collapse and the code-block hover-button opacity fade.
 *
 * Scope note: this config deliberately enables only the rules that enforce
 * those three constraints, rather than extending a preset such as
 * stylelint-config-standard. Prettier already owns formatting, and mixing
 * hundreds of cosmetic findings (hex casing, notation preferences, empty-line
 * conventions) into this gate would bury the design-system violations it
 * exists to surface.
 */

/**
 * The CSS named colors. `white` and `black` are as much a hardcoded palette
 * decision as `#ffffff` is, and they read as innocuous, so they are the form
 * drift is most likely to take once the obvious literals are blocked.
 *
 * The full list rather than a curated subset: a partial blacklist invites the
 * question "why is `silver` allowed?", and the answer would only ever be "we
 * forgot". `transparent` and `currentColor` are deliberately absent — they
 * carry no palette decision.
 */
const NAMED_COLORS = [
  'aliceblue', 'antiquewhite', 'aqua', 'aquamarine', 'azure', 'beige', 'bisque',
  'black', 'blanchedalmond', 'blue', 'blueviolet', 'brown', 'burlywood',
  'cadetblue', 'chartreuse', 'chocolate', 'coral', 'cornflowerblue', 'cornsilk',
  'crimson', 'cyan', 'darkblue', 'darkcyan', 'darkgoldenrod', 'darkgray',
  'darkgreen', 'darkgrey', 'darkkhaki', 'darkmagenta', 'darkolivegreen',
  'darkorange', 'darkorchid', 'darkred', 'darksalmon', 'darkseagreen',
  'darkslateblue', 'darkslategray', 'darkslategrey', 'darkturquoise',
  'darkviolet', 'deeppink', 'deepskyblue', 'dimgray', 'dimgrey', 'dodgerblue',
  'firebrick', 'floralwhite', 'forestgreen', 'fuchsia', 'gainsboro',
  'ghostwhite', 'gold', 'goldenrod', 'gray', 'green', 'greenyellow', 'grey',
  'honeydew', 'hotpink', 'indianred', 'indigo', 'ivory', 'khaki', 'lavender',
  'lavenderblush', 'lawngreen', 'lemonchiffon', 'lightblue', 'lightcoral',
  'lightcyan', 'lightgoldenrodyellow', 'lightgray', 'lightgreen', 'lightgrey',
  'lightpink', 'lightsalmon', 'lightseagreen', 'lightskyblue', 'lightslategray',
  'lightslategrey', 'lightsteelblue', 'lightyellow', 'lime', 'limegreen',
  'linen', 'magenta', 'maroon', 'mediumaquamarine', 'mediumblue',
  'mediumorchid', 'mediumpurple', 'mediumseagreen', 'mediumslateblue',
  'mediumspringgreen', 'mediumturquoise', 'mediumvioletred', 'midnightblue',
  'mintcream', 'mistyrose', 'moccasin', 'navajowhite', 'navy', 'oldlace',
  'olive', 'olivedrab', 'orange', 'orangered', 'orchid', 'palegoldenrod',
  'palegreen', 'paleturquoise', 'palevioletred', 'papayawhip', 'peachpuff',
  'peru', 'pink', 'plum', 'powderblue', 'purple', 'rebeccapurple', 'red',
  'rosybrown', 'royalblue', 'saddlebrown', 'salmon', 'sandybrown', 'seagreen',
  'seashell', 'sienna', 'silver', 'skyblue', 'slateblue', 'slategray',
  'slategrey', 'snow', 'springgreen', 'steelblue', 'tan', 'teal', 'thistle',
  'tomato', 'turquoise', 'violet', 'wheat', 'white', 'whitesmoke', 'yellow',
  'yellowgreen'
];

/**
 * Matches a raw color literal: hex, a named CSS color, or an
 * rgb/rgba/hsl/hsla/oklch/lab function whose arguments are literal numbers
 * rather than a token reference.
 *
 * This codebase consumes tokens as `hsl(var(--token))` — a color function
 * wrapping a custom property — so the test cannot simply ban color functions.
 * Instead the function alternative requires a digit (or `.`) where a token
 * reference would otherwise be, which `var(--x)` never satisfies.
 */
const RAW_COLOR_SOURCE = [
  String.raw`#[0-9a-fA-F]{3,8}\b`,
  String.raw`\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(\s*[\d.]`,
  // A named color must stand alone as a value. `(?<![\w-])`/`(?![\w-])` rather
  // than `\b` so that a token name merely *containing* one — `var(--white-
  // overlay)`, `--surface-red` — is not mistaken for the literal.
  String.raw`(?<![\w-])(?:${NAMED_COLORS.join('|')})(?![\w-])`
].join('|');

// Stylelint reads a "/…/"-delimited string as a regular expression.
const RAW_COLOR = `/${RAW_COLOR_SOURCE}/`;

/**
 * Properties where a raw color literal could stand in for a token.
 */
const COLOR_PROPERTIES = [
  'color',
  'background',
  'background-color',
  'background-image',
  'border',
  'border-color',
  'border-top',
  'border-right',
  'border-bottom',
  'border-left',
  'border-top-color',
  'border-right-color',
  'border-bottom-color',
  'border-left-color',
  'outline',
  'outline-color',
  'fill',
  'stroke',
  'caret-color',
  'text-decoration-color',
  'box-shadow'
];

/**
 * The single elevation shadow DESIGN.md permits, for floating surfaces only
 * (popovers, dropdowns). Modal panels get `none` — the backdrop separates them.
 */
const FLOATING_SURFACE_SHADOW = '0 4px 6px -1px rgb(0 0 0 / 0.1), 0 2px 4px -2px rgb(0 0 0 / 0.1)';

/**
 * Turns a literal CSS value into a regex source matching it whitespace-
 * insensitively. Long values are routinely wrapped across lines by the
 * formatter, and a line break must not change whether a value is compliant.
 */
function asFlexibleWhitespacePattern(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, String.raw`\$&`).replace(/\s+/g, String.raw`\s+`);
}

const APPROVED_SHADOW_PATTERN = asFlexibleWhitespacePattern(FLOATING_SURFACE_SHADOW);

const colorLiteralRules = Object.fromEntries(
  COLOR_PROPERTIES.map((property) => [property, [RAW_COLOR]])
);

// Component-local custom properties, e.g. `--brand: #ff00aa`. Without this,
// declaring a raw color as a custom property and then reading it back through
// `var(--brand)` launders the literal past every rule above — and reads as
// more idiomatic than the thing being blocked. Raw values belong in the token
// file, which the src/app.css override exempts.
colorLiteralRules['/^--/'] = [RAW_COLOR];

// The approved elevation shadow is specified in DESIGN.md as a literal rgb()
// value, so the raw-color rule would otherwise reject the very value the spec
// mandates. Flag any shadow that is not that value but does carry a literal.
colorLiteralRules['box-shadow'] = [`/^(?!${APPROVED_SHADOW_PATTERN}$).*(?:${RAW_COLOR_SOURCE})/`];

/**
 * Allowed `box-shadow` values.
 *
 * DESIGN.md's shadow rule is about *elevation* — the drop shadows that lift a
 * surface off the content plane. Focus rings and inset hairlines are drawn with
 * box-shadow too, but they are not elevation and the spec does not restrict
 * them; what matters there is that they use a token rather than a literal,
 * which the raw-color rule already enforces on this same property.
 *
 * So: permit the one approved elevation value, `none`, and any shadow built
 * entirely from tokens. A shadow carrying a literal color is still caught.
 */
const ALLOWED_BOX_SHADOWS = [
  new RegExp(`^${APPROVED_SHADOW_PATTERN}$`),
  'none',
  // Token-based shadows (focus rings, inset hairlines): must contain var()
  // and no literal color, which RAW_COLOR independently verifies.
  /var\(--/
];

/**
 * The two transitions DESIGN.md approves app-wide. Everything else is drift:
 * app.css disables transitions globally, so a component-level transition is
 * both unauthorized and (outside these two selectors) inert at runtime.
 */
const APPROVED_TRANSITION_SEGMENTS = [
  // Sidebar width collapse — the one intentional layout motion.
  String.raw`width 0\.25s ease-out`,
  // Code-block hover controls fading in.
  String.raw`opacity 0\.2s ease`
];

// `transition` takes a comma-separated list, so match per segment: an element
// legitimately needing both approved transitions would otherwise be rejected
// for combining two individually-approved values.
const APPROVED_TRANSITIONS = [
  new RegExp(
    `^(?:${APPROVED_TRANSITION_SEGMENTS.join('|')})` +
      `(?:\\s*,\\s*(?:${APPROVED_TRANSITION_SEGMENTS.join('|')}))*$`
  ),
  // Explicitly turning motion off is always allowed.
  /^none$/
];

/**
 * Motion bans. Kept separate from the color entries so the app.css override can
 * drop the color rules without also disabling these.
 */
const MOTION_DISALLOWED = {
  // Neither approved case uses a keyframe animation.
  animation: [/.*/],
  'animation-name': [/^(?!none$).*/]
};

const disallowedListMessage = (property, value) =>
  /^animation/.test(property)
    ? `Unauthorized animation on "${property}": "${value}". DESIGN.md permits only the ` +
      `sidebar width collapse and the code-block button fade.`
    : `Raw color literal "${value}" in "${property}". Use a design token — ` +
      `hsl(var(--token)) — and define raw values in src/app.css.`;

export default {
  // NOTE: customSyntax is applied per-extension in `overrides` below, not here.
  // postcss-html looks for embedded `<style>` blocks, so applying it globally
  // makes plain .css files parse as empty and silently skips every rule on
  // them — including src/lib/styles/noderef.css.

  ignoreFiles: [
    '**/node_modules/**',
    '**/build/**',
    '**/dist/**',
    '**/.svelte-kit/**',
    'src-tauri/**'
  ],

  // CLAUDE.md: "NO lint suppression — fix issues properly."
  //
  // The report* options below are not sufficient on their own: they flag a
  // *descriptionless* disable, so `/* stylelint-disable -- reason */` would
  // still silently switch the gate off. Renaming the configuration comment
  // means `stylelint-disable` is not a recognized directive at all — it reads
  // as an ordinary comment and suppresses nothing. The report* options then
  // catch anyone who finds the real keyword below.
  configurationComment: 'stylelint-design-token-override',
  reportDescriptionlessDisables: true,
  reportNeedlessDisables: true,
  reportInvalidScopeDisables: true,

  rules: {
    // ---- Colors: tokens only -------------------------------------------
    // `hsl(var(--token))` passes; `hsl(174 67% 35%)` and `#1d9387` do not.
    'declaration-property-value-disallowed-list': [
      { ...colorLiteralRules, ...MOTION_DISALLOWED },
      { message: disallowedListMessage }
    ],

    // ---- Elevation and motion: explicit allowlists ----------------------
    'declaration-property-value-allowed-list': [
      {
        'box-shadow': ALLOWED_BOX_SHADOWS,
        transition: APPROVED_TRANSITIONS,
        'transition-property': [/^(?:none|width|opacity)$/]
      },
      {
        message: (property, value) =>
          property === 'box-shadow'
            ? `Disallowed shadow "${value}". DESIGN.md allows a shadow only on floating ` +
              `surfaces, with the value "${FLOATING_SURFACE_SHADOW}"; modal panels use none.`
            : `Unauthorized transition on "${property}": "${value}". DESIGN.md permits only ` +
              `"width 0.25s ease-out" (sidebar) and "opacity 0.2s ease" (code-block buttons).`
      }
    ],

    // @keyframes has no approved use — neither permitted transition needs one.
    // The prefixed spellings are listed too: this app ships on WebKit (Tauri),
    // so `-webkit-keyframes` is the plausible form here, not an exotic case.
    'at-rule-disallowed-list': [
      'keyframes',
      '-webkit-keyframes',
      '-moz-keyframes',
      '-o-keyframes',
      '-ms-keyframes'
    ],

    // Tailwind and Svelte directives that would otherwise read as unknown.
    'at-rule-no-unknown': [
      true,
      { ignoreAtRules: ['tailwind', 'apply', 'layer', 'screen', 'variants', 'responsive'] }
    ],
    'selector-pseudo-class-no-unknown': [true, { ignorePseudoClasses: ['global'] }]
  },

  overrides: [
    {
      // Only Svelte files need the embedded-<style> parser. Plain .css files
      // use Stylelint's default CSS parser.
      files: ['**/*.svelte'],
      customSyntax: 'postcss-html'
    },
    {
      // Token definitions are the one place raw color values belong — that is
      // what a token *is*. The motion and shadow rules still apply here, since
      // app.css is also where the two approved transitions are declared.
      files: ['src/app.css'],
      rules: {
        // Drop only the color entries — the motion bans still apply, so this
        // exemption cannot widen into a blanket disable for the token file.
        'declaration-property-value-disallowed-list': [
          MOTION_DISALLOWED,
          { message: disallowedListMessage }
        ]
      }
    }
  ]
};
