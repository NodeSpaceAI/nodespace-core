/**
 * AccountSettings — Settings → Account.
 *
 * Owns the two account-access affordances the removed top-right
 * `pro-sync-pill` overlay used to be the sole home for: signing out and
 * opening the Invitations inbox manually. Sign-in itself is NOT
 * reimplemented here (it lives in `add-synced-database-dialog.svelte`); a
 * signed-out user is only pointed there.
 *
 * Signed-in status is read via `pro_current_person` (the daemon's GLOBAL
 * identity), not `proSync.userEmail` (per-database-attributed, per ADR-053) —
 * mirrors `add-synced-database-dialog.svelte`'s own identity check, and its
 * documented reason for avoiding `proSync`'s per-database getters here.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor, fireEvent } from '@testing-library/svelte';
import type { Node } from '$lib/types';

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
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import { membership } from '$lib/stores/membership.svelte';
import { SharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { DATABASE_SETTINGS_NODE_ID } from '$lib/plugins/ui-extensions';

const SIGNED_IN = { personId: 'person-1', email: 'alice@example.com' };
const SIGNED_OUT = { personId: '', email: '' };

/** Sets up `pro_current_person` to resolve `identity`; everything else no-ops. */
function mockIdentity(identity: { personId: string; email: string }) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'pro_current_person') return Promise.resolve(identity);
    return Promise.resolve(undefined);
  });
}

/** Seeds the active database's settings singleton, same shape `resolveProSyncVariant`
 *  (via `activeDatabaseSettings`) reads — see ui-extensions.test.ts's identical helper. */
function seedSettings(props: { sync_enabled?: boolean; auth_status?: string }): void {
  const node: Node = {
    id: DATABASE_SETTINGS_NODE_ID,
    nodeType: 'database-settings',
    content: '',
    properties: props,
    mentions: [],
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1
  };
  SharedNodeStore.getInstance().setNode(node, { type: 'database', reason: 'seed' }, true);
}

describe('AccountSettings', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined);
    SharedNodeStore.resetInstance();
    proSync.tier = 'unknown';
    proSync.userEmail = '';
    // The Labs "Team synchronization" toggle (default OFF) is a separate,
    // client-side visibility gate ANDed with `proSync.isPro` for this card
    // (see the dedicated describe block below) — default it ON here so the
    // pre-existing tests continue to exercise `proSync.isPro`/sign-in-state
    // behavior in isolation, as they did before that gate existed.
    labsFlags.syncEnabled = true;
  });

  afterEach(() => {
    cleanup();
    proSync.tier = 'unknown';
    proSync.userEmail = '';
    labsFlags.syncEnabled = false;
    membership.reset();
    vi.restoreAllMocks();
  });

  it('shows a "Not available" state in the community build (not Pro)', () => {
    proSync.tier = 'community';
    const { container } = render(AccountSettings);

    expect(container.textContent).toContain('Not available');
    expect(container.textContent).toContain("This build doesn't include NodeSpace Pro sync.");
    // No sign-out / invitations controls at all.
    expect(container.querySelectorAll('button')).toHaveLength(0);
    // Community tier never probes the daemon for identity.
    expect(mockInvoke).not.toHaveBeenCalledWith('pro_current_person');
  });

  it('shows a signed-out state pointing at Database settings when Pro but not signed in', async () => {
    proSync.tier = 'pro';
    mockIdentity(SIGNED_OUT);
    const { container } = render(AccountSettings);

    await waitFor(() => expect(container.textContent).toContain('Signed out'));
    expect(container.textContent).toContain('Not signed in.');
    expect(container.textContent).toContain('Sign in from Database settings');
    // No Sign out / Invitations buttons while signed out.
    expect(container.textContent).not.toContain('Sign out');
    expect(container.textContent).not.toContain('Invitations');
  });

  it('calls onNavigateToDatabase when the signed-out link is clicked', async () => {
    proSync.tier = 'pro';
    mockIdentity(SIGNED_OUT);
    const onNavigateToDatabase = vi.fn();
    const { container } = render(AccountSettings, { props: { onNavigateToDatabase } });

    await waitFor(() => expect(container.textContent).toContain('Signed out'));
    const link = Array.from(container.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('Sign in from Database settings')
    );
    expect(link).toBeDefined();
    await fireEvent.click(link!);

    expect(onNavigateToDatabase).toHaveBeenCalledTimes(1);
  });

  it('shows the signed-in email plus Invitations and Sign out controls', async () => {
    proSync.tier = 'pro';
    mockIdentity(SIGNED_IN);
    const { container } = render(AccountSettings);

    await waitFor(() => expect(container.textContent).toContain('Signed in'));
    expect(container.textContent).toContain('alice@example.com');

    const buttons = Array.from(container.querySelectorAll('button')).map((b) =>
      b.textContent?.trim()
    );
    expect(buttons).toContain('Invitations');
    expect(buttons).toContain('Sign out');
  });

  it('offers a "Turn on sync" action for a signed-in user who hasn\'t opted in yet (consent variant)', async () => {
    // Reachability closes the gap left by deleting the always-mounted
    // enable-sync-pill: collaboration-locked.svelte's own reopen button only
    // exists inside a specific collection's Collaboration tab, so this is the
    // only globally-reachable "Turn on sync" surface post-removal.
    proSync.tier = 'pro';
    seedSettings({ sync_enabled: false, auth_status: 'connected' });
    mockIdentity(SIGNED_IN);
    const { container } = render(AccountSettings);

    await waitFor(() => expect(container.textContent).toContain('Signed in'));
    expect(container.textContent).toContain("Sync isn't turned on for this database yet");
    const turnOnButton = Array.from(container.querySelectorAll('button')).find(
      (b) => b.textContent?.trim() === 'Turn on sync'
    );
    expect(turnOnButton).toBeDefined();

    proSync.consentPromptOpen = false;
    await fireEvent.click(turnOnButton!);
    expect(proSync.consentPromptOpen).toBe(true);
  });

  it('does not offer "Turn on sync" once sync is already enabled (connected variant)', async () => {
    proSync.tier = 'pro';
    seedSettings({ sync_enabled: true, auth_status: 'connected' });
    mockIdentity(SIGNED_IN);
    const { container } = render(AccountSettings);

    await waitFor(() => expect(container.textContent).toContain('Signed in'));
    const buttons = Array.from(container.querySelectorAll('button')).map((b) =>
      b.textContent?.trim()
    );
    expect(buttons).not.toContain('Turn on sync');
  });

  it('reflects the global daemon identity even when the active database is local-only', async () => {
    // The regression this section exists to avoid: `proSync.userEmail` is
    // per-database-attributed and forces empty for a local-only database
    // (ADR-053), even while the user is genuinely signed in to Pro globally.
    // Leaving `proSync.userEmail` empty here (unlike the other signed-in
    // tests, which don't touch it either — the component must not depend on
    // it at all) while `pro_current_person` reports a real identity must
    // still resolve to "Signed in".
    proSync.tier = 'pro';
    proSync.userEmail = '';
    mockIdentity(SIGNED_IN);
    const { container } = render(AccountSettings);

    await waitFor(() => expect(container.textContent).toContain('Signed in'));
    expect(container.textContent).toContain('alice@example.com');
    expect(container.textContent).not.toContain('Signed out');
  });

  it('opens the Invitations inbox modal when signed in', async () => {
    proSync.tier = 'pro';
    mockIdentity(SIGNED_IN);
    const { container } = render(AccountSettings);

    await waitFor(() => expect(container.textContent).toContain('Signed in'));
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
    mockIdentity(SIGNED_IN);
    const resetSpy = vi.spyOn(membership, 'reset');
    const { container } = render(AccountSettings);

    await waitFor(() => expect(container.textContent).toContain('Signed in'));
    const signOutButton = Array.from(container.querySelectorAll('button')).find(
      (b) => b.textContent?.trim() === 'Sign out'
    )!;
    await fireEvent.click(signOutButton);

    expect(mockInvoke).toHaveBeenCalledWith('pro_signout');
    expect(resetSpy).toHaveBeenCalledTimes(1);
    // proSync.signOut() clears userEmail, and signOut() also clears the
    // locally-tracked global identity — the card should fall back to signed-out.
    expect(container.textContent).toContain('Signed out');
  });

  describe('Labs "Team synchronization" toggle (default OFF)', () => {
    it('hides all Pro card content on a signed-out Pro-capable build when the flag is off (default)', () => {
      proSync.tier = 'pro';
      labsFlags.syncEnabled = false;
      const { container } = render(AccountSettings);

      // Identical to the community-build ("not Pro") rendering — the flag being
      // off must look exactly like the surface not existing at all.
      expect(container.textContent).toContain('Not available');
      expect(container.textContent).toContain("This build doesn't include NodeSpace Pro sync.");
      expect(container.querySelectorAll('button')).toHaveLength(0);
      // The card must not probe the daemon for identity while hidden.
      expect(mockInvoke).not.toHaveBeenCalledWith('pro_current_person');
    });

    it('hides Pro card content even for an already-signed-in Pro user when the flag is off', async () => {
      proSync.tier = 'pro';
      labsFlags.syncEnabled = false;
      mockIdentity(SIGNED_IN);
      const { container } = render(AccountSettings);

      expect(container.textContent).toContain('Not available');
      expect(container.textContent).not.toContain('alice@example.com');
      expect(container.querySelectorAll('button')).toHaveLength(0);
      expect(mockInvoke).not.toHaveBeenCalledWith('pro_current_person');
    });

    it('flipping the flag on reveals exactly the pre-existing signed-in behavior, unchanged', async () => {
      proSync.tier = 'pro';
      labsFlags.syncEnabled = false;
      mockIdentity(SIGNED_IN);
      const { container } = render(AccountSettings);
      expect(container.textContent).toContain('Not available');

      labsFlags.syncEnabled = true;

      await waitFor(() => expect(container.textContent).toContain('Signed in'));
      expect(container.textContent).toContain('alice@example.com');
      const buttons = Array.from(container.querySelectorAll('button')).map((b) =>
        b.textContent?.trim()
      );
      expect(buttons).toContain('Invitations');
      expect(buttons).toContain('Sign out');
    });
  });
});
