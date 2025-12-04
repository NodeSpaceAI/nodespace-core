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
 * - Disabled list rendering to prevent "1. Text" patterns from becoming HTML lists
 *
 * Examples of what this processes:
 * ✅ "**bold text**" → <span class="markdown-bold">bold text</span>
 * ✅ "*italic text*" → <span class="markdown-italic">italic text</span>
 * ✅ "`code`" → <code class="markdown-code-inline">code</code>
 * ✅ "1. Item" → "1. Item" (preserved as plain text, NOT <ol><li>Item</li></ol>)
 * ❌ "# Header" → "# Header" (preserved as plain text, NOT <h1>Header</h1>)
 *
 * GFM (GitHub Flavored Markdown) Features:
 * - GFM is enabled (gfm: true) for better markdown compatibility
 * - Tables with pipe syntax (| Header |) will render with bold first row (GFM standard)
 * - This is INTENDED behavior - first row of GFM tables represents headers
 *
 * Why GFM is enabled despite NodeSpace's semantic node types:
 * - Provides robust inline formatting parsing (bold, italic, strikethrough, code)
 * - Handles edge cases better than custom regex parsers (see Issue #350)
 * - Table rendering aligns with user expectations from GitHub/Discord
 * - List rendering is overridden (see list() renderer) to fit NodeSpace's architecture
 */

import { marked } from 'marked';
import type { Tokens } from 'marked';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('MarkedConfig');

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

    // Override inline code (codespan) rendering to match code block styling
    codespan(token: Tokens.Codespan): string {
      return `<code class="markdown-code-inline">${token.text}</code>`;
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
    },

    // CRITICAL: Disable ordered list processing to prevent "### 1. Text" from becoming a list
    // GFM detects "1. " pattern and creates lists, but for header nodes this is unwanted
    //
    // ARCHITECTURAL DECISION:
    // NodeSpace uses semantic node types (header, task, ordered-list) rather than
    // markdown-based list rendering. Users create lists via /ordered-list slash command,
    // not markdown syntax (1., 2., 3.). This renderer preserves list markers as plain text,
    // which is correct for our architecture where ordered-list nodes have dedicated components.
    //
    // FUTURE CONSIDERATION:
    // If rich-text nodes or markdown import features are added, may need conditional
    // list rendering based on node type context.
    list(token: Tokens.List): string {
      // Return plain text representation of list items with their markers preserved
      const items = token.items.map((item, index) => {
        // Recursively parse item tokens (handles nested formatting like **bold**)
        const text = this.parser.parse(item.tokens);
        // Preserve the list marker (1., 2., -, etc.) as plain text, NOT <li> tags
        let marker = '';
        if (token.ordered) {
          // For ordered lists, use the actual number (start + index) followed by dot and space
          const itemNumber = (token.start || 1) + index;
          marker = `${itemNumber}. `;
        } else {
          // For unordered lists, use dash
          marker = '- ';
        }
        // Add task checkbox if present
        const taskMarker = item.task ? (item.checked ? '[x] ' : '[ ] ') : '';
        return `${marker}${taskMarker}${text}`;
      });
      return items.join('\n');
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
      log.warn('marked() returned Promise unexpectedly, falling back to plain text');
      return escapeHtml(markdown);
    }
  } catch (error) {
    log.warn('marked.js parsing error:', error);
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
