<!--
  Account settings.

  Owns the two account-access affordances that used to be reachable ONLY
  through the now-removed top-right `pro-sync-pill` overlay: signing out, and
  opening the Invitations inbox manually. Deliberately minimal — this exists
  to not regress those two capabilities, not to redesign account management.

  Sign-in itself is NOT duplicated here: it already has a full flow in
  Settings → Database → "Add synced database…" (`add-synced-database-dialog.svelte`,
  independent of this store's per-database getters — see that file's doc
  comment). A signed-out user is pointed there via `onNavigateToDatabase`.

  Not reproduced here: the removed pill's auto-show-Invitations-for-a-
  signed-in-user-with-no-collection-access nudge. That onboarding cue was
  tied to the pill being always-mounted in the app chrome; it doesn't map
  onto a settings page a user only visits deliberately, and it isn't one of
  the two pill-only capabilities this section exists to preserve.
-->
<script lang="ts">
  import { proSync } from '$lib/stores/pro-sync.svelte';
  import { membership } from '$lib/stores/membership.svelte';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import { Card, CardHeader, CardContent } from '$lib/components/ui/card';
  import InvitationsInbox from '$lib/components/collaboration/invitations-inbox.svelte';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('AccountSettings');

  interface Props {
    /** Switches the Settings pane to the Database category, when provided. */
    onNavigateToDatabase?: () => void;
  }
  let { onNavigateToDatabase }: Props = $props();

  // Same signed-in signal the removed pill used: a live daemon identity,
  // independent of whether sync is enabled for the active database.
  const signedIn = $derived(proSync.userEmail !== '');

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
        {#if !proSync.isPro}
          <Badge variant="secondary">Not available</Badge>
        {:else if signedIn}
          <Badge class="border-green-500/25 bg-green-500/10 text-green-700">Signed in</Badge>
        {:else}
          <Badge variant="secondary">Signed out</Badge>
        {/if}
      </div>
      {#if !proSync.isPro}
        <p class="text-muted-foreground m-0 text-sm leading-relaxed">
          This build doesn't include NodeSpace Pro sync.
        </p>
      {:else if signedIn}
        <p class="text-muted-foreground m-0 text-sm leading-relaxed">
          Signed in as <span class="text-foreground font-medium">{proSync.userEmail}</span>.
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
    {#if proSync.isPro && signedIn}
      <CardContent class="flex gap-2 px-5 pb-5">
        <Button variant="outline" size="sm" onclick={() => (inboxOpen = true)}>Invitations</Button>
        <Button variant="outline" size="sm" disabled={signingOut} onclick={signOut}>
          {signingOut ? 'Signing out…' : 'Sign out'}
        </Button>
      </CardContent>
    {/if}
  </Card>
</div>

{#if proSync.isPro}
  <InvitationsInbox
    open={inboxOpen}
    onClose={() => (inboxOpen = false)}
    onLogout={signOut}
  />
{/if}
