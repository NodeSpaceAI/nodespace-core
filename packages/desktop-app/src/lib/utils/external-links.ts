/**
 * External Links Utility
 *
 * Handles opening external URLs (http/https, and mailto: through `openUrl`) with
 * the system default handler. Uses Tauri's opener plugin when running in Tauri,
 * falls back to window.open in browser mode.
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
 * Open a URL with the system's default handler: a web page in the browser, a
 * `mailto:` link in the mail client.
 *
 * @param url - The URL to open (must be http://, https:// or mailto:)
 * @returns Promise that resolves when the URL is opened, or rejects with an error
 */
export async function openUrl(url: string): Promise<void> {
  // Validate URL protocol
  if (!url.startsWith('http://') && !url.startsWith('https://') && !url.startsWith('mailto:')) {
    throw new Error(`Invalid URL protocol. Expected http://, https:// or mailto:, got: ${url}`);
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

/**
 * Extract the target node id from a `nodespace://` href.
 *
 * Accepts the formats emitted across the app:
 * - `nodespace://uuid`
 * - `nodespace://node/uuid` (full URI form)
 * - trailing query params (`?hierarchy=true`, `?deleted=true`) are stripped
 *
 * @returns the trimmed node id, or `null` when the href is not a nodespace
 *          link or carries no id.
 */
export function extractNodeIdFromHref(href: string): string | null {
  if (!isNodespaceUrl(href)) return null;

  let nodeId = href.slice('nodespace://'.length);

  // Full-URI form: nodespace://node/uuid
  if (nodeId.startsWith('node/')) {
    nodeId = nodeId.slice('node/'.length);
  }

  // Drop query params (e.g. ?hierarchy=true, ?deleted=true)
  const queryIndex = nodeId.indexOf('?');
  if (queryIndex !== -1) {
    nodeId = nodeId.slice(0, queryIndex);
  }

  nodeId = nodeId.trim();
  return nodeId === '' ? null : nodeId;
}
