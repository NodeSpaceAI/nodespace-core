<!--
  Workspace members — pending-admission approvals and invite-by-email for the
  workspace (tenant) the daemon is currently syncing.

  Someone who signs in to a workspace with admission enforcement lands
  `pending` and gets nothing until an owner or tenant admin approves them.
  This card is where that approval happens, and where an admin invites a new
  person by email (which creates another pending admission to approve).

  Visibility: rendered ONLY for an active owner / tenant admin. The caller's
  role comes from their own row in the same roster the pending list is drawn
  from (matched on the daemon's bound person id), so one `ListTenantMembers`
  call answers both "who is waiting" and "may I see this". Everyone else —
  a plain member, a pending user, an un-bound device, a local-only database,
  or a roster that failed to load (role unknown) — sees nothing at all. The
  cloud enforces the same gate server-side; this only keeps the controls from
  being offered to someone whose click would be refused.

  The parent (`account-settings.svelte`) mounts this only for a signed-in Pro
  user with the Labs "Team synchronization" flag on, and re-keys it on the
  active database, since `ListTenantMembers` is scoped to whichever tenant
  the daemon is syncing.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { membershipService, isTenantAdmin, type TenantMember } from '$lib/services/membership-service';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Card, CardHeader, CardContent } from '$lib/components/ui/card';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';
  import { databaseStore } from '$lib/stores/database.svelte';

  const log = createLogger('WorkspaceMembersCard');

  let members = $state<TenantMember[]>([]);
  let isAdmin = $state(false);
  /** Error from re-loading the roster after the admin view is already showing. */
  let listError = $state('');

  /** The caller's own person id — never offered "Remove" on their own row. */
  let myPersonId = $state('');

  const pending = $derived(members.filter((m) => m.status === 'pending'));
  /**
   * Active members the caller may remove: everyone except the owner (the cloud
   * refuses to remove the owner) and the caller themselves (leaving is a
   * separate flow, and a self-removed admin would lock themselves out).
   */
  const removable = $derived(
    members.filter((m) => m.status === 'active' && m.role !== 'owner' && m.personId !== myPersonId)
  );

  /**
   * Per-row remove/decline state, keyed by person id. `confirming` is the
   * inline confirmation step: the first click only arms it, the second acts.
   */
  let confirming = $state<Record<string, boolean>>({});
  let removing = $state<Record<string, boolean>>({});
  let removeErrors = $state<Record<string, string>>({});

  /** Per-row approve state, keyed by person id. */
  let approving = $state<Record<string, boolean>>({});
  let approveErrors = $state<Record<string, string>>({});

  let inviteEmail = $state('');
  let inviting = $state(false);
  let inviteError = $state('');
  let inviteNotice = $state('');

  // Deliberately loose: the cloud resolves the address and reports a clean
  // "no account" error; this only keeps an obviously incomplete entry from
  // costing a round-trip.
  const inviteReady = $derived(/^[^\s@]+@[^\s@]+$/.test(inviteEmail.trim()));

  /** Bumped per `load()`; a result from a superseded load is dropped. */
  let loadSeq = 0;

  async function load(): Promise<void> {
    const seq = ++loadSeq;
    try {
      // The roster is scoped to whichever tenant the daemon is syncing, and a
      // database switch re-targets it asynchronously — wait for that to land
      // so this never shows (or acts on) the previous database's tenant.
      await databaseStore.proSyncSettled();
      const [me, roster] = await Promise.all([
        membershipService.currentPerson(),
        membershipService.listTenantMembers()
      ]);
      if (seq !== loadSeq) return;
      const mine = me.personId ? roster.find((m) => m.personId === me.personId) : undefined;
      members = roster;
      myPersonId = me.personId;
      isAdmin = isTenantAdmin(mine);
      listError = '';
    } catch (err) {
      if (seq !== loadSeq) return;
      const message = toError(err).message;
      log.warn('workspace roster load failed', { error: message });
      // Before the first successful load the caller's role is unknown, so the
      // card stays hidden (a non-admin must never see it). Once an admin view
      // is showing, a failed refresh surfaces the error instead of vanishing.
      if (isAdmin) listError = message;
    }
  }

  // onMount, not $effect: `load` reads `isAdmin`, and an effect would re-run
  // (a second roster fetch) the moment the first load flips it. The parent
  // re-keys this component when the active database changes.
  onMount(() => {
    void load();
  });

  async function approve(member: TenantMember): Promise<void> {
    if (approving[member.personId]) return;
    approving[member.personId] = true;
    delete approveErrors[member.personId];
    try {
      await membershipService.approveAdmission(member.personId);
      log.info('admission approved');
      await load();
    } catch (err) {
      approveErrors[member.personId] = toError(err).message;
    } finally {
      delete approving[member.personId];
    }
  }

  /** Decline a pending admission or remove an active member (same cloud call). */
  async function remove(member: TenantMember): Promise<void> {
    if (removing[member.personId]) return;
    removing[member.personId] = true;
    delete removeErrors[member.personId];
    try {
      await membershipService.removeFromTenant(member.personId);
      log.info('member removed from workspace', { status: member.status });
      delete confirming[member.personId];
      await load();
    } catch (err) {
      removeErrors[member.personId] = toError(err).message;
      delete confirming[member.personId];
    } finally {
      delete removing[member.personId];
    }
  }

  async function invite(event: Event): Promise<void> {
    event.preventDefault();
    const email = inviteEmail.trim();
    if (!inviteReady || inviting) return;
    inviting = true;
    inviteError = '';
    inviteNotice = '';
    try {
      const result = await membershipService.initiateAdmission(email);
      inviteNotice = inviteOutcomeMessage(email, result.outcome, result.status);
      inviteEmail = '';
      await load();
    } catch (err) {
      inviteError = toError(err).message;
    } finally {
      inviting = false;
    }
  }

  function inviteOutcomeMessage(email: string, outcome: string, status: string): string {
    if (outcome === 'initiated') {
      return `Invited ${email}. They're now waiting for approval below — approve them to give access.`;
    }
    switch (status) {
      case 'pending':
        return `${email} is already waiting for approval below.`;
      case 'active':
        return `${email} is already a member of this workspace.`;
      default:
        return `${email} already has a ${status || 'non-active'} membership in this workspace, so no new invite was created.`;
    }
  }

  function displayName(member: TenantMember): string {
    return member.email || `Unknown email (${member.personId.slice(0, 8)})`;
  }
</script>

{#snippet removeControls(member: TenantMember, verb: 'Decline' | 'Remove')}
  {#if confirming[member.personId]}
    <span class="flex items-center gap-2">
      <span class="text-muted-foreground text-sm">{verb} {displayName(member)}?</span>
      <Button
        size="sm"
        variant="destructive"
        disabled={removing[member.personId]}
        onclick={() => void remove(member)}
        aria-label={`Confirm ${verb.toLowerCase()} ${displayName(member)}`}
      >
        {removing[member.personId] ? `${verb === 'Decline' ? 'Declining' : 'Removing'}…` : 'Confirm'}
      </Button>
      <Button
        size="sm"
        variant="ghost"
        disabled={removing[member.personId]}
        onclick={() => delete confirming[member.personId]}
        aria-label={`Cancel ${verb.toLowerCase()} ${displayName(member)}`}
      >
        Cancel
      </Button>
    </span>
  {:else}
    <Button
      size="sm"
      variant="outline"
      onclick={() => (confirming[member.personId] = true)}
      aria-label={`${verb} ${displayName(member)}`}
    >
      {verb}
    </Button>
  {/if}
{/snippet}

{#if isAdmin}
  <Card class="mb-4 gap-0 rounded-lg py-0" data-testid="workspace-members-card">
    <CardHeader class="p-5 pb-3">
      <span class="text-foreground mb-1.5 text-[0.9375rem] font-semibold">Workspace members</span>
      <p class="text-muted-foreground m-0 text-sm leading-relaxed">
        People who join this workspace wait here until an owner or admin approves them.
      </p>
    </CardHeader>

    <CardContent class="px-5 pb-4">
      <h3 class="text-foreground mb-2 text-sm font-medium">Waiting for approval</h3>
      {#if listError}
        <p class="text-destructive mb-2 text-sm" role="alert">
          {listError}
          <button
            type="button"
            class="text-primary ml-1 cursor-pointer bg-transparent p-0 underline underline-offset-2"
            onclick={() => void load()}
          >
            Retry
          </button>
        </p>
      {/if}
      {#if pending.length === 0}
        <p class="text-muted-foreground m-0 text-sm">No one is waiting for approval.</p>
      {:else}
        <ul class="m-0 flex list-none flex-col gap-2 p-0">
          {#each pending as member (member.personId)}
            <li class="flex flex-col gap-1" data-testid="pending-admission">
              <div class="flex items-center justify-between gap-3">
                <span class="text-foreground truncate text-sm">{displayName(member)}</span>
                <span class="flex shrink-0 items-center gap-2">
                  {@render removeControls(member, 'Decline')}
                  {#if !confirming[member.personId]}
                    <Button
                      size="sm"
                      disabled={approving[member.personId] || removing[member.personId]}
                      onclick={() => void approve(member)}
                      aria-label={`Approve ${displayName(member)}`}
                    >
                      {approving[member.personId] ? 'Approving…' : 'Approve'}
                    </Button>
                  {/if}
                </span>
              </div>
              {#if approveErrors[member.personId]}
                <p class="text-destructive m-0 text-sm" role="alert">
                  {approveErrors[member.personId]}
                </p>
              {/if}
              {#if removeErrors[member.personId]}
                <p class="text-destructive m-0 text-sm" role="alert">
                  {removeErrors[member.personId]}
                </p>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </CardContent>

    {#if removable.length > 0}
      <CardContent class="border-border border-t px-5 pt-4 pb-4">
        <h3 class="text-foreground mb-2 text-sm font-medium">Members</h3>
        <ul class="m-0 flex list-none flex-col gap-2 p-0">
          {#each removable as member (member.personId)}
            <li class="flex flex-col gap-1" data-testid="active-member">
              <div class="flex items-center justify-between gap-3">
                <span class="text-foreground truncate text-sm">{displayName(member)}</span>
                <span class="flex shrink-0 items-center gap-2">
                  {@render removeControls(member, 'Remove')}
                </span>
              </div>
              {#if removeErrors[member.personId]}
                <p class="text-destructive m-0 text-sm" role="alert">
                  {removeErrors[member.personId]}
                </p>
              {/if}
            </li>
          {/each}
        </ul>
      </CardContent>
    {/if}

    <CardContent class="border-border border-t px-5 pt-4 pb-5">
      <h3 class="text-foreground mb-1 text-sm font-medium">Invite by email</h3>
      <p class="text-muted-foreground mb-2 text-sm leading-relaxed">
        They need a NodeSpace Pro account. After you invite them, approve them above.
      </p>
      <form class="flex gap-2" onsubmit={invite}>
        <Input
          type="email"
          placeholder="name@example.com"
          aria-label="Invitee email"
          bind:value={inviteEmail}
          disabled={inviting}
        />
        <Button type="submit" size="sm" disabled={!inviteReady || inviting}>
          {inviting ? 'Inviting…' : 'Invite'}
        </Button>
      </form>
      {#if inviteError}
        <p class="text-destructive mt-2 mb-0 text-sm" role="alert">{inviteError}</p>
      {:else if inviteNotice}
        <p class="text-muted-foreground mt-2 mb-0 text-sm" role="status">{inviteNotice}</p>
      {/if}
    </CardContent>
  </Card>
{/if}
