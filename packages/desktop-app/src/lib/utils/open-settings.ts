import { navigationStore, setActiveTab, addTab } from '$lib/stores/navigation.svelte';
import { settingsStore } from '$lib/stores/settings.svelte';

/**
 * Open (or focus) the Settings tab — the single `type: 'settings'` singleton tab
 * shared by the sidebar entry and the File menu, so they never spawn duplicate
 * settings tabs. Pass a category id (e.g. `'database'`) to
 * focus that section on open.
 */
export function openSettings(category?: string): void {
  if (category) {
    settingsStore.initialCategory = category;
  }
  const state = navigationStore.state;
  const existing = state.tabs.find((t) => t.type === 'settings');
  if (existing) {
    setActiveTab(existing.id, existing.paneId);
  } else {
    addTab({
      id: 'settings',
      title: 'Settings',
      type: 'settings',
      closeable: true,
      paneId: state.activePaneId
    });
  }
}
