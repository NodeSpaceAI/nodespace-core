<script lang="ts">
  import { onMount } from 'svelte';
  import '../app.css';
  import '$lib/styles/noderef.css';
  import AppShell from '$lib/components/layout/app-shell.svelte';
  import { initializeSchemaPluginSystem } from '$lib/plugins/schema-plugin-loader';
  import { initializeTauriSyncListeners } from '$lib/services/tauri-sync-listener';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { initializeApp } from '$lib/services/app-initialization';

  let isInitialized = false;

  // Initialize database first, then schema plugins, then sync listeners
  onMount(async () => {
    try {
      // Step 1: Initialize database and Tauri services
      await initializeApp();

      // Step 2: Initialize schema plugin auto-registration system
      // This must happen after database is ready
      try {
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
      } catch (error) {
        console.error('[App Layout] Schema plugin initialization failed:', error);
      }

      // Step 3: Initialize Tauri domain event listeners for real-time synchronization
      try {
        await initializeTauriSyncListeners();
      } catch (error) {
        console.warn('[App Layout] Tauri sync listeners failed to initialize:', error);
        // Don't block app startup if sync listeners fail
        // App will continue to work, just without real-time updates
      }

      // Mark as initialized - this will trigger the app to render
      isInitialized = true;
      console.log('[App Layout] Initialization complete, rendering app');
    } catch (error) {
      console.error('[App Layout] Critical initialization error:', error);
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

{#if isInitialized}
  <AppShell>
    <slot />
  </AppShell>
{:else}
  <div class="initialization-screen">
    <p>Initializing NodeSpace...</p>
  </div>
{/if}
