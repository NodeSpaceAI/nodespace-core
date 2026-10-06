<!--
  Foreign machine-wide registration notice.

  The launchd registration for every user of this Mac, under this app's service
  label, runs another NodeSpace product's daemon (ADR-084 §4.3). It starts that
  daemon at every login, and the app cannot remove it: the file is root-owned,
  and only that product's uninstaller removes it. The app keeps working, so the
  notice does not block anything, and it can be dismissed for this session.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { getForeignMachineWideRegistration } from '$lib/services/daemon-status';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('ForeignRegistrationNotice');

  let visible = $state(false);

  onMount(() => {
    getForeignMachineWideRegistration()
      .then((foreign) => {
        visible = foreign;
      })
      .catch((err) => log.warn('Failed to check the machine-wide service registration', err));
  });
</script>

{#if visible}
  <div class="foreign-registration-notice" role="status">
    <span>
      Another NodeSpace product's background service is registered for all users of this Mac. To
      remove it, run that product's uninstaller.
    </span>
    <button onclick={() => (visible = false)}>Dismiss</button>
  </div>
{/if}

<style>
  .foreign-registration-notice {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 1rem;
    padding: 0.5rem 1rem;
    background: hsl(var(--muted) / 0.5);
    border-bottom: 1px solid hsl(var(--border));
    color: hsl(var(--foreground));
    font-size: 0.875rem;
    z-index: 100;
  }

  button {
    flex-shrink: 0;
    padding: 0.2rem 0.75rem;
    border-radius: 4px;
    border: 1px solid hsl(var(--border));
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-size: 0.8rem;
  }

  button:hover {
    background: hsl(var(--muted));
  }
</style>
