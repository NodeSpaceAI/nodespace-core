/**
 * Reactive switches for the test extension, read by its `when()` predicates.
 * Flipping one inside a test re-runs whatever host is rendering the registry.
 */
import type { TreeItemActionProps } from '@nodespace/extension-api';

export const testExtensionFlags = $state({
  overlay: false,
  modal: false,
  /** Shows the second modal-slot contribution (higher priority than `modal`). */
  modalSecondary: false,
  tab: false,
  /** Shows the second `collection` tab, ordered after `tab`. */
  tabSecondary: false,
  /** Makes the `throwing-when` contribution's `when()` throw. */
  throwingWhen: false,
  /** Shows the contribution whose component throws while rendering. */
  throwingComponent: false,
  /** Shows the settings section. */
  section: false,
  /** Shows the Databases-header action. */
  databaseActions: false,
  /** Shows the per-database row content. */
  databaseRow: false,
  /** Shows a row contribution whose component throws while rendering. */
  databaseRowThrowing: false,
  /** Shows the `collaboration.entry` contribution. */
  collaborationEntry: false,
  /** Shows the second `collaboration.entry` contribution (higher priority than the first). */
  collaborationEntrySecondary: false,
  /** Makes a `collaboration.entry` contribution's `when()` throw. */
  collaborationEntryThrowingWhen: false,
  /** Shows a `collaboration.entry` contribution whose `load()` rejects. */
  collaborationEntryFailingLoad: false,
  /** Shows a `collaboration.entry` contribution whose component throws while rendering. */
  collaborationEntryThrowing: false,
  /** The collection ids the fixture's `collectionTreeRoots` returns. */
  collectionTreeRoots: [] as string[],
  /** Shows the tree-item action on every item not in `treeActionHiddenFor`. */
  treeAction: false,
  /** Node ids the tree-item action's `when(item)` hides it on. */
  treeActionHiddenFor: [] as string[],
  /** Shows the second tree-item action (higher priority than the first). */
  treeActionSecondary: false,
  /** Node ids a tree-item action's `when(item)` throws for. */
  treeActionThrowingFor: [] as string[],
  /** Shows a tree-item action whose `load()` rejects. */
  treeActionFailingLoad: false,
  /** Shows a tree-item action whose component throws while rendering. */
  treeActionThrowing: false,
  /**
   * The item the tree-item action last opened, which shows the fixture's
   * `app-shell-modal` contribution; `null` hides it.
   */
  openedTreeItem: null as TreeItemActionProps | null
});

/** How many times each fixture component has mounted, by component name. */
export const testExtensionMounts: Record<string, number> = {};

export function countMount(component: string): void {
  testExtensionMounts[component] = (testExtensionMounts[component] ?? 0) + 1;
}

/** Turn every flag off and zero the mount counters. Call in `afterEach`. */
export function resetTestExtension(): void {
  testExtensionFlags.overlay = false;
  testExtensionFlags.modal = false;
  testExtensionFlags.modalSecondary = false;
  testExtensionFlags.tab = false;
  testExtensionFlags.tabSecondary = false;
  testExtensionFlags.throwingWhen = false;
  testExtensionFlags.throwingComponent = false;
  testExtensionFlags.section = false;
  testExtensionFlags.databaseActions = false;
  testExtensionFlags.databaseRow = false;
  testExtensionFlags.databaseRowThrowing = false;
  testExtensionFlags.collaborationEntry = false;
  testExtensionFlags.collaborationEntrySecondary = false;
  testExtensionFlags.collaborationEntryThrowingWhen = false;
  testExtensionFlags.collaborationEntryFailingLoad = false;
  testExtensionFlags.collaborationEntryThrowing = false;
  testExtensionFlags.collectionTreeRoots = [];
  testExtensionFlags.treeAction = false;
  testExtensionFlags.treeActionHiddenFor = [];
  testExtensionFlags.treeActionSecondary = false;
  testExtensionFlags.treeActionThrowingFor = [];
  testExtensionFlags.treeActionFailingLoad = false;
  testExtensionFlags.treeActionThrowing = false;
  testExtensionFlags.openedTreeItem = null;
  for (const name of Object.keys(testExtensionMounts)) delete testExtensionMounts[name];
}
