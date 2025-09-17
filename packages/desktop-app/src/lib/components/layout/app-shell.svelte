<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import NavigationSidebar from './navigation-sidebar.svelte';
  import TabSystem from './tab-system.svelte';
  import ThemeProvider from '$lib/design/components/theme-provider.svelte';
  import { initializeTheme } from '$lib/design/theme';
  import { layoutState, toggleSidebar } from '$lib/stores/layout';
  import { initializeBasicRegistry } from '$lib/registry/initialize.js';

  // TypeScript compatibility for Tauri window check

  // Initialize theme system and menu event listeners
  onMount(() => {
    const cleanup = initializeTheme();

    // Initialize the experimental node type registry
    initializeBasicRegistry();

    // Listen for menu events from Tauri (only if running in Tauri environment)
    let unlistenMenu: Promise<() => void> | null = null;

    if (
      typeof window !== 'undefined' &&
      (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
    ) {
      unlistenMenu = listen('menu-toggle-sidebar', () => {
        toggleSidebar();
      });
    }

    return async () => {
      cleanup?.();
      if (unlistenMenu) {
        (await unlistenMenu)();
      }
    };
  });

  // Subscribe to layout state
  $: isCollapsed = $layoutState.sidebarCollapsed;

  // Handle global keyboard shortcuts
  // function handleKeydown(_event: KeyboardEvent) {
  //   // No global shortcuts currently defined
  // }
</script>

<!-- <svelte:window on:keydown={handleKeydown} /> -->

<!-- 
  Application Shell Component
  
  Provides the main application layout with:
  - Collapsible navigation sidebar
  - Main content area with responsive sizing
  - Global keyboard shortcuts
  - Theme initialization
-->

<ThemeProvider>
  <div
    class="app-shell"
    class:sidebar-collapsed={isCollapsed}
    class:sidebar-expanded={!isCollapsed}
    role="application"
    aria-label="NodeSpace Application"
  >
    <!-- Navigation Sidebar -->
    <NavigationSidebar />

    <!-- Tab System - positioned to span both tabs and content grid areas -->
    <div class="tab-system-wrapper">
      <TabSystem let:activeTab>
        <!-- Main Content Area -->
        <main class="main-content">
          <slot {activeTab} />
        </main>
      </TabSystem>
    </div>
  </div>
</ThemeProvider>

<style>
  .app-shell {
    display: grid;
    grid-template-areas:
      'sidebar tabs'
      'sidebar content';
    grid-template-columns: auto 1fr;
    grid-template-rows: auto 1fr;
    height: 100vh;
    overflow: hidden;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
  }

  /* Navigation Sidebar */
  :global(.navigation-sidebar) {
    grid-area: sidebar;
  }

  /* Tab System Wrapper - spans both tabs and content areas */
  .tab-system-wrapper {
    grid-column: 2;
    grid-row: 1 / span 2;
    display: flex;
    flex-direction: column;
    min-height: 0;
    position: relative;
  }

  /* Main content area */
  .main-content {
    overflow: auto;
    position: relative;
    background: hsl(var(--content-background));
    transition: margin-left 0.3s ease;
    flex: 1;
    min-height: 0;
  }

  /* Ensure proper scrolling behavior */
  .main-content {
    /* Allow content to scroll independently */
    display: flex;
    flex-direction: column;
    min-height: 100vh;
  }

  /* Responsive behavior for smaller screens */
  @media (max-width: 768px) {
    .app-shell {
      grid-template-columns: auto 1fr;
    }

    .sidebar-collapsed {
      /* Mobile: collapsed sidebar should be minimal */
      width: auto;
    }

    .sidebar-expanded {
      /* Mobile: expanded sidebar might overlay content */
      width: 250px;
    }
  }

  /* Focus management for accessibility */
  .app-shell:focus-within {
    /* Ensure focus indicators are visible */
    outline: 2px solid hsl(var(--ring));
    outline-offset: 2px;
  }

  /* Prevent content shift during sidebar transitions */
  .main-content {
    will-change: margin-left;
  }

  /* Handle content overflow properly */
  .main-content > :global(*) {
    max-width: 100%;
  }
</style>
