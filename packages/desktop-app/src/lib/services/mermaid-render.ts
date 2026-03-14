import { createLogger } from '$lib/utils/logger';
import type { Mermaid } from 'mermaid';

const log = createLogger('MermaidRender');

// Singleton module import — loaded once, initialized lazily on first render
let mermaidModule: Promise<Mermaid> | null = null;

// Cache the last-used theme key to skip redundant initialize() calls and prevent
// race conditions where concurrent renders interleave init + render with different themes
let lastThemeKey: string | null = null;

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
  // Strip javascript: URIs from navigable/action attributes (mirrors data: URI treatment above)
  result = result.replace(/(href|src|xlink:href|action|formaction)\s*=\s*["']\s*javascript\s*:[^"']*/gi, '$1=""');
  // Strip data: URIs from href, src, and xlink:href attributes (can carry executable payloads)
  result = result.replace(/(href|src|xlink:href)\s*=\s*["']\s*data:[^"']*/gi, '$1=""');
  // Strip url(javascript:...) from inline styles and <style> blocks
  result = result.replace(/url\s*\(\s*['"]?\s*javascript\s*:[^)]*\)/gi, 'url(about:blank)');
  return result;
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
        themeVariables: buildThemeVariables(isDark)
      });
      lastThemeKey = themeKey;
    }
    // Use a unique render ID per call to avoid ID collisions when rapid re-renders
    // occur (e.g. two renders in-flight for the same node). After render, clean up
    // the temp element mermaid injects into the document.
    const renderId = `mermaid-${id}-${Date.now()}`;
    const { svg } = await mermaid.render(renderId, definition);
    document.getElementById(renderId)?.remove();
    return sanitizeSvg(svg);
  } catch (err) {
    log.error('Mermaid render failed', err);
    return null;
  }
}
