/**
 * Colour for a token whose value may carry its own alpha channel.
 *
 * Dark mode declares `--border` and `--input` as `H S% L% / A`. The stock
 * `hsl(var(--x) / <alpha-value>)` form then expands to
 * `hsl(0 0% 100% / 0.15 / 1)`, which is invalid: the declaration is dropped
 * and `border-color` falls back to `currentColor` (near-white text), and a
 * `bg-input/30` fill vanishes entirely. Mixing the token with transparent
 * keeps the token's own alpha and scales it by the requested opacity, and is
 * identical to the plain form for opaque light-mode tokens.
 *
 * @param {string} token CSS custom property name, e.g. "--border"
 * @returns {any} a Tailwind colour function; typed loosely because the bundled
 *   config typings only admit string colour values
 */
const withTokenAlpha = (token) => (/** @type {{ opacityValue?: string }} */ { opacityValue }) =>
  opacityValue === undefined
    ? `hsl(var(${token}))`
    : `color-mix(in srgb, hsl(var(${token})) calc(${opacityValue} * 100%), transparent)`;

/** @type {import('tailwindcss').Config} */
export default {
  content: ["./src/**/*.{html,js,svelte,ts}"],
  theme: {
    container: {
      center: true,
      padding: "2rem",
      screens: {
        "2xl": "1400px"
      }
    },
    extend: {
      colors: {
        border: withTokenAlpha("--border"),
        input: withTokenAlpha("--input"),
        "switch-track": "hsl(var(--switch-track) / <alpha-value>)",
        ring: "hsl(var(--ring) / <alpha-value>)",
        background: "hsl(var(--background) / <alpha-value>)",
        foreground: "hsl(var(--foreground) / <alpha-value>)",
        primary: {
          DEFAULT: "hsl(var(--primary) / <alpha-value>)",
          foreground: "hsl(var(--primary-foreground) / <alpha-value>)",
          // A --*-hover is the opaque fill of a hovered filled button, so this
          // entry carries no <alpha-value> placeholder: the bare form is what a
          // value meant to be used opaquely looks like.
          //
          // Note this does NOT make an alpha variant impossible. Tailwind v3
          // injects the alpha into `hsl(var(--x))` anyway, so
          // `hover:bg-primary-hover/90` still compiles. Nothing in the config
          // can forbid it; what actually holds the rule is the
          // `no alpha-fill hover left on a filled variant` assertion in
          // filled-variants.test.ts.
          hover: "hsl(var(--primary-hover))"
        },
        secondary: {
          DEFAULT: "hsl(var(--secondary) / <alpha-value>)",
          foreground: "hsl(var(--secondary-foreground) / <alpha-value>)"
        },
        destructive: {
          DEFAULT: "hsl(var(--destructive) / <alpha-value>)",
          foreground: "hsl(var(--destructive-foreground) / <alpha-value>)",
          hover: "hsl(var(--destructive-hover))"
        },
        muted: {
          DEFAULT: "hsl(var(--muted) / <alpha-value>)",
          foreground: "hsl(var(--muted-foreground) / <alpha-value>)"
        },
        accent: {
          DEFAULT: "hsl(var(--accent) / <alpha-value>)",
          foreground: "hsl(var(--accent-foreground) / <alpha-value>)"
        },
        popover: {
          DEFAULT: "hsl(var(--popover) / <alpha-value>)",
          foreground: "hsl(var(--popover-foreground) / <alpha-value>)"
        },
        card: {
          DEFAULT: "hsl(var(--card) / <alpha-value>)",
          foreground: "hsl(var(--card-foreground) / <alpha-value>)"
        }
      },
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)"
      },
      keyframes: {
        "accordion-down": {
          from: { height: "0" },
          to: { height: "var(--bits-accordion-content-height)" }
        },
        "accordion-up": {
          from: { height: "var(--bits-accordion-content-height)" },
          to: { height: "0" }
        }
      },
      animation: {
        "accordion-down": "accordion-down 0.2s ease-out",
        "accordion-up": "accordion-up 0.2s ease-out"
      }
    }
  },
  plugins: []
};