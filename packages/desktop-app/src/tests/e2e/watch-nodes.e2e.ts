/**
 * E2E: WatchNodes SSE stream delivers events for writes
 *
 * Opens an SSE connection via fetch (streaming) to the dev-proxy endpoint and
 * asserts that node create/update/delete operations produce the expected events.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { DaemonTestHarness } from './daemon-harness';

let h: DaemonTestHarness;

beforeAll(async () => {
  h = await DaemonTestHarness.start();
}, 15_000);

afterAll(async () => {
  await h?.stop();
});

/**
 * Subscribe to the SSE stream and wait for the first event matching `predicate`.
 * Uses fetch streaming — works in Node.js and Bun without extra packages.
 *
 * `onConnected` is called once the server sends the initial `: connected` comment,
 * guaranteeing the subscription is registered before any writes fire.
 *
 * Rejects after `timeoutMs` if no matching event arrives.
 */
async function waitForEvent(
  sseUrl: string,
  predicate: (event: Record<string, unknown>) => boolean,
  opts: { timeoutMs?: number; onConnected?: () => Promise<void> } = {}
): Promise<Record<string, unknown>> {
  const { timeoutMs = 5000, onConnected } = opts;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);

  let response: Response;
  try {
    response = await fetch(sseUrl, { signal: controller.signal });
  } catch (err) {
    clearTimeout(timer);
    throw err;
  }

  if (!response.ok || !response.body) {
    clearTimeout(timer);
    throw new Error(`SSE connect failed: ${response.status}`);
  }

  const decoder = new TextDecoder();
  let buf = '';
  let found: Record<string, unknown> | null = null;
  let connectedFired = false;
  const reader = response.body.getReader();

  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      const lines = buf.split('\n');
      buf = lines.pop() ?? '';

      for (const line of lines) {
        // Fire onConnected once the server acknowledges the SSE connection.
        // This guarantees writes happen only after the subscription is registered.
        if (!connectedFired && line.trim() === ': connected' && onConnected) {
          connectedFired = true;
          onConnected().catch(() => undefined);
          continue;
        }

        if (!line.startsWith('data:')) continue;
        const raw = line.slice(5).trim();
        if (!raw) continue;
        try {
          const ev = JSON.parse(raw) as Record<string, unknown>;
          if (predicate(ev)) {
            found = ev;
            controller.abort();
            break;
          }
        } catch {
          // ignore non-JSON lines (heartbeat comments)
        }
      }
      if (found) break;
    }
  } catch (err) {
    // AbortError is expected when we call controller.abort() after finding our event
    if (found) {
      clearTimeout(timer);
      reader.cancel().catch(() => undefined);
      return found;
    }
    clearTimeout(timer);
    reader.cancel().catch(() => undefined);
    throw err;
  }

  clearTimeout(timer);
  reader.cancel().catch(() => undefined);
  if (found) return found;
  throw new Error(`SSE stream ended without matching event`);
}

describe('WatchNodes SSE stream', () => {
  it('delivers nodeCreated event when a node is written', async () => {
    const id = crypto.randomUUID();

    // Pass onConnected so the write fires only after the SSE subscription is
    // confirmed — prevents a race where createNode completes before the server
    // has registered this client in sseClients.
    const eventPromise = waitForEvent(
      h.sseUrl,
      (ev) => ev.type === 'nodeCreated' && ev.nodeId === id,
      {
        onConnected: () =>
          h.adapter.createNode({ id, nodeType: 'text', content: 'watched create' })
            .then(() => undefined)
      }
    );

    const event = await eventPromise;
    expect(event.type).toBe('nodeCreated');
    expect(event.nodeId).toBe(id);
  });

  it('delivers nodeUpdated event when a node is modified', async () => {
    const id = crypto.randomUUID();
    await h.adapter.createNode({ id, nodeType: 'text', content: 'before update' });

    const eventPromise = waitForEvent(
      h.sseUrl,
      (ev) => ev.type === 'nodeUpdated' && ev.nodeId === id,
      {
        onConnected: () =>
          h.adapter.updateNode(id, 1, { content: 'after update' })
            .then(() => undefined)
      }
    );

    const event = await eventPromise;
    expect(event.type).toBe('nodeUpdated');
    expect(event.nodeId).toBe(id);
  });

  it('delivers nodeDeleted event when a node is removed', async () => {
    const id = crypto.randomUUID();
    await h.adapter.createNode({ id, nodeType: 'text', content: 'to be deleted' });

    const eventPromise = waitForEvent(
      h.sseUrl,
      (ev) => ev.type === 'nodeDeleted' && ev.nodeId === id,
      {
        onConnected: () =>
          h.adapter.deleteNode(id, 1)
            .then(() => undefined)
      }
    );

    const event = await eventPromise;
    expect(event.type).toBe('nodeDeleted');
    expect(event.nodeId).toBe(id);
  });
});
