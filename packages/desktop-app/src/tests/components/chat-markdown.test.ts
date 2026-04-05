/**
 * Unit tests for ChatMarkdown component — node card placeholder rendering
 *
 * Tests that nodespace:// URIs are rendered as placeholder spans
 * for hydration by NodeCardInline components.
 */

import { describe, it, expect, vi } from 'vitest';
import { marked, Renderer, type Tokens } from 'marked';

// Mock logger
vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

// Replicate the renderer logic from chat-markdown.svelte for unit testing
function createChatRenderer(): Renderer {
  const renderer = new Renderer();
  renderer.link = function (token: Tokens.Link): string {
    const href = token.href ?? '';
    const text = this.parser.parseInline(token.tokens);

    const nsMatch = href.match(/^nodespace:\/\/(.+)$/);
    if (nsMatch) {
      const nodeId = nsMatch[1];
      const safeText = text.replace(/"/g, '&quot;');
      return `<span class="ns-node-card-placeholder" data-node-id="${nodeId}" data-display-text="${safeText}"></span>`;
    }

    return `<a href="${href}" target="_blank" rel="noopener noreferrer">${text}</a>`;
  };
  return renderer;
}

function autolinkNodespaceUris(md: string): string {
  return md.replace(
    /(?<!\]\()(nodespace:\/\/[a-f0-9-]+)/gi,
    '[$1]($1)'
  );
}

function renderMarkdown(md: string): string {
  if (!md) return '';
  const raw = marked(autolinkNodespaceUris(md), {
    renderer: createChatRenderer(),
    breaks: true,
    gfm: true,
  });
  if (typeof raw !== 'string') return md;
  // Return raw HTML (DOMPurify sanitization tested separately via component tests)
  return raw;
}

describe('ChatMarkdown Rendering', () => {
  it('should render nodespace:// URIs as placeholder spans, not links', () => {
    const result = renderMarkdown('Check this: nodespace://abc-123-def');
    expect(result).toContain('ns-node-card-placeholder');
    expect(result).toContain('data-node-id="abc-123-def"');
    expect(result).not.toContain('<a href="nodespace://');
  });

  it('should render external links as regular anchors', () => {
    const result = renderMarkdown('See [Google](https://google.com)');
    expect(result).toContain('href="https://google.com"');
    expect(result).not.toContain('ns-node-card-placeholder');
  });

  it('should handle multiple nodespace:// URIs in one message', () => {
    const result = renderMarkdown('Nodes: nodespace://aaa-111 and nodespace://bbb-222');
    const placeholders = result.match(/ns-node-card-placeholder/g);
    expect(placeholders).toHaveLength(2);
    expect(result).toContain('data-node-id="aaa-111"');
    expect(result).toContain('data-node-id="bbb-222"');
  });

  it('should preserve display text in data attribute', () => {
    const result = renderMarkdown('[My Document](nodespace://abc-123)');
    expect(result).toContain('data-display-text="My Document"');
    expect(result).toContain('data-node-id="abc-123"');
  });

  it('should auto-link bare nodespace:// URIs', () => {
    const result = renderMarkdown('nodespace://abc-def-123');
    expect(result).toContain('ns-node-card-placeholder');
    expect(result).toContain('data-node-id="abc-def-123"');
  });

  it('should return empty string for empty content', () => {
    expect(renderMarkdown('')).toBe('');
  });

  it('should handle mixed content with markdown and node URIs', () => {
    const result = renderMarkdown('# Title\n\nSee nodespace://abc-123 for **details**.');
    expect(result).toContain('<h1>');
    expect(result).toContain('ns-node-card-placeholder');
    expect(result).toContain('<strong>details</strong>');
  });

  it('should render script tags in display text as placeholder (DOMPurify sanitizes in component)', () => {
    const result = renderMarkdown('[<script>alert(1)</script>](nodespace://abc-123)');
    // Renderer outputs placeholder span; DOMPurify strips <script> tags in the actual component
    expect(result).toContain('ns-node-card-placeholder');
    expect(result).toContain('data-node-id="abc-123"');
  });

  it('should escape quotes in display text data attribute', () => {
    const result = renderMarkdown('[He said "hello"](nodespace://abc-123)');
    expect(result).toContain('data-display-text=');
    expect(result).toContain('ns-node-card-placeholder');
    // Double quotes in text should be escaped to &quot;
    expect(result).not.toMatch(/data-display-text="[^"]*"[^"]*"/);
  });
});
