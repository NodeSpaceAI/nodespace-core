<script lang="ts">
  import Icon, { type NodeType } from '$lib/design/icons';
  import { onMount } from 'svelte';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';

  // Import icon configuration from registry for consistency
  import { getIconConfig } from '$lib/design/icons/registry';

  // Types for node results
  interface NodeResult {
    id: string;
    title: string;
    nodeType: NodeType;
    subtitle?: string;
    metadata?: string;
    isShortcut?: boolean;
    submenuPosition?: { x: number; y: number };
  }

  // Props
  export let position: { x: number; y: number } = { x: 0, y: 0 };
  export let query: string = '';
  export let results: NodeResult[] = [];
  export let visible: boolean = false;

  /**
   * UX Decision: Loading state removed (previously showed "Searching..." message)
   *
   * Rationale:
   * 1. 3-character minimum search threshold prevents premature backend calls
   * 2. Results appear fast enough that loading state creates visual noise
   * 3. Empty results state is sufficient feedback during search
   * 4. Avoids flickering "Searching..." → results transition
   *
   * Parent component (base-node.svelte) implements smart caching to minimize
   * backend calls, making loading indicators unnecessary for this UX pattern.
   */

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

  // Store initial position to prevent movement while typing
  let initialPosition: { x: number; y: number; showBelow: boolean } | null = null;

  // Smart position calculation - only calculate once when modal first appears
  $: if (visible && !initialPosition) {
    initialPosition = getSmartPosition(position);
  } else if (!visible) {
    initialPosition = null;
  }

  $: smartPosition = initialPosition || getSmartPosition(position);

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

    // For date-picker, attach position information for submenu positioning
    if (result.id === 'date-picker' && itemRefs[selectedIndex]) {
      const itemRect = itemRefs[selectedIndex].getBoundingClientRect();
      // Add position metadata to the result (now part of NodeResult interface)
      result.submenuPosition = {
        x: itemRect.right,
        y: itemRect.top
      };
    }

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
    {#if results.length === 0}
      <!-- Always show "Create new" option when no results, never show "No nodes found" -->
      {#if query}
        <div
          style="padding: 0.75rem; cursor: pointer; color: hsl(var(--foreground)); background: hsl(var(--muted) / 0.5); font-weight: 600; border-radius: var(--radius); text-align: center;"
          role="button"
          tabindex="0"
          on:mousedown={(e) => {
            e.preventDefault();
            onselect?.({ id: 'new', title: query, nodeType: 'text' });
          }}
          on:keydown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              onselect?.({ id: 'new', title: query, nodeType: 'text' });
            }
          }}
          on:mouseover={(e) => {
            e.currentTarget.style.background = 'hsl(var(--muted))';
          }}
          on:mouseout={(e) => {
            e.currentTarget.style.background = 'hsl(var(--muted) / 0.5)';
          }}
          on:blur={(e) => {
            e.currentTarget.style.background = 'hsl(var(--muted) / 0.5)';
          }}
          on:focus
        >
          + Create new "{query.length > 30 ? query.substring(0, 30) + '...' : query}" node
        </div>
      {:else}
        <div style="padding: 2rem; text-align: center; color: hsl(var(--muted-foreground));">
          <div style="font-weight: 500; margin-bottom: 0.25rem;">Start typing to search</div>
          <div style="font-size: 0.875rem;">Type @ followed by a node name</div>
        </div>
      {/if}
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
          on:mousedown={(e) => {
            e.preventDefault(); // Prevent blur on contenteditable
            selectResult(result);
          }}
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
          <div
            style="display: flex; align-items: center; {result.id === 'date-picker'
              ? 'justify-content: space-between; width: 100%;'
              : 'gap: 0.5rem;'}"
          >
            <div style="display: flex; align-items: center; gap: 0.5rem;">
              <div
                style="width: 20px; height: 20px; display: flex; align-items: center; justify-content: center;"
              >
                {#if result.nodeType && getNodeConfig(result.nodeType)}
                  <!-- Future: Dynamic icon based on node type -->
                  <Icon name={result.nodeType as any} size={16} />
                {:else}
                  <!-- Fallback: Static text circle -->
                  <div class="node-icon">
                    <div class="text-circle"></div>
                  </div>
                {/if}
              </div>
              <div style="font-weight: 500;">{result.title}</div>
            </div>
            {#if result.id === 'date-picker'}
              <div
                style="width: 20px; height: 20px; display: flex; align-items: center; justify-content: center; color: hsl(var(--muted-foreground));"
              >
                <ChevronRight size={16} />
              </div>
            {/if}
          </div>
        </div>
      {/each}

      <!-- Always show "Create new" option at the bottom when there's a query -->
      {#if query}
        <div
          style="padding: 0.75rem; cursor: pointer; color: hsl(var(--foreground)); background: hsl(var(--muted) / 0.5); font-weight: 600; border-top: 1px solid hsl(var(--border)); border-bottom-left-radius: var(--radius); border-bottom-right-radius: var(--radius); text-align: center;"
          role="button"
          tabindex="0"
          on:mousedown={(e) => {
            e.preventDefault();
            onselect?.({ id: 'new', title: query, nodeType: 'text' });
          }}
          on:keydown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              onselect?.({ id: 'new', title: query, nodeType: 'text' });
            }
          }}
          on:mouseover={(e) => {
            e.currentTarget.style.background = 'hsl(var(--muted))';
          }}
          on:mouseout={(e) => {
            e.currentTarget.style.background = 'hsl(var(--muted) / 0.5)';
          }}
          on:blur={(e) => {
            e.currentTarget.style.background = 'hsl(var(--muted) / 0.5)';
          }}
          on:focus
        >
          + Create new "{query.length > 30 ? query.substring(0, 30) + '...' : query}" node
        </div>
      {/if}
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
