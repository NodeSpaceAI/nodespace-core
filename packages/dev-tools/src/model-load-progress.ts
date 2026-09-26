/**
 * Relays the daemon's `ModelLoadProgressEvent` stream (`EnsureModelReady` /
 * `DownloadModel`) to browser clients as it arrives, over the dev-proxy's
 * `/api/events` SSE channel.
 *
 * This is the browser-mode mirror of the Tauri command layer's
 * `ensure_model_ready` (packages/desktop-app/src-tauri/src/commands/local_agent.rs),
 * which forwards each event live via `app.emit` and derives the call's
 * outcome from the stream:
 *
 *   - an `"error"` event fails the call with its `error_message`;
 *   - a stream that closes without a `"ready"` or `"error"` event also fails
 *     (e.g. the daemon-side load task panicked and dropped its sender) rather
 *     than reading as success.
 *
 * Buffering the stream until it ends and discarding it — what the proxy did
 * before — left a browser-mode chat send showing a phase-less overlay for the
 * whole download/verify/load, and reported an `"error"` event as success.
 */

/** `ModelLoadProgressEvent` as gRPC-js decodes it (camelCase, int64 as string). */
export interface ProtoModelLoadProgressEvent {
  eventType: string;
  modelId: string;
  message?: string | null;
  bytesDownloaded?: string | number | null;
  bytesTotal?: string | number | null;
  errorMessage?: string | null;
  engineSwapped?: boolean | null;
}

/**
 * SSE payload broadcast for each progress event. Mirrors the frontend's
 * `ModelLoadProgressSseEvent` (packages/desktop-app/src/lib/types/sse-events.ts).
 */
export interface ModelLoadProgressSseEvent {
  type: 'modelLoadProgress';
  modelId: string;
  /** `downloading` | `verifying` | `loading` | `ready` | `error` | `engine_swapped` */
  status: string;
  message?: string;
  bytesDownloaded?: number;
  bytesTotal?: number;
  [key: string]: unknown;
}

export interface ModelLoadRelay {
  /** Pass to `agentStream` as its per-event callback. */
  onEvent: (event: ProtoModelLoadProgressEvent) => void;
  /**
   * The failure message the stream reported, or `null` on success. With
   * `requireTerminal`, a stream that ended without `"ready"`/`"error"` is a
   * failure too.
   */
  failure: (options: { requireTerminal: boolean }) => string | null;
}

function optionalString(value: string | null | undefined): string | undefined {
  return value ? value : undefined;
}

function optionalCount(value: string | number | null | undefined): number | undefined {
  if (value === null || value === undefined || value === '') return undefined;
  const n = Number(value);
  return Number.isFinite(n) ? n : undefined;
}

export function toModelLoadProgressSse(
  event: ProtoModelLoadProgressEvent
): ModelLoadProgressSseEvent {
  const sse: ModelLoadProgressSseEvent = {
    type: 'modelLoadProgress',
    modelId: event.modelId,
    status: event.eventType
  };
  const message = optionalString(event.message);
  if (message !== undefined) sse.message = message;
  if (event.eventType === 'downloading') {
    const downloaded = optionalCount(event.bytesDownloaded);
    const total = optionalCount(event.bytesTotal);
    if (downloaded !== undefined && total !== undefined) {
      sse.bytesDownloaded = downloaded;
      sse.bytesTotal = total;
    }
  }
  return sse;
}

export function createModelLoadRelay(
  broadcast: (event: ModelLoadProgressSseEvent) => void
): ModelLoadRelay {
  let sawReady = false;
  let errorMessage: string | null = null;

  return {
    onEvent(event) {
      if (event.eventType === 'ready') sawReady = true;
      if (event.eventType === 'error') {
        errorMessage = optionalString(event.errorMessage) ?? 'Unknown error';
      }
      broadcast(toModelLoadProgressSse(event));
    },
    failure({ requireTerminal }) {
      if (errorMessage !== null) return errorMessage;
      if (requireTerminal && !sawReady) {
        return 'Model load stream ended without a ready or error event';
      }
      return null;
    }
  };
}
