<script lang="ts">
  import { onMount } from 'svelte';
  import { conflictNotifications } from '$lib/stores/conflict-notifications.svelte';

  const AUTO_DISMISS_MS = 4000;

  let timers = new Map<string, ReturnType<typeof setTimeout>>();

  function scheduleAutoDismiss(id: string): void {
    if (timers.has(id)) return;
    const t = setTimeout(() => {
      conflictNotifications.dismiss(id);
      timers.delete(id);
    }, AUTO_DISMISS_MS);
    timers.set(id, t);
  }

  function dismiss(id: string): void {
    const t = timers.get(id);
    if (t !== undefined) {
      clearTimeout(t);
      timers.delete(id);
    }
    conflictNotifications.dismiss(id);
  }

  $effect(() => {
    const activeIds = new Set(conflictNotifications.notifications.map((n) => n.id));
    for (const [id, t] of timers) {
      if (!activeIds.has(id)) {
        clearTimeout(t);
        timers.delete(id);
      }
    }
    for (const n of conflictNotifications.notifications) {
      scheduleAutoDismiss(n.id);
    }
  });

  onMount(() => {
    return () => {
      for (const t of timers.values()) {
        clearTimeout(t);
      }
      timers.clear();
    };
  });
</script>

{#if conflictNotifications.notifications.length > 0}
  <div class="conflict-toast-container" aria-live="polite" aria-label="Conflict notifications">
    {#each conflictNotifications.notifications as notification (notification.id)}
      <div class="conflict-toast" role="status">
        <span class="conflict-toast-icon" aria-hidden="true">⚠️</span>
        <span class="conflict-toast-message">{notification.message}</span>
        <button
          class="conflict-toast-dismiss"
          onclick={() => dismiss(notification.id)}
          aria-label="Dismiss notification"
        >
          ×
        </button>
      </div>
    {/each}
  </div>
{/if}

<style>
  .conflict-toast-container {
    position: fixed;
    bottom: 48px; /* above status bar */
    right: 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 900;
    pointer-events: none;
  }

  .conflict-toast {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    background-color: hsl(var(--popover));
    color: hsl(var(--popover-foreground));
    border: 1px solid hsl(var(--warning));
    border-radius: 6px;
    font-size: 13px;
    pointer-events: all;
    animation: toast-in 0.2s ease-out;
  }

  @keyframes toast-in {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .conflict-toast {
      animation: none;
    }
  }

  .conflict-toast-icon {
    color: hsl(var(--warning));
    flex-shrink: 0;
    font-size: 14px;
  }

  .conflict-toast-message {
    flex: 1;
  }

  .conflict-toast-dismiss {
    background: none;
    border: none;
    color: hsl(var(--muted-foreground));
    cursor: pointer;
    font-size: 18px;
    line-height: 1;
    padding: 0 2px;
    flex-shrink: 0;
  }

  .conflict-toast-dismiss:hover {
    color: hsl(var(--foreground));
  }
</style>
