/**
 * OnboardingWizard methodology step.
 *
 * The wizard's first non-binary step: the others are act-or-skip, this one
 * asks the user to choose among playbooks. These tests cover:
 *   - the step appears only when the backend offers something to choose
 *   - nothing is preselected, and the primary action stays disabled until a
 *     choice is made (picking a methodology for someone is worse than asking)
 *   - installing passes the chosen id through and reports what landed
 *   - a re-keyed id is disclosed rather than silently resolved
 *   - a partial install reads as an error even though the promise resolved
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

const READY_STATUS = {
  completed: false,
  pathConfigured: false,
  skillConfigured: false,
  pathAlreadyConfigured: true
};

const NO_AGENTS = { agents: [], detectionFailed: false };
const NAMED_IDENTITY = { firstName: 'A', lastName: 'B', email: 'a@b.c' };

const LINEAR = {
  id: 'linear',
  name: 'Linear-style',
  description: 'Issues, Cycles, and the automation around them.'
};

/** Advance the wizard to the methodology step. */
async function openAtMethodologyStep(container: HTMLElement) {
  await tick();
  await tick();
  await tick();
  // Identity is skipped (the person already has a name), the skill step is
  // absent (no agents), so PATH is first and already configured — one Next
  // lands on methodology.
  await fireEvent.click(buttonByText(container, 'Next'));
  await tick();
}

function mountMocks(overrides: Record<string, unknown> = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd in overrides) return Promise.resolve(overrides[cmd]);
    if (cmd === 'check_onboarding_status') return Promise.resolve(READY_STATUS);
    if (cmd === 'detect_agents') return Promise.resolve(NO_AGENTS);
    if (cmd === 'get_local_identity') return Promise.resolve(NAMED_IDENTITY);
    if (cmd === 'get_identity_prefill') return Promise.resolve({ name: null, email: null });
    if (cmd === 'list_methodologies') return Promise.resolve([LINEAR]);
    return Promise.resolve();
  });
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('OnboardingWizard methodology step', () => {
  it('offers the step when the backend has playbooks to choose from', async () => {
    mountMocks();
    const { container } = render(OnboardingWizard, { props: { open: true, onClose: vi.fn() } });
    await openAtMethodologyStep(container as HTMLElement);

    expect(container.textContent).toContain('How do you track work?');
    expect(container.textContent).toContain('Linear-style');
  });

  // A build shipping no playbooks must not show an empty question. Unlike the
  // skill step, there is no generic wording that would make sense here.
  it('drops the step entirely when nothing is on offer', async () => {
    mountMocks({ list_methodologies: [] });
    const { container } = render(OnboardingWizard, { props: { open: true, onClose: vi.fn() } });
    await openAtMethodologyStep(container as HTMLElement);

    expect(container.textContent).not.toContain('How do you track work?');
  });

  it('preselects nothing and keeps Install disabled until a choice is made', async () => {
    mountMocks();
    const { container } = render(OnboardingWizard, { props: { open: true, onClose: vi.fn() } });
    await openAtMethodologyStep(container as HTMLElement);

    const radios = container.querySelectorAll<HTMLInputElement>('input[type="radio"]');
    expect(radios.length).toBe(1);
    expect(Array.from(radios).some((r) => r.checked)).toBe(false);

    // Asserted via the attribute rather than the `disabled` property so the
    // test needs no DOM lib globals.
    expect(buttonByText(container as HTMLElement, 'Install').hasAttribute('disabled')).toBe(true);

    await fireEvent.change(radios[0]);
    await tick();
    expect(buttonByText(container as HTMLElement, 'Install').hasAttribute('disabled')).toBe(false);
  });

  it('installs the chosen playbook and reports what landed', async () => {
    mountMocks({
      install_methodology: {
        playbookId: 'linear',
        success: true,
        steps: [{ label: 'Create `issue` schema', outcome: { kind: 'created', id: 'issue' } }]
      }
    });
    const { container } = render(OnboardingWizard, { props: { open: true, onClose: vi.fn() } });
    await openAtMethodologyStep(container as HTMLElement);

    await fireEvent.change(container.querySelector<HTMLInputElement>('input[type="radio"]')!);
    await tick();
    await fireEvent.click(buttonByText(container as HTMLElement, 'Install'));
    await tick();
    await tick();

    expect(mockInvoke).toHaveBeenCalledWith('install_methodology', { methodologyId: 'linear' });
    expect(container.textContent).toContain('Installed 1 items.');
  });

  // The acceptance criterion: a collision is auto-resolved AND disclosed.
  // A user whose own `cycle` was left alone has to be told the new one is
  // called something else.
  it('discloses a re-keyed id rather than resolving it silently', async () => {
    mountMocks({
      install_methodology: {
        playbookId: 'linear',
        success: true,
        steps: [
          {
            label: 'Create `cycle` schema',
            outcome: { kind: 'suffixed', requested: 'cycle', created: 'cycle__2' }
          }
        ]
      }
    });
    const { container } = render(OnboardingWizard, { props: { open: true, onClose: vi.fn() } });
    await openAtMethodologyStep(container as HTMLElement);

    await fireEvent.change(container.querySelector<HTMLInputElement>('input[type="radio"]')!);
    await tick();
    await fireEvent.click(buttonByText(container as HTMLElement, 'Install'));
    await tick();
    await tick();

    expect(container.textContent).toContain('cycle → cycle__2');
    expect(container.textContent).toContain('left unchanged');
  });

  // A partial install resolves rather than throwing, so the step has to read
  // `success` to decide how it reads. Treating a resolved promise as done
  // would report a half-finished install as a clean one.
  it('reads a partial install as an error even though the call resolved', async () => {
    mountMocks({
      install_methodology: {
        playbookId: 'linear',
        success: false,
        steps: [
          { label: 'Create `issue` schema', outcome: { kind: 'created', id: 'issue' } },
          { label: 'Install Play: x', outcome: { kind: 'failed', message: 'boom' } }
        ]
      }
    });
    const { container } = render(OnboardingWizard, { props: { open: true, onClose: vi.fn() } });
    await openAtMethodologyStep(container as HTMLElement);

    await fireEvent.change(container.querySelector<HTMLInputElement>('input[type="radio"]')!);
    await tick();
    await fireEvent.click(buttonByText(container as HTMLElement, 'Install'));
    await tick();
    await tick();

    expect(container.querySelector('.error-banner')).not.toBeNull();
    expect(container.textContent).toContain('boom');
    expect(container.querySelector('.success-banner')).toBeNull();
  });

  it('Skip advances without installing anything', async () => {
    mountMocks();
    const { container } = render(OnboardingWizard, { props: { open: true, onClose: vi.fn() } });
    await openAtMethodologyStep(container as HTMLElement);

    await fireEvent.click(buttonByText(container as HTMLElement, 'Skip'));
    await tick();

    expect(mockInvoke).not.toHaveBeenCalledWith('install_methodology', expect.anything());
    expect(container.textContent).not.toContain('How do you track work?');
  });
});
