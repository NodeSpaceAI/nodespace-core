<!--
  ViewModeRenderer Component

  Renders markdown content for view mode WITHOUT using {@html}.
  Parses markdown into structured nodes and renders using Svelte components.

  This component produces identical visual output to raw marked/HTML rendering but
  avoids the ESLint svelte/no-at-html-tags warning by using structured rendering.

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
  import { parseContent, type ViewNode } from './view-mode-parser';
  import NodeRefInline from './node-ref-inline.svelte';
  import { isSafeLinkHref } from '$lib/utils/safe-link';

  // Props using Svelte 5 $props() rune
  interface Props {
    content: string;
    displayContent?: string | null;
    disableMarkdown?: boolean;
    enableBlockElements?: boolean; // Enable h2, bullet lists for quote-blocks
  }

  let { content, displayContent = null, disableMarkdown = false, enableBlockElements = false }: Props = $props();


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
  {:else if node.type === 'link' && isSafeLinkHref(node.href)}
    <a href={node.href} class="ns-noderef">{#each node.children as child}{@render renderNode(child)}{/each}</a>
  {:else if node.type === 'link'}
    {#each node.children as child}{@render renderNode(child)}{/each}
  {:else if node.type === 'noderef'}
    <NodeRefInline id={node.id} />
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
