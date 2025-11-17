<script>
  import { onMount } from 'svelte';
  import '../app.css';
  import AppShell from '$lib/components/layout/app-shell.svelte';
  import { PersistenceCoordinator } from '$lib/services/persistence-coordinator.svelte';
  import { initializeSchemaPluginSystem } from '$lib/plugins/schema-plugin-loader';

  // Initialize schema plugin auto-registration system on mount
  onMount(async () => {
    // This registers existing custom entity schemas as plugins on app startup
    const result = await initializeSchemaPluginSystem();

    if (!result.success) {
      console.warn(
        `[App Layout] Custom entities unavailable: ${result.error}. ` +
          `Core functionality will work normally.`
      );
      // Don't block app startup on plugin registration failure
      // Custom entities will be unavailable but app remains functional
    } else {
      console.log(
        `[App Layout] Custom entity system ready (${result.registeredCount} entities loaded)`
      );
    }
  });

  // Flush pending saves on window close to prevent data loss
  onMount(() => {
    // Check if running in Tauri (desktop app)
    // Tauri adds __TAURI_INTERNALS__ to window at runtime
    if ('__TAURI_INTERNALS__' in window) {
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
