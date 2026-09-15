<!--
  Collaboration locked placeholder.

  Shown in the collection viewer's Collaboration tab for the `sign-in` and
  `consent` variants — a Pro daemon whose active database has `sync_enabled:
  false`. People & roles live in the cloud tenant a database is bound to, so there
  is nothing to show until sync is enabled. Purely static: no membership calls, no
  daemon commands. Accepts the host collection's `nodeId` for a uniform
  viewer-extension signature (surfaced as a data attribute for debugging).

  The action depends on where the user is in the sign-in-first flow: once signed
  in (`consent`) the button reopens the publish-consent modal (mounted for that
  variant) — also the reopen path for a user who declined it on first sign-in,
  now that the top-right enable-sync pill that used to serve that purpose is
  gone; before sign-in (`sign-in`) there is no consent slot to open, so it
  points the user at Settings → Database to sign in first.
-->
<script lang="ts">
  import { proSync } from '$lib/stores/pro-sync.svelte';
  import { resolveProSyncVariant } from '$lib/plugins/ui-extensions.svelte';

  let { nodeId }: { nodeId: string } = $props();

  // Signed in but not yet opted into sync ⇒ the consent modal is available here.
  const canConsent = $derived(resolveProSyncVariant() === 'consent');

  function openConsent() {
    proSync.consentPromptOpen = true;
  }
</script>

<div class="collab-locked" data-collection-id={nodeId}>
  <div class="lock" aria-hidden="true">🔒</div>
  <p class="headline">Collaboration is off for this database</p>
  {#if canConsent}
    <p class="detail">
      Turn on sync for this database to invite people, manage roles, and share
      collections.
    </p>
    <button class="enable-btn" type="button" onclick={openConsent}> Turn on sync </button>
  {:else}
    <p class="detail">
      Sign in from Settings → Database ("Add synced database…"), then turn on
      sync to invite people, manage roles, and share collections.
    </p>
  {/if}
</div>

<style>
  .collab-locked {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 48px 24px;
    text-align: center;
    color: hsl(var(--muted-foreground));
  }

  .lock {
    font-size: 28px;
    opacity: 0.8;
  }

  .headline {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .detail {
    margin: 0;
    max-width: 360px;
    font-size: 13px;
    line-height: 1.5;
  }

  .enable-btn {
    margin-top: 8px;
    padding: 8px 16px;
    border: none;
    border-radius: 6px;
    background-color: #2563eb;
    color: #ffffff;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
  }

  .enable-btn:hover {
    background-color: #1d4ed8;
  }
</style>
