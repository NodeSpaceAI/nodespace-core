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

  // Props
  export let enableTransitions = true;
  export let transitionDuration = 300;

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

    // Add transition styles if enabled
    if (enableTransitions) {
      const style = document.createElement('style');
      style.id = 'nodespace-theme-transitions';
      style.textContent = `
        * {
          transition: 
            color ${transitionDuration}ms cubic-bezier(0.4, 0, 0.2, 1),
            background-color ${transitionDuration}ms cubic-bezier(0.4, 0, 0.2, 1),
            border-color ${transitionDuration}ms cubic-bezier(0.4, 0, 0.2, 1),
            box-shadow ${transitionDuration}ms cubic-bezier(0.4, 0, 0.2, 1);
        }
        
        /* Prevent transitions during initial load */
        .no-transitions * {
          transition: none !important;
        }
      `;
      document.head.appendChild(style);

      // Remove no-transitions class after initial render
      setTimeout(() => {
        document.body.classList.remove('no-transitions');
      }, 100);
    }
  });

  // Cleanup on destroy
  onDestroy(() => {
    if (cleanupTheme) {
      cleanupTheme();
    }

    // Remove transition styles
    const transitionStyle = document.getElementById('nodespace-theme-transitions');
    if (transitionStyle) {
      transitionStyle.remove();
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

    /* Ensure theme transitions apply to all children */
    transition:
      background-color 300ms cubic-bezier(0.4, 0, 0.2, 1),
      color 300ms cubic-bezier(0.4, 0, 0.2, 1);
  }

  /* Ensure proper stacking context */
  .theme-provider {
    position: relative;
    z-index: 0;
  }

  /* Global text selection styling */
  :global(.theme-provider ::selection) {
    background-color: hsl(var(--primary) / 0.2);
    color: hsl(var(--foreground));
  }

  /* Focus ring styling */
  :global(.theme-provider :focus-visible) {
    outline: 2px solid hsl(var(--ring));
    outline-offset: 2px;
    border-radius: var(--radius);
  }

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
