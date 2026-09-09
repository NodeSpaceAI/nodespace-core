<script lang="ts">
  import { Collapsible } from 'bits-ui';
  import {
    layoutStore,
    toggleSidebar,
    setCollectionsExpanded,
    setSchemaTypesExpanded,
    setAiChatsExpanded
  } from '$lib/stores/layout.svelte';
  import { navigationStore, setActiveTab, addTab } from '$lib/stores/navigation.svelte';
  import { openSettings } from '$lib/utils/open-settings';
  import { openConflicts } from '$lib/utils/open-conflicts';
  import { collectionsState, collectionsData } from '$lib/stores/collections.svelte';
  import { formatDateISO } from '$lib/utils/date-formatting.js';
  import { getNavigationService } from '$lib/services/navigation-service';
  import CollectionSubPanel from './collection-sub-panel.svelte';
  import DatabaseSwitcher from './database-switcher.svelte';
  import { onMount, onDestroy } from 'svelte';
  import { schemasStore, schemasData } from '$lib/stores/schemas.svelte';
  import { aiChatsData } from '$lib/stores/ai-chats.svelte';
  import {
    clearCollectionRefreshTimer,
    clearSchemaRefreshTimer,
    clearAiChatRefreshTimer
  } from '$lib/utils/collection-refresh';
  import { aiChatDisplayTitle } from '$lib/utils/ai-chat-title';

  // Read reactive store state directly (ADR-049)
  let isCollapsed = $derived(layoutStore.state.sidebarCollapsed);
  let navItems = $derived(layoutStore.navigationItems);
  // Collections expanded state from layout store (persisted)
  let collectionsExpanded = $derived(layoutStore.state.collectionsExpanded);
  // Schema Types expanded state from layout store (persisted)
  let schemaTypesExpanded = $derived(layoutStore.state.schemaTypesExpanded);
  // AI Chats expanded state from layout store (persisted)
  let aiChatsExpanded = $derived(layoutStore.state.aiChatsExpanded);

  // Collections state from collections store (UI-only, not persisted)
  let subPanelOpen = $derived(collectionsState.state.subPanelOpen);
  let expandedCollectionIds = $derived(collectionsState.state.expandedCollectionIds);

  // Collections data from backend
  let collections = $derived(collectionsData.collectionsTree);

  // Computed getters for sub-panel
  let collectionForPanel = $derived(collectionsState.selectedCollection);
  let collectionMembers = $derived(collectionsState.selectedCollectionMembers);

  // Inline "New collection" form for the Collections section. New collections are
  // created open (the app has no privacy toggle yet). A new collection starts with
  // zero members, so the store inserts it optimistically and exempts it from the
  // hide-empty filter — that is what makes it appear here the instant it is named.
  let creatingCollection = $state(false);
  let newCollectionName = $state('');
  let createBusy = $state(false);
  // Message shown when a create is rolled back. Optimistic UI hides the round
  // trip, so without this a rejected create (a duplicate name is the common
  // one) would just make the row silently vanish with no explanation.
  let createError = $state('');

  function focusOnMount(node: HTMLInputElement) {
    node.focus();
  }

  async function submitNewCollection() {
    const name = newCollectionName.trim();
    if (!name || createBusy) return;
    // Dismiss the form immediately — the store inserts the collection into the
    // list optimistically, so the user sees the new entry rather than a form
    // frozen on a round-trip.
    createBusy = true;
    createError = '';
    newCollectionName = '';
    creatingCollection = false;

    const id = await collectionsData.createCollection(name);
    createBusy = false;

    const failure = collectionsData.state.error;
    if (!id && failure) {
      // Rolled back. Reopen the form with the name restored so the user can see
      // what failed and correct it (e.g. pick a non-duplicate name).
      createError = failure;
      newCollectionName = name;
      creatingCollection = true;
    }
    // A null id with no error means the create was discarded because the store
    // moved to a different database. That collection belongs to the database we
    // left, so there is nothing to report or retry here — leave the form closed.
  }

  function cancelNewCollection() {
    newCollectionName = '';
    creatingCollection = false;
    createError = '';
  }

  // Schema types from global store (reactive — updates when schemas are created/deleted externally)
  let builtInSchemas = $derived(schemasStore.builtInSchemas);
  let customSchemas = $derived(schemasStore.customSchemas);

  // AI Chats list from global store (reactive — updates when chats are
  // created/updated elsewhere, via node:created/node:updated wiring in
  // tauri-sync-listener.ts's scheduleAiChatRefresh, same as Collections/Schema
  // Types above)
  let aiChats = $derived(aiChatsData.state.chats);
  let createChatBusy = $derived(aiChatsData.createBusy);
  let createChatError = $derived(aiChatsData.createError);

  // Load collections, schemas, and AI chats from backend on mount
  onMount(() => {
    collectionsData.loadCollections();
    schemasData.loadSchemas();
    aiChatsData.loadAiChats();
  });

  // Cancel any pending debounced refreshes when the sidebar is destroyed
  // to prevent async callbacks from firing after teardown.
  onDestroy(() => {
    clearCollectionRefreshTimer();
    clearSchemaRefreshTimer();
    clearAiChatRefreshTimer();
  });

  // Element references for click-outside detection
  let navElement: HTMLElement | null = $state(null);
  let subPanelElement: HTMLElement | null = $state(null);

  // Close sub-panel when the sidebar collapses (sidebar can only collapse via handleToggleSidebar)
  function handleToggleSidebar() {
    toggleSidebar();
    if (subPanelOpen) {
      collectionsState.clearSelection();
    }
  }

  // Click-outside handler for sub-panel dismissal
  $effect(() => {
    if (!subPanelOpen) return;

    function handleClickOutside(event: MouseEvent) {
      const target = event.target as Node;
      // Close if click is outside both nav and sub-panel
      const clickedOutsideNav = navElement && !navElement.contains(target);
      const clickedOutsideSubPanel = subPanelElement && !subPanelElement.contains(target);

      if (clickedOutsideNav && clickedOutsideSubPanel) {
        collectionsState.clearSelection();
      }
    }

    // Use capture phase to catch clicks before they bubble
    document.addEventListener('click', handleClickOutside, true);

    return () => {
      document.removeEventListener('click', handleClickOutside, true);
    };
  });

  function isCollectionExpanded(collectionId: string): boolean {
    return expandedCollectionIds.has(collectionId);
  }

  function handleCollectionClick(collectionId: string) {
    collectionsState.selectCollection(collectionId);
  }

  function handleCloseSubPanel() {
    collectionsState.closeSubPanel();
  }

  function handleNodeClick(nodeId: string, nodeType: string) {
    // Close sub-panel first
    handleCloseSubPanel();

    getNavigationService().focusOrOpenNode(nodeId, { nodeType });
  }

  /**
   * Get today's date in YYYY-MM-DD format
   */
  function getTodayDateId(): string {
    return formatDateISO(new Date());
  }

  /**
   * Handle Daily Journal navigation: focus today's date tab, or open one.
   *
   * `matchNodeType` because a date id is a date string rather than a real node
   * id, so an unqualified id match could focus some other tab that happens to
   * carry the same string.
   */
  function handleDailyJournalClick() {
    const todayId = getTodayDateId();
    getNavigationService().focusOrOpenNode(todayId, {
      nodeType: 'date',
      // Placeholder - DateNodeViewer updates it to "Today" on mount
      title: todayId,
      matchNodeType: true
    });
  }

  function handleSchemaClick(schemaId: string) {
    // `nodeType: 'query'` routes the tab to QueryNodeViewer; the viewer itself
    // branches on the loaded node (schema id → default type view, query id →
    // saved query) rather than trusting this decorative flag.
    // TODO: nest materialized saved queries under their type here.
    getNavigationService().focusOrOpenNode(schemaId, { nodeType: 'query' });
  }

  /**
   * Open (or focus) an ai-chat node in a tab, routed to `AiChatNodeViewer` via
   * `nodeType: 'ai-chat'` instead of `handleSchemaClick`'s hardcoded `'query'`.
   */
  function handleAiChatClick(chatId: string) {
    getNavigationService().focusOrOpenNode(chatId, { nodeType: 'ai-chat' });
  }

  /**
   * "+ New chat": create an ai-chat node immediately (no name prompt — a
   * chat's title comes from its content later) and open it. On failure,
   * `aiChatsData.createChat` leaves `createError` set for the section to
   * display; nothing to open in that case.
   */
  async function handleNewChat() {
    const created = await aiChatsData.createChat();
    if (created) {
      handleAiChatClick(created.id);
    }
  }

  /**
   * Open (or focus) the Search view. Search is not a node, so it opens a
   * singleton non-node tab — mirroring how Settings is opened from the menu —
   * rather than the daily-journal node-tab path.
   */
  function openSearchTab() {
    const currentState = navigationStore.state;
    const existingTab = currentState.tabs.find((tab) => tab.type === 'search');

    if (existingTab) {
      setActiveTab(existingTab.id, existingTab.paneId);
    } else {
      addTab(
        {
          id: 'search',
          title: 'Search',
          type: 'search',
          closeable: true,
          paneId: getNavigationService().targetPaneId()
        },
        true
      );
    }
  }

  // Handle navigation item clicks
  function handleNavItemClick(itemId: string) {
    // Close sub-panels when clicking non-collection nav items
    if (subPanelOpen) {
      collectionsState.clearSelection();
    }

    // Each nav item opens its own view (Favorites is not implemented yet).
    if (itemId === 'daily-journal') {
      handleDailyJournalClick();
    } else if (itemId === 'search') {
      openSearchTab();
    } else if (itemId === 'settings') {
      // Shared singleton tab (same one the File menu + database indicator open).
      openSettings();
    } else if (itemId === 'conflicts') {
      openConflicts();
    }

    // Update active state in navigation items
    layoutStore.setActiveNavItem(itemId);
  }
</script>

<nav
  bind:this={navElement}
  class="sidebar navigation-sidebar"
  class:sidebar-collapsed={isCollapsed}
  class:sidebar-expanded={!isCollapsed}
>
  <!-- Hamburger menu button -->
  <button
    class="hamburger-button"
    onclick={handleToggleSidebar}
    aria-label={isCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}
    aria-expanded={!isCollapsed}
  >
    <svg
      class="hamburger-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
    >
      <line x1="3" y1="6" x2="21" y2="6"></line>
      <line x1="3" y1="12" x2="21" y2="12"></line>
      <line x1="3" y1="18" x2="21" y2="18"></line>
    </svg>
  </button>

  <!-- Navigation items -->
  <div class="nav-items">
    <!-- Database switcher (ADR-053) — sidebar header. Hidden when collapsed
         since the trigger needs room for the database name. -->
    {#if !isCollapsed}
      <DatabaseSwitcher />
    {/if}

    <!-- Daily Journal (first item) -->
    {#each navItems.slice(0, 1) as item}
      <button
        class="nav-item"
        onclick={() => handleNavItemClick(item.id)}
        aria-label={item.label}
        disabled={item.type === 'placeholder'}
        title={isCollapsed ? item.label : undefined}
      >
        <svg
          class="nav-icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
        >
          <path d={item.icon}></path>
        </svg>
        {#if !isCollapsed}
          <span class="nav-label">{item.label}</span>
        {/if}
      </button>
    {/each}

    <!-- Collections section (after Daily Journal) - accordion toggle -->
    {#if !isCollapsed}
      <Collapsible.Root open={collectionsExpanded} onOpenChange={(open) => setCollectionsExpanded(open)}>
        <Collapsible.Trigger
          class="nav-item"
          aria-label={collectionsExpanded ? 'Collapse Collections' : 'Expand Collections'}
        >
          <svg
            class="nav-icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
          >
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
            {#if collectionsExpanded}
              <path d="M2 10h20" />
            {/if}
          </svg>
          <span class="nav-label">Collections</span>
        </Collapsible.Trigger>

        <Collapsible.Content>
          <div class="collection-list">
            {#each collections as collection (collection.id)}
              {@const hasChildren = collection.children && collection.children.length > 0}
              {@const isExpanded = isCollectionExpanded(collection.id)}
              <div class="collection-item" class:pending={collection.pending}>
                <div class="expand-area">
                  {#if hasChildren}
                    <button
                      class="expand-btn"
                      onclick={() => collectionsState.toggleCollectionExpanded(collection.id)}
                      aria-label={isExpanded ? 'Collapse' : 'Expand'}
                    >
                      <svg
                        class="expand-chevron"
                        class:rotate-90={isExpanded}
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                      >
                        <path d="M9 18l6-6-6-6" />
                      </svg>
                    </button>
                  {/if}
                </div>
                <button
                  class="collection-name-btn"
                  onclick={() => handleCollectionClick(collection.id)}
                >
                  {collection.name}
                </button>
              </div>

              <!-- Level 2 -->
              {#if hasChildren && isExpanded && collection.children}
                {#each collection.children as child (child.id)}
                  {@const childHasChildren = child.children && child.children.length > 0}
                  {@const childIsExpanded = isCollectionExpanded(child.id)}
                  <div class="collection-item level-2">
                    <div class="expand-area">
                      {#if childHasChildren}
                        <button
                          class="expand-btn"
                          onclick={() => collectionsState.toggleCollectionExpanded(child.id)}
                          aria-label={childIsExpanded ? 'Collapse' : 'Expand'}
                        >
                          <svg
                            class="expand-chevron"
                            class:rotate-90={childIsExpanded}
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2"
                          >
                            <path d="M9 18l6-6-6-6" />
                          </svg>
                        </button>
                      {/if}
                    </div>
                    <button
                      class="collection-name-btn"
                      onclick={() => handleCollectionClick(child.id)}
                    >
                      {child.name}
                    </button>
                  </div>

                  <!-- Level 3 -->
                  {#if childHasChildren && childIsExpanded && child.children}
                    {#each child.children as grandchild (grandchild.id)}
                      <div class="collection-item level-3">
                        <div class="expand-area"></div>
                        <button
                          class="collection-name-btn"
                          onclick={() => handleCollectionClick(grandchild.id)}
                        >
                          {grandchild.name}
                        </button>
                      </div>
                    {/each}
                  {/if}
                {/each}
              {/if}
            {/each}

            <!-- Create a new collection -->
            {#if creatingCollection}
              <div class="collection-item">
                <div class="expand-area"></div>
                <input
                  class="new-collection-input"
                  placeholder="Collection name…"
                  aria-label="New collection name"
                  aria-invalid={createError ? 'true' : undefined}
                  bind:value={newCollectionName}
                  use:focusOnMount
                  onkeydown={(e) => {
                    if (e.key === 'Enter') submitNewCollection();
                    else if (e.key === 'Escape') cancelNewCollection();
                  }}
                />
              </div>
              {#if createError}
                <div class="collection-item new-collection-error" role="alert">
                  <div class="expand-area"></div>
                  <span>{createError}</span>
                </div>
              {/if}
            {:else}
              <div class="collection-item">
                <div class="expand-area"></div>
                <button
                  class="collection-name-btn new-collection-btn"
                  onclick={() => (creatingCollection = true)}
                  title="Create a new collection"
                >
                  + New collection
                </button>
              </div>
            {/if}
          </div>
        </Collapsible.Content>
      </Collapsible.Root>
    {:else}
      <!-- Collapsed state: just show icon -->
      <button
        class="nav-item"
        title="Collections"
        onclick={() => {
          toggleSidebar();
          setCollectionsExpanded(true);
        }}
      >
        <svg
          class="nav-icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
        >
          <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
        </svg>
      </button>
    {/if}

    <!-- Schema Types section (after Collections) - accordion toggle -->
    {#if !isCollapsed}
      <Collapsible.Root open={schemaTypesExpanded} onOpenChange={(open) => setSchemaTypesExpanded(open)}>
        <Collapsible.Trigger
          class="nav-item"
          aria-label={schemaTypesExpanded ? 'Collapse Schema Types' : 'Expand Schema Types'}
        >
          <svg
            class="nav-icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <!-- 4-shape icon: triangle (top-left), square (top-right), square (bottom-left), diamond (bottom-right) -->
            <path d="M3 3 L9 3 L6 8 Z" />
            <rect x="11" y="3" width="6" height="6" />
            <rect x="3" y="13" width="6" height="6" />
            <path d="M14 13 L17 16 L14 19 L11 16 Z" />
          </svg>
          <span class="nav-label">Schema Types</span>
        </Collapsible.Trigger>

        <Collapsible.Content>
          <div class="schema-type-list">
            {#if builtInSchemas.length === 0 && customSchemas.length === 0}
              <span class="schema-type-empty">No types available</span>
            {:else}
              {#each builtInSchemas as schema (schema.id)}
                <button class="schema-type-item" onclick={() => handleSchemaClick(schema.id)}>
                  <span class="schema-type-name">{schema.content}</span>
                </button>
              {/each}

              {#if customSchemas.length > 0}
                {#if builtInSchemas.length > 0}
                  <div class="schema-type-separator"></div>
                {/if}
                {#each customSchemas as schema (schema.id)}
                  <button class="schema-type-item" onclick={() => handleSchemaClick(schema.id)}>
                    <span class="schema-type-name">{schema.content}</span>
                  </button>
                {/each}
              {/if}
            {/if}
          </div>
        </Collapsible.Content>
      </Collapsible.Root>
    {:else}
      <!-- Collapsed state: just show icon -->
      <button
        class="nav-item"
        title="Schema Types"
        onclick={() => {
          toggleSidebar();
          setSchemaTypesExpanded(true);
        }}
      >
        <svg
          class="nav-icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="M3 3 L9 3 L6 8 Z" />
          <rect x="11" y="3" width="6" height="6" />
          <rect x="3" y="13" width="6" height="6" />
          <path d="M14 13 L17 16 L14 19 L11 16 Z" />
        </svg>
      </button>
    {/if}

    <!-- AI Chats section (after Schema Types) - accordion toggle -->
    {#if !isCollapsed}
      <Collapsible.Root open={aiChatsExpanded} onOpenChange={(open) => setAiChatsExpanded(open)}>
        <Collapsible.Trigger
          class="nav-item"
          aria-label={aiChatsExpanded ? 'Collapse AI Chats' : 'Expand AI Chats'}
        >
          <svg
            class="nav-icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
          >
            <!-- Reuses the existing "aiSquare" glyph (design/icons/ui/ai-square.ts) -->
            <rect
              x="2"
              y="2"
              width="12"
              height="12"
              rx="2"
              ry="2"
              fill="currentColor"
              opacity="0.1"
              stroke="currentColor"
              stroke-width="1"
            />
            <text
              x="8"
              y="10.5"
              font-family="system-ui, -apple-system, sans-serif"
              font-size="6"
              font-weight="600"
              text-anchor="middle"
              fill="currentColor">AI</text
            >
          </svg>
          <span class="nav-label">AI Chats</span>
        </Collapsible.Trigger>

        <Collapsible.Content>
          <div class="ai-chat-list">
            {#if aiChats.length === 0}
              <span class="schema-type-empty">No chats yet</span>
            {:else}
              {#each aiChats as chat (chat.id)}
                <button class="ai-chat-item" onclick={() => handleAiChatClick(chat.id)}>
                  <span class="ai-chat-name">{aiChatDisplayTitle(chat.content)}</span>
                </button>
              {/each}
            {/if}

            <!-- Create a new chat: immediate, no name prompt (unlike collections) -->
            <button
              class="ai-chat-item ai-chat-new-btn"
              onclick={handleNewChat}
              disabled={createChatBusy}
              title="Start a new chat"
            >
              + New chat
            </button>
            {#if createChatError}
              <div class="ai-chat-item ai-chat-error" role="alert">
                <span>{createChatError}</span>
              </div>
            {/if}
          </div>
        </Collapsible.Content>
      </Collapsible.Root>
    {:else}
      <!-- Collapsed state: just show icon -->
      <button
        class="nav-item"
        title="AI Chats"
        onclick={() => {
          toggleSidebar();
          setAiChatsExpanded(true);
        }}
      >
        <svg
          class="nav-icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
        >
          <rect
            x="2"
            y="2"
            width="12"
            height="12"
            rx="2"
            ry="2"
            fill="currentColor"
            opacity="0.1"
            stroke="currentColor"
            stroke-width="1"
          />
          <text
            x="8"
            y="10.5"
            font-family="system-ui, -apple-system, sans-serif"
            font-size="6"
            font-weight="600"
            text-anchor="middle"
            fill="currentColor">AI</text
          >
        </svg>
      </button>
    {/if}

    <!-- Remaining nav items (Search, Favorites) -->
    {#each navItems.slice(1) as item}
      <button
        class="nav-item"
        onclick={() => handleNavItemClick(item.id)}
        aria-label={item.label}
        disabled={item.type === 'placeholder'}
        title={isCollapsed ? item.label : undefined}
      >
        <svg
          class="nav-icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
        >
          <path d={item.icon}></path>
        </svg>
        {#if !isCollapsed}
          <span class="nav-label">{item.label}</span>
        {/if}
      </button>
    {/each}
  </div>

  <!-- Collection sub-panel -->
  <div bind:this={subPanelElement}>
    <CollectionSubPanel
      open={subPanelOpen}
      collectionId={collectionForPanel?.id ?? ''}
      collectionName={collectionForPanel?.content ?? collectionForPanel?.name ?? ''}
      members={collectionMembers}
      onClose={handleCloseSubPanel}
      onNodeClick={handleNodeClick}
      onOpenCollection={() => {
        if (collectionForPanel) handleNodeClick(collectionForPanel.id, 'collection');
      }}
      onChanged={() => {
        if (collectionForPanel) collectionsState.selectCollection(collectionForPanel.id);
      }}
    />
  </div>

</nav>

<style>
  .sidebar {
    background: hsl(var(--sidebar-background));
    border-right: 1px solid hsl(var(--border));
    display: flex;
    flex-direction: column;
    height: 100vh;
    position: relative;
  }

  /* Sidebar transition animation configured in app.css */

  /* Collapsed sidebar - adjusted width to center hamburger menu */
  .sidebar-collapsed {
    width: 52px; /* Width to center hamburger: 8px left + 16px padding + 20px icon + 8px right = 52px */
    align-items: stretch; /* Don't center content - let nav items control their own alignment */
    padding: 1rem 0; /* Remove left padding - center everything */
  }

  /* Expanded sidebar - exact specifications from patterns.html */
  .sidebar-expanded {
    width: 240px; /* Good balance between functionality and size */
    padding: 1rem;
  }

  /* Hamburger button - base styles moved to positioning section */

  .hamburger-button:hover {
    color: hsl(var(--foreground));
  }

  /* Removed :focus-visible border - Tab key used for indent/outdent, not UI navigation */

  .hamburger-icon {
    width: 20px;
    height: 20px;
    flex-shrink: 0;
  }

  /* Hamburger button - keep at fixed position, adjust sidebar width instead */
  .hamburger-button {
    background: none;
    border: none;
    cursor: pointer;
    padding: 0.5rem;
    transition: color 0.2s;
    color: hsl(var(--foreground));
    position: absolute;
    left: 0.5rem; /* Fixed 8px from left edge */
    top: 1rem; /* Fixed top position */
    z-index: 10; /* Ensure it stays above nav items */
  }

  /* Navigation items container */
  .nav-items {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 0; /* Remove gaps between nav items */
    margin-top: 3rem; /* Space for absolutely positioned hamburger button */
  }

  /* Navigation items - Full-width navigation items with no gaps */
  .nav-item {
    background: none;
    border: none;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: 0.75rem;
    height: 40px; /* Fixed height for consistent alignment */
    box-sizing: border-box;
    transition:
      background-color 0.2s,
      color 0.2s;
    color: hsl(var(--muted-foreground));
    position: relative;
    text-align: left;
    font-weight: 500;
  }

  .nav-item:hover:not(:disabled) {
    background: hsl(var(--border)); /* Use border color for more visible hover effect */
    color: hsl(var(--foreground));
  }

  /* Removed :focus-visible border - Tab key used for indent/outdent, not UI navigation */

  .nav-item:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  /* Collapsed sidebar specific nav items - align icons at 16px from left edge */
  .sidebar-collapsed .nav-item {
    display: flex;
    align-items: center;
    justify-content: flex-start; /* Align left instead of center */
    margin: 0;
    width: 100%; /* Full width for background highlights */
    padding: 0.5rem 0; /* Vertical padding only - NO horizontal padding */
    padding-left: 1rem; /* This should put icon at 16px from edge */
    position: relative;
    box-sizing: border-box;
  }

  /* Collapsed sidebar - no special active styling */

  /* Expanded sidebar nav items */
  .sidebar-expanded .nav-item {
    /* Full-width navigation items with no gaps */
    margin: 0 -1rem;
    padding: 0.5rem 1rem;
    border-radius: 0;
  }

  /* No special active styling - nav items are just for navigation */

  /* Navigation icon */
  .nav-icon {
    width: 20px;
    height: 20px;
    flex-shrink: 0;
  }

  /* Navigation label */
  .nav-label {
    font-size: 0.875rem;
    white-space: nowrap;
    transition: opacity 0.2s ease-out;
  }

  /* Collections trigger styling - full-width hover to match nav items */
  .sidebar-expanded :global([data-collapsible-trigger]) {
    margin: 0 -1rem;
    padding: 0.5rem 1rem;
    width: calc(100% + 2rem); /* Full width including negative margins */
    display: flex;
    align-items: center;
    gap: 0.75rem;
    background: none;
    border: none;
    cursor: pointer;
    color: hsl(var(--muted-foreground));
    font-weight: 500;
    font-size: 0.875rem;
    text-align: left;
    height: 40px;
    box-sizing: border-box;
    transition:
      background-color 0.2s,
      color 0.2s;
  }

  .sidebar-expanded :global([data-collapsible-trigger]:hover) {
    background: hsl(var(--border));
    color: hsl(var(--foreground));
  }

  /* Chevron icon removed - Collections is now a simple accordion toggle */

  /* Collection list (expanded content) */
  .collection-list {
    display: flex;
    flex-direction: column;
    padding: 0; /* No gap between Collections header and sub-items */
    margin: 0 -1rem; /* Break out of sidebar padding */
    overflow-x: auto; /* Allow horizontal scroll for long names */
    overflow-y: auto; /* Allow vertical scroll when list is tall */
    max-height: calc(10 * 28px); /* ~10 items tall before scrolling (28px per item) */
    scrollbar-width: thin; /* Firefox - show thin scrollbar */
    background: hsl(var(--active-nav-background)); /* Subtle background to group collection items */
  }

  .collection-list::-webkit-scrollbar {
    display: none; /* Chrome/Safari/Opera */
  }

  /* Collection item - container for expand button and name */
  .collection-item {
    display: flex;
    align-items: center;
    gap: 0;
    min-width: 100%; /* Allow growing beyond container width */
    width: max-content; /* Size to content for horizontal scrolling */
    padding: 0 1rem 0 2.25rem; /* Reduced by 1.25rem (20px) since expand-area now provides that space */
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    transition:
      background-color 0.2s,
      color 0.2s;
  }

  .collection-item:hover {
    background: hsl(var(--border));
    color: hsl(var(--foreground));
  }

  /* Nested level indentation */
  .collection-item.level-2 {
    padding-left: 3rem; /* Reduced accordingly */
  }

  .collection-item.level-3 {
    padding-left: 3.75rem; /* Reduced accordingly */
  }

  /* Expand area - reserves consistent space for chevron whether item has children or not */
  .expand-area {
    width: 20px;
    height: 28px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  /* Expand/collapse button */
  .expand-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 28px;
    background: none;
    border: none;
    cursor: pointer;
    color: inherit;
    padding: 0;
    flex-shrink: 0;
  }

  .expand-btn:hover {
    color: hsl(var(--foreground));
  }

  /* Collection name button - takes remaining space */
  .collection-name-btn {
    flex: 1;
    background: none;
    border: none;
    cursor: pointer;
    color: inherit;
    text-align: left;
    padding: 0.4rem 0;
    font-size: inherit;
    white-space: nowrap; /* Keep on single line, scroll horizontally if needed */
  }

  /* Renders as a placeholder collection entry: same .collection-item row and
     .collection-name-btn layout as a real collection, so it sits indented under
     the "Collections" header rather than reading as a sibling of it. */
  .new-collection-btn {
    opacity: 0.75;
  }

  .collection-item:hover .new-collection-btn {
    opacity: 1;
  }

  /* Borderless so the row matches a real collection entry instead of drawing a
     box around itself. The UA focus ring is deliberately left in place — there
     is no border for it to conflict with, and it is the only focus affordance
     the input has. */
  .new-collection-input {
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    color: inherit;
    padding: 0.4rem 0;
    font-size: inherit;
  }

  .new-collection-input::placeholder {
    color: hsl(var(--muted-foreground));
    opacity: 0.75;
  }

  /* Rollback message for a failed create, shown under the reopened input */
  .new-collection-error {
    color: hsl(var(--destructive));
    padding-bottom: 0.4rem;
    white-space: normal;
  }

  .new-collection-error:hover {
    background: none;
    color: hsl(var(--destructive));
  }

  /* Optimistically-inserted collection awaiting backend confirmation */
  .collection-item.pending .collection-name-btn {
    opacity: 0.6;
  }

  /* Expand chevron inside collection item */
  .expand-chevron {
    width: 12px;
    height: 12px;
    flex-shrink: 0;
    transition: transform 0.15s ease-out;
  }

  .expand-chevron.rotate-90 {
    transform: rotate(90deg);
  }

  /* Schema type list (expanded content) - mirrors collection-list */
  .schema-type-list {
    display: flex;
    flex-direction: column;
    padding: 0;
    margin: 0 -1rem;
    background: hsl(var(--active-nav-background));
  }

  /* Schema type item - mirrors collection-item. Left padding (3.5rem) matches
     collection-item's effective text offset: its 2.25rem padding plus the 20px
     (1.25rem) expand-area chevron spacer that precedes the name — schema-type-item
     has no expand-area/chevron of its own, so the padding folds that space in
     directly to keep the two sections visually aligned. */
  .schema-type-item {
    display: flex;
    align-items: center;
    padding: 0.4rem 1rem 0.4rem 3.5rem;
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    color: hsl(var(--muted-foreground));
    font-size: 0.8125rem;
    width: 100%;
    transition:
      background-color 0.2s,
      color 0.2s;
  }

  .schema-type-item:hover {
    background: hsl(var(--border));
    color: hsl(var(--foreground));
  }

  .schema-type-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .schema-type-separator {
    height: 1px;
    background: hsl(var(--border));
    margin: 0.25rem 1rem;
  }

  .schema-type-empty {
    display: block;
    padding: 0.4rem 1rem 0.4rem 3.5rem;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    font-style: italic;
  }

  /* AI Chats list (expanded content) - mirrors schema-type-list; chats are a
     flat list (no expand-area/nesting), same as schema types. */
  .ai-chat-list {
    display: flex;
    flex-direction: column;
    padding: 0;
    margin: 0 -1rem;
    overflow-y: auto;
    max-height: calc(10 * 28px); /* ~10 items tall before scrolling, matches collection-list */
    scrollbar-width: thin;
    background: hsl(var(--active-nav-background));
  }

  .ai-chat-list::-webkit-scrollbar {
    display: none;
  }

  /* AI chat row - mirrors schema-type-item */
  .ai-chat-item {
    display: flex;
    align-items: center;
    padding: 0.4rem 1rem 0.4rem 3.5rem;
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    color: hsl(var(--muted-foreground));
    font-size: 0.8125rem;
    width: 100%;
    transition:
      background-color 0.2s,
      color 0.2s;
  }

  .ai-chat-item:hover:not(:disabled) {
    background: hsl(var(--border));
    color: hsl(var(--foreground));
  }

  .ai-chat-item:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .ai-chat-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* "+ New chat" row - mirrors new-collection-btn */
  .ai-chat-new-btn {
    opacity: 0.75;
  }

  .ai-chat-new-btn:hover:not(:disabled) {
    opacity: 1;
  }

  /* Failed-create message - mirrors new-collection-error */
  .ai-chat-error {
    color: hsl(var(--destructive));
    padding-bottom: 0.4rem;
    white-space: normal;
    cursor: default;
  }

  .ai-chat-error:hover {
    background: none;
    color: hsl(var(--destructive));
  }
</style>
