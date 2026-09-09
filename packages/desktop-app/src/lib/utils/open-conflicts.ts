import { navigationStore, setActiveTab, addTab } from '$lib/stores/navigation.svelte';

/**
 * Open (or focus) the Conflicts tab — the single `type: 'conflicts'` singleton
 * tab shared by the sidebar entry and the inline per-node conflict indicator,
 * so they never spawn duplicate tabs.
 */
export function openConflicts(): void {
  const state = navigationStore.state;
  const existing = state.tabs.find((t) => t.type === 'conflicts');
  if (existing) {
    setActiveTab(existing.id, existing.paneId);
  } else {
    addTab({
      id: 'conflicts',
      title: 'Conflicts',
      type: 'conflicts',
      closeable: true,
      paneId: state.activePaneId
    });
  }
}
