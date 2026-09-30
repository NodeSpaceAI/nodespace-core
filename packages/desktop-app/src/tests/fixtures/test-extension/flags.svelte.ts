/**
 * Reactive switches for the test extension, read by its `when()` predicates.
 * Flipping one inside a test re-runs whatever host is rendering the registry.
 */
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
  databaseRowThrowing: false
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
  for (const name of Object.keys(testExtensionMounts)) delete testExtensionMounts[name];
}
