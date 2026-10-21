/**
 * View-mode markdown parser (extracted from view-mode-renderer.svelte so the real
 * parse path is unit-testable, not just a simplified re-implementation).
 *
 * Turns raw node content into a ViewNode tree: markdown via marked, bare
 * `[[node-id]]` wikilinks routed to `noderef` nodes (see wikilink-refs), and
 * inline emphasis/code preserved. Pure — no component state.
 */
import { isSafeLinkHref } from '$lib/utils/safe-link';
import { marked } from "marked";
import type { Token, Tokens } from "marked";
import { splitTextIntoRefSegments } from "./wikilink-refs";

// Node types for structured rendering
export type ViewNode =
  | { type: 'text'; content: string }
  | { type: 'br' }
  | { type: 'bold'; children: ViewNode[] }
  | { type: 'italic'; children: ViewNode[] }
  | { type: 'strikethrough'; children: ViewNode[] }
  | { type: 'code'; content: string }
  | { type: 'bold-italic'; children: ViewNode[] }
  | { type: 'link'; href: string; children: ViewNode[] }
  // Bare `[[node-id]]` wikilink resolved to a clickable node reference
  | { type: 'noderef'; id: string }
  // Block-level elements (enabled via enableBlockElements prop)
  | { type: 'heading'; level: number; children: ViewNode[] }
  | { type: 'list'; ordered: boolean; items: ViewNode[][] }
  | { type: 'paragraph'; children: ViewNode[] };

/**
 * Parse content into ViewNode array for rendering
 */
export function parseContent(rawContent: string, markdown: boolean, blockElements: boolean): ViewNode[] {
  if (!rawContent) return [];

  // Use displayContent if provided, otherwise use content
  let processedContent = rawContent;

  if (!markdown) {
    // Raw text mode - just split by newlines
    return parseRawText(processedContent);
  }

  // Pre-process blank lines before marked parsing
  // marked.js with breaks:true converts all \n to <br>, losing \n\n detection
  const BLANK_LINE_PLACEHOLDER = '\u200B___BLANK___\u200B';

  processedContent = processedContent.replace(/\n\n+/g, (match) => {
    const blankLineCount = match.length - 1;
    return '\n' + BLANK_LINE_PLACEHOLDER.repeat(blankLineCount);
  });

  // Use marked.lexer to get tokens
  const tokens = marked.lexer(processedContent);
  const nodes: ViewNode[] = [];

  // Handle leading newlines
  const leadingNewlines = rawContent.match(/^\n+/);
  if (leadingNewlines) {
    for (let i = 0; i < leadingNewlines[0].length; i++) {
      nodes.push({ type: 'br' });
    }
  }

  // Process tokens
  for (const token of tokens) {
    nodes.push(...processToken(token, BLANK_LINE_PLACEHOLDER, blockElements));
  }

  // Handle trailing newlines
  const trailingNewlines = rawContent.match(/\n+$/);
  if (trailingNewlines) {
    for (let i = 0; i < trailingNewlines[0].length + 1; i++) {
      nodes.push({ type: 'br' });
    }
  }

  return nodes;
}

/**
 * Convert a single plain-text run into ViewNodes, splitting out any bare
 * `[[node-id]]` wikilinks (valid UUID/date ids) into `noderef` nodes while
 * leaving everything else — including invalid `[[...]]` tokens — as text.
 */
export function textRunToNodes(text: string): ViewNode[] {
  const nodes: ViewNode[] = [];
  for (const segment of splitTextIntoRefSegments(text)) {
    if (segment.kind === 'ref') {
      nodes.push({ type: 'noderef', id: segment.id });
    } else if (segment.value) {
      nodes.push({ type: 'text', content: segment.value });
    }
  }
  return nodes;
}

/**
 * Parse raw text (disableMarkdown mode)
 */
export function parseRawText(text: string): ViewNode[] {
  const lines = text.split('\n');
  const nodes: ViewNode[] = [];

  for (let i = 0; i < lines.length; i++) {
    if (i > 0) {
      nodes.push({ type: 'br' });
    }
    if (lines[i]) {
      nodes.push(...textRunToNodes(lines[i]));
    }
  }

  return nodes;
}

/**
 * Process a marked token into ViewNodes
 */
export function processToken(token: Token, blankPlaceholder: string, blockElements: boolean = false): ViewNode[] {
  const nodes: ViewNode[] = [];

  switch (token.type) {
    case 'paragraph': {
      const para = token as Tokens.Paragraph;
      if (para.tokens) {
        if (blockElements) {
          // Render as proper paragraph block
          const children = processInlineTokens(para.tokens, blankPlaceholder);
          nodes.push({ type: 'paragraph', children });
        } else {
          nodes.push(...processInlineTokens(para.tokens, blankPlaceholder));
        }
      }
      break;
    }

    case 'text': {
      const textToken = token as Tokens.Text;
      // Handle text that might contain blank line placeholders or line breaks
      let text = textToken.raw || textToken.text || '';

      // Process inline tokens if present (text tokens can have nested formatting)
      if ('tokens' in textToken && textToken.tokens) {
        nodes.push(...processInlineTokens(textToken.tokens, blankPlaceholder));
      } else {
        nodes.push(...processTextWithBreaks(text, blankPlaceholder));
      }
      break;
    }

    case 'space': {
      // Space tokens represent blank lines in marked
      nodes.push({ type: 'br' });
      break;
    }

    case 'heading': {
      const heading = token as Tokens.Heading;
      if (blockElements) {
        // Render as proper heading element (h1-h6)
        const children = heading.tokens
          ? processInlineTokens(heading.tokens, blankPlaceholder)
          : [{ type: 'text' as const, content: heading.text }];
        nodes.push({ type: 'heading', level: heading.depth, children });
      } else {
        // Preserve header syntax as plain text (NodeSpace handles headers separately)
        const level = '#'.repeat(heading.depth);
        nodes.push({ type: 'text', content: level + ' ' });
        if (heading.tokens) {
          nodes.push(...processInlineTokens(heading.tokens, blankPlaceholder));
        }
      }
      break;
    }

    case 'list': {
      const list = token as Tokens.List;
      if (blockElements) {
        // Render as proper list element
        const items: ViewNode[][] = [];
        for (const item of list.items) {
          const itemNodes: ViewNode[] = [];
          // Add task checkbox if present
          if (item.task) {
            const checkbox = item.checked ? '☑ ' : '☐ ';
            itemNodes.push({ type: 'text', content: checkbox });
          }
          if (item.tokens) {
            itemNodes.push(...processTokens(item.tokens, blankPlaceholder, blockElements));
          }
          items.push(itemNodes);
        }
        nodes.push({ type: 'list', ordered: list.ordered, items });
      } else {
        // Preserve list syntax as plain text
        for (let i = 0; i < list.items.length; i++) {
          const item = list.items[i];
          if (i > 0) {
            nodes.push({ type: 'br' });
          }

          // Add list marker
          let marker = '';
          if (list.ordered) {
            const itemNumber = (list.start || 1) + i;
            marker = `${itemNumber}. `;
          } else {
            marker = '- ';
          }

          // Add task checkbox if present
          if (item.task) {
            marker += item.checked ? '[x] ' : '[ ] ';
          }

          nodes.push({ type: 'text', content: marker });

          if (item.tokens) {
            nodes.push(...processTokens(item.tokens, blankPlaceholder, blockElements));
          }
        }
      }
      break;
    }

    case 'code': {
      // Fenced code block: render literally. A `[[id]]` inside a fence is not a
      // wikilink, so emit the raw text (fence markers included, as before) split
      // only on line breaks — NOT through the ref splitter. Restore the blank-line
      // sentinel the pre-lex step injected across the whole content (line breaks in
      // fenced code are real, not a wikilink concern) before splitting.
      const raw = ((token as Tokens.Code).raw ?? '').split(blankPlaceholder).join('\n');
      raw.split('\n').forEach((line, i) => {
        if (i > 0) nodes.push({ type: 'br' });
        if (line.length > 0) nodes.push({ type: 'text', content: line });
      });
      break;
    }

    default:
      // For any other token type, try to extract raw text
      if ('raw' in token && typeof token.raw === 'string') {
        nodes.push(...processTextWithBreaks(token.raw, blankPlaceholder));
      } else if ('text' in token && typeof token.text === 'string') {
        nodes.push(...processTextWithBreaks(token.text, blankPlaceholder));
      }
  }

  return nodes;
}

/**
 * Process multiple tokens
 */
export function processTokens(tokens: Token[], blankPlaceholder: string, blockElements: boolean = false): ViewNode[] {
  const nodes: ViewNode[] = [];
  for (const token of tokens) {
    nodes.push(...processToken(token, blankPlaceholder, blockElements));
  }
  return nodes;
}

/**
 * Process inline tokens (strong, em, codespan, etc.)
 */
export function processInlineTokens(tokens: Token[], blankPlaceholder: string): ViewNode[] {
  const nodes: ViewNode[] = [];

  for (const token of tokens) {
    switch (token.type) {
      case 'strong': {
        const strong = token as Tokens.Strong;
        const children = strong.tokens
          ? processInlineTokens(strong.tokens, blankPlaceholder)
          : [{ type: 'text' as const, content: strong.text }];

        // Check if children contain italic - if so, use bold-italic
        const hasItalic = children.some(c => c.type === 'italic');
        if (hasItalic) {
          // Flatten and re-wrap as bold-italic
          nodes.push({ type: 'bold-italic', children: flattenToText(children) });
        } else {
          nodes.push({ type: 'bold', children });
        }
        break;
      }

      case 'em': {
        const em = token as Tokens.Em;
        const children = em.tokens
          ? processInlineTokens(em.tokens, blankPlaceholder)
          : [{ type: 'text' as const, content: em.text }];

        // Check if children contain bold - if so, use bold-italic
        const hasBold = children.some(c => c.type === 'bold');
        if (hasBold) {
          nodes.push({ type: 'bold-italic', children: flattenToText(children) });
        } else {
          nodes.push({ type: 'italic', children });
        }
        break;
      }

      case 'codespan': {
        const code = token as Tokens.Codespan;
        nodes.push({ type: 'code', content: code.text });
        break;
      }

      case 'del': {
        const del = token as Tokens.Del;
        const children = del.tokens
          ? processInlineTokens(del.tokens, blankPlaceholder)
          : [{ type: 'text' as const, content: del.text }];
        nodes.push({ type: 'strikethrough', children });
        break;
      }

      case 'text': {
        const textToken = token as Tokens.Text;
        const text = textToken.raw || textToken.text || '';
        nodes.push(...processTextWithBreaks(text, blankPlaceholder));
        break;
      }

      case 'br': {
        nodes.push({ type: 'br' });
        break;
      }

      case 'link': {
        // Render as actual link node to preserve nodespace:// URIs and other links
        const link = token as Tokens.Link;
        const children = link.tokens
          ? processInlineTokens(link.tokens, blankPlaceholder)
          : [{ type: 'text' as const, content: link.text }];

        if (!isSafeLinkHref(link.href)) {
          // Unsafe scheme (javascript:, data:, ...): show the label only.
          nodes.push(...children);
          break;
        }
        nodes.push({
          type: 'link',
          href: link.href,
          children
        });
        break;
      }

      default:
        // Fallback for other inline content
        if ('text' in token && typeof token.text === 'string') {
          nodes.push(...processTextWithBreaks(token.text, blankPlaceholder));
        } else if ('raw' in token && typeof token.raw === 'string') {
          nodes.push(...processTextWithBreaks(token.raw, blankPlaceholder));
        }
    }
  }

  return nodes;
}

/**
 * Flatten nested nodes to just text nodes (for bold-italic combination)
 */
export function flattenToText(nodes: ViewNode[]): ViewNode[] {
  const result: ViewNode[] = [];
  for (const node of nodes) {
    if (
      node.type === 'text' ||
      node.type === 'br' ||
      node.type === 'code' ||
      node.type === 'strikethrough' ||
      node.type === 'noderef'
    ) {
      result.push(node);
    } else if ('children' in node) {
      result.push(...flattenToText(node.children));
    }
  }
  return result;
}

/**
 * Process text that may contain line breaks and blank line placeholders
 */
export function processTextWithBreaks(text: string, blankPlaceholder: string): ViewNode[] {
  const nodes: ViewNode[] = [];

  // First handle blank line placeholders
  const parts = text.split(blankPlaceholder);

  for (let i = 0; i < parts.length; i++) {
    if (i > 0) {
      // Each placeholder = one blank line = one <br>
      nodes.push({ type: 'br' });
    }

    const part = parts[i];
    if (part) {
      // Split by actual newlines (which marked converts to softbreaks)
      const lines = part.split('\n');
      for (let j = 0; j < lines.length; j++) {
        if (j > 0) {
          nodes.push({ type: 'br' });
        }
        if (lines[j]) {
          nodes.push(...textRunToNodes(lines[j]));
        }
      }
    }
  }

  return nodes;
}


