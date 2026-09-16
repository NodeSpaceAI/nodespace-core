/**
 * OnboardingWizard focus-trap tests
 *
 * The first-launch wizard was migrated from a hand-rolled overlay (Escape on the
 * backdrop, no focus management) to the shared `focusTrap` action. Because it is
 * multi-step — advancing a step or a configure step succeeding swaps the primary
 * action button — these tests drive REAL keyboard focus to confirm focus stays
 * inside the dialog across transitions and Escape dismisses from within.
 *
 * Queries are scoped to the render `container` (and focus is read via the
 * dialog's `ownerDocument`) rather than the global `screen`/`document.body`, so
 * the focus assertions are robust to global-document pollution from other test
 * files in the full-suite run.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { render, fireEvent } from '@testing-library/svelte';

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

import OnboardingWizard from '$lib/components/onboarding/onboarding-wizard.svelte';

const STATUS = {
  completed: false,
  pathConfigured: false,
  skillConfigured: false,
  pathAlreadyConfigured: false
};

function mockBackend(overrides: Partial<typeof STATUS> = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'check_onboarding_status') return Promise.resolve({ ...STATUS, ...overrides });
    return Promise.resolve();
  });
}

/** The dialog element + its owning document, scoped to this render. */
function dialogOf(container: HTMLElement) {
  const dialog = container.querySelector<HTMLElement>('[role="dialog"]');
  if (!dialog) throw new Error('dialog not rendered');
  return { dialog, doc: dialog.ownerDocument };
}

function buttonByText(root: HTMLElement, text: string): HTMLElement {
  const btn = Array.from(root.querySelectorAll<HTMLElement>('button')).find(
    (b) => b.textContent?.trim() === text
  );
  if (!btn) throw new Error(`button "${text}" not found`);
  return btn;
}

describe('OnboardingWizard (focus-trap, #1414)', () => {
  let onClose: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    onClose = vi.fn();
    mockInvoke.mockReset();
    mockBackend();
  });

  it('does not render when closed', () => {
    const { container } = render(OnboardingWizard, { props: { open: false, onClose } });
    expect(container.querySelector('[role="dialog"]')).toBeNull();
  });

  it('moves focus into the dialog on open and dismisses on Escape from inside', async () => {
    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    const { dialog, doc } = dialogOf(container);
    await tick(); // let focusTrap + the focus effect settle

    // Real keyboard focus lands inside the dialog (not a synthetic overlay event).
    expect(dialog.contains(doc.activeElement)).toBe(true);

    await fireEvent.keyDown(doc.activeElement!, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('anchors focus to the step primary action on open', async () => {
    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    const { dialog, doc } = dialogOf(container);
    await tick();

    expect(doc.activeElement).toBe(buttonByText(dialog, 'Add to PATH'));
  });

  it('re-anchors focus inside the dialog after advancing a step', async () => {
    // PATH already configured → the path step shows "Next" immediately.
    mockBackend({ pathAlreadyConfigured: true });
    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    const { dialog, doc } = dialogOf(container);
    await tick(); // path step ("Next") rendered + focused

    await fireEvent.click(buttonByText(dialog, 'Next')); // advance to the summary step
    await tick();

    // Focus did not fall to <body>; it followed into the new step, so the trap's
    // Escape handling still works.
    expect(dialog.contains(doc.activeElement)).toBe(true);
    await fireEvent.keyDown(doc.activeElement!, { key: 'Escape' });
    expect(onClose).toHaveBeenCalled();
  });

  it('closes on backdrop click but not on dialog-content click', async () => {
    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    const { dialog } = dialogOf(container);

    await fireEvent.click(dialog); // inside the dialog — must not close
    expect(onClose).not.toHaveBeenCalled();

    await fireEvent.click(dialog.parentElement!); // the backdrop
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
