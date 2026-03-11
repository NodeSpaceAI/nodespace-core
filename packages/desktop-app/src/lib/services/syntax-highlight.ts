import { createLogger } from '$lib/utils/logger';

const log = createLogger('SyntaxHighlight');

// Token type for structured rendering (no {@html})
export type HighlightToken = {
  content: string;
  color: string; // CSS color value from theme
  fontStyle?: string; // 'italic' | 'underline' (CSS font-style values only)
  fontWeight?: string; // 'bold' (CSS font-weight; separate from fontStyle)
};

export type HighlightLine = HighlightToken[];

let highlighterPromise: Promise<import('shiki').Highlighter> | null = null;

async function getHighlighter() {
  if (!highlighterPromise) {
    highlighterPromise = (async () => {
      const { createHighlighter } = await import('shiki');
      return createHighlighter({
        themes: ['github-light', 'github-dark'],
        langs: [] // load on demand
      });
    })();
  }
  return highlighterPromise;
}

export async function highlightCode(
  code: string,
  language: string,
  isDark: boolean
): Promise<HighlightLine[] | null> {
  try {
    const highlighter = await getHighlighter();
    // Load language grammar if not already loaded
    const loadedLangs = highlighter.getLoadedLanguages();
    if (!loadedLangs.includes(language as never) && language !== 'plaintext') {
      try {
        await highlighter.loadLanguage(language as never);
      } catch {
        log.warn(`Language not supported by Shiki: ${language}, falling back to plaintext`);
        return null; // Caller falls back to plain text
      }
    }
    const theme = isDark ? 'github-dark' : 'github-light';
    const tokens = highlighter.codeToTokens(code, { lang: language as never, theme });
    // Map Shiki token structure to our HighlightToken type
    // Shiki FontStyle bitmask: 0=None, 1=Italic, 2=Bold, 4=Underline
    return tokens.tokens.map((line) =>
      line.map((token) => {
        const result: HighlightToken = {
          content: token.content,
          color: token.color ?? 'inherit'
        };
        if (token.fontStyle !== undefined && token.fontStyle !== 0) {
          // italic and underline map to CSS font-style
          const fontStyleParts: string[] = [];
          if (token.fontStyle & 1) fontStyleParts.push('italic');
          if (token.fontStyle & 4) fontStyleParts.push('underline');
          if (fontStyleParts.length > 0) result.fontStyle = fontStyleParts.join(' ');
          // bold maps to CSS font-weight (separate property)
          if (token.fontStyle & 2) result.fontWeight = 'bold';
        }
        return result;
      })
    );
  } catch (err) {
    log.error('Syntax highlighting failed', err);
    return null;
  }
}
