<script lang="ts">
  import { statusBar } from '$lib/stores/status-bar';
  import { fade } from 'svelte/transition';

  $: state = $statusBar;
</script>

{#if state.enabled}
  <div
    class="status-bar"
    class:success={state.type === 'success'}
    class:error={state.type === 'error'}
    transition:fade={{ duration: 150 }}
  >
    <!-- Left side: stale nodes count for vector indexing -->
    <div class="left-content">
      {#if state.staleNodesCount > 0}
        <span class="stale-count">{state.staleNodesCount} nodes queued for vector indexing</span>
      {/if}
    </div>

    <div class="spacer"></div>

    <!-- Right side: status message and progress -->
    <div class="right-content">
      {#if state.message}
        <span class="message">{state.message}</span>
      {/if}
      {#if state.progress !== undefined}
        <div class="progress-bar">
          <div class="progress-fill" style="width: {state.progress}%"></div>
        </div>
      {/if}
    </div>
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

  .left-content {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .stale-count {
    opacity: 0.8;
  }

  .spacer {
    flex: 1;
  }

  .right-content {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .message {
    flex-shrink: 0;
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
    transition: width 0.2s ease;
  }
</style>
