/**
 * Reactive wrapper over {@link UiExtensionRegistry}
 * ========================================================
 *
 * The registry (`ui-extensions.ts`) holds declarative, non-reactive data and
 * never evaluates a contribution's `when()`. This module layers reactivity on
 * top (ADR-049): it returns only the contributions whose `when()` currently
 * holds, and for a replaceable slot also whether anything is registered for it,
 * which does not depend on `when()`. Reactivity comes from whatever the
 * predicates read, so hosts call these accessors inside a `$derived` or a
 * template and re-run when that state changes.
 *
 * A throwing `when()` counts as false and is logged once per contribution key
 * (ADR-082 §3.4); it is logged again only after it has returned normally in
 * between. A tree-item action's `when(item)` is asked once per item, so it
 * "returns normally" once it has done so for every item it threw for: a
 * predicate that fails for some items and not others is logged once, not on
 * every re-evaluation of the tree, and neither is one that fails for them all.
 */

import {
  uiExtensionRegistry,
  type ChromeSlot,
  type ChromeContribution,
  type Contribution,
  type Keyed,
  type ReplaceableSlot,
  type ReplaceableSlotContribution,
  type SettingsSectionContribution,
  type SettingsSlot,
  type SettingsSlotContributionFor,
  type TreeItemActionContribution,
  type TreeItemActionProps,
  type ViewerTabContribution
} from './ui-extensions';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('UiExtensions');

/**
 * By contribution key, the items its `when()` threw for and has not returned
 * normally for since; `''` stands for a contribution asked about no item. A key
 * is present exactly while its set is non-empty, so its warning is armed again
 * once the key is gone.
 */
const failing = new Map<string, Set<string>>();

/**
 * Runs the `when()` of the contribution keyed `key`, asked about the tree item
 * `nodeId` when there is one; a throw counts as false.
 */
function holds(key: string, when: () => unknown, nodeId?: string): boolean {
  const item = nodeId ?? '';
  try {
    const active = Boolean(when());
    const items = failing.get(key);
    if (items?.delete(item) && items.size === 0) failing.delete(key);
    return active;
  } catch (error) {
    const items = failing.get(key);
    if (items) {
      items.add(item);
    } else {
      failing.set(key, new Set([item]));
      log.warn('Contribution when() threw; treating it as false', {
        key,
        ...(nodeId !== undefined && { nodeId }),
        error
      });
    }
    return false;
  }
}

/** Whether a contribution should be shown now: no `when` means always. */
export function isContributionActive(c: Pick<Keyed<Contribution>, 'key' | 'when'>): boolean {
  if (!c.when) return true;
  return holds(c.key, () => c.when?.());
}

/** Chrome contributions for `slot` whose `when()` currently holds. */
export function getActiveChromeContributions(slot: ChromeSlot): Keyed<ChromeContribution>[] {
  return uiExtensionRegistry.chromeFor(slot).filter(isContributionActive);
}

/** Viewer tabs for `nodeType` whose `when()` currently holds. */
export function getActiveViewerTabs(nodeType: string): Keyed<ViewerTabContribution>[] {
  return uiExtensionRegistry.viewerTabsFor(nodeType).filter(isContributionActive);
}

/** Settings sections whose `when()` currently holds, in registration order. */
export function getActiveSettingsSections(): Keyed<SettingsSectionContribution>[] {
  return uiExtensionRegistry.settingsSections().filter(isContributionActive);
}

/** Contributions to the Databases-page `slot` whose `when()` currently holds. */
export function getActiveSettingsSlot<S extends SettingsSlot>(
  slot: S
): Keyed<SettingsSlotContributionFor<S>>[] {
  return uiExtensionRegistry.settingsSlotFor(slot).filter(isContributionActive);
}

/** Tree-item actions whose `when(item)` currently holds for `item`, in priority order. */
export function getActiveTreeItemActions(
  item: TreeItemActionProps
): Keyed<TreeItemActionContribution>[] {
  return uiExtensionRegistry
    .treeItemActions()
    .filter((c) => !c.when || holds(c.key, () => c.when?.(item), item.nodeId));
}

/** What a replaceable-slot host renders; see {@link getReplaceableSlot}. */
export interface ReplaceableSlotState {
  /**
   * Whether any contribution is registered for the slot, visible or not. While
   * one is, the host never renders core's default.
   */
  registered: boolean;
  /**
   * The contribution to render: the one with the highest priority, ties in
   * registration order, among those whose `when()` currently holds. `null` when
   * none does.
   */
  active: Keyed<ReplaceableSlotContribution> | null;
}

/**
 * The state of the replaceable `slot`. Call it inside a `$derived` or a
 * template: `active` re-evaluates when the state the `when()` predicates read
 * changes. `registered` reflects registration, which is static, so it never
 * depends on a predicate.
 */
export function getReplaceableSlot(slot: ReplaceableSlot): ReplaceableSlotState {
  const registered = uiExtensionRegistry.replaceableSlotFor(slot);
  return {
    registered: registered.length > 0,
    active: registered.find(isContributionActive) ?? null
  };
}
