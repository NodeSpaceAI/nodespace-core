<!--
  Theme Provider Component
  
  Provides theme context to the entire application and manages
  theme initialization, switching, and CSS custom property updates.
-->

<script lang="ts">
  import { onMount, setContext, onDestroy } from 'svelte';
  import {
    initializeTheme,
    currentTheme,
    themePreference,
    setTheme,
    toggleTheme,
    resetThemeToSystem
  } from '../theme.js';

  // Theme context for child components
  let themeContext: {
    theme: 'light' | 'dark';
    preference: string;
    setTheme: typeof setTheme;
    toggleTheme: typeof toggleTheme;
    resetThemeToSystem: typeof resetThemeToSystem;
  };
  let cleanupTheme: (() => void) | undefined;

  // Create and provide theme context
  $: {
    themeContext = {
      theme: $currentTheme,
      preference: $themePreference,
      setTheme,
      toggleTheme,
      resetThemeToSystem
    };

    // Provide context to child components
    setContext('theme', themeContext);
  }

  // Initialize theme system on mount
  onMount(() => {
    cleanupTheme = initializeTheme();
  });

  // Cleanup on destroy
  onDestroy(() => {
    if (cleanupTheme) {
      cleanupTheme();
    }
  });
</script>

<!-- Theme provider wrapper -->
<div class="theme-provider" data-theme={$currentTheme}>
  <slot {themeContext} />
</div>

<style>
  .theme-provider {
    /* Ensure full viewport coverage */
    min-height: 100vh;
    min-width: 100vw;

    /* Apply theme-aware background */
    background-color: hsl(var(--background));
    color: hsl(var(--foreground));

    /* Font smoothing for better text rendering */
    -webkit-font-smoothing: antialiased;
    -moz-osx-font-smoothing: grayscale;
  }

  /* Ensure proper stacking context */
  .theme-provider {
    position: relative;
    z-index: 0;
  }

  /* Focus ring styling - REMOVED global rule that was affecting textareas
     Tab-specific focus styles remain in tab-system.svelte for keyboard navigation */

  /* Scrollbar styling for webkit browsers */
  :global(.theme-provider ::-webkit-scrollbar) {
    width: 12px;
    height: 12px;
  }

  :global(.theme-provider ::-webkit-scrollbar-track) {
    background: hsl(var(--muted));
    border-radius: var(--radius);
  }

  :global(.theme-provider ::-webkit-scrollbar-thumb) {
    background: hsl(var(--border));
    border-radius: var(--radius);
    border: 2px solid hsl(var(--muted));
  }

  :global(.theme-provider ::-webkit-scrollbar-thumb:hover) {
    background: hsl(var(--muted-foreground));
  }
</style>
