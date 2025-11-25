/**
 * Client ID - Unique identifier for this browser session
 *
 * Used for SSE filtering to prevent clients from receiving their own updates.
 * Generated once per browser session and persisted in sessionStorage.
 */

const CLIENT_ID_KEY = 'nodespace_client_id';

/**
 * Get or create the client ID for this browser session
 * Stored in sessionStorage so it persists across page reloads but not across browser restarts
 */
export function getClientId(): string {
  if (typeof window === 'undefined') {
    // SSR or test environment - return a test ID
    return 'test-client';
  }

  // Check sessionStorage first
  let clientId = window.sessionStorage.getItem(CLIENT_ID_KEY);

  if (!clientId) {
    // Generate new client ID
    clientId = globalThis.crypto.randomUUID();
    window.sessionStorage.setItem(CLIENT_ID_KEY, clientId);
  }

  return clientId;
}

/**
 * Reset the client ID (for testing only)
 */
export function resetClientId(): void {
  if (typeof window !== 'undefined') {
    window.sessionStorage.removeItem(CLIENT_ID_KEY);
  }
}
