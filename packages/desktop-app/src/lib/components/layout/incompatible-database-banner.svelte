<!--
  Incompatible database banner.

  Shown instead of the generic "background service is not running" banner when
  the daemon stopped because its database was created by a different version
  of NodeSpace. NodeSpace does not migrate databases, so Retry cannot help: the
  way forward is to move that database aside and start fresh. This banner says
  so and offers exactly that, behind a confirm step that states where the old
  file goes. Nothing is deleted.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    getIncompatibleDatabase,
    resetIncompatibleDatabase,
    type IncompatibleDatabase
  } from '$lib/services/daemon-status';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('IncompatibleDatabaseBanner');

  let record = $state<IncompatibleDatabase | null>(null);
  let confirming = $state(false);
  let resetting = $state(false);
  let error = $state<string | null>(null);

  onMount(() => {
    getIncompatibleDatabase()
      .then((r) => {
        record = r;
      })
      .catch((err) => log.warn('Failed to read the incompatible database record', err));
  });

  async function moveAside() {
    resetting = true;
    error = null;
    try {
      const result = await resetIncompatibleDatabase();
      log.info(
        result.backupPath
          ? `Moved the incompatible database aside to ${result.backupPath}`
          : 'The incompatible database was already gone; started fresh'
      );
      // A healthy status unmounts this banner. Anything else keeps it up with
      // the confirm step closed, reflecting whatever the daemon reports now.
      confirming = false;
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
      log.error('Failed to reset the incompatible database', err);
    } finally {
      resetting = false;
    }
  }
</script>

<div class="incompatible-database-banner" role="alert">
  <div class="message">
    <strong>This database was created by a different version of NodeSpace and can&apos;t be opened.</strong>
    <span>
      NodeSpace doesn't convert databases between versions. Move it aside to start with a fresh
      one — the old file is kept as a backup next to it, not deleted.
    </span>
    {#if record}
      <span class="path" title={record.detail}>{record.databasePath}</span>
    {/if}
    {#if error}
      <span class="error">Couldn't move the database aside: {error}</span>
    {/if}
  </div>
  <div class="actions">
    {#if confirming}
      <button class="primary" disabled={resetting} onclick={moveAside}>
        {resetting ? 'Moving aside…' : 'Move aside and start fresh'}
      </button>
      <button disabled={resetting} onclick={() => (confirming = false)}>Cancel</button>
    {:else}
      <button onclick={() => (confirming = true)}>Start fresh…</button>
    {/if}
  </div>
</div>

<style>
  .incompatible-database-banner {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 1rem;
    padding: 0.625rem 1rem;
    background: hsl(var(--destructive) / 0.1);
    border-bottom: 1px solid hsl(var(--destructive) / 0.4);
    /* Same reasoning as app-shell's daemon-error-banner: on a tint, the state
       color itself is the readable text color, not --destructive-foreground. */
    color: hsl(var(--destructive));
    font-size: 0.875rem;
    z-index: 100;
  }

  .message {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
    min-width: 0;
  }

  .path {
    font-family: 'SF Mono', Monaco, 'Cascadia Code', 'Roboto Mono', Consolas, monospace;
    font-size: 0.75rem;
    opacity: 0.85;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .error {
    font-weight: 600;
  }

  .actions {
    display: flex;
    gap: 0.5rem;
    flex-shrink: 0;
  }

  button {
    padding: 0.2rem 0.75rem;
    border-radius: 4px;
    border: 1px solid hsl(var(--destructive) / 0.6);
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-size: 0.8rem;
  }

  button:hover:not(:disabled) {
    background: hsl(var(--destructive) / 0.2);
  }

  button.primary {
    background: hsl(var(--destructive));
    color: hsl(var(--destructive-foreground));
  }

  button.primary:hover:not(:disabled) {
    background: hsl(var(--destructive) / 0.9);
  }

  button:disabled {
    cursor: default;
    opacity: 0.6;
  }
</style>
