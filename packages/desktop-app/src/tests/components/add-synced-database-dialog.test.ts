/**
 * AddSyncedDatabaseDialog — the "add synced database from cloud" wizard
 * (sign in -> list tenants -> pick one -> name + create -> bind).
 *
 * `databaseStore.create`/`switchTo` are spied directly (mirrors
 * first-pro-consent-slot.test.ts's pattern) rather than mocking the whole
 * store module, so the store's own real reactive fields (`error`) still work.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, screen, fireEvent, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';
vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

type SyncStatusCallback = (event: { payload: { user_email?: string } }) => void;
let listenCallback: SyncStatusCallback | null = null;
const mockListen = vi.fn(async (_name: string, cb: SyncStatusCallback) => {
  listenCallback = cb;
  return () => {
    listenCallback = null;
  };
});
vi.mock('@tauri-apps/api/event', () => ({
  listen: (...args: [string, SyncStatusCallback]) => mockListen(...args)
}));

import AddSyncedDatabaseDialog from '$lib/components/settings/sections/add-synced-database-dialog.svelte';
import { databaseStore, type DatabaseInfo } from '$lib/stores/database.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';

function dbEntry(partial: Partial<DatabaseInfo> = {}): DatabaseInfo {
  return {
    id: 'db-new',
    name: 'Demo',
    path: '/tmp/demo.db',
    isDefault: false,
    status: 'open',
    createdAt: new Date().toISOString(),
    lastOpenedAt: null,
    boundTenantSchema: null,
    boundTenantCollection: null,
    ...partial
  };
}

const SIGNED_IN = { personId: 'person-1', email: 'demo@nodespace.dev' };
const SIGNED_OUT = { personId: '', email: '' };

const ONE_ACTIVE_TENANT = {
  memberships: [{ tenantId: 't1', schema: 'tenant_demo', status: 'active', role: 'member' }],
  selection: 'auto-select' as const,
  autoSelected: { tenantId: 't1', schema: 'tenant_demo', status: 'active', role: 'member' }
};

const TWO_ACTIVE_TENANTS = {
  memberships: [
    { tenantId: 't1', schema: 'tenant_demo', status: 'active', role: 'member' },
    { tenantId: 't2', schema: 'tenant_other', status: 'active', role: 'owner' }
  ],
  selection: 'picker' as const,
  autoSelected: null
};

const NO_ACTIVE_TENANTS = {
  memberships: [{ tenantId: 't1', schema: 'tenant_pending', status: 'pending', role: 'member' }],
  selection: 'no-workspace' as const,
  autoSelected: null
};

beforeEach(() => {
  mockInvoke.mockReset();
  mockListen.mockClear();
  listenCallback = null;
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('AddSyncedDatabaseDialog', () => {
  it('renders nothing when closed', () => {
    render(AddSyncedDatabaseDialog, { props: { open: false } });
    expect(screen.queryByText('Add synced database')).toBeNull();
  });

  it('signed in + exactly one active tenant -> auto-selects and jumps to naming', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'pro_current_person') return Promise.resolve(SIGNED_IN);
      if (cmd === 'pro_list_tenant_memberships') return Promise.resolve(ONE_ACTIVE_TENANT);
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    render(AddSyncedDatabaseDialog, { props: { open: true } });

    await waitFor(() => expect(screen.getByPlaceholderText('e.g. Work')).toBeTruthy());
    // Prefilled from the tenant schema, e.g. "tenant_demo" -> "Demo".
    expect((screen.getByPlaceholderText('e.g. Work') as HTMLInputElement).value).toBe('Demo');
    expect(screen.getByText('Create & sync')).toBeTruthy();
  });

  it('signed in + multiple active tenants -> shows a picker, no auto-jump', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'pro_current_person') return Promise.resolve(SIGNED_IN);
      if (cmd === 'pro_list_tenant_memberships') return Promise.resolve(TWO_ACTIVE_TENANTS);
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    render(AddSyncedDatabaseDialog, { props: { open: true } });

    await waitFor(() => expect(screen.getByText('tenant_demo')).toBeTruthy());
    expect(screen.getByText('tenant_other')).toBeTruthy();
    // Naming step hasn't been reached yet — no name input rendered.
    expect(screen.queryByPlaceholderText('e.g. Work')).toBeNull();

    await fireEvent.click(screen.getByText('tenant_other'));

    await waitFor(() =>
      expect((screen.getByPlaceholderText('e.g. Work') as HTMLInputElement).value).toBe('Other')
    );
  });

  it('filters out non-active memberships before deciding auto-select vs picker', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'pro_current_person') return Promise.resolve(SIGNED_IN);
      if (cmd === 'pro_list_tenant_memberships') return Promise.resolve(NO_ACTIVE_TENANTS);
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    render(AddSyncedDatabaseDialog, { props: { open: true } });

    await waitFor(() =>
      expect(screen.getByText(/don't belong to any active workspace/)).toBeTruthy()
    );
  });

  it('not signed in -> offers sign-in, then loads tenants once sync:status carries an email', async () => {
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'pro_current_person') return Promise.resolve(SIGNED_OUT);
      if (cmd === 'pro_initiate_oauth') {
        expect(args).toEqual({ provider: 'google' });
        return Promise.resolve('attempt-1');
      }
      if (cmd === 'pro_list_tenant_memberships') return Promise.resolve(ONE_ACTIVE_TENANT);
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    render(AddSyncedDatabaseDialog, { props: { open: true } });

    await waitFor(() => expect(screen.getByText('Continue with Google')).toBeTruthy());

    await fireEvent.click(screen.getByText('Continue with Google'));

    await waitFor(() => expect(mockListen).toHaveBeenCalledWith('sync:status', expect.any(Function)));
    expect(listenCallback).not.toBeNull();

    // Simulate the daemon's status stream reporting the freshly-signed-in email.
    listenCallback?.({ payload: { user_email: 'demo@nodespace.dev' } });

    await waitFor(() => expect(screen.getByPlaceholderText('e.g. Work')).toBeTruthy());
  });

  it('confirming the bind creates the database, switches to it, mints a landing collection, then binds', async () => {
    const createSpy = vi.spyOn(databaseStore, 'create').mockResolvedValue(dbEntry());
    const switchToSpy = vi.spyOn(databaseStore, 'switchTo').mockResolvedValue(undefined);
    const createNodeSpy = vi
      .spyOn(backendAdapter, 'createNode')
      .mockResolvedValue('coll-landing');

    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'pro_current_person') return Promise.resolve(SIGNED_IN);
      if (cmd === 'pro_list_tenant_memberships') return Promise.resolve(ONE_ACTIVE_TENANT);
      if (cmd === 'pro_bind_tenant') {
        expect(args).toEqual({
          databaseId: 'db-new',
          schema: 'tenant_demo',
          collection: 'coll-landing'
        });
        return Promise.resolve({ synced: true, schema: 'tenant_demo' });
      }
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    let openProp = true;
    const { rerender } = render(AddSyncedDatabaseDialog, { props: { open: openProp } });

    await waitFor(() => expect(screen.getByText('Create & sync')).toBeTruthy());
    await fireEvent.click(screen.getByText('Create & sync'));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('pro_bind_tenant', {
        databaseId: 'db-new',
        schema: 'tenant_demo',
        collection: 'coll-landing'
      })
    );
    expect(createSpy).toHaveBeenCalledWith('Demo');
    // Switched onto the new database exactly once, and BEFORE minting the
    // collection — createNode routes through whichever database is active.
    expect(switchToSpy).toHaveBeenCalledTimes(1);
    expect(switchToSpy).toHaveBeenCalledWith('db-new');
    expect(createNodeSpy).toHaveBeenCalledWith(
      expect.objectContaining({
        nodeType: 'collection',
        content: 'My Workspace',
        properties: { collection: { restrictedToMembers: true } }
      })
    );
    const switchOrder = switchToSpy.mock.invocationCallOrder[0];
    const createNodeOrder = createNodeSpy.mock.invocationCallOrder[0];
    const bindOrder = mockInvoke.mock.calls.findIndex(([cmd]) => cmd === 'pro_bind_tenant');
    expect(switchOrder).toBeLessThan(createNodeOrder);
    expect(createNodeOrder).toBeLessThan(mockInvoke.mock.invocationCallOrder[bindOrder]);

    // The dialog closes itself (sets the bindable `open` back to false) on success.
    openProp = false;
    await rerender({ open: openProp });
  });

  it('a failed bind leaves the dialog on an error step, database already switched to', async () => {
    vi.spyOn(databaseStore, 'create').mockResolvedValue(dbEntry());
    const switchToSpy = vi.spyOn(databaseStore, 'switchTo').mockResolvedValue(undefined);
    vi.spyOn(backendAdapter, 'createNode').mockResolvedValue('coll-landing');

    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'pro_current_person') return Promise.resolve(SIGNED_IN);
      if (cmd === 'pro_list_tenant_memberships') return Promise.resolve(ONE_ACTIVE_TENANT);
      if (cmd === 'pro_bind_tenant') return Promise.reject(new Error('FailedPrecondition'));
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    render(AddSyncedDatabaseDialog, { props: { open: true } });

    await waitFor(() => expect(screen.getByText('Create & sync')).toBeTruthy());
    await fireEvent.click(screen.getByText('Create & sync'));

    await waitFor(() => expect(screen.getByText('FailedPrecondition')).toBeTruthy());
    // The switch to the new (still local-only) database happens up front, before
    // the mint/bind that can fail — so it's already been called once here, not
    // rolled back.
    expect(switchToSpy).toHaveBeenCalledTimes(1);
  });

  it('a failed collection mint leaves the dialog on an error step without calling pro_bind_tenant', async () => {
    vi.spyOn(databaseStore, 'create').mockResolvedValue(dbEntry());
    vi.spyOn(databaseStore, 'switchTo').mockResolvedValue(undefined);
    vi.spyOn(backendAdapter, 'createNode').mockRejectedValue(new Error('disk full'));

    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'pro_current_person') return Promise.resolve(SIGNED_IN);
      if (cmd === 'pro_list_tenant_memberships') return Promise.resolve(ONE_ACTIVE_TENANT);
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    render(AddSyncedDatabaseDialog, { props: { open: true } });

    await waitFor(() => expect(screen.getByText('Create & sync')).toBeTruthy());
    await fireEvent.click(screen.getByText('Create & sync'));

    await waitFor(() => expect(screen.getByText('disk full')).toBeTruthy());
    expect(mockInvoke).not.toHaveBeenCalledWith('pro_bind_tenant', expect.anything());
  });

  it('an identity-check failure surfaces the error with a retry action', async () => {
    const identitySpy = vi.fn().mockRejectedValue(new Error('daemon unreachable'));
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'pro_current_person') return identitySpy();
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    render(AddSyncedDatabaseDialog, { props: { open: true } });

    await waitFor(() => expect(screen.getByText('daemon unreachable')).toBeTruthy());
    expect(screen.getByText('Retry')).toBeTruthy();

    // No tenant has been picked yet — Retry here means "recheck sign-in",
    // i.e. it goes through pro_current_person again (unlike a bind-stage
    // Retry, which resumes confirmBind instead — see the next test).
    await fireEvent.click(screen.getByText('Retry'));
    await waitFor(() => expect(identitySpy).toHaveBeenCalledTimes(2));
  });

  it('retry after a failed bind reuses the already-created database and collection instead of duplicating them', async () => {
    const createSpy = vi.spyOn(databaseStore, 'create').mockResolvedValue(dbEntry());
    const switchToSpy = vi.spyOn(databaseStore, 'switchTo').mockResolvedValue(undefined);
    const createNodeSpy = vi
      .spyOn(backendAdapter, 'createNode')
      .mockResolvedValue('coll-landing');

    const identitySpy = vi.fn().mockResolvedValue(SIGNED_IN);
    const tenantsSpy = vi.fn().mockResolvedValue(ONE_ACTIVE_TENANT);
    let bindCallCount = 0;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'pro_current_person') return identitySpy();
      if (cmd === 'pro_list_tenant_memberships') return tenantsSpy();
      if (cmd === 'pro_bind_tenant') {
        bindCallCount += 1;
        if (bindCallCount === 1) {
          return Promise.reject(new Error('transient network blip'));
        }
        expect(args).toEqual({
          databaseId: 'db-new',
          schema: 'tenant_demo',
          collection: 'coll-landing'
        });
        return Promise.resolve({ synced: true, schema: 'tenant_demo' });
      }
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    render(AddSyncedDatabaseDialog, { props: { open: true } });
    await waitFor(() => expect(screen.getByText('Create & sync')).toBeTruthy());
    await fireEvent.click(screen.getByText('Create & sync'));

    await waitFor(() => expect(screen.getByText('transient network blip')).toBeTruthy());
    expect(createSpy).toHaveBeenCalledTimes(1);
    expect(createNodeSpy).toHaveBeenCalledTimes(1);

    await fireEvent.click(screen.getByText('Retry'));

    await waitFor(() => expect(bindCallCount).toBe(2));
    // The whole point of the fix: a retry must NOT recreate the database or
    // mint a second collection, and must not restart from the identity check
    // or tenant listing either — it resumes confirmBind directly.
    expect(createSpy).toHaveBeenCalledTimes(1);
    expect(createNodeSpy).toHaveBeenCalledTimes(1);
    expect(identitySpy).toHaveBeenCalledTimes(1);
    expect(tenantsSpy).toHaveBeenCalledTimes(1);
    expect(switchToSpy).toHaveBeenCalledTimes(2);
  });

  it('cannot be dismissed while a bind is in flight, and closes on its own once it settles', async () => {
    vi.spyOn(databaseStore, 'create').mockResolvedValue(dbEntry());
    vi.spyOn(databaseStore, 'switchTo').mockResolvedValue(undefined);
    vi.spyOn(backendAdapter, 'createNode').mockResolvedValue('coll-landing');

    // A mutable holder (rather than a bare reassigned `let`) so TypeScript
    // doesn't narrow the closure-assigned function to `never` at the call site.
    const bindGate: { resolve: (() => void) | null } = { resolve: null };
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'pro_current_person') return Promise.resolve(SIGNED_IN);
      if (cmd === 'pro_list_tenant_memberships') return Promise.resolve(ONE_ACTIVE_TENANT);
      if (cmd === 'pro_bind_tenant') {
        return new Promise((resolve) => {
          bindGate.resolve = () => resolve({ synced: true, schema: 'tenant_demo' });
        });
      }
      throw new Error(`unexpected invoke: ${cmd}`);
    });

    render(AddSyncedDatabaseDialog, { props: { open: true } });
    await waitFor(() => expect(screen.getByText('Create & sync')).toBeTruthy());
    await fireEvent.click(screen.getByText('Create & sync'));
    await waitFor(() => expect(screen.getByText(/Creating .* binding/)).toBeTruthy());

    // The X close button is not rendered at all while binding.
    expect(screen.queryByRole('button', { name: 'Close' })).toBeNull();

    // Escape must not close the dialog mid-bind: Dialog.Content's
    // escapeKeydownBehavior is set to 'ignore' while step === 'binding' (see
    // the file doc comment) — without it this would unmount the dialog
    // content entirely (confirmed: this assertion fails without the fix).
    await fireEvent.keyDown(document, { key: 'Escape' });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.getByText(/Creating .* binding/)).toBeTruthy();

    // Once the bind resolves, the dialog closes itself via the normal
    // success path (close()), unaffected by the binding-only guard above.
    bindGate.resolve?.();
    await waitFor(() => expect(screen.queryByText(/Creating .* binding/)).toBeNull());
  });
});
