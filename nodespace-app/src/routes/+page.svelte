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
        <h2>NodeSpace Demos</h2>
        <p>Explore the NodeSpace features and components:</p>

        <div class="demo-links">
          <a href="/noderef-demo" class="demo-link">
            <span class="demo-icon">🎨</span>
            <div>
              <h3>BaseNode Decoration System</h3>
              <p>
                Interactive demonstration of rich node reference decorations with all node types
              </p>
            </div>
          </a>

          <a href="/autocomplete-demo" class="demo-link">
            <span class="demo-icon">@</span>
            <div>
              <h3>@ Trigger Autocomplete</h3>
              <p>Universal node reference system with real-time autocomplete</p>
            </div>
          </a>

          <a href="/basenode-autocomplete-demo" class="demo-link">
            <span class="demo-icon">📝</span>
            <div>
              <h3>BaseNode Editor</h3>
              <p>Advanced text editor with @ triggers and node references</p>
            </div>
          </a>
        </div>

        <div class="editor-instructions">
          <h3>Interactive Editor Features:</h3>
          <ol>
            <li>Type text in the editor below</li>
            <li>Use Cmd+B, Cmd+I to format text (bold/italic toggle)</li>
            <li>Use Tab/Shift+Tab to indent/outdent nodes</li>
            <li>Press Enter to create new nodes</li>
          </ol>
        </div>
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

  .demo-links {
    display: grid;
    gap: 1rem;
    margin: 1.5rem 0;
  }

  .demo-link {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 1rem;
    background: hsl(var(--muted) / 0.5);
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    text-decoration: none;
    color: hsl(var(--foreground));
    transition: all 150ms ease-out;
  }

  .demo-link:hover {
    background: hsl(var(--accent));
    border-color: hsl(var(--primary));
    transform: translateY(-1px);
    box-shadow: 0 2px 4px hsl(var(--border) / 0.2);
  }

  .demo-icon {
    font-size: 1.5rem;
    flex-shrink: 0;
  }

  .demo-link h3 {
    margin: 0 0 0.25rem 0;
    font-size: 1rem;
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .demo-link p {
    margin: 0;
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
  }

  .editor-instructions {
    margin-top: 2rem;
    padding-top: 1.5rem;
    border-top: 1px solid hsl(var(--border));
  }

  .editor-instructions h3 {
    margin: 0 0 1rem 0;
    font-size: 1rem;
    color: hsl(var(--foreground));
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
