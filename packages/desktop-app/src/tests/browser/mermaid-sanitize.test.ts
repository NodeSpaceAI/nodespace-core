import { describe, it, expect, vi } from 'vitest';
import { sanitizeSvg } from '../../lib/services/mermaid-render.js';

// Runs under real Chromium (see vitest.browser.config.ts), not Happy-DOM. DOMPurify
// versions past 3.4.7 fail open under Happy-DOM's DOM approximation — sanitizeSvg
// (and renderMermaid, which calls it unconditionally on every render) are only
// meaningfully tested against a real browser DOM, which is why this whole suite
// lives in the browser tier instead of the fast Happy-DOM tier.
describe('sanitizeSvg', () => {
  it('removes script tags from SVG output', () => {
    const svg = '<svg><script>alert("xss")</script></svg>';
    const result = sanitizeSvg(svg);
    expect(result).not.toContain('<script');
    expect(result).not.toContain('alert("xss")');
  });

  it('removes multiline script tags', () => {
    const svg = '<svg><script type="text/javascript">\nalert("xss");\n</script></svg>';
    const result = sanitizeSvg(svg);
    expect(result).not.toContain('<script');
    expect(result).not.toContain('alert');
  });

  it('removes event handler attributes with double-quoted values', () => {
    const svg = '<svg><rect onclick="alert(1)" onmouseover="evil()"/></svg>';
    const result = sanitizeSvg(svg);
    expect(result).not.toContain('onclick');
    expect(result).not.toContain('onmouseover');
    expect(result).toContain('<rect');
  });

  it('removes event handler attributes with single-quoted values', () => {
    const svg = "<svg><rect onclick='alert(1)'/></svg>";
    const result = sanitizeSvg(svg);
    expect(result).not.toContain('onclick');
  });

  it('removes event handler attributes with unquoted values', () => {
    const svg = '<svg><rect onclick=alert(1)/></svg>';
    const result = sanitizeSvg(svg);
    expect(result).not.toContain('onclick');
  });

  it('removes javascript: URIs', () => {
    const svg = '<svg><a href="javascript:alert(1)">click</a></svg>';
    const result = sanitizeSvg(svg);
    expect(result).not.toContain('javascript:');
  });

  it('removes javascript: URIs with whitespace after colon', () => {
    const svg = '<svg><a href="javascript: alert(1)">click</a></svg>';
    const result = sanitizeSvg(svg);
    expect(result).not.toContain('javascript:');
    expect(result).not.toContain('javascript');
  });

  it('removes data: URIs from href attributes', () => {
    const svg = '<svg><a href="data:text/html,<script>alert(1)</script>">click</a></svg>';
    const result = sanitizeSvg(svg);
    expect(result).not.toContain('data:text/html');
  });

  it('removes url(javascript:...) from inline styles', () => {
    const svg = '<svg><rect style="fill:url(javascript:alert(1))"/></svg>';
    const result = sanitizeSvg(svg);
    expect(result).not.toContain('javascript:');
  });

  it('preserves legitimate SVG content', () => {
    const svg =
      '<svg viewBox="0 0 100 100"><rect x="10" y="10" width="80" height="80"/><text>Hello</text></svg>';
    const result = sanitizeSvg(svg);
    expect(result).toContain('<rect');
    expect(result).toContain('<text>');
    expect(result).toContain('viewBox');
  });

  it('handles empty SVG string', () => {
    const result = sanitizeSvg('');
    expect(result).toBe('');
  });

  it('handles SVG with no threats', () => {
    const clean = '<svg><circle cx="50" cy="50" r="40"/></svg>';
    const result = sanitizeSvg(clean);
    expect(result).toContain('<circle');
    expect(result).toContain('cx="50"');
    expect(result).toContain('cy="50"');
    expect(result).toContain('r="40"');
  });

  it('strips foreignObject HTML labels but preserves native <text> labels', () => {
    // Root cause of the wordless-diagram bug: mermaid's default htmlLabels:true
    // wraps every node label in <foreignObject> (arbitrary HTML), which the SVG
    // profile removes — so the labels vanish. Native <text> labels (emitted when
    // htmlLabels:false) must survive sanitization. This is why renderMermaid
    // initializes with flowchart/class htmlLabels disabled.
    const withForeignObject =
      '<svg><g class="node"><foreignObject width="80" height="20"><div xmlns="http://www.w3.org/1999/xhtml">Label A</div></foreignObject></g></svg>';
    const foResult = sanitizeSvg(withForeignObject);
    expect(foResult).not.toContain('foreignObject');

    const withText = '<svg><g class="node"><text>Label A</text></g></svg>';
    const textResult = sanitizeSvg(withText);
    expect(textResult).toContain('<text>');
    expect(textResult).toContain('Label A');
  });
});

// Mock the mermaid module only — renderMermaid's own sanitizeSvg call runs for
// real, which is the whole point of this file living in the browser tier.
vi.mock('mermaid', () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn().mockResolvedValue({ svg: '<svg><text>diagram</text></svg>' })
  }
}));

describe('renderMermaid', () => {
  it('returns null on render failure', async () => {
    const { default: mermaid } = await import('mermaid');
    (mermaid.render as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('Invalid syntax'));

    const { renderMermaid } = await import('../../lib/services/mermaid-render.js');
    const result = await renderMermaid('invalid mermaid syntax %%%', 'test-id');
    expect(result).toBeNull();
  });

  it('returns sanitized SVG string on success', async () => {
    const { default: mermaid } = await import('mermaid');
    (mermaid.render as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      svg: '<svg><text>flowchart</text></svg>'
    });

    const { renderMermaid } = await import('../../lib/services/mermaid-render.js');
    const result = await renderMermaid('graph TD; A-->B;', 'test-id');
    expect(result).not.toBeNull();
    expect(result).toContain('<svg>');
    expect(result).toContain('flowchart');
  });

  it('initializes mermaid with HTML labels disabled (labels survive sanitization)', async () => {
    const { default: mermaid } = await import('mermaid');
    (mermaid.initialize as ReturnType<typeof vi.fn>).mockClear();
    (mermaid.render as ReturnType<typeof vi.fn>).mockResolvedValue({
      svg: '<svg><text>x</text></svg>'
    });

    const { renderMermaid } = await import('../../lib/services/mermaid-render.js');
    // Toggle the theme across two renders so an initialize() call is guaranteed
    // regardless of the module-level theme cache's prior state.
    await renderMermaid('graph TD; A-->B;', 'init-light', false);
    await renderMermaid('graph TD; A-->B;', 'init-dark', true);

    expect(mermaid.initialize).toHaveBeenCalled();
    const cfg = (mermaid.initialize as ReturnType<typeof vi.fn>).mock.calls.at(-1)?.[0] as {
      htmlLabels?: boolean;
      flowchart?: { htmlLabels?: boolean };
      class?: { htmlLabels?: boolean };
    };
    expect(cfg.htmlLabels).toBe(false);
    expect(cfg.flowchart?.htmlLabels).toBe(false);
    expect(cfg.class?.htmlLabels).toBe(false);
  });

  it.each([
    ['light', false],
    ['dark', true]
  ])('renders flowchart labels as visible <text> with no foreignObject (%s)', async (label, isDark) => {
    // Real mermaid, real sanitizer: the mock delegates to the actual module so
    // renderMermaid's own initialize() config is what drives the layout.
    const { default: mermaid } = await import('mermaid');
    const actual = (await vi.importActual<typeof import('mermaid')>('mermaid')).default;
    (mermaid.initialize as ReturnType<typeof vi.fn>).mockImplementation(actual.initialize);
    (mermaid.render as ReturnType<typeof vi.fn>).mockImplementation(actual.render);

    try {
      const { renderMermaid } = await import('../../lib/services/mermaid-render.js');
      const result = await renderMermaid('graph TD; Alpha-->Beta;', `real-${label}`, isDark);
      expect(result).not.toBeNull();
      expect(result).not.toContain('foreignObject');
      const doc = new window.DOMParser().parseFromString(result as string, 'image/svg+xml');
      const text = Array.from(doc.querySelectorAll('text')).map((t) => t.textContent);
      expect(text.join(' ')).toContain('Alpha');
      expect(text.join(' ')).toContain('Beta');
    } finally {
      (mermaid.initialize as ReturnType<typeof vi.fn>).mockReset();
      (mermaid.render as ReturnType<typeof vi.fn>).mockReset();
    }
  });
});
