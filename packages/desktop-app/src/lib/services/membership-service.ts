/**
 * Membership service — thin, typed wrappers over the Pro `pro_*` membership
 * Tauri commands.
 *
 * The daemon forwards the signed-in user's JWT to the matching cloud RPC, so the
 * admin / last-admin / open-vs-restricted gates are enforced *server-side*; these
 * wrappers carry no authority of their own. Every command resolves the Pro client
 * or errors in community mode, so callers must gate on `proSync.tier === 'pro'`
 * (the membership store does this) before invoking — the community build never
 * reaches here.
 *
 * Boundary mapping: the Rust command DTOs serialize snake_case (`person_id`,
 * `expires_at`, …); this module maps them to idiomatic camelCase interfaces so
 * the rest of the frontend never sees the wire shape.
 */
import { invoke } from '@tauri-apps/api/core';

import { createLogger } from '$lib/utils/logger';

const log = createLogger('MembershipService');

/** ADR-037 permission tiers. */
export type Permission = 'admin' | 'modify' | 'readOnly';

/** A collection roster entry (person + their tier). */
export interface Member {
	personId: string;
	permission: Permission;
}

/** A pending invite (admin-only listing). */
export interface Invite {
	/** uuid — the handle {@link MembershipService.revokeInvite} takes. */
	id: string;
	/** 64-hex share code (bearer); may be surfaced for the admin to copy. */
	code: string;
	/** Bound invitee email; `''` for a bearer share-code. */
	email: string;
	permission: Permission;
	/** RFC3339; `''` when the invite never expires. */
	expiresAt: string;
}

/** A pending join request (admin-only listing). */
export interface JoinRequest {
	/** uuid — the handle {@link MembershipService.approveRequest} / revokeInvite take. */
	id: string;
	/** The requester's person_node_id. */
	requestedBy: string;
	/** RFC3339. */
	createdAt: string;
}

/** The caller's own identity. */
export interface Person {
	/** Bound PersonNode id; `''` on an un-bound device ("role unknown"). */
	personId: string;
	/** Signed-in email; `''` when signed out. */
	email: string;
}

/** A collection the caller could join but isn't a member of yet (browse & join). */
export interface JoinableCollection {
	/** Collection node id. */
	id: string;
	/** Display name (the collection node's content). */
	name: string;
	/** `true` => needs a request (admin approval); `false` => open self-join. */
	restricted: boolean;
}

/**
 * One row of the active workspace's (tenant's) roster. `status === 'pending'`
 * rows are admissions waiting for an owner or tenant admin to approve them.
 */
export interface TenantMember {
	/** Person node id — the handle {@link MembershipService.approveAdmission} takes. */
	personId: string;
	/** Signed-in email; `''` when the cloud has none on file. */
	email: string;
	/** `'owner' | 'tenant_admin' | 'member'` (open set). */
	role: string;
	/** `'pending' | 'active'`. */
	status: string;
}

/** Result of {@link MembershipService.initiateAdmission}. */
export interface AdmissionResult {
	/** `'initiated'` (a new pending admission) or `'already_member'`. */
	outcome: string;
	personId: string;
	/** The invitee's admission status after the call. */
	status: string;
}

/** Tenant roles allowed to approve admissions (the cloud enforces the same set). */
const TENANT_ADMIN_ROLES = new Set(['owner', 'tenant_admin']);

/**
 * Whether `member` may approve admissions and manage the workspace roster: an
 * ACTIVE owner or tenant admin. A pending owner/admin row (never expected, but
 * the status is authoritative) does not qualify.
 */
export function isTenantAdmin(member: TenantMember | undefined): boolean {
	return !!member && member.status === 'active' && TENANT_ADMIN_ROLES.has(member.role);
}

// Raw wire shapes (snake_case, as serialized by the Rust command DTOs).
interface RawMember {
	person_id: string;
	permission: string;
}
interface RawInvite {
	id: string;
	code: string;
	email: string;
	permission: string;
	expires_at: string;
}
interface RawRequest {
	id: string;
	requested_by: string;
	created_at: string;
}
interface RawPerson {
	person_id: string;
	email: string;
}
interface RawTenantMember {
	person_id: string;
	email: string;
	role: string;
	status: string;
}
interface RawAdmission {
	outcome: string;
	person_id: string;
	status: string;
}
interface RawJoinableCollection {
	id: string;
	name: string;
	restricted: boolean;
}

/**
 * Typed facade over the membership commands. A single class (not the
 * Tauri/Mock/Http triplet used by `collection-service`) because tests mock
 * `@tauri-apps/api/core` directly, matching the `pro-sync` store's test pattern.
 */
export class MembershipService {
	/** Roster for a collection (any member can read). */
	async listMembers(collectionId: string): Promise<Member[]> {
		log.debug('listMembers', { collectionId });
		const rows = await invoke<RawMember[]>('pro_list_members', { collectionId });
		// Collapse duplicate rows for the same person: a person can carry more than
		// one `member_of` edge (e.g. a role-bearing edge plus a plain membership
		// edge), so the roster RPC can return the same `person_id` twice. Keep the
		// highest-privilege permission and emit one row per person — otherwise the
		// keyed `{#each … (personId)}` roster hits a duplicate-key crash and the
		// whole Collaboration panel is stuck on "Loading members…".
		const RANK: Record<Permission, number> = { readOnly: 0, modify: 1, admin: 2 };
		const byPerson = new Map<string, Member>();
		for (const r of rows) {
			const permission = r.permission as Permission;
			const existing = byPerson.get(r.person_id);
			if (!existing || (RANK[permission] ?? 0) > (RANK[existing.permission] ?? 0)) {
				byPerson.set(r.person_id, { personId: r.person_id, permission });
			}
		}
		return [...byPerson.values()];
	}

	/** Add a member or change their role (admin only, server-gated). */
	async setMember(collectionId: string, personId: string, permission: Permission): Promise<void> {
		log.debug('setMember', { collectionId, personId, permission });
		await invoke<void>('pro_set_member', { collectionId, personId, permission });
	}

	/** Remove a member / kick (admin only, last-admin protected server-side). */
	async removeMember(collectionId: string, personId: string): Promise<void> {
		log.debug('removeMember', { collectionId, personId });
		await invoke<void>('pro_remove_member', { collectionId, personId });
	}

	/** Leave a collection (self-removal). */
	async leaveCollection(collectionId: string): Promise<void> {
		log.debug('leaveCollection', { collectionId });
		await invoke<void>('pro_leave_collection', { collectionId });
	}

	/**
	 * Mint an invite code (admin only). `email` binds the invite to that address
	 * (only that signed-in user may redeem); omit for a bearer share-code.
	 * `ttlSecs` overrides the server default lifetime (7 days). Returns the code.
	 */
	async createInvite(
		collectionId: string,
		permission: Permission,
		email?: string,
		ttlSecs?: number
	): Promise<string> {
		log.debug('createInvite', { collectionId, permission, hasEmail: !!email, ttlSecs });
		return invoke<string>('pro_create_invite', {
			collectionId,
			permission,
			email: email || undefined,
			ttlSecs: ttlSecs || undefined
		});
	}

	/** Redeem an invite code (invitee self-provisions). Returns the joined collection id. */
	async acceptInvite(code: string): Promise<string> {
		log.debug('acceptInvite', { hasCode: !!code });
		return invoke<string>('pro_accept_invite', { code });
	}

	/** Ask to join a restricted collection. Returns the request id. */
	async requestJoin(collectionId: string): Promise<string> {
		log.debug('requestJoin', { collectionId });
		return invoke<string>('pro_request_join', { collectionId });
	}

	/**
	 * Self-join an OPEN collection (the complement to {@link requestJoin}, which is
	 * for restricted collections). The open-vs-restricted gate is enforced
	 * server-side — the daemon/cloud reject a restricted collection with a permission
	 * error, so no client-side check is needed.
	 */
	async joinCollection(collectionId: string): Promise<void> {
		log.debug('joinCollection', { collectionId });
		await invoke<void>('pro_join_collection', { collectionId });
	}

	/**
	 * List the collections the signed-in user could join but isn't a member of yet
	 * (open + restricted) — collection discovery (browse & join). The daemon forwards
	 * the caller's JWT, so the cloud RPC filters to what the caller can see and
	 * excludes their own memberships server-side.
	 */
	async listJoinable(): Promise<JoinableCollection[]> {
		log.debug('listJoinable');
		const rows = await invoke<RawJoinableCollection[]>('pro_list_joinable_collections');
		return rows.map((r) => ({ id: r.id, name: r.name, restricted: r.restricted }));
	}

	/**
	 * Approve a pending join request (admin only). `permission` of `undefined`
	 * grants the originally-requested tier.
	 */
	async approveRequest(requestId: string, permission?: Permission): Promise<void> {
		log.debug('approveRequest', { requestId, permission });
		await invoke<void>('pro_approve_request', { requestId, permission });
	}

	/** List a collection's pending invites (admin only, server-gated). */
	async listInvites(collectionId: string): Promise<Invite[]> {
		log.debug('listInvites', { collectionId });
		const rows = await invoke<RawInvite[]>('pro_list_invites', { collectionId });
		return rows.map((r) => ({
			id: r.id,
			code: r.code,
			email: r.email,
			permission: r.permission as Permission,
			expiresAt: r.expires_at
		}));
	}

	/** List a collection's pending join requests (admin only, server-gated). */
	async listRequests(collectionId: string): Promise<JoinRequest[]> {
		log.debug('listRequests', { collectionId });
		const rows = await invoke<RawRequest[]>('pro_list_requests', { collectionId });
		return rows.map((r) => ({ id: r.id, requestedBy: r.requested_by, createdAt: r.created_at }));
	}

	/** Revoke a pending invite OR join request by id (admin only, server-gated). */
	async revokeInvite(inviteId: string): Promise<void> {
		log.debug('revokeInvite', { inviteId });
		await invoke<void>('pro_revoke_invite', { inviteId });
	}

	/**
	 * The caller's own identity (bound PersonNode id + email), so the UI can tell
	 * which roster row is "me" and gate admin controls on the caller's own
	 * per-collection role. `personId` is `''` on an un-bound device.
	 */
	async currentPerson(): Promise<Person> {
		const p = await invoke<RawPerson>('pro_current_person');
		return { personId: p.person_id, email: p.email };
	}

	/**
	 * The active workspace's roster, pending admissions included. Readable by
	 * any active member; the Rust command rewrites daemon errors into
	 * actionable sentences, so callers can surface the rejection message as-is.
	 */
	async listTenantMembers(): Promise<TenantMember[]> {
		const rows = await invoke<RawTenantMember[]>('pro_list_tenant_members');
		return rows.map((r) => ({
			personId: r.person_id,
			email: r.email,
			role: r.role,
			status: r.status
		}));
	}

	/** Approve a pending admission (owner / tenant admin only, server-gated). */
	async approveAdmission(personId: string): Promise<void> {
		log.debug('approveAdmission', { personId });
		await invoke<void>('pro_approve_admission', { personId });
	}

	/**
	 * Invite someone to the active workspace by email. Creates a pending
	 * admission that an owner or tenant admin then approves; the invitee must
	 * already have a NodeSpace Pro account.
	 */
	async initiateAdmission(email: string): Promise<AdmissionResult> {
		log.debug('initiateAdmission');
		const r = await invoke<RawAdmission>('pro_initiate_admission', { email });
		return { outcome: r.outcome, personId: r.person_id, status: r.status };
	}
}

/** Shared singleton — import this, not the class. */
export const membershipService = new MembershipService();
