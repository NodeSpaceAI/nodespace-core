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
  import type { NodeReferenceService } from '$lib/services/NodeReferenceService';

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
  import Card from '$lib/components/ui/card/card.svelte';
  import CardContent from '$lib/components/ui/card/card-content.svelte';
  import CardHeader from '$lib/components/ui/card/card-header.svelte';
  import CardTitle from '$lib/components/ui/card/card-title.svelte';
  import Button from '$lib/components/ui/button/button.svelte';
  import type { AutocompleteResult, NodeSuggestion } from '$lib/services/NodeReferenceService';
  import type { NodeSpaceNode } from '$lib/services/MockDatabaseService';

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
    nodeSelect: { node: NodeSpaceNode | NewNodeRequest };
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

  // Node type configuration
  const NODE_TYPE_CONFIG = {
    text: { icon: '📄', label: 'Text' },
    task: { icon: '☐', label: 'Task' },
    user: { icon: '👤', label: 'User' },
    date: { icon: '📅', label: 'Date' },
    ai_chat: { icon: '🤖', label: 'AI Chat' },
    entity: { icon: '🏷️', label: 'Entity' },
    query: { icon: '🔍', label: 'Query' }
  } as const;

  const DEFAULT_NODE_TYPE = { icon: '📄', label: 'Node' };

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
          nodeType: node.type,
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
    const modalWidth = 400;
    const modalHeight = 300;
    const padding = 16;

    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;

    let x = pos.x;
    let y = pos.y;
    let placement: 'below' | 'above' = 'below';

    // Adjust horizontal position if too close to right edge
    if (x + modalWidth > viewportWidth - padding) {
      x = viewportWidth - modalWidth - padding;
    }

    // Adjust horizontal position if too close to left edge
    if (x < padding) {
      x = padding;
    }

    // Adjust vertical position
    if (y + modalHeight > viewportHeight - padding) {
      // Place above cursor if no room below
      y = pos.y - modalHeight - 20;
      placement = 'above';

      // If still not enough room, place at top
      if (y < padding) {
        y = padding;
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

  function getNodeTypeConfig(nodeType: string): { icon: string; label: string } {
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
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <div class="fixed inset-0 z-50 bg-black/20 backdrop-blur-sm" on:click={closeModal}>
    <!-- Modal content -->
    <!-- svelte-ignore a11y-click-events-have-key-events -->
    <!-- svelte-ignore a11y-no-static-element-interactions -->
    <div
      bind:this={modalElement}
      class="fixed z-50 w-96 max-h-80"
      style="left: {adjustedPosition.x}px; top: {adjustedPosition.y}px;"
      on:click|stopPropagation
    >
      <Card class="shadow-lg border-border/50 backdrop-blur-sm bg-popover/95">
        <!-- Header -->
        <CardHeader class="pb-2">
          <CardTitle class="text-sm font-medium text-muted-foreground flex items-center gap-2">
            <span class="text-lg">@</span>
            {#if query}
              Reference: "{query}"
            {:else}
              Recent Nodes
            {/if}
            {#if totalResults > 0}
              <span class="text-xs bg-muted px-1.5 py-0.5 rounded">
                {totalResults}{hasMore ? '+' : ''}
              </span>
            {/if}
          </CardTitle>
        </CardHeader>

        <!-- Content -->
        <CardContent class="p-0 pb-3">
          <div class="max-h-64 overflow-y-auto">
            {#if isLoading}
              <!-- Loading state -->
              <div class="flex items-center justify-center py-8">
                <div
                  class="animate-spin h-5 w-5 border-2 border-primary border-t-transparent rounded-full"
                ></div>
                <span class="ml-2 text-sm text-muted-foreground">Searching...</span>
              </div>
            {:else if searchError}
              <!-- Error state -->
              <div class="px-4 py-6 text-center">
                <div class="text-destructive text-sm">{searchError}</div>
                <Button
                  variant="ghost"
                  size="sm"
                  class="mt-2"
                  on:click={() => performSearch(query)}
                >
                  Try Again
                </Button>
              </div>
            {:else if searchResults.length === 0}
              <!-- Empty state -->
              <div class="px-4 py-6 text-center">
                <div class="text-muted-foreground text-sm mb-2">
                  {query ? 'No matching nodes found' : 'No recent nodes'}
                </div>
                {#if query}
                  <!-- Create new node option when no results -->
                  <Button variant="outline" size="sm" class="gap-2" on:click={createNewNode}>
                    <span class="text-lg">✨</span>
                    Create "{query}"
                  </Button>
                {/if}
              </div>
            {:else}
              <!-- Results list -->
              <div class="px-2">
                {#each searchResults as suggestion, index (suggestion.nodeId)}
                  {@const isSelected = index === selectedIndex}
                  {@const nodeConfig = getNodeTypeConfig(suggestion.nodeType)}

                  <button
                    class={cn(
                      'w-full flex items-start gap-3 p-2 rounded-md text-left transition-colors',
                      'hover:bg-accent/50 focus:bg-accent/50 focus:outline-none',
                      isSelected && 'bg-accent text-accent-foreground'
                    )}
                    on:click={() => selectNodeSuggestion(suggestion)}
                    aria-label="Select {suggestion.title}"
                  >
                    <!-- Node type icon -->
                    <div class="flex-shrink-0 w-5 h-5 flex items-center justify-center mt-0.5">
                      <span class="text-sm">{nodeConfig.icon}</span>
                    </div>

                    <!-- Node content -->
                    <div class="flex-1 min-w-0">
                      <div class="font-medium text-sm truncate">
                        <!-- Safe highlighting without XSS risk -->
                        {#each parseHighlights(suggestion.title, suggestion.matchPositions) as segment}
                          {#if segment.highlighted}
                            <mark class="bg-yellow-200 dark:bg-yellow-800 px-0.5 rounded"
                              >{segment.text}</mark
                            >
                          {:else}
                            {segment.text}
                          {/if}
                        {/each}
                      </div>

                      {#if suggestion.content && suggestion.content !== suggestion.title}
                        <div class="text-xs text-muted-foreground mt-1 line-clamp-2">
                          {suggestion.content.substring(0, 100)}{suggestion.content.length > 100
                            ? '...'
                            : ''}
                        </div>
                      {/if}

                      {#if suggestion.hierarchy.length > 1}
                        <div class="text-xs text-muted-foreground mt-1 flex items-center gap-1">
                          <span>📁</span>
                          <span class="truncate">
                            {suggestion.hierarchy.slice(0, -1).join(' › ')}
                          </span>
                        </div>
                      {/if}
                    </div>

                    <!-- Metadata -->
                    <div class="flex-shrink-0 flex flex-col items-end gap-1">
                      <span class="text-xs text-muted-foreground bg-muted px-1.5 py-0.5 rounded">
                        {nodeConfig.label}
                      </span>

                      {#if suggestion.relevanceScore > 0.8}
                        <div class="w-2 h-2 bg-green-500 rounded-full" title="High relevance"></div>
                      {:else if suggestion.relevanceScore > 0.6}
                        <div
                          class="w-2 h-2 bg-yellow-500 rounded-full"
                          title="Medium relevance"
                        ></div>
                      {/if}
                    </div>
                  </button>
                {/each}

                <!-- Create new node option -->
                {#if query}
                  {@const isCreateSelected = selectedIndex === searchResults.length}

                  <div class="border-t border-border mt-2 pt-2">
                    <button
                      class={cn(
                        'w-full flex items-center gap-3 p-2 rounded-md text-left transition-colors',
                        'hover:bg-accent/50 focus:bg-accent/50 focus:outline-none',
                        isCreateSelected && 'bg-accent text-accent-foreground'
                      )}
                      on:click={createNewNode}
                      aria-label="Create new node with content: {query}"
                    >
                      <div class="flex-shrink-0 w-5 h-5 flex items-center justify-center">
                        <span class="text-sm">✨</span>
                      </div>

                      <div class="flex-1">
                        <div class="font-medium text-sm">Create new node</div>
                        <div class="text-xs text-muted-foreground">
                          "{query}"
                        </div>
                      </div>

                      <div class="flex-shrink-0">
                        <span class="text-xs text-muted-foreground bg-muted px-1.5 py-0.5 rounded">
                          Text
                        </span>
                      </div>
                    </button>
                  </div>
                {/if}
              </div>
            {/if}
          </div>

          <!-- Footer with keyboard shortcuts -->
          <div
            class="px-4 pt-2 border-t border-border text-xs text-muted-foreground flex items-center justify-between"
          >
            <div class="flex items-center gap-3">
              <span><kbd class="px-1 bg-muted rounded">↕</kbd> Navigate</span>
              <span><kbd class="px-1 bg-muted rounded">⏎</kbd> Select</span>
            </div>
            <span><kbd class="px-1 bg-muted rounded">Esc</kbd> Close</span>
          </div>
        </CardContent>
      </Card>
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
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
</style>
