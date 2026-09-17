/**
 * DatabaseSettings — Settings → Database.
 *
 * Covers the "Add synced database…" entry's visibility gate: it is a direct
 * `proSync.isPro` read that bypasses the `resolveProSyncVariant()` chokepoint
 * (ADR-049), so the Labs "Team synchronization" toggle (default OFF) needs
 * its own explicit AND — `proSync.isPro && labsFlags.syncEnabled` — same as
 * `account-settings.test.ts` covers for the "NodeSpace Pro" card.
 *
 * No Tauri bridge is mocked here: `isTauriBridgePresent()` (database.svelte.ts)
 * is false under plain Happy-DOM, so `databaseStore.load()` takes its
 * no-bridge branch (a single implicit local database, no `invoke` call) and
 * `identity-card.svelte`'s own `invoke('get_local_identity')` rejects into
 * its own caught/logged fallback — both harmless for what this file checks.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn()
}));

import DatabaseSettings from '$lib/components/settings/sections/database-settings.svelte';
import { proSync } from '$lib/stores/pro-sync.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';

function addSyncedButton(container: HTMLElement): HTMLElement | undefined {
  return Array.from(container.querySelectorAll('button')).find(
    (b) => b.textContent?.trim() === 'Add synced database…'
  );
}

describe('DatabaseSettings', () => {
  beforeEach(() => {
    proSync.tier = 'unknown';
    labsFlags.syncEnabled = false;
  });

  afterEach(() => {
    cleanup();
    proSync.tier = 'unknown';
    labsFlags.syncEnabled = false;
    vi.restoreAllMocks();
  });

  it('hides "Add synced database…" for a Pro-capable build when the Labs flag is off (default)', () => {
    proSync.tier = 'pro';
    labsFlags.syncEnabled = false;
    const { container } = render(DatabaseSettings);

    expect(addSyncedButton(container)).toBeUndefined();
  });

  it('shows "Add synced database…" for a Pro-capable build once the Labs flag is on', () => {
    proSync.tier = 'pro';
    labsFlags.syncEnabled = true;
    const { container } = render(DatabaseSettings);

    expect(addSyncedButton(container)).toBeDefined();
  });

  it('never shows "Add synced database…" on a community build, flag on or off', () => {
    proSync.tier = 'community';

    labsFlags.syncEnabled = false;
    const off = render(DatabaseSettings);
    expect(addSyncedButton(off.container)).toBeUndefined();
    off.unmount();

    labsFlags.syncEnabled = true;
    const { container } = render(DatabaseSettings);
    expect(addSyncedButton(container)).toBeUndefined();
  });
});
