<script lang="ts">
  import { onMount } from 'svelte';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import {
    seedKindLabel,
    seedUpdateKey,
    seedUpdatesStore,
    type PendingSeedUpdate,
    type PendingSeedUpdateDetail
  } from '$lib/stores/seed-updates.svelte';
  import { toError } from '$lib/types/errors';

  const updates = $derived(seedUpdatesStore.updates);

  /** The update whose two versions are shown, by `seedUpdateKey`. */
  let reviewingKey = $state<string | null>(null);
  let detail = $state<PendingSeedUpdateDetail | null>(null);
  let takeTarget = $state<PendingSeedUpdate | null>(null);
  let takeDialogOpen = $state(false);
  let working = $state(false);
  let error = $state<string | null>(null);

  onMount(() => {
    seedUpdatesStore.load();
  });

  function partLabel(update: PendingSeedUpdate): string {
    return update.aspect === 'guidance' ? 'Body' : 'Name and settings';
  }

  function lastEdited(update: PendingSeedUpdate): string {
    const date = new Date(update.lastEditedAt);
    return Number.isNaN(date.getTime()) ? update.lastEditedAt : date.toLocaleString();
  }

  async function toggleReview(update: PendingSeedUpdate) {
    const key = seedUpdateKey(update);
    if (reviewingKey === key) {
      reviewingKey = null;
      detail = null;
      return;
    }
    reviewingKey = key;
    detail = null;
    error = null;
    try {
      const loaded = await seedUpdatesStore.detail(update);
      // A second row may have been opened while this one loaded.
      if (reviewingKey === key) detail = loaded;
    } catch (e) {
      if (reviewingKey === key) {
        reviewingKey = null;
        error = toError(e).message;
      }
    }
  }

  async function settle(update: PendingSeedUpdate, action: () => Promise<void>) {
    working = true;
    error = null;
    try {
      await action();
      if (reviewingKey === seedUpdateKey(update)) {
        reviewingKey = null;
        detail = null;
      }
    } catch (e) {
      error = toError(e).message;
    } finally {
      working = false;
    }
  }

  function keepMine(update: PendingSeedUpdate) {
    return settle(update, () => seedUpdatesStore.keepMine(update));
  }

  function startTake(update: PendingSeedUpdate) {
    takeTarget = update;
    takeDialogOpen = true;
  }

  async function confirmTake() {
    const update = takeTarget;
    takeDialogOpen = false;
    takeTarget = null;
    if (update) {
      await settle(update, () => seedUpdatesStore.takeShipped(update));
    }
  }
</script>

<div class="max-w-[820px]">
  <h2 class="text-foreground mb-2 text-xl font-semibold">Built-in updates</h2>
  <p class="text-muted-foreground mb-6 text-sm leading-relaxed">
    NodeSpace ships skills, plays, saved queries and other built-in items that you can edit. When a
    newer version of one you edited ships, yours is kept and the new one waits here. Nothing is
    replaced until you choose. Keeping yours settles it until the shipped version changes again.
  </p>

  {#if error}
    <div
      class="border-destructive/40 bg-destructive/10 text-destructive mb-4 rounded-[var(--radius)] border px-3 py-2 text-sm"
      role="alert"
    >
      {error}
    </div>
  {/if}

  <div class="flex flex-col gap-2">
    {#each updates as update (seedUpdateKey(update))}
      {@const key = seedUpdateKey(update)}
      <div
        class="border-border bg-muted/40 rounded-[var(--radius)] border p-3"
        data-testid="seed-update"
      >
        <div class="flex items-start justify-between gap-4">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span class="text-foreground truncate font-medium">{update.title}</span>
              <span class="text-muted-foreground bg-muted rounded px-1.5 py-0.5 text-xs">
                {seedKindLabel(update.nodeType)}
              </span>
            </div>
            <div class="text-muted-foreground mt-1 text-xs">
              {partLabel(update)} · you last edited it {lastEdited(update)}
            </div>
            {#if !update.shippedAvailable}
              <div class="text-muted-foreground mt-1 text-xs">
                This version of NodeSpace does not include the shipped version, so it can only be
                kept.
              </div>
            {/if}
          </div>

          <div class="flex shrink-0 gap-1.5">
            <Button
              variant="ghost"
              size="sm"
              aria-expanded={reviewingKey === key}
              disabled={!update.shippedAvailable}
              onclick={() => toggleReview(update)}
            >
              {reviewingKey === key ? 'Hide' : 'Compare'}
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={working}
              onclick={() => keepMine(update)}
            >
              Keep mine
            </Button>
            <Button
              variant="default"
              size="sm"
              disabled={working || !update.shippedAvailable}
              onclick={() => startTake(update)}
            >
              Take shipped…
            </Button>
          </div>
        </div>

        {#if reviewingKey === key}
          {#if detail}
            <div class="mt-3 grid grid-cols-1 gap-3 md:grid-cols-2">
              <div class="min-w-0">
                <div class="text-muted-foreground mb-1 text-xs font-medium">Shipped</div>
                <pre class="version-text" data-testid="seed-update-shipped">{detail.shipped}</pre>
              </div>
              <div class="min-w-0">
                <div class="text-muted-foreground mb-1 text-xs font-medium">Yours</div>
                <pre class="version-text" data-testid="seed-update-yours">{detail.yours}</pre>
              </div>
            </div>
          {:else}
            <div class="text-muted-foreground mt-3 text-sm">Loading both versions…</div>
          {/if}
        {/if}
      </div>
    {/each}

    {#if updates.length === 0 && seedUpdatesStore.loaded}
      <div class="text-muted-foreground text-sm">
        Nothing to review. Built-in items you have edited are up to date with your choices.
      </div>
    {/if}
  </div>
</div>

<Dialog.Root bind:open={takeDialogOpen}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>Take the shipped version</Dialog.Title>
      <Dialog.Description>
        Replace your {takeTarget?.aspect === 'guidance' ? 'body' : 'name and settings'} of
        <strong>{takeTarget?.title}</strong> with the shipped version? Your edit to it is discarded and
        cannot be restored.
      </Dialog.Description>
    </Dialog.Header>
    <Dialog.Footer>
      <Button variant="ghost" onclick={() => (takeDialogOpen = false)}>Cancel</Button>
      <Button variant="destructive" onclick={confirmTake}>Replace mine</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<style>
  .version-text {
    max-height: 24rem;
    overflow: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    border: 1px solid hsl(var(--border));
    border-radius: var(--radius);
    background: hsl(var(--background));
    padding: 0.5rem 0.75rem;
    font-size: 0.75rem;
    line-height: 1.5;
    color: hsl(var(--foreground));
  }
</style>
