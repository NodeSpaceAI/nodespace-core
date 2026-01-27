<!--
  BacklinksPanel - Collapsible panel showing container-level backlinks

  Design matches Schema-Driven Property Forms:
  - Simple collapsible trigger with count and chevron
  - List of node links with icons (like node tree, but without children)
  - Clean, minimal styling
-->

<script lang="ts">
  import { Collapsible } from 'bits-ui';
  import * as tauriCommands from '$lib/services/tauri-commands';
  import Icon, { type IconName } from '$lib/design/icons/icon.svelte';
  import { onMount } from 'svelte';
  import type { Node } from '$lib/types/node';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('BacklinksPanel');

  let { nodeId }: { nodeId: string } = $props();

  let backlinks = $state<Node[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let isOpen = $state(false);

  onMount(async () => {
    try {
      // Get root IDs that mention this node
      const rootIds = await tauriCommands.getMentioningRoots(nodeId);

      // Fetch full node data for each root explicitly
      // (don't rely on cache - ensure we always have the latest data)
      const nodes = await Promise.all(rootIds.map((id) => tauriCommands.getNode(id)));

      // Filter out null values (nodes that no longer exist)
      backlinks = nodes.filter((node): node is Node => node !== null);
    } catch (err) {
      log.error('Failed to load backlinks:', err);
      error = err instanceof Error ? err.message : 'Failed to load backlinks';
    } finally {
      loading = false;
    }
  });

  function getNodeIcon(nodeType: string): IconName {
    const iconMap: Record<string, IconName> = {
      date: 'calendar',
      task: 'circle',
      text: 'text',
      'ai-chat': 'aiSquare'
    };
    return iconMap[nodeType] || 'text';
  }
</script>

<!-- Fixed position at bottom, expands upward -->
<div class="backlinks-panel-container">
  <Collapsible.Root bind:open={isOpen}>
    <!-- Trigger stays at bottom -->
    <Collapsible.Trigger aria-label="Toggle backlinks panel" aria-expanded={isOpen}>
      <div class="flex items-center justify-between">
        <span class="text-sm text-muted-foreground">
          Mentioned by: ({loading ? '...' : backlinks.length}
          {backlinks.length === 1 ? 'node' : 'nodes'})
        </span>

        <svg
          viewBox="0 0 16 16"
          fill="none"
          class="h-4 w-4 text-muted-foreground transition-transform duration-200"
          class:rotate-180={isOpen}
          aria-hidden="true"
        >
          <path
            d="M4 10l4-4 4 4"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
      </div>
    </Collapsible.Trigger>

    <!-- Content expands above trigger -->
    <Collapsible.Content class="backlinks-content-wrapper">
      <div class="backlinks-content">
        {#if loading}
          <div class="px-4 py-3 text-sm text-muted-foreground">Loading backlinks...</div>
        {:else if error}
          <div class="px-4 py-3 text-sm text-destructive">{error}</div>
        {:else if backlinks.length > 0}
          <ul class="flex flex-col gap-1">
            {#each backlinks as backlink}
              <li>
                <a
                  href="nodespace://{backlink.id}"
                  class="flex items-center gap-2 px-4 py-2 text-sm no-underline hover:bg-muted/50 transition-colors rounded"
                >
                  <Icon name={getNodeIcon(backlink.nodeType)} size={16} />
                  <span class="flex-1 truncate">
                    {backlink.content || backlink.id}
                  </span>
                </a>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </Collapsible.Content>
  </Collapsible.Root>
</div>

<style>
  .backlinks-panel-container {
    /* Push to bottom of flex parent and stay there */
    margin-top: auto;
    flex-shrink: 0;
    background: hsl(var(--background));
    display: flex;
    flex-direction: column-reverse; /* Reverse: trigger renders at bottom, content above */
    padding: 0 var(--viewer-padding-horizontal);
  }

  .backlinks-panel-container :global([data-collapsible-trigger]) {
    width: calc(100% + (var(--viewer-padding-horizontal) * 2)); /* Extend to container edges */
    margin-left: calc(-1 * var(--viewer-padding-horizontal)); /* Break out of container padding */
    padding: 0.75rem var(--viewer-padding-horizontal); /* Match container padding for content alignment */
    border-top: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    cursor: pointer;
    transition: opacity 0.2s;
  }

  .backlinks-panel-container :global([data-collapsible-trigger]:hover) {
    opacity: 0.8;
  }

  .backlinks-content {
    height: var(--backlinks-panel-height);
    overflow-y: auto;
    background: hsl(var(--background));
  }

  .rotate-180 {
    transform: rotate(180deg);
  }
</style>
