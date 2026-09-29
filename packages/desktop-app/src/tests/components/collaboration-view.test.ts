/**
 * CollaborationView component tests.
 * Verifies admin controls gate on the caller's own role and the server's
 * stakeholder gate, and that the community build renders nothing.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, fireEvent, cleanup } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
	createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const { membershipMock, proSyncMock } = vi.hoisted(() => ({
	membershipMock: {
		get: vi.fn(),
		currentUserRole: vi.fn(),
		currentPerson: { personId: 'me' } as { personId: string } | null,
		displayFor: (id: string) => id,
		loadCollection: vi.fn(() => Promise.resolve()),
		setMember: vi.fn(() => Promise.resolve()),
		removeMember: vi.fn(() => Promise.resolve()),
		leaveCollection: vi.fn(() => Promise.resolve()),
		createInvite: vi.fn(() => Promise.resolve('CODE123')),
		revokeInvite: vi.fn(() => Promise.resolve()),
		approveRequest: vi.fn(() => Promise.resolve()),
		rejectRequest: vi.fn(() => Promise.resolve())
	},
	proSyncMock: { isPro: true, onProConfirmed: vi.fn(() => () => {}) }
}));

vi.mock('$lib/stores/membership.svelte', () => ({ membership: membershipMock }));
vi.mock('$lib/stores/pro-sync.svelte', () => ({ proSync: proSyncMock }));

import CollaborationView from '$lib/components/collaboration/collaboration-view.svelte';

function roster(
	members: Array<{ personId: string; permission: string }>,
	invites: Array<Record<string, string>> = [],
	requests: Array<Record<string, string>> = [],
	stakeholderGate: boolean | null = null
) {
	membershipMock.get.mockReturnValue({
		members,
		invites,
		requests,
		stakeholderGate,
		loading: false,
		error: null
	});
}

const INVITE = { id: 'i1', code: 'abc', email: '', permission: 'modify', expiresAt: '' };
const REQUEST = { id: 'r1', requestedBy: 'carol', createdAt: '2026-07-02T00:00:00Z' };
const STAKEHOLDER_ONLY =
	'Only the collection’s creator or a workspace owner/admin can add people to this open collection or change their roles.';

describe('CollaborationView', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		proSyncMock.isPro = true;
		membershipMock.currentPerson = { personId: 'me' };
	});

	it('renders nothing in community mode', () => {
		proSyncMock.isPro = false;
		roster([{ personId: 'me', permission: 'admin' }]);
		const { queryByText } = render(CollaborationView, { props: { collectionId: 'c1' } });
		expect(queryByText('Members')).toBeNull();
		cleanup();
	});

	it('an admin sees role selects, remove, and the add form; self can leave', () => {
		membershipMock.currentUserRole.mockReturnValue('admin');
		roster([
			{ personId: 'me', permission: 'admin' },
			{ personId: 'bob', permission: 'modify' }
		]);
		const { getByLabelText, getByText, queryByLabelText } = render(CollaborationView, {
			props: { collectionId: 'c1' }
		});
		// admin controls on the OTHER member
		expect(getByLabelText('Role for bob')).toBeTruthy();
		expect(getByText('Remove')).toBeTruthy();
		// self row: no role select, a Leave button
		expect(queryByLabelText('Role for me')).toBeNull();
		expect(getByText('Leave')).toBeTruthy();
		// add-existing admin form
		expect(getByText('Add someone already in the workspace')).toBeTruthy();
		cleanup();
	});

	it('a non-admin sees a read-only roster (no selects, no add form)', () => {
		membershipMock.currentUserRole.mockReturnValue('readOnly');
		roster([
			{ personId: 'me', permission: 'readOnly' },
			{ personId: 'bob', permission: 'admin' }
		]);
		const { queryByLabelText, queryByText } = render(CollaborationView, {
			props: { collectionId: 'c1' }
		});
		expect(queryByLabelText('Role for bob')).toBeNull();
		expect(queryByText('Add someone already in the workspace')).toBeNull();
		cleanup();
	});

	it('changing a member role calls setMember', async () => {
		membershipMock.currentUserRole.mockReturnValue('admin');
		roster([
			{ personId: 'me', permission: 'admin' },
			{ personId: 'bob', permission: 'readOnly' }
		]);
		const { getByLabelText } = render(CollaborationView, { props: { collectionId: 'c1' } });
		await fireEvent.change(getByLabelText('Role for bob'), { target: { value: 'modify' } });
		expect(membershipMock.setMember).toHaveBeenCalledWith('c1', 'bob', 'modify');
		cleanup();
	});

	it('loads the collection on mount', () => {
		membershipMock.currentUserRole.mockReturnValue('admin');
		roster([{ personId: 'me', permission: 'admin' }]);
		render(CollaborationView, { props: { collectionId: 'c9' } });
		expect(membershipMock.loadCollection).toHaveBeenCalledWith('c9');
		cleanup();
	});

	// --- S4: invites & requests (admin-only) ---

	it('admin sees the Invites and Join requests sections; non-admin does not', () => {
		membershipMock.currentUserRole.mockReturnValue('admin');
		roster([{ personId: 'me', permission: 'admin' }]);
		const admin = render(CollaborationView, { props: { collectionId: 'c1' } });
		expect(admin.getByText('Create invite')).toBeTruthy();
		expect(admin.getByText('Join requests')).toBeTruthy();
		cleanup();

		membershipMock.currentUserRole.mockReturnValue('readOnly');
		roster([{ personId: 'me', permission: 'readOnly' }]);
		const viewer = render(CollaborationView, { props: { collectionId: 'c1' } });
		expect(viewer.queryByText('Create invite')).toBeNull();
		expect(viewer.queryByText('Join requests')).toBeNull();
		cleanup();
	});

	it('creating a bearer invite calls createInvite (default role/ttl) and shows the code', async () => {
		membershipMock.currentUserRole.mockReturnValue('admin');
		roster([{ personId: 'me', permission: 'admin' }]);
		const { getByText } = render(CollaborationView, { props: { collectionId: 'c1' } });
		await fireEvent.click(getByText('Create invite'));
		// default role readOnly, default ttl 7 days (604800), no email ⇒ undefined
		expect(membershipMock.createInvite).toHaveBeenCalledWith('c1', 'readOnly', undefined, 604800);
		expect(getByText('CODE123')).toBeTruthy(); // bearer code surfaced to copy
		cleanup();
	});

	it('revoking a pending invite calls revokeInvite', async () => {
		membershipMock.currentUserRole.mockReturnValue('admin');
		roster(
			[{ personId: 'me', permission: 'admin' }],
			[{ id: 'i1', code: 'abc', email: '', permission: 'modify', expiresAt: '' }]
		);
		const { getByText } = render(CollaborationView, { props: { collectionId: 'c1' } });
		await fireEvent.click(getByText('Revoke'));
		expect(membershipMock.revokeInvite).toHaveBeenCalledWith('c1', 'i1');
		cleanup();
	});

	it('approving a request uses the selected role; rejecting calls rejectRequest', async () => {
		membershipMock.currentUserRole.mockReturnValue('admin');
		roster(
			[{ personId: 'me', permission: 'admin' }],
			[],
			[{ id: 'r1', requestedBy: 'bob', createdAt: '2026-07-02T00:00:00Z' }]
		);
		const { getByText, getByLabelText } = render(CollaborationView, {
			props: { collectionId: 'c1' }
		});
		await fireEvent.change(getByLabelText('Approve role for bob'), {
			target: { value: 'modify' }
		});
		await fireEvent.click(getByText('Approve'));
		expect(membershipMock.approveRequest).toHaveBeenCalledWith('c1', 'r1', 'modify');

		await fireEvent.click(getByText('Reject'));
		expect(membershipMock.rejectRequest).toHaveBeenCalledWith('c1', 'r1');
		cleanup();
	});


	// --- Stakeholder gate (ADR-037 §2a): on an open collection only its creator or a
	//     workspace owner/admin may add people, change roles, invite or approve. The
	//     gate value is what the server reports for each kind of caller. ---

	type Scenario = {
		who: string;
		role: 'admin' | 'modify' | 'readOnly' | null;
		gate: boolean | null;
		manage: boolean;
	};
	const scenarios: Scenario[] = [
		{ who: 'the creator (open, collection admin)', role: 'admin', gate: true, manage: true },
		{ who: 'a workspace owner (open, collection admin)', role: 'admin', gate: true, manage: true },
		{ who: 'a workspace admin (open, collection admin)', role: 'admin', gate: true, manage: true },
		{ who: 'a collection admin who is not a stakeholder (open)', role: 'admin', gate: false, manage: false },
		{ who: 'a pending workspace member holding admin (open)', role: 'admin', gate: false, manage: false },
		{ who: 'a plain member (open)', role: 'modify', gate: false, manage: false },
		{ who: 'a workspace admin with an editor tier (open)', role: 'modify', gate: true, manage: false },
		{ who: 'a collection admin (restricted)', role: 'admin', gate: true, manage: true },
		{ who: 'a viewer (restricted)', role: 'readOnly', gate: true, manage: false },
		{ who: 'a collection admin on an older daemon (gate unknown)', role: 'admin', gate: null, manage: true }
	];

	it.each(scenarios)('$who: manage controls shown = $manage', ({ role, gate, manage }) => {
		membershipMock.currentUserRole.mockReturnValue(role);
		roster(
			[
				{ personId: 'me', permission: role ?? 'readOnly' },
				{ personId: 'bob', permission: 'modify' }
			],
			[INVITE],
			[REQUEST],
			gate
		);
		const { queryByLabelText, queryByText, queryByPlaceholderText } = render(CollaborationView, {
			props: { collectionId: 'c1' }
		});
		const shown = (el: unknown) => el !== null;
		// Change role, add someone, create an invite, approve a request.
		expect(shown(queryByLabelText('Role for bob'))).toBe(manage);
		expect(shown(queryByPlaceholderText('person node id'))).toBe(manage);
		expect(shown(queryByText('Create invite'))).toBe(manage);
		expect(shown(queryByText('Approve'))).toBe(manage);
		expect(shown(queryByLabelText('Approve role for carol'))).toBe(manage);
		// Remove / revoke / reject stay with every collection admin (deletes aren't
		// stakeholder-limited).
		const admin = role === 'admin';
		expect(shown(queryByText('Remove'))).toBe(admin);
		expect(shown(queryByText('Revoke'))).toBe(admin);
		expect(shown(queryByText('Reject'))).toBe(admin);
		// A refused admin is told why instead.
		expect(shown(queryByText(STAKEHOLDER_ONLY))).toBe(admin && !manage);
		// Self-service leave is always there.
		expect(queryByText('Leave')).toBeTruthy();
		cleanup();
	});

	it('a refused admin sees each other member\'s role as a badge, not a select', () => {
		membershipMock.currentUserRole.mockReturnValue('admin');
		roster(
			[
				{ personId: 'me', permission: 'admin' },
				{ personId: 'bob', permission: 'modify' }
			],
			[],
			[],
			false
		);
		const { queryByLabelText, getAllByText } = render(CollaborationView, {
			props: { collectionId: 'c1' }
		});
		expect(queryByLabelText('Role for bob')).toBeNull();
		expect(getAllByText('Editor').length).toBe(1);
		cleanup();
	});

	// An older daemon doesn't report the gate, so a non-stakeholder admin still sees
	// the controls; the cloud's refusal is then explained, not shown raw.
	const SEED_REFUSAL =
		'SetMember failed: status: PermissionDenied, message: "only the creator or a tenant owner/admin may add or change another person\'s membership of open collection c1"';
	const INVITE_REFUSAL =
		'CreateInvite failed: status: PermissionDenied, message: "only the creator or a tenant owner/admin can create invites for open collection c1"';

	it('explains the stakeholder refusal when changing a role on an older daemon', async () => {
		membershipMock.currentUserRole.mockReturnValue('admin');
		roster(
			[
				{ personId: 'me', permission: 'admin' },
				{ personId: 'bob', permission: 'readOnly' }
			],
			[],
			[],
			null
		);
		membershipMock.setMember.mockRejectedValueOnce(new Error(SEED_REFUSAL));
		const { getByLabelText, findByRole } = render(CollaborationView, {
			props: { collectionId: 'c1' }
		});
		await fireEvent.change(getByLabelText('Role for bob'), { target: { value: 'modify' } });
		expect((await findByRole('alert')).textContent).toBe(STAKEHOLDER_ONLY);
		cleanup();
	});

	it('explains the stakeholder refusal when creating an invite on an older daemon', async () => {
		membershipMock.currentUserRole.mockReturnValue('admin');
		roster([{ personId: 'me', permission: 'admin' }], [], [], null);
		membershipMock.createInvite.mockRejectedValueOnce(new Error(INVITE_REFUSAL));
		const { getByText, findByRole } = render(CollaborationView, {
			props: { collectionId: 'c1' }
		});
		await fireEvent.click(getByText('Create invite'));
		expect((await findByRole('alert')).textContent).toBe(
			'Only the collection’s creator or a workspace owner/admin can create invites for this open collection.'
		);
		cleanup();
	});

	it('explains the creator-only admin grant on an open collection', async () => {
		membershipMock.currentUserRole.mockReturnValue('admin');
		roster(
			[
				{ personId: 'me', permission: 'admin' },
				{ personId: 'bob', permission: 'readOnly' }
			],
			[],
			[],
			true
		);
		membershipMock.setMember.mockRejectedValueOnce(
			new Error(
				'SetMember failed: status: FailedPrecondition, message: "only the creator may grant admin on open collection c1"'
			)
		);
		const { getByLabelText, findByRole } = render(CollaborationView, {
			props: { collectionId: 'c1' }
		});
		await fireEvent.change(getByLabelText('Role for bob'), { target: { value: 'admin' } });
		expect((await findByRole('alert')).textContent).toBe(
			'Only the collection’s creator can make someone an admin of this open collection.'
		);
		cleanup();
	});
});
