/**
 * External Links Utility
 *
 * Handles opening external URLs (http/https) in the system default browser.
 * Uses Tauri's opener plugin when running in Tauri, falls back to window.open in browser mode.
 */

import { createLogger } from './logger';

const log = createLogger('ExternalLinks');

/**
 * Check if running in Tauri environment
 */
function isTauri(): boolean {
  return (
    typeof window !== 'undefined' &&
    !!(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
  );
}

/**
 * Open a URL in the system default browser
 *
 * @param url - The URL to open (must be http:// or https://)
 * @returns Promise that resolves when the URL is opened, or rejects with an error
 */
export async function openUrl(url: string): Promise<void> {
  // Validate URL protocol
  if (!url.startsWith('http://') && !url.startsWith('https://')) {
    throw new Error(`Invalid URL protocol. Expected http:// or https://, got: ${url}`);
  }

  if (isTauri()) {
    // Use Tauri opener plugin
    const { openUrl: tauriOpenUrl } = await import('@tauri-apps/plugin-opener');
    log.debug(`Opening URL in system browser: ${url}`);
    await tauriOpenUrl(url);
  } else {
    // Browser mode fallback - open in new tab
    log.debug(`Opening URL in new tab (browser mode): ${url}`);
    window.open(url, '_blank', 'noopener,noreferrer');
  }
}

/**
 * Check if a URL is an external link (http/https)
 */
export function isExternalUrl(url: string): boolean {
  return url.startsWith('http://') || url.startsWith('https://');
}

/**
 * Check if a URL is a nodespace link
 */
export function isNodespaceUrl(url: string): boolean {
  return url.startsWith('nodespace://');
}
