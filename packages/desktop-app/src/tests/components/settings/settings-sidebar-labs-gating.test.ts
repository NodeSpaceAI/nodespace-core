/**
 * settings-sidebar.svelte — AI Models and Playbooks Labs gating.
 *
 * "AI Models" is listed only while the Labs "AI Chat" flag is on, and
 * "Playbooks" only while the Labs "Playbooks" flag is on. Both default off.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';

import SettingsSidebar from '$lib/components/settings/settings-sidebar.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';

function resetFlags() {
  localStorage.clear();
  labsFlags.aiChatEnabled = false;
  labsFlags.playbooksEnabled = false;
}

describe('SettingsSidebar — AI Models and Playbooks Labs gating', () => {
  beforeEach(resetFlags);
  afterEach(() => {
    cleanup();
    resetFlags();
  });

  it('hides both categories by default while the rest of the list is unaffected', () => {
    const { queryByText, getByText } = render(SettingsSidebar, {
      props: { activeCategory: 'database', onCategoryChange: vi.fn() }
    });

    expect(queryByText('AI Models')).toBeNull();
    expect(queryByText('Playbooks')).toBeNull();
    expect(queryByText('Work Tracking')).toBeNull();
    expect(getByText('Display')).toBeTruthy();
    expect(getByText('Import Sources')).toBeTruthy();
    expect(getByText('Integrations')).toBeTruthy();
    expect(getByText('Labs')).toBeTruthy();
  });

  it('shows AI Models only when the AI Chat flag is on', () => {
    labsFlags.aiChatEnabled = true;
    const { getByText, queryByText } = render(SettingsSidebar, {
      props: { activeCategory: 'database', onCategoryChange: vi.fn() }
    });

    expect(getByText('AI Models')).toBeTruthy();
    expect(queryByText('Playbooks')).toBeNull();
  });

  it('shows Playbooks only when the Playbooks flag is on, and clicking reports "playbooks"', async () => {
    labsFlags.playbooksEnabled = true;
    const onCategoryChange = vi.fn();
    const { getByText, queryByText } = render(SettingsSidebar, {
      props: { activeCategory: 'database', onCategoryChange }
    });

    expect(queryByText('AI Models')).toBeNull();
    await fireEvent.click(getByText('Playbooks'));
    expect(onCategoryChange).toHaveBeenCalledWith('playbooks');
  });

  it('reactively shows/hides both as the flags toggle, without remounting', async () => {
    const { queryByText, findByText } = render(SettingsSidebar, {
      props: { activeCategory: 'database', onCategoryChange: vi.fn() }
    });

    labsFlags.playbooksEnabled = true;
    labsFlags.aiChatEnabled = true;
    expect(await findByText('Playbooks')).toBeTruthy();
    expect(await findByText('AI Models')).toBeTruthy();

    labsFlags.playbooksEnabled = false;
    labsFlags.aiChatEnabled = false;
    await vi.waitFor(() => {
      expect(queryByText('Playbooks')).toBeNull();
      expect(queryByText('AI Models')).toBeNull();
    });
  });
});
