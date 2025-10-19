<!--
  OrderedListIcon - Numbered List Icon for Ordered List Nodes

  CSS-based approach with numbered list overlay icon
  Displays Material Design numbered list icon on circular background

  Design System Reference: docs/design-system/patterns/foundations/icon-systems.html → OrderedListNode Icon System
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
  class="ns-icon ordered-list-icon {className}"
  class:ordered-list-icon-with-ring={hasChildren}
  style="--icon-size: {size}px"
  role="img"
  aria-label="Ordered list icon"
>
  {#if hasChildren}
    <div class="node-ring"></div>
  {/if}
  <div class="list-circle"></div>
</div>

<style>
  .ns-icon {
    width: var(--icon-size, 20px);
    height: var(--icon-size, 20px);
    position: relative;
    display: block;
    flex-shrink: 0;
  }

  .ordered-list-icon-with-ring {
    width: calc(var(--icon-size, 20px) + 4px);
    height: calc(var(--icon-size, 20px) + 4px);
  }

  /* Ordered list circle - 16px circle at (2,2) position in 20px container */
  .list-circle {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    position: absolute;
    top: 2px;
    left: 2px;
    background: hsl(var(--node-text));
  }

  /* Numbered list overlay - Custom SVG with shorter horizontal lines and better spacing
     NOTE: This is the actual implementation from design-system-icons.css, not the Material Design original
     Custom modifications: numbers shifted right (x=5 vs x=2), shorter lines (h8 vs h14) */
  .list-circle::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    width: 16px;
    height: 16px;
    transform: translate(-50%, -50%);
    background-image: var(--ordered-list-icon);
    background-repeat: no-repeat;
    background-position: center;
    background-size: 16px 16px;
  }

  /* Light theme - white icon color */
  .list-circle {
    --ordered-list-icon: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' height='16px' viewBox='0 0 24 24' width='16px' fill='%23fcfcfc'%3E%3Cpath d='M0 0h24v24H0V0z' fill='none'/%3E%3Cpath d='M5 17h2v.5H6v1h1v.5H5v1h3v-4H5v1zm1-9h1V4H5v1h1v3zm-1 3h1.8L5 13.1v.9h3v-1H6.2L8 10.9V10H5v1zm5-6v1h8V5h-8zm0 14h8v-1h-8v1zm0-6h8v-1h-8v1z'/%3E%3C/svg%3E");
  }

  /* Dark theme - dark icon color */
  :global(.dark) .list-circle {
    --ordered-list-icon: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' height='16px' viewBox='0 0 24 24' width='16px' fill='%23252523'%3E%3Cpath d='M0 0h24v24H0V0z' fill='none'/%3E%3Cpath d='M5 17h2v.5H6v1h1v.5H5v1h3v-4H5v1zm1-9h1V4H5v1h1v3zm-1 3h1.8L5 13.1v.9h3v-1H6.2L8 10.9V10H5v1zm5-6v1h8V5h-8zm0 14h8v-1h-8v1zm0-6h8v-1h-8v1z'/%3E%3C/svg%3E");
  }

  /* Parent node ring - NOT used for ordered-list since canHaveChildren: false */
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
