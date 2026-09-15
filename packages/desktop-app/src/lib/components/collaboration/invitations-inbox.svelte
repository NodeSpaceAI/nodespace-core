<!--
  InvitationsInbox — onboarding entry point. A modal
  opened from the Pro account menu (Invitations), and auto-shown for a signed-in
  user who has no collection access yet. Lets a user redeem a share-code invite,
  browse the collections they can join, and join an open one directly / request a
  restricted one.

  Email-bound invites are auto-redeemed server-side on reconnect
  (`redeem_my_invites`), so they need no action here — the copy just says so.
  The discovery list comes from `list_joinable_collections` (memberships excluded
  and visibility filtered server-side). Listing "my pending invites / my request
  statuses" still has no client-facing backend (only admins can list a
  collection's invites/requests); that remains deferred to a follow-up.
-->
<script lang="ts">
	import { focusTrap } from '$lib/actions/focus-trap';
	import { membership } from '$lib/stores/membership.svelte';
	import { createLogger } from '$lib/utils/logger';
	import { toError } from '$lib/types/errors';

	const log = createLogger('InvitationsInbox');

	let {
		open = false,
		onClose,
		onLogout
	}: {
		open?: boolean;
		onClose: () => void;
		// When provided, a signed-in user with no code/membership can log out from
		// here — reverting to the free/signed-out state. Omitted callers get
		// the plain Close-only footer.
		onLogout?: () => void | Promise<void>;
	} = $props();

	let loggingOut = $state(false);

	async function logout() {
		if (!onLogout || loggingOut) return;
		loggingOut = true;
		try {
			await onLogout();
		} finally {
			loggingOut = false;
		}
	}

	let code = $state('');
	let redeeming = $state(false);
	let redeemMsg = $state<string | null>(null);
	let redeemErr = $state<string | null>(null);

	// Collection discovery (browse & join). The joinable list lives in the store
	// (loaded server-side, memberships excluded). `actingId` is the collection a
	// per-row Join/Request is in flight for, so only that row's buttons disable.
	let actingId = $state<string | null>(null);
	let browseMsg = $state<string | null>(null);
	let browseErr = $state<string | null>(null);

	// Load the joinable list the first time the modal is opened (Pro only). Reopening
	// after a join reflects the store's already-updated list without a refetch; the
	// explicit Refresh button re-queries on demand.
	let loadedOnce = false;
	$effect(() => {
		if (open && membership.isPro && !loadedOnce) {
			loadedOnce = true;
			void membership.loadJoinable();
		}
	});

	function friendly(e: unknown): string {
		return toError(e).message.replace(/^Error:\s*/, '');
	}

	async function redeem() {
		const c = code.trim();
		if (!c) return;
		redeeming = true;
		redeemMsg = null;
		redeemErr = null;
		try {
			await membership.acceptInvite(c);
			redeemMsg = 'You now have access — it will appear in your sidebar as it syncs.';
			code = '';
		} catch (e) {
			log.warn('redeem failed', { error: e });
			redeemErr = friendly(e);
		} finally {
			redeeming = false;
		}
	}

	async function join(id: string, name: string) {
		if (actingId) return;
		actingId = id;
		browseMsg = null;
		browseErr = null;
		try {
			await membership.joinCollection(id);
			browseMsg = `Joined ${name} — it will appear in your sidebar as it syncs.`;
		} catch (e) {
			log.warn('joinCollection failed', { id, error: e });
			browseErr = friendly(e);
		} finally {
			actingId = null;
		}
	}

	async function request(id: string, name: string) {
		if (actingId) return;
		actingId = id;
		browseMsg = null;
		browseErr = null;
		try {
			await membership.requestJoin(id);
			browseMsg = `Request sent for ${name} — an admin will review it.`;
		} catch (e) {
			log.warn('requestJoin failed', { id, error: e });
			browseErr = friendly(e);
		} finally {
			actingId = null;
		}
	}
</script>

{#if open}
	<div class="overlay" role="presentation" tabindex="-1" onclick={onClose}>
		<div
			class="modal"
			role="dialog"
			aria-modal="true"
			aria-labelledby="inv-title"
			use:focusTrap={{ onEscape: onClose }}
			onclick={(e) => e.stopPropagation()}
			onkeydown={(e) => e.stopPropagation()}
			tabindex="0"
		>
			<h2 id="inv-title">Invitations</h2>
			<p class="lead">
				Email invites apply automatically when you sign in. To join with a share code, or ask to
				join a restricted collection, use the options below.
			</p>

			<section>
				<h3>Redeem an invite code</h3>
				<div class="row">
					<input
						class="in"
						type="text"
						placeholder="paste invite code"
						bind:value={code}
						disabled={redeeming}
					/>
					<button class="btn btn-primary" disabled={redeeming || !code.trim()} onclick={redeem}>
						{redeeming ? 'Redeeming…' : 'Redeem'}
					</button>
				</div>
				{#if redeemMsg}<p class="ok" role="status">{redeemMsg}</p>{/if}
				{#if redeemErr}<p class="err" role="alert">{redeemErr}</p>{/if}
			</section>

			<section>
				<div class="head">
					<h3>Discover collections</h3>
					<button
						class="link"
						type="button"
						disabled={membership.joinableLoading || actingId !== null}
						onclick={() => membership.loadJoinable()}
					>
						{membership.joinableLoading ? 'Refreshing…' : 'Refresh'}
					</button>
				</div>
				<p class="hint">
					Open collections you can join directly; restricted ones need an admin's approval.
				</p>

				{#if membership.joinableLoading && membership.joinable.length === 0}
					<p class="muted">Loading…</p>
				{:else if membership.joinableError}
					<p class="err" role="alert">{friendly(membership.joinableError)}</p>
				{:else if membership.joinable.length === 0}
					<p class="muted">No collections available to join right now.</p>
				{:else}
					<ul class="list">
						{#each membership.joinable as c (c.id)}
							<li class="item">
								<span class="name" title={c.id}>{c.name || c.id}</span>
								{#if c.restricted}
									<span class="tag tag-restricted">Restricted</span>
									<button
										class="btn btn-ghost"
										disabled={actingId !== null}
										onclick={() => request(c.id, c.name || c.id)}
									>
										{actingId === c.id ? 'Sending…' : 'Request'}
									</button>
								{:else}
									<span class="tag tag-open">Open</span>
									<button
										class="btn btn-primary"
										disabled={actingId !== null}
										onclick={() => join(c.id, c.name || c.id)}
									>
										{actingId === c.id ? 'Joining…' : 'Join'}
									</button>
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
				{#if browseMsg}<p class="ok" role="status">{browseMsg}</p>{/if}
				{#if browseErr}<p class="err" role="alert">{browseErr}</p>{/if}
			</section>

			<div class="actions">
				{#if onLogout}
					<button
						class="btn btn-logout"
						type="button"
						disabled={loggingOut}
						onclick={logout}
					>
						{loggingOut ? 'Logging out…' : 'Log out'}
					</button>
				{/if}
				<button class="btn btn-ghost" onclick={onClose}>Close</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.overlay {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.5);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 1000;
	}
	.modal {
		background: var(--surface-1, #ffffff);
		color: var(--text-primary, #1f2937);
		border-radius: 8px;
		padding: 24px;
		width: min(460px, calc(100vw - 48px));
		box-shadow: 0 4px 16px rgba(0, 0, 0, 0.2);
	}
	h2 {
		margin: 0 0 4px;
		font-size: 1.1rem;
		font-weight: 600;
	}
	.lead {
		margin: 0 0 16px;
		font-size: 0.85rem;
		color: var(--text-secondary, #6b7280);
	}
	section {
		margin-bottom: 16px;
	}
	h3 {
		margin: 0 0 8px;
		font-size: 0.9rem;
		font-weight: 600;
	}
	.hint {
		margin: -2px 0 8px;
		font-size: 0.78rem;
		color: var(--text-secondary, #6b7280);
	}
	.head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 8px;
	}
	.link {
		background: none;
		border: none;
		padding: 0;
		font-size: 0.78rem;
		font-weight: 600;
		color: #2563eb;
		cursor: pointer;
	}
	.link:disabled {
		opacity: 0.6;
		cursor: default;
	}
	.muted {
		margin: 4px 0 0;
		font-size: 0.82rem;
		color: var(--text-secondary, #6b7280);
	}
	.list {
		list-style: none;
		margin: 4px 0 0;
		padding: 0;
		max-height: 220px;
		overflow-y: auto;
	}
	.item {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 6px 0;
		border-bottom: 1px solid var(--border-color, #eceef1);
	}
	.item:last-child {
		border-bottom: none;
	}
	.name {
		flex: 1;
		min-width: 0;
		font-size: 0.875rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.tag {
		flex: none;
		font-size: 0.7rem;
		font-weight: 600;
		padding: 2px 6px;
		border-radius: 4px;
	}
	.tag-restricted {
		background: #fef3c7;
		color: #92400e;
	}
	.tag-open {
		background: #dcfce7;
		color: #166534;
	}
	.row {
		display: flex;
		gap: 8px;
	}
	.in {
		flex: 1;
		min-width: 0;
		padding: 8px 10px;
		font-size: 0.875rem;
		border: 1px solid var(--border-color, #d1d5db);
		border-radius: 6px;
		background: var(--surface-1, #fff);
		color: var(--text-primary, #1f2937);
	}
	.btn {
		padding: 8px 14px;
		border: 1px solid transparent;
		border-radius: 6px;
		font-size: 0.85rem;
		font-weight: 600;
		cursor: pointer;
	}
	.btn:disabled {
		opacity: 0.6;
		cursor: default;
	}
	.btn-primary {
		background: #2563eb;
		color: #fff;
	}
	.btn-ghost {
		background: var(--surface-2, #e5e7eb);
		color: var(--text-primary, #1f2937);
	}
	.btn-logout {
		background: transparent;
		border-color: var(--border-color, #d1d5db);
		color: #dc2626;
	}
	.ok {
		margin: 8px 0 0;
		font-size: 0.8rem;
		color: #16a34a;
	}
	.err {
		margin: 8px 0 0;
		font-size: 0.8rem;
		color: #dc2626;
	}
	.actions {
		display: flex;
		justify-content: space-between;
		gap: 8px;
		margin-top: 8px;
	}
</style>
