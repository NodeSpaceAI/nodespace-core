<!--
  HorizontalLineIcon - Horizontal line icon for thematic break nodes

  CSS-based approach matching design-system-icons.css exactly
  Uses background SVG for horizontal line overlay
-->

<script lang="ts">
  let {
    size = 20,
    className = ''
  }: {
    size?: number;
    className?: string;
  } = $props();
</script>

<div
  class="ns-icon horizontal-line-icon {className}"
  style="--icon-size: {size}px"
  role="img"
  aria-label="Horizontal line icon"
>
  <div class="hr-circle"></div>
</div>

<style>
  .ns-icon {
    width: var(--icon-size, 20px);
    height: var(--icon-size, 20px);
    position: relative;
    display: block;
    flex-shrink: 0;
  }

  /* HR circle - 16px circle at (2,2) position in 20px container */
  .hr-circle {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    position: absolute;
    top: 2px;
    left: 2px;
    background: hsl(var(--node-text));
  }

  /* Horizontal line overlay - centered line */
  .hr-circle::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    width: 16px;
    height: 16px;
    transform: translate(-50%, -50%);
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16' viewBox='0 0 16 16'%3E%3Cline x1='3' y1='8' x2='13' y2='8' stroke='%23fcfcfc' stroke-width='2' stroke-linecap='round'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: center;
    background-size: 16px 16px;
  }

  /* Dark theme */
  :global(.dark) .hr-circle::after {
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16' viewBox='0 0 16 16'%3E%3Cline x1='3' y1='8' x2='13' y2='8' stroke='%23252523' stroke-width='2' stroke-linecap='round'/%3E%3C/svg%3E");
  }
</style>
