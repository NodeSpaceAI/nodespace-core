import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('$lib/utils/logger', () => ({
	createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

// Hoisted mocks so the vi.mock factories can reference them (vi.mock is hoisted
// above imports; vi.hoisted keeps the mock objects available there).
const { svc, proSyncMock } = vi.hoisted(() => ({
	svc: {
		listMembers: vi.fn(),
		listInvites: vi.fn(),
		listRequests: vi.fn(),
		currentPerson: vi.fn(),
		setMember: vi.fn(),
		removeMember: vi.fn(),
		leaveCollection: vi.fn(),
		createInvite: vi.fn(),
		revokeInvite: vi.fn(),
		approveRequest: vi.fn(),
		acceptInvite: vi.fn(),
		requestJoin: vi.fn(),
		joinCollection: vi.fn(),
		listJoinable: vi.fn()
	},
	proSyncMock: { isPro: true }
}));

vi.mock('$lib/services/membership-service', () => ({ membershipService: svc }));
vi.mock('$lib/stores/pro-sync.svelte', () => ({ proSync: proSyncMock }));
// membership.isPro now delegates to the two-axis gate; map it back to the store's
// own `isPro` flag so these roster/invite tests keep toggling one signal.
vi.mock('$lib/plugins/ui-extensions.svelte', () => ({
	isProSyncActive: () => proSyncMock.isPro
}));

import { membership } from '$lib/stores/membership.svelte';

describe('MembershipStore', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		proSyncMock.isPro = true;
		membership.reset();
		// Sensible defaults; individual tests override.
		svc.currentPerson.mockResolvedValue({ personId: 'me', email: 'me@x.com' });
		svc.listMembers.mockResolvedValue([]);
		svc.listInvites.mockResolvedValue([]);
		svc.listRequests.mockResolvedValue([]);
	});

	it('loads roster, derives admin role, and fetches invites+requests for an admin', async () => {
		svc.listMembers.mockResolvedValue([
			{ personId: 'me', permission: 'admin' },
			{ personId: 'bob', permission: 'modify' }
		]);
		svc.listInvites.mockResolvedValue([
			{ id: 'i1', code: 'c', email: '', permission: 'modify', expiresAt: '' }
		]);
		svc.listRequests.mockResolvedValue([
			{ id: 'r1', requestedBy: 'carol', createdAt: '2026-07-02T00:00:00Z' }
		]);

		await membership.loadCollection('c1');

		expect(membership.get('c1').members).toHaveLength(2);
		expect(membership.currentUserRole('c1')).toBe('admin');
		expect(membership.isAdmin('c1')).toBe(true);
		expect(membership.get('c1').invites).toHaveLength(1);
		expect(membership.get('c1').requests).toHaveLength(1);
		expect(svc.listInvites).toHaveBeenCalledWith('c1');
	});

	it('does NOT fetch invites/requests for a non-admin', async () => {
		svc.listMembers.mockResolvedValue([{ personId: 'me', permission: 'readOnly' }]);

		await membership.loadCollection('c1');

		expect(membership.currentUserRole('c1')).toBe('readOnly');
		expect(membership.isAdmin('c1')).toBe(false);
		expect(svc.listInvites).not.toHaveBeenCalled();
		expect(svc.listRequests).not.toHaveBeenCalled();
		expect(membership.get('c1').invites).toEqual([]);
	});

	it('is inert in community mode (never touches the service)', async () => {
		proSyncMock.isPro = false;

		await membership.loadCollection('c1');

		expect(svc.currentPerson).not.toHaveBeenCalled();
		expect(svc.listMembers).not.toHaveBeenCalled();
		expect(membership.get('c1').members).toEqual([]);
		expect(membership.currentUserRole('c1')).toBeNull();
	});

	it('mutations are inert in community mode (never reach the service)', async () => {
		proSyncMock.isPro = false;
		// void mutation: no-ops silently, never touches the service
		await membership.setMember('c1', 'bob', 'modify');
		expect(svc.setMember).not.toHaveBeenCalled();
		// value-returning mutations: throw (never a fake success) and never touch the
		// service — these are the S5 onboarding entry points, so the guard matters.
		await expect(membership.acceptInvite('code')).rejects.toThrow(/Pro/);
		await expect(membership.requestJoin('c1')).rejects.toThrow(/Pro/);
		await expect(membership.joinCollection('c1')).rejects.toThrow(/Pro/);
		expect(svc.acceptInvite).not.toHaveBeenCalled();
		expect(svc.requestJoin).not.toHaveBeenCalled();
		expect(svc.joinCollection).not.toHaveBeenCalled();
	});

	it('joinCollection forwards to the service when Pro (open-collection self-join)', async () => {
		svc.joinCollection.mockResolvedValue(undefined);
		await membership.joinCollection('c1');
		expect(svc.joinCollection).toHaveBeenCalledWith('c1');
	});

	it('loadJoinable caches the discovery list (Pro)', async () => {
		svc.listJoinable.mockResolvedValue([
			{ id: 'c-open', name: 'Marketing', restricted: false },
			{ id: 'c-r', name: 'Legal', restricted: true }
		]);
		await membership.loadJoinable();
		expect(membership.joinable).toHaveLength(2);
		expect(membership.joinableError).toBeNull();
		expect(membership.joinableLoading).toBe(false);
	});

	it('loadJoinable records an error and is inert in community mode', async () => {
		svc.listJoinable.mockRejectedValue(new Error('boom'));
		await membership.loadJoinable();
		expect(membership.joinableError).toContain('boom');
		expect(membership.joinable).toEqual([]);

		proSyncMock.isPro = false;
		svc.listJoinable.mockClear();
		await membership.loadJoinable();
		expect(svc.listJoinable).not.toHaveBeenCalled();
	});

	it('loadJoinable surfaces the real CommandError message when the service rejects with a plain object, not "[object Object]"', async () => {
		// membershipService wraps a Tauri `pro_list_joinable_collections`
		// invoke; a Rust `Result<_, CommandError>` `Err` rejects with a plain
		// object, never an Error instance. The old bare `String(e)` fallback
		// here (no `instanceof Error` guard at all) stringified this to the
		// literal "[object Object]".
		svc.listJoinable.mockRejectedValue({
			message: 'Pro subscription expired',
			code: 'SUBSCRIPTION_EXPIRED'
		});
		await membership.loadJoinable();
		expect(membership.joinableError).toBe('Pro subscription expired');
	});

	it('joining an open collection drops it from a loaded discovery list', async () => {
		svc.listJoinable.mockResolvedValue([
			{ id: 'c-open', name: 'Marketing', restricted: false },
			{ id: 'c-r', name: 'Legal', restricted: true }
		]);
		await membership.loadJoinable();
		svc.joinCollection.mockResolvedValue(undefined);

		await membership.joinCollection('c-open');
		expect(membership.joinable.map((c) => c.id)).toEqual(['c-r']);
	});

	it('requesting a restricted collection leaves it in the discovery list', async () => {
		svc.listJoinable.mockResolvedValue([{ id: 'c-r', name: 'Legal', restricted: true }]);
		await membership.loadJoinable();
		svc.requestJoin.mockResolvedValue('req-1');

		await membership.requestJoin('c-r');
		expect(membership.joinable.map((c) => c.id)).toEqual(['c-r']);
	});

	it('currentUserRole is null when the caller identity is unknown', async () => {
		svc.currentPerson.mockResolvedValue({ personId: '', email: '' });
		svc.listMembers.mockResolvedValue([{ personId: 'someone', permission: 'admin' }]);

		await membership.loadCollection('c1');

		expect(membership.currentUserRole('c1')).toBeNull();
		expect(membership.isAdmin('c1')).toBe(false);
		expect(svc.listInvites).not.toHaveBeenCalled(); // unknown identity ⇒ not admin
	});

	it('a mutation refreshes the affected collection', async () => {
		svc.listMembers.mockResolvedValue([{ personId: 'me', permission: 'admin' }]);
		await membership.loadCollection('c1');
		expect(svc.listMembers).toHaveBeenCalledTimes(1);

		await membership.setMember('c1', 'bob', 'modify');
		expect(svc.setMember).toHaveBeenCalledWith('c1', 'bob', 'modify');
		expect(svc.listMembers).toHaveBeenCalledTimes(2); // reloaded after the mutation
	});

	it('leaving a collection drops its cache entry', async () => {
		svc.listMembers.mockResolvedValue([{ personId: 'me', permission: 'modify' }]);
		await membership.loadCollection('c1');
		expect(membership.get('c1').members).toHaveLength(1);

		await membership.leaveCollection('c1');
		expect(svc.leaveCollection).toHaveBeenCalledWith('c1');
		expect(membership.get('c1').members).toEqual([]);
	});

	it('reset clears cache and identity', async () => {
		svc.listMembers.mockResolvedValue([{ personId: 'me', permission: 'admin' }]);
		await membership.loadCollection('c1');
		expect(membership.currentPerson).not.toBeNull();

		membership.reset();
		expect(membership.currentPerson).toBeNull();
		expect(membership.get('c1').members).toEqual([]);
	});

	describe('invalidateForDatabaseSwitch', () => {
		it('drops the per-collection roster cache but keeps the caller identity', async () => {
			svc.listMembers.mockResolvedValue([{ personId: 'me', permission: 'admin' }]);
			await membership.loadCollection('c1');
			expect(membership.get('c1').members).toHaveLength(1);
			expect(membership.currentPerson).not.toBeNull();

			membership.invalidateForDatabaseSwitch();

			// The roster is gone — a same-id collection in the newly-active
			// database (has_role edges are per-database, ADR-053) must not keep
			// rendering the previous database's roster.
			expect(membership.get('c1').members).toEqual([]);
			// Identity is per Pro sign-in, not per-database — unlike `reset()`
			// (sign-out), a switch must not force a re-fetch of it.
			expect(membership.currentPerson).not.toBeNull();
		});

		it('drops the joinable-collection discovery list', async () => {
			svc.listJoinable.mockResolvedValue([{ id: 'c-open', name: 'Marketing', restricted: false }]);
			await membership.loadJoinable();
			expect(membership.joinable).toHaveLength(1);

			membership.invalidateForDatabaseSwitch();

			expect(membership.joinable).toEqual([]);
		});
	});
});
