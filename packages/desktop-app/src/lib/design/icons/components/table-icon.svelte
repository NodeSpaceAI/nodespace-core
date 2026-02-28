<!--
  TableIcon - Grid/table icon for table nodes

  CSS-based approach matching design-system-icons.css exactly
  Uses background SVG for 2x2 grid overlay
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
  class="ns-icon table-icon {className}"
  style="--icon-size: {size}px"
  role="img"
  aria-label="Table icon"
>
  <div class="table-circle"></div>
</div>

<style>
  .ns-icon {
    width: var(--icon-size, 20px);
    height: var(--icon-size, 20px);
    position: relative;
    display: block;
    flex-shrink: 0;
  }

  /* Table circle - 16px circle at (2,2) position in 20px container */
  .table-circle {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    position: absolute;
    top: 2px;
    left: 2px;
    background: hsl(var(--node-text));
  }

  /* 2x2 grid overlay */
  .table-circle::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    width: 16px;
    height: 16px;
    transform: translate(-50%, -50%);
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16' viewBox='0 0 16 16'%3E%3Crect x='3' y='4' width='4' height='3' rx='0.5' fill='%23fcfcfc'/%3E%3Crect x='9' y='4' width='4' height='3' rx='0.5' fill='%23fcfcfc'/%3E%3Crect x='3' y='9' width='4' height='3' rx='0.5' fill='%23fcfcfc'/%3E%3Crect x='9' y='9' width='4' height='3' rx='0.5' fill='%23fcfcfc'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: center;
    background-size: 16px 16px;
  }

  /* Dark theme */
  :global(.dark) .table-circle::after {
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16' viewBox='0 0 16 16'%3E%3Crect x='3' y='4' width='4' height='3' rx='0.5' fill='%23252523'/%3E%3Crect x='9' y='4' width='4' height='3' rx='0.5' fill='%23252523'/%3E%3Crect x='3' y='9' width='4' height='3' rx='0.5' fill='%23252523'/%3E%3Crect x='9' y='9' width='4' height='3' rx='0.5' fill='%23252523'/%3E%3C/svg%3E");
  }
</style>
