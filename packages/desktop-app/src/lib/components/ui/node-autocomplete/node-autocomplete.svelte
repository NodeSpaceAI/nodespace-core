<script lang="ts">
  import Icon, { type NodeType } from '$lib/design/icons';
  import { onMount } from 'svelte';

  // Import icon configuration from registry for consistency
  import { getIconConfig } from '$lib/design/icons/registry';

  // Types for node results
  interface NodeResult {
    id: string;
    title: string;
    type: NodeType;
    subtitle?: string;
    metadata?: string;
  }

  // Props
  export let position: { x: number; y: number } = { x: 0, y: 0 };
  export let query: string = '';
  export let results: NodeResult[] = [];
  export let loading: boolean = false;
  export let visible: boolean = false;

  // Event handler props (Svelte 5 pattern)
  export let onselect: ((_result: NodeResult) => void) | undefined = undefined;
  export let onclose: (() => void) | undefined = undefined;

  // State
  let selectedIndex: number = 0;
  let containerRef: HTMLElement | undefined = undefined; // Explicitly assigned
  let itemRefs: HTMLElement[] = [];

  // Helper function to get node configuration
  function getNodeConfig(nodeType: NodeType) {
    const iconConfig = getIconConfig(nodeType);
    const labels: Record<NodeType, string> = {
      text: 'Text',
      document: 'Document',
      task: 'Task',
      'ai-chat': 'AI Chat',
      ai_chat: 'AI Chat',
      user: 'User',
      entity: 'Entity',
      query: 'Query'
    };

    return {
      label: labels[nodeType] || 'Node',
      color: iconConfig.colorVar,
      semanticClass: iconConfig.semanticClass
    };
  }

  // Smart positioning to avoid viewport edges
  function getSmartPosition(pos: { x: number; y: number }) {
    const padding = 16;
    const maxWidth = 360;
    const maxHeight = 300;
    const spacingFromCursor = 20; // Distance from cursor

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
      // Show below cursor - use top-left corner positioning
      y = y + spacingFromCursor;
    } else {
      // Show above cursor - use bottom-left corner positioning
      // Position so the bottom of the modal is spacingFromCursor above the cursor
      y = y - spacingFromCursor;
    }

    return { x, y, showBelow };
  }

  // Smart position calculation
  $: smartPosition = getSmartPosition(position);

  function scrollToSelected() {
    const selectedItem = itemRefs[selectedIndex];
    if (selectedItem && containerRef) {
      selectedItem.scrollIntoView({
        block: 'nearest',
        behavior: 'smooth'
      });
    }
  }

  // Keyboard navigation
  function handleKeydown(event: KeyboardEvent) {
    if (!visible || results.length === 0) return;

    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        event.stopPropagation(); // Prevent cursor movement in background
        selectedIndex = Math.min(selectedIndex + 1, results.length - 1);
        scrollToSelected();
        break;
      case 'ArrowUp':
        event.preventDefault();
        event.stopPropagation(); // Prevent cursor movement in background
        selectedIndex = Math.max(selectedIndex - 1, 0);
        scrollToSelected();
        break;
      case 'Enter':
        event.preventDefault();
        event.stopPropagation(); // Prevent creating new node in background
        if (results[selectedIndex]) {
          selectResult(results[selectedIndex]);
        }
        break;
      case 'Escape':
        event.preventDefault();
        event.stopPropagation(); // Prevent any background actions
        onclose?.();
        break;
    }
  }

  function selectResult(result: NodeResult) {
    selectedIndex = results.indexOf(result);
    onselect?.(result);
  }

  // Reset selection when results change
  $: if (results) {
    selectedIndex = 0;
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
      min-width: 300px;
      max-height: 300px;
      background: hsl(var(--popover));
      border: 1px solid hsl(var(--border));
      border-radius: var(--radius);
      z-index: 1000;
      overflow-y: auto;
    "
    role="listbox"
    aria-label="Node reference autocomplete"
  >
    {#if loading}
      <div
        style="padding: 2rem; display: flex; align-items: center; justify-content: center; gap: 0.75rem; color: hsl(var(--muted-foreground));"
      >
        <div
          style="
          width: 16px;
          height: 16px;
          border: 2px solid hsl(var(--border));
          border-top: 2px solid hsl(var(--primary));
          border-radius: 50%;
          animation: spin 1s linear infinite;
        "
        ></div>
        <span>Searching nodes...</span>
      </div>
    {:else if results.length === 0}
      <div style="padding: 2rem; text-align: center; color: hsl(var(--muted-foreground));">
        {#if query}
          <div style="margin-bottom: 0.5rem; font-weight: 500;">No nodes found matching</div>
          <code
            style="background: hsl(var(--input)); color: hsl(var(--foreground)); padding: 0.25rem 0.5rem; border-radius: 0.25rem; font-size: 0.75rem;"
            >{query}</code
          >
        {:else}
          <div style="font-weight: 500; margin-bottom: 0.25rem;">No nodes available</div>
          <div style="font-size: 0.875rem;">Start typing to search</div>
        {/if}
      </div>
    {:else}
      {#each results as result, index}
        <div
          bind:this={itemRefs[index]}
          style="
            padding: 0.75rem;
            {index < results.length - 1 ? 'border-bottom: 1px solid hsl(var(--border));' : ''}
            cursor: pointer;
            {index === 0
            ? 'border-top-left-radius: var(--radius); border-top-right-radius: var(--radius);'
            : ''}
            {index === results.length - 1
            ? 'border-bottom-left-radius: var(--radius); border-bottom-right-radius: var(--radius);'
            : ''}
            {selectedIndex === index ? 'background: hsl(var(--muted));' : ''}
          "
          role="option"
          aria-selected={selectedIndex === index}
          tabindex={selectedIndex === index ? 0 : -1}
          on:click={() => selectResult(result)}
          on:mouseover={() => (selectedIndex = index)}
          on:mouseenter={() => {
            if (selectedIndex !== index) {
              selectedIndex = index;
            }
          }}
          on:focus={() => (selectedIndex = index)}
          on:keydown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              selectResult(result);
            }
          }}
        >
          <div style="display: flex; align-items: center; gap: 0.5rem;">
            <div
              style="width: 20px; height: 20px; display: flex; align-items: center; justify-content: center;"
            >
              {#if result.type && getNodeConfig(result.type)}
                <!-- Future: Dynamic icon based on node type -->
                <Icon name={result.type as any} size={16} />
              {:else}
                <!-- Fallback: Static text circle -->
                <div class="node-icon">
                  <div class="text-circle"></div>
                </div>
              {/if}
            </div>
            <div style="font-weight: 500;">{result.title}</div>
          </div>
        </div>
      {/each}
    {/if}
  </div>
{/if}

<style>
  /* Spin animation for loading */
  @keyframes spin {
    0% {
      transform: rotate(0deg);
    }
    100% {
      transform: rotate(360deg);
    }
  }
</style>
