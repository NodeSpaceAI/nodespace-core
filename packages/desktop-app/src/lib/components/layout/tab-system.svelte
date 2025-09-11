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
    {#snippet children({ activeTab })}
      <!-- Content based on activeTab -->
    {/snippet}
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

  // Handle close button click with event stopping
  function handleCloseClick(event: MouseEvent, tabId: string): void {
    event.stopPropagation();
    closeTab(tabId);
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
        
        {#if tab.closeable}
          <Button
            variant="ghost"
            size="icon"
            class="tab-close-button"
            aria-label={`Close ${tab.title} tab`}
            on:click={(event) => handleCloseClick(event, tab.id)}
          >
            <svg
              width="12"
              height="12"
              viewBox="0 0 12 12"
              fill="none"
              xmlns="http://www.w3.org/2000/svg"
              aria-hidden="true"
            >
              <path
                d="M9 3L3 9M3 3L9 9"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            </svg>
          </Button>
        {/if}
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
    border-bottom: 1px solid hsl(var(--border));
    padding: 0;
    overflow-x: auto;
    scrollbar-width: none;
    -ms-overflow-style: none;
  }

  .tab-bar::-webkit-scrollbar {
    display: none;
  }

  .tab-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    min-width: 0;
    flex-shrink: 0;
    cursor: pointer;
    border-right: 1px solid hsl(var(--border));
    background-color: transparent;
    color: hsl(var(--muted-foreground));
    transition: all 0.2s ease-in-out;
    font-weight: 500;
    font-size: 14px;
    outline: none;
    position: relative;
  }

  .tab-item:hover {
    background-color: hsl(var(--accent));
    color: hsl(var(--accent-foreground));
  }

  .tab-item:focus-visible {
    background-color: hsl(var(--accent));
    color: hsl(var(--accent-foreground));
    box-shadow: inset 0 0 0 2px hsl(var(--ring));
  }

  .tab-item--active {
    background-color: hsl(var(--background));
    color: hsl(var(--foreground));
    border-bottom: 1px solid hsl(var(--background));
    margin-bottom: -1px;
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

  /* Custom styling for close button */
  :global(.tab-close-button) {
    width: 20px !important;
    height: 20px !important;
    padding: 0 !important;
    border-radius: var(--radius) !important;
    opacity: 0.7;
    transition: opacity 0.2s ease-in-out;
    flex-shrink: 0;
  }

  :global(.tab-close-button:hover) {
    opacity: 1;
    background-color: hsl(var(--destructive) / 0.1) !important;
    color: hsl(var(--destructive)) !important;
  }

  .tab-item--active :global(.tab-close-button) {
    opacity: 0.8;
  }

  .tab-item--active :global(.tab-close-button:hover) {
    opacity: 1;
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