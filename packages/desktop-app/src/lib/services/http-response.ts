/**
 * Dev-proxy response handling for browser mode.
 *
 * Every service that fetches from the dev-proxy reads its responses through
 * `handleResponse`, so a failed request surfaces the same way whichever
 * service sent it: a `BackendError` carrying the proxy's `message` and `code`.
 */

/**
 * Shape of the JSON error body dev-proxy sends for a failed request —
 * mirrors the Tauri command layer's `CommandError` (see
 * packages/desktop-app/app-lib/src/commands/nodes.rs) closely enough that
 * a well-formed body carries the same `code`/`conflictData` a Tauri
 * CommandError would.
 */
interface BackendErrorBody {
  message?: string;
  code?: string;
  conflictData?: unknown;
}

/**
 * Typed error thrown by `handleResponse` for a failed request.
 *
 * Unlike a bare `Error(message)`, this preserves the wire-level detail a
 * caller needs to classify the failure: the backend's structured error
 * `code` (e.g. `"SUBTREE_ACCESS_DENIED"`, `"VERSION_CONFLICT"`), the HTTP
 * `status`, and — when the body carries one — the structured `conflictData`
 * payload the daemon attaches to OCC-conflict and subtree-access refusals.
 *
 * `code`/`conflictData` are read structurally (not via `instanceof`) by
 * `isSubtreeAccessDenied` / `isVersionConflict` in `$lib/types/errors`, so a
 * `BackendError` shaped like a `SUBTREE_ACCESS_DENIED` or `VERSION_CONFLICT`
 * CommandError is classified the same way regardless of transport.
 */
export class BackendError extends Error {
  constructor(
    message: string,
    readonly code?: string,
    readonly status?: number,
    readonly conflictData?: unknown
  ) {
    super(message);
    this.name = 'BackendError';
  }
}

/**
 * Read a dev-proxy response: the parsed body on success (`undefined` for an
 * empty one), a `BackendError` on failure.
 */
export async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    const fallbackMessage = `HTTP ${response.status}: ${response.statusText}`;

    // This try only guards the JSON parse itself — a genuinely malformed
    // (non-JSON) error body takes the SyntaxError branch below. A
    // well-formed body, whatever its content, falls through to error
    // construction OUTSIDE the try so it can't be caught by its own catch.
    let errorData: BackendErrorBody | undefined;
    try {
      errorData = await response.json();
    } catch (parseError) {
      if (parseError instanceof SyntaxError) {
        throw new BackendError(fallbackMessage, undefined, response.status);
      }
      throw parseError;
    }

    throw new BackendError(
      errorData?.message || fallbackMessage,
      errorData?.code,
      response.status,
      errorData?.conflictData
    );
  }

  if (response.status === 204 || response.headers.get('content-length') === '0') {
    return undefined as T;
  }

  return await response.json();
}
