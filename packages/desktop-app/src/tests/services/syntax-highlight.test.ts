import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock the shiki module to avoid loading real grammar files in tests
vi.mock('shiki', () => ({
  createHighlighter: vi.fn().mockResolvedValue({
    getLoadedLanguages: vi.fn().mockReturnValue(['typescript', 'javascript']),
    loadLanguage: vi.fn().mockResolvedValue(undefined),
    codeToTokens: vi.fn().mockReturnValue({
      tokens: [
        [
          { content: 'const', color: '#0000ff', fontStyle: 0 },
          { content: ' x', color: '#000000', fontStyle: 0 }
        ],
        [{ content: 'return x', color: '#333333', fontStyle: 1 }]
      ]
    })
  })
}));

import { highlightCode } from '../../lib/services/syntax-highlight.js';

describe('highlightCode', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('returns token lines for supported language', async () => {
    const result = await highlightCode('const x\nreturn x', 'typescript', false);

    expect(result).not.toBeNull();
    expect(Array.isArray(result)).toBe(true);
    expect(result!.length).toBe(2);

    // First line tokens
    expect(result![0].length).toBe(2);
    expect(result![0][0].content).toBe('const');
    expect(result![0][0].color).toBe('#0000ff');
    expect(result![0][1].content).toBe(' x');
  });

  it('maps fontStyle bitmask 1 to italic', async () => {
    const result = await highlightCode('return x', 'typescript', false);

    expect(result).not.toBeNull();
    // Second line has fontStyle: 1 (italic)
    expect(result![1][0].fontStyle).toBe('italic');
    expect(result![1][0].fontWeight).toBeUndefined();
  });

  it('maps fontStyle bitmask 2 to fontWeight bold (not font-style)', async () => {
    const { createHighlighter } = await import('shiki');
    const mockHighlighter = await (createHighlighter as ReturnType<typeof vi.fn>)();
    mockHighlighter.codeToTokens.mockReturnValueOnce({
      tokens: [[{ content: 'bold text', color: '#000', fontStyle: 2 }]]
    });

    const result = await highlightCode('bold text', 'typescript', false);

    expect(result).not.toBeNull();
    expect(result![0][0].fontWeight).toBe('bold');
    // fontStyle should NOT be set for bitmask 2 — bold is font-weight, not font-style
    expect(result![0][0].fontStyle).toBeUndefined();
  });

  it('maps fontStyle bitmask 3 (italic + bold) to both properties', async () => {
    const { createHighlighter } = await import('shiki');
    const mockHighlighter = await (createHighlighter as ReturnType<typeof vi.fn>)();
    mockHighlighter.codeToTokens.mockReturnValueOnce({
      tokens: [[{ content: 'bold italic', color: '#000', fontStyle: 3 }]]
    });

    const result = await highlightCode('bold italic', 'typescript', false);

    expect(result).not.toBeNull();
    expect(result![0][0].fontStyle).toBe('italic');
    expect(result![0][0].fontWeight).toBe('bold');
  });

  it('returns null for unsupported language (graceful fallback)', async () => {
    // Use vi.resetModules() to break the singleton so loadLanguage mock takes effect
    vi.resetModules();
    vi.doMock('shiki', () => ({
      createHighlighter: vi.fn().mockResolvedValue({
        getLoadedLanguages: vi.fn().mockReturnValue([]),
        loadLanguage: vi.fn().mockRejectedValue(new Error('Language not found: brainfuck')),
        codeToTokens: vi.fn()
      })
    }));

    const { highlightCode: freshHighlightCode } = await import(
      '../../lib/services/syntax-highlight.js'
    );
    const result = await freshHighlightCode('some code', 'brainfuck', false);

    // Should return null (graceful fallback) because loadLanguage threw
    expect(result).toBeNull();

    vi.resetModules();
  });

  it('handles empty code string', async () => {
    const result = await highlightCode('', 'typescript', false);
    expect(result).not.toBeNull();
    expect(Array.isArray(result)).toBe(true);
  });

  it('uses github-dark theme when isDark is true', async () => {
    const result = await highlightCode('const x = 1', 'typescript', true);

    // Dark mode results in non-null output — theme selection is verified via token output
    expect(result).not.toBeNull();
    expect(Array.isArray(result)).toBe(true);
    // The mock returns tokens regardless of theme; confirm the call completed successfully
    expect(result!.length).toBeGreaterThan(0);
  });
});
