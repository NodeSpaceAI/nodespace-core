<!--
  Other daemon banner.

  Shown instead of the generic "background service is not running" banner when
  another NodeSpace daemon, outside this app's service registration (a
  Homebrew service, or one started by hand), holds the socket, so this app's
  own daemon could not start (ADR-084 §4.3). The user stops that daemon and
  chooses Retry, which starts this app's daemon again.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { getOtherDaemon, retryDaemonStart } from '$lib/services/daemon-status';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('OtherDaemonBanner');

  /** The extension ids the other daemon reported; empty or null when none. */
  let executable = $state<string | null>(null);
  let retrying = $state(false);
  let error = $state<string | null>(null);

  function readExecutable() {
    getOtherDaemon()
      .then((path) => {
        executable = path;
      })
      .catch((err) => log.warn('Failed to read the other background service', err));
  }

  onMount(readExecutable);

  async function retry() {
    retrying = true;
    error = null;
    try {
      // A healthy status unmounts this banner. If a daemon outside this app's
      // registration still holds the socket, it may be a different one now.
      if ((await retryDaemonStart()) === 'other_daemon') readExecutable();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
      log.error('Failed to start the background service again', err);
    } finally {
      retrying = false;
    }
  }
</script>

<div class="other-daemon-banner" role="alert">
  <div class="message">
    <span>
      {#if executable}
        Another NodeSpace background service is running ({executable}). Stop it, then choose Retry.
      {:else}
        Another NodeSpace background service is running. Stop it, then choose Retry.
      {/if}
    </span>
    {#if error}
      <span class="error">Couldn't start the background service: {error}</span>
    {/if}
  </div>
  <button disabled={retrying} onclick={retry}>{retrying ? 'Retrying…' : 'Retry'}</button>
</div>

<style>
  .other-daemon-banner {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 1rem;
    padding: 0.5rem 1rem;
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
    overflow-wrap: anywhere;
  }

  .error {
    font-weight: 600;
  }

  button {
    flex-shrink: 0;
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

  button:disabled {
    cursor: default;
    opacity: 0.6;
  }
</style>
