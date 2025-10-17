<script>
  import { onMount } from 'svelte';
  import '../app.css';
  import AppShell from '$lib/components/layout/app-shell.svelte';
  import { PersistenceCoordinator } from '$lib/services/persistence-coordinator.svelte';

  // Flush pending saves on window close to prevent data loss
  onMount(() => {
    // Check if running in Tauri (desktop app)
    // @ts-expect-error - Tauri internal property not in Window type
    if (window.__TAURI_INTERNALS__) {
      // Import Tauri APIs only if we're in Tauri
      (async () => {
        const { getCurrentWindow } = await import('@tauri-apps/api/window');
        const appWindow = getCurrentWindow();

        // Listen for Tauri window close event
        await appWindow.onCloseRequested(async () => {
          // Flush all pending debounced operations immediately
          PersistenceCoordinator.getInstance().flushPending();
        });
      })();
    } else {
      // Running in browser - use beforeunload
      const handleBeforeUnload = () => {
        PersistenceCoordinator.getInstance().flushPending();
      };

      window.addEventListener('beforeunload', handleBeforeUnload);
      return () => window.removeEventListener('beforeunload', handleBeforeUnload);
    }
  });
</script>

<AppShell>
  <slot />
</AppShell>
