<!--
  First-Pro consent slot.

  Registry wrapper contributed to the `app-shell-modal` slot for the `consent`
  variant — a Pro daemon whose active database is signed in but has not yet opted
  into sync. Owns the enable-sync action and mounts the presentational
  `FirstProConsentModal`.

  Sign-in has already happened by the time this slot mounts (the sign-in-first
  flow), so the modal only decides the public-workspace publish. It auto-opens
  once per fresh sign-in episode; a "Keep local" decline records that episode so
  it doesn't immediately reopen, while `collaboration-locked.svelte`'s
  "Turn on sync" button (rendered for the `consent` variant) can reopen it. A
  brief status line confirms the decline registered.

  Never rendered in the community build (that resolves to `teaser`), so invoking
  Pro commands here is safe.
-->
<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import FirstProConsentModal from '$lib/components/first-pro-consent-modal.svelte';
  import { proSync } from '$lib/stores/pro-sync.svelte';
  import { databaseStore } from '$lib/stores/database.svelte';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('FirstProConsentSlot');

  const KEPT_LOCAL_NOTICE_MS = 4000;
  // Per-database "declined the publish consent" marker, so a Keep-local choice
  // survives a webview reload / app restart instead of re-popping this irreversible
  // dialog every launch. Keyed by database id — the decision is per-database.
  const DECLINE_KEY_PREFIX = 'ns:consent-declined:';

  function declineKeyFor(id: string | null): string | null {
    return id ? DECLINE_KEY_PREFIX + id : null;
  }

  function hasPersistedDecline(id: string | null): boolean {
    const key = declineKeyFor(id);
    if (!key) return false;
    try {
      return typeof localStorage !== 'undefined' && localStorage.getItem(key) === '1';
    } catch {
      return false; // storage unavailable — fall back to showing the consent
    }
  }

  function persistDecline(id: string | null): void {
    const key = declineKeyFor(id);
    if (!key) return;
    try {
      if (typeof localStorage !== 'undefined') localStorage.setItem(key, '1');
    } catch {
      /* ignore — an unpersisted decline still holds for this session */
    }
  }

  let pending = $state(false);
  let showKeptLocalNotice = $state(false);
  let noticeTimer: ReturnType<typeof setTimeout> | null = null;

  // Auto-open once per fresh sign-in episode (unless declined), plus any manual
  // reopen via `collaboration-locked.svelte`'s "Turn on sync" button. Derived
  // from the episode counter rather than pushed by an effect (ADR-049). A
  // decline is honoured two ways: in-session via `consentDeclinedEpisode`, and
  // across reloads via a persisted per-database marker — so the modal is a
  // one-time nudge, not a per-launch pop-up. `consentPromptOpen` remains
  // available for any surface to reopen it deliberately.
  const autoOpen = $derived(
    proSync.signedInEpisode > 0 &&
      proSync.signedInEpisode !== proSync.consentDeclinedEpisode &&
      !hasPersistedDecline(databaseStore.activeDatabaseId)
  );
  const open = $derived(proSync.isPro && (proSync.consentPromptOpen || autoOpen));

  async function handleMerge() {
    if (pending) return;
    pending = true;
    try {
      // The user is already signed in (sign-in precedes this consent), so this only
      // records the publish consent — it flips `sync_enabled` for this database, and
      // the daemon's push sweep, gated on that flag, uploads the graph. Nothing has
      // left the device before this point.
      await invoke('pro_enable_sync');
    } catch (e) {
      log.warn('first-pro consent: enabling sync failed', { error: e });
    } finally {
      pending = false;
      proSync.consentPromptOpen = false;
      // Enabling flips the variant to `connected`, unmounting this slot. Re-pull
      // the settings node directly so that flip doesn't depend on the
      // `node:updated` watch event, which can be lost for good.
      databaseStore.refreshDatabaseSettings();
    }
  }

  function handleKeepLocal() {
    // Decline: leave sync disabled and share nothing, but stay signed in. Record the
    // decline for this sign-in episode (so the auto-open doesn't immediately reopen)
    // and persist it per-database (so a reload/restart doesn't re-pop the dialog), and
    // surface a brief confirmation so the choice visibly registers.
    // `collaboration-locked.svelte`'s "Turn on sync" button remains, so the
    // user can revisit this later from that collection's Collaboration tab.
    proSync.consentPromptOpen = false;
    proSync.consentDeclinedEpisode = proSync.signedInEpisode;
    persistDecline(databaseStore.activeDatabaseId);
    showKeptLocalNotice = true;
    if (noticeTimer) clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => {
      showKeptLocalNotice = false;
      noticeTimer = null;
    }, KEPT_LOCAL_NOTICE_MS);
  }

  // Clear a pending notice timer if the slot unmounts (e.g. the user enables sync).
  $effect(() => () => {
    if (noticeTimer) clearTimeout(noticeTimer);
  });
</script>

<FirstProConsentModal {open} {pending} onMerge={handleMerge} onKeepLocal={handleKeepLocal} />

{#if showKeptLocalNotice}
  <div class="kept-local-notice" role="status" aria-live="polite">Kept local — sync stays off</div>
{/if}

<style>
  .kept-local-notice {
    position: fixed;
    bottom: 20px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 1000;
    padding: 8px 16px;
    border-radius: 8px;
    background: hsl(var(--popover));
    color: hsl(var(--popover-foreground));
    border: 1px solid hsl(var(--border));
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.18);
    font-size: 13px;
    font-weight: 500;
  }
</style>
