<!--
  Circle Icon Component
  
  Clean SVG component for text and document nodes with optional ring effect for parent nodes.
  Replaces the complex conditional logic in the old icon component.
  
  Features:
  - Simple filled circle for nodes without children
  - Ring effect for parent nodes (hasChildren=true)
  - Consistent 16x16 viewBox
  - Performance optimized (no HTML injection)
-->

<script lang="ts">
  export let size: number = 20;
  export let color: string = 'currentColor';
  export let hasChildren: boolean = false;
  export let className: string = '';
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 16 16"
  fill={color}
  class={`circle-icon ${className}`}
  role="img"
  aria-label="Node icon"
>
  {#if hasChildren}
    <!-- Parent node with ring effect: 16px background ring + 11px inner circle -->
    <!-- Background ring: consistent blue-gray color for all parent nodes -->
    <circle cx="8" cy="8" r="8" fill="hsl(200 40% 45%)" opacity="0.3" />
    <!-- Inner circle: consistent blue-gray color -->
    <circle cx="8" cy="8" r="5.5" fill="hsl(200 40% 45%)" />
  {:else}
    <!-- Simple child/leaf node: single filled circle -->
    <!-- Transparent background ring maintains consistent sizing -->
    <circle cx="8" cy="8" r="8" fill="hsl(200 40% 45%)" opacity="0" />
    <!-- Main circle: consistent blue-gray color -->
    <circle cx="8" cy="8" r="5.5" fill="hsl(200 40% 45%)" />
  {/if}
</svg>

<style>
  .circle-icon {
    display: block;
    flex-shrink: 0;
  }
</style>
