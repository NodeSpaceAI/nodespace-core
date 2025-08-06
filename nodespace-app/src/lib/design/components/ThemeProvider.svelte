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
    designTokens,
    nodeTokens,
    type ThemeContext
  } from '../theme.js';

  // Props
  export let enableTransitions = true;
  export let transitionDuration = 300;

  // Theme context for child components
  let themeContext: ThemeContext;
  let cleanupTheme: (() => void) | undefined;

  // Create and provide theme context
  $: {
    themeContext = {
      theme: $currentTheme,
      preference: $themePreference,
      tokens: $designTokens,
      nodeTokens: $nodeTokens,
      setTheme: (theme) => themePreference.set(theme),
      toggleTheme: () => {
        const current = $themePreference;
        if (current === 'system') {
          themePreference.set($currentTheme === 'dark' ? 'light' : 'dark');
        } else if (current === 'light') {
          themePreference.set('dark');
        } else {
          themePreference.set('light');
        }
      },
      resetThemeToSystem: () => themePreference.set('system')
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
    background-color: var(--ns-color-surface-background);
    color: var(--ns-color-text-primary);

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
    background-color: var(--ns-color-primary-200);
    color: var(--ns-color-text-primary);
  }

  :global([data-theme='dark'] .theme-provider ::selection) {
    background-color: var(--ns-color-primary-800);
    color: var(--ns-color-text-primary);
  }

  /* Focus ring styling */
  :global(.theme-provider :focus-visible) {
    outline: 2px solid var(--ns-color-primary-500);
    outline-offset: 2px;
    border-radius: var(--ns-radius-base);
  }

  /* Scrollbar styling for webkit browsers */
  :global(.theme-provider ::-webkit-scrollbar) {
    width: 12px;
    height: 12px;
  }

  :global(.theme-provider ::-webkit-scrollbar-track) {
    background: var(--ns-color-surface-panel);
    border-radius: var(--ns-radius-base);
  }

  :global(.theme-provider ::-webkit-scrollbar-thumb) {
    background: var(--ns-color-border-strong);
    border-radius: var(--ns-radius-base);
    border: 2px solid var(--ns-color-surface-panel);
  }

  :global(.theme-provider ::-webkit-scrollbar-thumb:hover) {
    background: var(--ns-color-text-tertiary);
  }

  /* Dark theme scrollbar adjustments */
  :global([data-theme='dark'] .theme-provider ::-webkit-scrollbar-thumb) {
    background: var(--ns-color-border-default);
  }

  :global([data-theme='dark'] .theme-provider ::-webkit-scrollbar-thumb:hover) {
    background: var(--ns-color-text-tertiary);
  }
</style>
