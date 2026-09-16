<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import {
    getLogEntries,
    getDiagnosticStats,
    clearLogEntries,
    exportLogsAsJson,
    type DiagnosticLogEntry,
    type DiagnosticStats
  } from '$lib/services/diagnostic-logger';

  let isOpen = $state(false);
  let logEntries = $state<DiagnosticLogEntry[]>([]);
  let stats = $state<DiagnosticStats | null>(null);
  let autoRefresh = $state(true);
  let refreshInterval: ReturnType<typeof setInterval> | null = null;
  let dbInitError = $state<string | null>(null);

  // Check for database initialization error
  function checkDbInitError() {
    const win = window as unknown as { __DB_INIT_ERROR__?: string };
    if (win.__DB_INIT_ERROR__) {
      dbInitError = win.__DB_INIT_ERROR__;
    }
  }

  // Keyboard shortcut handler
  function handleKeydown(event: KeyboardEvent) {
    // Ctrl+Shift+D (or Cmd+Shift+D on Mac) to toggle panel
    if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === 'd') {
      event.preventDefault();
      isOpen = !isOpen;
      if (isOpen) {
        refreshLogs();
      }
    }
  }

  function refreshLogs() {
    logEntries = getLogEntries();
    stats = getDiagnosticStats();
  }

  function handleClearLogs() {
    clearLogEntries();
    refreshLogs();
  }

  function handleExportLogs() {
    const json = exportLogsAsJson();
    const blob = new globalThis.Blob([json], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `nodespace-diagnostics-${new Date().toISOString()}.json`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  }

  function formatDuration(ms: number): string {
    if (ms < 1) return '<1ms';
    if (ms < 1000) return `${ms.toFixed(1)}ms`;
    return `${(ms / 1000).toFixed(2)}s`;
  }

  onMount(() => {
    window.addEventListener('keydown', handleKeydown);

    // Check for database initialization error
    checkDbInitError();

    // Auto-refresh logs every 2 seconds when panel is open
    refreshInterval = setInterval(() => {
      if (isOpen && autoRefresh) {
        refreshLogs();
      }
    }, 2000);
  });

  onDestroy(() => {
    window.removeEventListener('keydown', handleKeydown);
    if (refreshInterval) {
      clearInterval(refreshInterval);
    }
  });
</script>

{#if isOpen}
  <div class="diagnostic-panel">
    <div class="panel-header">
      <h2>Diagnostic Panel</h2>
      <div class="header-actions">
        <span class="shortcut-hint">Ctrl+Shift+D to toggle</span>
        <button class="close-button" onclick={() => (isOpen = false)}>X</button>
      </div>
    </div>

    {#if dbInitError}
      <div class="init-error-banner">
        <strong>DATABASE INITIALIZATION FAILED:</strong> {dbInitError}
        <p class="error-hint">This is why all operations are failing. Check console/terminal for more details.</p>
      </div>
    {/if}

    <div class="panel-content">
      <div class="logs-tab">
        <div class="toolbar">
          <label class="auto-refresh">
            <input type="checkbox" bind:checked={autoRefresh} />
            Auto-refresh
          </label>
          <button onclick={refreshLogs}>Refresh</button>
          <button onclick={handleClearLogs}>Clear</button>
          <button onclick={handleExportLogs}>Export JSON</button>
        </div>

        {#if stats}
          <div class="stats-bar">
            <span>Total: {stats.totalCalls}</span>
            <span class="success">Success: {stats.successCalls}</span>
            <span class="error">Errors: {stats.errorCalls}</span>
            <span>Avg: {formatDuration(stats.avgDurationMs)}</span>
          </div>
        {/if}

        <div class="log-list">
          {#each [...logEntries].reverse() as entry (entry.id)}
            <div class="log-entry" class:error={entry.status === 'error'} class:pending={entry.status === 'pending'}>
              <div class="entry-header">
                <span class="method">{entry.method}</span>
                <span class="status" class:success={entry.status === 'success'} class:error={entry.status === 'error'}>
                  {entry.status}
                </span>
                <span class="duration">{formatDuration(entry.durationMs)}</span>
                <span class="timestamp">{new Date(entry.timestamp).toLocaleTimeString()}</span>
              </div>
              <div class="entry-details">
                <div class="args">
                  <strong>Args:</strong>
                  <code>{JSON.stringify(entry.args, null, 2)}</code>
                </div>
                {#if entry.result !== undefined}
                  <div class="result">
                    <strong>Result:</strong>
                    <code>{JSON.stringify(entry.result, null, 2)}</code>
                  </div>
                {/if}
                {#if entry.error}
                  <div class="error-msg">
                    <strong>Error:</strong>
                    <code>{entry.error}</code>
                  </div>
                {/if}
              </div>
            </div>
          {/each}
          {#if logEntries.length === 0}
            <div class="empty-message">No backend calls logged yet. Make some operations in the app.</div>
          {/if}
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  /*
    Surface palette is deliberately a fixed dark console (#1e1e1e/#252525/#333
    surfaces, #e0e0e0/#888 text, #444 borders) in BOTH themes, matching the
    monospace developer-tool affordance — it is not theme-tracking chrome, so it
    does not use --background/--foreground. Status colors below DO use the
    semantic tokens, which stay legible against this dark surface in either theme.
  */
  .diagnostic-panel {
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    height: 50vh;
    background: #1e1e1e;
    border-top: 2px solid #444;
    z-index: 10000;
    display: flex;
    flex-direction: column;
    font-family: monospace;
    font-size: 12px;
    color: #e0e0e0;
  }

  .panel-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 12px;
    background: #252525;
    border-bottom: 1px solid #444;
  }

  .panel-header h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .shortcut-hint {
    color: #888;
    font-size: 11px;
  }

  .close-button {
    background: transparent;
    border: 1px solid #444;
    color: #e0e0e0;
    padding: 4px 8px;
    cursor: pointer;
    border-radius: 4px;
  }

  .close-button:hover {
    background: #333;
  }

  /*
    Tinted for the same reason as the .status badges: white on a solid
    --destructive fill is 4.57:1 in light but only 2.86:1 in dark, and the
    inverse foreground fails in light — no single solid pairing works for both.
  */
  .init-error-banner {
    background: hsl(var(--destructive) / 0.2);
    color: hsl(var(--destructive));
    border-left: 3px solid hsl(var(--destructive));
    padding: 12px 16px;
    margin: 0;
    font-weight: 500;
  }

  .init-error-banner strong {
    display: block;
    margin-bottom: 4px;
  }

  .init-error-banner .error-hint {
    margin: 8px 0 0 0;
    font-size: 11px;
    opacity: 0.9;
  }

  .panel-content {
    flex: 1;
    overflow: auto;
    padding: 12px;
  }

  .toolbar {
    display: flex;
    gap: 8px;
    margin-bottom: 12px;
    align-items: center;
  }

  .toolbar button {
    padding: 4px 12px;
    background: #252525;
    border: 1px solid #444;
    color: #e0e0e0;
    border-radius: 4px;
    cursor: pointer;
  }

  .toolbar button:hover:not(:disabled) {
    background: #333;
  }

  .toolbar button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .auto-refresh {
    display: flex;
    align-items: center;
    gap: 4px;
    color: #888;
  }

  .stats-bar {
    display: flex;
    gap: 16px;
    padding: 8px 12px;
    background: #252525;
    border-radius: 4px;
    margin-bottom: 12px;
  }

  .stats-bar .success {
    color: hsl(var(--success));
  }

  .stats-bar .error {
    color: hsl(var(--destructive));
  }

  .log-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .log-entry {
    background: #252525;
    border: 1px solid #444;
    border-radius: 4px;
    padding: 8px;
  }

  .log-entry.error {
    border-color: hsl(var(--destructive));
  }

  .log-entry.pending {
    border-color: hsl(var(--warning));
  }

  .entry-header {
    display: flex;
    gap: 12px;
    align-items: center;
    margin-bottom: 8px;
  }

  .method {
    font-weight: 600;
    color: hsl(var(--primary));
  }

  .status {
    padding: 2px 6px;
    border-radius: 3px;
    font-size: 10px;
    text-transform: uppercase;
  }

  /*
    Badges tint the background and use the state color as text, rather than white
    on a solid fill: --success-foreground/--destructive-foreground are white in
    both themes, which lands at ~2.5-2.8:1 on the solid mid-tone fills. Tinting
    keeps the state legible against this dark console surface in either theme.
  */
  .status.success {
    background: hsl(var(--success) / 0.2);
    color: hsl(var(--success));
  }

  .status.error {
    background: hsl(var(--destructive) / 0.2);
    color: hsl(var(--destructive));
  }

  .duration {
    color: #888;
  }

  .timestamp {
    color: #888;
    margin-left: auto;
  }

  .entry-details {
    font-size: 11px;
  }

  .entry-details code {
    display: block;
    background: #1e1e1e;
    padding: 4px 8px;
    border-radius: 3px;
    overflow-x: auto;
    white-space: pre-wrap;
    word-break: break-all;
    max-height: 100px;
    overflow-y: auto;
  }

  .entry-details .args,
  .entry-details .result,
  .entry-details .error-msg {
    margin-top: 4px;
  }

  .error-msg {
    color: hsl(var(--destructive));
  }

  .empty-message {
    color: #888;
    text-align: center;
    padding: 20px;
  }
</style>
