/**
 * settings-pane.svelte — defense-in-depth Account routing guard.
 *
 * settings-sidebar.svelte already hides the "Account" tab while the Labs
 * "Team synchronization" flag is off (settings-sidebar-account-gating.test.ts),
 * which stops a normal click. This file covers the belt-and-suspenders guard
 * in settings-pane.svelte itself: even if `activeCategory` is (or becomes)
 * 'account' by some other means while the flag is off, the pane must fall
 * back to 'database' rather than render the "NodeSpace Pro" surface.
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
import { proSync } from '$lib/stores/pro-sync.svelte';

describe('SettingsPane — Account routing guard when the Labs flag is off', () => {
  beforeEach(() => {
    localStorage.clear();
    labsFlags.syncEnabled = false;
    proSync.tier = 'unknown';
    settingsStore.initialCategory = null;
  });

  afterEach(() => {
    cleanup();
    localStorage.clear();
    labsFlags.syncEnabled = false;
    proSync.tier = 'unknown';
    settingsStore.initialCategory = null;
    vi.restoreAllMocks();
  });

  it('falls back to Database when opened directly on "account" while the flag is off', async () => {
    settingsStore.initialCategory = 'account';
    const { queryByText, getByText } = render(SettingsPane);

    await waitFor(() => expect(getByText('Databases')).toBeTruthy());
    // No "NodeSpace Pro" account content ever surfaces.
    expect(queryByText('NodeSpace Pro')).toBeNull();
  });

  it('does not redirect away from "account" once the flag is on', async () => {
    labsFlags.syncEnabled = true;
    settingsStore.initialCategory = 'account';
    const { getByText } = render(SettingsPane);

    await waitFor(() => expect(getByText('NodeSpace Pro')).toBeTruthy());
  });

  it('redirects to Database if the flag flips off while "account" is already the active view', async () => {
    // No UI path can reach this today (Labs and Account are mutually
    // exclusive views within one settings pane, so a user can't flip the
    // Switch while Account is showing) — but the $effect's own doc comment
    // claims this case is covered ("the flag being turned off elsewhere"),
    // so back that claim directly rather than leaving it implicit.
    labsFlags.syncEnabled = true;
    settingsStore.initialCategory = 'account';
    const { getByText, queryByText } = render(SettingsPane);

    await waitFor(() => expect(getByText('NodeSpace Pro')).toBeTruthy());

    labsFlags.syncEnabled = false;

    await waitFor(() => expect(getByText('Databases')).toBeTruthy());
    expect(queryByText('NodeSpace Pro')).toBeNull();
  });
});
