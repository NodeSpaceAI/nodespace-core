/**
 * settings-sidebar.svelte — Account tab Labs gating.
 *
 * The "Account" category is the actual safety mechanism hiding the
 * NodeSpace Pro surface from ordinary users by default: it only renders in
 * the category list when `labsFlags.syncEnabled` is true — matching how
 * navigation-sidebar.svelte gates its AI Chats section behind
 * `labsFlags.aiChatEnabled` (see navigation-sidebar-ai-chats-gating.test.ts).
 * This file guards against a future edit accidentally dropping that gate
 * without CI catching it.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';

import SettingsSidebar from '$lib/components/settings/settings-sidebar.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';

describe('SettingsSidebar — Account tab Labs gating', () => {
  beforeEach(() => {
    localStorage.clear();
    labsFlags.syncEnabled = false;
  });

  afterEach(() => {
    cleanup();
    localStorage.clear();
    labsFlags.syncEnabled = false;
  });

  it('hides the Account tab when the flag is off (default)', () => {
    const { queryByText, getByText } = render(SettingsSidebar, {
      props: { activeCategory: 'database', onCategoryChange: vi.fn() }
    });

    expect(queryByText('Account')).toBeNull();
    // The rest of the category list is unaffected.
    expect(getByText('Database')).toBeTruthy();
    expect(getByText('Display')).toBeTruthy();
    expect(getByText('Labs')).toBeTruthy();
  });

  it('shows the Account tab when the flag is on', () => {
    labsFlags.syncEnabled = true;
    const { getByText } = render(SettingsSidebar, {
      props: { activeCategory: 'database', onCategoryChange: vi.fn() }
    });

    expect(getByText('Account')).toBeTruthy();
  });

  it('reactively hides/shows the Account tab as the flag is toggled, without remounting', async () => {
    const { queryByText, findByText } = render(SettingsSidebar, {
      props: { activeCategory: 'database', onCategoryChange: vi.fn() }
    });

    expect(queryByText('Account')).toBeNull();

    labsFlags.syncEnabled = true;
    expect(await findByText('Account')).toBeTruthy();

    labsFlags.syncEnabled = false;
    await vi.waitFor(() => expect(queryByText('Account')).toBeNull());
  });

  it('invokes onCategoryChange with "account" when the tab is clicked', async () => {
    labsFlags.syncEnabled = true;
    const onCategoryChange = vi.fn();
    const { getByText } = render(SettingsSidebar, {
      props: { activeCategory: 'database', onCategoryChange }
    });

    const { fireEvent } = await import('@testing-library/svelte');
    await fireEvent.click(getByText('Account'));

    expect(onCategoryChange).toHaveBeenCalledWith('account');
  });
});
