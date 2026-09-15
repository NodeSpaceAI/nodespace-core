<!--
  Pro re-login slot.

  Registry wrapper that owns the re-login orchestration the app shell used to hold,
  and mounts the unchanged, presentational `ProReloginModal`. Contributed to the
  `app-shell-modal` slot for the sync-enabled variants (`sign-in` / `connected`);
  the modal itself only becomes visible on an AUTH_REQUIRED transition.

  The daemon emits AUTH_REQUIRED when a persisted refresh token can't be renewed
  (T18). Dismissal is tracked per-episode by comparing `proSync.authRequiredEpisode`
  (bumped each time the daemon re-enters auth-required) against the last-dismissed
  episode, so the modal re-arms automatically with no `$effect` (ADR-049). Both the
  current and last-dismissed episode live on `proSync`, so a dismissal survives this
  slot remounting when the variant flips between `sign-in` and `connected`.
-->
<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import ProReloginModal from '$lib/components/pro-relogin-modal.svelte';
  import { proSync } from '$lib/stores/pro-sync.svelte';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('ProReloginSlot');

  let reloginPending = $state(false);

  const showReloginModal = $derived(
    proSync.isPro &&
      proSync.state === 'auth-required' &&
      proSync.dismissedReloginEpisode !== proSync.authRequiredEpisode
  );

  async function handleReloginSignIn() {
    if (reloginPending) return;
    reloginPending = true;
    try {
      // Same PKCE flow Settings > Database's "Add synced database…" dialog uses;
      // the daemon opens the browser and the modal closes as `sync:status`
      // transitions away from auth-required.
      await invoke('pro_initiate_oauth');
    } catch (e) {
      log.warn('pro_initiate_oauth (relogin) failed', { error: e });
    } finally {
      reloginPending = false;
    }
  }

  function handleReloginWorkOffline() {
    proSync.dismissedReloginEpisode = proSync.authRequiredEpisode;
  }
</script>

<ProReloginModal
  open={showReloginModal}
  detail={proSync.detail}
  pending={reloginPending}
  onSignIn={handleReloginSignIn}
  onWorkOffline={handleReloginWorkOffline}
/>
