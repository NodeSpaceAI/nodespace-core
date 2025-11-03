<script lang="ts">
  import { toggleTheme } from '$lib/design/theme.js';

  // Global keyboard shortcuts (kept at route level)
  function handleKeydown(event: KeyboardEvent) {
    // Toggle theme - Cmd+\ (Mac) or Ctrl+\ (Windows/Linux) per design system
    if ((event.metaKey || event.ctrlKey) && event.key === '\\') {
      event.preventDefault();
      toggleTheme();
    }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

<!--
  Content rendering moved to PaneManager > PaneContent
  This route now only handles global keyboard shortcuts
  Each PaneContent instance derives its activeTab from its own pane.id
  instead of the global $tabState.activePaneId
-->
