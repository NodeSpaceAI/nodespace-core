<!--
  RelationshipField — a single-valued relationship edited as a property field.

  Renders one promoted group (see `isFormPromoted`): the related node when there
  is one, and a type-ahead search over nodes of the group's target type when the
  user sets or changes it. The search matches each candidate's stored title,
  which is its `title_template` rendering where the type declares one — so a
  person is found by "Sam Lee", not by a raw first- or last-name field.

  Writes go through the same orientation rule as the Relationships modal
  (`addEdge`/`removeEdge` → `resolveEdgeEndpoints`), so an inbound group is
  written from the declaring side under its forward name. Picking a node when
  one is already set is a plain create: the daemon replaces the existing edge
  into a `one` end rather than rejecting the second, so there is no
  remove-then-add window in which the field is empty.

  The caller renders the label (so each form keeps its own label styling) and
  passes `fieldId` to associate it; `onChanged` re-fetches the relationships
  view after a write so the field shows what the store now holds.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import { Input } from '$lib/components/ui/input';
  import LoaderIcon from '@lucide/svelte/icons/loader-circle';
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link';
  import XIcon from '@lucide/svelte/icons/x';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { addEdge, removeEdge, searchTargets } from '$lib/services/relationship-viewer-service';
  import {
    filterUnlinkedTargets,
    type RelationshipGroupView
  } from '$lib/services/relationship-grouping';
  import type { Node } from '$lib/types';

  const log = createLogger('RelationshipField');

  let {
    nodeId,
    group,
    fieldId,
    onChanged
  }: {
    nodeId: string;
    group: RelationshipGroupView;
    fieldId: string;
    onChanged: () => Promise<void>;
  } = $props();

  // A `one` end holds at most one edge; the daemon replaces rather than adds.
  const current = $derived(group.rows[0] ?? null);

  let editing = $state(false);
  let query = $state('');
  let results = $state<Node[]>([]);
  let highlighted = $state(0);
  let searching = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let inputEl = $state<HTMLInputElement | null>(null);
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  // Only the most recently STARTED search may write `results`.
  let searchGeneration = 0;

  const showSearch = $derived(editing || !current);
  const listboxId = $derived(`${fieldId}-options`);
  const listOpen = $derived(showSearch && query.trim().length > 0);

  function nodeLabel(node: Node): string {
    return node.title?.trim() || node.content?.trim() || node.id;
  }

  async function startEditing() {
    editing = true;
    error = null;
    // The input only exists once `editing` has rendered it.
    await tick();
    inputEl?.focus();
  }

  function stopEditing() {
    editing = false;
    query = '';
    results = [];
    highlighted = 0;
    searching = false;
    searchGeneration++;
    if (searchTimer) clearTimeout(searchTimer);
  }

  function onInput(value: string) {
    query = value;
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void runSearch(), 200);
  }

  async function runSearch() {
    const generation = ++searchGeneration;
    const q = query.trim();
    if (!q) {
      results = [];
      searching = false;
      return;
    }
    searching = true;
    try {
      const found = await searchTargets(group.targetType, q);
      if (generation !== searchGeneration) return;
      // The current value is not a choice: picking it would be a no-op write.
      results = filterUnlinkedTargets(group, found);
      highlighted = 0;
    } catch (err) {
      if (generation !== searchGeneration) return;
      log.error('Relationship target search failed', err);
      results = [];
    } finally {
      if (generation === searchGeneration) searching = false;
    }
  }

  async function write(fn: () => Promise<void>) {
    busy = true;
    error = null;
    try {
      await fn();
      await onChanged();
      stopEditing();
    } catch (err) {
      log.error('Relationship field write failed', err);
      error = toError(err).message;
    } finally {
      busy = false;
    }
  }

  function select(node: Node) {
    void write(() => addEdge(nodeId, group, node.id));
  }

  function clear() {
    const row = current;
    if (!row) return;
    void write(() => removeEdge(nodeId, group, row.id));
  }

  function openCurrent() {
    const row = current;
    if (!row) return;
    getNavigationService().focusOrOpenNode(row.id, { nodeType: row.nodeType, title: row.label });
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      stopEditing();
    } else if (event.key === 'ArrowDown' && results.length > 0) {
      event.preventDefault();
      highlighted = (highlighted + 1) % results.length;
    } else if (event.key === 'ArrowUp' && results.length > 0) {
      event.preventDefault();
      highlighted = (highlighted - 1 + results.length) % results.length;
    } else if (event.key === 'Enter' && results[highlighted]) {
      event.preventDefault();
      select(results[highlighted]);
    }
  }
</script>

<div class="relative">
  {#if showSearch}
    <Input
      id={fieldId}
      bind:ref={inputEl}
      role="combobox"
      aria-expanded={listOpen}
      aria-controls={listboxId}
      aria-autocomplete="list"
      autocomplete="off"
      placeholder="Search {group.targetType ?? 'nodes'}…"
      value={query}
      disabled={busy}
      oninput={(event) => onInput(event.currentTarget.value)}
      onkeydown={onKeydown}
      onblur={() => {
        // Options take the click on mousedown (see below), so a blur here is
        // the user leaving the field — drop back to the current value.
        if (current) stopEditing();
      }}
    />
    {#if listOpen}
      <ul
        id={listboxId}
        role="listbox"
        class="absolute z-50 mt-1 max-h-60 w-full overflow-y-auto rounded-md border bg-popover p-1 text-sm text-popover-foreground shadow-md"
      >
        {#if searching && results.length === 0}
          <li class="flex items-center gap-2 px-2 py-1.5 text-muted-foreground">
            <LoaderIcon class="size-3 animate-spin" />
            <span>Searching…</span>
          </li>
        {:else if results.length === 0}
          <li class="px-2 py-1.5 text-muted-foreground">No matches.</li>
        {:else}
          {#each results as node, index (node.id)}
            <li
              role="option"
              aria-selected={index === highlighted}
              class="cursor-pointer truncate rounded-sm px-2 py-1.5"
              class:bg-accent={index === highlighted}
              class:text-accent-foreground={index === highlighted}
              onmouseenter={() => (highlighted = index)}
              onmousedown={(event) => {
                // Select before the input's blur can close the list.
                event.preventDefault();
                select(node);
              }}
            >
              {nodeLabel(node)}
            </li>
          {/each}
        {/if}
      </ul>
    {/if}
  {:else if current}
    <div
      class="flex h-9 w-full items-center rounded-md border border-input bg-transparent text-sm shadow-xs"
    >
      <button
        id={fieldId}
        type="button"
        class="min-w-0 flex-1 truncate px-3 text-left font-medium"
        title="Change {group.label.toLowerCase()}"
        disabled={busy}
        onclick={startEditing}
      >
        {current.label}
      </button>
      <button
        type="button"
        class="px-1.5 text-muted-foreground hover:text-foreground"
        aria-label="Open {current.label}"
        onclick={openCurrent}
      >
        <ExternalLinkIcon class="size-3.5" />
      </button>
      {#if !group.required}
        <button
          type="button"
          class="pr-2.5 pl-1.5 text-muted-foreground hover:text-foreground"
          aria-label="Clear {group.label.toLowerCase()}"
          disabled={busy}
          onclick={clear}
        >
          <XIcon class="size-3.5" />
        </button>
      {/if}
    </div>
  {/if}
  {#if error}
    <p class="mt-1 text-xs text-destructive">{error}</p>
  {/if}
</div>
