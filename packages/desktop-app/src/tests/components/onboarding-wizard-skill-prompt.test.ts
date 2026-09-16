/**
 * The skill step's QUESTION -- the title and body shown before the user
 * clicks "Add Skill" -- must name the agents the install is actually about
 * to target.
 *
 * The bug this covers: the title read "Add NodeSpace to Claude Code?" and the
 * body named only Claude Code, as static string literals, no matter what was
 * detected. A user with Claude Code AND Antigravity CLI was asked about one
 * agent and then, seconds later, correctly told the skill went into two --
 * the question undersold its own outcome. Both now derive from the same
 * detected-agents list (and the same formatter) as the success banner, so
 * they cannot drift apart again.
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

/** Render the wizard with `agents` detected, and advance to the skill step. */
async function renderAtSkillStep(agents: string[], detectionFailed = false) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'check_onboarding_status') {
      return Promise.resolve({
        completed: false,
        pathConfigured: false,
        skillConfigured: false,
        pathAlreadyConfigured: true // path step auto-advances
      });
    }
    if (cmd === 'detect_agents') {
      return Promise.resolve({ agents, detectionFailed });
    }
    return Promise.resolve();
  });

  const { container } = render(OnboardingWizard, {
    props: { open: true, onClose: vi.fn() }
  });
  await tick();
  await tick(); // status and detect_agents resolve independently
  await fireEvent.click(buttonByText(container, 'Next')); // -> skill step
  await tick();
  return container;
}

describe('OnboardingWizard skill-step prompt', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it('names every detected agent in the title, not just Claude Code', async () => {
    const container = await renderAtSkillStep(['claude-code', 'antigravity']);

    const title = container.querySelector('h2');
    expect(title?.textContent).toBe('Add NodeSpace to Claude Code and Antigravity CLI?');
  });

  it('names every detected agent in the body too, consistently with the title', async () => {
    const container = await renderAtSkillStep(['claude-code', 'antigravity']);

    const body = container.querySelector('.onboarding-header p');
    expect(body?.textContent).toContain('Claude Code and Antigravity CLI');
  });

  it('uses a serial comma when three agents are detected', async () => {
    const container = await renderAtSkillStep(['claude-code', 'antigravity', 'codex']);

    const title = container.querySelector('h2');
    expect(title?.textContent).toBe(
      'Add NodeSpace to Claude Code, Antigravity CLI, and Codex?'
    );
  });

  /**
   * The single-agent case is the one that was already correct, so it must
   * keep reading exactly as it did -- including the singular verb.
   */
  it('reads naturally, in the singular, when only Claude Code is detected', async () => {
    const container = await renderAtSkillStep(['claude-code']);

    expect(container.querySelector('h2')?.textContent).toBe('Add NodeSpace to Claude Code?');
    expect(container.querySelector('.onboarding-header p')?.textContent).toContain(
      'Claude Code knows how to interact'
    );
  });

  it('uses the plural verb for multiple detected agents', async () => {
    const container = await renderAtSkillStep(['claude-code', 'codex']);

    expect(container.querySelector('.onboarding-header p')?.textContent).toContain(
      'Claude Code and Codex know how to interact'
    );
  });

  /**
   * The Claude Code plugin marketplace caveat has no analogue in any other
   * harness, so naming it when Claude Code isn't even a target would send the
   * user looking for a `/plugin` command their agent does not have.
   */
  it('shows the plugin-marketplace caveat only when Claude Code is a target', async () => {
    const withClaude = await renderAtSkillStep(['claude-code', 'codex']);
    expect(withClaude.textContent).toContain('/plugin install nodespace@');

    const withoutClaude = await renderAtSkillStep(['codex', 'opencode']);
    expect(withoutClaude.textContent).not.toContain('/plugin install nodespace@');
  });

  /**
   * Detection can fail outright on the Rust side (the installer won't
   * resolve, no runtime on $PATH). That is NOT the same as "this machine has
   * no coding agents", and must not silently drop the step: the user may
   * well have an agent installed, and letting them try surfaces a real
   * installer error instead of nothing at all.
   */
  it('still offers the step, with generic wording, when detection failed', async () => {
    const container = await renderAtSkillStep([], true);

    expect(container.querySelector('h2')?.textContent).toBe(
      'Add NodeSpace to your coding agents?'
    );
    // Generic, but never wrong -- which a guess at one specific agent, the
    // bug this whole change removes, would not be.
    expect(container.querySelector('.onboarding-header p')?.textContent).toContain(
      'your coding agents know how to interact'
    );
  });

  /**
   * The genuinely-nothing-detected case is different from the failure above:
   * there is nothing to install into, so the step is skipped entirely rather
   * than asking a question with no possible answer.
   */
  it('skips the step entirely when detection ran and found no agents', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'check_onboarding_status') {
        return Promise.resolve({
          completed: false,
          pathConfigured: false,
          skillConfigured: false,
          pathAlreadyConfigured: true
        });
      }
      if (cmd === 'detect_agents') {
        return Promise.resolve({ agents: [], detectionFailed: false });
      }
      return Promise.resolve();
    });

    const { container } = render(OnboardingWizard, {
      props: { open: true, onClose: vi.fn() }
    });
    await tick();
    await tick();
    await fireEvent.click(buttonByText(container, 'Next'));
    await tick();

    expect(container.querySelector('h2')?.textContent).not.toContain('Add NodeSpace to');
  });
});
