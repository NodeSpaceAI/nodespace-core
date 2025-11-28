<!--
  ViewModeRenderer Component

  Renders markdown content for view mode WITHOUT using {@html}.
  Parses markdown into structured nodes and renders using Svelte components.

  This component produces identical visual output to markdownToHtml() but avoids
  the ESLint svelte/no-at-html-tags warning by using structured rendering.

  Supports:
  - Bold (**text** or __text__) → <span class="markdown-bold">
  - Italic (*text* or _text_) → <span class="markdown-italic">
  - Strikethrough (~~text~~) → <del>
  - Inline code (`code`) → <code class="markdown-code-inline">
  - Line breaks (\n) → <br>
  - Blank lines (\n\n) → multiple <br>
  - Leading/trailing newlines preserved
  - disableMarkdown mode (raw text with line breaks only)
  - Header syntax preserved as text (# Header stays as text)
  - List syntax preserved as text (1. Item stays as text)
-->

<script lang="ts">
  import { marked } from 'marked';
  import type { Token, Tokens } from 'marked';

  // Props using Svelte 5 $props() rune
  interface Props {
    content: string;
    displayContent?: string | null;
    disableMarkdown?: boolean;
  }

  let { content, displayContent = null, disableMarkdown = false }: Props = $props();

  // Node types for structured rendering
  type ViewNode =
    | { type: 'text'; content: string }
    | { type: 'br' }
    | { type: 'bold'; children: ViewNode[] }
    | { type: 'italic'; children: ViewNode[] }
    | { type: 'strikethrough'; children: ViewNode[] }
    | { type: 'code'; content: string }
    | { type: 'bold-italic'; children: ViewNode[] };

  /**
   * Parse content into ViewNode array for rendering
   */
  function parseContent(rawContent: string, markdown: boolean): ViewNode[] {
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
      nodes.push(...processToken(token, BLANK_LINE_PLACEHOLDER));
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
   * Parse raw text (disableMarkdown mode)
   */
  function parseRawText(text: string): ViewNode[] {
    const lines = text.split('\n');
    const nodes: ViewNode[] = [];

    for (let i = 0; i < lines.length; i++) {
      if (i > 0) {
        nodes.push({ type: 'br' });
      }
      if (lines[i]) {
        nodes.push({ type: 'text', content: lines[i] });
      }
    }

    return nodes;
  }

  /**
   * Process a marked token into ViewNodes
   */
  function processToken(token: Token, blankPlaceholder: string): ViewNode[] {
    const nodes: ViewNode[] = [];

    switch (token.type) {
      case 'paragraph': {
        const para = token as Tokens.Paragraph;
        if (para.tokens) {
          nodes.push(...processInlineTokens(para.tokens, blankPlaceholder));
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
        // Preserve header syntax as plain text (NodeSpace handles headers separately)
        const heading = token as Tokens.Heading;
        const level = '#'.repeat(heading.depth);
        nodes.push({ type: 'text', content: level + ' ' });
        if (heading.tokens) {
          nodes.push(...processInlineTokens(heading.tokens, blankPlaceholder));
        }
        break;
      }

      case 'list': {
        // Preserve list syntax as plain text
        const list = token as Tokens.List;
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
            nodes.push(...processTokens(item.tokens, blankPlaceholder));
          }
        }
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
  function processTokens(tokens: Token[], blankPlaceholder: string): ViewNode[] {
    const nodes: ViewNode[] = [];
    for (const token of tokens) {
      nodes.push(...processToken(token, blankPlaceholder));
    }
    return nodes;
  }

  /**
   * Process inline tokens (strong, em, codespan, etc.)
   */
  function processInlineTokens(tokens: Token[], blankPlaceholder: string): ViewNode[] {
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
          // For links, just render the text content (links are handled elsewhere in NodeSpace)
          const link = token as Tokens.Link;
          if (link.tokens) {
            nodes.push(...processInlineTokens(link.tokens, blankPlaceholder));
          } else {
            nodes.push({ type: 'text', content: link.text });
          }
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
  function flattenToText(nodes: ViewNode[]): ViewNode[] {
    const result: ViewNode[] = [];
    for (const node of nodes) {
      if (node.type === 'text' || node.type === 'br' || node.type === 'code' || node.type === 'strikethrough') {
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
  function processTextWithBreaks(text: string, blankPlaceholder: string): ViewNode[] {
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
            nodes.push({ type: 'text', content: lines[j] });
          }
        }
      }
    }

    return nodes;
  }

  // Compute nodes from content using $derived
  let viewNodes = $derived.by(() => {
    const sourceContent = displayContent ?? content;
    return parseContent(sourceContent, !disableMarkdown);
  });
</script>

<!-- Recursive node renderer -->
{#snippet renderNode(node: ViewNode)}
  {#if node.type === 'text'}
    {node.content}
  {:else if node.type === 'br'}
    <br>
  {:else if node.type === 'bold'}
    <span class="markdown-bold">{#each node.children as child}{@render renderNode(child)}{/each}</span>
  {:else if node.type === 'italic'}
    <span class="markdown-italic">{#each node.children as child}{@render renderNode(child)}{/each}</span>
  {:else if node.type === 'strikethrough'}
    <del>{#each node.children as child}{@render renderNode(child)}{/each}</del>
  {:else if node.type === 'bold-italic'}
    <span class="markdown-bold markdown-italic">{#each node.children as child}{@render renderNode(child)}{/each}</span>
  {:else if node.type === 'code'}
    <code class="markdown-code-inline">{node.content}</code>
  {/if}
{/snippet}

<!-- Render all nodes -->
{#each viewNodes as node}
  {@render renderNode(node)}
{/each}
