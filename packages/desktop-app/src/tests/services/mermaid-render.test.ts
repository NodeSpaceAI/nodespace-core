import { describe, it, expect, vi } from 'vitest';
import { sanitizeSvg } from '../../lib/services/mermaid-render.js';

// Mock the mermaid module to avoid DOM/browser dependencies in tests
vi.mock('mermaid', () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn().mockResolvedValue({ svg: '<svg><text>diagram</text></svg>' })
  }
}));

describe('sanitizeSvg', () => {
  it('removes script tags from SVG output', () => {
    const svg = '<svg><script>alert("xss")</script><rect/></svg>';
    const result = sanitizeSvg(svg);
    expect(result).not.toContain('<script');
    expect(result).not.toContain('alert("xss")');
    expect(result).toContain('<rect/>');
  });

  it('removes multiline script tags', () => {
    const svg = '<svg><script type="text/javascript">\nalert("xss");\n</script><rect/></svg>';
    const result = sanitizeSvg(svg);
    expect(result).not.toContain('<script');
    expect(result).not.toContain('alert');
    expect(result).toContain('<rect/>');
  });

  it('removes event handler attributes', () => {
    const svg = '<svg><rect onclick="alert(1)" onmouseover="evil()"/></svg>';
    const result = sanitizeSvg(svg);
    expect(result).not.toContain('onclick');
    expect(result).not.toContain('onmouseover');
  });

  it('removes javascript: URIs', () => {
    const svg = '<svg><a href="javascript:alert(1)">click</a></svg>';
    const result = sanitizeSvg(svg);
    expect(result).not.toContain('javascript:');
  });

  it('preserves legitimate SVG content', () => {
    const svg = '<svg viewBox="0 0 100 100"><rect x="10" y="10" width="80" height="80"/><text>Hello</text></svg>';
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
    expect(result).toBe(clean);
  });
});

describe('renderMermaid', () => {
  it('returns null on render failure', async () => {
    const { default: mermaid } = await import('mermaid');
    (mermaid.render as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
      new Error('Invalid syntax')
    );

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
});
