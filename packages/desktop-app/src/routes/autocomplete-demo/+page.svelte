<script lang="ts">
  import ThemeProvider from '$lib/design/components/ThemeProvider.svelte';
  import AutocompleteModalDemo from '$lib/components/AutocompleteModalDemo.svelte';
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

<svelte:head>
  <title>AutocompleteModal Demo - NodeSpace</title>
</svelte:head>

<ThemeProvider>
  <main class="demo-app">
    <header class="app-header">
      <div class="header-content">
        <div>
          <h1>AutocompleteModal Demo</h1>
          <p>Universal Node Reference System @ Trigger</p>
        </div>
        <button
          class="theme-toggle"
          onclick={toggleTheme}
          title="Toggle theme ({$themePreference})"
        >
          {getThemeIcon($themePreference)}
          {$themePreference}
        </button>
      </div>
    </header>

    <div class="demo-content">
      <AutocompleteModalDemo />
    </div>

    <footer class="app-footer">
      <span>NodeSpace AutocompleteModal Component Demo</span>
      <a href="/" class="back-link">← Back to Main</a>
    </footer>
  </main>
</ThemeProvider>

<style>
  .demo-app {
    min-height: 100vh;
    display: flex;
    flex-direction: column;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    background-color: hsl(var(--background));
    color: hsl(var(--foreground));
  }

  .app-header {
    background: hsl(var(--card));
    border-bottom: 1px solid hsl(var(--border));
    padding: 1.5rem 0;
  }

  .header-content {
    max-width: 1200px;
    margin: 0 auto;
    padding: 0 2rem;
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .app-header h1 {
    margin: 0 0 0.25rem 0;
    font-size: 1.75rem;
    font-weight: 700;
    color: hsl(var(--foreground));
  }

  .app-header p {
    margin: 0;
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
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
    transition: background-color 0.2s;
  }

  .theme-toggle:hover {
    background: hsl(var(--muted));
  }

  .demo-content {
    flex: 1;
    background: hsl(var(--background));
  }

  .app-footer {
    padding: 1rem 2rem;
    background: hsl(var(--card));
    border-top: 1px solid hsl(var(--border));
    display: flex;
    justify-content: space-between;
    align-items: center;
    color: hsl(var(--muted-foreground));
    font-size: 0.875rem;
  }

  .back-link {
    color: hsl(var(--primary));
    text-decoration: none;
    font-weight: 500;
  }

  .back-link:hover {
    text-decoration: underline;
  }
</style>
