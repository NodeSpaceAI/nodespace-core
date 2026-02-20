<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { invoke } from '@tauri-apps/api/core';
  import NavigationSidebar from './navigation-sidebar.svelte';
  import PaneManager from './pane-manager.svelte';
  import StatusBar from '$lib/components/status-bar.svelte';
  import { importService } from '$lib/services/import-service';
  import { statusBar } from '$lib/stores/status-bar';
  import ThemeProvider from '$lib/design/components/theme-provider.svelte';
  import NodeServiceContext from '$lib/contexts/node-service-context.svelte';
  import { initializeTheme } from '$lib/design/theme';
  import { layoutState, toggleSidebar, loadPersistedLayoutState } from '$lib/stores/layout';
  import { registerCorePlugins } from '$lib/plugins/core-plugins';
  import { pluginRegistry } from '$lib/plugins/index';
  import { toggleTheme, setTheme } from '$lib/design/theme';
  import { SharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { browserSyncService } from '$lib/services/browser-sync-service';
  import { MCP_EVENTS } from '$lib/constants';
  import type { Node } from '$lib/types';
  import { collectionsData } from '$lib/stores/collections';
  import { loadPersistedState, addTab, tabState, setActiveTab } from '$lib/stores/navigation';
  import { get } from 'svelte/store';
  import { TabPersistenceService } from '$lib/services/tab-persistence-service';
  import { createLogger } from '$lib/utils/logger';
  import { openUrl, isExternalUrl, isNodespaceUrl } from '$lib/utils/external-links';

  // Logger instance for AppShell component
  const log = createLogger('AppShell');

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
      log.debug('[MCP] Node created:', event.payload.node.id);
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
        log.debug('[MCP] Node updated:', event.payload.node_id);
        try {
          const node = await invoke<Node>('get_node', { id: event.payload.node_id });
          if (node) {
            sharedNodeStore.setNode(node, { type: 'mcp-server' }, false);

            // Invalidate collection member caches since node title may have changed
            // This is a lightweight operation - just clears the cache, members reload on demand
            collectionsData.invalidateAllMembers();
          } else {
            log.warn(
              '[MCP] Node not found after update event:',
              event.payload.node_id
            );
          }
        } catch (error) {
          log.error(`[MCP] Failed to fetch node after update event: ${event.payload.node_id}`, error);
        }
      }
    );

    // Listen for node deletion events from MCP
    const unlistenNodeDeleted = listen<{ node_id: string }>(MCP_EVENTS.NODE_DELETED, (event) => {
      log.debug('[MCP] Node deleted:', event.payload.node_id);
      sharedNodeStore.deleteNode(
        event.payload.node_id,
        { type: 'mcp-server' },
        false // Don't skip persistence - let store handle it
      );
    });

    // Listen for relationship events to update collection sidebar
    // When member_of relationships change, invalidate cached collection members
    interface RelationshipPayload {
      id: string;
      fromId: string;
      toId: string;
      relationshipType: string;
    }

    const unlistenRelationshipCreated = listen<RelationshipPayload>(
      MCP_EVENTS.RELATIONSHIP_CREATED,
      (event) => {
        if (event.payload.relationshipType === 'member_of') {
          log.debug('[MCP] member_of relationship created, invalidating collection members:', event.payload.toId);
          // Invalidate the collection that received a new member
          collectionsData.invalidateMembers(event.payload.toId);
          // Also reload collection counts
          collectionsData.loadCollections();
        }
      }
    );

    const unlistenRelationshipDeleted = listen<RelationshipPayload>(
      MCP_EVENTS.RELATIONSHIP_DELETED,
      (event) => {
        if (event.payload.relationshipType === 'member_of') {
          log.debug('[MCP] member_of relationship deleted, invalidating collection members:', event.payload.toId);
          // Invalidate the collection that lost a member
          collectionsData.invalidateMembers(event.payload.toId);
          // Also reload collection counts
          collectionsData.loadCollections();
        }
      }
    );

    // Return cleanup function
    return async () => {
      (await unlistenNodeCreated)();
      (await unlistenNodeUpdated)();
      (await unlistenNodeDeleted)();
      (await unlistenRelationshipCreated)();
      (await unlistenRelationshipDeleted)();
    };
  }

  // TypeScript compatibility for Tauri window check

  // Initialize theme system and menu event listeners
  onMount(() => {
    const cleanup = initializeTheme();

    // Load persisted tab state from storage
    const stateLoaded = loadPersistedState();
    if (stateLoaded) {
      log.debug('Persisted tab state loaded successfully');
    } else {
      log.debug('No persisted tab state found, using default state');
    }

    // Load persisted layout state (sidebar collapsed/expanded) from storage
    const layoutStateLoaded = loadPersistedLayoutState();
    if (layoutStateLoaded) {
      log.debug('Persisted layout state loaded successfully');
    } else {
      log.debug('No persisted layout state found, using default state');
    }

    // Initialize the unified plugin registry with core plugins
    registerCorePlugins(pluginRegistry);

    // Listen for menu events from Tauri (only if running in Tauri environment)
    let unlistenMenu: Promise<() => void> | null = null;
    let unlistenStatusBar: Promise<() => void> | null = null;
    let unlistenImport: Promise<() => void> | null = null;
    let unlistenDatabase: Promise<() => void> | null = null;
    let unlistenSettings: Promise<() => void> | null = null;
    let cleanupMCP: (() => Promise<void>) | null = null;
    let staleNodesInterval: ReturnType<typeof setInterval> | null = null;

    if (
      typeof window !== 'undefined' &&
      (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
    ) {
      // Sync theme from backend preferences (overrides localStorage if different)
      invoke<{ activeDatabasePath: string; savedDatabasePath: string | null; display: { renderMarkdown: boolean; theme: string } }>('get_settings')
        .then((settings) => {
          const savedTheme = settings.display.theme;
          if (['system', 'light', 'dark'].includes(savedTheme)) {
            setTheme(savedTheme as 'system' | 'light' | 'dark');
          }
        })
        .catch((err) => {
          log.debug('Could not sync theme from backend preferences:', err);
        });

      unlistenMenu = listen('menu-toggle-sidebar', () => {
        toggleSidebar();
      });

      // Listen for status bar toggle from View menu
      unlistenStatusBar = listen('menu-toggle-status-bar', () => {
        statusBar.toggle();
      });

      // Poll for stale nodes count (embedding queue) every 5 seconds
      const updateStaleNodesCount = async () => {
        try {
          const count = await invoke<number>('get_stale_root_count');
          statusBar.setStaleNodesCount(count);
        } catch (error) {
          log.error('Failed to get stale nodes count:', error);
        }
      };
      // Initial fetch
      updateStaleNodesCount();
      // Set up polling interval
      staleNodesInterval = setInterval(updateStaleNodesCount, 5000);

      // Listen for import folder menu event
      unlistenImport = listen('menu-import-folder', async () => {
        const folderPath = await importService.selectFolder();
        if (!folderPath) return;

        // Track if we've received step 9 (complete) to know when to unsubscribe
        let unsubProgress: (() => void) | null = null;
        let importFailed = false;

        // Subscribe to progress updates - show step-based messages
        // NOTE: Phase 2 runs in background after importDirectory returns,
        // so we must NOT unsubscribe until step 9 is received
        unsubProgress = importService.onProgress(async (event) => {
          // Calculate overall progress based on step (9 steps total)
          // Steps 2-3 have per-file progress, others are single events
          let progress: number;
          if (event.step <= 3 && event.total > 0) {
            // For reading/parsing steps, use item progress within the step
            const stepBase = (event.step - 1) * (100 / 9);
            const stepProgress = (event.current / event.total) * (100 / 9);
            progress = Math.round(stepBase + stepProgress);
          } else {
            // For other steps, progress is just the step percentage
            progress = Math.round((event.step / 9) * 100);
          }

          // Step 9 (complete) shows success message and triggers cleanup
          if (event.step === 9) {
            if (!importFailed) {
              statusBar.success(event.message);
            }
            // Unsubscribe now that import is fully complete
            if (unsubProgress) {
              unsubProgress();
              unsubProgress = null;
            }
            // Refresh collections after background import completes
            await collectionsData.loadCollections();
          } else {
            statusBar.show(event.message, progress);
          }
        });

        try {
          const result = await importService.importDirectory(folderPath, {
            auto_collection_routing: true,
            exclude_patterns: ['design-system', 'node_modules', '.git'],
          });

          // Phase 1 complete - Phase 2 runs in background
          // If there were parsing failures, show error (but don't override progress)
          if (result.failed > 0) {
            importFailed = true;
            statusBar.error(`Import complete: ${result.successful} imported, ${result.failed} failed`);
            // Unsubscribe since we're showing error
            if (unsubProgress) {
              unsubProgress();
              unsubProgress = null;
            }
          }
          // NOTE: Do NOT unsubProgress here - Phase 2 still running
        } catch (error) {
          log.error('Import failed', error);
          importFailed = true;
          statusBar.error('Import failed: ' + (error instanceof Error ? error.message : String(error)));
          // Unsubscribe on error
          if (unsubProgress) {
            unsubProgress();
            unsubProgress = null;
          }
        }
      });

      // Listen for database selection from menu
      unlistenDatabase = listen('menu-select-database', async () => {
        try {
          const result = await invoke<{ newPath: string; requiresRestart: boolean }>(
            'select_new_database'
          );
          if (result.requiresRestart) {
            const confirmed = window.confirm(
              `Database location changed to:\n${result.newPath}\n\nNodeSpace needs to restart to use the new database. Restart now?`
            );
            if (confirmed) {
              await invoke('restart_app');
            }
          }
        } catch (err) {
          if (err !== 'No folder selected') {
            log.error('Database selection failed:', err);
          }
        }
      });

      // Listen for settings menu — open or focus settings tab
      unlistenSettings = listen('menu-open-settings', () => {
        const state = get(tabState);
        const existingSettingsTab = state.tabs.find((t) => t.type === 'settings');
        if (existingSettingsTab) {
          setActiveTab(existingSettingsTab.id, existingSettingsTab.paneId);
        } else {
          const activePaneId = state.activePaneId;
          addTab({
            id: 'settings',
            title: 'Settings',
            type: 'settings',
            closeable: true,
            paneId: activePaneId
          });
        }
      });

      // Set up MCP event listeners for real-time UI updates
      cleanupMCP = setupMCPListeners(SharedNodeStore.getInstance());
    } else {
      // Browser mode: Initialize SSE-based sync for real-time updates
      // This connects to dev-proxy's /api/events endpoint for external change notifications
      browserSyncService.initialize().catch((error) => {
        log.error('Failed to initialize browser sync service:', error);
      });
    }

    // Global click handler for links (nodespace://, http://, https://)
    const handleLinkClick = (event: MouseEvent) => {
      const target = event.target as HTMLElement;

      // Find closest anchor element (handles clicking on children of <a>)
      const anchor = target.closest('a');
      if (!anchor) return;

      const href = anchor.getAttribute('href');
      if (!href) return;

      // Handle external links (http/https) - open in system browser
      if (isExternalUrl(href)) {
        event.preventDefault();
        event.stopPropagation();

        openUrl(href).catch((error) => {
          log.error('Failed to open external link:', error);
          statusBar.error('Failed to open link in browser');
        });
        return;
      }

      // Only handle nodespace:// protocol from here
      if (!isNodespaceUrl(href)) return;

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

      // Validate node ID is not empty
      // NavigationService will handle resolution (UUIDs, date nodes, etc.)
      if (!nodeId || nodeId.trim() === '') {
        log.error('Empty node ID in link');
        statusBar.error('Invalid link: empty document reference');
        return;
      }

      // Check for Cmd+Click (Mac) or Ctrl+Click (Windows/Linux)
      const modifierPressed = event.metaKey || event.ctrlKey;
      const shiftPressed = event.shiftKey;

      // Determine navigation action:
      // - Cmd+Shift+Click: Open in other pane (don't navigate current)
      // - Cmd+Click: Open in new tab (same pane)
      // - Click: Navigate in current tab
      const openInOtherPane = modifierPressed && shiftPressed;
      const openInNewTab = modifierPressed && !shiftPressed;

      // Prevent default navigation for modifier key combinations
      if (modifierPressed) {
        event.preventDefault();
      }

      // Navigate using NavigationService (lazy import)
      (async () => {
        const { getNavigationService } = await import('$lib/services/navigation-service');
        const navService = getNavigationService();

        // Find which pane the click originated from by traversing up the DOM
        const sourcePaneElement = (event.target as HTMLElement).closest('[data-pane-id]');
        const sourcePaneId = sourcePaneElement?.getAttribute('data-pane-id') ?? undefined;

        // Pre-resolve node target to provide user feedback before navigation.
        // Note: Navigation methods call resolveNodeTarget again internally, but the result
        // is cached in SharedNodeStore so this doesn't cause redundant database queries.
        const target = await navService.resolveNodeTarget(nodeId);

        if (!target) {
          // Node not found - show user-friendly error instead of crashing
          log.warn(`Broken link: node ${nodeId} not found`);
          statusBar.error('This link points to a deleted or non-existent document');
          return;
        }

        if (openInOtherPane) {
          // Cmd+Shift+Click: Open in OTHER pane (not the source pane)
          navService.navigateToNodeInOtherPane(nodeId, sourcePaneId);
        } else {
          // Regular/Cmd+Click: Navigate in source pane
          navService.navigateToNode(nodeId, openInNewTab, sourcePaneId);
        }
      })();
    };

    // Attach global event listener in capture phase (fires before bubble phase)
    // This ensures we catch the event before any other handlers
    document.addEventListener('click', handleLinkClick, true);

    return async () => {
      // Flush any pending tab state saves before unmounting
      TabPersistenceService.flush();

      cleanup?.();
      if (unlistenMenu) {
        (await unlistenMenu)();
      }
      if (unlistenStatusBar) {
        (await unlistenStatusBar)();
      }
      if (unlistenImport) {
        (await unlistenImport)();
      }
      if (unlistenDatabase) {
        (await unlistenDatabase)();
      }
      if (unlistenSettings) {
        (await unlistenSettings)();
      }
      if (cleanupMCP) {
        await cleanupMCP();
      }
      // Cleanup stale nodes polling interval
      if (staleNodesInterval) {
        clearInterval(staleNodesInterval);
      }
      // Cleanup browser sync service (SSE connection)
      browserSyncService.destroy();
      // Cleanup click handler (must match capture phase flag)
      document.removeEventListener('click', handleLinkClick, true);
    };
  });

  // Subscribe to layout state
  $: isCollapsed = $layoutState.sidebarCollapsed;

  // Handle global keyboard shortcuts
  function handleKeydown(event: KeyboardEvent) {
    // Toggle theme - Cmd+\ (Mac) or Ctrl+\ (Windows/Linux)
    if ((event.metaKey || event.ctrlKey) && event.key === '\\') {
      event.preventDefault();
      toggleTheme();
    }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

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
    <div class="app-container">
      <div
        class="app-shell"
        class:sidebar-collapsed={isCollapsed}
        class:sidebar-expanded={!isCollapsed}
        role="application"
        aria-label="NodeSpace Application"
      >
        <!-- Navigation Sidebar -->
        <NavigationSidebar />

        <!-- Pane Manager - positioned to span both tabs and content grid areas -->
        <div class="pane-manager-wrapper">
          <!-- PaneManager now renders content directly via PaneContent components -->
          <PaneManager />
        </div>
      </div>

      <!-- Status Bar - shows import progress, etc. (pushes content up, not overlay) -->
      <StatusBar />
    </div>
  </NodeServiceContext>
</ThemeProvider>

<style>
  /* Container for app-shell and status bar (flexbox column) */
  .app-container {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
  }

  .app-shell {
    display: grid;
    grid-template-areas:
      'sidebar tabs'
      'sidebar content';
    grid-template-columns: auto 1fr;
    grid-template-rows: auto minmax(0, 1fr); /* minmax(0, 1fr) prevents overflow */
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  /* Navigation Sidebar */
  :global(.navigation-sidebar) {
    grid-area: sidebar;
  }

  /* Pane Manager Wrapper - spans both tabs and content areas */
  .pane-manager-wrapper {
    grid-column: 2;
    grid-row: 1 / span 2;
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden; /* Ensure content doesn't overflow when status bar takes space */
    position: relative;
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
</style>
