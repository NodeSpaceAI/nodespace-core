<!-- 
  TabSystem Component
  
  A browser-like tab system that manages multiple content tabs according to the NodeSpace design system.
  
  Features:
  - Single tab: No tab bar shown, just content
  - Multiple tabs: Show tab bar with truncated titles (~25 characters)
  - Active tab: White background, full opacity text
  - Inactive tabs: Muted background, reduced opacity
  - Tab closing: Close button for closeable tabs
  - Tab switching: Click to activate tabs
  - Keyboard navigation: Arrow keys, Enter/Space
  - Accessibility: ARIA labels, proper focus management
  
  Integration:
  - Uses navigation store (tabState, setActiveTab, closeTab)
  - Slot pattern for content with activeTab prop
  - Design system compliant colors and styling
  
  Usage:
  <TabSystem>
    Snippet: children({ activeTab })
      Content based on activeTab
  </TabSystem>
-->
<script lang="ts">
  import { tabState, setActiveTab, closeTab, type Tab } from '$lib/stores/navigation.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import { cn } from '$lib/utils.js';

  // Truncate title to specified length with ellipsis
  function truncateTitle(title: string, maxLength: number = 25): string {
    return title.length > maxLength ? title.substring(0, maxLength - 1) + '…' : title;
  }

  // Handle tab click to switch active tab
  function handleTabClick(tabId: string): void {
    setActiveTab(tabId);
  }


  // Handle keyboard navigation for accessibility
  function handleTabKeydown(event: KeyboardEvent, tabId: string): void {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      setActiveTab(tabId);
    } else if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') {
      event.preventDefault();
      const tabs = $tabState.tabs;
      const currentIndex = tabs.findIndex(tab => tab.id === tabId);
      
      let nextIndex: number;
      if (event.key === 'ArrowRight') {
        nextIndex = currentIndex === tabs.length - 1 ? 0 : currentIndex + 1;
      } else {
        nextIndex = currentIndex === 0 ? tabs.length - 1 : currentIndex - 1;
      }
      
      setActiveTab(tabs[nextIndex].id);
    }
  }

  // Get active tab for slot prop
  $: activeTab = $tabState.tabs.find(tab => tab.id === $tabState.activeTabId);
</script>

<!-- Tab bar - only shown when there are multiple tabs -->
{#if $tabState.tabs.length > 1}
  <div class="tab-bar" role="tablist" aria-label="Content tabs">
    {#each $tabState.tabs as tab (tab.id)}
      <div
        class={cn(
          'tab-item',
          tab.id === $tabState.activeTabId && 'tab-item--active'
        )}
        role="tab"
        tabindex={tab.id === $tabState.activeTabId ? 0 : -1}
        aria-selected={tab.id === $tabState.activeTabId}
        aria-controls={`tab-panel-${tab.id}`}
        on:click={() => handleTabClick(tab.id)}
        on:keydown={(event) => handleTabKeydown(event, tab.id)}
      >
        <span class="tab-title" title={tab.title}>
          {truncateTitle(tab.title)}
        </span>
      </div>
    {/each}
  </div>
{/if}

<!-- Tab content area -->
<div 
  class="tab-content"
  role="tabpanel"
  id={`tab-panel-${$tabState.activeTabId}`}
  aria-labelledby={`tab-${$tabState.activeTabId}`}
>
  <slot {activeTab} />
</div>

<style>
  .tab-bar {
    display: flex;
    align-items: center;
    background-color: hsl(var(--muted));
    padding: 0;
    margin: 0;
    overflow-x: auto;
    scrollbar-width: none;
    -ms-overflow-style: none;
    position: relative;
    /* Ensure tabs start from the very edge */
    margin-left: 0;
    padding-left: 0;
  }

  /* Add border to the empty space after the last tab */
  .tab-bar::after {
    content: '';
    flex: 1;
    border-bottom: 1px solid hsl(var(--border));
    height: 1px;
    align-self: flex-end;
  }

  .tab-bar::-webkit-scrollbar {
    display: none;
  }

  .tab-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 16px;
    height: 40px;
    min-width: 0;
    flex-shrink: 0;
    cursor: pointer;
    background-color: transparent;
    color: hsl(var(--muted-foreground));
    font-weight: 500;
    font-size: 14px;
    outline: none;
    position: relative;
    box-sizing: border-box;
    line-height: 1.2;
    /* Use box-shadow for borders to avoid mitered corners */
    box-shadow: 
      inset 1px 0 0 0 hsl(var(--border)), /* left border */
      inset 0 -1px 0 0 hsl(var(--border)); /* bottom border */
    top: 0;
  }

  /* Add right border to the last tab */
  .tab-item:last-child {
    box-shadow: 
      inset 1px 0 0 0 hsl(var(--border)),  /* left border */
      inset 0 -1px 0 0 hsl(var(--border)), /* bottom border */
      inset -1px 0 0 0 hsl(var(--border)); /* right border */
  }
  
  /* Remove bottom border from last tab when it's active */
  .tab-item:last-child.tab-item--active {
    box-shadow: 
      inset 1px 0 0 0 hsl(var(--border)),  /* left border */
      inset -1px 0 0 0 hsl(var(--border)); /* right border only */
  }

  /* Hide left border from tab immediately after active tab to avoid double border */
  .tab-item--active + .tab-item {
    box-shadow: 
      inset 0 -1px 0 0 hsl(var(--border)); /* bottom border only */
  }

  .tab-item:hover {
    background-color: hsl(var(--border));
    color: hsl(var(--foreground));
  }

  .tab-item:focus-visible {
    background-color: hsl(var(--hover-background));
    color: hsl(var(--hover-foreground));
    box-shadow: 
      inset 1px 0 0 0 hsl(var(--border)),  /* left border */
      inset 0 -1px 0 0 hsl(var(--border)), /* bottom border */
      inset 0 0 0 2px hsl(var(--ring));    /* focus ring */
  }

  .tab-item--active {
    background-color: hsl(var(--background));
    color: hsl(var(--foreground));
    position: relative;
    box-sizing: border-box;
    line-height: 1.2;
    /* Identical properties to inactive tabs */
    padding: 0 16px;
    height: 40px;
    display: flex;
    align-items: center;
    gap: 8px;
    /* Ensure exact same positioning as inactive tabs */
    top: 0;
    /* Remove margin-bottom to prevent text shifting */
    margin-bottom: 0;
    /* Active tab borders only (accent will be separate) */
    box-shadow: 
      inset 1px 0 0 0 hsl(var(--border)),  /* left border */
      inset -1px 0 0 0 hsl(var(--border)); /* right border */
  }
  
  /* Active tab accent - positioned at the very top edge */
  .tab-item--active::before {
    content: '';
    position: absolute;
    top: 0;     /* At the top edge of the tab */
    left: 0;    /* Start at tab edge (no extending beyond) */
    right: 0;   /* End at tab edge (no extending beyond) */
    height: 4px; /* Standard accent height */
    background: hsl(var(--primary));
    z-index: 1;  /* Lower z-index so it doesn't interfere with text */
    pointer-events: none;
  }
  
  /* Special handling for first tab accent to reach browser edge */
  .tab-item:first-child.tab-item--active::before {
    left: -100px;   /* Extend far left to ensure it reaches the edge */
  }

  /* Cover the content border below active tab */
  .tab-item--active::after {
    content: '';
    position: absolute;
    bottom: 0;    /* Position at the bottom edge of the tab */
    left: 0;
    right: 0;
    height: 1px;
    background: hsl(var(--background));
    z-index: 1;
  }

  .tab-item--active:hover {
    background-color: hsl(var(--background));
    color: hsl(var(--foreground));
  }

  .tab-title {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    min-width: 0;
    flex: 1;
  }

  .tab-content {
    flex: 1;
    min-height: 0;
    background-color: hsl(var(--background));
  }


  /* Responsive behavior */
  @media (max-width: 768px) {
    .tab-item {
      padding: 8px 12px;
      font-size: 13px;
    }
    
    .tab-title {
      max-width: 120px;
    }
  }
</style>