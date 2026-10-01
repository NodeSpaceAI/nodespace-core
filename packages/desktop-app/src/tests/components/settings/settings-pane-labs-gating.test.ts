/**
 * settings-pane.svelte — routing guard for the Labs-gated AI Models and
 * Playbooks categories. A remembered/requested active category that is hidden
 * while its flag is off must fall back to Database.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn()
}));

vi.mock('$lib/services/methodology-service', () => ({
  listMethodologies: vi.fn().mockResolvedValue([]),
  installMethodology: vi.fn(),
  summarizeReport: vi.fn()
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

describe('SettingsPane — Labs routing guard for AI Models and Playbooks', () => {
  beforeEach(resetFlags);
  afterEach(() => {
    cleanup();
    resetFlags();
  });

  it('falls back to Database when opened on "playbooks" while the flag is off', async () => {
    settingsStore.initialCategory = 'playbooks';
    const { getByText, queryByRole } = render(SettingsPane);

    await waitFor(() => expect(getByText('Databases')).toBeTruthy());
    expect(queryByRole('heading', { name: 'Playbooks' })).toBeNull();
  });

  it('renders the Playbooks page when the flag is on', async () => {
    labsFlags.playbooksEnabled = true;
    settingsStore.initialCategory = 'playbooks';
    const { findByRole } = render(SettingsPane);

    expect(await findByRole('heading', { name: 'Playbooks' })).toBeTruthy();
  });

  it('falls back to Database when the flag flips off while "playbooks" is active', async () => {
    labsFlags.playbooksEnabled = true;
    settingsStore.initialCategory = 'playbooks';
    const { findByRole, getByText, queryByRole } = render(SettingsPane);
    await findByRole('heading', { name: 'Playbooks' });

    labsFlags.playbooksEnabled = false;

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
});
