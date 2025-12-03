<!--
  Query Icon Component

  SVG-based query icon with filter symbol inside a circle.
  Follows design system patterns for node type icons.

  Features:
  - Circle with filter/funnel symbol
  - Theme-aware colors
  - Consistent 20x20px sizing
  - No state variations (queries don't have states like tasks)
-->

<script lang="ts">
	let { size = 20, className = '' }: { size?: number; className?: string } = $props();
</script>

<svg
	class="query-icon {className}"
	width={size}
	height={size}
	viewBox="0 0 20 20"
	fill="none"
	xmlns="http://www.w3.org/2000/svg"
	role="img"
	aria-label="Query icon"
>
	<!-- Outer circle -->
	<circle
		cx="10"
		cy="10"
		r="8"
		stroke="currentColor"
		stroke-width="1.5"
		fill="none"
	/>

	<!-- Filter/funnel symbol -->
	<path
		d="M6 6.5 L8.5 9.5 L8.5 12 L11.5 13.5 L11.5 9.5 L14 6.5 Z"
		stroke="currentColor"
		stroke-width="1.2"
		stroke-linecap="round"
		stroke-linejoin="round"
		fill="none"
	/>
</svg>

<style>
	.query-icon {
		display: block;
		flex-shrink: 0;
		color: hsl(var(--primary));
	}
</style>
