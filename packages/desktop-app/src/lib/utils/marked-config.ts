/**
 * Custom marked.js configuration for NodeSpace
 *
 * CRITICAL: This configuration is designed ONLY for inline formatting (bold, italic).
 * Headers are handled separately by NodeSpace's inheritHeaderLevel system and
 * rendered as actual <h1>, <h2>, <h3> elements in base-node.svelte.
 *
 * This marked.js integration provides:
 * - Custom CSS classes for inline formatting compatibility
 * - Robust parsing for edge cases (nested formatting, malformed syntax)
 * - Preservation of header syntax as plain text (not processed as HTML headers)
 *
 * Examples of what this processes:
 * ✅ "**bold text**" → <span class="markdown-bold">bold text</span>
 * ✅ "*italic text*" → <span class="markdown-italic">italic text</span>
 * ❌ "# Header" → "# Header" (preserved as plain text, NOT <h1>Header</h1>)
 */

import { marked } from 'marked';
import type { Tokens } from 'marked';

// Configure marked with custom renderer that uses NodeSpace CSS classes
marked.use({
  renderer: {
    // Override strong rendering to use custom CSS classes
    strong(token: Tokens.Strong): string {
      // Extract the rendered text content from the token
      const text = this.parser.parseInline(token.tokens);
      return `<span class="markdown-bold">${text}</span>`;
    },

    // Override em rendering to use custom CSS classes
    em(token: Tokens.Em): string {
      // Extract the rendered text content from the token
      const text = this.parser.parseInline(token.tokens);
      return `<span class="markdown-italic">${text}</span>`;
    },

    // Override paragraph to avoid wrapping inline content in <p> tags
    paragraph(token: Tokens.Paragraph): string {
      const text = this.parser.parseInline(token.tokens);
      return text;
    },

    // CRITICAL: Disable header processing - NodeSpace handles headers separately
    // Headers are controlled by inheritHeaderLevel property, not markdown syntax
    heading(token: Tokens.Heading): string {
      // Return the raw token text instead of processing as HTML header
      // This preserves "# Header text" as plain text for NodeSpace's header system
      const level = '#'.repeat(token.depth);
      const text = this.parser.parseInline(token.tokens);
      return `${level} ${text}`;
    }
  },
  // Configure options
  breaks: true, // Convert \n to <br> for proper line break rendering
  gfm: true // GitHub Flavored Markdown
});

/**
 * Convert markdown to HTML using marked.js with NodeSpace styling
 *
 * This replaces the custom regex-based parser in ContentEditableController
 * and handles edge cases like nested formatting correctly.
 */
export function markdownToHtml(markdown: string): string {
  try {
    const html = marked(markdown);
    // Handle both sync and async returns from marked()
    if (typeof html === 'string') {
      // Strip <p> tags but preserve leading/trailing whitespace
      return html.replace(/^<p>|<\/p>$/g, '');
    } else {
      // If marked returns a Promise (shouldn't happen with our config, but handle it)
      console.warn('marked() returned Promise unexpectedly, falling back to plain text');
      return escapeHtml(markdown);
    }
  } catch (error) {
    console.warn('marked.js parsing error:', error);
    // Fallback to plain text if parsing fails
    return escapeHtml(markdown);
  }
}

/**
 * Convert HTML back to markdown
 * This handles both standard formatting and mixed syntax patterns
 * that NodeSpace uses (bold, italic, and combinations including mixed markers)
 */
export function htmlToMarkdown(html: string): string {
  let markdown = html;

  // Handle the edit-mode format with syntax preservation first
  // This regex matches the edit-mode format: <span class="markdown-syntax">MARKER<span class="...">content</span>MARKER</span>
  markdown = markdown.replace(
    /<span class="markdown-syntax">([^<]+)<span class="[^"]*">(.*?)<\/span>([^<]+)<\/span>/g,
    (match, openMarker, content, closeMarker) => {
      // Return the original markdown syntax
      return openMarker + content + closeMarker;
    }
  );

  // Handle nested combinations for non-edit mode (order matters)
  // Bold + Italic combinations
  markdown = markdown.replace(
    /<span class="markdown-bold markdown-italic">(.*?)<\/span>/g,
    '***$1***'
  );
  markdown = markdown.replace(
    /<span class="markdown-italic markdown-bold">(.*?)<\/span>/g,
    '***$1***'
  );

  // Handle individual formatting for non-edit mode
  markdown = markdown.replace(/<span class="markdown-bold">(.*?)<\/span>/g, '**$1**');
  markdown = markdown.replace(/<span class="markdown-italic">(.*?)<\/span>/g, '*$1*');

  // Handle standard HTML tags in case they slip through
  markdown = markdown.replace(/<strong>(.*?)<\/strong>/g, '**$1**');
  markdown = markdown.replace(/<em>(.*?)<\/em>/g, '*$1*');

  // CRITICAL: Handle HTML headers that shouldn't have been created
  // Convert back to markdown header syntax (failsafe for any edge cases)
  markdown = markdown.replace(/<h([1-6])>(.*?)<\/h[1-6]>/g, (match, level, content) => {
    const headerSymbols = '#'.repeat(parseInt(level));
    return `${headerSymbols} ${content}`;
  });

  // Clean up any remaining HTML tags
  markdown = markdown.replace(/<[^>]*>/g, '');

  return markdown;
}

/**
 * Escape HTML characters to prevent XSS
 */
function escapeHtml(text: string): string {
  const div = document.createElement('div');
  div.textContent = text;
  return div.innerHTML;
}
