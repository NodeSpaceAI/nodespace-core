/**
 * The remote-tool confirmation card in a real browser: what it shows (credentials
 * on their own line, the whole destination, arguments with an expander and their
 * real size, invisible characters made visible), where focus goes (the safe
 * choice), what it sends (the daemon's id and digest, unchanged, once), and when
 * it refuses to be answered (a changed card, another device's card).
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import { tick } from 'svelte';
import { userEvent } from '@vitest/browser/context';

import RemoteToolConfirmationCard from '$lib/components/chat/remote-tool-confirmation-card.svelte';
import type { AiChatMessageNode } from '$lib/types/ai-chat-node';
import { confirmationDigest } from '$lib/utils/confirmation-card';

const SUMMARY = 'run `git`; credentials in the environment: TOKEN from keychain:tok';
const ARGS = '{"q":"status"}';

async function cardMessage(overrides: Partial<AiChatMessageNode> = {}): Promise<AiChatMessageNode> {
  return {
    id: 'card-1',
    nodeType: 'ai-chat-message',
    content: SUMMARY,
    version: 1,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    properties: {},
    mentions: [],
    role: 'assistant',
    partKind: 'confirmation',
    confirmationId: 'req-1',
    confirmationDigest: await confirmationDigest(SUMMARY, ARGS),
    confirmationStatus: 'pending',
    confirmationArguments: ARGS,
    confirmationCredentials: ['TOKEN from keychain:tok'],
    ...overrides
  } as unknown as AiChatMessageNode;
}

function mount(message: AiChatMessageNode, onAnswer?: (...a: unknown[]) => Promise<boolean>) {
  return render(RemoteToolConfirmationCard, { props: { message, onAnswer } });
}

function choice(container: HTMLElement, which: 'allow' | 'deny'): HTMLElement & { disabled: boolean } {
  return container.querySelector(`[data-choice="${which}"]`) as HTMLElement & { disabled: boolean };
}

afterEach(() => cleanup());

describe('RemoteToolConfirmationCard (real browser)', () => {
  it('shows the credentials on their own line, the whole summary and the real argument size', async () => {
    const { container } = mount(await cardMessage(), async () => true);
    expect(container.querySelector('[data-testid="credentials"]')?.textContent).toContain(
      'TOKEN from keychain:tok'
    );
    expect(container.querySelector('[data-testid="summary"]')?.textContent).toBe(SUMMARY);
    expect(container.textContent).toContain(`Arguments (${ARGS.length} bytes)`);
  });

  it('puts focus on Don\'t allow, never on Allow', async () => {
    const { container } = mount(await cardMessage(), async () => true);
    await vi.waitUntil(() => document.activeElement === choice(container, 'deny'));
    expect(document.activeElement).not.toBe(choice(container, 'allow'));
  });

  it('Allow sends the id and the digest from the card, unchanged, once', async () => {
    const message = await cardMessage();
    const onAnswer = vi.fn(async () => true);
    const { container } = mount(message, onAnswer);
    await vi.waitUntil(() => !choice(container, 'allow').disabled);
    await userEvent.click(choice(container, 'allow'));
    // A second activation, made from script because the button is now disabled.
    choice(container, 'allow').click();
    choice(container, 'deny').click();
    await tick();
    expect(onAnswer).toHaveBeenCalledTimes(1);
    expect(onAnswer).toHaveBeenCalledWith('req-1', message.confirmationDigest, true);
    expect(choice(container, 'allow').disabled).toBe(true);
    expect(choice(container, 'deny').disabled).toBe(true);
  });

  it('Escape does not allow: it answers no', async () => {
    const message = await cardMessage();
    const onAnswer = vi.fn(async () => true);
    const { container } = mount(message, onAnswer);
    await vi.waitUntil(() => document.activeElement === choice(container, 'deny'));
    await userEvent.keyboard('{Escape}');
    expect(onAnswer).toHaveBeenCalledWith('req-1', message.confirmationDigest, false);
  });

  it('Enter on the focused choice answers no (the default is the safe one)', async () => {
    const message = await cardMessage();
    const onAnswer = vi.fn(async () => true);
    const { container } = mount(message, onAnswer);
    await vi.waitUntil(() => document.activeElement === choice(container, 'deny'));
    await userEvent.keyboard('{Enter}');
    expect(onAnswer).toHaveBeenCalledWith('req-1', message.confirmationDigest, false);
  });

  it('a failed answer is shown and the choices come back', async () => {
    const onAnswer = vi.fn(async () => {
      throw new Error('socket closed');
    });
    const { container } = mount(await cardMessage(), onAnswer);
    await vi.waitUntil(() => !choice(container, 'allow').disabled);
    await userEvent.click(choice(container, 'allow'));
    await vi.waitUntil(() => container.textContent?.includes('socket closed'));
    expect(choice(container, 'allow').disabled).toBe(false);
  });

  it('an answer the daemon did not accept says the question has ended', async () => {
    const { container } = mount(await cardMessage(), async () => false);
    await vi.waitUntil(() => !choice(container, 'allow').disabled);
    await userEvent.click(choice(container, 'allow'));
    await vi.waitUntil(() => container.textContent?.includes('already ended'));
  });

  it('a card whose summary was changed after the daemon wrote it cannot be answered', async () => {
    const message = await cardMessage({ content: 'run `rm`; credentials in the environment: TOKEN from keychain:tok' });
    const onAnswer = vi.fn(async () => true);
    const { container } = mount(message, onAnswer);
    await vi.waitUntil(() => container.textContent?.includes('cannot be answered'));
    expect(container.querySelector('[data-choice]')).toBeNull();
    expect(onAnswer).not.toHaveBeenCalled();
  });

  it('a card whose arguments were changed cannot be answered', async () => {
    const message = await cardMessage({ confirmationArguments: '{"q":"rm -rf"}' });
    const { container } = mount(message, async () => true);
    await vi.waitUntil(() => container.textContent?.includes('cannot be answered'));
    expect(container.querySelector('[data-choice]')).toBeNull();
  });

  it('a credential line that says less than the summary cannot be answered', async () => {
    const message = await cardMessage({ confirmationCredentials: ['TOKEN from keychain:other'] });
    const { container } = mount(message, async () => true);
    await vi.waitUntil(() => container.textContent?.includes('cannot be answered'));
  });

  it("another device's card has no id or digest and is a record only", async () => {
    const message = await cardMessage({ confirmationId: undefined, confirmationDigest: undefined });
    const { container } = mount(message, async () => true);
    expect(container.textContent).toContain('Waiting on the device that is running this chat');
    expect(container.querySelector('[data-choice]')).toBeNull();
  });

  it('a card with an id but no digest is a record only', async () => {
    const message = await cardMessage({ confirmationDigest: undefined });
    const { container } = mount(message, async () => true);
    expect(container.textContent).toContain('Waiting on the device that is running this chat');
    expect(container.querySelector('[data-choice]')).toBeNull();
  });

  it('answers once even if a disabled choice is forced back on', async () => {
    const onAnswer = vi.fn(async () => true);
    const { container } = mount(await cardMessage(), onAnswer);
    await vi.waitUntil(() => !choice(container, 'allow').disabled);
    await userEvent.click(choice(container, 'allow'));
    // Script re-enables the choices and activates them again: the card's own
    // guard, not only the `disabled` attribute, holds.
    for (const which of ['allow', 'deny'] as const) {
      const button = choice(container, which);
      button.disabled = false;
      button.click();
    }
    await tick();
    expect(onAnswer).toHaveBeenCalledTimes(1);
  });

  it('a viewer that cannot answer shows the choices nowhere', async () => {
    const { container } = mount(await cardMessage(), undefined);
    expect(container.querySelector('[data-choice]')).toBeNull();
  });

  it('long arguments are cut with an expander that shows them all and their size', async () => {
    const long = JSON.stringify({ q: 'x'.repeat(2000) });
    const message = await cardMessage({
      confirmationArguments: long,
      confirmationDigest: await confirmationDigest(SUMMARY, long)
    });
    const { container } = mount(message, async () => true);
    const preview = container.querySelector('[data-testid="arguments-preview"]')!.textContent!;
    expect(preview).toContain('more bytes');
    expect(preview.length).toBeLessThan(long.length);
    expect(container.textContent).toContain(`Arguments (${long.length} bytes)`);
    expect(container.querySelector('[data-testid="arguments-full"]')!.textContent).toBe(long);
  });

  it('shows invisible and bidirectional characters in the summary and the arguments as escapes', async () => {
    const summary = 'run `git`‮; credentials: T from keychain:t​\nINFO forged';
    const args = '{"q":"a b\u{e0041}"}';
    const message = await cardMessage({
      content: summary,
      confirmationArguments: args,
      confirmationDigest: await confirmationDigest(summary, args),
      confirmationCredentials: ['T from keychain:t']
    });
    const { container } = mount(message, async () => true);
    const text = container.textContent ?? '';
    for (const ch of ['‮', '​', ' ', '\u{e0041}']) expect(text).not.toContain(ch);
    expect(text).toContain('\\u{202E}');
    expect(text).toContain('\\n');
  });

  it.each([
    ['allowed', 'You allowed this call'],
    ['denied', 'You did not allow this call'],
    ['expired', 'No answer in time'],
    ['cancelled', 'ended without an answer']
  ])('a %s card is a record with no choices', async (status, words) => {
    const { container } = mount(
      await cardMessage({ confirmationStatus: status as never }),
      async () => true
    );
    expect(container.textContent).toContain(words);
    expect(container.querySelector('[data-choice]')).toBeNull();
    expect(container.querySelector('section')?.getAttribute('role')).toBe('group');
  });

  it('a pending card is announced as an alert', async () => {
    const { container } = mount(await cardMessage(), async () => true);
    expect(container.querySelector('section')?.getAttribute('role')).toBe('alert');
  });
});
