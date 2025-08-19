<script lang="ts">
  import ThemeProvider from '$lib/design/components/ThemeProvider.svelte';
  import BaseNodeViewer from '$lib/design/components/BaseNodeViewer.svelte';
  import { themePreference, currentTheme } from '$lib/design/theme.js';

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
  <main class="minimal-app">
    <header class="app-header">
      <h1>NodeSpace Demo</h1>
      <button class="theme-toggle" onclick={toggleTheme} title="Toggle theme ({$themePreference})">
        {getThemeIcon($themePreference)}
        {$themePreference}
      </button>
    </header>

    <div class="content">
      <div class="instructions">
        <h2>NodeSpace Editor</h2>
        <p>Interactive text editor with markdown formatting and hierarchical nodes:</p>
        <ol>
          <li>Type text in the editor below</li>
          <li>Use Cmd+B, Cmd+I, Cmd+U to format text</li>
          <li>Use Tab/Shift+Tab to indent/outdent nodes</li>
          <li>Press Enter to create new nodes</li>
        </ol>
      </div>

      <div class="editor-container">
        <BaseNodeViewer />
      </div>
    </div>

    <footer class="app-footer">
      <span>Current theme: {$themePreference} ({$currentTheme})</span>
    </footer>
  </main>
</ThemeProvider>

<style>
  .minimal-app {
    height: 100vh;
    display: flex;
    flex-direction: column;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    background-color: hsl(var(--background));
    color: hsl(var(--foreground));
  }

  .app-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 1rem 2rem;
    background: hsl(var(--card));
    border-bottom: 1px solid hsl(var(--border));
  }

  .app-header h1 {
    margin: 0;
    font-size: 1.5rem;
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .theme-toggle {
    padding: 0.5rem 1rem;
    border: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    border-radius: 0.375rem;
    cursor: pointer;
    font-size: 0.875rem;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .theme-toggle:hover {
    background: hsl(var(--muted));
  }

  .content {
    flex: 1;
    padding: 2rem;
    max-width: 800px;
    margin: 0 auto;
    width: 100%;
  }

  .instructions {
    margin-bottom: 2rem;
    padding: 1.5rem;
    background: hsl(var(--card));
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
  }

  .instructions h2 {
    margin: 0 0 1rem 0;
    font-size: 1.25rem;
    color: hsl(var(--foreground));
  }

  .instructions p {
    margin: 0 0 1rem 0;
    color: hsl(var(--muted-foreground));
  }

  .instructions ol {
    margin: 0;
    padding-left: 1.5rem;
    color: hsl(var(--muted-foreground));
  }

  .instructions li {
    margin-bottom: 0.5rem;
  }

  .editor-container {
    background: hsl(var(--card));
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    padding: 1.5rem;
    min-height: 300px;
  }

  .app-footer {
    padding: 1rem 2rem;
    background: hsl(var(--card));
    border-top: 1px solid hsl(var(--border));
    text-align: center;
    color: hsl(var(--muted-foreground));
    font-size: 0.875rem;
  }
</style>
