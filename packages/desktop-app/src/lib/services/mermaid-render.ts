import { createLogger } from '$lib/utils/logger';
import type { Mermaid } from 'mermaid';

const log = createLogger('MermaidRender');

// Singleton promise — prevents double-initialization if called concurrently
let mermaidPromise: Promise<Mermaid> | null = null;

async function getMermaid(): Promise<Mermaid> {
  if (!mermaidPromise) {
    mermaidPromise = import('mermaid').then(({ default: mermaid }) => {
      mermaid.initialize({
        startOnLoad: false,
        securityLevel: 'strict', // Sandboxed — no JS execution in diagrams
        // Note: Mermaid's theme is set globally at initialize() time and cannot be
        // changed per-render. Dark mode for Mermaid diagrams is not supported in this
        // iteration; only Shiki highlighting respects the OS color scheme.
        theme: 'default'
      });
      return mermaid;
    });
  }
  return mermaidPromise;
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
export async function renderMermaid(definition: string, id: string): Promise<string | null> {
  try {
    const mermaid = await getMermaid();
    const { svg } = await mermaid.render(`mermaid-${id}`, definition);
    return sanitizeSvg(svg);
  } catch (err) {
    log.error('Mermaid render failed', err);
    return null; // Caller shows error state
  }
}
