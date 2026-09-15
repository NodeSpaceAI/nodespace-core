/**
 * AccountSettings — Settings → Account.
 *
 * Owns the two account-access affordances the removed top-right
 * `pro-sync-pill` overlay used to be the sole home for: signing out and
 * opening the Invitations inbox manually. Sign-in itself is NOT
 * reimplemented here (it lives in `add-synced-database-dialog.svelte`); a
 * signed-out user is only pointed there.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const mockInvoke = vi.fn();
import { mockTauriCore } from '../../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

import AccountSettings from '$lib/components/settings/sections/account-settings.svelte';
import { proSync } from '$lib/stores/pro-sync.svelte';
import { membership } from '$lib/stores/membership.svelte';

describe('AccountSettings', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined);
    proSync.tier = 'unknown';
    proSync.userEmail = '';
  });

  afterEach(() => {
    proSync.tier = 'unknown';
    proSync.userEmail = '';
    vi.restoreAllMocks();
  });

  it('shows a "Not available" state in the community build (not Pro)', () => {
    proSync.tier = 'community';
    const { container } = render(AccountSettings);

    expect(container.textContent).toContain('Not available');
    expect(container.textContent).toContain("This build doesn't include NodeSpace Pro sync.");
    // No sign-out / invitations controls at all.
    expect(container.querySelectorAll('button')).toHaveLength(0);
  });

  it('shows a signed-out state pointing at Database settings when Pro but not signed in', () => {
    proSync.tier = 'pro';
    const { container } = render(AccountSettings);

    expect(container.textContent).toContain('Signed out');
    expect(container.textContent).toContain('Not signed in.');
    expect(container.textContent).toContain('Sign in from Database settings');
    // No Sign out / Invitations buttons while signed out.
    expect(container.textContent).not.toContain('Sign out');
    expect(container.textContent).not.toContain('Invitations');
  });

  it('calls onNavigateToDatabase when the signed-out link is clicked', async () => {
    proSync.tier = 'pro';
    const onNavigateToDatabase = vi.fn();
    const { container } = render(AccountSettings, { props: { onNavigateToDatabase } });

    const link = Array.from(container.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('Sign in from Database settings')
    );
    expect(link).toBeDefined();
    await fireEvent.click(link!);

    expect(onNavigateToDatabase).toHaveBeenCalledTimes(1);
  });

  it('shows the signed-in email plus Invitations and Sign out controls', () => {
    proSync.tier = 'pro';
    proSync.userEmail = 'alice@example.com';
    const { container } = render(AccountSettings);

    expect(container.textContent).toContain('Signed in');
    expect(container.textContent).toContain('alice@example.com');

    const buttons = Array.from(container.querySelectorAll('button')).map((b) =>
      b.textContent?.trim()
    );
    expect(buttons).toContain('Invitations');
    expect(buttons).toContain('Sign out');
  });

  it('opens the Invitations inbox modal when signed in', async () => {
    proSync.tier = 'pro';
    proSync.userEmail = 'alice@example.com';
    const { container } = render(AccountSettings);

    expect(container.querySelector('[role="dialog"]')).toBeNull();

    const invitationsButton = Array.from(container.querySelectorAll('button')).find(
      (b) => b.textContent?.trim() === 'Invitations'
    )!;
    await fireEvent.click(invitationsButton);

    expect(container.querySelector('[role="dialog"]')).not.toBeNull();
    expect(container.textContent).toContain('Invitations');
  });

  it('signs out via the daemon and clears cached membership state', async () => {
    proSync.tier = 'pro';
    proSync.userEmail = 'alice@example.com';
    const resetSpy = vi.spyOn(membership, 'reset');
    const { container } = render(AccountSettings);

    const signOutButton = Array.from(container.querySelectorAll('button')).find(
      (b) => b.textContent?.trim() === 'Sign out'
    )!;
    await fireEvent.click(signOutButton);

    expect(mockInvoke).toHaveBeenCalledWith('pro_signout');
    expect(resetSpy).toHaveBeenCalledTimes(1);
    // proSync.signOut() clears userEmail — the card should fall back to signed-out.
    expect(container.textContent).toContain('Signed out');
  });
});
