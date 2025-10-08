/**
 * Simple Markdown Utilities
 *
 * Provides basic markdown parsing for TextNode components.
 * Supports headers, bold, italic, and basic formatting.
 * Lightweight implementation focused on essential features.
 */

export interface MarkdownOptions {
  allowHeadings: boolean;
  allowBold: boolean;
  allowItalic: boolean;
  allowLinks: boolean;
  allowCode: boolean;
  allowLineBreaks: boolean;
}

const defaultOptions: MarkdownOptions = {
  allowHeadings: true,
  allowBold: true,
  allowItalic: true,
  allowLinks: true,
  allowCode: true,
  allowLineBreaks: true
};

/**
 * Convert markdown text to HTML
 */
export function parseMarkdown(markdown: string, options: Partial<MarkdownOptions> = {}): string {
  if (!markdown) return '';

  const opts = { ...defaultOptions, ...options };
  let html = markdown;

  // Escape HTML entities first
  html = escapeHtml(html);

  // Process block elements first (headers, paragraphs)
  if (opts.allowHeadings) {
    html = processHeadings(html);
  }

  // Process inline elements
  if (opts.allowBold) {
    html = processBold(html);
  }

  if (opts.allowItalic) {
    html = processItalic(html);
  }

  if (opts.allowCode) {
    html = processInlineCode(html);
  }

  if (opts.allowLinks) {
    html = processLinks(html);
  }

  // Process line breaks last
  if (opts.allowLineBreaks) {
    html = processLineBreaks(html);
  }

  return html.trim();
}

/**
 * Escape HTML entities to prevent XSS
 */
function escapeHtml(text: string): string {
  const div = document.createElement('div');
  div.textContent = text;
  return div.innerHTML;
}

/**
 * Process markdown headings (# ## ###)
 */
function processHeadings(html: string): string {
  return html.replace(/^(#{1,6})\s+(.+)$/gm, (match, hashes, content) => {
    const level = hashes.length;
    return `<h${level} class="ns-markdown-heading ns-markdown-h${level}">${content.trim()}</h${level}>`;
  });
}

/**
 * Process bold text (**text** or __text__)
 */
function processBold(html: string): string {
  return html
    .replace(/\*\*(.*?)\*\*/g, '<strong class="ns-markdown-bold">$1</strong>')
    .replace(/__(.*?)__/g, '<strong class="ns-markdown-bold">$1</strong>');
}

/**
 * Process italic text (*text* or _text_)
 */
function processItalic(html: string): string {
  return html
    .replace(/(?<!\*)\*(?!\*)([^*]+)\*(?!\*)/g, '<em class="ns-markdown-italic">$1</em>')
    .replace(/(?<!_)_(?!_)([^_]+)_(?!_)/g, '<em class="ns-markdown-italic">$1</em>');
}

/**
 * Process inline code (`code`)
 */
function processInlineCode(html: string): string {
  return html.replace(/`([^`]+)`/g, '<code class="ns-markdown-code">$1</code>');
}

/**
 * Process simple links [text](url)
 */
function processLinks(html: string): string {
  return html.replace(
    /\[([^\]]+)\]\(([^)]+)\)/g,
    '<a href="$2" class="ns-markdown-link" target="_blank" rel="noopener noreferrer">$1</a>'
  );
}

/**
 * Process line breaks (convert double newlines to paragraphs, single to <br>)
 */
function processLineBreaks(html: string): string {
  // Split by double newlines for paragraphs
  const paragraphs = html.split(/\n\s*\n/);

  return paragraphs
    .map((p) => {
      if (!p.trim()) return '';

      // Check if this is already wrapped in a block element
      if (p.trim().startsWith('<h') || p.trim().startsWith('<p')) {
        return p.replace(/\n/g, '<br>');
      }

      // Wrap in paragraph and convert single newlines to <br>
      const content = p.replace(/\n/g, '<br>');
      return `<p class="ns-markdown-paragraph">${content}</p>`;
    })
    .filter((p) => p.length > 0)
    .join('');
}

/**
 * Strip all markdown formatting (for plain text extraction)
 */
export function stripMarkdown(markdown: string): string {
  if (!markdown) return '';

  return (
    markdown
      // Remove headings
      .replace(/^#{1,6}\s+/gm, '')
      // Remove bold/italic
      .replace(/\*\*(.*?)\*\*/g, '$1')
      .replace(/__(.*?)__/g, '$1')
      .replace(/\*([^*]+)\*/g, '$1')
      .replace(/_([^_]+)_/g, '$1')
      // Remove inline code
      .replace(/`([^`]+)`/g, '$1')
      // Remove links
      .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
      // Normalize whitespace
      .replace(/\s+/g, ' ')
      .trim()
  );
}

/**
 * Get word count from markdown text
 */
export function getWordCount(markdown: string): number {
  const plainText = stripMarkdown(markdown);
  if (!plainText) return 0;

  return plainText.split(/\s+/).filter((word) => word.length > 0).length;
}

/**
 * Validate markdown syntax (basic check)
 */
export function validateMarkdown(markdown: string): { isValid: boolean; errors: string[] } {
  const errors: string[] = [];

  // Check for unclosed bold/italic markers
  const boldCount = (markdown.match(/\*\*/g) || []).length;
  const italicCount = (markdown.match(/(?<!\*)\*(?!\*)/g) || []).length;
  const underscoreBoldCount = (markdown.match(/__/g) || []).length;
  const underscoreItalicCount = (markdown.match(/(?<!_)_(?!_)/g) || []).length;

  if (boldCount % 2 !== 0) {
    errors.push('Unclosed bold markers (**)');
  }

  if (italicCount % 2 !== 0) {
    errors.push('Unclosed italic markers (*)');
  }

  if (underscoreBoldCount % 2 !== 0) {
    errors.push('Unclosed bold markers (__)');
  }

  if (underscoreItalicCount % 2 !== 0) {
    errors.push('Unclosed italic markers (_)');
  }

  // Check for unclosed code markers
  const codeCount = (markdown.match(/`/g) || []).length;
  if (codeCount % 2 !== 0) {
    errors.push('Unclosed code markers (`)');
  }

  return {
    isValid: errors.length === 0,
    errors
  };
}

/**
 * Preview markdown without full parsing (for performance)
 */
export function previewMarkdown(markdown: string, maxLength: number = 150): string {
  const plainText = stripMarkdown(markdown);

  if (plainText.length <= maxLength) {
    return plainText;
  }

  return plainText.substring(0, maxLength).trim() + '...';
}
