<!--
  Add-synced-database-from-cloud flow (ADR-053 discovery-driven bind).

  Wizard: sign in (if needed) -> list the signed-in user's tenants -> pick one
  (or auto-select the only one) -> name + create a local database -> mint a
  private landing collection for it -> bind it to the tenant (activate-on-bind
  starts the cursor-0 catch-up automatically, server-side — nothing further to
  trigger from here).

  The collection step exists because `BindTenant` requires a non-empty
  collection for any non-empty schema (nodespace-sync's `bind_tenant` rejects
  schema-without-collection outright — it does not auto-mint one), and
  `ListTenantMemberships` returns only tenant/schema/status/role, no
  collection. A brand-new tenant member also has no existing collection to
  pick from yet: `ListJoinableCollections` is scoped to whichever tenant the
  daemon is CURRENTLY syncing, so it cannot discover one before the very bind
  that starts that session (chicken-and-egg). So this mints a fresh, private,
  RESTRICTED collection locally before binding — the client-side mirror of
  the daemon's own boot-time seed (`nodespaced-pro`'s
  `mint_restricted_landing_collection`, used when a fresh install is
  auto-seeded from `NODESPACED_PRO_SCHEMA`): once it syncs, the cloud
  `auto_admin_on_restrict` trigger makes the signed-in user its sole admin, so
  every node landing there is private to them by construction (ADR-037). No
  acceptance criterion here asks the user to pick a collection — this mirrors
  that: minting one is implicit, not a wizard step.

  Sign-in is a GLOBAL daemon concept (one OAuth session), independent of
  which database is bound to what — unlike `proSync`'s per-database-attributed
  getters (ADR-053: the store forces a synthetic 'local-only' for any database
  affirmatively known to be unbound, which would misreport "signed out" here
  if the app's currently-active database happens to be local-only while the
  daemon's OAuth session is genuinely live). So this dialog checks/awaits
  sign-in via `pro_current_person` (GetIdentity) and the raw `sync:status`
  event directly, rather than reading `proSync`.

  Retry-after-failure resumes rather than restarts: `confirmBind` remembers
  the database/collection it already created (`createdDatabaseId` /
  `mintedCollectionId`) and reuses them on a subsequent call instead of
  creating fresh ones — a bind-step failure (a flaky `BindTenant` call, say)
  must not leave behind a pile of orphaned local databases and "My Workspace"
  collections from every retry. The generic error step's Retry button routes
  to `confirmBind()` (resume) rather than `checkIdentity()` (full restart)
  whenever a tenant has already been picked — see `retry()`.

  Closing while a bind is in flight is blocked outright, not merely relabeled
  as a no-op cancel: `confirmBind`'s chain (create -> switchTo -> mint ->
  bind) has already committed to changing the user's active database by the
  time any of it can fail, and none of those steps are safely abortable
  mid-flight without risking a WORSE inconsistent state than just letting it
  finish. Every dismissal path is blocked while `step === 'binding'`: the X
  button is unrendered (`showCloseButton`) and Escape/outside-click are set
  to bits-ui's `escapeKeydownBehavior`/`interactOutsideBehavior: 'ignore'`.
  Note this can't be done by vetoing `Dialog.Root`'s `onOpenChange` (the
  approach this file used at first) — bits-ui's dialog state is a
  `$bindable` the primitive mutates directly on every dismiss interaction
  (`open = v; onOpenChange(v)`, in that order, per its own source), so by the
  time `onOpenChange` fires the primitive has already closed regardless of
  what the callback does; `escapeKeydownBehavior`/`interactOutsideBehavior`
  are bits-ui's actual extension points for stopping a dismissal before it
  starts, and are what this file uses instead — no cancellation token
  invented here, no precedent needed.
-->
<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { databaseStore } from '$lib/stores/database.svelte';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('AddSyncedDatabase');

  interface Props {
    open: boolean;
  }
  let { open = $bindable(false) }: Props = $props();

  interface TenantMembership {
    tenantId: string;
    schema: string;
    status: string;
    role: string;
  }

  interface TenantMembershipsResult {
    memberships: TenantMembership[];
    selection: 'no-workspace' | 'auto-select' | 'picker';
    autoSelected: TenantMembership | null;
  }

  type Step =
    | 'checking'
    | 'sign-in'
    | 'loading-tenants'
    | 'no-workspace'
    | 'pick-tenant'
    | 'name-database'
    | 'binding'
    | 'error';

  let step = $state<Step>('checking');
  let errorMessage = $state('');
  let tenants = $state<TenantMembership[]>([]);
  let selectedTenant = $state<TenantMembership | null>(null);
  let databaseName = $state('');
  let signingIn = $state(false);
  let unlistenStatus: UnlistenFn | null = null;
  /**
   * The database/collection `confirmBind` already created, if a previous
   * attempt got that far before failing. Non-null means a Retry must reuse
   * them rather than creating duplicates — see the file doc comment.
   */
  let createdDatabaseId = $state<string | null>(null);
  let mintedCollectionId = $state<string | null>(null);

  /** Friendly display name for a tenant schema, e.g. "tenant_demo" -> "Demo". */
  function tenantLabel(schema: string): string {
    const bare = schema.replace(/^tenant_/, '').replace(/_/g, ' ').trim();
    return bare.length ? bare.charAt(0).toUpperCase() + bare.slice(1) : schema;
  }

  function stopListeningForSignIn(): void {
    unlistenStatus?.();
    unlistenStatus = null;
  }

  /** GetIdentity — the global "is anyone signed in" check (see file doc). */
  async function checkIdentity(): Promise<void> {
    step = 'checking';
    errorMessage = '';
    try {
      const identity = await invoke<{ personId: string; email: string }>('pro_current_person');
      if (identity.email) {
        await loadTenants();
      } else {
        step = 'sign-in';
      }
    } catch (err) {
      step = 'error';
      errorMessage = err instanceof Error ? err.message : String(err);
    }
  }

  /** Kick off PKCE sign-in, mirroring pro-sync-pill's startSignIn. `provider`
   *  empty = the Worker email/password form; 'google' = direct GoTrue OAuth. */
  async function startSignIn(provider = ''): Promise<void> {
    if (signingIn) return;
    signingIn = true;
    errorMessage = '';
    try {
      await invoke('pro_initiate_oauth', provider ? { provider } : {});
    } catch (err) {
      signingIn = false;
      errorMessage = err instanceof Error ? err.message : String(err);
      return;
    }
    // The daemon's sync:status stream broadcasts globally, unscoped to any one
    // database's attribution — user_email on the raw event reflects the true
    // signed-in state the moment the PKCE flow completes.
    stopListeningForSignIn();
    unlistenStatus = await listen<{ user_email?: string }>('sync:status', (event) => {
      if (event.payload.user_email) {
        signingIn = false;
        stopListeningForSignIn();
        void loadTenants();
      }
    });
  }

  async function loadTenants(): Promise<void> {
    step = 'loading-tenants';
    errorMessage = '';
    try {
      const result = await invoke<TenantMembershipsResult>('pro_list_tenant_memberships');
      tenants = result.memberships.filter((t) => t.status === 'active');
      if (tenants.length === 0) {
        step = 'no-workspace';
      } else if (result.selection === 'auto-select' && result.autoSelected) {
        pickTenant(result.autoSelected);
      } else {
        step = 'pick-tenant';
      }
    } catch (err) {
      step = 'error';
      errorMessage = err instanceof Error ? err.message : String(err);
    }
  }

  function pickTenant(tenant: TenantMembership): void {
    selectedTenant = tenant;
    databaseName = tenantLabel(tenant.schema);
    step = 'name-database';
  }

  function backToPicker(): void {
    selectedTenant = null;
    // Abandoning the in-progress attempt for whichever tenant was picked —
    // a subsequent pick (of the same or a different tenant) must not silently
    // reuse a database/collection created for this abandoned one.
    createdDatabaseId = null;
    mintedCollectionId = null;
    step = tenants.length > 1 ? 'pick-tenant' : 'sign-in';
    if (tenants.length <= 1) void checkIdentity();
  }

  async function confirmBind(): Promise<void> {
    const tenant = selectedTenant;
    const name = databaseName.trim();
    if (!tenant || !name) return;
    step = 'binding';
    errorMessage = '';
    try {
      // Reuse a database/collection from a previous failed attempt (a Retry)
      // instead of creating fresh ones every time — see the file doc comment.
      let dbId = createdDatabaseId;
      if (!dbId) {
        const entry = await databaseStore.create(name);
        if (!entry) {
          throw new Error(databaseStore.error ?? 'Failed to create the local database');
        }
        dbId = entry.id;
        // Recorded immediately on success, before anything that can still
        // fail below — a later failure must see this database as already
        // created, not attempt to create a second one.
        createdDatabaseId = dbId;
      }

      // Switch onto the new (still local-only) database FIRST — `create_node`
      // below routes through the data-plane client's currently-active database,
      // so the landing collection must be minted after this, or it would land in
      // whichever database the app had open before this flow started. Cheap and
      // idempotent to repeat on a retry.
      await databaseStore.switchTo(dbId);

      // Mint this bind's private landing collection (see the file doc comment
      // for why this step exists at all) — reusing one from a previous attempt
      // rather than minting a second "My Workspace" collection on retry.
      let collectionId = mintedCollectionId;
      if (!collectionId) {
        collectionId = await backendAdapter.createNode({
          id: globalThis.crypto.randomUUID(),
          nodeType: 'collection',
          content: 'My Workspace',
          properties: { collection: { restrictedToMembers: true } }
        });
        mintedCollectionId = collectionId;
      }

      // BindTenant activates the sync session server-side (activate-on-bind,
      // including the cursor-0 catch-up) — the desktop is already showing this
      // database from the switchTo above, so no further re-point is needed.
      await invoke('pro_bind_tenant', {
        databaseId: dbId,
        schema: tenant.schema,
        collection: collectionId
      });
      close();
    } catch (err) {
      // The local database/collection (if created before the failure) are left
      // in place, never deleted here, and the desktop may already be showing the
      // database (the switchTo above) — `createdDatabaseId`/`mintedCollectionId`
      // stay set so a Retry reuses them (see `retry()`) instead of piling up a
      // fresh orphan on every failed attempt.
      log.error('Failed to bind the new database to the tenant', err);
      step = 'error';
      errorMessage = err instanceof Error ? err.message : String(err);
    }
  }

  /**
   * The generic error step's Retry action. A tenant already picked means the
   * failure happened during `confirmBind` (create/switch/mint/bind) — resume
   * that attempt (reusing `createdDatabaseId`/`mintedCollectionId` if set)
   * rather than restarting the whole wizard from the identity check, which
   * would otherwise re-run `databaseStore.create` and mint a second
   * collection on every retry.
   */
  function retry(): void {
    if (selectedTenant) {
      void confirmBind();
    } else {
      void checkIdentity();
    }
  }

  function close(): void {
    open = false;
  }

  function reset(): void {
    step = 'checking';
    errorMessage = '';
    tenants = [];
    selectedTenant = null;
    databaseName = '';
    signingIn = false;
    createdDatabaseId = null;
    mintedCollectionId = null;
    stopListeningForSignIn();
  }

  $effect(() => {
    if (open) {
      void checkIdentity();
    } else {
      reset();
    }
  });
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    class="sm:max-w-md"
    showCloseButton={step !== 'binding'}
    escapeKeydownBehavior={step === 'binding' ? 'ignore' : 'close'}
    interactOutsideBehavior={step === 'binding' ? 'ignore' : 'close'}
  >
    <Dialog.Header>
      <Dialog.Title>Add synced database</Dialog.Title>
      <Dialog.Description>
        Sign in, pick a workspace, and NodeSpace creates a new local database bound to it.
      </Dialog.Description>
    </Dialog.Header>

    {#if step === 'checking' || step === 'loading-tenants'}
      <p class="text-muted-foreground py-6 text-center text-sm">
        {step === 'checking' ? 'Checking sign-in status…' : 'Loading your workspaces…'}
      </p>
    {:else if step === 'sign-in'}
      <div class="flex flex-col gap-2 py-2">
        <p class="text-muted-foreground mb-1 text-sm">
          Sign in to NodeSpace Pro to see the workspaces you can sync a database to.
        </p>
        {#if errorMessage}
          <div
            class="border-destructive/40 bg-destructive/10 text-destructive rounded-[var(--radius)] border px-3 py-2 text-sm"
            role="alert"
          >
            {errorMessage}
          </div>
        {/if}
        <Button variant="default" disabled={signingIn} onclick={() => startSignIn('google')}>
          {signingIn ? 'Waiting for sign-in…' : 'Continue with Google'}
        </Button>
        <Button variant="outline" disabled={signingIn} onclick={() => startSignIn('')}>
          Sign in with email
        </Button>
      </div>
    {:else if step === 'no-workspace'}
      <div class="py-4">
        <p class="text-muted-foreground text-sm leading-relaxed">
          You're signed in, but you don't belong to any active workspace yet. Ask a workspace
          admin to invite you, then try again.
        </p>
      </div>
      <Dialog.Footer>
        <Button variant="outline" onclick={() => void checkIdentity()}>Retry</Button>
        <Button variant="ghost" onclick={close}>Close</Button>
      </Dialog.Footer>
    {:else if step === 'pick-tenant'}
      <div class="flex flex-col gap-2 py-2">
        <p class="text-muted-foreground mb-1 text-sm">Choose a workspace to sync a database to:</p>
        {#each tenants as tenant (tenant.tenantId)}
          <button
            type="button"
            class="border-border bg-muted/40 hover:bg-muted flex items-center justify-between gap-3 rounded-[var(--radius)] border p-3 text-left transition-colors"
            onclick={() => pickTenant(tenant)}
          >
            <span class="min-w-0">
              <span class="text-foreground block truncate font-medium">
                {tenantLabel(tenant.schema)}
              </span>
              <span class="text-muted-foreground block truncate font-mono text-xs">
                {tenant.schema}
              </span>
            </span>
            {#if tenant.role}
              <span class="text-muted-foreground bg-muted shrink-0 rounded px-1.5 py-0.5 text-xs">
                {tenant.role}
              </span>
            {/if}
          </button>
        {/each}
      </div>
    {:else if step === 'name-database'}
      <div class="flex flex-col gap-2 py-2">
        <p class="text-muted-foreground mb-1 text-sm">
          Syncing to <span class="text-foreground font-medium"
            >{selectedTenant ? tenantLabel(selectedTenant.schema) : ''}</span
          >. Name the new local database:
        </p>
        <Input
          bind:value={databaseName}
          placeholder="e.g. Work"
          onkeydown={(e: KeyboardEvent) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              void confirmBind();
            }
          }}
        />
      </div>
      <Dialog.Footer>
        {#if tenants.length > 1}
          <Button variant="ghost" onclick={backToPicker}>Back</Button>
        {/if}
        <Button variant="default" disabled={!databaseName.trim()} onclick={confirmBind}>
          Create &amp; sync
        </Button>
      </Dialog.Footer>
    {:else if step === 'binding'}
      <p class="text-muted-foreground py-6 text-center text-sm">
        Creating &amp; binding the database…
      </p>
    {:else if step === 'error'}
      <div class="py-2">
        <div
          class="border-destructive/40 bg-destructive/10 text-destructive rounded-[var(--radius)] border px-3 py-2 text-sm"
          role="alert"
        >
          {errorMessage}
        </div>
      </div>
      <Dialog.Footer>
        <Button variant="outline" onclick={retry}>Retry</Button>
        <Button variant="ghost" onclick={close}>Close</Button>
      </Dialog.Footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>
