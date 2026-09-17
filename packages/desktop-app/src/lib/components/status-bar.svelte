<script lang="ts">
  import { statusBar } from '$lib/stores/status-bar.svelte';

  const state = $derived(statusBar.state);
</script>

{#if state.enabled}
  <div
    class="status-bar"
    class:success={state.type === 'success'}
    class:error={state.type === 'error'}
  >
    {#if state.message}
      <span class="message" title={state.message}>{state.message}</span>
    {/if}
    {#if state.progress !== undefined}
      <div class="progress-bar">
        <div class="progress-fill" style="width: {state.progress}%"></div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .status-bar {
    /* No longer fixed - participates in flex layout to push content up */
    flex-shrink: 0;
    height: 24px;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px;
    background: hsl(var(--muted));
    border-top: 1px solid hsl(var(--border));
    font-size: 12px;
    color: hsl(var(--muted-foreground));
  }

  .status-bar.success {
    background: hsl(var(--success) / 0.1);
    color: hsl(var(--success));
  }

  .status-bar.error {
    background: hsl(var(--destructive) / 0.1);
    color: hsl(var(--destructive));
  }

  .message {
    /* A long message (e.g. an error naming multiple install links) must fit
       the fixed-height bar rather than overflow past the window edge and
       clip its own actionable content. `min-width: 0` is required for a
       flex child to shrink below its content's intrinsic width at all —
       flex items default to `min-width: auto`, which ignores `overflow`
       otherwise. The full text is still reachable via the `title` tooltip. */
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .progress-bar {
    width: 200px;
    height: 4px;
    background: hsl(var(--border));
    border-radius: 2px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: hsl(var(--primary));
  }
</style>
