/**
 * App Initialization - Runs when the app mounts
 *
 * Initializes critical backend services that must be ready before
 * any components try to use Tauri commands.
 */

import { createLogger } from '$lib/utils/logger';

const log = createLogger('AppInit');

// Tauri API types
interface TauriCore {
  invoke: (command: string, ...args: unknown[]) => Promise<unknown>;
}

interface TauriAPI {
  core?: TauriCore;
  invoke?: (command: string, ...args: unknown[]) => Promise<unknown>;
}

interface WindowWithTauri extends Window {
  __TAURI__?: TauriAPI;
}

declare const window: WindowWithTauri;

let initialized = false;

/**
 * Check if running in Tauri desktop environment
 */
function isTauriEnvironment(): boolean {
  return (
    typeof window !== 'undefined' &&
    ('__TAURI__' in window || '__TAURI_INTERNALS__' in window)
  );
}

/**
 * Wait for Tauri API to be available
 *
 * Tauri injects the API asynchronously after the webview loads.
 * We need to wait for window.__TAURI__.core.invoke to be available.
 */
async function waitForTauriReady(): Promise<void> {
  const maxAttempts = 200; // 10 seconds with 50ms delays
  let attempts = 0;
  const delayMs = 50;

  while (attempts < maxAttempts) {
    // Tauri 2.x uses __TAURI__.core.invoke
    if (
      typeof window !== 'undefined' &&
      typeof window.__TAURI__ !== 'undefined' &&
      typeof window.__TAURI__.core?.invoke === 'function'
    ) {
      log.debug(`Tauri API ready after ${attempts * delayMs}ms`);
      return;
    }

    // Fallback for older Tauri versions that use __TAURI__.invoke
    if (
      typeof window !== 'undefined' &&
      typeof window.__TAURI__?.invoke === 'function'
    ) {
      log.debug(`Tauri API ready (legacy) after ${attempts * delayMs}ms`);
      return;
    }

    await new Promise((resolve) => setTimeout(resolve, delayMs));
    attempts++;
  }

  // More detailed error message for debugging
  const isWindow = typeof window !== 'undefined';
  const hasTauri = isWindow && typeof window.__TAURI__ !== 'undefined';
  const hasInvokeCore = hasTauri && typeof window.__TAURI__?.core?.invoke === 'function';
  const hasInvokeLegacy = hasTauri && typeof window.__TAURI__?.invoke === 'function';

  log.error('Tauri API check results:', {
    isWindow,
    hasTauri,
    hasInvokeCore,
    hasInvokeLegacy,
    tauriKeys: hasTauri && window.__TAURI__ ? Object.keys(window.__TAURI__) : 'N/A'
  });

  throw new Error(
    `Tauri API did not become available after ${maxAttempts * delayMs}ms. ` +
    `isWindow=${isWindow}, hasTauri=${hasTauri}, hasInvokeCore=${hasInvokeCore}, hasInvokeLegacy=${hasInvokeLegacy}`
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

  // Skip Tauri initialization in browser mode (using HTTP dev-proxy)
  if (!isTauriEnvironment()) {
    log.debug('Running in browser mode, skipping Tauri initialization');
    return;
  }

  try {
    // Wait for Tauri API to be available
    await waitForTauriReady();

    // Initialize database
    // This ensures SchemaService and other services are available to Tauri commands
    try {
      // Tauri 2.x uses window.__TAURI__.core.invoke
      const invoke = window.__TAURI__?.core?.invoke || window.__TAURI__?.invoke;
      if (!invoke) {
        throw new Error('Tauri invoke function not available');
      }
      await invoke('initialize_database');
      log.info('Database initialized');
    } catch (error: unknown) {
      // Check if already initialized (this is expected on subsequent calls)
      const errorMsg = error instanceof Error ? error.message : String(error);
      if (errorMsg.includes('already initialized')) {
        log.debug('Database already initialized');
      } else {
        log.warn('Database initialization warning:', error);
      }
    }
  } catch (error: unknown) {
    log.error('Critical initialization error:', error);
    throw error;
  }
}
