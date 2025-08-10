<!--
  MockTextElement Component
  
  Hidden div that mirrors textarea with character-level spans for precise cursor positioning.
  Used by BaseNode to map click coordinates to character positions.
-->

<script lang="ts">
  import { onMount, afterUpdate } from 'svelte';

  // Props match textarea styling for accurate mirroring
  export let content = '';
  export let fontFamily = '';
  export let fontSize = '';
  export let fontWeight = '';
  export let lineHeight = '';
  export let width = 0;
  export let multiline = false;

  // Element reference for external access
  let mockElement: HTMLDivElement;

  // Expose the element reference for position calculations
  export function getElement(): HTMLDivElement | undefined {
    return mockElement;
  }

  // Split content into characters with support for grapheme clusters
  // This handles Unicode properly including emojis and accented characters
  $: characters = Array.from(content).map((char, idx) => ({ char, idx }));

  // Update positioning on content or style changes
  let lastContent = '';
  let lastWidth = 0;
  
  afterUpdate(() => {
    if (content !== lastContent || width !== lastWidth) {
      lastContent = content;
      lastWidth = width;
    }
  });

  // Expose mock element for external positioning calculations
  export { mockElement };
</script>

<div 
  bind:this={mockElement}
  class="mock-text-element"
  style="
    font-family: {fontFamily};
    font-size: {fontSize};
    font-weight: {fontWeight};
    line-height: {lineHeight};
    width: {width}px;
    position: absolute;
    top: -9999px;
    left: -9999px;
    visibility: hidden;
    white-space: {multiline ? 'pre-wrap' : 'nowrap'};
    word-break: break-word;
    overflow-wrap: break-word;
    box-sizing: border-box;
    padding: 0;
    margin: 0;
    border: none;
    background: transparent;
  "
  aria-hidden="true"
>
  {#each characters as { char, idx }}
    <span 
      id="mock-char-{idx}" 
      data-position="{idx}"
      class="mock-char"
    >
      {#if char === '\n'}
        <br />
      {:else if char === ' '}
        <!-- Preserve spaces with non-breaking space for accurate measurements -->
        &nbsp;
      {:else}
        {char}
      {/if}
    </span>
  {/each}
</div>

<style>
  .mock-text-element {
    /* Ensure exact mirroring of textarea */
    display: inline-block;
    min-height: 1em;
    overflow: hidden;
    /* Important: prevent any layout interference */
    pointer-events: none;
    user-select: none;
    z-index: -1;
  }

  .mock-char {
    /* Characters should have no additional styling */
    display: inline;
    margin: 0;
    padding: 0;
    border: none;
    background: transparent;
    /* Enable position measurements */
    position: relative;
  }
</style>