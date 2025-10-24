<!--
  BacklinksPanel - Collapsible panel showing container-level backlinks

  Design matches Schema-Driven Property Forms:
  - Simple collapsible trigger with count and chevron
  - List of node links with icons (like node tree, but without children)
  - Clean, minimal styling
-->

<script lang="ts">
  import { Collapsible } from 'bits-ui';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import Icon, { type IconName } from '$lib/design/icons/icon.svelte';
  import { onMount } from 'svelte';
  import type { Node } from '$lib/types/node';

  let { nodeId }: { nodeId: string } = $props();

  let backlinks = $state<Node[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let isOpen = $state(false);

  onMount(async () => {
    try {
      // Get container IDs that mention this node
      const containerIds = await backendAdapter.getMentioningContainers(nodeId);

      // Fetch full node data for each container explicitly
      // (don't rely on cache - ensure we always have the latest data)
      const nodes = await Promise.all(containerIds.map((id) => backendAdapter.getNode(id)));

      // Filter out null values (nodes that no longer exist)
      backlinks = nodes.filter((node): node is Node => node !== null);
    } catch (err) {
      console.error('Failed to load backlinks:', err);
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

<!-- Add top border and spacing to separate from children section -->
<div class="border-t pt-2 mt-4">
  <Collapsible.Root bind:open={isOpen}>
    <Collapsible.Trigger
      class="flex w-full items-center justify-between py-3 font-medium transition-all hover:opacity-80"
    >
      <div class="flex items-center gap-3">
        <span class="text-sm text-muted-foreground">
          Mentioned by: ({loading ? '...' : backlinks.length}
          {backlinks.length === 1 ? 'node' : 'nodes'})
        </span>
      </div>

      <div class="flex items-center gap-2">
        <svg
          viewBox="0 0 16 16"
          fill="none"
          class="h-4 w-4 text-muted-foreground transition-transform duration-200"
          class:rotate-180={isOpen}
        >
          <path
            d="M4 6l4 4 4-4"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
      </div>
    </Collapsible.Trigger>

    <Collapsible.Content>
      <div class="pb-4">
        {#if loading}
          <div class="px-2 py-2 text-sm text-muted-foreground">Loading backlinks...</div>
        {:else if error}
          <div class="px-2 py-2 text-sm text-destructive">{error}</div>
        {:else if backlinks.length > 0}
          <ul class="flex flex-col gap-1">
            {#each backlinks as backlink}
              <li>
                <a
                  href="nodespace://{backlink.id}"
                  class="flex items-center gap-2 px-2 py-1.5 text-sm no-underline"
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
  .rotate-180 {
    transform: rotate(180deg);
  }

  /* Reset button styles to match demo */
  :global([data-collapsible-trigger]) {
    background: none;
    border: none;
    cursor: pointer;
  }
</style>
