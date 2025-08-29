<script context="module" lang="ts">
  // Re-export types from separate file for better TypeScript support
  export type { IconName } from './types.js';
</script>

<script lang="ts">
  import { textIcon } from './ui/text';
  import { circleIcon } from './ui/circle';
  import { circleRingIcon } from './ui/circleRing';
  import { chevronRightIcon } from './ui/chevronRight';
  import type { IconName } from './types.js';

  // Icon registry mapping names to SVG paths
  const iconRegistry: Record<IconName, string> = {
    text: textIcon,
    circle: circleIcon,
    circleRing: circleRingIcon,
    chevronRight: chevronRightIcon
  };

  // Component props
  export let name: IconName = 'text';
  export let size: number = 20;
  export let className: string = '';
  export let color: string = 'currentColor';

  // Get the SVG path for the specified icon
  $: iconPath = iconRegistry[name];

  // Validate that the icon exists
  $: if (!iconPath) {
    console.warn(`Icon "${name}" not found in registry`);
  }
</script>

<!-- 
  SVG Icon Component
  - Uses standard SVG viewBox coordinate system (viewBox "0 -960 960 960")
  - Supports currentColor for shadcn-svelte theme integration
  - Default size: 20px (can be overridden)
  - Accessible with proper ARIA labeling
-->
<svg
  width={size}
  height={size}
  viewBox={name === 'circle' || name === 'circleRing' || name === 'chevronRight'
    ? '0 0 16 16'
    : '0 -960 960 960'}
  fill={color}
  class={`ns-icon ns-icon--${name} ${className}`}
  role="img"
  aria-label={`${name} icon`}
>
  {#if iconPath}
    {#if name === 'circle'}
      <!-- Simple filled circle: both circles always present, ring transparent -->
      <!-- Background ring: 16px diameter (transparent when no children) -->
      <circle cx="8" cy="8" r="8" fill={color} opacity="0" />
      <!-- Inner circle: 11px diameter (r=5.5) -->
      <circle cx="8" cy="8" r="5.5" fill={color} />
    {:else if name === 'circleRing'}
      <!-- Layered circles: 16px parent ring + 11px inner circle -->
      <!-- Background ring: 16px diameter filled area -->
      <circle cx="8" cy="8" r="8" fill={color} opacity="0.5" />
      <!-- Inner circle: 11px diameter (r=5.5) -->
      <circle cx="8" cy="8" r="5.5" fill={color} />
    {:else if name === 'chevronRight'}
      <!-- Chevron right: 16x16 viewBox, points right by default -->
      <path d={iconPath} fill={color} />
    {:else}
      <path d={iconPath} />
    {/if}
  {/if}
</svg>

<style>
  .ns-icon {
    display: inline-block;
    flex-shrink: 0;
    vertical-align: middle;
  }

  /* Icon-specific styling can be added here for each icon type */
</style>
