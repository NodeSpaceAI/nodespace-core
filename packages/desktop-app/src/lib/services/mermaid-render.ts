import { createLogger } from '$lib/utils/logger';
import createDOMPurify from 'dompurify';
import type { Mermaid } from 'mermaid';

const log = createLogger('MermaidRender');

// Singleton module import — loaded once, initialized lazily on first render
let mermaidModule: Promise<Mermaid> | null = null;

// Cache the last-used theme key to skip redundant initialize() calls and prevent
// race conditions where concurrent renders interleave init + render with different themes
let lastThemeKey: string | null = null;

// Monotonic counter for unique render IDs — avoids ms-precision collisions from Date.now()
let renderCounter = 0;

async function getMermaidModule(): Promise<Mermaid> {
  if (!mermaidModule) {
    mermaidModule = import('mermaid').then(({ default: mermaid }) => mermaid);
  }
  return mermaidModule;
}

/**
 * Read a CSS custom property from the document root as a resolved hsl() string.
 * The variables are stored as raw HSL components (e.g. "200 40% 55%"), so we
 * wrap them in hsl() for Mermaid's themeVariables.
 */
function cssVar(name: string, fallback: string): string {
  if (typeof document === 'undefined') return fallback;
  const raw = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return raw ? `hsl(${raw})` : fallback;
}

function buildThemeVariables(isDark: boolean) {
  const background = cssVar('--background', isDark ? '#1e1e1e' : '#fafafa');
  const foreground = cssVar('--foreground', isDark ? '#e5e5e5' : '#262626');
  const border = cssVar('--border', isDark ? '#3d3d3d' : '#e0e0e0');
  const muted = cssVar('--muted', isDark ? '#1e2a2e' : '#f0f4f5');
  const mutedFg = cssVar('--muted-foreground', isDark ? '#bfbfbf' : '#404040');
  const primary = cssVar('--primary', isDark ? '#6ab0c8' : '#4a8fa8');
  const card = cssVar('--card', isDark ? '#1e2a2e' : '#ffffff');

  return {
    // Node boxes
    background,
    primaryColor: card,
    primaryTextColor: foreground,
    primaryBorderColor: border,

    // Edges / lines
    lineColor: mutedFg,
    edgeLabelBackground: background,

    // Secondary boxes (subgraphs, alt frames, etc.)
    secondaryColor: muted,
    secondaryTextColor: foreground,
    secondaryBorderColor: border,

    // Tertiary / accent boxes
    tertiaryColor: muted,
    tertiaryTextColor: foreground,
    tertiaryBorderColor: primary,

    // Sequence diagram specifics
    actorBkg: card,
    actorBorder: border,
    actorTextColor: foreground,
    actorLineColor: mutedFg,
    signalColor: foreground,
    signalTextColor: foreground,
    labelBoxBkgColor: muted,
    labelBoxBorderColor: border,
    labelTextColor: foreground,
    loopTextColor: foreground,
    noteBkgColor: muted,
    noteBorderColor: border,
    noteTextColor: foreground,
    activationBkgColor: muted,
    activationBorderColor: primary,

    // General text
    titleColor: foreground,
    textColor: foreground,
    nodeBorder: border,
    clusterBkg: muted,
    clusterBorder: border,
    defaultLinkColor: mutedFg,
    fontFamily: 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif'
  };
}

// Build a DOMPurify instance bound to document.defaultView rather than relying on the module-
// level default export or the global `window` binding. In tests each file gets a fresh
// happy-dom document, and other test files' global-`window` stubbing/teardown can leave the
// global `window` binding stale/detached from the live document by the time this runs, even
// though `document` itself is always current.
function getDOMPurify() {
  const purify = createDOMPurify(document.defaultView ?? undefined);
  // DOMPurify treats `style` as a URI-safe attribute and does not parse its CSS content, so
  // it won't strip a javascript: URI hidden inside url(...) in an inline style. Mermaid relies
  // on inline styles for theming, so we can't forbid the attribute outright — scrub dangerous
  // CSS constructs from it directly instead. Known gap: CSS backslash-escapes/whitespace
  // splicing (e.g. "jav\61script:") can obfuscate past this regex. Accepted for now since the
  // app's CSP (script-src 'self', object-src 'none') already blocks javascript: URIs from
  // executing in any modern webview — revisit if that CSP is ever relaxed.
  purify.addHook('uponSanitizeAttribute', (_node, data) => {
    if (data.attrName === 'style') {
      data.attrValue = data.attrValue
        .replace(/(?:javascript|vbscript):/gi, '')
        .replace(/expression\s*\(/gi, '');
    }
  });
  return purify;
}

export function sanitizeSvg(svg: string): string {
  return getDOMPurify().sanitize(svg, { USE_PROFILES: { svg: true, svgFilters: true } });
}

// Returns sanitized SVG string, or null on failure
export async function renderMermaid(
  definition: string,
  id: string,
  isDark = false
): Promise<string | null> {
  try {
    const themeKey = isDark ? 'dark' : 'light';
    const mermaid = await getMermaidModule();
    if (themeKey !== lastThemeKey) {
      mermaid.initialize({
        startOnLoad: false,
        securityLevel: 'strict',
        theme: 'base',
        themeVariables: buildThemeVariables(isDark),
        // Emit node labels as native SVG <text>, not HTML inside <foreignObject>.
        // sanitizeSvg() runs DOMPurify's `svg` profile, which drops <foreignObject>
        // (it can host arbitrary HTML) — so with mermaid's default htmlLabels:true
        // every label is stripped and diagrams render as empty, wordless boxes.
        // Mermaid v11 reads the root-level flag for the shared layout, so the
        // per-diagram flags alone leave labels in <foreignObject>; set all three.
        // sequence/state/er emit <text> natively and need no flag.
        htmlLabels: false,
        flowchart: { htmlLabels: false },
        class: { htmlLabels: false }
      });
      lastThemeKey = themeKey;
    }
    // Use a unique render ID per call to avoid ID collisions when rapid re-renders
    // occur (e.g. two renders in-flight for the same node). The temp element mermaid
    // injects into the document is cleaned up in finally so it always runs even on failure.
    const renderId = `mermaid-${id}-${++renderCounter}`;
    try {
      const { svg } = await mermaid.render(renderId, definition);
      return sanitizeSvg(svg);
    } finally {
      document.getElementById(renderId)?.remove();
    }
  } catch (err) {
    log.error('Mermaid render failed', err);
    return null;
  }
}
