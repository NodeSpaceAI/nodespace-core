<!--
  MarkdownRenderer Component
  
  Safely renders markdown content without using {@html}.
  Parses markdown into structured data and renders using Svelte components.
-->

<script lang="ts">
  interface MarkdownNode {
    type: 'paragraph' | 'heading' | 'text' | 'bold' | 'italic' | 'code' | 'link' | 'br';
    content?: string;
    children?: MarkdownNode[];
    level?: number; // for headings
    href?: string; // for links
    className?: string;
  }

  // Props
  export let content: string = '';
  export let nodes: MarkdownNode[] = [];

  /**
   * Parse markdown content into structured nodes without HTML
   */
  function parseMarkdownSafe(markdown: string): MarkdownNode[] {
    if (!markdown) return [];

    // Split by double newlines for paragraphs
    const paragraphs = markdown.split(/\n\s*\n/);
    const result: MarkdownNode[] = [];

    for (const para of paragraphs) {
      if (!para.trim()) continue;

      // Check if it's a heading
      const headingMatch = para.match(/^(#{1,6})\s+(.+)$/);
      if (headingMatch) {
        const level = headingMatch[1].length;
        const headingContent = headingMatch[2].trim();
        result.push({
          type: 'heading',
          level,
          children: parseInlineContent(headingContent),
          className: `ns-markdown-heading ns-markdown-h${level}`
        });
      } else {
        // Regular paragraph - convert single newlines to <br>
        const paragraphContent = para.replace(/\n/g, '\n'); // Keep newlines for processing
        result.push({
          type: 'paragraph',
          children: parseInlineContent(paragraphContent),
          className: 'ns-markdown-paragraph'
        });
      }
    }

    return result;
  }

  /**
   * Parse inline content (bold, italic, code, links, line breaks)
   */
  function parseInlineContent(text: string): MarkdownNode[] {
    const result: MarkdownNode[] = [];
    let remaining = text;

    while (remaining.length > 0) {
      // Look for line breaks first
      const brMatch = remaining.match(/^(.+?)\n/);
      if (brMatch) {
        const beforeBr = brMatch[1];
        if (beforeBr) {
          result.push(...parseFormattingNodes(beforeBr));
        }
        result.push({ type: 'br' });
        remaining = remaining.slice(brMatch[0].length);
        continue;
      }

      // No more line breaks, process the rest
      result.push(...parseFormattingNodes(remaining));
      break;
    }

    return result;
  }

  /**
   * Parse formatting (bold, italic, code, links) from text
   */
  function parseFormattingNodes(text: string): MarkdownNode[] {
    const result: MarkdownNode[] = [];
    let remaining = text;

    while (remaining.length > 0) {
      // Look for bold (**text** or __text__)
      const boldMatch =
        remaining.match(/^(.*?)\*\*(.*?)\*\*(.*)/s) || remaining.match(/^(.*?)__(.*?)__(.*)/s);

      if (boldMatch) {
        const [, before, boldText, after] = boldMatch;
        if (before) result.push({ type: 'text', content: before });
        result.push({
          type: 'bold',
          children: [{ type: 'text', content: boldText }],
          className: 'ns-markdown-bold'
        });
        remaining = after;
        continue;
      }

      // Look for italic (*text* or _text_)
      const italicMatch =
        remaining.match(/^(.*?)(?<!\*)\*(?!\*)([^*]+)\*(?!\*)(.*)/s) ||
        remaining.match(/^(.*?)(?<!_)_(?!_)([^_]+)_(?!_)(.*)/s);

      if (italicMatch) {
        const [, before, italicText, after] = italicMatch;
        if (before) result.push({ type: 'text', content: before });
        result.push({
          type: 'italic',
          children: [{ type: 'text', content: italicText }],
          className: 'ns-markdown-italic'
        });
        remaining = after;
        continue;
      }

      // Look for inline code (`code`)
      const codeMatch = remaining.match(/^(.*?)`([^`]+)`(.*)/s);
      if (codeMatch) {
        const [, before, codeText, after] = codeMatch;
        if (before) result.push({ type: 'text', content: before });
        result.push({
          type: 'code',
          content: codeText,
          className: 'ns-markdown-code'
        });
        remaining = after;
        continue;
      }

      // Look for links [text](url)
      const linkMatch = remaining.match(/^(.*?)\[([^\]]+)\]\(([^)]+)\)(.*)/s);
      if (linkMatch) {
        const [, before, linkText, url, after] = linkMatch;
        if (before) result.push({ type: 'text', content: before });
        result.push({
          type: 'link',
          href: url,
          children: [{ type: 'text', content: linkText }],
          className: 'ns-markdown-link'
        });
        remaining = after;
        continue;
      }

      // No more formatting, add remaining as text
      result.push({ type: 'text', content: remaining });
      break;
    }

    return result;
  }

  // Use nodes directly if provided, otherwise parse content
  $: finalNodes = nodes.length > 0 ? nodes : parseMarkdownSafe(content);
</script>

<!-- Render markdown nodes -->
{#each finalNodes as node}
  {#if node.type === 'paragraph'}
    <p class={node.className || ''}>
      <svelte:self nodes={node.children || []} />
    </p>
  {:else if node.type === 'heading'}
    {#if node.level === 1}
      <h1 class={node.className || ''}><svelte:self nodes={node.children || []} /></h1>
    {:else if node.level === 2}
      <h2 class={node.className || ''}><svelte:self nodes={node.children || []} /></h2>
    {:else if node.level === 3}
      <h3 class={node.className || ''}><svelte:self nodes={node.children || []} /></h3>
    {:else if node.level === 4}
      <h4 class={node.className || ''}><svelte:self nodes={node.children || []} /></h4>
    {:else if node.level === 5}
      <h5 class={node.className || ''}><svelte:self nodes={node.children || []} /></h5>
    {:else if node.level === 6}
      <h6 class={node.className || ''}><svelte:self nodes={node.children || []} /></h6>
    {/if}
  {:else if node.type === 'bold'}
    <strong class={node.className || ''}><svelte:self nodes={node.children || []} /></strong>
  {:else if node.type === 'italic'}
    <em class={node.className || ''}><svelte:self nodes={node.children || []} /></em>
  {:else if node.type === 'code'}
    <code class={node.className || ''}>{node.content || ''}</code>
  {:else if node.type === 'link'}
    <a
      href={node.href || ''}
      class={node.className || ''}
      target="_blank"
      rel="noopener noreferrer"
    >
      <svelte:self nodes={node.children || []} />
    </a>
  {:else if node.type === 'br'}
    <br />
  {:else if node.type === 'text'}
    {node.content || ''}
  {/if}
{/each}
