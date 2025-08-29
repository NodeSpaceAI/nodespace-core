<script lang="ts">
  import { cn } from '$lib/utils';
  import Badge from '../badge/badge.svelte';
  import { createEventDispatcher, onMount } from 'svelte';

  // Types for node results
  interface NodeResult {
    id: string;
    title: string;
    type: 'text' | 'task' | 'ai-chat' | 'entity' | 'query' | 'user' | 'date' | 'document';
    subtitle?: string;
    metadata?: string;
  }

  // Props
  export let position: { x: number; y: number } = { x: 0, y: 0 };
  export let query: string = '';
  export let results: NodeResult[] = [];
  export let loading: boolean = false;
  export let visible: boolean = false;

  // State
  let selectedIndex: number = 0;
  let containerRef: HTMLElement | undefined = undefined; // Explicitly assigned
  let itemRefs: HTMLElement[] = [];

  const dispatch = createEventDispatcher<{
    select: NodeResult;
    close: void;
  }>();

  // Node type configuration with proper NodeSpace colors
  const nodeTypeConfig = {
    text: { icon: '📄', label: 'Text', color: 'hsl(142 71% 45%)' },
    task: { icon: '✅', label: 'Task', color: 'hsl(25 95% 53%)' },
    'ai-chat': { icon: '🤖', label: 'AI Chat', color: 'hsl(221 83% 53%)' },
    entity: { icon: '🏷️', label: 'Entity', color: 'hsl(271 81% 56%)' },
    query: { icon: '🔍', label: 'Query', color: 'hsl(330 81% 60%)' },
    user: { icon: '👤', label: 'User', color: 'hsl(142 71% 45%)' },
    date: { icon: '📅', label: 'Date', color: 'hsl(142 71% 45%)' },
    document: { icon: '📋', label: 'Document', color: 'hsl(142 71% 45%)' }
  };

  // Smart positioning to avoid viewport edges
  function getSmartPosition(pos: { x: number; y: number }) {
    const padding = 16;
    const maxWidth = 360;
    const maxHeight = 300;

    let { x, y } = pos;

    // Adjust horizontal position
    if (x + maxWidth > window.innerWidth - padding) {
      x = window.innerWidth - maxWidth - padding;
    }
    if (x < padding) {
      x = padding;
    }

    // Adjust vertical position (show above cursor if not enough space below)
    if (y + maxHeight > window.innerHeight - padding) {
      y = y - maxHeight - 20; // Show above cursor
    } else {
      y = y + 20; // Show below cursor
    }

    return { x, y };
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
        selectedIndex = Math.min(selectedIndex + 1, results.length - 1);
        scrollToSelected();
        break;
      case 'ArrowUp':
        event.preventDefault();
        selectedIndex = Math.max(selectedIndex - 1, 0);
        scrollToSelected();
        break;
      case 'Enter':
        event.preventDefault();
        if (results[selectedIndex]) {
          selectResult(results[selectedIndex]);
        }
        break;
      case 'Escape':
        event.preventDefault();
        dispatch('close');
        break;
    }
  }

  function selectResult(result: NodeResult) {
    selectedIndex = results.indexOf(result);
    dispatch('select', result);
  }

  // Return plain text highlighting for now (no HTML markup)
  function highlightMatch(text: string): string {
    // For now, just return the original text to avoid XSS warnings
    // TODO: Implement proper Svelte component-based highlighting
    return text;
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
    class={cn(
      // Professional popover styling using design system tokens
      'fixed z-[9999] bg-popover text-popover-foreground',
      'border border-border rounded-lg shadow-lg',
      'min-w-[340px] max-w-[400px] max-h-[320px]',
      'overflow-hidden',
      // Smooth animations matching design system
      'animate-in fade-in-0 zoom-in-95 duration-200',
      'data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95'
    )}
    style="left: {smartPosition.x}px; top: {smartPosition.y}px;"
    role="listbox"
    aria-label="Node reference autocomplete"
  >
    <!-- Professional header with search context -->
    <div class="px-4 py-3 border-b border-border bg-muted/30">
      <div class="flex items-center gap-2.5 text-xs">
        <div class="flex items-center gap-2">
          <div class="w-4 h-4 rounded-sm bg-primary/10 flex items-center justify-center">
            <span class="text-[10px] font-bold text-primary">@</span>
          </div>
          <span class="font-semibold text-foreground">Node Reference</span>
        </div>
        {#if query}
          <div class="flex items-center gap-1.5 text-muted-foreground">
            <span>•</span>
            <code class="bg-muted/80 px-2 py-0.5 rounded-md text-[10px] font-mono border"
              >{query}</code
            >
          </div>
        {/if}
      </div>
    </div>

    <!-- Results container -->
    <div class="overflow-y-auto max-h-[240px] py-1">
      {#if loading}
        <div class="flex items-center gap-3 px-4 py-5 text-sm text-muted-foreground">
          <div
            class="animate-spin h-4 w-4 border-2 border-primary/30 border-r-primary rounded-full"
          ></div>
          <span>Searching nodes...</span>
        </div>
      {:else if results.length === 0}
        <div class="px-4 py-8 text-center text-muted-foreground">
          {#if query}
            <div class="text-sm mb-2 font-medium">No nodes found matching</div>
            <code class="bg-muted/80 px-2.5 py-1 rounded-md text-xs border">{query}</code>
          {:else}
            <div class="text-sm font-medium">No nodes available</div>
            <div class="text-xs mt-1">Start typing to search</div>
          {/if}
        </div>
      {:else}
        {#each results as result, index (result.id)}
          <button
            bind:this={itemRefs[index]}
            class={cn(
              // Base professional item styling
              'w-full flex items-center gap-3.5 px-4 py-3 text-left',
              'transition-all duration-150 ease-out',
              'focus:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2',
              'border-l-2 border-transparent',
              // Selection states with professional styling
              selectedIndex === index
                ? 'bg-accent/80 text-accent-foreground border-l-primary shadow-sm'
                : 'hover:bg-accent/40 hover:border-l-accent-foreground/20'
            )}
            role="option"
            aria-selected={selectedIndex === index}
            onclick={() => selectResult(result)}
            onmouseenter={() => {
              selectedIndex = index;
            }}
          >
            <!-- Professional node type icon -->
            <div
              class={cn(
                'flex-shrink-0 w-8 h-8 rounded-lg flex items-center justify-center',
                'text-sm font-semibold shadow-sm border',
                selectedIndex === index ? 'shadow-md' : ''
              )}
              style="background-color: {nodeTypeConfig[result.type]
                .color}12; color: {nodeTypeConfig[result.type]
                .color}; border-color: {nodeTypeConfig[result.type].color}25;"
            >
              {nodeTypeConfig[result.type].icon}
            </div>

            <!-- Content with improved typography -->
            <div class="flex-1 min-w-0">
              <!-- Title with professional highlighting -->
              <div class="font-semibold text-sm leading-tight mb-1">
                {highlightMatch(result.title)}
              </div>

              <!-- Subtitle with better contrast -->
              {#if result.subtitle}
                <div class="text-xs text-muted-foreground leading-relaxed pr-2">
                  {highlightMatch(result.subtitle)}
                </div>
              {/if}
            </div>

            <!-- Professional metadata badge -->
            {#if result.metadata}
              <div class="flex-shrink-0">
                <Badge
                  variant="secondary"
                  class={cn(
                    'text-[10px] px-2.5 py-1 h-auto font-medium border',
                    selectedIndex === index ? 'bg-background/50' : ''
                  )}
                >
                  {result.metadata}
                </Badge>
              </div>
            {/if}
          </button>
        {/each}
      {/if}
    </div>

    <!-- Professional footer with keyboard hints -->
    {#if results.length > 0}
      <div class="px-4 py-2.5 border-t border-border bg-muted/20">
        <div
          class="flex items-center justify-between text-[10px] text-muted-foreground font-semibold"
        >
          <div class="flex items-center gap-3">
            <span class="flex items-center gap-1">
              <kbd
                class="inline-flex items-center rounded border bg-muted px-1.5 py-0.5 text-[9px] font-mono"
                >↑↓</kbd
              >
              Navigate
            </span>
          </div>
          <div class="flex items-center gap-3">
            <span class="flex items-center gap-1">
              <kbd
                class="inline-flex items-center rounded border bg-muted px-1.5 py-0.5 text-[9px] font-mono"
                >Enter</kbd
              >
              Select
            </span>
            <span class="flex items-center gap-1">
              <kbd
                class="inline-flex items-center rounded border bg-muted px-1.5 py-0.5 text-[9px] font-mono"
                >Esc</kbd
              >
              Close
            </span>
          </div>
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  /* Professional highlight styling for search matches */
  :global(mark) {
    background-color: hsl(var(--primary) / 0.12);
    color: hsl(var(--primary));
    padding: 0.125rem 0.25rem;
    border-radius: 0.25rem;
    font-weight: 600;
    font-size: inherit;
  }

  :global(.dark mark) {
    background-color: hsl(var(--primary) / 0.2);
    color: hsl(var(--primary-foreground));
  }
</style>
