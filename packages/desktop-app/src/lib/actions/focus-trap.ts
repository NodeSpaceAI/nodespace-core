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

  function handleFocusIn(event: FocusEvent) {
    const target = event.target;
    if (!(target instanceof Element) || node.contains(target)) return;
    // Another modal on top (an extension dialog, say) owns focus; contesting it
    // would leave the two scopes pulling focus back and forth.
    const otherModal = target.closest('[aria-modal="true"]');
    if (otherModal && !node.contains(otherModal)) return;
    (focusable()[0] ?? node).focus();
  }

  node.addEventListener('keydown', handleKeydown);
  document.addEventListener('focusin', handleFocusIn);

  // Move focus into the dialog on open so keyboard users start inside it and the
  // Escape/Tab handling above actually receives their key presses.
  (focusable()[0] ?? node).focus();

  return {
    update(next: FocusTrapParams = {}) {
      onEscape = next.onEscape;
    },
    destroy() {
      node.removeEventListener('keydown', handleKeydown);
      document.removeEventListener('focusin', handleFocusIn);
      // Return focus to whatever had it before the dialog opened.
      previouslyFocused?.focus?.();
    }
  };
}
