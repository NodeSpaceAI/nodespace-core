import { createLogger } from '$lib/utils/logger';

const log = createLogger('MermaidRender');

let mermaidInitialized = false;

async function getMermaid() {
  const { default: mermaid } = await import('mermaid');
  if (!mermaidInitialized) {
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: 'strict', // Sandboxed — no JS execution in diagrams
      theme: 'default'
    });
    mermaidInitialized = true;
  }
  return mermaid;
}

export function sanitizeSvg(svg: string): string {
  // Remove script tags and event handlers from SVG output
  return svg
    .replace(/<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>/gi, '')
    .replace(/\s*on\w+\s*=\s*["'][^"']*["']/gi, '')
    .replace(/javascript:[^"'\s>]*/gi, '');
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
