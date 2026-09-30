/**
 * Reactive wrapper over {@link UiExtensionRegistry}
 * ========================================================
 *
 * The registry (`ui-extensions.ts`) holds declarative, non-reactive data. This
 * module layers reactivity on top (ADR-049): it returns only the contributions
 * matching the active Pro-sync variant. The variant state machine lives in
 * `pro-sync-variant.svelte.ts`; its reads are reactive, so these functions
 * re-run when called inside a `$derived`/template.
 *
 * Importing this module also registers the built-in Pro UI extension (side-effect
 * import of `./pro-plugin`), so any consumer of the wrapper sees the contributions
 * without a separate init call.
 */

import './pro-plugin';

import {
  uiExtensionRegistry,
  type ChromeSlot,
  type ChromeContribution,
  type ViewerExtension
} from './ui-extensions';
import { resolveProSyncVariant } from './pro-sync-variant.svelte';

/** Chrome contributions for `slot` that match the currently-resolved variant. */
export function getActiveChromeContributions(slot: ChromeSlot): ChromeContribution[] {
  const variant = resolveProSyncVariant();
  return uiExtensionRegistry.chromeFor(slot).filter((c) => c.variant === variant);
}

/** Viewer extensions for `nodeType` that match the currently-resolved variant. */
export function getActiveViewerExtensions(nodeType: string): ViewerExtension[] {
  const variant = resolveProSyncVariant();
  return uiExtensionRegistry.viewersFor(nodeType).filter((e) => e.variant === variant);
}
