/**
 * After `configure_skill` resolves, the wizard must name which agents
 * actually got the skill, not just say "Claude Code skill" regardless of
 * what happened. The bug this covers had a correct multi-agent install
 * (Claude Code AND Antigravity CLI) reading as if Antigravity had never been
 * touched at all, because nothing in the UI ever showed the real per-agent
 * result.
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

describe('OnboardingWizard skill-install reporting', () => {
  let onClose: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    onClose = vi.fn();
    mockInvoke.mockReset();
  });

  it('names every agent actually installed into, not just Claude Code', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_onboarding_status') {
        return Promise.resolve({
          completed: false,
          pathConfigured: false,
          skillConfigured: false,
          detectedAgents: ['claude-code'],
          pathAlreadyConfigured: true // path step auto-advances
        });
      }
      if (cmd === 'configure_skill') {
        return Promise.resolve({
          success: true,
          agentsInstalled: ['claude-code', 'antigravity'],
          agentsSkipped: [],
          cliOnPath: true,
          cliWarning: null,
          error: null,
          failureIsNew: false
        });
      }
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick(); // path step already configured -> shows "Next"

    await fireEvent.click(buttonByText(container, 'Next')); // -> skill step
    await tick();

    await fireEvent.click(buttonByText(container, 'Add Skill'));
    await tick();
    await tick(); // let the invoke() promise resolve and stepSuccess flip

    const banner = container.querySelector('.success-banner');
    expect(banner?.textContent).toContain('Claude Code');
    expect(banner?.textContent).toContain('Antigravity CLI');

    await fireEvent.click(buttonByText(container, 'Next')); // -> summary step
    await tick();

    const summary = container.querySelector('.summary-list');
    expect(summary?.textContent).toContain('Claude Code');
    expect(summary?.textContent).toContain('Antigravity CLI');
  });

  it('reports a detected-but-skipped agent with its reason, distinct from a successful install', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_onboarding_status') {
        return Promise.resolve({
          completed: false,
          pathConfigured: false,
          skillConfigured: false,
          detectedAgents: ['claude-code'],
          pathAlreadyConfigured: true
        });
      }
      if (cmd === 'configure_skill') {
        return Promise.resolve({
          success: true,
          agentsInstalled: ['claude-code'],
          agentsSkipped: [
            { agent: 'codex', reason: 'detected but no files to install (package may be incomplete)' }
          ],
          cliOnPath: true,
          cliWarning: null,
          error: null,
          failureIsNew: false
        });
      }
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();

    await fireEvent.click(buttonByText(container, 'Next'));
    await tick();
    await fireEvent.click(buttonByText(container, 'Add Skill'));
    await tick();
    await tick();

    expect(container.textContent).toContain('Codex');
    expect(container.textContent).toContain('detected but no files to install');
  });

  // The bug this covers: when Claude Code is the only detected agent and it
  // is skipped because a plugin-managed copy already exists,
  // `agentsInstalled` is empty -- the same shape as "nothing happened at
  // all". Without checking `agentsSkipped` too, the success banner fell back
  // to "Skill file written", which is false: this run wrote nothing.
  it('does not claim a skill file was written when the only agent was skipped as plugin-managed', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_onboarding_status') {
        return Promise.resolve({
          completed: false,
          pathConfigured: false,
          skillConfigured: false,
          detectedAgents: ['claude-code'],
          pathAlreadyConfigured: true
        });
      }
      if (cmd === 'configure_skill') {
        return Promise.resolve({
          success: true,
          agentsInstalled: [],
          agentsSkipped: [
            {
              agent: 'claude-code',
              reason: 'already installed via the Claude Code plugin marketplace, not overwriting'
            }
          ],
          cliOnPath: true,
          cliWarning: null,
          error: null,
          failureIsNew: false
        });
      }
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();

    await fireEvent.click(buttonByText(container, 'Next'));
    await tick();
    await fireEvent.click(buttonByText(container, 'Add Skill'));
    await tick();
    await tick();

    const banner = container.querySelector('.success-banner');
    expect(banner?.textContent).not.toContain('Skill file written');
    expect(container.textContent).toContain('plugin marketplace');
  });

  // `configure_skill`'s idempotency guard on the Rust side returns
  // `agentsSkipped: []` unconditionally once skill_installed is already
  // persisted true, even when the real reason nothing installed was a
  // plugin-managed skip -- so a revisited onboarding session can produce
  // exactly this empty-both-arrays shape. The banner text must not assert
  // anything it can't back up (like "a file was written") in that case.
  it('makes no specific claim when the result has neither an install nor a skip to point to', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_onboarding_status') {
        return Promise.resolve({
          completed: false,
          pathConfigured: false,
          skillConfigured: false,
          detectedAgents: ['claude-code'],
          pathAlreadyConfigured: true
        });
      }
      if (cmd === 'configure_skill') {
        return Promise.resolve({
          success: true,
          agentsInstalled: [],
          agentsSkipped: [],
          cliOnPath: true,
          cliWarning: null,
          error: null,
          failureIsNew: false
        });
      }
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, { props: { open: true, onClose } });
    await tick();

    await fireEvent.click(buttonByText(container, 'Next'));
    await tick();
    await fireEvent.click(buttonByText(container, 'Add Skill'));
    await tick();
    await tick();

    const banner = container.querySelector('.success-banner');
    expect(banner?.textContent).not.toContain('Skill file written');
    // Deliberately agent-neutral: this branch fires precisely when the result
    // names no agent at all, so naming one here would be a guess.
    expect(banner?.textContent?.trim()).toBe('Agent integration is set up.');
  });
});
