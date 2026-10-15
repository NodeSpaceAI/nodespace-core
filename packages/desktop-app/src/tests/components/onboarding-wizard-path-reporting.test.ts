/**
 * OnboardingWizard PATH step reporting
 *
 * The PATH step reports what `configure_path` actually did: which file was
 * created or changed, and that already-open terminals need a restart. It also
 * says so when `nodespace` already resolves on the PATH.
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

function mockBackend(
  status: Partial<typeof STATUS>,
  configurePath: () => Promise<unknown> = () => Promise.resolve(undefined)
) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'check_onboarding_status') return Promise.resolve({ ...STATUS, ...status });
    if (cmd === 'detect_agents') return Promise.resolve({ agents: [], detectionFailed: false });
    if (cmd === 'configure_path') return configurePath();
    return Promise.resolve();
  });
}

function buttonByText(root: HTMLElement, text: string): HTMLElement {
  const btn = Array.from(root.querySelectorAll<HTMLElement>('button')).find(
    (b) => b.textContent?.trim() === text
  );
  if (!btn) throw new Error(`button "${text}" not found`);
  return btn;
}

describe('OnboardingWizard PATH step reporting', () => {
  const onClose = vi.fn();

  beforeEach(() => {
    mockInvoke.mockReset();
    onClose.mockReset();
  });

  it('names the file it created and says open terminals need a restart', async () => {
    mockBackend({}, () =>
      Promise.resolve({
        alreadyOnPath: false,
        files: [{ path: '/Users/me/.zshrc', outcome: 'created' }],
        restartRequired: true
      })
    );
    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();

    await fireEvent.click(buttonByText(container, 'Add to PATH'));
    await tick();

    const banner = container.querySelector('.success-banner');
    expect(banner?.textContent).toContain('Created');
    expect(banner?.textContent).toContain('/Users/me/.zshrc');
    expect(banner?.textContent).toContain('Restart open terminals');
  });

  it('says a line was added to an existing file', async () => {
    mockBackend({}, () =>
      Promise.resolve({
        alreadyOnPath: false,
        files: [{ path: '/Users/me/.bash_profile', outcome: 'written' }],
        restartRequired: true
      })
    );
    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();

    await fireEvent.click(buttonByText(container, 'Add to PATH'));
    await tick();

    expect(container.querySelector('.success-banner')?.textContent).toContain('Added to');
  });

  it('shows the failure and keeps Skip available when nothing could be written', async () => {
    mockBackend({}, () => Promise.reject(new Error('Failed to write /Users/me/.zshrc: denied')));
    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();

    await fireEvent.click(buttonByText(container, 'Add to PATH'));
    await tick();

    expect(container.querySelector('.error-banner')?.textContent).toContain('denied');
    expect(buttonByText(container, 'Skip')).toBeTruthy();
  });

  it('offers nothing to change when nodespace already resolves', async () => {
    mockBackend({ pathAlreadyConfigured: true });
    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();

    expect(container.querySelector('.info-banner')?.textContent).toContain('already runs');
    expect(() => buttonByText(container, 'Add to PATH')).toThrow();
  });
});
