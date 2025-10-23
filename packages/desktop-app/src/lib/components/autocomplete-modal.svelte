<!--
  AutocompleteModal - Universal Node Reference System UI Component
  
  Provides the user interface for the @ trigger functionality, allowing users to
  search and select existing nodes or create new ones. Integrates with the 
  NodeReferenceService for search functionality and follows NodeSpace design patterns.
  
  Key Features:
  - Real-time search filtering as user types
  - Keyboard navigation (arrow keys, enter, escape)
  - Visual selection highlighting
  - Position near cursor with screen edge handling
  - Node type icons and metadata display
  - Create new node option
  - Accessibility support
-->

<script context="module" lang="ts">
  import type { NodeReferenceService } from '$lib/services/node-reference-service';

  export interface AutocompleteModalProps {
    visible: boolean;
    position: { x: number; y: number };
    query: string;
    nodeReferenceService: NodeReferenceService;
  }

  export interface NewNodeRequest {
    type: 'create';
    content: string;
    nodeType: string;
  }
</script>

<script lang="ts">
  import { createEventDispatcher, onMount, onDestroy } from 'svelte';
  import { cn } from '$lib/utils.js';
  import Button from '$lib/components/ui/button/button.svelte';
  import Badge from '$lib/components/ui/badge/badge.svelte';
  import Separator from '$lib/components/ui/separator/separator.svelte';
  import Icon, { type IconName } from '$lib/design/icons';
  import type { AutocompleteResult, NodeSuggestion } from '$lib/services/node-reference-service';
  import type { Node } from '$lib/types';

  // ============================================================================
  // Component Props & Types
  // ============================================================================

  // Props
  export let visible: boolean = false;
  export let position: { x: number; y: number } = { x: 0, y: 0 };
  export let query: string = '';
  export let nodeReferenceService: NodeReferenceService | null = null;

  // Event dispatcher
  const dispatch = createEventDispatcher<{
    nodeSelect: { node: Node | NewNodeRequest };
    close: void;
  }>();

  // ============================================================================
  // Component State
  // ============================================================================

  let modalElement: HTMLDivElement;
  let searchResults: NodeSuggestion[] = [];
  let selectedIndex: number = 0;
  let isLoading: boolean = false;
  let searchError: string | null = null;
  let totalResults: number = 0;
  let hasMore: boolean = false;

  // Debounced search
  let searchTimeout: number | null = null;
  const SEARCH_DEBOUNCE_MS = 150;

  // Modal positioning
  let adjustedPosition: { x: number; y: number; placement: 'below' | 'above' } = {
    x: 0,
    y: 0,
    placement: 'below'
  };

  // Node type configuration with proper Icon components
  const NODE_TYPE_CONFIG = {
    text: { icon: 'circle' as IconName, label: 'Text' },
    task: { icon: 'taskIncomplete' as IconName, label: 'Task' },
    user: { icon: 'circle' as IconName, label: 'User' },
    date: { icon: 'circle' as IconName, label: 'Date' },
    ai_chat: { icon: 'aiSquare' as IconName, label: 'AI Chat' },
    entity: { icon: 'circle' as IconName, label: 'Entity' },
    query: { icon: 'circle' as IconName, label: 'Query' }
  } as const;

  const DEFAULT_NODE_TYPE = { icon: 'circle' as IconName, label: 'Node' };

  // ============================================================================
  // Reactive Statements
  // ============================================================================

  // Update search results when query changes
  $: if (visible && query) {
    performSearch(query);
  } else if (visible && !query) {
    // Show recent/default nodes when query is empty
    showDefaultResults();
  }

  // Reset selection when results change
  $: if (searchResults) {
    selectedIndex = 0;
  }

  // Adjust modal position based on screen boundaries
  $: if (visible) {
    adjustedPosition = calculateModalPosition(position);
  }

  // ============================================================================
  // Search Functions
  // ============================================================================

  async function performSearch(searchQuery: string): Promise<void> {
    if (!searchQuery.trim()) {
      searchResults = [];
      return;
    }

    // Clear existing timeout
    if (searchTimeout) {
      clearTimeout(searchTimeout);
    }

    // Debounce search
    searchTimeout = window.setTimeout(async () => {
      isLoading = true;
      searchError = null;

      try {
        const triggerContext = {
          trigger: '@',
          query: searchQuery,
          startPosition: 0,
          endPosition: searchQuery.length,
          element: document.activeElement as HTMLElement,
          isValid: true,
          metadata: {}
        };

        if (!nodeReferenceService) {
          throw new Error('NodeReferenceService not available');
        }
        const result: AutocompleteResult =
          await nodeReferenceService.showAutocomplete(triggerContext);

        searchResults = result.suggestions;
        totalResults = result.totalCount;
        hasMore = result.hasMore;
        selectedIndex = 0;
      } catch (error) {
        console.error('AutocompleteModal: Search failed', error);
        searchError = 'Search failed. Please try again.';
        searchResults = [];
      } finally {
        isLoading = false;
      }
    }, SEARCH_DEBOUNCE_MS);
  }

  async function showDefaultResults(): Promise<void> {
    isLoading = true;
    searchError = null;

    try {
      // Get recent nodes or default suggestions
      if (!nodeReferenceService) {
        throw new Error('NodeReferenceService not available');
      }
      const recentNodes = await nodeReferenceService.searchNodes('', undefined);

      // Convert to suggestions format
      const suggestions: NodeSuggestion[] = [];
      for (const node of recentNodes.slice(0, 5)) {
        const suggestion: NodeSuggestion = {
          nodeId: node.id,
          title: extractNodeTitle(node.content),
          content: node.content.substring(0, 200),
          nodeType: node.nodeType || 'text',
          relevanceScore: 0.5,
          matchType: 'title',
          matchPositions: [],
          hierarchy: [],
          metadata: {}
        };
        suggestions.push(suggestion);
      }

      searchResults = suggestions;
      selectedIndex = 0;
    } catch (error) {
      console.error('AutocompleteModal: Failed to load default results', error);
      searchResults = [];
    } finally {
      isLoading = false;
    }
  }

  // ============================================================================
  // Node Selection Functions
  // ============================================================================

  function selectCurrentNode(): void {
    if (selectedIndex < searchResults.length) {
      const selectedSuggestion = searchResults[selectedIndex];
      selectNodeSuggestion(selectedSuggestion);
    } else {
      // Select "Create new node" option
      createNewNode();
    }
  }

  async function selectNodeSuggestion(suggestion: NodeSuggestion): Promise<void> {
    try {
      if (!nodeReferenceService) {
        throw new Error('NodeReferenceService not available');
      }
      const node = await nodeReferenceService.resolveNodespaceURI(
        nodeReferenceService.createNodespaceURI(suggestion.nodeId)
      );

      if (node) {
        dispatch('nodeSelect', { node });
        closeModal();
      } else {
        console.error('AutocompleteModal: Failed to resolve selected node', suggestion.nodeId);
      }
    } catch (error) {
      console.error('AutocompleteModal: Error selecting node', error);
    }
  }

  function createNewNode(): void {
    const newNodeRequest: NewNodeRequest = {
      type: 'create',
      content: query,
      nodeType: 'text' // Default to text node
    };

    dispatch('nodeSelect', { node: newNodeRequest });
    closeModal();
  }

  function closeModal(): void {
    dispatch('close');
  }

  // ============================================================================
  // Keyboard Navigation
  // ============================================================================

  function handleKeydown(event: KeyboardEvent): void {
    if (!visible) return;

    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        moveSelection(1);
        break;

      case 'ArrowUp':
        event.preventDefault();
        moveSelection(-1);
        break;

      case 'Enter':
        event.preventDefault();
        selectCurrentNode();
        break;

      case 'Escape':
        event.preventDefault();
        closeModal();
        break;

      case 'Tab':
        event.preventDefault();
        moveSelection(event.shiftKey ? -1 : 1);
        break;
    }
  }

  function moveSelection(direction: number): void {
    const maxIndex = searchResults.length; // includes "Create new" option
    selectedIndex = Math.max(0, Math.min(maxIndex, selectedIndex + direction));
  }

  // ============================================================================
  // Position Calculation
  // ============================================================================

  function calculateModalPosition(pos: { x: number; y: number }): {
    x: number;
    y: number;
    placement: 'below' | 'above';
  } {
    // More responsive width calculation
    const modalWidth = Math.min(448, window.innerWidth - 32); // max-w-md with padding
    const modalHeight = 320; // matches max-h-[320px]
    const padding = 16;
    const offset = 8; // Small offset from cursor for better visual connection

    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;

    let x = pos.x;
    let y = pos.y + offset;
    let placement: 'below' | 'above' = 'below';

    // Smart horizontal positioning
    if (x + modalWidth > viewportWidth - padding) {
      // Align to right edge if overflowing
      x = viewportWidth - modalWidth - padding;
    }

    // Ensure minimum left padding
    if (x < padding) {
      x = padding;
    }

    // Smart vertical positioning with preference for below
    if (y + modalHeight > viewportHeight - padding) {
      // Try placing above with offset
      y = pos.y - modalHeight - offset;
      placement = 'above';

      // If still overflowing above, use best fit position
      if (y < padding) {
        // Calculate best position that fits
        const availableBelow = viewportHeight - pos.y - padding;
        const availableAbove = pos.y - padding;

        if (availableBelow >= availableAbove) {
          y = pos.y + offset;
          placement = 'below';
        } else {
          y = padding;
          placement = 'above';
        }
      }
    }

    return { x, y, placement };
  }

  // ============================================================================
  // Utility Functions
  // ============================================================================

  function extractNodeTitle(content: string): string {
    if (!content) return 'Untitled';

    const lines = content.split('\n');
    const firstLine = lines[0].trim();

    // Remove markdown header syntax
    const headerMatch = firstLine.match(/^#{1,6}\s*(.*)$/);
    if (headerMatch) {
      return headerMatch[1].trim() || 'Untitled';
    }

    // Return first non-empty line, truncated
    return firstLine.substring(0, 50) || 'Untitled';
  }

  function getNodeTypeConfig(nodeType: string): { icon: IconName; label: string } {
    return NODE_TYPE_CONFIG[nodeType as keyof typeof NODE_TYPE_CONFIG] || DEFAULT_NODE_TYPE;
  }

  // ============================================================================
  // Safe Highlighting System (No XSS Risk)
  // ============================================================================

  interface HighlightSegment {
    text: string;
    highlighted: boolean;
  }

  function parseHighlights(text: string, positions: number[]): HighlightSegment[] {
    if (!positions.length) {
      return [{ text, highlighted: false }];
    }

    const segments: HighlightSegment[] = [];
    let lastIndex = 0;

    for (const pos of positions) {
      // Add non-highlighted text before this position
      if (pos > lastIndex) {
        segments.push({ text: text.substring(lastIndex, pos), highlighted: false });
      }

      // Add highlighted character at this position
      if (pos < text.length) {
        segments.push({ text: text[pos], highlighted: true });
      }

      lastIndex = pos + 1;
    }

    // Add remaining non-highlighted text
    if (lastIndex < text.length) {
      segments.push({ text: text.substring(lastIndex), highlighted: false });
    }

    return segments;
  }

  // ============================================================================
  // Lifecycle
  // ============================================================================

  onMount(() => {
    document.addEventListener('keydown', handleKeydown);
  });

  onDestroy(() => {
    document.removeEventListener('keydown', handleKeydown);
    if (searchTimeout) {
      clearTimeout(searchTimeout);
    }
  });
</script>

<!-- ============================================================================ -->
<!-- Template -->
<!-- ============================================================================ -->

{#if visible}
  <!-- Modal overlay for click-outside handling -->
  <div
    class="fixed inset-0 z-50 bg-red-500/50 backdrop-blur-[2px]"
    on:click={closeModal}
    on:keydown={(e) => e.key === 'Escape' && closeModal()}
    role="button"
    tabindex="-1"
    style="animation: overlayFadeIn 0.15s ease-out;"
    aria-label="Close autocomplete modal"
  >
    <!-- Modal positioned using shadcn-svelte dropdown patterns -->
    <div
      bind:this={modalElement}
      class="fixed z-50 min-w-80 max-w-md"
      style="left: {adjustedPosition.x}px; top: {adjustedPosition.y}px; animation: modalSlideIn 0.2s ease-out;"
      on:click|stopPropagation
      on:keydown|stopPropagation
      role="dialog"
      tabindex="0"
      aria-modal="true"
      aria-label="Node autocomplete suggestions"
      aria-describedby="autocomplete-header autocomplete-shortcuts"
    >
      <!-- Use shadcn-svelte dropdown content styling -->
      <div
        class="bg-popover text-popover-foreground z-50 min-w-[8rem] overflow-y-auto overflow-x-hidden rounded-md border p-1 shadow-md outline-none max-h-[320px] data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95"
      >
        <!-- Header using dropdown menu patterns -->
        <div id="autocomplete-header" class="flex items-center gap-2 px-2 py-1.5 text-sm">
          <div class="flex items-center gap-1.5">
            <span class="text-muted-foreground font-medium">@</span>
            <span class="font-medium text-foreground">
              {#if query}
                {query}
              {:else}
                Recent nodes
              {/if}
            </span>
          </div>

          <!-- Results count badge -->
          {#if totalResults > 0 && !isLoading}
            <div class="ml-auto">
              <Badge variant="secondary" class="text-xs h-5">
                {totalResults}{hasMore ? '+' : ''}
              </Badge>
            </div>
          {/if}

          <!-- Keyboard shortcuts -->
          {#if !totalResults || totalResults === 0}
            <div id="autocomplete-shortcuts" class="flex items-center gap-1 ml-auto">
              <Badge variant="outline" class="text-xs h-5 px-1">↑↓</Badge>
              <Badge variant="outline" class="text-xs h-5 px-1">⏎</Badge>
              <Badge variant="outline" class="text-xs h-5 px-1">esc</Badge>
            </div>
          {/if}
        </div>

        <!-- Separator after header -->
        {#if searchResults.length > 0 || isLoading || searchError}
          <Separator class="my-1" />
        {/if}

        <!-- Content area -->
        {#if isLoading}
          <!-- Loading state using dropdown item structure -->
          <div
            class="relative flex cursor-default select-none items-center rounded-sm px-2 py-6 text-sm justify-center gap-3"
          >
            <div
              class="w-4 h-4 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
            ></div>
            <span class="text-muted-foreground">Searching...</span>
          </div>
        {:else if searchError}
          <!-- Error state -->
          <div class="px-2 py-4 text-center space-y-3">
            <div class="text-sm text-destructive">{searchError}</div>
            <Button
              variant="ghost"
              size="sm"
              class="h-7 px-2 text-xs"
              onclick={() => performSearch(query)}
            >
              Try again
            </Button>
          </div>
        {:else if searchResults.length === 0}
          <!-- Empty state -->
          <div class="px-2 py-4 text-center space-y-3">
            <div class="text-sm text-muted-foreground">
              {query ? 'No matching nodes found' : 'No recent nodes'}
            </div>
            {#if query}
              <Button
                variant="outline"
                size="sm"
                class="h-7 px-2 text-xs gap-1"
                onclick={createNewNode}
              >
                <span class="text-sm">✨</span>
                Create "{query.length > 20 ? query.substring(0, 20) + '...' : query}"
              </Button>
            {/if}
          </div>
        {:else}
          <!-- Results using proper dropdown menu items -->
          {#each searchResults as suggestion, index (suggestion.nodeId)}
            {@const isSelected = index === selectedIndex}
            {@const nodeConfig = getNodeTypeConfig(suggestion.nodeType)}

            <div
              class={cn(
                'flex items-start gap-3 py-2 px-2 cursor-pointer rounded-sm transition-colors hover:bg-accent hover:text-accent-foreground',
                isSelected && 'bg-accent text-accent-foreground'
              )}
              role="button"
              tabindex="0"
              on:click={() => selectNodeSuggestion(suggestion)}
              on:keydown={(e) => e.key === 'Enter' && selectNodeSuggestion(suggestion)}
            >
              <!-- Node type icon with semantic class -->
              <div
                class="flex-shrink-0 w-6 h-6 flex items-center justify-center rounded bg-muted/50 node-icon"
              >
                <Icon name={nodeConfig.icon} size={14} />
              </div>

              <!-- Node content -->
              <div class="flex-1 min-w-0 space-y-1">
                <!-- Title with highlighting -->
                <div class="font-medium text-sm leading-tight">
                  {#each parseHighlights(suggestion.title, suggestion.matchPositions) as segment}
                    {#if segment.highlighted}
                      <mark class="bg-primary/20 text-primary px-0.5 rounded font-semibold">
                        {segment.text}
                      </mark>
                    {:else}
                      {segment.text}
                    {/if}
                  {/each}
                </div>

                <!-- Content preview -->
                {#if suggestion.content && suggestion.content !== suggestion.title}
                  <div class="text-xs text-muted-foreground leading-relaxed line-clamp-2">
                    {suggestion.content.substring(0, 100)}{suggestion.content.length > 100
                      ? '...'
                      : ''}
                  </div>
                {/if}

                <!-- Hierarchy path -->
                {#if suggestion.hierarchy.length > 1}
                  <div class="flex items-center gap-1 text-xs text-muted-foreground">
                    <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="2"
                        d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2H5a2 2 0 00-2-2z"
                      ></path>
                    </svg>
                    <span class="truncate max-w-32">
                      {suggestion.hierarchy.slice(0, -1).join(' › ')}
                    </span>
                  </div>
                {/if}
              </div>

              <!-- Metadata -->
              <div class="flex-shrink-0 flex flex-col items-end gap-1">
                <!-- Node type badge -->
                <Badge variant="secondary" class="text-xs h-5">
                  {nodeConfig.label}
                </Badge>

                <!-- Relevance indicator -->
                {#if suggestion.relevanceScore > 0.85}
                  <Badge variant="outline" class="text-xs h-4 px-1 text-emerald-600">
                    Excellent
                  </Badge>
                {:else if suggestion.relevanceScore > 0.7}
                  <Badge variant="outline" class="text-xs h-4 px-1 text-blue-600">Good</Badge>
                {/if}
              </div>
            </div>
          {/each}

          <!-- Create new node option -->
          {#if query}
            {@const isCreateSelected = selectedIndex === searchResults.length}

            <Separator class="my-1" />

            <div
              class={cn(
                'flex items-center gap-3 py-2 px-2 cursor-pointer rounded-sm transition-colors hover:bg-accent hover:text-accent-foreground',
                isCreateSelected && 'bg-accent text-accent-foreground'
              )}
              role="button"
              tabindex="0"
              on:click={createNewNode}
              on:keydown={(e) => e.key === 'Enter' && createNewNode()}
            >
              <!-- Create icon -->
              <div
                class="flex-shrink-0 w-6 h-6 flex items-center justify-center rounded bg-primary/10 node-icon"
              >
                <Icon name="circle" size={14} color="hsl(var(--primary))" />
              </div>

              <!-- Content -->
              <div class="flex-1 space-y-0.5">
                <div class="font-medium text-sm">Create new node</div>
                <div class="text-xs text-muted-foreground truncate">
                  "{query.length > 30 ? query.substring(0, 30) + '...' : query}"
                </div>
              </div>

              <!-- Node type badge -->
              <Badge variant="secondary" class="text-xs h-5">Text</Badge>
            </div>
          {/if}
        {/if}
      </div>
    </div>
  </div>
{/if}

<!-- ============================================================================ -->
<!-- Styles -->
<!-- ============================================================================ -->

<style>
  .line-clamp-2 {
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  /* Modern, smooth entry animations */
  @keyframes overlayFadeIn {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }

  @keyframes modalSlideIn {
    from {
      opacity: 0;
      transform: translateY(-8px) scale(0.96);
    }
    to {
      opacity: 1;
      transform: translateY(0) scale(1);
    }
  }

  /* Enhanced scrollbar styling for better visual consistency */
  .max-h-\[320px\]::-webkit-scrollbar {
    width: 6px;
  }

  .max-h-\[320px\]::-webkit-scrollbar-track {
    background: transparent;
  }

  .max-h-\[320px\]::-webkit-scrollbar-thumb {
    background: hsl(var(--muted-foreground) / 0.2);
    border-radius: 3px;
  }

  .max-h-\[320px\]::-webkit-scrollbar-thumb:hover {
    background: hsl(var(--muted-foreground) / 0.3);
  }

  /* Ensure smooth scrolling behavior */
  .max-h-\[320px\] {
    scroll-behavior: smooth;
  }
</style>
