<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { pluginRegistry } from '$lib/plugins/plugin-registry';
  import { createLogger } from '$lib/utils/logger';
  import type { Node } from '$lib/types/node';
  import { toError } from '$lib/types/errors';

  const log = createLogger('SearchPane');

  const SEARCH_LIMIT = 25;
  const DEBOUNCE_MS = 300;

  let query = $state('');
  let results = $state<Node[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let hasSearched = $state(false);

  let inputEl: HTMLInputElement | null = $state(null);
  let debounceTimer: ReturnType<typeof setTimeout> | undefined;
  // Monotonic token so a slow response from an earlier query can never clobber
  // the results of a later one.
  let requestToken = 0;

  onMount(() => inputEl?.focus());
  onDestroy(() => clearTimeout(debounceTimer));

  async function runSearch(term: string) {
    const trimmed = term.trim();
    if (!trimmed) {
      requestToken++; // cancel any in-flight response
      results = [];
      hasSearched = false;
      error = null;
      loading = false;
      return;
    }

    const token = ++requestToken;
    loading = true;
    error = null;

    try {
      const found = await invoke<Node[]>('search_roots', {
        params: { query: trimmed, limit: SEARCH_LIMIT }
      });
      if (token !== requestToken) return; // superseded by a newer query
      results = found;
      hasSearched = true;
    } catch (e) {
      if (token !== requestToken) return;
      log.error('search_roots failed', e);
      error = toError(e).message;
      results = [];
      hasSearched = true;
    } finally {
      if (token === requestToken) loading = false;
    }
  }

  function onInput() {
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => runSearch(query), DEBOUNCE_MS);
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter') {
      clearTimeout(debounceTimer);
      runSearch(query);
    }
  }

  function resultTitle(node: Node): string {
    const pluginTitle = pluginRegistry.getNodeTitle(node);
    if (pluginTitle && pluginTitle.trim()) return pluginTitle.trim();
    const firstLine = node.content?.split('\n').find((line) => line.trim());
    return firstLine?.trim() || '(untitled)';
  }

  function openResult(node: Node) {
    getNavigationService().focusOrOpenNode(node.id, { nodeType: node.nodeType });
  }
</script>

<div class="search-pane">
  <div class="search-header">
    <input
      bind:this={inputEl}
      class="search-input"
      type="text"
      placeholder="Search nodes…"
      bind:value={query}
      oninput={onInput}
      onkeydown={onKeydown}
    />
  </div>

  <div class="search-results">
    {#if loading}
      <div class="search-status">Searching…</div>
    {:else if error}
      <div class="search-status search-error">{error}</div>
    {:else if hasSearched && results.length === 0}
      <div class="search-status">No results for “{query.trim()}”.</div>
    {:else if results.length > 0}
      <ul class="result-list">
        {#each results as node (node.id)}
          <li>
            <button type="button" class="result-item" onclick={() => openResult(node)}>
              <span class="result-title">{resultTitle(node)}</span>
              <span class="result-type">{node.nodeType}</span>
            </button>
          </li>
        {/each}
      </ul>
    {:else}
      <div class="search-status">Type to search your nodes.</div>
    {/if}
  </div>
</div>

<style>
  .search-pane {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: hsl(var(--background));
  }

  .search-header {
    padding: 1.5rem 2rem 0.75rem;
  }

  .search-input {
    width: 100%;
    box-sizing: border-box;
    padding: 0.6rem 0.9rem;
    font-size: 1rem;
    color: hsl(var(--foreground));
    background: hsl(var(--muted));
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    outline: none;
  }

  .search-input:focus {
    border-color: hsl(var(--ring));
  }

  .search-results {
    flex: 1;
    overflow-y: auto;
    padding: 0.5rem 1rem 1.5rem;
  }

  .search-status {
    padding: 1rem 1rem;
    color: hsl(var(--muted-foreground));
    font-size: 0.9rem;
  }

  .search-error {
    color: hsl(var(--destructive));
  }

  .result-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .result-item {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.75rem;
    width: 100%;
    text-align: left;
    padding: 0.55rem 0.75rem;
    background: transparent;
    border: none;
    border-radius: 0.375rem;
    cursor: pointer;
    color: hsl(var(--foreground));
  }

  .result-item:hover {
    background: hsl(var(--muted));
  }

  .result-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .result-type {
    flex-shrink: 0;
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
  }
</style>
