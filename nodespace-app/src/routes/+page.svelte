<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { getName, getVersion } from '@tauri-apps/api/app';
  import { onMount } from 'svelte';

  let appName = $state('NodeSpace');
  let appVersion = $state('0.1.0');
  let testMessage = $state('');

  onMount(async () => {
    try {
      appName = await getName();
      appVersion = await getVersion();
    } catch (error) {
      console.warn('Could not get app info:', error);
    }
  });

  async function testConnection() {
    try {
      testMessage = await invoke('greet', { name: 'NodeSpace' });
    } catch (error) {
      testMessage = 'Connection test failed';
      console.error('Test failed:', error);
    }
  }
</script>

<main class="nodespace-app">
  <header class="app-header">
    <h1>{appName}</h1>
    <div class="app-info">
      <span class="version">v{appVersion}</span>
      <button class="test-btn" onclick={testConnection}>Test Connection</button>
    </div>
  </header>

  <div class="app-layout">
    <aside class="sidebar">
      <div class="panel journal-view">
        <h3>JournalView</h3>
        <p>Hierarchical note organization panel</p>
        <div class="placeholder-content">
          <div class="node-item">📝 Daily Notes</div>
          <div class="node-item">📚 Projects</div>
          <div class="node-item">💡 Ideas</div>
        </div>
      </div>

      <div class="panel library-view">
        <h3>LibraryView</h3>
        <p>Knowledge base and documents</p>
        <div class="placeholder-content">
          <div class="node-item">📋 Templates</div>
          <div class="node-item">🔍 Saved Searches</div>
          <div class="node-item">📊 Reports</div>
        </div>
      </div>
    </aside>

    <main class="main-content">
      <div class="panel node-viewer">
        <h3>NodeViewer</h3>
        <p>Main content editing and viewing area</p>
        <div class="placeholder-content">
          <div class="editor-placeholder">
            <p>Select a node from the sidebar to begin editing...</p>
            <p>This area will render different node types:</p>
            <ul>
              <li>TextNode - Rich markdown content</li>
              <li>TaskNode - Todo management</li>
              <li>AIChatNode - AI conversations</li>
              <li>EntityNode - Structured data</li>
              <li>QueryNode - Live data views</li>
            </ul>
          </div>
        </div>
      </div>
    </main>

    <aside class="right-sidebar">
      <div class="panel ai-chat-view">
        <h3>AIChatView</h3>
        <p>AI assistant interaction panel</p>
        <div class="placeholder-content">
          <div class="chat-placeholder">
            <div class="chat-message user">How can I organize my notes?</div>
            <div class="chat-message ai">I can help you create a hierarchical structure...</div>
            <div class="chat-input-area">
              <input type="text" placeholder="Ask AI anything..." disabled />
              <button disabled>Send</button>
            </div>
          </div>
        </div>
      </div>
    </aside>
  </div>

  <footer class="app-footer">
    <div class="status-bar">
      <span class="status">Ready</span>
      {#if testMessage}
        <span class="test-result">{testMessage}</span>
      {/if}
    </div>
  </footer>
</main>

<style>
  :global(body) {
    margin: 0;
    padding: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    font-size: 14px;
    line-height: 1.5;
    color: #333;
    background-color: #f8f9fa;
  }

  .nodespace-app {
    height: 100vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .app-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 20px;
    background: #fff;
    border-bottom: 1px solid #e1e5e9;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.1);
  }

  .app-header h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 600;
    color: #2d3748;
  }

  .app-info {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .version {
    color: #6b7280;
    font-size: 12px;
    font-family: 'SF Mono', Monaco, monospace;
  }

  .test-btn {
    padding: 6px 12px;
    font-size: 12px;
    background: #3b82f6;
    color: white;
    border: none;
    border-radius: 4px;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .test-btn:hover {
    background: #2563eb;
  }

  .app-layout {
    display: flex;
    flex: 1;
    overflow: hidden;
  }

  .sidebar {
    width: 280px;
    background: #f8f9fa;
    border-right: 1px solid #e1e5e9;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }

  .main-content {
    flex: 1;
    background: #fff;
    overflow-y: auto;
  }

  .right-sidebar {
    width: 320px;
    background: #f8f9fa;
    border-left: 1px solid #e1e5e9;
    overflow-y: auto;
  }

  .panel {
    margin: 12px;
    padding: 16px;
    background: #fff;
    border: 1px solid #e1e5e9;
    border-radius: 8px;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.1);
  }

  .panel h3 {
    margin: 0 0 8px 0;
    font-size: 16px;
    font-weight: 600;
    color: #2d3748;
  }

  .panel p {
    margin: 0 0 12px 0;
    color: #6b7280;
    font-size: 13px;
  }

  .placeholder-content {
    color: #9ca3af;
  }

  .node-item {
    padding: 8px 12px;
    margin: 4px 0;
    background: #f3f4f6;
    border-radius: 4px;
    cursor: pointer;
    transition: background-color 0.2s;
    font-size: 13px;
  }

  .node-item:hover {
    background: #e5e7eb;
  }

  .editor-placeholder {
    text-align: center;
    padding: 40px 20px;
  }

  .editor-placeholder p {
    margin: 0 0 16px 0;
    color: #6b7280;
  }

  .editor-placeholder ul {
    text-align: left;
    max-width: 300px;
    margin: 0 auto;
  }

  .editor-placeholder li {
    margin: 8px 0;
    color: #9ca3af;
    font-size: 13px;
  }

  .chat-placeholder {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .chat-message {
    padding: 8px 12px;
    border-radius: 8px;
    font-size: 13px;
    max-width: 80%;
  }

  .chat-message.user {
    background: #3b82f6;
    color: white;
    align-self: flex-end;
    text-align: right;
  }

  .chat-message.ai {
    background: #f3f4f6;
    color: #374151;
    align-self: flex-start;
  }

  .chat-input-area {
    display: flex;
    gap: 8px;
    margin-top: 12px;
  }

  .chat-input-area input {
    flex: 1;
    padding: 8px 12px;
    border: 1px solid #d1d5db;
    border-radius: 4px;
    font-size: 13px;
  }

  .chat-input-area button {
    padding: 8px 16px;
    background: #6b7280;
    color: white;
    border: none;
    border-radius: 4px;
    font-size: 13px;
    cursor: not-allowed;
  }

  .app-footer {
    background: #fff;
    border-top: 1px solid #e1e5e9;
    padding: 8px 20px;
  }

  .status-bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 12px;
    color: #6b7280;
  }

  .test-result {
    color: #10b981;
    font-weight: 500;
  }

  @media (prefers-color-scheme: dark) {
    :global(body) {
      background-color: #1f2937;
      color: #f9fafb;
    }

    .app-header {
      background: #374151;
      border-bottom-color: #4b5563;
    }

    .app-header h1 {
      color: #f9fafb;
    }

    .sidebar,
    .right-sidebar {
      background: #1f2937;
      border-color: #4b5563;
    }

    .main-content {
      background: #111827;
    }

    .panel {
      background: #374151;
      border-color: #4b5563;
    }

    .panel h3 {
      color: #f9fafb;
    }

    .node-item {
      background: #4b5563;
      color: #d1d5db;
    }

    .node-item:hover {
      background: #6b7280;
    }

    .chat-message.ai {
      background: #4b5563;
      color: #d1d5db;
    }

    .app-footer {
      background: #374151;
      border-top-color: #4b5563;
    }
  }
</style>
