/**
 * AiChatHeader — the title is laid out without clipping.
 *
 * The header truncates its title with an ellipsis, which is right only when
 * the title actually runs out of room. Whether it does is a layout question
 * (`scrollWidth` against `clientWidth`, element rects), and Happy-DOM computes
 * no layout, so this needs a real browser.
 *
 * `sharedNodeStore` is the real singleton: the header reads the title from it.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import type { Node } from '$lib/types';
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import AiChatHeader from '$lib/components/viewers/ai-chat-header.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';

const CHAT_ID = '5f0c8a52-2f4e-4b1e-9c56-0d6f3c1b7a10';
const HEADER_WIDTH = 400;
const ACTIONS_WIDTH = 120;

function seedChat(title: string): void {
  const node: Node = {
    lifecycleStatus: 'active',
    id: CHAT_ID,
    nodeType: 'ai-chat',
    content: title,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    mentions: []
  };
  sharedNodeStore.setNode(node, { type: 'database', reason: 'seed' });
}

/** Stands in for the model selector the native viewer puts at the right. */
const actions = createRawSnippet(() => ({
  render: () => `<div data-testid="actions" style="width: ${ACTIONS_WIDTH}px; height: 1rem"></div>`
}));

/** Render the header in a fixed-width host, as a viewer pane would hold it. */
function renderHeader({ withActions = true } = {}): HTMLElement {
  const host = document.createElement('div');
  host.style.width = `${HEADER_WIDTH}px`;
  document.body.appendChild(host);
  render(AiChatHeader, {
    target: host,
    props: withActions ? { nodeId: CHAT_ID, actions } : { nodeId: CHAT_ID }
  });
  return host;
}

function titleButton(host: HTMLElement): HTMLElement {
  const button = host.querySelector<HTMLElement>('.chat-viewer-title-button');
  if (!button) throw new Error('No title button rendered');
  return button;
}

/** Where an element's text starts: its left edge, inside border and padding. */
function textLeft(el: HTMLElement): number {
  const style = getComputedStyle(el);
  return (
    el.getBoundingClientRect().left +
    parseFloat(style.borderLeftWidth) +
    parseFloat(style.paddingLeft)
  );
}

describe('AiChatHeader — title layout (browser mode)', () => {
  let host: HTMLElement | undefined;

  beforeEach(() => {
    sharedNodeStore.clearAll();
  });

  afterEach(() => {
    cleanup();
    host?.remove();
    host = undefined;
    sharedNodeStore.clearAll();
  });

  it('shows a short title in full', () => {
    seedChat('Untitled');
    host = renderHeader();

    const button = titleButton(host);
    expect(button.textContent?.trim()).toBe('Untitled');
    expect(button.scrollWidth).toBeLessThanOrEqual(button.clientWidth);

    // The left side takes the row's free space rather than shrinking to the
    // title: everything but the header's padding, the gap and the actions.
    const left = host.querySelector<HTMLElement>('.chat-viewer-header-left');
    if (!left) throw new Error('Header left side not rendered');
    const rem = parseFloat(getComputedStyle(document.documentElement).fontSize);
    expect(left.getBoundingClientRect().width).toBeCloseTo(
      HEADER_WIDTH - ACTIONS_WIDTH - 2.75 * rem,
      0
    );
  });

  it('shows a short title in full when the header has no actions', () => {
    seedChat('Untitled');
    host = renderHeader({ withActions: false });

    const button = titleButton(host);
    expect(button.scrollWidth).toBeLessThanOrEqual(button.clientWidth);
  });

  it('truncates a long title only where it reaches the actions', () => {
    seedChat('A very long chat title that cannot possibly fit beside the model selector '.repeat(3));
    host = renderHeader();

    const button = titleButton(host);
    const left = host.querySelector<HTMLElement>('.chat-viewer-header-left');
    const actionsEl = host.querySelector<HTMLElement>('[data-testid="actions"]');
    if (!left || !actionsEl) throw new Error('Header sides not rendered');

    expect(button.scrollWidth).toBeGreaterThan(button.clientWidth);

    const style = getComputedStyle(button);
    const textRight =
      button.getBoundingClientRect().right -
      parseFloat(style.borderRightWidth) -
      parseFloat(style.paddingRight);
    const leftRect = left.getBoundingClientRect();

    // The text runs the full width of the left side, and the hover target
    // around it stops short of the actions.
    expect(textRight).toBeCloseTo(leftRect.right, 0);
    expect(button.getBoundingClientRect().right).toBeLessThanOrEqual(
      actionsEl.getBoundingClientRect().left
    );
  });

  it('keeps the text in place when switching to edit mode', async () => {
    seedChat('Planning notes');
    host = renderHeader();

    const button = titleButton(host);
    const displayLeft = textLeft(button);
    const displayRect = button.getBoundingClientRect();
    expect(parseFloat(getComputedStyle(button).paddingLeft)).toBeGreaterThan(0);

    await fireEvent.click(button);

    const input = host.querySelector<HTMLInputElement>('.chat-viewer-title-input');
    if (!input) throw new Error('No title input rendered');
    const inputRect = input.getBoundingClientRect();
    expect(textLeft(input)).toBeCloseTo(displayLeft, 1);
    expect(inputRect.top).toBeCloseTo(displayRect.top, 1);
    expect(inputRect.height).toBeCloseTo(displayRect.height, 1);

    // The input fills the left side and stays clear of the actions.
    const actionsEl = host.querySelector<HTMLElement>('[data-testid="actions"]');
    if (!actionsEl) throw new Error('Actions not rendered');
    expect(inputRect.right).toBeLessThanOrEqual(
      actionsEl.getBoundingClientRect().left
    );
  });
});
