/**
 * coreModal - Svelte action marking an element as one of core's open modals.
 *
 * Put it on the dialog element of a hand-rolled core modal, next to
 * `focusTrap`. While the element is mounted `shell.modalOpen` (host API) is
 * true; destroying the element releases it, so the signal follows the
 * `{#if}` that shows the modal and cannot stick on. Not exported to
 * extensions: their own modals must not count as core's.
 */
import { registerCoreModal } from '$lib/stores/core-modal-presence.svelte';

export function coreModal(_node: HTMLElement) {
  const release = registerCoreModal();
  return { destroy: release };
}
