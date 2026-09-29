/**
 * WorkspaceMembersCard — pending-admission approvals + invite-by-email for the
 * workspace (tenant) the daemon is syncing. Visible only to an ACTIVE owner or
 * tenant admin; everyone else (member, pending, un-bound, failed load) sees
 * nothing.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor, fireEvent, screen } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const mockInvoke = vi.fn();
import { mockTauriCore } from '../../helpers/mock-tauri-core';
vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

import WorkspaceMembersCard from '$lib/components/settings/sections/workspace-members-card.svelte';
import { databaseStore } from '$lib/stores/database.svelte';

type Row = { person_id: string; email: string; role: string; status: string };

const OWNER: Row = { person_id: 'p-owner', email: 'owner@ex.com', role: 'owner', status: 'active' };
const ADMIN: Row = { person_id: 'p-admin', email: 'admin@ex.com', role: 'tenant_admin', status: 'active' };
const MEMBER: Row = { person_id: 'p-member', email: 'member@ex.com', role: 'member', status: 'active' };
const PENDING: Row = { person_id: 'p-pend', email: 'new@ex.com', role: 'member', status: 'pending' };

interface Setup {
  me: string;
  roster: Row[] | (() => Row[]);
  approve?: () => Promise<unknown>;
  remove?: (args: { personId: string }) => Promise<unknown>;
  initiate?: (args: { email: string }) => Promise<unknown>;
}

function setup({ me, roster, approve, remove, initiate }: Setup) {
  mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case 'pro_current_person':
        return Promise.resolve({ person_id: me, email: 'x@ex.com' });
      case 'pro_list_tenant_members':
        return Promise.resolve(typeof roster === 'function' ? roster() : roster);
      case 'pro_approve_admission':
        return approve ? approve() : Promise.resolve(undefined);
      case 'pro_remove_from_tenant':
        return remove ? remove(args as { personId: string }) : Promise.resolve(undefined);
      case 'pro_initiate_admission':
        return initiate
          ? initiate(args as { email: string })
          : Promise.resolve({ outcome: 'initiated', person_id: 'p-x', status: 'pending' });
      default:
        return Promise.resolve(undefined);
    }
  });
}

function invokedWith(cmd: string) {
  return mockInvoke.mock.calls.filter(([c]) => c === cmd);
}

async function flush() {
  await waitFor(() => expect(invokedWith('pro_list_tenant_members').length).toBeGreaterThan(0));
  // Let the Promise.all resolve and the component re-render.
  await new Promise((r) => setTimeout(r, 0));
}

beforeEach(() => {
  mockInvoke.mockReset();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('WorkspaceMembersCard — visibility', () => {
  it('shows the card to an active owner', async () => {
    setup({ me: OWNER.person_id, roster: [OWNER, PENDING] });
    const { findByTestId } = render(WorkspaceMembersCard);
    expect(await findByTestId('workspace-members-card')).toBeTruthy();
  });

  it('shows the card to an active tenant admin', async () => {
    setup({ me: ADMIN.person_id, roster: [OWNER, ADMIN, PENDING] });
    const { findByTestId } = render(WorkspaceMembersCard);
    expect(await findByTestId('workspace-members-card')).toBeTruthy();
  });

  it('renders nothing for a plain member', async () => {
    setup({ me: MEMBER.person_id, roster: [OWNER, MEMBER, PENDING] });
    const { container } = render(WorkspaceMembersCard);
    await flush();
    expect(container.textContent?.trim()).toBe('');
  });

  it('renders nothing for a pending user, even one whose row says owner', async () => {
    const pendingOwner = { ...OWNER, status: 'pending' };
    setup({ me: OWNER.person_id, roster: [pendingOwner] });
    const { container } = render(WorkspaceMembersCard);
    await flush();
    expect(container.textContent?.trim()).toBe('');
  });

  it('renders nothing on an un-bound device (empty person id)', async () => {
    setup({ me: '', roster: [OWNER, PENDING] });
    const { container } = render(WorkspaceMembersCard);
    await flush();
    expect(container.textContent?.trim()).toBe('');
  });

  it('renders nothing when the roster fails to load (role unknown)', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'pro_current_person') return Promise.resolve({ person_id: OWNER.person_id, email: '' });
      if (cmd === 'pro_list_tenant_members') return Promise.reject('Loading workspace members failed: boom');
      return Promise.resolve(undefined);
    });
    const { container } = render(WorkspaceMembersCard);
    await flush();
    expect(container.textContent?.trim()).toBe('');
  });
});

describe('WorkspaceMembersCard — approve', () => {
  it('lists only pending rows, with an empty state when none are waiting', async () => {
    setup({ me: OWNER.person_id, roster: [OWNER, MEMBER] });
    render(WorkspaceMembersCard);
    expect(await screen.findByText('No one is waiting for approval.')).toBeTruthy();
    expect(screen.queryAllByTestId('pending-admission')).toHaveLength(0);
  });

  it('approves a pending admission and reloads the roster', async () => {
    let approved = false;
    setup({
      me: OWNER.person_id,
      roster: () => (approved ? [OWNER, { ...PENDING, status: 'active' }] : [OWNER, MEMBER, PENDING]),
      approve: () => {
        approved = true;
        return Promise.resolve(undefined);
      }
    });
    render(WorkspaceMembersCard);
    const rows = await screen.findAllByTestId('pending-admission');
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain('new@ex.com');

    await fireEvent.click(screen.getByRole('button', { name: 'Approve new@ex.com' }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('pro_approve_admission', { personId: 'p-pend' })
    );
    expect(await screen.findByText('No one is waiting for approval.')).toBeTruthy();
    expect(invokedWith('pro_list_tenant_members').length).toBe(2);
  });

  it('shows the actionable approve error on that row and keeps it listed', async () => {
    setup({
      me: ADMIN.person_id,
      roster: [ADMIN, PENDING],
      approve: () =>
        Promise.reject(
          'Only a workspace owner or admin can approve members. Ask one of them to approve this person.'
        )
    });
    render(WorkspaceMembersCard);
    await fireEvent.click(await screen.findByRole('button', { name: 'Approve new@ex.com' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Only a workspace owner or admin can approve members');
    expect(screen.getAllByTestId('pending-admission')).toHaveLength(1);
    expect(screen.getByRole('button', { name: 'Approve new@ex.com' })).toHaveProperty('disabled', false);
  });

  it('falls back to a short person id when the pending row has no email', async () => {
    setup({ me: OWNER.person_id, roster: [OWNER, { ...PENDING, email: '', person_id: 'abcdef123456' }] });
    render(WorkspaceMembersCard);
    expect(await screen.findByText('Unknown email (abcdef12)')).toBeTruthy();
  });
});

describe('WorkspaceMembersCard — decline and remove', () => {
  it('declines a pending admission only after a confirmation step, then reloads', async () => {
    let declined = false;
    setup({
      me: OWNER.person_id,
      roster: () => (declined ? [OWNER] : [OWNER, PENDING]),
      remove: () => {
        declined = true;
        return Promise.resolve(undefined);
      }
    });
    render(WorkspaceMembersCard);
    await fireEvent.click(await screen.findByRole('button', { name: 'Decline new@ex.com' }));

    // First click only arms the confirmation; nothing has been sent.
    expect(invokedWith('pro_remove_from_tenant')).toHaveLength(0);
    expect(screen.getByText('Decline new@ex.com?')).toBeTruthy();
    // Approve is hidden while confirming so the two can't be mis-clicked.
    expect(screen.queryByRole('button', { name: 'Approve new@ex.com' })).toBeNull();

    await fireEvent.click(screen.getByRole('button', { name: 'Confirm decline new@ex.com' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('pro_remove_from_tenant', { personId: 'p-pend' })
    );
    expect(await screen.findByText('No one is waiting for approval.')).toBeTruthy();
    expect(invokedWith('pro_list_tenant_members').length).toBe(2);
  });

  it('cancelling the confirmation sends nothing and restores Approve', async () => {
    setup({ me: OWNER.person_id, roster: [OWNER, PENDING] });
    render(WorkspaceMembersCard);
    await fireEvent.click(await screen.findByRole('button', { name: 'Decline new@ex.com' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel decline new@ex.com' }));
    expect(invokedWith('pro_remove_from_tenant')).toHaveLength(0);
    expect(screen.getByRole('button', { name: 'Approve new@ex.com' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Decline new@ex.com' })).toBeTruthy();
  });

  it('shows the actionable decline error on that row and keeps it listed', async () => {
    setup({
      me: ADMIN.person_id,
      roster: [ADMIN, PENDING],
      remove: () => Promise.reject('That person is no longer in this workspace. Refresh the list.')
    });
    render(WorkspaceMembersCard);
    await fireEvent.click(await screen.findByRole('button', { name: 'Decline new@ex.com' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Confirm decline new@ex.com' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('no longer in this workspace');
    expect(screen.getAllByTestId('pending-admission')).toHaveLength(1);
    // The confirmation is disarmed so a retry needs a fresh deliberate click.
    expect(screen.getByRole('button', { name: 'Decline new@ex.com' })).toBeTruthy();
  });

  it('lists active members for removal, excluding the owner and the caller', async () => {
    setup({ me: ADMIN.person_id, roster: [OWNER, ADMIN, MEMBER, PENDING] });
    render(WorkspaceMembersCard);
    const rows = await screen.findAllByTestId('active-member');
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain('member@ex.com');
    expect(screen.queryByRole('button', { name: 'Remove owner@ex.com' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Remove admin@ex.com' })).toBeNull();
    // Pending rows are declined, never "removed".
    expect(screen.queryByRole('button', { name: 'Remove new@ex.com' })).toBeNull();
  });

  it('shows no Members section when nobody else is removable', async () => {
    setup({ me: OWNER.person_id, roster: [OWNER, PENDING] });
    render(WorkspaceMembersCard);
    await screen.findByTestId('pending-admission');
    expect(screen.queryByTestId('active-member')).toBeNull();
    expect(screen.queryByText('Members')).toBeNull();
  });

  it('removes a member after confirmation and reloads the roster', async () => {
    let removed = false;
    setup({
      me: OWNER.person_id,
      roster: () => (removed ? [OWNER] : [OWNER, MEMBER]),
      remove: () => {
        removed = true;
        return Promise.resolve(undefined);
      }
    });
    render(WorkspaceMembersCard);
    await fireEvent.click(await screen.findByRole('button', { name: 'Remove member@ex.com' }));
    expect(invokedWith('pro_remove_from_tenant')).toHaveLength(0);
    await fireEvent.click(screen.getByRole('button', { name: 'Confirm remove member@ex.com' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('pro_remove_from_tenant', { personId: 'p-member' })
    );
    await waitFor(() => expect(screen.queryByTestId('active-member')).toBeNull());
  });
});

describe('WorkspaceMembersCard — invite by email', () => {
  async function renderAsOwner(initiate?: Setup['initiate'], roster: Row[] = [OWNER]) {
    setup({ me: OWNER.person_id, roster, initiate });
    render(WorkspaceMembersCard);
    return (await screen.findByLabelText('Invitee email')) as HTMLInputElement;
  }

  it('keeps Invite disabled until the entry looks like an email', async () => {
    const input = await renderAsOwner();
    const button = screen.getByRole('button', { name: 'Invite' });
    expect(button).toHaveProperty('disabled', true);
    await fireEvent.input(input, { target: { value: 'not-an-email' } });
    expect(button).toHaveProperty('disabled', true);
    await fireEvent.input(input, { target: { value: 'friend@ex.com' } });
    expect(button).toHaveProperty('disabled', false);
  });

  it('invites by trimmed email, confirms, clears the field, and reloads the roster', async () => {
    const input = await renderAsOwner();
    await fireEvent.input(input, { target: { value: '  friend@ex.com ' } });
    await fireEvent.submit(input.closest('form')!);

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('pro_initiate_admission', { email: 'friend@ex.com' })
    );
    const status = await screen.findByRole('status');
    expect(status.textContent).toContain('Invited friend@ex.com');
    expect(status.textContent).toContain('approve them');
    expect(input.value).toBe('');
    await waitFor(() => expect(invokedWith('pro_list_tenant_members').length).toBe(2));
  });

  it('reports an already-pending invitee without calling it an error', async () => {
    const input = await renderAsOwner(() =>
      Promise.resolve({ outcome: 'already_member', person_id: 'p-pend', status: 'pending' })
    );
    await fireEvent.input(input, { target: { value: 'new@ex.com' } });
    await fireEvent.submit(input.closest('form')!);
    const status = await screen.findByRole('status');
    expect(status.textContent).toContain('new@ex.com is already waiting for approval');
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('reports an already-active invitee as a member', async () => {
    const input = await renderAsOwner(() =>
      Promise.resolve({ outcome: 'already_member', person_id: 'p-member', status: 'active' })
    );
    await fireEvent.input(input, { target: { value: 'member@ex.com' } });
    await fireEvent.submit(input.closest('form')!);
    expect((await screen.findByRole('status')).textContent).toContain(
      'member@ex.com is already a member of this workspace'
    );
  });

  it('surfaces the actionable no-account error and keeps the typed email', async () => {
    const input = await renderAsOwner(() =>
      Promise.reject(
        'No NodeSpace account uses that email yet. Ask them to sign in to NodeSpace Pro once, then invite them again.'
      )
    );
    await fireEvent.input(input, { target: { value: 'ghost@ex.com' } });
    await fireEvent.submit(input.closest('form')!);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Ask them to sign in to NodeSpace Pro once');
    expect(input.value).toBe('ghost@ex.com');
    expect(screen.queryByRole('status')).toBeNull();
  });
});

describe('WorkspaceMembersCard — database switch and refresh failures', () => {
  it('waits for the Pro re-target to settle before reading the roster', async () => {
    let settle!: () => void;
    vi.spyOn(databaseStore, 'proSyncSettled').mockReturnValue(
      new Promise<void>((r) => {
        settle = r;
      })
    );
    setup({ me: OWNER.person_id, roster: [OWNER, PENDING] });
    render(WorkspaceMembersCard);

    await new Promise((r) => setTimeout(r, 10));
    expect(invokedWith('pro_list_tenant_members')).toHaveLength(0);
    expect(screen.queryByTestId('workspace-members-card')).toBeNull();

    settle();
    expect(await screen.findByTestId('workspace-members-card')).toBeTruthy();
  });

  it('keeps the admin card with a Retry when a later refresh fails', async () => {
    let calls = 0;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'pro_current_person')
        return Promise.resolve({ person_id: OWNER.person_id, email: '' });
      if (cmd === 'pro_list_tenant_members') {
        calls += 1;
        return calls === 2
          ? Promise.reject(
              "Loading workspace members failed: NodeSpace Pro couldn't reach the cloud."
            )
          : Promise.resolve([OWNER, PENDING]);
      }
      return Promise.resolve(undefined);
    });
    render(WorkspaceMembersCard);

    await fireEvent.click(await screen.findByRole('button', { name: 'Approve new@ex.com' }));

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain("couldn't reach the cloud");
    expect(screen.getByTestId('workspace-members-card')).toBeTruthy();

    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull());
    expect(calls).toBe(3);
    expect(screen.getAllByTestId('pending-admission')).toHaveLength(1);
  });
});
