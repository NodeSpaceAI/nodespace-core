<script context="module" lang="ts">
  // Icon name type (expandable for future icons)
  export type IconName = 'text';
</script>

<script lang="ts">
  import { textIcon } from './ui/text.js';

  // Icon registry mapping names to SVG paths
  const iconRegistry: Record<IconName, string> = {
    text: textIcon
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
  - Uses Material Design icon specifications (viewBox "0 -960 960 960")
  - Supports currentColor for theme integration
  - Default size: 20px (can be overridden)
  - Accessible with proper ARIA labeling
-->
<svg
  width={size}
  height={size}
  viewBox="0 -960 960 960"
  fill={color}
  class={`ns-icon ns-icon--${name} ${className}`}
  role="img"
  aria-label={`${name} icon`}
>
  {#if iconPath}
    <path d={iconPath} />
  {:else}
    <!-- Fallback for missing icons - simple square -->
    <rect x="200" y="-760" width="560" height="560" rx="40" />
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
