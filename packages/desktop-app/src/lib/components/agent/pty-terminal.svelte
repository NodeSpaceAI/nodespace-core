<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { ptyWriteInput, ptyResizeTerminal } from '$lib/services/tauri-commands';
  import { createLogger } from '$lib/utils/logger';
  import { formatDroppedNotice } from './pty-output';

  const log = createLogger('PtyTerminal');

  let { sessionId }: { sessionId: string } = $props();

  let terminalEl: HTMLDivElement;
  let terminal: Terminal;
  let fitAddon: FitAddon;
  let unlistenOutput: UnlistenFn | null = null;
  let unlistenClosed: UnlistenFn | null = null;
  let resizeObserver: ResizeObserver | null = null;

  onMount(async () => {
    terminal = new Terminal({
      cursorBlink: true,
      fontFamily: 'JetBrains Mono, Menlo, Monaco, Consolas, monospace',
      fontSize: 13,
      theme: {
        background: 'hsl(222 47% 8%)',
        foreground: 'hsl(0 0% 90%)',
        cursor: 'hsl(210 100% 70%)',
        selectionBackground: 'hsl(210 100% 50% / 0.3)',
      },
    });

    fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(terminalEl);
    fitAddon.fit();

    // Forward keystrokes to the PTY
    terminal.onData(async (data) => {
      try {
        await ptyWriteInput(sessionId, Array.from(new TextEncoder().encode(data)));
      } catch (e) {
        log.warn('write_input failed', e);
      }
    });

    // Subscribe to output events from the Tauri backend
    unlistenOutput = await listen<{ data: number[]; timestampMs: number; droppedChunks: number }>(
      `pty-output-${sessionId}`,
      (event) => {
        const { data, droppedChunks } = event.payload;
        if (droppedChunks > 0) {
          // The daemon discarded output because this stream fell behind a
          // burst; mark the gap so the user knows the rendering is incomplete.
          log.warn('PTY output dropped', { sessionId, droppedChunks });
          terminal.write(formatDroppedNotice(droppedChunks));
          return;
        }
        terminal.write(new Uint8Array(data));
      }
    );

    unlistenClosed = await listen(`pty-closed-${sessionId}`, () => {
      terminal.writeln('\r\n\x1b[33m[Session closed]\x1b[0m');
    });

    // Resize terminal when the component or window resizes
    resizeObserver = new ResizeObserver(() => {
      fitAddon.fit();
      const { cols, rows } = terminal;
      ptyResizeTerminal(sessionId, cols, rows).catch((e) =>
        log.warn('resize_terminal failed', e)
      );
    });
    resizeObserver.observe(terminalEl);
  });

  onDestroy(() => {
    resizeObserver?.disconnect();
    unlistenOutput?.();
    unlistenClosed?.();
    terminal?.dispose();
  });
</script>

<div bind:this={terminalEl} class="pty-terminal"></div>

<style>
  .pty-terminal {
    width: 100%;
    height: 100%;
    overflow: hidden;
    background: hsl(222 47% 8%);
  }

  /* xterm.js injects its own styles; ensure its canvas fills the container */
  :global(.pty-terminal .xterm) {
    height: 100%;
  }

  :global(.pty-terminal .xterm-viewport) {
    overflow-y: auto;
  }
</style>
