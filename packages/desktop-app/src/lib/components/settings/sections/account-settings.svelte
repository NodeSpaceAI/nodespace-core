<!--
  Account settings.

  Owns the account-access affordances that used to be reachable ONLY through
  now-removed always-mounted app chrome: signing out and opening the
  Invitations inbox manually (formerly the top-right `pro-sync-pill`), and
  reopening the first-Pro publish-consent modal after a decline (formerly the
  top-right `enable-sync-pill`, which had no per-collection scoping —
  `collaboration-locked.svelte`'s own "Turn on sync" button only reaches a
  user who happens to open a collection's Collaboration tab). Deliberately
  minimal otherwise — this exists to not regress those capabilities, not to
  redesign account management.

  Sign-in itself is NOT duplicated here: it already has a full flow in
  Settings → Database → "Add synced database…" (`add-synced-database-dialog.svelte`,
  independent of this store's per-database getters — see that file's doc
  comment). A signed-out user is pointed there via `onNavigateToDatabase`.

  Not reproduced here: the removed pill's auto-show-Invitations-for-a-
  signed-in-user-with-no-collection-access nudge. That onboarding cue was
  tied to the pill being always-mounted in the app chrome; it doesn't map
  onto a settings page a user only visits deliberately, and it isn't one of
  the pill-only capabilities this section exists to preserve.
-->
<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { proSync } from '$lib/stores/pro-sync.svelte';
  import { labsFlags } from '$lib/stores/labs-flags.svelte';
  import { membership } from '$lib/stores/membership.svelte';
  import { resolveProSyncVariant } from '$lib/plugins/ui-extensions.svelte';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import { Card, CardHeader, CardContent } from '$lib/components/ui/card';
  import InvitationsInbox from '$lib/components/collaboration/invitations-inbox.svelte';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';

  const log = createLogger('AccountSettings');

  interface Props {
    /** Switches the Settings pane to the Database category, when provided. */
    onNavigateToDatabase?: () => void;
  }
  let { onNavigateToDatabase }: Props = $props();

  // Direct `proSync.isPro` read that bypasses the `resolveProSyncVariant()`
  // chokepoint (ADR-049) — this card must ALSO stay hidden behind the Labs
  // "Team synchronization" toggle (default OFF), a pure client-side
  // visibility flag; it does not touch `proSync.isPro`/tier-detection itself,
  // and community builds are unaffected either way (`proSync.isPro` is always
  // `false` there regardless of the flag).
  const syncUiEnabled = $derived(proSync.isPro && labsFlags.syncEnabled);

  // Sign-in is a GLOBAL daemon concept (one OAuth session), independent of
  // which database is bound to what — unlike `proSync`'s per-database-
  // attributed `userEmail` getter (ADR-053: the store forces a synthetic
  // 'local-only' entry, with an empty userEmail, for any database
  // affirmatively known to be unbound). Reading `proSync.userEmail` here
  // would misreport "signed out" — and hide Sign out / Invitations — for a
  // genuinely signed-in user whose currently-active database just happens to
  // be local-only. `add-synced-database-dialog.svelte` hits this exact trap
  // and documents it; this section avoids it the same way, by checking the
  // daemon's global identity (`pro_current_person`) directly instead.
  let identityEmail = $state('');
  let identityLoading = $state(false);
  const signedIn = $derived(identityEmail !== '');

  async function loadIdentity(): Promise<void> {
    if (!syncUiEnabled) return;
    identityLoading = true;
    try {
      const identity = await invoke<{ personId: string; email: string }>('pro_current_person');
      identityEmail = identity.email;
    } catch (err) {
      log.warn('pro_current_person invoke failed', { error: toError(err) });
      identityEmail = '';
    } finally {
      identityLoading = false;
    }
  }

  $effect(() => {
    void loadIdentity();
  });

  let signingOut = $state(false);
  let inboxOpen = $state(false);

  async function signOut() {
    if (signingOut) return;
    signingOut = true;
    try {
      await proSync.signOut();
      // Drop cached membership state so the next signed-in user doesn't
      // inherit this session's roster/identity (mirrors the removed pill's
      // sign-out handler — identity is per-session).
      membership.reset();
      identityEmail = '';
      log.info('signed out');
    } finally {
      signingOut = false;
      inboxOpen = false;
    }
  }
</script>

<div class="max-w-[640px]">
  <h2 class="text-foreground mb-1.5 text-xl font-semibold">Account</h2>
  <p class="text-muted-foreground mb-6 text-sm leading-relaxed">
    Your NodeSpace Pro sign-in — separate from the local identity in Database settings.
  </p>

  <Card class="mb-4 gap-0 rounded-lg py-0">
    <CardHeader class="p-5 pb-4">
      <div class="mb-1.5 flex items-center gap-2.5">
        <span class="text-foreground text-[0.9375rem] font-semibold">NodeSpace Pro</span>
        {#if !syncUiEnabled}
          <Badge variant="secondary">Not available</Badge>
        {:else if identityLoading}
          <Badge variant="secondary">Loading…</Badge>
        {:else if signedIn}
          <Badge class="border-green-500/25 bg-green-500/10 text-green-700">Signed in</Badge>
        {:else}
          <Badge variant="secondary">Signed out</Badge>
        {/if}
      </div>
      {#if !syncUiEnabled}
        <p class="text-muted-foreground m-0 text-sm leading-relaxed">
          Team synchronization is under heavy development. To help us test this capability,
          contact us at developer@nodespace.ai
        </p>
      {:else if identityLoading}
        <p class="text-muted-foreground m-0 text-sm leading-relaxed">Checking sign-in status…</p>
      {:else if signedIn}
        <p class="text-muted-foreground m-0 text-sm leading-relaxed">
          Signed in as <span class="text-foreground font-medium">{identityEmail}</span>.
        </p>
      {:else}
        <p class="text-muted-foreground m-0 text-sm leading-relaxed">
          Not signed in.
          {#if onNavigateToDatabase}
            <button
              type="button"
              class="text-primary cursor-pointer bg-transparent p-0 underline underline-offset-2"
              onclick={onNavigateToDatabase}
            >
              Sign in from Database settings
            </button>
          {:else}
            Sign in from Database settings
          {/if}
          → "Add synced database…".
        </p>
      {/if}
    </CardHeader>
    {#if syncUiEnabled && signedIn}
      {#if resolveProSyncVariant() === 'consent'}
        <CardContent class="px-5 pb-3">
          <p class="text-muted-foreground mb-2 text-sm leading-relaxed">
            Sync isn't turned on for this database yet — nothing has been shared.
          </p>
          <Button
            variant="outline"
            size="sm"
            onclick={() => (proSync.consentPromptOpen = true)}
          >
            Turn on sync
          </Button>
        </CardContent>
      {/if}
      <CardContent class="flex gap-2 px-5 pb-5">
        <Button
          variant="outline"
          size="sm"
          disabled={signingOut}
          onclick={() => (inboxOpen = true)}
        >
          Invitations
        </Button>
        <Button variant="outline" size="sm" disabled={signingOut} onclick={signOut}>
          {signingOut ? 'Signing out…' : 'Sign out'}
        </Button>
      </CardContent>
    {/if}
  </Card>
</div>

{#if syncUiEnabled}
  <InvitationsInbox
    open={inboxOpen}
    onClose={() => (inboxOpen = false)}
    onLogout={signOut}
  />
{/if}
