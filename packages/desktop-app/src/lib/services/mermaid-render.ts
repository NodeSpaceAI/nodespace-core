import { createLogger } from '$lib/utils/logger';
import type { Mermaid } from 'mermaid';

const log = createLogger('MermaidRender');

// Singleton module import — loaded once, re-initialized per render when theme changes
let mermaidModule: Promise<Mermaid> | null = null;

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

export function sanitizeSvg(svg: string): string {
  // Remove script tags
  let result = svg.replace(/<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>/gi, '');
  // Remove event handler attributes (all quoting styles: "...", '...', or unquoted)
  result = result.replace(/\s+on\w+(\s*=\s*("[^"]*"|'[^']*'|[^\s>]*))?/gi, '');
  // Remove javascript: URIs (including javascript: with whitespace after colon)
  result = result.replace(/javascript\s*:[^"'\s>]*/gi, '');
  return result;
}

// Returns sanitized SVG string, or null on failure
export async function renderMermaid(
  definition: string,
  id: string,
  isDark = false
): Promise<string | null> {
  try {
    const mermaid = await getMermaidModule();
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: 'strict',
      theme: 'base',
      themeVariables: buildThemeVariables(isDark)
    });
    const { svg } = await mermaid.render(`mermaid-${id}`, definition);
    return sanitizeSvg(svg);
  } catch (err) {
    log.error('Mermaid render failed', err);
    return null;
  }
}
