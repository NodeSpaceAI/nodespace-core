/**
 * focusTrap - Svelte action for accessible modal dialogs
 *
 * The app's hand-rolled modals (overlay + content + `stopPropagation`) never
 * moved focus into the dialog on open and never trapped Tab, so under real
 * keyboard focus an Escape press could be swallowed before reaching the close
 * handler and Tab could wander out to background content. This action fixes all
 * three concerns in one place, without changing a modal's markup or styling:
 *
 *  - on mount it remembers the previously-focused element and moves focus to the
 *    first focusable element inside the dialog (or the container itself);
 *  - it keeps Tab / Shift+Tab cycling within the dialog;
 *  - it invokes `onEscape` when Escape is pressed (caught on the container, so
 *    `stopPropagation` on inner content can't hide it);
 *  - it takes focus back when something outside the dialog grabs it (an
 *    editor's autofocus, say), unless that element sits inside another modal
 *    (`aria-modal="true"`), which legitimately holds focus while it is on top;
 *    Content portalled outside the dialog that is not `aria-modal` (popovers,
 *    selects) would be pulled back too; callers that need that must mark it
 *    `aria-modal="true"` or not use the trap;
 *  - on destroy (the modal closing) it restores focus to where it was.
 *
 * Apply to the dialog content element, which must be focusable (e.g.
 * `tabindex="0"`) so focus has somewhere to land when there are no focusable
 * children:
 *   <div role="dialog" tabindex="0" use:focusTrap={{ onEscape: close }}>…</div>
 */

const FOCUSABLE =
  'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';

export interface FocusTrapParams {
  /** Called when Escape is pressed inside the trap (typically the close handler). */
  onEscape?: () => void;
}

/** Open traps, oldest first; only the last one reclaims focus. */
const activeTraps: HTMLElement[] = [];

export function focusTrap(node: HTMLElement, params: FocusTrapParams = {}) {
  let onEscape = params.onEscape;
  const previouslyFocused = document.activeElement as HTMLElement | null;

  function focusable(): HTMLElement[] {
    return Array.from(node.querySelectorAll<HTMLElement>(FOCUSABLE));
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.stopPropagation();
      onEscape?.();
      return;
    }
    if (event.key !== 'Tab') return;

    const items = focusable();
    if (items.length === 0) {
      // Nothing tabbable inside — keep focus pinned to the dialog itself.
      event.preventDefault();
      node.focus();
      return;
    }

    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement;

    if (event.shiftKey) {
      if (active === first || !node.contains(active)) {
        event.preventDefault();
        last.focus();
      }
    } else if (active === last || !node.contains(active)) {
      event.preventDefault();
      first.focus();
    }
  }

  // The element that last held focus inside the dialog, so a reclaim puts
  // focus back where the user was rather than on the first control (which, in
  // a dialog, is often a close or destructive button).
  let lastFocusedInside: HTMLElement | null = null;
  let reclaiming = false;

  function handleFocusIn(event: FocusEvent) {
    const target = event.target;
    if (!(target instanceof Element)) return;
    if (node.contains(target)) {
      if (target instanceof HTMLElement && target !== node) lastFocusedInside = target;
      return;
    }
    // Focus falling to the page itself is not something to fight over.
    if (target === document.body || target === document.documentElement) return;
    // Only the topmost trap reclaims, so two traps never pull focus back and forth.
    if (activeTraps[activeTraps.length - 1] !== node || reclaiming) return;
    // Another modal on top (an extension dialog, say) owns focus; contesting it
    // would leave the two scopes pulling focus back and forth.
    const otherModal = target.closest('[aria-modal="true"]');
    if (otherModal && !node.contains(otherModal)) return;

    reclaiming = true;
    try {
      if (lastFocusedInside && node.contains(lastFocusedInside) && lastFocusedInside.isConnected) {
        lastFocusedInside.focus();
      }
      // Nothing to return to, or it refused focus (disabled or hidden since).
      if (!lastFocusedInside || document.activeElement !== lastFocusedInside) {
        if (!node.hasAttribute('tabindex')) node.setAttribute('tabindex', '-1');
        node.focus();
      }
    } finally {
      reclaiming = false;
    }
  }

  node.addEventListener('keydown', handleKeydown);

  // Move focus into the dialog on open so keyboard users start inside it and the
  // Escape/Tab handling above actually receives their key presses.
  (focusable()[0] ?? node).focus();

  // Registered after the initial focus move, so that move is not remembered as
  // "where the user was".
  activeTraps.push(node);
  document.addEventListener('focusin', handleFocusIn);

  return {
    update(next: FocusTrapParams = {}) {
      onEscape = next.onEscape;
    },
    destroy() {
      node.removeEventListener('keydown', handleKeydown);
      document.removeEventListener('focusin', handleFocusIn);
      const index = activeTraps.indexOf(node);
      if (index !== -1) activeTraps.splice(index, 1);
      // Return focus to whatever had it before the dialog opened.
      previouslyFocused?.focus?.();
    }
  };
}
