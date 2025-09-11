<script lang="ts">
  import { layoutState, navigationItems, toggleSidebar } from '$lib/stores/layout.js';
  import { toggleTheme } from '$lib/design/theme.js';

  // Subscribe to stores
  $: isCollapsed = $layoutState.sidebarCollapsed;
  $: navItems = $navigationItems;

  // Handle navigation item clicks
  function handleNavItemClick(itemId: string) {
    // For now, just update active state in navigation items
    navigationItems.update(items => 
      items.map(item => ({
        ...item,
        active: item.id === itemId
      }))
    );
  }

  // Keyboard shortcuts
  function handleKeydown(event: KeyboardEvent) {
    // Toggle theme with Cmd/Ctrl + Shift + T
    if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key === 'T') {
      event.preventDefault();
      toggleTheme();
    }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

<nav 
  class="sidebar navigation-sidebar"
  class:sidebar-collapsed={isCollapsed}
  class:sidebar-expanded={!isCollapsed}
  role="navigation"
  aria-label="Main navigation"
>
  <!-- Hamburger menu button -->
  <button
    class="hamburger-button"
    on:click={toggleSidebar}
    aria-label={isCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}
    aria-expanded={!isCollapsed}
  >
    <svg class="hamburger-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <line x1="3" y1="6" x2="21" y2="6"></line>
      <line x1="3" y1="12" x2="21" y2="12"></line>
      <line x1="3" y1="18" x2="21" y2="18"></line>
    </svg>
  </button>

  <!-- Navigation items -->
  <div class="nav-items">
    {#each navItems as item}
      <button
        class="nav-item"
        class:active={item.active}
        on:click={() => handleNavItemClick(item.id)}
        aria-label={item.label}
        disabled={item.type === 'placeholder'}
        title={isCollapsed ? item.label : undefined}
      >
        <svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d={item.icon}></path>
        </svg>
        {#if !isCollapsed}
          <span class="nav-label">{item.label}</span>
        {/if}
      </button>
    {/each}
  </div>

  <!-- Theme toggle (at bottom) -->
  <button
    class="theme-toggle"
    on:click={toggleTheme}
    aria-label="Toggle theme"
    title={isCollapsed ? "Toggle theme" : undefined}
  >
    <svg class="theme-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <circle cx="12" cy="12" r="5"/>
      <path d="M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42"/>
    </svg>
    {#if !isCollapsed}
      <span class="theme-label">Toggle Theme</span>
    {/if}
  </button>
</nav>

<style>
  .sidebar {
    background: hsl(var(--background));
    display: flex;
    flex-direction: column;
    height: 100vh;
    transition: width 0.3s ease;
    position: relative;
  }

  /* Collapsed sidebar - exact specifications from patterns.html */
  .sidebar-collapsed {
    width: 48px;
    align-items: center; /* Center all content within the 48px panel */
    padding: 1rem 0; /* Remove left padding - center everything */
  }

  /* Expanded sidebar - exact specifications from patterns.html */
  .sidebar-expanded {
    width: 240px; /* Good balance between functionality and size */
    padding: 1rem;
  }

  /* Hamburger button */
  .hamburger-button {
    background: none;
    border: none;
    cursor: pointer;
    padding: 0;
    transition: color 0.2s;
    color: hsl(var(--foreground));
    flex-shrink: 0;
  }

  .hamburger-button:hover {
    color: hsl(var(--primary));
  }

  .hamburger-button:focus-visible {
    outline: 2px solid hsl(var(--primary));
    outline-offset: 2px;
    border-radius: 4px;
  }

  .hamburger-icon {
    width: 20px;
    height: 20px;
    flex-shrink: 0;
  }

  /* Consistent hamburger positioning - perfectly centered in collapsed sidebar */
  .sidebar-collapsed .hamburger-icon {
    margin-bottom: 0.5rem;
    /* Centered automatically by container align-items: center */
    /* Ensure consistent alignment with nav items */
  }

  .sidebar-expanded .hamburger-icon {
    margin-bottom: 0.5rem;
    width: 20px;
    height: 20px;
    align-self: flex-start;
    margin-left: calc(1rem - 18px); /* Move left to align with nav icons at same position */
  }

  /* Navigation items container */
  .nav-items {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
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
    transition: background-color 0.2s, color 0.2s;
    color: hsl(var(--muted-foreground));
    position: relative;
    text-align: left;
    font-weight: 500;
  }

  .nav-item:hover:not(:disabled) {
    background: hsl(var(--muted));
    color: hsl(var(--foreground));
  }

  .nav-item:focus-visible {
    outline: 2px solid hsl(var(--primary));
    outline-offset: 2px;
    border-radius: 4px;
  }

  .nav-item:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  /* Collapsed sidebar specific nav items - perfect icon centering */
  .sidebar-collapsed .nav-item {
    display: flex;
    align-items: center;
    justify-content: center; /* Center the icons within the nav item */
    margin: 0;
    width: 100%; /* Full width for background highlights */
    padding: 0.5rem 0; /* Vertical padding only - NO horizontal padding */
    position: relative;
  }

  /* Collapsed sidebar active item - overlay accent without affecting icon position */
  .sidebar-collapsed .nav-item.active {
    /* DO NOT add padding-left - keep icon perfectly centered */
    border-left: none; /* Remove default border */
    padding-left: 0; /* Override general .nav-item.active padding-left */
    padding-right: 0; /* Ensure no horizontal padding */
  }

  .sidebar-collapsed .nav-item.active::before {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0; /* Accent at the very left edge of the sidebar */
    width: 4px;
    background: hsl(var(--primary));
    /* Accent overlays without pushing content */
  }

  /* Expanded sidebar nav items */
  .sidebar-expanded .nav-item {
    /* Full-width navigation items with no gaps */
    margin: 0 -1rem;
    padding: 0.5rem 1rem;
    border-radius: 0;
  }

  .sidebar-expanded .nav-item.active {
    background: hsl(var(--muted));
    color: hsl(var(--foreground));
    font-weight: 600;
    border-left: 4px solid hsl(var(--primary));
    padding-left: calc(1rem - 4px);
  }

  /* Navigation icon */
  .nav-icon {
    width: 16px;
    height: 16px;
    flex-shrink: 0;
  }

  /* Navigation label */
  .nav-label {
    font-size: 0.875rem;
    white-space: nowrap;
  }

  /* Theme toggle */
  .theme-toggle {
    background: none;
    border: none;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    transition: background-color 0.2s, color 0.2s;
    color: hsl(var(--muted-foreground));
    margin-top: auto;
    text-align: left;
    font-weight: 500;
  }

  .theme-toggle:hover {
    background: hsl(var(--muted));
    color: hsl(var(--foreground));
  }

  .theme-toggle:focus-visible {
    outline: 2px solid hsl(var(--primary));
    outline-offset: 2px;
    border-radius: 4px;
  }

  /* Collapsed state theme toggle - centered like nav items */
  .sidebar-collapsed .theme-toggle {
    justify-content: center;
    padding: 0.5rem 0;
    margin: 0;
  }

  /* Expanded state theme toggle */
  .sidebar-expanded .theme-toggle {
    margin: 0 -1rem;
    padding: 0.5rem 1rem;
  }

  .theme-icon {
    width: 16px;
    height: 16px;
    flex-shrink: 0;
  }

  .theme-label {
    font-size: 0.875rem;
    white-space: nowrap;
  }
</style>