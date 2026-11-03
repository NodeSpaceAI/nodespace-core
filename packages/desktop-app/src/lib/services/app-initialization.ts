/**
 * App Initialization - Runs when the app mounts
 *
 * Initializes critical backend services that must be ready before
 * any components try to use Tauri commands.
 *
 * Also handles graceful shutdown to flush pending data persistence
 * before the app closes (Issue: nodes not persisting on other machines).
 */

import { createLogger } from '$lib/utils/logger';
import { sharedNodeStore } from './shared-node-store.svelte';

const log = createLogger('AppInit');

interface WindowWithTauriInternals extends Window {
  __TAURI_INTERNALS__?: unknown;
}

declare const window: WindowWithTauriInternals;

let initialized = false;

/**
 * Wait for the Tauri IPC bridge to be available.
 *
 * Tauri injects `__TAURI_INTERNALS__` asynchronously after the webview
 * loads; it's present regardless of the `withGlobalTauri` config option,
 * unlike `window.__TAURI__` which requires that flag.
 */
async function waitForTauriReady(): Promise<void> {
  const maxAttempts = 200; // 10 seconds with 50ms delays
  let attempts = 0;
  const delayMs = 50;

  while (attempts < maxAttempts) {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      log.debug(`Tauri API ready after ${attempts * delayMs}ms`);
      return;
    }

    await new Promise((resolve) => setTimeout(resolve, delayMs));
    attempts++;
  }

  const isWindow = typeof window !== 'undefined';
  const hasInternals = isWindow && '__TAURI_INTERNALS__' in window;

  log.error('Tauri API check results:', { isWindow, hasInternals });

  throw new Error(
    `Tauri API did not become available after ${maxAttempts * delayMs}ms. ` +
    `isWindow=${isWindow}, hasInternals=${hasInternals}`
  );
}

/**
 * Initialize app services asynchronously on app mount
 *
 * This runs in the first onMount hook before schema plugins or sync listeners initialize.
 * It ensures the database and all Tauri services are initialized before
 * any components try to call Tauri commands.
 */
export async function initializeApp(): Promise<void> {
  // Only initialize once
  if (initialized) {
    return;
  }
  initialized = true;

  try {
    // Wait for Tauri API to be available
    await waitForTauriReady();

    // Database and all services are initialized by nodespaced at Tauri setup time.
    // No explicit initialize_database call needed here.
    log.debug('Tauri ready — services initialized at startup');

    // Register shutdown handlers to flush pending data on close
    registerShutdownHandlers();
  } catch (error: unknown) {
    log.error('Critical initialization error:', error);
    throw error;
  }
}

// Track whether shutdown handlers have been registered
let shutdownHandlersRegistered = false;

/**
 * Register shutdown handlers to flush pending data before app closes.
 *
 * CRITICAL: This prevents data loss when the app is closed before
 * debounced persistence operations complete (500ms debounce window).
 *
 * This is called during app initialization to register:
 * 1. Browser beforeunload event (best-effort flush on reload)
 * 2. Tauri window close event
 */
export function registerShutdownHandlers(): void {
  if (shutdownHandlersRegistered) {
    return;
  }
  shutdownHandlersRegistered = true;

  // beforeunload handler (reload)
  if (typeof window !== 'undefined') {
    window.addEventListener('beforeunload', () => {
      log.debug('Window closing - flushing pending operations...');

      // Check if we have pending writes
      if (sharedNodeStore.hasPendingWrites()) {
        log.info('Flushing pending node writes before close');

        // Note: beforeunload is sync, but we start the flush anyway.
        // For Tauri apps, the close event handler below handles async flushing.
        // On a reload, this provides best-effort flushing.
        sharedNodeStore.flushAllPending().catch((err) => {
          log.error('Error flushing pending writes:', err);
        });
      }
    });

    log.debug('Registered beforeunload shutdown handler');
  }

  // Tauri window close handler
  registerTauriCloseHandler();
}

/**
 * Register Tauri-specific window close handler
 *
 * Uses Tauri 2.x event API to intercept window close and flush
 * pending operations before allowing the window to actually close.
 *
 * `destroy()` MUST run unconditionally at the end, not just on the
 * has-pending-writes path. Tauri's own manager calls
 * `prevent_close()` on every `CloseRequested` whenever a JS listener is
 * registered for it -- which this one always is -- before this callback
 * even runs, and holds the window open regardless of what the callback
 * does. `destroy()` is the only thing that bypasses that and actually
 * closes the window (which is what drives `ExitRequested` -> `Exit` on
 * the Rust side, including the "Quit" tray menu item, which requests a
 * close through this exact path rather than a direct app-exit API). Any
 * code path that returns without calling it leaves the window -- and the
 * whole app -- silently un-closable: no error, just nothing happens.
 */
async function registerTauriCloseHandler(): Promise<void> {
  try {
    // Dynamic import to avoid issues in non-Tauri environments
    const { getCurrentWindow } = await import('@tauri-apps/api/window');

    const currentWindow = getCurrentWindow();

    // Listen for close request
    await currentWindow.onCloseRequested(async (event) => {
      log.debug('Tauri window close requested - flushing pending operations...');

      // Check if we have pending writes
      if (sharedNodeStore.hasPendingWrites()) {
        log.info('Flushing pending node writes before Tauri window close');

        // Tauri already withholds the close regardless of this call (see the
        // function doc comment) -- kept so the intent is explicit here and
        // the flush below isn't racing a close Tauri's own default behavior
        // happens to prevent for us anyway.
        event.preventDefault();

        try {
          // Flush all pending writes
          await sharedNodeStore.flushAllPending();
          log.info('Pending writes flushed successfully');
        } catch (err) {
          log.error('Error flushing pending writes:', err);
        }
      }

      // Always destroy -- see the function doc comment for why this
      // cannot be conditional on hasPendingWrites(). This is now the one
      // call the whole fix hinges on, so a throw here can't be allowed to
      // silently leave the window (and the app) un-closable again.
      try {
        await currentWindow.destroy();
      } catch (err) {
        log.error('Error destroying window on close request:', err);
      }
    });

    log.debug('Registered Tauri close request handler');
  } catch (err) {
    log.warn('Failed to register Tauri close handler:', err);
  }
}
