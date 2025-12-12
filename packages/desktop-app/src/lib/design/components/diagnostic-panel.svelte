<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import {
    getLogEntries,
    getDiagnosticStats,
    clearLogEntries,
    exportLogsAsJson,
    getDatabaseDiagnostics,
    testNodePersistence,
    type DiagnosticLogEntry,
    type DiagnosticStats,
    type DatabaseDiagnostics,
    type TestPersistenceResult
  } from '$lib/services/diagnostic-logger';

  let isOpen = $state(false);
  let activeTab = $state<'logs' | 'database' | 'test'>('logs');
  let logEntries = $state<DiagnosticLogEntry[]>([]);
  let stats = $state<DiagnosticStats | null>(null);
  let dbDiagnostics = $state<DatabaseDiagnostics | null>(null);
  let testResult = $state<TestPersistenceResult | null>(null);
  let isLoading = $state(false);
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

  async function fetchDatabaseDiagnostics() {
    isLoading = true;
    try {
      dbDiagnostics = await getDatabaseDiagnostics();
    } finally {
      isLoading = false;
    }
  }

  async function runPersistenceTest() {
    isLoading = true;
    testResult = null;
    try {
      testResult = await testNodePersistence();
    } finally {
      isLoading = false;
    }
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

  function copyToClipboard(text: string) {
    globalThis.navigator.clipboard.writeText(text);
  }

  function formatDuration(ms: number): string {
    if (ms < 1) return '<1ms';
    if (ms < 1000) return `${ms.toFixed(1)}ms`;
    return `${(ms / 1000).toFixed(2)}s`;
  }

  function formatBytes(bytes: number | null): string {
    if (bytes === null) return 'N/A';
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
  }

  onMount(() => {
    window.addEventListener('keydown', handleKeydown);

    // Check for database initialization error
    checkDbInitError();

    // Auto-refresh logs every 2 seconds when panel is open
    refreshInterval = setInterval(() => {
      if (isOpen && autoRefresh && activeTab === 'logs') {
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

  // Initial load when tab changes
  $effect(() => {
    if (isOpen && activeTab === 'database' && !dbDiagnostics) {
      fetchDatabaseDiagnostics();
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

    <div class="tabs">
      <button
        class="tab"
        class:active={activeTab === 'logs'}
        onclick={() => {
          activeTab = 'logs';
          refreshLogs();
        }}
      >
        Call Logs
      </button>
      <button
        class="tab"
        class:active={activeTab === 'database'}
        onclick={() => {
          activeTab = 'database';
        }}
      >
        Database
      </button>
      <button
        class="tab"
        class:active={activeTab === 'test'}
        onclick={() => {
          activeTab = 'test';
        }}
      >
        Persistence Test
      </button>
    </div>

    <div class="panel-content">
      {#if activeTab === 'logs'}
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
      {:else if activeTab === 'database'}
        <div class="database-tab">
          <div class="toolbar">
            <button onclick={fetchDatabaseDiagnostics} disabled={isLoading}>
              {isLoading ? 'Loading...' : 'Refresh'}
            </button>
            {#if dbDiagnostics}
              <button onclick={() => copyToClipboard(JSON.stringify(dbDiagnostics, null, 2))}>
                Copy JSON
              </button>
            {/if}
          </div>

          {#if dbDiagnostics}
            <div class="db-info">
              <div class="info-row">
                <span class="label">Database Path:</span>
                <span class="value path">{dbDiagnostics.databasePath}</span>
              </div>
              <div class="info-row">
                <span class="label">Exists:</span>
                <span class="value" class:success={dbDiagnostics.databaseExists} class:error={!dbDiagnostics.databaseExists}>
                  {dbDiagnostics.databaseExists ? 'Yes' : 'No'}
                </span>
              </div>
              <div class="info-row">
                <span class="label">Size:</span>
                <span class="value">{formatBytes(dbDiagnostics.databaseSizeBytes)}</span>
              </div>
              <div class="info-row">
                <span class="label">Total Nodes:</span>
                <span class="value">{dbDiagnostics.totalNodeCount}</span>
              </div>
              <div class="info-row">
                <span class="label">Root Nodes:</span>
                <span class="value">{dbDiagnostics.rootNodeCount}</span>
              </div>
              <div class="info-row">
                <span class="label">Schema Count:</span>
                <span class="value">{dbDiagnostics.schemaCount}</span>
              </div>
              {#if dbDiagnostics.recentNodeIds.length > 0}
                <div class="info-section">
                  <strong>Recent Node IDs:</strong>
                  <ul>
                    {#each dbDiagnostics.recentNodeIds as nodeId}
                      <li><code>{nodeId}</code></li>
                    {/each}
                  </ul>
                </div>
              {/if}
              {#if dbDiagnostics.errors.length > 0}
                <div class="info-section errors">
                  <strong>Errors:</strong>
                  <ul>
                    {#each dbDiagnostics.errors as error}
                      <li class="error">{error}</li>
                    {/each}
                  </ul>
                </div>
              {/if}
            </div>
          {:else if isLoading}
            <div class="loading">Loading database diagnostics...</div>
          {:else}
            <div class="empty-message">Click Refresh to load database diagnostics.</div>
          {/if}
        </div>
      {:else if activeTab === 'test'}
        <div class="test-tab">
          <div class="toolbar">
            <button onclick={runPersistenceTest} disabled={isLoading}>
              {isLoading ? 'Testing...' : 'Run Persistence Test'}
            </button>
          </div>

          <p class="test-description">
            This test creates a temporary node, reads it back, verifies the content matches, and then deletes it.
            If any step fails, it will show where the persistence issue occurs.
          </p>

          {#if testResult}
            <div class="test-results">
              <div class="test-row">
                <span class="label">Test ID:</span>
                <code class="value">{testResult.testId}</code>
              </div>
              <div class="test-row">
                <span class="label">Create:</span>
                <span class="value" class:success={testResult.created} class:error={!testResult.created}>
                  {testResult.created ? 'SUCCESS' : 'FAILED'}
                </span>
                {#if testResult.createError}
                  <span class="error-detail">{testResult.createError}</span>
                {/if}
              </div>
              <div class="test-row">
                <span class="label">Verify (Read Back):</span>
                <span class="value" class:success={testResult.verified} class:error={!testResult.verified}>
                  {testResult.verified ? 'SUCCESS' : 'FAILED'}
                </span>
                {#if testResult.verifyError}
                  <span class="error-detail">{testResult.verifyError}</span>
                {/if}
              </div>
              <div class="test-row">
                <span class="label">Content Match:</span>
                <span class="value" class:success={testResult.contentMatched} class:error={!testResult.contentMatched}>
                  {testResult.contentMatched ? 'YES' : 'NO'}
                </span>
              </div>
              <div class="test-row">
                <span class="label">Cleanup:</span>
                <span class="value" class:success={testResult.cleanupSuccess} class:error={!testResult.cleanupSuccess}>
                  {testResult.cleanupSuccess ? 'SUCCESS' : 'FAILED'}
                </span>
              </div>

              <div class="test-summary" class:success={testResult.verified && testResult.contentMatched} class:error={!testResult.verified || !testResult.contentMatched}>
                {#if testResult.verified && testResult.contentMatched}
                  Persistence is working correctly!
                {:else if !testResult.created}
                  ISSUE: Node creation failed. Check backend logs.
                {:else if !testResult.verified}
                  ISSUE: Node created but NOT found on read-back. Database not persisting!
                {:else if !testResult.contentMatched}
                  ISSUE: Node found but content doesn't match. Data corruption possible.
                {/if}
              </div>
            </div>
          {:else if isLoading}
            <div class="loading">Running persistence test...</div>
          {:else}
            <div class="empty-message">Click "Run Persistence Test" to test node persistence.</div>
          {/if}
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .diagnostic-panel {
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    height: 50vh;
    background: var(--color-bg-secondary, #1e1e1e);
    border-top: 2px solid var(--color-border, #444);
    z-index: 10000;
    display: flex;
    flex-direction: column;
    font-family: monospace;
    font-size: 12px;
    color: var(--color-text, #e0e0e0);
  }

  .panel-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 12px;
    background: var(--color-bg-tertiary, #252525);
    border-bottom: 1px solid var(--color-border, #444);
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
    color: var(--color-text-muted, #888);
    font-size: 11px;
  }

  .close-button {
    background: transparent;
    border: 1px solid var(--color-border, #444);
    color: var(--color-text, #e0e0e0);
    padding: 4px 8px;
    cursor: pointer;
    border-radius: 4px;
  }

  .close-button:hover {
    background: var(--color-bg-hover, #333);
  }

  .init-error-banner {
    background: #da3633;
    color: white;
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

  .tabs {
    display: flex;
    border-bottom: 1px solid var(--color-border, #444);
    background: var(--color-bg-tertiary, #252525);
  }

  .tab {
    padding: 8px 16px;
    background: transparent;
    border: none;
    color: var(--color-text-muted, #888);
    cursor: pointer;
    border-bottom: 2px solid transparent;
  }

  .tab:hover {
    color: var(--color-text, #e0e0e0);
  }

  .tab.active {
    color: var(--color-primary, #58a6ff);
    border-bottom-color: var(--color-primary, #58a6ff);
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
    background: var(--color-bg-tertiary, #252525);
    border: 1px solid var(--color-border, #444);
    color: var(--color-text, #e0e0e0);
    border-radius: 4px;
    cursor: pointer;
  }

  .toolbar button:hover:not(:disabled) {
    background: var(--color-bg-hover, #333);
  }

  .toolbar button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .auto-refresh {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--color-text-muted, #888);
  }

  .stats-bar {
    display: flex;
    gap: 16px;
    padding: 8px 12px;
    background: var(--color-bg-tertiary, #252525);
    border-radius: 4px;
    margin-bottom: 12px;
  }

  .stats-bar .success {
    color: #3fb950;
  }

  .stats-bar .error {
    color: #f85149;
  }

  .log-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .log-entry {
    background: var(--color-bg-tertiary, #252525);
    border: 1px solid var(--color-border, #444);
    border-radius: 4px;
    padding: 8px;
  }

  .log-entry.error {
    border-color: #f85149;
  }

  .log-entry.pending {
    border-color: #d29922;
  }

  .entry-header {
    display: flex;
    gap: 12px;
    align-items: center;
    margin-bottom: 8px;
  }

  .method {
    font-weight: 600;
    color: var(--color-primary, #58a6ff);
  }

  .status {
    padding: 2px 6px;
    border-radius: 3px;
    font-size: 10px;
    text-transform: uppercase;
  }

  .status.success {
    background: #238636;
    color: white;
  }

  .status.error {
    background: #da3633;
    color: white;
  }

  .duration {
    color: var(--color-text-muted, #888);
  }

  .timestamp {
    color: var(--color-text-muted, #888);
    margin-left: auto;
  }

  .entry-details {
    font-size: 11px;
  }

  .entry-details code {
    display: block;
    background: var(--color-bg-secondary, #1e1e1e);
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
    color: #f85149;
  }

  .empty-message {
    color: var(--color-text-muted, #888);
    text-align: center;
    padding: 20px;
  }

  .loading {
    color: var(--color-text-muted, #888);
    text-align: center;
    padding: 20px;
  }

  /* Database tab styles */
  .db-info {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .info-row {
    display: flex;
    gap: 12px;
    align-items: baseline;
  }

  .info-row .label {
    min-width: 120px;
    color: var(--color-text-muted, #888);
  }

  .info-row .value {
    font-weight: 500;
  }

  .info-row .value.path {
    font-family: monospace;
    font-size: 11px;
    word-break: break-all;
  }

  .info-row .value.success {
    color: #3fb950;
  }

  .info-row .value.error {
    color: #f85149;
  }

  .info-section {
    margin-top: 12px;
    padding-top: 12px;
    border-top: 1px solid var(--color-border, #444);
  }

  .info-section ul {
    margin: 8px 0 0 0;
    padding-left: 20px;
  }

  .info-section li {
    margin: 4px 0;
  }

  .info-section.errors li {
    color: #f85149;
  }

  /* Test tab styles */
  .test-description {
    color: var(--color-text-muted, #888);
    margin-bottom: 16px;
    line-height: 1.5;
  }

  .test-results {
    background: var(--color-bg-tertiary, #252525);
    border-radius: 4px;
    padding: 16px;
  }

  .test-row {
    display: flex;
    gap: 12px;
    align-items: baseline;
    margin-bottom: 8px;
  }

  .test-row .label {
    min-width: 150px;
    color: var(--color-text-muted, #888);
  }

  .test-row .value {
    font-weight: 600;
  }

  .test-row .value.success {
    color: #3fb950;
  }

  .test-row .value.error {
    color: #f85149;
  }

  .test-row .error-detail {
    color: #f85149;
    font-size: 11px;
    margin-left: 8px;
  }

  .test-summary {
    margin-top: 16px;
    padding: 12px;
    border-radius: 4px;
    font-weight: 600;
    text-align: center;
  }

  .test-summary.success {
    background: #238636;
    color: white;
  }

  .test-summary.error {
    background: #da3633;
    color: white;
  }
</style>
