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

  Block-level rendering (enableBlockElements: true):
  - Used by quote-blocks to render full markdown within quoted content
  - ## Heading → <h2 class="quote-heading">
  - - item → <ul><li> bullet list
  - Enables proper display of rich content in quote blocks
-->

<script lang="ts">
  import { marked } from 'marked';
  import type { Token, Tokens } from 'marked';

  // Props using Svelte 5 $props() rune
  interface Props {
    content: string;
    displayContent?: string | null;
    disableMarkdown?: boolean;
    enableBlockElements?: boolean; // Enable h2, bullet lists for quote-blocks
  }

  let { content, displayContent = null, disableMarkdown = false, enableBlockElements = false }: Props = $props();

  // Node types for structured rendering
  type ViewNode =
    | { type: 'text'; content: string }
    | { type: 'br' }
    | { type: 'bold'; children: ViewNode[] }
    | { type: 'italic'; children: ViewNode[] }
    | { type: 'strikethrough'; children: ViewNode[] }
    | { type: 'code'; content: string }
    | { type: 'bold-italic'; children: ViewNode[] }
    | { type: 'link'; href: string; children: ViewNode[] }
    // Block-level elements (enabled via enableBlockElements prop)
    | { type: 'heading'; level: number; children: ViewNode[] }
    | { type: 'list'; ordered: boolean; items: ViewNode[][] }
    | { type: 'paragraph'; children: ViewNode[] };

  /**
   * Parse content into ViewNode array for rendering
   */
  function parseContent(rawContent: string, markdown: boolean, blockElements: boolean): ViewNode[] {
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
  function processToken(token: Token, blankPlaceholder: string, blockElements: boolean = false): ViewNode[] {
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
  function processTokens(tokens: Token[], blankPlaceholder: string, blockElements: boolean = false): ViewNode[] {
    const nodes: ViewNode[] = [];
    for (const token of tokens) {
      nodes.push(...processToken(token, blankPlaceholder, blockElements));
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
          // Render as actual link node to preserve nodespace:// URIs and other links
          const link = token as Tokens.Link;
          const children = link.tokens
            ? processInlineTokens(link.tokens, blankPlaceholder)
            : [{ type: 'text' as const, content: link.text }];

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
    return parseContent(sourceContent, !disableMarkdown, enableBlockElements);
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
  {:else if node.type === 'link'}
    <a href={node.href} class="ns-noderef">{#each node.children as child}{@render renderNode(child)}{/each}</a>
  {:else if node.type === 'heading'}
    {#if node.level === 1}
      <h1 class="quote-heading">{#each node.children as child}{@render renderNode(child)}{/each}</h1>
    {:else if node.level === 2}
      <h2 class="quote-heading">{#each node.children as child}{@render renderNode(child)}{/each}</h2>
    {:else if node.level === 3}
      <h3 class="quote-heading">{#each node.children as child}{@render renderNode(child)}{/each}</h3>
    {:else if node.level === 4}
      <h4 class="quote-heading">{#each node.children as child}{@render renderNode(child)}{/each}</h4>
    {:else if node.level === 5}
      <h5 class="quote-heading">{#each node.children as child}{@render renderNode(child)}{/each}</h5>
    {:else}
      <h6 class="quote-heading">{#each node.children as child}{@render renderNode(child)}{/each}</h6>
    {/if}
  {:else if node.type === 'list'}
    {#if node.ordered}
      <ol class="quote-list">{#each node.items as item}<li>{#each item as child}{@render renderNode(child)}{/each}</li>{/each}</ol>
    {:else}
      <ul class="quote-list">{#each node.items as item}<li>{#each item as child}{@render renderNode(child)}{/each}</li>{/each}</ul>
    {/if}
  {:else if node.type === 'paragraph'}
    <p class="quote-paragraph">{#each node.children as child}{@render renderNode(child)}{/each}</p>
  {/if}
{/snippet}

<!-- Render all nodes -->
{#each viewNodes as node}
  {@render renderNode(node)}
{/each}

<style>
  /* Block-level element styles for quote blocks */

  /* Headings within quote blocks */
  .quote-heading {
    margin: 0.5em 0 0.25em 0;
    font-weight: 600;
    line-height: 1.3;
  }

  h1.quote-heading {
    font-size: 1.5em;
  }

  h2.quote-heading {
    font-size: 1.25em;
  }

  h3.quote-heading {
    font-size: 1.1em;
  }

  h4.quote-heading,
  h5.quote-heading,
  h6.quote-heading {
    font-size: 1em;
  }

  /* First heading in a quote block shouldn't have top margin */
  .quote-heading:first-child {
    margin-top: 0;
  }

  /* Lists within quote blocks */
  .quote-list {
    margin: 0.25em 0;
    padding-left: 1.5em;
  }

  .quote-list li {
    margin: 0.125em 0;
    line-height: 1.5;
  }

  /* Unordered list styling */
  ul.quote-list {
    list-style-type: disc;
  }

  /* Nested unordered lists use different markers */
  ul.quote-list ul.quote-list {
    list-style-type: circle;
  }

  /* Ordered list styling */
  ol.quote-list {
    list-style-type: decimal;
  }

  /* Paragraphs within quote blocks */
  .quote-paragraph {
    margin: 0.25em 0;
    line-height: 1.5;
  }

  /* First paragraph shouldn't have top margin */
  .quote-paragraph:first-child {
    margin-top: 0;
  }

  /* Last paragraph shouldn't have bottom margin */
  .quote-paragraph:last-child {
    margin-bottom: 0;
  }
</style>
