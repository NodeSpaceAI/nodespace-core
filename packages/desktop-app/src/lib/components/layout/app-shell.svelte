<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import NavigationSidebar from './navigation-sidebar.svelte';
  import TabSystem from './tab-system.svelte';
  import ThemeProvider from '$lib/design/components/theme-provider.svelte';
  import NodeServiceContext from '$lib/contexts/node-service-context.svelte';
  import { initializeTheme } from '$lib/design/theme';
  import { layoutState, toggleSidebar } from '$lib/stores/layout';
  import { registerCorePlugins } from '$lib/plugins/core-plugins';
  import { pluginRegistry } from '$lib/plugins/index';

  // TypeScript compatibility for Tauri window check

  // Initialize theme system and menu event listeners
  onMount(() => {
    const cleanup = initializeTheme();

    // Initialize the unified plugin registry with core plugins
    registerCorePlugins(pluginRegistry);

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

    // PHASE 1: Global click handler for nodespace:// and node:// links (console.log only)
    const handleLinkClick = (event: MouseEvent) => {
      const target = event.target as HTMLElement;

      // Find closest anchor element (handles clicking on children of <a>)
      const anchor = target.closest('a');
      if (!anchor) return;

      const href = anchor.getAttribute('href');
      // Handle both node:// and nodespace:// protocols
      if (!href || (!href.startsWith('nodespace://') && !href.startsWith('node://'))) return;

      // Prevent default browser navigation
      event.preventDefault();
      event.stopPropagation();

      // Extract node UUID from various formats:
      // - node://uuid (current format)
      // - nodespace://uuid (alternative format)
      // - nodespace://node/uuid (full URI format)
      let nodeId = href.replace('nodespace://', '').replace('node://', '');

      // Handle nodespace://node/uuid format
      if (nodeId.startsWith('node/')) {
        nodeId = nodeId.replace('node/', '');
      }

      // Remove query parameters if present (e.g., ?hierarchy=true)
      const queryIndex = nodeId.indexOf('?');
      if (queryIndex !== -1) {
        nodeId = nodeId.substring(0, queryIndex);
      }

      // Validate node ID format (UUID or date format YYYY-MM-DD)
      const uuidRegex = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
      const dateRegex = /^\d{4}-\d{2}-\d{2}$/;
      if (!uuidRegex.test(nodeId) && !dateRegex.test(nodeId)) {
        console.error(`[NavigationService] Invalid node ID format: ${nodeId}`);
        return;
      }

      // Check for Cmd+Click (Mac) or Ctrl+Click (Windows/Linux)
      const openInNewTab = event.metaKey || event.ctrlKey;

      // Phase 2-3: Actually navigate using NavigationService (lazy import)
      (async () => {
        const { getNavigationService } = await import('$lib/services/navigation-service');
        const navService = getNavigationService();
        navService.navigateToNode(nodeId, openInNewTab);
      })();
    };

    // Attach global event listener in capture phase (fires before bubble phase)
    // This ensures we catch the event before any other handlers
    document.addEventListener('click', handleLinkClick, true);

    return async () => {
      cleanup?.();
      if (unlistenMenu) {
        (await unlistenMenu)();
      }
      // Cleanup click handler (must match capture phase flag)
      document.removeEventListener('click', handleLinkClick, true);
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
  <NodeServiceContext>
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
  </NodeServiceContext>
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
    overflow: hidden; /* Don't scroll here - let child components handle it */
    position: relative;
    background: hsl(var(--content-background));
    transition: margin-left 0.3s ease;
    flex: 1;
    min-height: 0; /* Critical for flex children to scroll properly */
    display: flex;
    flex-direction: column;
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
