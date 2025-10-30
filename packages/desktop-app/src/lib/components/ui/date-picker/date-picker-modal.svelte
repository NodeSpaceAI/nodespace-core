<script lang="ts">
  import { onMount } from 'svelte';

  // Props (Svelte 5 runes syntax)
  let {
    position = { x: 0, y: 0 },
    visible = false,
    onselect,
    onclose
  }: {
    position?: { x: number; y: number };
    visible?: boolean;
    onselect?: (_date: Date) => void;
    onclose?: () => void;
  } = $props();

  // State
  let containerRef = $state<HTMLElement | undefined>(undefined);
  let dateInputElement = $state<HTMLElement | null>(null);

  // Initialize with today's date in YYYY-MM-DD format
  let selectedDate = $state(new Date().toISOString().split('T')[0]);

  // Smart positioning to avoid viewport edges (similar to node-autocomplete)
  function getSmartPosition(pos: { x: number; y: number }) {
    const padding = 16;
    const maxWidth = 320;
    const maxHeight = 400;
    const spacingFromCursor = 20;

    let { x, y } = pos;

    // Adjust horizontal position
    if (x + maxWidth > window.innerWidth - padding) {
      x = window.innerWidth - maxWidth - padding;
    }
    if (x < padding) {
      x = padding;
    }

    // Adjust vertical position (show above cursor if not enough space below)
    const showBelow = y + maxHeight + spacingFromCursor <= window.innerHeight - padding;

    if (showBelow) {
      y = y + spacingFromCursor;
    } else {
      y = y - spacingFromCursor;
    }

    return { x, y, showBelow };
  }

  // Store initial position to prevent movement
  let initialPosition: { x: number; y: number; showBelow: boolean } | null = null;

  // Smart position calculation - only calculate once when modal first appears
  $effect(() => {
    if (visible && !initialPosition) {
      initialPosition = getSmartPosition(position);
    } else if (!visible) {
      initialPosition = null;
    }
  });

  let smartPosition = $derived(initialPosition || getSmartPosition(position));

  // Handle date selection
  function handleDateSelect() {
    if (!selectedDate) return;

    // Parse YYYY-MM-DD string to Date
    const jsDate = new Date(selectedDate);
    onselect?.(jsDate);
  }

  // Keyboard navigation
  function handleKeydown(event: KeyboardEvent) {
    if (!visible) return;

    switch (event.key) {
      case 'Escape':
        event.preventDefault();
        event.stopPropagation();
        onclose?.();
        break;
      case 'Enter':
        event.preventDefault();
        event.stopPropagation();
        handleDateSelect();
        break;
    }
  }

  // Bind keyboard events
  onMount(() => {
    document.addEventListener('keydown', handleKeydown);
    return () => {
      document.removeEventListener('keydown', handleKeydown);
    };
  });
</script>

{#if visible}
  <div
    bind:this={containerRef}
    style="
      position: fixed;
      {smartPosition.showBelow
      ? `top: ${smartPosition.y}px;`
      : `bottom: ${window.innerHeight - smartPosition.y}px;`}
      left: {smartPosition.x}px;
      min-width: 280px;
      background: hsl(var(--popover));
      border: 1px solid hsl(var(--border));
      border-radius: var(--radius);
      z-index: 1000;
      padding: 1rem;
      box-shadow: 0 4px 6px -1px rgb(0 0 0 / 0.1), 0 2px 4px -2px rgb(0 0 0 / 0.1);
    "
    role="dialog"
    aria-label="Date picker"
  >
    <div
      style="margin-bottom: 0.75rem; font-weight: 600; color: hsl(var(--foreground)); text-align: center;"
    >
      Select a date
    </div>
    <input
      bind:this={dateInputElement}
      bind:value={selectedDate}
      type="date"
      onchange={handleDateSelect}
      style="
        width: 100%;
        padding: 0.5rem;
        border: 1px solid hsl(var(--border));
        border-radius: var(--radius);
        background: hsl(var(--background));
        color: hsl(var(--foreground));
        font-size: 1rem;
        cursor: pointer;
      "
    />
  </div>
{/if}
