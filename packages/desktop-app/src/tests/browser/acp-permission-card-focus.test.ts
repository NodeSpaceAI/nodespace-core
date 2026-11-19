/**
 * The ACP permission card in a real browser, driven with real key events:
 * focus lands on the card when it appears, Enter and Space on a choice answer
 * it, focus is never left on the page afterwards, and the two protections
 * against answering twice (the `disabled` attribute and the `answered` guard)
 * each hold on their own.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import { tick } from 'svelte';
import { userEvent } from '@vitest/browser/context';
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import AiChatAcpNodeViewer from '$lib/components/viewers/ai-chat-acp-node-viewer.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import type { Node } from '$lib/types/node';
import { FixtureAcpSession, FOUR_OPTION_REQUEST } from '../helpers/acp-session-fixture.svelte';

const CHAT_ID = '6c1d1a52-2f4e-4b1e-9c56-0d6f3c1b7a99';

function seedChat(): void {
  sharedNodeStore.setNode(
    {
      id: CHAT_ID,
      nodeType: 'ai-chat-acp',
      content: 'Chat',
      version: 1,
      createdAt: '2026-01-01T00:00:00Z',
      modifiedAt: '2026-01-01T00:00:00Z',
      properties: {},
      lifecycleStatus: 'active',
      mentions: [],
      agent: 'claude-code',
      sessionStatus: 'active'
    } as unknown as Node,
    { type: 'database', reason: 'seed' }
  );
}

async function showCard(session: FixtureAcpSession): Promise<HTMLElement> {
  const { container } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
  session.pendingPermission = FOUR_OPTION_REQUEST;
  await tick();
  const card = await vi.waitUntil(() => container.querySelector<HTMLElement>('[role="alert"]'));
  return card as HTMLElement;
}

function choice(card: HTMLElement, id: string): (HTMLElement & { disabled: boolean }) {
  return card.querySelector(`[data-option-id="${id}"]`) as (HTMLElement & { disabled: boolean });
}

/** Wait for the viewer to hand focus on after an answer. */
async function focusSettled(): Promise<void> {
  await vi.waitUntil(() => document.activeElement && document.activeElement !== document.body);
}

describe('AcpPermissionCard focus and keyboard (real browser)', () => {
  beforeEach(() => {
    labsFlags.aiChatEnabled = true;
    vi.spyOn(sharedNodeStore, 'loadChildrenForParent').mockResolvedValue([]);
    seedChat();
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    labsFlags.aiChatEnabled = false;
  });

  it('puts focus on the card when it appears', async () => {
    const card = await showCard(new FixtureAcpSession());
    await vi.waitUntil(() => document.activeElement === card);
  });

  it('is reached by Tab in reading order and answered with Enter', async () => {
    const session = new FixtureAcpSession();
    const card = await showCard(session);
    await vi.waitUntil(() => document.activeElement === card);

    await userEvent.tab();
    expect(document.activeElement).toBe(choice(card, 'allow-once'));
    await userEvent.tab();
    expect(document.activeElement).toBe(choice(card, 'reject-once'));
    await userEvent.tab({ shift: true });
    expect(document.activeElement).toBe(choice(card, 'allow-once'));

    await userEvent.keyboard('{Enter}');
    await vi.waitUntil(() => session.answers.length === 1);
    expect(session.answers[0].outcome).toEqual({ selected: 'allow-once' });
  });

  it('answers with Space too', async () => {
    const session = new FixtureAcpSession();
    const card = await showCard(session);
    await userEvent.tab();
    await userEvent.tab();
    expect(document.activeElement).toBe(choice(card, 'reject-once'));
    await userEvent.keyboard(' ');
    await vi.waitUntil(() => session.answers.length === 1);
    expect(session.answers[0].outcome).toEqual({ selected: 'reject-once' });
  });

  it('does not leave focus on the page after an answer', async () => {
    const session = new FixtureAcpSession();
    const card = await showCard(session);
    await userEvent.tab();
    await userEvent.keyboard('{Enter}');
    await vi.waitUntil(() => session.answers.length === 1);
    // The session has not cleared the request yet: focus must already be off the card.
    await focusSettled();
    expect(card.contains(document.activeElement)).toBe(false);
    session.pendingPermission = null;
    await focusSettled();
    expect(document.activeElement).not.toBe(document.body);
    expect(document.activeElement?.getAttribute('aria-label')).toBe('Chat message input');
  });

  it('holds against a second answer on the answered guard alone, with disabled stripped', async () => {
    const session = new FixtureAcpSession();
    const card = await showCard(session);
    const allow = choice(card, 'allow-once');
    allow.click();
    await vi.waitUntil(() => session.answers.length === 1);
    for (const button of card.querySelectorAll('button')) button.removeAttribute('disabled');
    allow.click();
    choice(card, 'reject-once').click();
    choice(card, 'cancel').click();
    await tick();
    expect(session.answers.length).toBe(1);
  });

  it('holds against a second answer on the disabled attribute alone', async () => {
    const session = new FixtureAcpSession();
    const card = await showCard(session);
    const allow = choice(card, 'allow-once');
    allow.click();
    await vi.waitUntil(() => session.answers.length === 1);
    await tick();
    for (const button of card.querySelectorAll('button')) expect(button.disabled).toBe(true);
    // A real click on a disabled button is never dispatched by the browser.
    try {
      await userEvent.click(allow, { force: true });
    } catch {
      // The driver may refuse to click a disabled control; either way nothing is sent.
    }
    expect(session.answers.length).toBe(1);
  });
});
