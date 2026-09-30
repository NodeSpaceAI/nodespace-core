/**
 * Reactive wrapper over {@link UiExtensionRegistry}
 * ========================================================
 *
 * The registry (`ui-extensions.ts`) holds declarative, non-reactive data and
 * never evaluates a contribution's `when()`. This module layers reactivity on
 * top (ADR-049): it returns only the contributions whose `when()` currently
 * holds. Reactivity comes from whatever the predicates read, so hosts call
 * these accessors inside a `$derived` or a template and re-run when that state
 * changes.
 *
 * A throwing `when()` counts as false and is logged once per contribution key
 * (ADR-082 §2.4); it is logged again only after it has returned normally in
 * between.
 */

// Transitional: the built-in extension registers itself as a side effect of this
// import. Removed once builds inject extensions through
// `virtual:nodespace-extensions` (ADR-082 §2.1).
import './pro-plugin';

import {
  uiExtensionRegistry,
  type ChromeSlot,
  type ChromeContribution,
  type Contribution,
  type Keyed,
  type SettingsSectionContribution,
  type SettingsSlot,
  type SettingsSlotContributionFor,
  type ViewerTabContribution
} from './ui-extensions';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('UiExtensions');

/** Keys whose `when()` threw and has not returned normally since. */
const warnedKeys = new Set<string>();

/** Whether a contribution should be shown now: no `when` means always. */
export function isContributionActive(c: Pick<Keyed<Contribution>, 'key' | 'when'>): boolean {
  if (!c.when) return true;
  try {
    const active = Boolean(c.when());
    warnedKeys.delete(c.key);
    return active;
  } catch (error) {
    if (!warnedKeys.has(c.key)) {
      warnedKeys.add(c.key);
      log.warn('Contribution when() threw; treating it as false', { key: c.key, error });
    }
    return false;
  }
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
