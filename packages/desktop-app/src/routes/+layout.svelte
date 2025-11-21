<script lang="ts">
  import { onMount } from 'svelte';
  import '../app.css';
  import AppShell from '$lib/components/layout/app-shell.svelte';
  import { initializeSchemaPluginSystem } from '$lib/plugins/schema-plugin-loader';
  import { initializeTauriSyncListeners } from '$lib/services/tauri-sync-listener';
  import { sharedNodeStore } from '$lib/services/shared-node-store';

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

  // Initialize Tauri LIVE SELECT event listeners for real-time synchronization
  onMount(async () => {
    try {
      await initializeTauriSyncListeners();
    } catch (error) {
      console.warn('[App Layout] Tauri sync listeners failed to initialize:', error);
      // Don't block app startup if sync listeners fail
      // App will continue to work, just without real-time updates
    }
  });

  // Flush pending saves on window close to prevent data loss
  onMount(() => {
    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      // Check if there are pending writes
      if (sharedNodeStore.hasPendingWrites()) {
        // Try to flush pending operations
        // Note: beforeunload has limited async support, but we try our best
        event.preventDefault();
        // returnValue assignment is required for Chrome
        event.returnValue = '';

        // Flush pending operations synchronously if possible
        // Note: async operations may not complete in beforeunload, but we try
        sharedNodeStore.flushAllPending();
      }
    };

    window.addEventListener('beforeunload', handleBeforeUnload);

    return () => {
      window.removeEventListener('beforeunload', handleBeforeUnload);
    };
  });
</script>

<AppShell>
  <slot />
</AppShell>
