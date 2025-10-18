<!--
  QuoteBlockIcon - Quotation Mark Icon for Quote Block Nodes

  CSS-based approach matching design-system-icons.css exactly
  Uses background SVG for quotation mark overlay

  Design System Reference: docs/design-system/components.html → Quote Block Nodes
-->

<script lang="ts">
  let {
    size = 20,
    hasChildren = false,
    className = ''
  }: {
    size?: number;
    hasChildren?: boolean;
    className?: string;
  } = $props();
</script>

<div
  class="ns-icon quote-block-icon {className}"
  class:quote-block-icon-with-ring={hasChildren}
  style="--icon-size: {size}px"
  role="img"
  aria-label="Quote block icon"
>
  {#if hasChildren}
    <div class="node-ring"></div>
  {/if}
  <div class="quote-circle"></div>
</div>

<style>
  .ns-icon {
    width: var(--icon-size, 20px);
    height: var(--icon-size, 20px);
    position: relative;
    display: block;
    flex-shrink: 0;
  }

  .quote-block-icon-with-ring {
    width: calc(var(--icon-size, 20px) + 4px);
    height: calc(var(--icon-size, 20px) + 4px);
  }

  /* Quote circle - 16px circle at (2,2) position in 20px container */
  .quote-circle {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    position: absolute;
    top: 2px;
    left: 2px;
    background: hsl(var(--node-text));
  }

  /* Quotation mark overlay - exact SVG from design system */
  .quote-circle::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    width: 16px;
    height: 16px;
    transform: translate(-50%, -50%);
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' height='16px' viewBox='0 0 24 24' width='16px'%3E%3Cpath d='M6 17h3l2-4V7H5v6h3zm8 0h3l2-4V7h-6v6h3z' fill='%23fcfcfc'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: center;
    background-size: 16px 16px;
  }

  /* Dark theme - quotation mark uses dark background color */
  :global(.dark) .quote-circle::after {
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' height='16px' viewBox='0 0 24 24' width='16px'%3E%3Cpath d='M6 17h3l2-4V7H5v6h3zm8 0h3l2-4V7h-6v6h3z' fill='%231f1f1f'/%3E%3C/svg%3E");
  }

  /* Parent node ring */
  .node-ring {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    border: 2px solid hsl(var(--node-text) / 0.5);
    box-sizing: border-box;
    position: absolute;
    top: 0;
    left: 0;
  }
</style>
