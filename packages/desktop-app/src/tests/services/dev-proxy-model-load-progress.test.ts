/**
 * The dev-proxy relays the daemon's EnsureModelReady / DownloadModel progress
 * stream to the browser as it arrives, and derives the HTTP outcome from it
 * the way the Tauri `ensure_model_ready` command does
 * (packages/desktop-app/src-tauri/src/commands/local_agent.rs).
 *
 * Previously the proxy buffered the whole stream and discarded it, so a
 * browser-mode chat send showed a phase-less overlay for the entire
 * download/verify/load, and an `"error"` event was reported as success.
 */

import { describe, it, expect, vi } from 'vitest';
import { createModelLoadRelay, toModelLoadProgressSse } from '../../../../dev-tools/src/model-load-progress';
import type { ModelLoadProgressSseEvent } from '$lib/types/sse-events';

describe('toModelLoadProgressSse', () => {
  it('maps a phase event to the SSE payload', () => {
    expect(
      toModelLoadProgressSse({ eventType: 'verifying', modelId: 'm1', message: 'Checking' })
    ).toEqual({ type: 'modelLoadProgress', modelId: 'm1', status: 'verifying', message: 'Checking' });
  });

  it('converts int64-as-string byte counts on downloading events', () => {
    expect(
      toModelLoadProgressSse({
        eventType: 'downloading',
        modelId: 'm1',
        bytesDownloaded: '1048576',
        bytesTotal: '4294967296'
      })
    ).toEqual({
      type: 'modelLoadProgress',
      modelId: 'm1',
      status: 'downloading',
      bytesDownloaded: 1048576,
      bytesTotal: 4294967296
    });
  });

  it('omits empty optional fields that gRPC-js fills with defaults', () => {
    expect(
      toModelLoadProgressSse({
        eventType: 'loading',
        modelId: 'm1',
        message: '',
        bytesDownloaded: '0',
        bytesTotal: '0',
        errorMessage: ''
      })
    ).toEqual({ type: 'modelLoadProgress', modelId: 'm1', status: 'loading' });
  });
});

describe('createModelLoadRelay', () => {
  it('broadcasts each event as it arrives, in order', () => {
    const seen: ModelLoadProgressSseEvent[] = [];
    const relay = createModelLoadRelay((e) => seen.push(e));

    relay.onEvent({ eventType: 'verifying', modelId: 'm1' });
    expect(seen.map((e) => e.status)).toEqual(['verifying']);

    relay.onEvent({ eventType: 'loading', modelId: 'm1' });
    relay.onEvent({ eventType: 'ready', modelId: 'm1' });
    expect(seen.map((e) => e.status)).toEqual(['verifying', 'loading', 'ready']);
  });

  it('succeeds once a ready event arrived', () => {
    const relay = createModelLoadRelay(vi.fn());
    relay.onEvent({ eventType: 'loading', modelId: 'm1' });
    relay.onEvent({ eventType: 'ready', modelId: 'm1' });
    expect(relay.failure({ requireTerminal: true })).toBeNull();
  });

  it('fails with the error event message', () => {
    const relay = createModelLoadRelay(vi.fn());
    relay.onEvent({ eventType: 'loading', modelId: 'm1' });
    relay.onEvent({ eventType: 'error', modelId: 'm1', errorMessage: 'Out of memory' });
    expect(relay.failure({ requireTerminal: true })).toBe('Out of memory');
    expect(relay.failure({ requireTerminal: false })).toBe('Out of memory');
  });

  it('falls back to a generic message for an error event without one', () => {
    const relay = createModelLoadRelay(vi.fn());
    relay.onEvent({ eventType: 'error', modelId: 'm1', errorMessage: '' });
    expect(relay.failure({ requireTerminal: false })).toBe('Unknown error');
  });

  it('fails a stream that ends without a terminal event only when one is required', () => {
    const relay = createModelLoadRelay(vi.fn());
    relay.onEvent({ eventType: 'loading', modelId: 'm1' });
    expect(relay.failure({ requireTerminal: true })).toBe(
      'Model load stream ended without a ready or error event'
    );
    expect(relay.failure({ requireTerminal: false })).toBeNull();
  });
});
