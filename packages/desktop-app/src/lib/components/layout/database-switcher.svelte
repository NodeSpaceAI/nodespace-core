<script lang="ts">
  import { onMount } from 'svelte';
  import { databaseStore } from '$lib/stores/database.svelte';
  import { openSettings } from '$lib/utils/open-settings';
  import HardDrive from '@lucide/svelte/icons/hard-drive';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import Settings from '@lucide/svelte/icons/settings';

  // Read-only indicator of the current database. Switching + management now live
  // in Settings → Database (a database is bound to at most one tenant, so keeping
  // the switch out of this inline control avoids conflating "which database" with
  // "which tenant"); clicking opens that Settings section.
  const activeDatabase = $derived(databaseStore.activeDatabase);
  const activeName = $derived(activeDatabase?.name ?? 'Default Database');

  onMount(() => {
    databaseStore.load();
  });
</script>

<div class="db-switcher">
  <button
    class="db-trigger"
    aria-label="Open database settings (current: {activeName})"
    title="Manage databases in Settings"
    onclick={() => openSettings('database')}
  >
    {#if activeDatabase?.status === 'missing'}
      <TriangleAlert class="db-glyph db-glyph-missing" />
    {:else}
      <HardDrive class="db-glyph" />
    {/if}
    <span class="db-name">{activeName}</span>
    <Settings class="db-chevron" />
  </button>
</div>

<style>
  .db-switcher {
    margin: 0 -1rem 0.5rem;
    padding: 0 1rem;
  }

  :global(.db-trigger) {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    height: 40px;
    padding: 0 0.5rem;
    background: none;
    border: 1px solid hsl(var(--border));
    border-radius: var(--radius);
    color: hsl(var(--foreground));
    cursor: pointer;
    font-size: 0.875rem;
    font-weight: 500;
    text-align: left;
  }

  :global(.db-trigger:hover) {
    background: hsl(var(--border));
  }

  :global(.db-trigger .db-glyph) {
    width: 16px;
    height: 16px;
    flex-shrink: 0;
    color: hsl(var(--muted-foreground));
  }

  :global(.db-trigger .db-glyph-missing),
  :global(.db-glyph-missing) {
    color: hsl(var(--destructive));
  }

  .db-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  :global(.db-trigger .db-chevron) {
    width: 16px;
    height: 16px;
    flex-shrink: 0;
    color: hsl(var(--muted-foreground));
  }
</style>
