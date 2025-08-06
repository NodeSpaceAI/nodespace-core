<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { getName, getVersion } from '@tauri-apps/api/app';
  import { onMount } from 'svelte';
  import ThemeProvider from '$lib/design/components/ThemeProvider.svelte';
  import BaseNode from '$lib/design/components/BaseNode.svelte';
  import { themePreference, currentTheme } from '$lib/design/theme.js';

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

  function toggleTheme() {
    const current = $themePreference;
    if (current === 'system') {
      themePreference.set($currentTheme === 'dark' ? 'light' : 'dark');
    } else if (current === 'light') {
      themePreference.set('dark');
    } else {
      themePreference.set('light');
    }
  }

  function getThemeIcon(theme: string): string {
    switch (theme) {
      case 'light':
        return '☀️';
      case 'dark':
        return '🌙';
      case 'system':
        return '🖥️';
      default:
        return '🖥️';
    }
  }
</script>

<ThemeProvider>
  <main class="nodespace-app">
    <header class="app-header">
      <h1>{appName}</h1>
      <div class="app-info">
        <span class="version">v{appVersion}</span>
        <button class="ns-button" onclick={toggleTheme} title="Toggle theme ({$themePreference})">
          {getThemeIcon($themePreference)}
        </button>
        <button class="ns-button ns-button--primary" onclick={testConnection}
          >Test Connection</button
        >
      </div>
    </header>

    <div class="app-layout">
      <aside class="sidebar">
        <div class="ns-panel journal-view">
          <h3>JournalView</h3>
          <p>Hierarchical note organization panel</p>
          <div class="placeholder-content">
            <BaseNode
              nodeType="text"
              nodeId="daily-notes"
              title="Daily Notes"
              content="Today's thoughts and observations"
              compact={true}
              hasChildren={true}
            />
            <BaseNode
              nodeType="text"
              nodeId="projects"
              title="Projects"
              content="Active project documentation"
              compact={true}
            />
            <BaseNode
              nodeType="text"
              nodeId="ideas"
              title="Ideas"
              content="Creative concepts and inspiration"
              compact={true}
            />
          </div>
        </div>

        <div class="ns-panel library-view">
          <h3>LibraryView</h3>
          <p>Knowledge base and documents</p>
          <div class="placeholder-content">
            <BaseNode
              nodeType="entity"
              nodeId="templates"
              title="Templates"
              content="Reusable document templates"
              compact={true}
              hasChildren={true}
            />
            <BaseNode
              nodeType="query"
              nodeId="saved-searches"
              title="Saved Searches"
              content="Frequently used search queries"
              compact={true}
            />
            <BaseNode
              nodeType="entity"
              nodeId="reports"
              title="Reports"
              content="Generated analytics and insights"
              compact={true}
              hasChildren={true}
            />
          </div>
        </div>
      </aside>

      <main class="main-content">
        <div class="ns-panel node-viewer">
          <h3>NodeViewer</h3>
          <p>Main content editing and viewing area</p>
          <div class="editor-placeholder">
            <p>Select a node from the sidebar to begin editing...</p>
            <p>This area will render different node types:</p>

            <div class="node-examples">
              <BaseNode
                nodeType="text"
                nodeId="example-text"
                title="TextNode Example (Childless)"
                subtitle="Rich markdown content"
                content="This is an example of a text node with markdown support and rich formatting capabilities."
              />

              <BaseNode
                nodeType="text"
                nodeId="example-text-parent"
                title="TextNode Example (Parent)"
                subtitle="Rich markdown content with children"
                content="This is an example of a parent text node that contains child nodes."
                hasChildren={true}
              />

              <BaseNode
                nodeType="task"
                nodeId="example-task"
                title="TaskNode Example (Childless)"
                subtitle="Todo management"
                content="☐ Complete design system implementation\n☑ Set up Tauri app structure\n☐ Add AI integration"
              />

              <BaseNode
                nodeType="task"
                nodeId="example-task-parent"
                title="TaskNode Example (Parent)"
                subtitle="Todo management with subtasks"
                content="☐ Complete design system implementation\n☑ Set up Tauri app structure\n☐ Add AI integration"
                hasChildren={true}
              />

              <BaseNode
                nodeType="ai-chat"
                nodeId="example-ai-chat"
                title="AIChatNode Example (Childless)"
                subtitle="AI conversations"
                content="AI Assistant: How can I help you organize your knowledge today?"
              />

              <BaseNode
                nodeType="ai-chat"
                nodeId="example-ai-chat-parent"
                title="AIChatNode Example (Parent)"
                subtitle="AI conversations with follow-ups"
                content="AI Assistant: How can I help you organize your knowledge today?"
                hasChildren={true}
              />

              <BaseNode
                nodeType="task"
                nodeId="example-icon-override"
                title="Icon Override Test"
                subtitle="Using nodeIcon prop to override default"
                content="This node has nodeType='task' but uses the 'text' icon via nodeIcon prop."
                nodeIcon="text"
                hasChildren={true}
              />
            </div>
          </div>
        </div>
      </main>

      <aside class="right-sidebar">
        <div class="ns-panel ai-chat-view">
          <h3>AIChatView</h3>
          <p>AI assistant interaction panel</p>
          <div class="chat-placeholder">
            <BaseNode
              nodeType="ai-chat"
              nodeId="chat-example"
              title="AI Conversation"
              content="How can I organize my notes effectively?"
              compact={true}
            />
            <div class="chat-input-area">
              <input class="ns-input" type="text" placeholder="Ask AI anything..." disabled />
              <button class="ns-button" disabled>Send</button>
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
        <span class="theme-indicator">Theme: {$themePreference} ({$currentTheme})</span>
      </div>
    </footer>
  </main>
</ThemeProvider>

<style>
  /* Reset and base styles using design system tokens */
  :global(body) {
    margin: 0;
    padding: 0;
    /* ThemeProvider will handle theme-aware styling */
  }

  .nodespace-app {
    height: 100vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    font-family: var(
      --ns-font-family-ui,
      -apple-system,
      BlinkMacSystemFont,
      'Segoe UI',
      Roboto,
      sans-serif
    );
    background-color: var(--ns-color-surface-background, #ffffff);
    color: var(--ns-color-text-primary, #1a1a1a);
  }

  .app-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: var(--ns-spacing-3, 0.75rem) var(--ns-spacing-5, 1.25rem);
    background: var(--ns-color-surface-elevated, #ffffff);
    border-bottom: 1px solid var(--ns-color-border-default, #e1e5e9);
    box-shadow: var(--ns-shadow-sm, 0 1px 3px rgba(0, 0, 0, 0.1));
    z-index: 10;
  }

  .app-header h1 {
    margin: 0;
    font-size: var(--ns-font-size-xl, 1.25rem);
    font-weight: var(--ns-font-weight-semibold, 600);
    color: var(--ns-color-text-primary, #1a1a1a);
  }

  .app-info {
    display: flex;
    align-items: center;
    gap: var(--ns-spacing-3, 0.75rem);
  }

  .version {
    color: var(--ns-color-text-tertiary, #666666);
    font-size: var(--ns-font-size-xs, 0.75rem);
    font-family: var(--ns-font-family-mono, 'SF Mono', Monaco, monospace);
  }

  .app-layout {
    display: flex;
    flex: 1;
    overflow: hidden;
  }

  .sidebar {
    width: 280px;
    background: var(--ns-color-surface-panel, #f8f9fa);
    border-right: 1px solid var(--ns-color-border-default, #e1e5e9);
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }

  .main-content {
    flex: 1;
    background: var(--ns-color-surface-background, #ffffff);
    overflow-y: auto;
  }

  .right-sidebar {
    width: 320px;
    background: var(--ns-color-surface-panel, #f8f9fa);
    border-left: 1px solid var(--ns-color-border-default, #e1e5e9);
    overflow-y: auto;
  }

  /* Override panel styling to use design system */
  .ns-panel {
    margin: var(--ns-spacing-3, 0.75rem);
    padding: var(--ns-spacing-4, 1rem);
    background: var(--ns-color-surface-elevated, #ffffff);
    border: 1px solid var(--ns-color-border-default, #e1e5e9);
    border-radius: var(--ns-radius-lg, 0.5rem);
    box-shadow: var(--ns-shadow-sm, 0 1px 3px rgba(0, 0, 0, 0.1));
  }

  .ns-panel h3 {
    margin: 0 0 var(--ns-spacing-2, 0.5rem) 0;
    font-size: var(--ns-font-size-base, 1rem);
    font-weight: var(--ns-font-weight-semibold, 600);
    color: var(--ns-color-text-primary, #1a1a1a);
  }

  .ns-panel p {
    margin: 0 0 var(--ns-spacing-3, 0.75rem) 0;
    color: var(--ns-color-text-secondary, #333333);
    font-size: var(--ns-font-size-sm, 0.875rem);
  }

  .placeholder-content {
    color: var(--ns-color-text-tertiary, #666666);
  }

  .editor-placeholder {
    text-align: center;
    padding: var(--ns-spacing-10, 2.5rem) var(--ns-spacing-5, 1.25rem);
  }

  .editor-placeholder p {
    margin: 0 0 var(--ns-spacing-4, 1rem) 0;
    color: var(--ns-color-text-secondary, #333333);
  }

  .node-examples {
    display: flex;
    flex-direction: column;
    gap: var(--ns-spacing-4, 1rem);
    max-width: 600px;
    margin: var(--ns-spacing-6, 1.5rem) auto 0;
    text-align: left;
  }

  .chat-placeholder {
    display: flex;
    flex-direction: column;
    gap: var(--ns-spacing-3, 0.75rem);
  }

  .chat-input-area {
    display: flex;
    gap: var(--ns-spacing-2, 0.5rem);
    margin-top: var(--ns-spacing-3, 0.75rem);
  }

  .app-footer {
    background: var(--ns-color-surface-elevated, #ffffff);
    border-top: 1px solid var(--ns-color-border-default, #e1e5e9);
    padding: var(--ns-spacing-2, 0.5rem) var(--ns-spacing-5, 1.25rem);
  }

  .status-bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: var(--ns-font-size-xs, 0.75rem);
    color: var(--ns-color-text-tertiary, #666666);
    gap: var(--ns-spacing-4, 1rem);
  }

  .test-result {
    color: var(--ns-color-state-success, #10b981);
    font-weight: var(--ns-font-weight-medium, 500);
  }

  .theme-indicator {
    color: var(--ns-color-text-tertiary, #666666);
    font-family: var(--ns-font-family-mono, 'SF Mono', Monaco, monospace);
  }

  /* Responsive adjustments */
  @media (max-width: 768px) {
    .app-layout {
      flex-direction: column;
    }

    .sidebar,
    .right-sidebar {
      width: 100%;
      border: none;
      border-top: 1px solid var(--ns-color-border-default, #e1e5e9);
      max-height: 200px;
    }

    .app-info {
      flex-wrap: wrap;
      gap: var(--ns-spacing-2, 0.5rem);
    }

    .status-bar {
      flex-direction: column;
      align-items: flex-start;
      gap: var(--ns-spacing-1, 0.25rem);
    }
  }

  /* High contrast mode support */
  @media (prefers-contrast: high) {
    .ns-panel {
      border-width: 2px;
    }

    .app-header {
      border-bottom-width: 2px;
    }

    .sidebar,
    .right-sidebar {
      border-width: 2px;
    }
  }

  /* Reduced motion support */
  @media (prefers-reduced-motion: reduce) {
    * {
      transition: none !important;
      animation: none !important;
    }
  }

  /* Print styles */
  @media print {
    .app-header,
    .app-footer,
    .sidebar,
    .right-sidebar {
      display: none;
    }

    .main-content {
      width: 100%;
      overflow: visible;
    }

    .ns-panel {
      box-shadow: none;
      border: 1px solid var(--ns-color-border-default, #e1e5e9);
      break-inside: avoid;
      margin-bottom: var(--ns-spacing-4, 1rem);
    }
  }
</style>
