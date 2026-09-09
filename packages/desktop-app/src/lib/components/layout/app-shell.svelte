<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { invoke } from '@tauri-apps/api/core';
  import NavigationSidebar from './navigation-sidebar.svelte';
  import PaneManager from './pane-manager.svelte';
  import StatusBar from '$lib/components/status-bar.svelte';
  import { statusBar } from '$lib/stores/status-bar.svelte';
  import ThemeProvider from '$lib/design/components/theme-provider.svelte';
  import NodeServiceContext from '$lib/contexts/node-service-context.svelte';
  import { initializeTheme } from '$lib/design/theme';
  import { layoutStore, toggleSidebar, loadPersistedLayoutState } from '$lib/stores/layout.svelte';
  import { registerCorePlugins } from '$lib/plugins/core-plugins';
  import { pluginRegistry } from '$lib/plugins/index';
  import { toggleTheme, setTheme } from '$lib/design/theme';
  import { SharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { browserSyncService } from '$lib/services/browser-sync-service';
  import { MCP_EVENTS } from '$lib/constants';
  import type { Node } from '$lib/types';
  import { collectionsData } from '$lib/stores/collections.svelte';
  import { schemasData } from '$lib/stores/schemas.svelte';
  import { loadPersistedState, addTab, navigationStore, setActiveTab } from '$lib/stores/navigation.svelte';
  import { settingsStore } from '$lib/stores/settings.svelte';
  import { importModalStore } from '$lib/stores/import-modal.svelte';
  import ImportOptionsModal from '$lib/components/settings/import-options-modal.svelte';
  import { TabPersistenceService } from '$lib/services/tab-persistence-service';
  import { createLogger } from '$lib/utils/logger';
  import {
    openUrl,
    isExternalUrl,
    isNodespaceUrl,
    extractNodeIdFromHref
  } from '$lib/utils/external-links';
  import OnboardingWizard from '$lib/components/onboarding/onboarding-wizard.svelte';
  import { proSync } from '$lib/stores/pro-sync.svelte';
  import { getActiveChromeContributions } from '$lib/plugins/ui-extensions.svelte';
  import ExtensionOutlet from '$lib/plugins/extension-outlet.svelte';
  import ConflictToast from '$lib/components/conflict-toast.svelte';
  import NodeRefPreview from '$lib/components/references/node-ref-preview.svelte';
  import { nodeRefPreview } from '$lib/services/node-ref-preview.svelte';
  import UpdateBanner from '$lib/components/update-banner.svelte';
  import { updateStatus } from '$lib/stores/update-status.svelte';
  import { conflictNotifications } from '$lib/stores/conflict-notifications.svelte';
  import { conflictsStore } from '$lib/stores/conflicts.svelte';
  import {
    daemonStatus,
    startDaemonStatusListener,
    refreshDaemonStatus
  } from '$lib/services/daemon-status';
  import { databaseStore } from '$lib/stores/database.svelte';

  // Logger instance for AppShell component
  const log = createLogger('AppShell');

  // Daemon connectivity state. `connecting`
  // covers the brief window before the first daemon-status event arrives (or
  // a short grace period elapses); `unreachable` reflects a terminal
  // `not_running` event.
  // Read directly from the store via $-auto-subscription — no effect mirroring the
  // store into local $state (ADR-049).
  const daemonConnecting = $derived($daemonStatus.connecting);
  const daemonUnreachable = $derived($daemonStatus.unreachable);

  // First-launch onboarding wizard.
  let showOnboarding = $state(false);
  // True when this is the backfill nudge (an already-onboarded
  // install whose seeded local person is still blank) rather than the full
  // first-launch wizard — see the `identityOnly` prop on OnboardingWizard.
  let onboardingIdentityOnly = $state(false);

  // Conflict journal (ADR-068): load on every startup, unconditionally —
  // unlike the deleted Recovered Items log, this is NOT Pro-gated; the
  // journal is designed to work on a purely local-only install. The inline
  // per-node indicator and the Conflicts view both read `conflictsStore`
  // directly; this only decides whether a one-time startup nudge is worth
  // showing when open conflicts already exist.
  function loadConflicts(): void {
    conflictsStore.load().then(() => {
      const open = conflictsStore.records.filter((r) => r.status === 'open');
      if (open.length === 0) return;
      conflictNotifications.add({
        nodeId: open[0].nodeIds[0] ?? '',
        message: `${open.length} unresolved conflict${open.length === 1 ? '' : 's'} — open Conflicts to review.`,
        conflictType: 'conflict-journal'
      });
    });
  }

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
          // ADR-053: capture the database generation so a switch mid-fetch drops
          // this write instead of writing the previous database's node into the
          // now-active store.
          const epoch = sharedNodeStore.currentEpoch();
          const node = await invoke<Node>('get_node', { id: event.payload.node_id });
          if (node && sharedNodeStore.currentEpoch() === epoch) {
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

    // Subscribe to the app-update check (startup event + on-demand). Best-effort;
    // surfaces the non-blocking <UpdateBanner /> only when a newer release exists.
    void updateStatus.init();

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
    let unlistenSettings: Promise<() => void> | null = null;
    let unlistenIntegrations: Promise<() => void> | null = null;
    let unlistenSkillCliMissing: Promise<() => void> | null = null;
    let unlistenSkillInstallFailed: Promise<() => void> | null = null;
    let cleanupMCP: (() => Promise<void>) | null = null;
    let staleNodesInterval: ReturnType<typeof setInterval> | null = null;
    let cleanupProSync: (() => void) | null = null;
    let unlistenTier: Promise<() => void> | null = null;
    let unlistenSelectDatabase: Promise<() => void> | null = null;

    if (
      typeof window !== 'undefined' &&
      (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
    ) {
      // Pro-tier sync listener. The pill component returns null
      // when the daemon's capability probe says community-tier, so
      // starting the listener unconditionally is safe and lets the
      // tier-detected event update the store reactively.
      proSync
        .start()
        .then((stop) => (cleanupProSync = stop))
        .catch((e) => log.warn('proSync.start failed', { error: e }));

      // Load the conflict journal on every startup — unconditional, not
      // gated on Pro tier (ADR-068: the journal works on a purely
      // local-only install, which is exactly the defect it fixes).
      loadConflicts();

      // Sync theme from backend preferences (overrides localStorage if different)
      invoke<{ activeDatabasePath: string; display: { renderMarkdown: boolean; theme: string } }>('get_settings')
        .then((settings) => {
          const savedTheme = settings.display.theme;
          if (['system', 'light', 'dark'].includes(savedTheme)) {
            setTheme(savedTheme as 'system' | 'light' | 'dark');
          }
        })
        .catch((err) => {
          log.debug('Could not sync theme from backend preferences:', err);
        });

      // Re-load schemas and collections once the daemon's Pro-tier capability probe
      // resolves. Kept in addition to the generic daemon-reconnect hook below since
      // Pro tier detection is a distinct signal that can complete after the initial
      // healthy event (it depends on gRPC being reachable, not just the socket).
      unlistenTier = listen('pro:tier-detected', () => {
        schemasData.loadSchemas();
        collectionsData.loadCollections();
      });

      // Picking a database from the tray while the app is already running
      // relaunches a second process; `tauri-plugin-single-instance` detects
      // this one is already up, focuses its window (Rust side), and forwards
      // the requested database here instead of the relaunch spawning a second
      // window. Switch through the same path an in-app switcher click uses.
      unlistenSelectDatabase = listen<string>('tray:select-database', (event) => {
        databaseStore.switchTo(event.payload).catch((err) => {
          log.error('Failed to switch database from tray selection:', err);
        });
      });

      // A relaunch can arrive before this `listen()` call above has finished
      // its IPC round-trip (the webview is still booting) — Tauri does not
      // buffer an event emitted with zero listeners, so that live event is
      // dropped on the floor. Pull the pull-based fallback stash once the
      // listener is guaranteed to be registered so a pick that raced boot
      // still lands.
      unlistenSelectDatabase.then(() => {
        databaseStore.applyPendingTraySelection().catch((err) => {
          log.error('Failed to apply pending tray database selection:', err);
        });
      });

      // Show a warning if the nodespace CLI is not on $PATH after skill install.
      unlistenSkillCliMissing = listen<{ warning: string }>('skill:cli-missing', (event) => {
        log.warn('nodespace CLI not on PATH:', event.payload.warning);
        statusBar.error(event.payload.warning);
      });

      // Surface a genuine skill-install failure once (backend suppresses the
      // event on repeat failures across launches — see skill_setup.rs).
      unlistenSkillInstallFailed = listen<{ error: string }>('skill:install-failed', (event) => {
        log.warn('NodeSpace skill install failed:', event.payload.error);
        statusBar.error(`NodeSpace skill installation failed: ${event.payload.error}`);
      });

      // Start the shared daemon-status listener.
      // Drives daemonConnecting/daemonUnreachable above and fans out reconnect
      // events to any store registered via onDaemonReconnect.
      startDaemonStatusListener();

      // Show first-launch onboarding wizard if setup has not been completed.
      // Otherwise, an already-onboarded install whose seeded
      // local person is still blank gets the lightweight backfill nudge
      // instead — never both in the same launch.
      invoke<{ completed: boolean }>('check_onboarding_status')
        .then((status) => {
          if (!status.completed) {
            showOnboarding = true;
            return;
          }
          invoke<boolean>('should_prompt_identity_backfill')
            .then((shouldPrompt) => {
              if (shouldPrompt) {
                onboardingIdentityOnly = true;
                showOnboarding = true;
              }
            })
            .catch((err) => log.debug('Could not check identity backfill status:', err));
        })
        .catch((err) => log.debug('Could not check onboarding status:', err));

      unlistenMenu = listen('menu-toggle-sidebar', () => {
        toggleSidebar();
      });

      // Listen for status bar toggle from View menu
      unlistenStatusBar = listen('menu-toggle-status-bar', () => {
        statusBar.toggle();
      });

      // Poll for stale nodes count (embedding queue) every 5 seconds
      let lastStaleCount = 0;
      const updateStaleNodesCount = async () => {
        try {
          const count = await invoke<number>('get_stale_root_count');
          if (count > 0) {
            statusBar.show(`${count} nodes queued for vector indexing`);
          } else if (lastStaleCount > 0) {
            // Only clear if we were previously showing a stale count
            statusBar.clearMessage();
          }
          lastStaleCount = count;
        } catch (error) {
          log.error('Failed to get stale nodes count:', error);
        }
      };
      // Initial fetch
      updateStaleNodesCount();
      // Set up polling interval
      staleNodesInterval = setInterval(updateStaleNodesCount, 5000);

      // Listen for import folder menu event — opens the same import options
      // modal Settings uses, rather than importing directly. Both entry
      // points now share one flow (see ImportOptionsModal).
      unlistenImport = listen('menu-import-folder', () => {
        importModalStore.show();
      });

      // Listen for settings menu — open or focus settings tab
      unlistenSettings = listen('menu-open-settings', () => {
        const state = navigationStore.state;
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

      // Listen for File → Integrations menu — open settings tab focused on Integrations.
      // Sets settingsInitialCategory before addTab so the pane reads it synchronously on mount.
      unlistenIntegrations = listen('menu-open-integrations', () => {
        settingsStore.initialCategory = 'integrations';
        const state = navigationStore.state;
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

      // Extract node ID (handles nodespace://uuid, nodespace://node/uuid,
      // and trailing query params). NavigationService resolves it further.
      const nodeId = extractNodeIdFromHref(href);
      if (!nodeId) {
        log.error('Empty node ID in link');
        statusBar.error('Invalid link: empty document reference');
        return;
      }

      // Check for Cmd+Click (Mac) or Ctrl+Click (Windows/Linux)
      const modifierPressed = event.metaKey || event.ctrlKey;
      const shiftPressed = event.shiftKey;

      // Find which pane the click originated from by traversing up the DOM
      const sourcePaneElement = (event.target as HTMLElement).closest('[data-pane-id]');
      const sourcePaneId = sourcePaneElement?.getAttribute('data-pane-id') ?? undefined;

      // Determine navigation action:
      // Click = in-place, Cmd+Click = new tab, Cmd+Shift+Click = other pane
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

    // --- Node-reference hover/focus previews -------------------------------
    // Both reference render paths (markdown-link and NodeRefInline) resolve to
    // the same `a.ns-noderef` anchor, so one delegated trigger covers them all.
    // The controller owns the reveal delay and content; the card renders itself.
    const nodeRefAnchor = (target: unknown): HTMLElement | null => {
      if (!(target instanceof Element)) return null;
      const anchor = target.closest('a.ns-noderef');
      if (!(anchor instanceof HTMLElement)) return null;
      return isNodespaceUrl(anchor.getAttribute('href') ?? '') ? anchor : null;
    };

    const handleRefPointerOver = (event: MouseEvent) => {
      const anchor = nodeRefAnchor(event.target);
      if (anchor) nodeRefPreview.requestPreview(anchor);
    };

    const handleRefPointerOut = (event: MouseEvent) => {
      const anchor = nodeRefAnchor(event.target);
      if (!anchor) return;
      // Ignore moves that stay within the same anchor (e.g. onto a nested span).
      const next = event.relatedTarget;
      if (next instanceof HTMLElement && anchor.contains(next)) return;
      nodeRefPreview.hide();
    };

    const handleRefFocusIn = (event: FocusEvent) => {
      const anchor = nodeRefAnchor(event.target);
      if (anchor) nodeRefPreview.requestPreview(anchor);
    };

    const handleRefFocusOut = (event: FocusEvent) => {
      // Only dismiss when focus actually leaves a reference — a card shown via
      // hover must not be torn down by an unrelated element losing focus.
      if (nodeRefAnchor(event.target)) nodeRefPreview.hide();
    };

    const handleRefDismiss = (event: KeyboardEvent) => {
      if (event.key === 'Escape') nodeRefPreview.hide();
    };

    // Scroll shifts anchors out from under a fixed card — dismiss rather than
    // chase. Capture phase catches scrolls on any nested scroll container.
    const handleRefScroll = () => nodeRefPreview.hide();

    document.addEventListener('mouseover', handleRefPointerOver);
    document.addEventListener('mouseout', handleRefPointerOut);
    document.addEventListener('focusin', handleRefFocusIn);
    document.addEventListener('focusout', handleRefFocusOut);
    document.addEventListener('keydown', handleRefDismiss);
    document.addEventListener('scroll', handleRefScroll, true);

    return async () => {
      // Flush any pending tab state saves before unmounting
      TabPersistenceService.flush();

      cleanup?.();
      updateStatus.stop();
      if (unlistenMenu) {
        (await unlistenMenu)();
      }
      if (unlistenStatusBar) {
        (await unlistenStatusBar)();
      }
      if (unlistenImport) {
        (await unlistenImport)();
      }
      if (unlistenSettings) {
        (await unlistenSettings)();
      }
      if (unlistenIntegrations) {
        (await unlistenIntegrations)();
      }
      if (unlistenSkillCliMissing) {
        (await unlistenSkillCliMissing)();
      }
      if (unlistenSkillInstallFailed) {
        (await unlistenSkillInstallFailed)();
      }
      if (unlistenTier) {
        (await unlistenTier)();
      }
      if (unlistenSelectDatabase) {
        (await unlistenSelectDatabase)();
      }
      cleanupProSync?.();
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

      // Cleanup node-reference preview triggers
      document.removeEventListener('mouseover', handleRefPointerOver);
      document.removeEventListener('mouseout', handleRefPointerOut);
      document.removeEventListener('focusin', handleRefFocusIn);
      document.removeEventListener('focusout', handleRefFocusOut);
      document.removeEventListener('keydown', handleRefDismiss);
      document.removeEventListener('scroll', handleRefScroll, true);
      nodeRefPreview.hide();
    };
  });

  // Subscribe to layout state
  const isCollapsed = $derived(layoutStore.state.sidebarCollapsed);

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
      {#if daemonUnreachable}
        <div class="daemon-error-banner" role="alert">
          <span>
            NodeSpace background service is not running. Some features may be unavailable.
          </span>
          <button onclick={() => refreshDaemonStatus()}> Retry </button>
        </div>
      {:else if daemonConnecting}
        <div class="daemon-connecting-banner" role="status">
          <span>Connecting to local service…</span>
        </div>
      {/if}
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

        <!-- Pro chrome overlay slot (sync pill / upgrade teaser / enable-sync
             prompt) — registry-driven. The active variant resolves from
             the daemon tier and the active database's DatabaseSettingsNode, so the
             community build shows an upgrade teaser and each Pro state its own
             pill. Floats top-right of the app-shell content. -->
        <div class="pro-sync-pill-slot">
          {#each getActiveChromeContributions('app-shell-overlay') as c (c.variant)}
            <ExtensionOutlet load={c.lazyLoad} />
          {/each}
        </div>
      </div>

      <!-- Status Bar - shows import progress, etc. (pushes content up, not overlay) -->
      <StatusBar />
    </div>

    <!-- First-launch onboarding wizard (or the identity backfill nudge) -->
    <OnboardingWizard
      open={showOnboarding}
      identityOnly={onboardingIdentityOnly}
      onClose={() => {
        showOnboarding = false;
        onboardingIdentityOnly = false;
      }}
    />

    <!-- Hover/focus preview card for node references (single shared instance) -->
    <NodeRefPreview />

    <!-- Conflict resolution notifications -->
    <ConflictToast />

    <!-- Non-blocking "a newer NodeSpace release is available" banner. -->
    <UpdateBanner />

    <!-- Shared import-folder modal (File → Import → "Import Folder..." menu
         and Settings → Import Sources both open this single instance). -->
    <ImportOptionsModal bind:open={importModalStore.open} />

    <!-- Pro chrome modal slot (re-login prompt when the daemon's session can't be
         refreshed, T18) — registry-driven. -->
    {#each getActiveChromeContributions('app-shell-modal') as c (c.variant)}
      <ExtensionOutlet load={c.lazyLoad} />
    {/each}
  </NodeServiceContext>
</ThemeProvider>

<style>
  .daemon-error-banner {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 1rem;
    padding: 0.5rem 1rem;
    background: hsl(var(--destructive) / 0.15);
    border-bottom: 1px solid hsl(var(--destructive) / 0.4);
    color: hsl(var(--destructive-foreground));
    font-size: 0.875rem;
    z-index: 100;
  }

  .daemon-error-banner button {
    padding: 0.2rem 0.75rem;
    border-radius: 4px;
    border: 1px solid hsl(var(--destructive) / 0.6);
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-size: 0.8rem;
  }

  .daemon-error-banner button:hover {
    background: hsl(var(--destructive) / 0.2);
  }

  .daemon-connecting-banner {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0.5rem 1rem;
    background: hsl(var(--muted) / 0.5);
    border-bottom: 1px solid hsl(var(--border));
    color: hsl(var(--muted-foreground));
    font-size: 0.875rem;
    z-index: 100;
  }

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
    position: relative; /* anchor for .pro-sync-pill-slot */
  }

  /* Pro-tier sync-status pill (floats over the content grid).
     Hidden when the capability probe returns community-tier. */
  .pro-sync-pill-slot {
    position: absolute;
    top: 8px;
    right: 12px;
    z-index: 10;
    pointer-events: auto;
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
