<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { invoke } from '@tauri-apps/api/core';
  import NavigationSidebar from './navigation-sidebar.svelte';
  import TabSystem from './tab-system.svelte';
  import ThemeProvider from '$lib/design/components/theme-provider.svelte';
  import NodeServiceContext from '$lib/contexts/node-service-context.svelte';
  import { initializeTheme } from '$lib/design/theme';
  import { layoutState, toggleSidebar } from '$lib/stores/layout';
  import { registerCorePlugins } from '$lib/plugins/core-plugins';
  import { pluginRegistry } from '$lib/plugins/index';
  import { isValidDateString } from '$lib/utils/date-formatting';
  import { SharedNodeStore } from '$lib/services/shared-node-store';
  import { MCP_EVENTS } from '$lib/constants';
  import type { Node } from '$lib/types';

  // Constants
  const LOG_PREFIX = '[AppShell]';

  /**
   * Sets up MCP event listeners for real-time UI updates
   *
   * Listens to Tauri events emitted by the MCP server and updates the SharedNodeStore
   * to trigger reactive UI updates across all components.
   *
   * @param sharedNodeStore - The shared node store instance to update
   * @returns Cleanup function that removes all MCP event listeners
   */
  function setupMCPListeners(sharedNodeStore: SharedNodeStore): () => Promise<void> {
    // Listen for node creation events from MCP
    const unlistenNodeCreated = listen<{ node: Node }>(MCP_EVENTS.NODE_CREATED, (event) => {
      console.log(`${LOG_PREFIX} [MCP] Node created:`, event.payload.node.id);
      sharedNodeStore.setNode(
        event.payload.node,
        { type: 'mcp-server' },
        true // Skip persistence - already saved by MCP backend
      );
    });

    // Listen for node update events from MCP (hybrid approach - fetch full node)
    const unlistenNodeUpdated = listen<{ node_id: string }>(
      MCP_EVENTS.NODE_UPDATED,
      async (event) => {
        console.log(`${LOG_PREFIX} [MCP] Node updated:`, event.payload.node_id);
        try {
          const node = await invoke<Node>('get_node', { id: event.payload.node_id });
          if (node) {
            sharedNodeStore.setNode(node, { type: 'mcp-server' }, false);
          } else {
            console.warn(
              `${LOG_PREFIX} [MCP] Node not found after update event:`,
              event.payload.node_id
            );
          }
        } catch (error) {
          console.error(
            `${LOG_PREFIX} [MCP] Failed to fetch node after update event:`,
            event.payload.node_id,
            error
          );
        }
      }
    );

    // Listen for node deletion events from MCP
    const unlistenNodeDeleted = listen<{ node_id: string }>(MCP_EVENTS.NODE_DELETED, (event) => {
      console.log(`${LOG_PREFIX} [MCP] Node deleted:`, event.payload.node_id);
      sharedNodeStore.deleteNode(
        event.payload.node_id,
        { type: 'mcp-server' },
        false // Don't skip persistence - let store handle it
      );
    });

    // Return cleanup function
    return async () => {
      (await unlistenNodeCreated)();
      (await unlistenNodeUpdated)();
      (await unlistenNodeDeleted)();
    };
  }

  // TypeScript compatibility for Tauri window check

  // Initialize theme system and menu event listeners
  onMount(() => {
    const cleanup = initializeTheme();

    // Initialize the unified plugin registry with core plugins
    registerCorePlugins(pluginRegistry);

    // Listen for menu events from Tauri (only if running in Tauri environment)
    let unlistenMenu: Promise<() => void> | null = null;
    let cleanupMCP: (() => Promise<void>) | null = null;

    if (
      typeof window !== 'undefined' &&
      (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
    ) {
      unlistenMenu = listen('menu-toggle-sidebar', () => {
        toggleSidebar();
      });

      // Set up MCP event listeners for real-time UI updates
      cleanupMCP = setupMCPListeners(SharedNodeStore.getInstance());
    }

    // Global click handler for nodespace:// links
    const handleLinkClick = (event: MouseEvent) => {
      const target = event.target as HTMLElement;

      // Find closest anchor element (handles clicking on children of <a>)
      const anchor = target.closest('a');
      if (!anchor) return;

      const href = anchor.getAttribute('href');

      // Only support nodespace:// protocol
      if (!href || !href.startsWith('nodespace://')) return;

      // Prevent default browser navigation
      event.preventDefault();
      event.stopPropagation();

      // Extract node ID from various formats:
      // - nodespace://uuid (standard format)
      // - nodespace://node/uuid (full URI format)
      let nodeId = href.replace('nodespace://', '');

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
      if (!uuidRegex.test(nodeId) && !isValidDateString(nodeId)) {
        console.error(`${LOG_PREFIX} Invalid node ID format: ${nodeId}`);
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
      if (cleanupMCP) {
        await cleanupMCP();
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
