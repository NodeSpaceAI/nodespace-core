/**
 * OnboardingWizard identity step.
 *
 * The setup wizard never asked who the local user is, leaving the seeded
 * local person node permanently blank. These tests cover:
 *   - the step is shown ONLY while the seeded person is blank, and asked
 *     first (before the PATH step)
 *   - a prefill from git/OS is shown for confirmation but never written
 *     until the user explicitly saves
 *   - Skip leaves the install untouched (no `set_local_identity` call)
 *   - the `identityOnly` backfill-nudge mode renders just the one step and
 *     dismisses itself (closing the dialog) on both Save and Skip
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

function buttonByText(root: HTMLElement, text: string): HTMLElement {
  const btn = Array.from(root.querySelectorAll<HTMLElement>('button')).find(
    (b) => b.textContent?.trim() === text
  );
  if (!btn) throw new Error(`button "${text}" not found`);
  return btn;
}

const BLANK_STATUS = {
  completed: false,
  pathConfigured: false,
  skillConfigured: false,
  detectedAgents: [],
  pathAlreadyConfigured: true // so the PATH step, once reached, shows "Next" immediately
};

const BLANK_IDENTITY = {
  nodeId: 'person-1',
  firstName: '',
  lastName: '',
  email: '',
  isBlank: true
};
const FILLED_IDENTITY = {
  nodeId: 'person-1',
  firstName: 'Alice',
  lastName: 'Example',
  email: 'alice@example.com',
  isBlank: false
};

describe('OnboardingWizard identity step', () => {
  let onClose: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    onClose = vi.fn();
    mockInvoke.mockReset();
  });

  it('asks for identity first when the seeded person is blank', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_onboarding_status') return Promise.resolve(BLANK_STATUS);
      if (cmd === 'get_local_identity') return Promise.resolve(BLANK_IDENTITY);
      if (cmd === 'get_identity_prefill') return Promise.resolve({ name: null, email: null });
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();
    await tick(); // let both onMount invoke() calls resolve

    expect(container.textContent).toContain('Who are you?');
    expect(container.querySelector<HTMLInputElement>('#identity-first-name')).not.toBeNull();
    expect(container.querySelector<HTMLInputElement>('#identity-last-name')).not.toBeNull();
    // 3 real steps for this scenario (identity, path, summary — no skill
    // step, detectedAgents is empty): the step SEQUENCE — not just the
    // step currently rendered — must actually include 'identity', or
    // navigation (Skip/Next) later in the flow has nowhere sane to land.
    expect(container.querySelectorAll('.step-dot').length).toBe(3);
  });

  it('does not show the identity step when the seeded person already has a name/email', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_onboarding_status') return Promise.resolve(BLANK_STATUS);
      if (cmd === 'get_local_identity') return Promise.resolve(FILLED_IDENTITY);
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();
    await tick();

    // Straight to the PATH step (already configured -> "Next" immediately),
    // never the identity prompt.
    expect(container.textContent).not.toContain('Who are you?');
    expect(buttonByText(container, 'Next')).toBeTruthy();
  });

  it('shows a git/OS prefill for confirmation but writes nothing until Save is clicked', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_onboarding_status') return Promise.resolve(BLANK_STATUS);
      if (cmd === 'get_local_identity') return Promise.resolve(BLANK_IDENTITY);
      if (cmd === 'get_identity_prefill') {
        return Promise.resolve({ name: 'Alice Example', email: 'alice@example.com' });
      }
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();
    await tick();
    await tick(); // get_local_identity then get_identity_prefill are two sequential awaits

    const firstNameInput = container.querySelector<HTMLInputElement>('#identity-first-name')!;
    const lastNameInput = container.querySelector<HTMLInputElement>('#identity-last-name')!;
    const emailInput = container.querySelector<HTMLInputElement>('#identity-email')!;
    expect(firstNameInput.value).toBe('Alice');
    expect(lastNameInput.value).toBe('Example');
    expect(emailInput.value).toBe('alice@example.com');

    // Prefill alone must never call the write command.
    expect(mockInvoke).not.toHaveBeenCalledWith('set_local_identity', expect.anything());
  });

  it('Skip advances past the identity step without saving anything', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_onboarding_status') return Promise.resolve(BLANK_STATUS);
      if (cmd === 'get_local_identity') return Promise.resolve(BLANK_IDENTITY);
      if (cmd === 'get_identity_prefill') return Promise.resolve({ name: null, email: null });
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();
    await tick();

    await fireEvent.click(buttonByText(container, 'Skip'));
    await tick();

    expect(mockInvoke).not.toHaveBeenCalledWith('set_local_identity', expect.anything());
    // Advanced to the next real step (PATH, already configured -> "Next").
    expect(container.textContent).not.toContain('Who are you?');
    expect(buttonByText(container, 'Next')).toBeTruthy();
  });

  it('Save writes the trimmed name/email and advances to the next step', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_onboarding_status') return Promise.resolve(BLANK_STATUS);
      if (cmd === 'get_local_identity') return Promise.resolve(BLANK_IDENTITY);
      if (cmd === 'get_identity_prefill') return Promise.resolve({ name: null, email: null });
      if (cmd === 'set_local_identity') return Promise.resolve(FILLED_IDENTITY);
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();
    await tick();

    const firstNameInput = container.querySelector<HTMLInputElement>('#identity-first-name')!;
    const lastNameInput = container.querySelector<HTMLInputElement>('#identity-last-name')!;
    const emailInput = container.querySelector<HTMLInputElement>('#identity-email')!;
    await fireEvent.input(firstNameInput, { target: { value: '  Alice  ' } });
    await fireEvent.input(lastNameInput, { target: { value: '  Example  ' } });
    await fireEvent.input(emailInput, { target: { value: '  alice@example.com  ' } });

    await fireEvent.click(buttonByText(container, 'Save'));
    await tick();
    await tick();

    expect(mockInvoke).toHaveBeenCalledWith('set_local_identity', {
      firstName: 'Alice',
      lastName: 'Example',
      email: 'alice@example.com'
    });
    expect(container.textContent).toContain("You're recorded as the owner");
  });

  it('identityOnly mode renders only the identity step and closes on Save', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_local_identity') return Promise.resolve(BLANK_IDENTITY);
      if (cmd === 'get_identity_prefill') return Promise.resolve({ name: null, email: null });
      if (cmd === 'set_local_identity') return Promise.resolve(FILLED_IDENTITY);
      if (cmd === 'dismiss_identity_backfill_prompt') return Promise.resolve();
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, {
      props: { open: true, onClose, identityOnly: true }
    });
    await tick();
    await tick();

    expect(container.textContent).toContain('Who are you?');
    // No path/skill/summary machinery reachable — check_onboarding_status
    // must never be called in this mode.
    expect(mockInvoke).not.toHaveBeenCalledWith('check_onboarding_status');

    await fireEvent.click(buttonByText(container, 'Save'));
    await tick();
    await tick();

    expect(mockInvoke).toHaveBeenCalledWith('set_local_identity', {
      firstName: '',
      lastName: '',
      email: ''
    });
    expect(mockInvoke).toHaveBeenCalledWith('dismiss_identity_backfill_prompt');
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('Skip during the main wizard sets identityPromptDismissed via complete_onboarding', async () => {
    // Regression test: a user who declines identity during the
    // main onboarding wizard must not be hit with the separate backfill
    // nudge on their very next launch for the exact thing they just said no
    // to. Skipping here has no direct dismiss command like identityOnly mode
    // does — instead the skip must be threaded through to `complete_onboarding`
    // so the backend can set `identity_prompt_dismissed` itself.
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_onboarding_status') return Promise.resolve(BLANK_STATUS);
      if (cmd === 'get_local_identity') return Promise.resolve(BLANK_IDENTITY);
      if (cmd === 'get_identity_prefill') return Promise.resolve({ name: null, email: null });
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();
    await tick();

    // identity -> path (already configured, "Next" shows immediately)
    await fireEvent.click(buttonByText(container, 'Skip'));
    await tick();

    await fireEvent.click(buttonByText(container, 'Next')); // path -> summary
    await tick();

    await fireEvent.click(buttonByText(container, 'Open NodeSpace')); // -> finishWizard
    await tick();
    await tick();

    expect(mockInvoke).toHaveBeenCalledWith('complete_onboarding', {
      pathConfigured: false,
      skillConfigured: false,
      identitySkipped: true
    });
  });

  it('Save during the main wizard leaves identitySkipped false on complete_onboarding', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_onboarding_status') return Promise.resolve(BLANK_STATUS);
      if (cmd === 'get_local_identity') return Promise.resolve(BLANK_IDENTITY);
      if (cmd === 'get_identity_prefill') return Promise.resolve({ name: null, email: null });
      if (cmd === 'set_local_identity') return Promise.resolve(FILLED_IDENTITY);
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();
    await tick();

    await fireEvent.click(buttonByText(container, 'Save'));
    await tick();
    await tick();

    await fireEvent.click(buttonByText(container, 'Next')); // identity -> path
    await tick();

    await fireEvent.click(buttonByText(container, 'Next')); // path -> summary
    await tick();

    await fireEvent.click(buttonByText(container, 'Open NodeSpace'));
    await tick();
    await tick();

    expect(mockInvoke).toHaveBeenCalledWith('complete_onboarding', {
      pathConfigured: false,
      skillConfigured: false,
      identitySkipped: false
    });
  });

  it('identityOnly mode dismisses and closes on Skip too', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_local_identity') return Promise.resolve(BLANK_IDENTITY);
      if (cmd === 'get_identity_prefill') return Promise.resolve({ name: null, email: null });
      if (cmd === 'dismiss_identity_backfill_prompt') return Promise.resolve();
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, {
      props: { open: true, onClose, identityOnly: true }
    });
    await tick();
    await tick();

    await fireEvent.click(buttonByText(container, 'Skip'));
    await tick();

    expect(mockInvoke).not.toHaveBeenCalledWith('set_local_identity', expect.anything());
    expect(mockInvoke).toHaveBeenCalledWith('dismiss_identity_backfill_prompt');
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
