/**
 * settings-sidebar.svelte — AI Models Labs gating.
 *
 * "AI Models" is listed only while the Labs "AI Chat" flag is on (default
 * off). The Labs "Playbooks" flag lists nothing here: it gates the Plays
 * section of the navigation sidebar.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';

import SettingsSidebar from '$lib/components/settings/settings-sidebar.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';

function resetFlags() {
  localStorage.clear();
  labsFlags.aiChatEnabled = false;
  labsFlags.playbooksEnabled = false;
}

describe('SettingsSidebar — AI Models Labs gating', () => {
  beforeEach(resetFlags);
  afterEach(() => {
    cleanup();
    resetFlags();
  });

  it('hides AI Models by default while the rest of the list is unaffected', () => {
    const { queryByText, getByText } = render(SettingsSidebar, {
      props: { activeCategory: 'database', onCategoryChange: vi.fn() }
    });

    expect(queryByText('AI Models')).toBeNull();
    expect(queryByText('Work Tracking')).toBeNull();
    expect(getByText('Display')).toBeTruthy();
    expect(getByText('Import Sources')).toBeTruthy();
    expect(getByText('Integrations')).toBeTruthy();
    expect(getByText('Labs')).toBeTruthy();
  });

  it('shows AI Models only when the AI Chat flag is on', () => {
    labsFlags.aiChatEnabled = true;
    const { getByText } = render(SettingsSidebar, {
      props: { activeCategory: 'database', onCategoryChange: vi.fn() }
    });

    expect(getByText('AI Models')).toBeTruthy();
  });

  it('lists no Playbooks category, whether the Playbooks flag is on or off', () => {
    labsFlags.playbooksEnabled = true;
    const { queryByText } = render(SettingsSidebar, {
      props: { activeCategory: 'database', onCategoryChange: vi.fn() }
    });

    expect(queryByText('Playbooks')).toBeNull();
  });

  it('reactively shows/hides AI Models as the flag toggles, without remounting', async () => {
    const { queryByText, findByText } = render(SettingsSidebar, {
      props: { activeCategory: 'database', onCategoryChange: vi.fn() }
    });

    labsFlags.aiChatEnabled = true;
    expect(await findByText('AI Models')).toBeTruthy();

    labsFlags.aiChatEnabled = false;
    await vi.waitFor(() => {
      expect(queryByText('AI Models')).toBeNull();
    });
  });
});
