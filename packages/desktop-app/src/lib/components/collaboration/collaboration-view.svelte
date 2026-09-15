<!--
  CollaborationView — per-collection membership management. Shows the roster
  (people + role) and, for admins, controls to change a
  member's role, remove them (last-admin protected server-side), and add someone
  already in the workspace. Self-service "Leave" for the caller. Non-admins see a
  read-only roster.

  Pro-only: rendered only when proSync.tier === 'pro' (the host tab is gated too),
  so the community build never mounts this. Invites & join requests (the primary
  onboarding paths) live in the sibling InvitesPanel (S4).
-->
<script lang="ts">
	import { onMount } from 'svelte';
	import { membership } from '$lib/stores/membership.svelte';
	import type { Permission } from '$lib/services/membership-service';
	import { proSync } from '$lib/stores/pro-sync.svelte';
	import { createLogger } from '$lib/utils/logger';
	import { toError } from '$lib/types/errors';

	const log = createLogger('CollaborationView');

	let { collectionId }: { collectionId: string } = $props();

	// readOnly → modify → admin, in ascending-privilege order for the role select.
	const PERMISSIONS: Permission[] = ['readOnly', 'modify', 'admin'];
	const ROLE_LABEL: Record<Permission, string> = {
		readOnly: 'Viewer',
		modify: 'Editor',
		admin: 'Admin'
	};

	let actionError = $state<string | null>(null);
	let busyPerson = $state<string | null>(null);
	let pendingRemove = $state<string | null>(null);
	// Add-existing-person form. A raw person-id input for now — a proper
	// autocomplete picker over locally-synced person nodes is a follow-up once a
	// "list persons" source exists; invites (S4) are the primary way to bring in
	// people who aren't already in the workspace.
	let addId = $state('');
	let addRole = $state<Permission>('readOnly');
	let adding = $state(false);

	// --- Invites & requests, admin-only ---
	const TTL_PRESETS: { label: string; secs?: number }[] = [
		{ label: '1 day', secs: 86400 },
		{ label: '7 days', secs: 604800 },
		{ label: 'Never', secs: undefined }
	];
	let inviteRole = $state<Permission>('readOnly');
	let inviteEmail = $state('');
	let inviteTtlIdx = $state(1); // default: 7 days
	let creatingInvite = $state(false);
	let mintedCode = $state<string | null>(null);
	let busyInvite = $state<string | null>(null);
	let approveRole = $state<Record<string, Permission>>({});
	let busyRequest = $state<string | null>(null);

	// Load the roster on mount if Pro is already confirmed (the host tab is Pro-gated, so
	// this is the common path). If tier confirms after mount, load then - via the store's
	// transition hook, not a reactive $effect that watches isPro (ADR-049).
	onMount(() => {
		if (!collectionId) return;
		if (proSync.isPro) {
			membership.loadCollection(collectionId);
			return;
		}
		return proSync.onProConfirmed(() => membership.loadCollection(collectionId));
	});

	let view = $derived(membership.get(collectionId));
	let myRole = $derived(membership.currentUserRole(collectionId));
	let amAdmin = $derived(myRole === 'admin');
	let myId = $derived(membership.currentPerson?.personId ?? '');

	function friendly(e: unknown): string {
		const s = toError(e).message;
		// Surface the last-admin protection clearly (daemon FAILED_PRECONDITION).
		if (/last[ _]?admin|only admin|last remaining admin/i.test(s)) {
			return 'You can’t remove or demote the last admin of this collection.';
		}
		return s.replace(/^Error:\s*/, '');
	}

	async function changeRole(personId: string, permission: Permission) {
		actionError = null;
		busyPerson = personId;
		try {
			await membership.setMember(collectionId, personId, permission);
		} catch (e) {
			log.warn('changeRole failed', { personId, error: e });
			actionError = friendly(e);
		} finally {
			busyPerson = null;
		}
	}

	async function confirmRemove(personId: string) {
		actionError = null;
		busyPerson = personId;
		try {
			await membership.removeMember(collectionId, personId);
			pendingRemove = null;
		} catch (e) {
			log.warn('removeMember failed', { personId, error: e });
			actionError = friendly(e);
		} finally {
			busyPerson = null;
		}
	}

	async function leave() {
		actionError = null;
		busyPerson = myId;
		try {
			await membership.leaveCollection(collectionId);
		} catch (e) {
			log.warn('leaveCollection failed', { error: e });
			actionError = friendly(e);
		} finally {
			busyPerson = null;
		}
	}

	async function addExisting() {
		const id = addId.trim();
		if (!id) return;
		actionError = null;
		adding = true;
		try {
			await membership.setMember(collectionId, id, addRole);
			addId = '';
		} catch (e) {
			log.warn('addExisting failed', { error: e });
			actionError = friendly(e);
		} finally {
			adding = false;
		}
	}

	async function createInvite() {
		actionError = null;
		creatingInvite = true;
		mintedCode = null;
		try {
			const email = inviteEmail.trim() || undefined;
			const code = await membership.createInvite(
				collectionId,
				inviteRole,
				email,
				TTL_PRESETS[inviteTtlIdx].secs
			);
			// A bearer (no-email) invite yields a share code to copy; an email-bound
			// invite is delivered to that address, so don't surface a code for it.
			if (!email) mintedCode = code;
			inviteEmail = '';
		} catch (e) {
			log.warn('createInvite failed', { error: e });
			actionError = friendly(e);
		} finally {
			creatingInvite = false;
		}
	}

	async function copyCode(code: string) {
		try {
			if (typeof window !== 'undefined' && window.navigator?.clipboard) {
				await window.navigator.clipboard.writeText(code);
			}
		} catch (e) {
			log.warn('clipboard write failed', { error: e });
		}
	}

	async function revoke(inviteId: string) {
		actionError = null;
		busyInvite = inviteId;
		try {
			await membership.revokeInvite(collectionId, inviteId);
		} catch (e) {
			log.warn('revokeInvite failed', { error: e });
			actionError = friendly(e);
		} finally {
			busyInvite = null;
		}
	}

	async function approve(requestId: string) {
		actionError = null;
		busyRequest = requestId;
		try {
			await membership.approveRequest(collectionId, requestId, approveRole[requestId] ?? 'readOnly');
		} catch (e) {
			log.warn('approveRequest failed', { error: e });
			actionError = friendly(e);
		} finally {
			busyRequest = null;
		}
	}

	async function reject(requestId: string) {
		actionError = null;
		busyRequest = requestId;
		try {
			await membership.rejectRequest(collectionId, requestId);
		} catch (e) {
			log.warn('rejectRequest failed', { error: e });
			actionError = friendly(e);
		} finally {
			busyRequest = null;
		}
	}

	function expiryLabel(iso: string): string {
		if (!iso) return 'never expires';
		const d = new Date(iso);
		return isNaN(d.getTime()) ? iso : `expires ${d.toLocaleDateString()}`;
	}
</script>

{#if proSync.isPro}
	<div class="collaboration">
		{#if view.loading && view.members.length === 0}
			<div class="state-row">Loading members…</div>
		{:else if view.error && view.members.length === 0}
			<div class="state-row error">{friendly(view.error)}</div>
		{:else}
			<div class="section-head">
				<h2>Members</h2>
				<span class="count">{view.members.length}</span>
			</div>

			{#if actionError}
				<div class="state-row error" role="alert">{actionError}</div>
			{/if}

			<ul class="roster">
				{#each view.members as m (m.personId)}
					{@const isSelf = m.personId === myId}
					<li class="member">
						<span class="member-name" title={m.personId}>
							{membership.displayFor(m.personId)}{#if isSelf}<span class="you"> (you)</span>{/if}
						</span>

						{#if amAdmin && !isSelf}
							<select
								class="role-select"
								value={m.permission}
								disabled={busyPerson === m.personId}
								onchange={(e) => changeRole(m.personId, e.currentTarget.value as Permission)}
								aria-label="Role for {m.personId}"
							>
								{#each PERMISSIONS as p}
									<option value={p}>{ROLE_LABEL[p]}</option>
								{/each}
							</select>

							{#if pendingRemove === m.personId}
								<button
									class="btn btn-danger"
									disabled={busyPerson === m.personId}
									onclick={() => confirmRemove(m.personId)}>Confirm remove</button
								>
								<button class="btn btn-ghost" onclick={() => (pendingRemove = null)}>Cancel</button>
							{:else}
								<button class="btn btn-ghost" onclick={() => (pendingRemove = m.personId)}
									>Remove</button
								>
							{/if}
						{:else}
							<span class="role-badge" data-role={m.permission}>{ROLE_LABEL[m.permission]}</span>
							{#if isSelf}
								<button class="btn btn-ghost" disabled={busyPerson === myId} onclick={leave}
									>Leave</button
								>
							{/if}
						{/if}
					</li>
				{/each}
			</ul>

			{#if amAdmin}
				<div class="add-existing">
					<h3>Add someone already in the workspace</h3>
					<div class="add-row">
						<input
							class="add-input"
							type="text"
							placeholder="person node id"
							bind:value={addId}
							disabled={adding}
						/>
						<select class="role-select" bind:value={addRole} disabled={adding} aria-label="Role">
							{#each PERMISSIONS as p}
								<option value={p}>{ROLE_LABEL[p]}</option>
							{/each}
						</select>
						<button class="btn btn-primary" disabled={adding || !addId.trim()} onclick={addExisting}>
							{adding ? 'Adding…' : 'Add'}
						</button>
					</div>
					<p class="hint">To bring in someone new, use <strong>Invites</strong> below.</p>
				</div>

				<div class="invites">
					<div class="section-head">
						<h3>Invites</h3>
					</div>
					<div class="add-row">
						<select class="role-select" bind:value={inviteRole} aria-label="Invite role">
							{#each PERMISSIONS as p}
								<option value={p}>{ROLE_LABEL[p]}</option>
							{/each}
						</select>
						<input
							class="add-input"
							type="email"
							placeholder="email (optional)"
							bind:value={inviteEmail}
							disabled={creatingInvite}
						/>
						<select class="role-select" bind:value={inviteTtlIdx} aria-label="Invite expiry">
							{#each TTL_PRESETS as t, i}
								<option value={i}>{t.label}</option>
							{/each}
						</select>
						<button class="btn btn-primary" disabled={creatingInvite} onclick={createInvite}>
							{creatingInvite ? 'Creating…' : 'Create invite'}
						</button>
					</div>

					{#if mintedCode}
						<div class="minted" role="status">
							<code>{mintedCode}</code>
							<button class="btn btn-ghost" onclick={() => mintedCode && copyCode(mintedCode)}
								>Copy code</button
							>
						</div>
					{/if}

					{#if view.invites.length > 0}
						<ul class="roster">
							{#each view.invites as inv (inv.id)}
								<li class="member">
									<span class="member-name">{inv.email || '(share code)'}</span>
									<span class="role-badge" data-role={inv.permission}>{ROLE_LABEL[inv.permission]}</span
									>
									<span class="expiry">{expiryLabel(inv.expiresAt)}</span>
									{#if inv.code}
										<button class="btn btn-ghost" onclick={() => copyCode(inv.code)}>Copy code</button>
									{/if}
									<button
										class="btn btn-ghost"
										disabled={busyInvite === inv.id}
										onclick={() => revoke(inv.id)}>Revoke</button
									>
								</li>
							{/each}
						</ul>
					{:else}
						<p class="hint">No pending invites.</p>
					{/if}
				</div>

				<div class="requests">
					<div class="section-head">
						<h3>Join requests</h3>
						{#if view.requests.length > 0}<span class="count">{view.requests.length}</span>{/if}
					</div>
					{#if view.requests.length > 0}
						<ul class="roster">
							{#each view.requests as req (req.id)}
								<li class="member">
									<span class="member-name" title={req.requestedBy}
										>{membership.displayFor(req.requestedBy)}</span
									>
									<select
										class="role-select"
										value={approveRole[req.id] ?? 'readOnly'}
										onchange={(e) => (approveRole[req.id] = e.currentTarget.value as Permission)}
										aria-label="Approve role for {req.requestedBy}"
									>
										{#each PERMISSIONS as p}
											<option value={p}>{ROLE_LABEL[p]}</option>
										{/each}
									</select>
									<button
										class="btn btn-primary"
										disabled={busyRequest === req.id}
										onclick={() => approve(req.id)}>Approve</button
									>
									<button
										class="btn btn-ghost"
										disabled={busyRequest === req.id}
										onclick={() => reject(req.id)}>Reject</button
									>
								</li>
							{/each}
						</ul>
					{:else}
						<p class="hint">No pending requests.</p>
					{/if}
				</div>
			{/if}
		{/if}
	</div>
{/if}

<style>
	.collaboration {
		padding: 1.25rem 2rem;
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
	}
	.section-head {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}
	.section-head h2 {
		font-size: 1rem;
		font-weight: 600;
		margin: 0;
		color: hsl(var(--foreground));
	}
	.count {
		font-size: 0.8rem;
		color: hsl(var(--muted-foreground));
		padding: 0.1rem 0.5rem;
		background: hsl(var(--muted));
		border-radius: 9999px;
	}
	.state-row {
		font-size: 0.875rem;
		color: hsl(var(--muted-foreground));
	}
	.state-row.error {
		color: hsl(var(--destructive));
	}
	.roster {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}
	.member {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.4rem 0.5rem;
		border-radius: 6px;
	}
	.member:hover {
		background: hsl(var(--muted) / 0.5);
	}
	.member-name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: hsl(var(--foreground));
		font-size: 0.9rem;
	}
	.you {
		color: hsl(var(--muted-foreground));
	}
	.role-badge {
		font-size: 0.75rem;
		padding: 0.1rem 0.5rem;
		border-radius: 9999px;
		background: hsl(var(--muted));
		color: hsl(var(--muted-foreground));
	}
	.role-badge[data-role='admin'] {
		background: hsl(var(--primary) / 0.15);
		color: hsl(var(--primary));
	}
	.role-select,
	.add-input {
		font-size: 0.85rem;
		padding: 0.25rem 0.4rem;
		border: 1px solid hsl(var(--border));
		border-radius: 6px;
		background: hsl(var(--background));
		color: hsl(var(--foreground));
	}
	.add-input {
		flex: 1;
		min-width: 0;
	}
	.btn {
		font-size: 0.8rem;
		padding: 0.25rem 0.6rem;
		border-radius: 6px;
		border: 1px solid transparent;
		cursor: pointer;
	}
	.btn:disabled {
		opacity: 0.6;
		cursor: default;
	}
	.btn-ghost {
		background: transparent;
		border-color: hsl(var(--border));
		color: hsl(var(--foreground));
	}
	.btn-danger {
		background: hsl(var(--destructive));
		color: #fff;
	}
	.btn-primary {
		background: hsl(var(--primary));
		color: hsl(var(--primary-foreground));
	}
	.add-existing {
		margin-top: 0.75rem;
		border-top: 1px solid hsl(var(--border));
		padding-top: 0.75rem;
	}
	.add-existing h3 {
		font-size: 0.85rem;
		font-weight: 600;
		margin: 0 0 0.5rem;
		color: hsl(var(--foreground));
	}
	.add-row {
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}
	.hint {
		font-size: 0.78rem;
		color: hsl(var(--muted-foreground));
		margin: 0.4rem 0 0;
	}
	.invites,
	.requests {
		margin-top: 0.75rem;
		border-top: 1px solid hsl(var(--border));
		padding-top: 0.75rem;
	}
	.section-head h3 {
		font-size: 0.85rem;
		font-weight: 600;
		margin: 0;
		color: hsl(var(--foreground));
	}
	.minted {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		margin: 0.5rem 0;
	}
	.minted code {
		font-family: var(--font-mono, monospace);
		font-size: 0.8rem;
		background: hsl(var(--muted));
		padding: 0.2rem 0.45rem;
		border-radius: 6px;
		overflow-wrap: anywhere;
	}
	.expiry {
		font-size: 0.75rem;
		color: hsl(var(--muted-foreground));
	}
</style>
