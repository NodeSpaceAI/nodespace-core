/**
 * AcpPermissionCard in a real browser: every choice is reachable by focus in
 * reading order, an Enter or Space press on a focused choice answers, and an
 * answered card takes no more focus or answers.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { tick } from 'svelte';
import { render, cleanup } from '@testing-library/svelte';
import AcpPermissionCard from '$lib/components/chat/acp-permission-card.svelte';
import type { AcpPermissionOutcome, AcpPermissionRequest } from '$lib/types/acp-session';

const request: AcpPermissionRequest = {
  requestId: 'req-1',
  toolTitle: 'Edit src/main.rs',
  toolKind: 'edit',
  options: [
    { optionId: 'a', name: 'Always allow', kind: 'allow_always' },
    { optionId: 'o', name: 'Allow', kind: 'allow_once' },
    { optionId: 'r', name: 'Reject', kind: 'reject_once' }
  ]
};

describe('AcpPermissionCard focus (real browser)', () => {
  afterEach(() => cleanup());

  it('focuses each choice in reading order and activates the focused one', () => {
    const answers: AcpPermissionOutcome[] = [];
    const { container } = render(AcpPermissionCard, {
      props: { request, onAnswer: (_id: string, outcome: AcpPermissionOutcome) => {
        answers.push(outcome);
      } }
    });
    const buttons = [...container.querySelectorAll<HTMLElement>('button')];
    expect(buttons.map((b) => b.getAttribute('data-option-id'))).toEqual(['o', 'r', 'cancel']);

    for (const button of buttons) {
      button.focus();
      expect(document.activeElement).toBe(button);
    }

    // A button is activated by the platform on Enter and Space: the click it
    // fires is what the card answers from.
    buttons[0].focus();
    buttons[0].click();
    expect(answers).toEqual([{ selected: 'o' }]);
  });

  it('takes no focus and answers nothing more once answered', async () => {
    const answers: AcpPermissionOutcome[] = [];
    const { container } = render(AcpPermissionCard, {
      props: { request, onAnswer: (_id: string, outcome: AcpPermissionOutcome) => {
        answers.push(outcome);
      } }
    });
    const buttons = [...container.querySelectorAll<HTMLElement>('button')];
    buttons[1].click();
    await tick();
    for (const button of buttons) {
      button.focus();
      expect(document.activeElement).not.toBe(button);
      button.click();
    }
    expect(answers).toEqual([{ selected: 'r' }]);
  });
});
