<script lang="ts">
  import { onMount } from 'svelte';
  import '../app.css';
  import '$lib/styles/noderef.css';
  import AppShell from '$lib/components/layout/app-shell.svelte';
  import DiagnosticPanel from '$lib/design/components/diagnostic-panel.svelte';
  import DeleteConfirmationModal from '$lib/components/delete-confirmation-modal.svelte';
  import SubtreeAccessDeniedModal from '$lib/components/subtree-access-denied-modal.svelte';
  import VersionBadge from '$lib/components/dev/version-badge.svelte';
  import { initializeSchemaPluginSystem } from '$lib/plugins/schema-plugin-loader';
  import { initializeTauriSyncListeners } from '$lib/services/tauri-sync-listener';
  import { initializeApp } from '$lib/services/app-initialization';
  import { onDaemonReconnect } from '$lib/services/daemon-status';
  import {
    isChannelEnabled,
    captureDomSnapshot,
    captureStoreDump
  } from '$lib/services/debug-channel';
  import { toError } from '$lib/types/errors';

  let isInitialized = false;
  let initError: string | null = null;

  // Retry schema plugin registration + sync listener init once the daemon
  // reconnects, in case the initial attempt ran while it was still starting.
  let schemaPluginsReady = false;
  let syncListenersReady = false;

  async function loadSchemaPlugins() {
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
        schemaPluginsReady = true;
        console.log(
          `[App Layout] Custom entity system ready (${result.registeredCount} entities loaded)`
        );
      }
    } catch (error) {
      console.error('[App Layout] Schema plugin initialization failed:', error);
    }
  }

  async function loadSyncListeners() {
    try {
      await initializeTauriSyncListeners();
      syncListenersReady = true;
    } catch (error) {
      console.warn('[App Layout] Tauri sync listeners failed to initialize:', error);
      // Don't block app startup if sync listeners fail
      // App will continue to work, just without real-time updates
    }
  }

  // Initialize Tauri injection, then render AppShell immediately. Daemon
  // readiness is NOT a render-blocking gate — AppShell owns its own
  // connecting/unreachable UI via the daemon-status service, and each
  // daemon-dependent store retries its load on daemon reconnect.
  onMount(async () => {
    try {
      // Step 1: Initialize Tauri injection (fast, one-time environment check)
      await initializeApp();

      // Mark as initialized - this will trigger the app to render
      isInitialized = true;
      console.log('[App Layout] Tauri ready, rendering app');

      // Step 2: Initialize schema plugin auto-registration system.
      // Not render-blocking — schemas load opportunistically and retry on
      // daemon reconnect (see schema-plugin-loader / schemas store).
      await loadSchemaPlugins();

      // Step 3: Initialize Tauri domain event listeners for real-time synchronization
      await loadSyncListeners();
    } catch (error) {
      console.error('[App Layout] Critical initialization error:', error);
      // Store error for display on screen
      initError = toError(error).message;
    }
  });

  // Expose on-demand debug-channel captures for AI agents / developers driving
  // the packaged WKWebView via Safari Web Inspector. Gated on the same
  // NS_FRONTEND_LOG probe the rest of the debug channel uses — normal builds
  // never touch `window.__ns_debug`.
  onMount(() => {
    void isChannelEnabled().then(enabled => {
      if (!enabled) return;
      (window as unknown as { __ns_debug: unknown }).__ns_debug = {
        captureDomSnapshot,
        captureStoreDump
      };
    });
  });

  // Retry whichever of the above didn't succeed on the first attempt once
  // the daemon signals it is reachable.
  onMount(() => {
    const unsubscribe = onDaemonReconnect(() => {
      if (!schemaPluginsReady) loadSchemaPlugins();
      if (!syncListenersReady) loadSyncListeners();
    });
    return unsubscribe;
  });

  // Flushing pending saves on window close is handled by
  // registerShutdownHandlers() in app-initialization.ts, which owns the
  // single Tauri-aware beforeunload/close-request registration.
</script>

{#if isInitialized}
  <AppShell>
    <slot />
  </AppShell>
{:else if initError}
  <div class="initialization-error">
    <h1>Initialization Failed</h1>
    <p class="error-message">{initError}</p>
    <p class="hint">Please report this issue with the error message above.</p>
    <p class="hint">Press Ctrl+Shift+D to open the diagnostic panel for more details.</p>
  </div>
{:else}
  <div class="initialization-screen">
    <p>Initializing NodeSpace...</p>
  </div>
{/if}

<!-- Diagnostic Panel - Press Ctrl+Shift+D to toggle -->
<DiagnosticPanel />

<!-- Delete confirmation modal - mounted globally so services can trigger it -->
<DeleteConfirmationModal />

<!-- Subtree-access-denied refusal modal - mounted globally so the delete path can trigger it -->
<SubtreeAccessDeniedModal />

<!-- Dev-only build/version badge - hidden unless localStorage['ns:show-build-badge']==='1' -->
<VersionBadge />

<style>
  .initialization-error {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100vh;
    background: #1a1a1a;
    color: #fff;
    padding: 2rem;
    text-align: center;
  }

  .initialization-error h1 {
    color: #f85149;
    margin-bottom: 1rem;
  }

  .initialization-error .error-message {
    background: #2d1f1f;
    border: 1px solid #f85149;
    border-radius: 8px;
    padding: 1rem 2rem;
    font-family: monospace;
    font-size: 14px;
    max-width: 80%;
    word-break: break-word;
    margin-bottom: 1.5rem;
  }

  .initialization-error .hint {
    color: #888;
    font-size: 14px;
    margin: 0.5rem 0;
  }
</style>
