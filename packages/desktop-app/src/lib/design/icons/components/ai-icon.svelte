<!--
  AI Icon Component
  
  Specialized icon for AI-related nodes with square design and "AI" text.
  Provides clean, type-safe alternative to HTML injection for AI icons.
  
  Features:
  - Rounded square design consistent with design system
  - "AI" text with proper font rendering
  - No HTML injection security risks
  - Consistent 16x16 viewBox
  - Semantic AI-specific styling
-->

<script lang="ts">
  export let size: number = 20;
  export let color: string = 'currentColor';
  export let className: string = '';
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 16 16"
  fill={color}
  class={`ai-icon ${className}`}
  role="img"
  aria-label="AI node icon"
>
  <!-- Rounded square background with subtle fill and stroke -->
  <rect
    x="2"
    y="2"
    width="12"
    height="12"
    rx="2"
    ry="2"
    fill={color}
    opacity="0.1"
    stroke={color}
    stroke-width="1"
  />

  <!-- "AI" text centered in square -->
  <text
    x="8"
    y="10.5"
    font-family="system-ui, -apple-system, sans-serif"
    font-size="6"
    font-weight="600"
    text-anchor="middle"
    fill={color}
  >
    AI
  </text>
</svg>

<style>
  .ai-icon {
    display: inline-block;
    flex-shrink: 0;
    vertical-align: middle;
  }

  /* AI-specific styling */
  .ai-icon text {
    /* Ensure text renders consistently across browsers */
    user-select: none;
    pointer-events: none;
  }
</style>
