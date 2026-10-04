/**
 * settings-pane.svelte — routing guard for the Labs-gated AI Models category.
 * A remembered/requested active category that is hidden while its flag is off
 * must fall back to Database. So must an id no category has: the Labs
 * "Playbooks" flag gates the Plays navigation section, not a Settings page.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn()
}));

import SettingsPane from '$lib/components/settings/settings-pane.svelte';
import { settingsStore } from '$lib/stores/settings.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';

function resetFlags() {
  localStorage.clear();
  labsFlags.aiChatEnabled = false;
  labsFlags.playbooksEnabled = false;
  settingsStore.initialCategory = null;
}

describe('SettingsPane — Labs routing guard', () => {
  beforeEach(resetFlags);
  afterEach(() => {
    cleanup();
    resetFlags();
  });

  it('has no Playbooks page, whether the Playbooks flag is on or off', async () => {
    labsFlags.playbooksEnabled = true;
    settingsStore.initialCategory = 'playbooks';
    const { getByText, queryByRole } = render(SettingsPane);

    await waitFor(() => expect(getByText('Databases')).toBeTruthy());
    expect(queryByRole('heading', { name: 'Playbooks' })).toBeNull();
  });

  it('falls back to Database when opened on "ai-models" while AI Chat is off', async () => {
    settingsStore.initialCategory = 'ai-models';
    const { getByText, queryByText } = render(SettingsPane);

    await waitFor(() => expect(getByText('Databases')).toBeTruthy());
    expect(queryByText('AI Models')).toBeNull();
  });

  it('stays on "ai-models" (no redirect to Database) when AI Chat is on', async () => {
    labsFlags.aiChatEnabled = true;
    settingsStore.initialCategory = 'ai-models';
    const { queryByText, getAllByText } = render(SettingsPane);

    // The sidebar lists it and the Database page never renders.
    await waitFor(() => expect(getAllByText('AI Models').length).toBeGreaterThan(0));
    expect(queryByText('Databases')).toBeNull();
  });

  it('falls back to Database when AI Chat flips off while "ai-models" is active', async () => {
    labsFlags.aiChatEnabled = true;
    settingsStore.initialCategory = 'ai-models';
    const { getByText, getAllByText, queryByText } = render(SettingsPane);
    await waitFor(() => expect(getAllByText('AI Models').length).toBeGreaterThan(0));

    labsFlags.aiChatEnabled = false;

    await waitFor(() => expect(getByText('Databases')).toBeTruthy());
    expect(queryByText('AI Models')).toBeNull();
  });
});
