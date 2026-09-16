<script lang="ts">
  /**
   * Non-blocking "an update is available" banner. Renders only when the update
   * store says a newer version exists and the user hasn't dismissed it. "Download"
   * opens the release page (the app ships no in-app installer — see
   * `update-status.svelte.ts`); dismissal is per-version.
   */
  import { updateStatus } from '$lib/stores/update-status.svelte';

  let downloading = $state(false);

  async function onDownload() {
    downloading = true;
    try {
      await updateStatus.download();
    } finally {
      downloading = false;
    }
  }
</script>

{#if updateStatus.showBanner}
  <div class="update-banner" role="status" aria-live="polite">
    <span class="msg">
      NodeSpace <strong>{updateStatus.latest}</strong> is available
      <span class="cur">(you have {updateStatus.current})</span>
    </span>
    <span class="actions">
      <button class="download" onclick={onDownload} disabled={downloading}>
        {downloading ? 'Opening…' : 'Download'}
      </button>
      <button class="dismiss" onclick={() => updateStatus.dismiss()} aria-label="Dismiss update notice">
        Later
      </button>
    </span>
  </div>
{/if}

<style>
  .update-banner {
    position: fixed;
    top: 8px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 1000;
    display: flex;
    align-items: center;
    gap: 14px;
    max-width: calc(100vw - 32px);
    padding: 8px 12px;
    border-radius: var(--radius);
    background: hsl(var(--popover));
    color: hsl(var(--popover-foreground));
    border: 1px solid hsl(var(--border));
    font-size: 13px;
  }
  .msg {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cur {
    color: hsl(var(--muted-foreground));
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
  }
  button {
    font: inherit;
    padding: 4px 12px;
    border-radius: 6px;
    cursor: pointer;
    border: 1px solid transparent;
  }
  .download {
    background: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
  }
  .download:hover:not(:disabled) {
    filter: brightness(1.08);
  }
  .download:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .dismiss {
    background: transparent;
    color: hsl(var(--muted-foreground));
    border-color: hsl(var(--border));
  }
  .dismiss:hover {
    color: hsl(var(--foreground));
  }
</style>
