/**
 * navigation-sidebar.svelte — AI Chats Labs gating.
 *
 * The "AI Chats" section (collapsible list + "+ New chat") is the actual
 * safety mechanism hiding AI Chat from ordinary users by default: it only
 * renders when `labsFlags.aiChatEnabled` is true. That gate wraps BOTH the
 * expanded-sidebar branch (accordion trigger) and the collapsed-icon branch
 * (icon-only button) — see the `{#if labsFlags.aiChatEnabled}` around both in
 * navigation-sidebar.svelte. This file guards against a future edit
 * accidentally dropping that gate without CI catching it.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';

import NavigationSidebar from '$lib/components/layout/navigation-sidebar.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import { layoutStore } from '$lib/stores/layout.svelte';

function setCollapsed(collapsed: boolean) {
  layoutStore.state = { ...layoutStore.state, sidebarCollapsed: collapsed };
}

describe('NavigationSidebar — AI Chats Labs gating', () => {
  beforeEach(() => {
    localStorage.clear();
    labsFlags.aiChatEnabled = false;
    setCollapsed(false);
  });

  afterEach(() => {
    cleanup();
    localStorage.clear();
    labsFlags.aiChatEnabled = false;
    setCollapsed(false);
  });

  it('expanded sidebar: hides the AI Chats section and "+ New chat" when the flag is off', () => {
    setCollapsed(false);
    const { container, queryByText } = render(NavigationSidebar);

    expect(queryByText('AI Chats')).toBeNull();
    expect(queryByText('+ New chat')).toBeNull();
    expect(container.querySelector('[aria-label="Expand AI Chats"]')).toBeNull();
    expect(container.querySelector('[aria-label="Collapse AI Chats"]')).toBeNull();
  });

  it('collapsed sidebar: hides the AI Chats icon button when the flag is off', () => {
    setCollapsed(true);
    const { container, queryByText } = render(NavigationSidebar);

    expect(container.querySelector('button[title="AI Chats"]')).toBeNull();
    expect(queryByText('AI Chats')).toBeNull();
    expect(queryByText('+ New chat')).toBeNull();
  });

  it('expanded sidebar: renders the AI Chats section when the flag is on', () => {
    labsFlags.aiChatEnabled = true;
    setCollapsed(false);
    const { getByText, container } = render(NavigationSidebar);

    expect(getByText('AI Chats')).toBeTruthy();
    expect(getByText('+ New chat')).toBeTruthy();
    expect(container.querySelector('[aria-label="Expand AI Chats"]')).not.toBeNull();
  });

  it('collapsed sidebar: renders the AI Chats icon button when the flag is on', () => {
    labsFlags.aiChatEnabled = true;
    setCollapsed(true);
    const { container } = render(NavigationSidebar);

    expect(container.querySelector('button[title="AI Chats"]')).not.toBeNull();
  });
});
