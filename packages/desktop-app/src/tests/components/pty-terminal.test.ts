/**
 * pty-terminal — rendering of daemon loss markers.
 *
 * When a PTY output stream falls behind a burst, the daemon drops chunks and
 * sends a marker (`droppedChunks > 0`, empty `data`) in their place. The
 * terminal must show a visible truncation notice for it instead of silently
 * rendering a desynced stream.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

vi.mock('$lib/services/tauri-commands', () => ({
  ptyWriteInput: vi.fn().mockResolvedValue(undefined),
  ptyResizeTerminal: vi.fn().mockResolvedValue(undefined)
}));

const terminalWrite = vi.fn();
vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 80;
    rows = 24;
    write = terminalWrite;
    writeln = vi.fn();
    loadAddon = vi.fn();
    open = vi.fn();
    onData = vi.fn();
    dispose = vi.fn();
  }
}));

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit = vi.fn();
  }
}));

type OutputPayload = { data: number[]; timestampMs: number; droppedChunks: number };
const listeners = new Map<string, (event: { payload: OutputPayload }) => void>();
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((name: string, handler: (event: { payload: OutputPayload }) => void) => {
    listeners.set(name, handler);
    return Promise.resolve(() => {});
  })
}));

import PtyTerminal from '$lib/components/agent/pty-terminal.svelte';

async function mountAndGetOutputHandler(sessionId: string) {
  render(PtyTerminal, { props: { sessionId } });
  await vi.waitFor(() => expect(listeners.has(`pty-output-${sessionId}`)).toBe(true));
  return listeners.get(`pty-output-${sessionId}`)!;
}

describe('PtyTerminal', () => {
  beforeEach(() => {
    terminalWrite.mockReset();
    listeners.clear();
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe = vi.fn();
        disconnect = vi.fn();
      }
    );
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('writes ordinary output bytes straight to the terminal', async () => {
    const onOutput = await mountAndGetOutputHandler('s1');

    onOutput({ payload: { data: [104, 105], timestampMs: 1, droppedChunks: 0 } });

    expect(terminalWrite).toHaveBeenCalledTimes(1);
    expect(terminalWrite.mock.calls[0][0]).toEqual(new Uint8Array([104, 105]));
  });

  it('shows a truncation notice for a loss marker', async () => {
    const onOutput = await mountAndGetOutputHandler('s2');

    onOutput({ payload: { data: [], timestampMs: 1, droppedChunks: 42 } });

    expect(terminalWrite).toHaveBeenCalledTimes(1);
    const written = terminalWrite.mock.calls[0][0];
    expect(typeof written).toBe('string');
    expect(written).toContain('[output truncated: 42 chunks dropped]');
  });
});
