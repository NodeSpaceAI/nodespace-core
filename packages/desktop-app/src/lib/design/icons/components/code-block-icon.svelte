<!--
  CodeBlockIcon - Curly Braces Icon for Code Block Nodes

  CSS-based approach matching design-system-icons.css exactly
  Uses background SVG for curly braces overlay

  Design System Reference: docs/design-system/components.html → Code Block Nodes
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
  class="ns-icon code-block-icon {className}"
  class:code-block-icon-with-ring={hasChildren}
  style="--icon-size: {size}px"
  role="img"
  aria-label="Code block icon"
>
  {#if hasChildren}
    <div class="node-ring"></div>
  {/if}
  <div class="code-circle"></div>
</div>

<style>
  .ns-icon {
    width: var(--icon-size, 20px);
    height: var(--icon-size, 20px);
    position: relative;
    display: block;
    flex-shrink: 0;
  }

  .code-block-icon-with-ring {
    width: calc(var(--icon-size, 20px) + 4px);
    height: calc(var(--icon-size, 20px) + 4px);
  }

  /* Code circle - 16px circle at (2,2) position in 20px container */
  .code-circle {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    position: absolute;
    top: 2px;
    left: 2px;
    background: hsl(var(--node-text));
  }

  /* Curly braces overlay - exact SVG from design system */
  .code-circle::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    width: 16px;
    height: 16px;
    transform: translate(-50%, -50%);
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16' viewBox='0 0 20 20'%3E%3Cpath d='M4,6.25v1.5C4,8.16,3.66,8.5,3.25,8.5H2v3h1.25C3.66,11.5,4,11.84,4,12.25v1.5C4,14.99,5.01,16,6.25,16H8v-1.5H6.25 c-0.41,0-0.75-0.34-0.75-0.75v-1.5c0-1.16-0.88-2.11-2-2.24V9.99c1.12-0.12,2-1.08,2-2.24v-1.5c0-0.41,0.34-0.75,0.75-0.75H8V4 H6.25C5.01,4,4,5.01,4,6.25z' fill='%23fcfcfc'/%3E%3Cpath d='M16.75,8.5C16.34,8.5,16,8.16,16,7.75v-1.5C16,5.01,14.99,4,13.75,4H12v1.5h1.75c0.41,0,0.75,0.34,0.75,0.75v1.5 c0,1.16,0.88,2.11,2,2.24v0.03c-1.12,0.12-2,1.08-2,2.24v1.5c0,0.41-0.34,0.75-0.75,0.75H12V16h1.75c1.24,0,2.25-1.01,2.25-2.25 v-1.5c0-0.41,0.34-0.75,0.75-0.75H18v-3H16.75z' fill='%23fcfcfc'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: center;
    background-size: 16px 16px;
  }

  /* Dark theme - curly braces use dark background color */
  :global(.dark) .code-circle::after {
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16' viewBox='0 0 20 20'%3E%3Cpath d='M4,6.25v1.5C4,8.16,3.66,8.5,3.25,8.5H2v3h1.25C3.66,11.5,4,11.84,4,12.25v1.5C4,14.99,5.01,16,6.25,16H8v-1.5H6.25 c-0.41,0-0.75-0.34-0.75-0.75v-1.5c0-1.16-0.88-2.11-2-2.24V9.99c1.12-0.12,2-1.08,2-2.24v-1.5c0-0.41,0.34-0.75,0.75-0.75H8V4 H6.25C5.01,4,4,5.01,4,6.25z' fill='%23252523'/%3E%3Cpath d='M16.75,8.5C16.34,8.5,16,8.16,16,7.75v-1.5C16,5.01,14.99,4,13.75,4H12v1.5h1.75c0.41,0,0.75,0.34,0.75,0.75v1.5 c0,1.16,0.88,2.11,2,2.24v0.03c-1.12,0.12-2,1.08-2,2.24v1.5c0,0.41-0.34,0.75-0.75,0.75H12V16h1.75c1.24,0,2.25-1.01,2.25-2.25 v-1.5c0-0.41,0.34-0.75,0.75-0.75H18v-3H16.75z' fill='%23252523'/%3E%3C/svg%3E");
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
