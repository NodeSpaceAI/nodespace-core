<script context="module" lang="ts">
  // Icon name type (expandable for future icons)
  export type IconName = 'text' | 'circle' | 'circle-ring' | 'chevron-right';
</script>

<script lang="ts">
  import { textIcon } from './ui/text.js';
  import { circleIcon } from './ui/circle.js';
  import { circleRingIcon } from './ui/circle-ring.js';
  import { chevronRightIcon } from './ui/chevron-right.js';

  // Icon registry mapping names to SVG paths
  const iconRegistry: Record<IconName, string> = {
    text: textIcon,
    circle: circleIcon,
    'circle-ring': circleRingIcon,
    'chevron-right': chevronRightIcon
  };

  // Component props
  export let name: IconName;
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
  viewBox="{name === 'circle' || name === 'circle-ring' || name === 'chevron-right' ? '0 0 16 16' : '0 -960 960 960'}"
  fill={color}
  class={`ns-icon ns-icon--${name} ${className}`}
  role="img"
  aria-label={`${name} icon`}
>
  {#if iconPath}
    {#if name === 'circle'}
      <!-- Simple filled circle: both circles always present, ring transparent -->
      <!-- Background ring: 16px diameter (transparent when no children) -->
      <circle cx="8" cy="8" r="7" fill={color} opacity="0" />
      <!-- Inner circle: 10px diameter -->
      <circle cx="8" cy="8" r="4" fill={color} />
    {:else if name === 'circle-ring'}
      <!-- Layered circles: 16px parent ring + 10px inner circle -->
      <!-- Background ring: 16px diameter filled area -->
      <circle cx="8" cy="8" r="7" fill={color} opacity="0.5" />
      <!-- Inner circle: 10px diameter (covers center, creating ring effect) -->
      <circle cx="8" cy="8" r="4" fill={color} />
    {:else if name === 'chevron-right'}
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
